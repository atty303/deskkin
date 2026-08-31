#!/usr/bin/env python3
"""Build and control the bounded Phase 3P CoreS3 application."""

from __future__ import annotations

import argparse
import fcntl
import getpass
import ipaddress
import json
import os
import select
import secrets
import stat
import subprocess
import sys
import termios
import time
import uuid
from pathlib import Path

SCHEMA = 1
PORT = 39042
FRAME_MAX = 188
DEFAULT_PROFILE = Path(".deskkin/phase3-device/wifi.age")
DEFAULT_IDENTITY = Path("~/.config/chezmoi/age/identity.txt")
BOOT_COMPLETE_STAGE = 9
COMMANDS = {
    "identity-init": 1,
    "identity-list": 2,
    "identity-unpair": 3,
    "wifi-provision": 4,
    "wifi-status": 5,
    "wifi-clear": 6,
    "run": 7,
    "status": 8,
    "shutdown": 9,
    "pet-benchmark-start": 10,
    "pet-benchmark-status": 11,
}

PET_BENCHMARK_DURATION_SECONDS = 60.0
PET_BENCHMARK_REQUESTS = 1_200
AMP_GATE_TIMEOUT_SECONDS = 15.0

BOOT_ERRORS = {
    1: "boot_devices_unavailable",
    2: "boot_noise_resolver",
    3: "boot_service_worker",
    4: "boot_ui_platform",
    5: "boot_ui_component",
    6: "boot_framebuffer",
    7: "boot_display_transfer",
    8: "boot_display_enable",
}

OPERATION_STAGES = (
    "idle",
    "wifi_association",
    "dhcp",
    "tcp_connect",
    "noise",
    "bootstrap",
    "availability_read",
    "view_application",
    "display_transfer",
)

OPERATION_ERRORS = {
    0: None,
    1: "identity_store",
    2: "wifi_association",
    3: "dhcp_timeout",
    5: "noise",
    6: "noise",
    7: "pairing_rejected",
    8: "pairing_busy",
    9: "pairing_expired",
    10: "protocol_incompatible",
    11: "authorization_denied",
}


class DeviceError(Exception):
    pass


def ensure_private_directory(path: Path) -> None:
    path.mkdir(mode=0o700, parents=True, exist_ok=True)
    metadata = path.lstat()
    if stat.S_ISLNK(metadata.st_mode) or not stat.S_ISDIR(metadata.st_mode):
        raise DeviceError("profile_directory_not_private")
    os.chmod(path, 0o700)
    if stat.S_IMODE(path.stat().st_mode) != 0o700:
        raise DeviceError("profile_directory_not_private")


def zeroize(value: bytearray) -> None:
    value[:] = b"\0" * len(value)


def validate_profile(value: object) -> dict[str, object]:
    if not isinstance(value, dict) or set(value) != {"schema_version", "ssid", "password", "host_ipv4"}:
        raise DeviceError("profile_schema_invalid")
    if value["schema_version"] != SCHEMA or not all(isinstance(value[key], str) for key in ("ssid", "password", "host_ipv4")):
        raise DeviceError("profile_schema_invalid")
    ssid = value["ssid"].encode("utf-8")
    password = value["password"].encode("ascii", errors="strict")
    try:
        address = ipaddress.ip_address(value["host_ipv4"])
    except ValueError as error:
        raise DeviceError("profile_schema_invalid") from error
    if not 1 <= len(ssid) <= 32 or not 8 <= len(password) <= 63:
        raise DeviceError("profile_schema_invalid")
    if any(byte < 0x20 or byte > 0x7E for byte in password):
        raise DeviceError("profile_schema_invalid")
    if not isinstance(address, ipaddress.IPv4Address) or not address.is_private or address.is_link_local or address.is_loopback:
        raise DeviceError("profile_schema_invalid")
    first, second = address.packed[:2]
    if not (first == 10 or first == 172 and 16 <= second <= 31 or first == 192 and second == 168):
        raise DeviceError("profile_schema_invalid")
    return value


def prompt_profile() -> dict[str, object]:
    value = {
        "schema_version": SCHEMA,
        "ssid": getpass.getpass("SSID: "),
        "password": getpass.getpass("WPA2 password: "),
        "host_ipv4": getpass.getpass("Host RFC1918 IPv4: "),
    }
    return validate_profile(value)


def age_recipient(identity: Path) -> str:
    process = subprocess.run(
        ["age-keygen", "-y", str(identity)],
        check=False,
        stdout=subprocess.PIPE,
        stderr=subprocess.DEVNULL,
        text=True,
    )
    recipient = process.stdout.strip()
    if process.returncode != 0 or not recipient.startswith("age1"):
        raise DeviceError("profile_encrypt_failed")
    return recipient


def create_profile(profile: Path, identity: Path, value: dict[str, object] | None = None) -> None:
    ensure_private_directory(profile.parent)
    data = bytearray(json.dumps(validate_profile(value or prompt_profile()), separators=(",", ":")).encode())
    temporary = profile.with_name(f".{profile.name}.{secrets.token_hex(8)}.tmp")
    descriptor = os.open(temporary, os.O_CREAT | os.O_EXCL | os.O_WRONLY, 0o600)
    try:
        recipient = age_recipient(identity)
        process = subprocess.run(
            ["age", "-r", recipient],
            input=data,
            check=False,
            stdout=subprocess.PIPE,
            stderr=subprocess.DEVNULL,
        )
        if process.returncode != 0:
            raise DeviceError("profile_encrypt_failed")
        os.write(descriptor, process.stdout)
        os.fsync(descriptor)
        os.close(descriptor)
        descriptor = -1
        os.replace(temporary, profile)
        directory = os.open(profile.parent, os.O_RDONLY | os.O_DIRECTORY)
        try:
            os.fsync(directory)
        finally:
            os.close(directory)
    finally:
        zeroize(data)
        if descriptor >= 0:
            os.close(descriptor)
        try:
            temporary.unlink()
        except FileNotFoundError:
            pass


def mutable_subprocess_output(command: list[str], maximum: int, *, input_data: bytearray | None = None, cwd: Path | None = None) -> tuple[int, bytearray]:
    process = subprocess.Popen(
        command,
        stdin=subprocess.PIPE if input_data is not None else subprocess.DEVNULL,
        stdout=subprocess.PIPE,
        stderr=subprocess.DEVNULL,
        cwd=cwd,
    )
    if input_data is not None:
        assert process.stdin is not None
        process.stdin.write(input_data)
        process.stdin.close()
    assert process.stdout is not None
    output = bytearray(maximum + 1)
    length = process.stdout.readinto(output)
    process.stdout.close()
    returncode = process.wait()
    del output[length:]
    return returncode, output


def decrypt_profile_raw(profile: Path, identity: Path) -> bytearray:
    returncode, output = mutable_subprocess_output(
        ["age", "-d", "-i", str(identity), str(profile)], 4096
    )
    if returncode != 0 or len(output) > 4096:
        zeroize(output)
        raise DeviceError("profile_decrypt_failed")
    return output


def decrypt_profile(profile: Path, identity: Path) -> bytearray:
    plaintext = decrypt_profile_raw(profile, identity)
    try:
        value = json.loads(plaintext)
        validate_profile(value)
    except (UnicodeDecodeError, json.JSONDecodeError, DeviceError, ValueError) as error:
        zeroize(plaintext)
        raise DeviceError("profile_schema_invalid") from error
    return plaintext


def wifi_payload(value: dict[str, object]) -> bytearray:
    validated = validate_profile(value)
    ssid = bytearray(validated["ssid"], "utf-8")
    password = bytearray(validated["password"], "utf-8")
    address = ipaddress.IPv4Address(validated["host_ipv4"]).packed
    try:
        return bytearray([len(ssid)]) + ssid + bytearray([len(password)]) + password + bytearray(address) + bytearray(PORT.to_bytes(2, "big"))
    finally:
        zeroize(ssid)
        zeroize(password)


def profile_payload_from_json(root: Path, plaintext: bytearray) -> bytearray:
    returncode, payload = mutable_subprocess_output(
        ["cargo", "run", "--locked", "--quiet", "-p", "deskkin-desktop-host", "--bin", "device_profile"],
        160,
        input_data=plaintext,
        cwd=root,
    )
    if returncode != 0 or len(payload) > 160:
        zeroize(payload)
        raise DeviceError("profile_schema_invalid")
    return payload


def control_frame(command: str, owner_generation: int, payload: bytearray | bytes = b"") -> bytearray:
    if command not in COMMANDS or len(payload) > 160:
        raise DeviceError("control_invalid")
    command_id = secrets.token_bytes(16)
    body = bytearray([SCHEMA, COMMANDS[command]]) + bytearray(command_id)
    body.extend(owner_generation.to_bytes(8, "big"))
    body.extend(len(payload).to_bytes(2, "big"))
    body.extend(payload)
    return bytearray(len(body).to_bytes(2, "big")) + body


def discover_device(requested: str | None) -> Path:
    if requested:
        candidates = [Path(requested)]
    else:
        root = Path("/dev/serial/by-id")
        candidates = sorted(root.glob("usb-Espressif_USB_JTAG_serial_debug_unit_*-if00")) if root.is_dir() else []
    if len(candidates) != 1:
        raise DeviceError("physical_device_required" if not candidates else "device_selection_ambiguous")
    resolved = candidates[0].resolve(strict=True)
    if not stat.S_ISCHR(resolved.stat().st_mode):
        raise DeviceError("device_not_recognized")
    return resolved


def write_all(descriptor: int, value: bytes | bytearray, timeout: float) -> None:
    deadline = time.monotonic() + timeout
    offset = 0
    while offset < len(value):
        remaining = deadline - time.monotonic()
        if remaining <= 0 or not select.select([], [descriptor], [], remaining)[1]:
            raise DeviceError("control_timeout")
        try:
            written = os.write(descriptor, value[offset:])
        except BlockingIOError:
            continue
        if written <= 0:
            raise DeviceError("control_timeout")
        offset += written


def read_exact(descriptor: int, length: int, timeout: float) -> bytes:
    deadline = time.monotonic() + timeout
    value = bytearray()
    while len(value) < length:
        remaining = deadline - time.monotonic()
        if remaining <= 0 or not select.select([descriptor], [], [], remaining)[0]:
            raise DeviceError("control_timeout")
        try:
            chunk = os.read(descriptor, length - len(value))
        except BlockingIOError:
            continue
        if not chunk:
            raise DeviceError("control_timeout")
        value.extend(chunk)
    return bytes(value)


def read_control_response(descriptor: int, frame: bytearray, timeout: float) -> bytes:
    deadline = time.monotonic() + timeout
    buffered = bytearray()
    while len(buffered) <= 4096:
        candidate_start: int | None = None
        for start in range(max(0, len(buffered) - 1)):
            length = int.from_bytes(buffered[start : start + 2], "big")
            if not 18 <= length <= 80 or start + 3 > len(buffered):
                continue
            if buffered[start + 2] != SCHEMA:
                continue
            available_id = min(16, max(0, len(buffered) - (start + 4)))
            if buffered[start + 4 : start + 4 + available_id] != frame[4 : 4 + available_id]:
                continue
            candidate_start = start
            if available_id < 16 or len(buffered) < start + length + 2:
                break
            result = bytes(buffered[start + 2 : start + length + 2])
            if result[2:18] == frame[4:20]:
                return result
        if candidate_start is not None:
            if candidate_start > 0:
                del buffered[:candidate_start]
        elif len(buffered) > 19:
            del buffered[:-19]
        remaining = deadline - time.monotonic()
        if remaining <= 0 or not select.select([descriptor], [], [], remaining)[0]:
            raise DeviceError("control_timeout")
        try:
            chunk = os.read(descriptor, min(512, 4097 - len(buffered)))
        except BlockingIOError:
            continue
        if not chunk:
            raise DeviceError("control_timeout")
        buffered.extend(chunk)
    raise DeviceError("control_response_length")


def exchange_once(device: Path, frame: bytearray, startup_delay: float, response_timeout: float) -> bytes:
    descriptor = os.open(device, os.O_RDWR | os.O_NOCTTY | os.O_NONBLOCK)
    try:
        attributes = termios.tcgetattr(descriptor)
        attributes[0] = attributes[1] = attributes[3] = 0
        attributes[2] = termios.CS8 | termios.CREAD | termios.CLOCAL
        attributes[4] = termios.B115200
        attributes[5] = termios.B115200
        attributes[6][termios.VMIN] = 0
        attributes[6][termios.VTIME] = 0
        termios.tcsetattr(descriptor, termios.TCSANOW, attributes)
        time.sleep(startup_delay)
        termios.tcflush(descriptor, termios.TCIOFLUSH)
        write_all(descriptor, frame, 2.0)
        return read_control_response(descriptor, frame, response_timeout)
    finally:
        # USB Serial/JTAG can retain unsent host output when device RX is not
        # draining.  Discard it so closing a diagnostic/control attempt cannot
        # block behind the tty driver's drain timeout.
        try:
            termios.tcflush(descriptor, termios.TCIOFLUSH)
        except termios.error:
            pass
        os.close(descriptor)


def exchange(device: Path, frame: bytearray, recover_status_transport: bool = True) -> bytes:
    if frame[3] in {COMMANDS["status"], COMMANDS["pet-benchmark-status"]} and not recover_status_transport:
        return exchange_once(device, frame, 0.0, 2.0)
    if frame[3] not in {COMMANDS["status"], COMMANDS["pet-benchmark-status"]}:
        return exchange_once(device, frame, 0.25, 2.0)
    # A flash/reset can leave USB Serial/JTAG visible before its device-side RX
    # service is ready.  Send immediately first, then reopen the tty for a
    # bounded recovery.  Continuous monitoring disables this recovery so its
    # 250 ms sampling cadence is not hidden behind readiness sleeps.
    startup_delays = (0.25, 3.0, 5.0, 5.0)
    for attempt, startup_delay in enumerate(startup_delays):
        try:
            return exchange_once(device, frame, startup_delay, 5.0 if attempt else 2.0)
        except DeviceError as error:
            if attempt == len(startup_delays) - 1 or str(error) != "control_timeout":
                raise
    raise DeviceError("control_timeout")


def status_boot_error(status: bytes) -> str | None:
    if len(status) < 80 or status[79] == 0:
        return None
    return BOOT_ERRORS.get(status[79], "boot_unknown")


def report_status(status: bytes) -> None:
    if len(status) < 80:
        raise DeviceError("control_invalid")
    print(
        json.dumps(
            {
                "shell_state": status[26] & 0x7F,
                "availability": min(status[27], 2),
                "last_stage": OPERATION_STAGES[min(status[76], len(OPERATION_STAGES) - 1)],
                "last_error": operation_error(status[76], status[77]),
                "boot_stage": status[78],
                "boot_error": status_boot_error(status),
            },
            separators=(",", ":"),
        ),
        file=sys.stderr,
    )


def await_boot_complete(status: bytes, device_arg: str | None, timeout: float = 15.0) -> bytes:
    deadline = time.monotonic() + timeout
    while True:
        if len(status) != 80:
            raise DeviceError("control_invalid")
        if status_boot_error(status) is not None or status[78] == BOOT_COMPLETE_STAGE:
            return status
        if status[78] > BOOT_COMPLETE_STAGE:
            raise DeviceError("boot_unknown")
        if time.monotonic() >= deadline:
            raise DeviceError("boot_not_ready")
        time.sleep(0.25)
        status = run_control("status", device_arg, recover_status_transport=False)


def operation_error(stage: int, error: int) -> str | None:
    if error == 4:
        return "tcp_connect" if stage <= 3 else "availability_timeout"
    return OPERATION_ERRORS.get(error, "noise")


def run_control(
    command: str,
    device_arg: str | None,
    payload: bytearray | bytes = b"",
    recover_status_transport: bool = True,
) -> bytes:
    device = discover_device(device_arg)
    generation = 0
    if command in {"identity-init", "identity-unpair", "wifi-provision", "wifi-clear", "run", "shutdown", "pet-benchmark-start"}:
        status_frame = control_frame("status", 0)
        try:
            status = exchange(device, status_frame, recover_status_transport)
            if len(status) != 80:
                raise DeviceError("control_invalid")
            if status[1] != 0:
                raise DeviceError("device_rejected")
            if error := status_boot_error(status):
                raise DeviceError(error)
            generation = int.from_bytes(status[18:26], "big")
        finally:
            zeroize(status_frame)
    frame = control_frame(command, generation, payload)
    try:
        result = exchange(device, frame, recover_status_transport)
        if result[1] != 0:
            raise DeviceError("device_rejected")
        if command == "identity-list":
            state = result[26] if len(result) >= 27 else 0
            peer = result[27:59].hex() if len(result) == 59 else None
            print(json.dumps({"state": state, "peer_id": peer}, separators=(",", ":")), file=sys.stderr)
        return result
    finally:
        zeroize(frame)


def device_environment(root: Path) -> tuple[Path, dict[str, str]]:
    state = root / ".deskkin"
    environment = os.environ.copy()
    toolchain = state / "rustup/toolchains/deskkin-esp"
    clang = toolchain / "xtensa-esp32-elf-clang/esp-20.1.1_20250829/esp-clang"
    environment["ZEPHYR_TOOLCHAIN_VARIANT"] = "zephyr"
    environment["ZEPHYR_SDK_INSTALL_DIR"] = str(state / "sdk")
    environment["ZEPHYR_BASE"] = str(state / "west/zephyr")
    environment["LIBCLANG_PATH"] = str(clang / "lib")
    environment["SOURCE_DATE_EPOCH"] = "0"
    environment["PATH"] = os.pathsep.join(
        (
            str(toolchain / "bin"),
            str(state / "venv/bin"),
            str(state / "sdk/sysroots/x86_64-pokysdk-linux/usr/bin"),
            environment["PATH"],
        )
    )
    return state, environment


def build(root: Path) -> None:
    state, environment = device_environment(root)
    environment["DESKKIN_STATE_DIR"] = str(state)
    subprocess.run(
        [str(root / "scripts/bootstrap_core_s3.sh"), "--verify-only"],
        check=True,
        cwd=root,
        env=environment,
        stdout=sys.stderr,
    )
    device_state = state / "phase3-device"
    ensure_private_directory(device_state)
    build_dir = device_state / "build"
    west = state / "venv/bin/west"
    subprocess.run(
        [
            str(west),
            "build",
            "--pristine",
            "always",
            "--board",
            "m5stack_cores3/esp32s3/procpu",
            "--build-dir",
            str(build_dir),
            str(root / "apps/core-s3-device"),
        ],
        check=True,
        cwd=state / "west",
        env=environment,
        stdout=sys.stderr,
    )
    subprocess.run(
        [
            str(west),
            "build",
            "--pristine",
            "always",
            "--board",
            "m5stack_cores3/esp32s3/procpu",
            "--build-dir",
            str(device_state / "inert-build"),
            str(root / "apps/core-s3-inert"),
        ],
        check=True,
        cwd=state / "west",
        env=environment,
        stdout=sys.stderr,
    )


def build_amp(root: Path) -> None:
    state, environment = device_environment(root)
    environment["DESKKIN_STATE_DIR"] = str(state)
    subprocess.run(
        [str(root / "scripts/bootstrap_core_s3.sh"), "--verify-only"],
        check=True,
        cwd=root,
        env=environment,
        stdout=sys.stderr,
    )
    device_state = state / "phase3-device"
    ensure_private_directory(device_state)
    subprocess.run(
        [
            str(state / "venv/bin/west"),
            "build",
            "--sysbuild",
            "--pristine",
            "always",
            "--board",
            "m5stack_cores3/esp32s3/procpu",
            "--build-dir",
            str(device_state / "amp-build"),
            str(root / "apps/core-s3-amp"),
        ],
        check=True,
        cwd=state / "west",
        env=environment,
        stdout=sys.stderr,
    )


def flash_amp(root: Path, device_arg: str | None) -> None:
    device = discover_device(device_arg)
    state, environment = device_environment(root)
    build_dir = state / "phase3-device/amp-build"
    if not (build_dir / "domains.yaml").is_file():
        raise DeviceError("firmware_build_required")
    west = state / "venv/bin/west"
    subprocess.run(
        [
            str(west),
            "flash",
            "--no-rebuild",
            "--build-dir",
            str(build_dir),
            "--runner",
            "esp32",
            "--",
            "--esp-device",
            str(device),
            "--esp-baud-rate",
            "460800",
        ],
        check=True,
        cwd=state / "west",
        env=environment,
        stdout=sys.stderr,
    )


def reset_amp(root: Path, device_arg: str | None) -> None:
    device = discover_device(device_arg)
    state, environment = device_environment(root)
    subprocess.run(
        [str(state / "venv/bin/esptool"), "--port", str(device), "run"],
        check=True,
        env=environment,
        stdout=sys.stderr,
    )


def amp_fault_isolation_gate(device_arg: str | None) -> dict[str, object]:
    gate_started = time.monotonic()
    deadline = gate_started + AMP_GATE_TIMEOUT_SECONDS
    responses = 0
    live_generation: int | None = None
    last_generation = 0
    max_response_ms = 0
    first_request = True
    while time.monotonic() < deadline:
        started = time.monotonic()
        try:
            status = run_control("status", device_arg, recover_status_transport=first_request)
        except DeviceError:
            if live_generation is not None:
                raise DeviceError("amp_supervisor_unresponsive")
            first_request = False
            continue
        first_request = False
        response_ms = int((time.monotonic() - started) * 1000)
        max_response_ms = max(max_response_ms, response_ms)
        responses += 1
        generation = int.from_bytes(status[28:32], "big")
        availability = status[27]
        if availability == 1 and generation > 0:
            live_generation = generation
            last_generation = generation
        elif live_generation is not None and availability == 2:
            if generation != last_generation:
                raise DeviceError("amp_generation_changed_after_fault")
            summary: dict[str, object] = {
                "operation": "amp_fault_isolation",
                "operation_id": 1,
                "parent_operation_id": None,
                "status": "success",
                "error_type": None,
                "effect_id": None,
                "virtual_time_ms": 0,
                "end_virtual_time_ms": int(AMP_GATE_TIMEOUT_SECONDS * 1000),
                "duration_ms": int((time.monotonic() - gate_started) * 1000),
                "render_width": None,
                "render_height": None,
                "value": "pass",
                "status_responses": responses,
                "live_generation": live_generation,
                "stalled_generation": generation,
                "max_status_response_ms": max_response_ms,
            }
            print(json.dumps(summary, separators=(",", ":")), file=sys.stderr)
            return summary
        time.sleep(0.25)
    if live_generation is None:
        raise DeviceError("amp_heartbeat_not_observed")
    raise DeviceError("amp_fault_not_observed")


def flash(root: Path, device_arg: str | None) -> None:
    device = discover_device(device_arg)
    state, environment = device_environment(root)
    build_dir = state / "phase3-device/build"
    if not (build_dir / "zephyr/zephyr.elf").is_file():
        raise DeviceError("firmware_build_required")
    subprocess.run(
        [
            str(state / "venv/bin/west"),
            "flash",
            "--no-rebuild",
            "--build-dir",
            str(build_dir),
            "--runner",
            "esp32",
            "--",
            "--esp-device",
            str(device),
            "--esp-baud-rate",
            "460800",
        ],
        check=True,
        cwd=state / "west",
        env=environment,
        stdout=sys.stderr,
    )


def recover(root: Path, device_arg: str | None) -> None:
    device = discover_device(device_arg)
    state, environment = device_environment(root)
    build_dir = state / "phase3-device/inert-build"
    if not (build_dir / "zephyr/zephyr.elf").is_file():
        raise DeviceError("inert_firmware_build_required")
    esptool = state / "venv/bin/esptool"
    subprocess.run(
        [str(esptool), "--port", str(device), "erase-region", "0x3b0000", "0x30000"],
        check=True,
        env=environment,
        stdout=sys.stderr,
    )
    readback = state / f"phase3-device/.storage-readback-{secrets.token_hex(8)}.bin"
    try:
        subprocess.run(
            [str(esptool), "--port", str(device), "read-flash", "0x3b0000", "0x30000", str(readback)],
            check=True,
            env=environment,
            stdout=sys.stderr,
        )
        if readback.stat().st_size != 0x30000 or any(byte != 0xFF for byte in readback.read_bytes()):
            raise DeviceError("storage_erase_readback_failed")
    finally:
        try:
            readback.unlink()
        except FileNotFoundError:
            pass
    subprocess.run(
        [
            str(state / "venv/bin/west"),
            "flash",
            "--no-rebuild",
            "--build-dir",
            str(build_dir),
            "--runner",
            "esp32",
            "--",
            "--esp-device",
            str(device),
            "--esp-baud-rate",
            "460800",
        ],
        check=True,
        cwd=state / "west",
        env=environment,
        stdout=sys.stderr,
    )


def publish_result(root: Path, action: str, result: str, run_id: str) -> Path:
    directory = root / ".deskkin/phase3-device/results"
    directory.mkdir(mode=0o700, parents=True, exist_ok=True)
    os.chmod(directory, 0o700)
    path = directory / f"{action}.json"
    temporary = directory / f".{action}.{secrets.token_hex(8)}.tmp"
    value = {
        "schema_version": 1,
        "result": result,
        "run_id": run_id,
        "completed_unix_ms": int(time.time() * 1000),
    }
    descriptor = os.open(temporary, os.O_CREAT | os.O_EXCL | os.O_WRONLY, 0o600)
    try:
        os.write(descriptor, json.dumps(value, separators=(",", ":")).encode())
        os.fsync(descriptor)
    finally:
        os.close(descriptor)
    os.replace(temporary, path)
    directory_descriptor = os.open(directory, os.O_RDONLY | os.O_DIRECTORY)
    try:
        os.fsync(directory_descriptor)
    finally:
        os.close(directory_descriptor)
    return path


def publish_diagnostic(root: Path, run_id: str, outcome: str, records: list[dict[str, object]]) -> None:
    directory = root / ".deskkin/phase3/device/diagnostics"
    for ancestor in (root / ".deskkin", root / ".deskkin/phase3", root / ".deskkin/phase3/device", directory):
        if ancestor.is_symlink():
            raise OSError("diagnostic_symlink")
    directory.mkdir(mode=0o700, parents=True, exist_ok=True)
    os.chmod(directory, 0o700)
    lock_path = directory.parent / ".diagnostics.lock"
    lock_descriptor = os.open(lock_path, os.O_CREAT | os.O_RDWR, 0o600)
    fcntl.flock(lock_descriptor, fcntl.LOCK_EX)
    created = int(time.time() * 1000)
    latest = records[-1] if records else {}
    session_context = latest.get("session_context_id")
    operation_context = latest.get("operation_context_id")
    value = {
        "schema_version": 1,
        "resource": {"program": "deskkin-core-s3-runner", "version": "0.1.0", "role": "physical_device"},
        "run_id": run_id,
        "scenario_run_id": run_id,
        "session_context_id": session_context if session_context and session_context != "00" * 16 else None,
        "operation_context_id": operation_context if operation_context and operation_context != "00" * 16 else None,
        "outcome": outcome,
        "completeness": "complete",
        "health": "healthy",
        "terminal": True,
        "missing_reason": None,
        "owner": None,
        "retained": False,
        "created_unix_ms": created,
        "records": records,
    }
    path = directory / f"{run_id}.json"
    temporary = directory / f".{run_id}.tmp"
    try:
        descriptor = os.open(temporary, os.O_CREAT | os.O_EXCL | os.O_WRONLY, 0o600)
        try:
            os.write(descriptor, json.dumps(value, separators=(",", ":")).encode())
            os.fsync(descriptor)
        finally:
            os.close(descriptor)
        os.replace(temporary, path)
        entries = []
        for candidate in directory.glob("*.json"):
            try:
                item = json.loads(candidate.read_bytes())
                entries.append((candidate, item.get("outcome") == "success", bool(item.get("retained")), candidate.stat().st_mtime_ns))
            except (OSError, json.JSONDecodeError):
                continue
        for success, limit in ((True, 10), (False, 20)):
            selected = sorted((entry for entry in entries if entry[1] == success and not entry[2]), key=lambda entry: entry[3], reverse=True)
            for candidate, _, _, _ in selected[limit:]:
                candidate.unlink()
        retained = {entry[0] for entry in entries if entry[2]}
        total = sorted((candidate for candidate in directory.glob("*.json") if candidate not in retained), key=lambda candidate: candidate.stat().st_mtime_ns)
        size = sum(candidate.stat().st_size for candidate in total)
        while size > 16 * 1024 * 1024 and total:
            candidate = total.pop(0)
            size -= candidate.stat().st_size
            candidate.unlink()
        directory_descriptor = os.open(directory, os.O_RDONLY | os.O_DIRECTORY)
        try:
            os.fsync(directory_descriptor)
        finally:
            os.close(directory_descriptor)
    finally:
        fcntl.flock(lock_descriptor, fcntl.LOCK_UN)
        os.close(lock_descriptor)


def monitor_device(device_arg: str | None, duration_seconds: float, expected_attempt: int) -> list[dict[str, object]]:
    records: list[dict[str, object]] = []
    started = time.monotonic()
    last_elapsed = 0
    previous: tuple[int, int, int, bytes, bytes, bool, int, int, int, int, int] | None = None
    try:
        while duration_seconds <= 0 or time.monotonic() - started < duration_seconds:
            result = run_control("status", device_arg, recover_status_transport=False)
            if len(result) < 78:
                raise DeviceError("control_invalid")
            shell_and_valid = result[26]
            value = (
                shell_and_valid & 0x7F,
                result[27],
                int.from_bytes(result[28:32], "big"),
                result[32:48],
                result[48:64],
                bool(shell_and_valid & 0x80),
                int.from_bytes(result[64:68], "big"),
                int.from_bytes(result[68:72], "big"),
                int.from_bytes(result[72:76], "big"),
                result[76],
                result[77],
            )
            if value != previous:
                elapsed = int((time.monotonic() - started) * 1000)
                stage = OPERATION_STAGES[min(value[9], len(OPERATION_STAGES) - 1)]
                error_type = operation_error(value[9], value[10])
                records.append(
                    {
                        "operation": stage,
                        "operation_id": len(records) + 1,
                        "parent_operation_id": None,
                        "status": "success" if error_type is None else "error",
                        "error_type": error_type,
                        "effect_id": None,
                        "virtual_time_ms": elapsed,
                        "end_virtual_time_ms": elapsed,
                        "duration_ms": elapsed - last_elapsed,
                        "render_width": 320,
                        "render_height": 240,
                        "value": ("unknown", "available", "unavailable")[min(value[1], 2)],
                        "session_context_id": value[3].hex(),
                        "operation_context_id": value[4].hex(),
                        "rgb565_digest": f"{value[2]:08x}",
                        "shell_state": value[0],
                        "valid_availability_result": value[5],
                        "run_attempt": value[6],
                        "result_attempt": value[7],
                        "frame_attempt": value[8],
                        "stage": stage,
                    }
                )
                last_elapsed = elapsed
                previous = value
            time.sleep(0.25)
    except KeyboardInterrupt:
        pass
    return records


def run_succeeded(records: list[dict[str, object]], expected_attempt: int) -> bool:
    zero_context = "00" * 16
    return any(
        record.get("shell_state") == 4
        and record.get("valid_availability_result") is True
        and record.get("session_context_id") != zero_context
        and record.get("operation_context_id") != zero_context
        and record.get("rgb565_digest") not in {None, "00000000"}
        and record.get("run_attempt") == expected_attempt
        and record.get("result_attempt") == expected_attempt
        and record.get("frame_attempt") == expected_attempt
        for record in records
    )


def decode_pet_benchmark(status: bytes) -> dict[str, int]:
    if len(status) != 80 or status[0] != SCHEMA or status[1] != 0:
        raise DeviceError("control_invalid")

    def unsigned(start: int, end: int) -> int:
        return int.from_bytes(status[start:end], "big")

    return {
        "state": status[26],
        "allocation_failures": status[27],
        "display_transfer_failures": unsigned(28, 30),
        "duration_ms": unsigned(30, 34),
        "animation_update_requests": unsigned(34, 38),
        "completed_frames": unsigned(38, 42),
        "render_total_us": unsigned(42, 46),
        "display_transfer_total_us": unsigned(46, 50),
        "render_max_us": unsigned(50, 54),
        "display_transfer_max_us": unsigned(54, 58),
        "frames_within_50ms": unsigned(58, 62),
        "stalls_over_250ms": unsigned(62, 64),
        "deadline_misses": unsigned(64, 66),
        "max_consecutive_misses": unsigned(66, 68),
        "transferred_lines": unsigned(68, 72),
        "transferred_bytes": unsigned(72, 76),
        "frame_digest_updates": unsigned(76, 80),
    }


def pet_benchmark_passed(summary: dict[str, int]) -> bool:
    completed = summary["completed_frames"]
    return (
        summary["state"] == 2
        and 60_000 <= summary["duration_ms"] <= 60_500
        and summary["animation_update_requests"] == PET_BENCHMARK_REQUESTS
        and completed == PET_BENCHMARK_REQUESTS
        and summary["frames_within_50ms"] * 100 >= completed * 95
        and summary["stalls_over_250ms"] == 0
        and summary["allocation_failures"] == 0
        and summary["display_transfer_failures"] == 0
        and summary["frame_digest_updates"] > 0
        and summary["transferred_lines"] == completed * 240
        and summary["transferred_bytes"] == completed * 320 * 240 * 2
    )


def pet_benchmark_record(summary: dict[str, int], passed: bool) -> dict[str, object]:
    completed = summary["completed_frames"]
    return {
        "operation": "pet_render_benchmark",
        "operation_id": 1,
        "parent_operation_id": None,
        "status": "success" if passed else "error",
        "error_type": None if passed else "benchmark_gate_failed",
        "effect_id": None,
        "virtual_time_ms": 0,
        "end_virtual_time_ms": summary["duration_ms"],
        "duration_ms": summary["duration_ms"],
        "render_width": 320,
        "render_height": 240,
        "value": "pass" if passed else "fail",
        **summary,
        "render_mean_us": summary["render_total_us"] // completed if completed else None,
        "display_transfer_mean_us": summary["display_transfer_total_us"] // completed if completed else None,
    }


def await_pet_benchmark(device_arg: str | None) -> dict[str, int]:
    time.sleep(PET_BENCHMARK_DURATION_SECONDS + 0.5)
    deadline = time.monotonic() + 5.0
    while True:
        summary = decode_pet_benchmark(
            run_control("pet-benchmark-status", device_arg, recover_status_transport=False)
        )
        if summary["state"] in {2, 3}:
            return summary
        if summary["state"] != 1 or time.monotonic() >= deadline:
            raise DeviceError("benchmark_timeout")
        time.sleep(0.1)


def action_record(action: str, status: str, error_type: str | None = None) -> dict[str, object]:
    operation = {
        "profile": "control_route",
        "build": "control_route",
        "amp-build": "control_route",
        "amp-flash": "device_ui",
        "amp-gate": "amp_fault_isolation",
        "flash": "device_ui",
        "identity": "identity_init",
        "provision": "nvs_publication",
        "status": "device_ui",
        "run": "device_ui",
        "benchmark": "pet_render_benchmark",
        "recover": "nvs_publication",
    }[action]
    return {
        "operation": operation,
        "operation_id": 1,
        "parent_operation_id": None,
        "status": status,
        "error_type": error_type,
        "effect_id": None,
        "virtual_time_ms": 0,
        "end_virtual_time_ms": 0,
        "duration_ms": 0,
        "render_width": None,
        "render_height": None,
        "value": "success" if status == "success" else "error",
    }


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("action", choices=("profile", "build", "amp-build", "amp-flash", "amp-gate", "flash", "identity", "provision", "status", "run", "benchmark", "recover"))
    parser.add_argument("identity_action", nargs="?")
    parser.add_argument("--profile", type=Path)
    parser.add_argument("--age-identity", type=Path)
    parser.add_argument("--device")
    parser.add_argument("--peer-id")
    parser.add_argument("--erase-storage", action="store_true")
    parser.add_argument("--duration-seconds", type=float, default=0)
    parser.add_argument("--recording", choices=("on", "off"), default="on")
    args = parser.parse_args()
    root = Path(__file__).resolve().parents[1]
    profile = (args.profile or root / DEFAULT_PROFILE).expanduser()
    identity = (args.age_identity or DEFAULT_IDENTITY).expanduser()
    run_id = str(uuid.uuid4())
    result = "error"
    exit_code = 2
    records: list[dict[str, object]] = []
    error_type: str | None = None
    diagnostic_allowed = True
    try:
        if args.action == "profile":
            create_profile(profile, identity)
        elif args.action == "build":
            build(root)
        elif args.action == "amp-build":
            build_amp(root)
        elif args.action == "amp-flash":
            flash_amp(root, args.device)
        elif args.action == "amp-gate":
            reset_amp(root, args.device)
            records = [amp_fault_isolation_gate(args.device)]
        elif args.action == "flash":
            flash(root, args.device)
        elif args.action == "recover":
            if not args.erase_storage:
                raise DeviceError("erase_storage_confirmation_required")
            recover(root, args.device)
        elif args.action == "identity":
            action = args.identity_action
            if action not in {"init", "list", "unpair"}:
                raise DeviceError("identity_action_required")
            payload = bytearray.fromhex(args.peer_id) if action == "unpair" and args.peer_id else bytearray()
            if action == "unpair" and len(payload) != 32:
                raise DeviceError("exact_peer_id_required")
            try:
                run_control(f"identity-{action}", args.device, payload)
            finally:
                zeroize(payload)
        elif args.action == "provision":
            explicit = args.profile is not None
            plaintext: bytearray | None = None
            payload = bytearray()
            try:
                if profile.exists():
                    plaintext = decrypt_profile_raw(profile, identity)
                    payload = profile_payload_from_json(root, plaintext)
                elif explicit:
                    raise DeviceError("profile_decrypt_failed")
                else:
                    value = prompt_profile()
                    payload = wifi_payload(value)
                run_control("wifi-provision", args.device, payload)
            finally:
                zeroize(payload)
                if plaintext is not None:
                    zeroize(plaintext)
        elif args.action == "status":
            status = run_control("status", args.device)
            status = await_boot_complete(status, args.device)
            report_status(status)
            if error := status_boot_error(status):
                raise DeviceError(error)
        elif args.action == "run":
            accepted = run_control("run", args.device)
            if len(accepted) < 30:
                raise DeviceError("control_invalid")
            expected_attempt = int.from_bytes(accepted[26:30], "big")
            records = monitor_device(args.device, args.duration_seconds, expected_attempt)
            if not run_succeeded(records, expected_attempt):
                raise DeviceError("availability_timeout")
        elif args.action == "benchmark":
            run_control("shutdown", args.device)
            run_control("pet-benchmark-start", args.device)
            summary = await_pet_benchmark(args.device)
            passed = pet_benchmark_passed(summary)
            records = [pet_benchmark_record(summary, passed)]
            print(json.dumps(summary, separators=(",", ":")), file=sys.stderr)
            if not passed:
                raise DeviceError("benchmark_gate_failed")
        result = "success"
        exit_code = 0
    except (DeviceError, OSError, subprocess.SubprocessError, UnicodeError, ValueError) as error:
        message = str(error) if isinstance(error, DeviceError) else "device_operation_failed"
        boot_failure = message in {*BOOT_ERRORS.values(), "boot_unknown", "boot_not_ready"}
        control_failure = message in {"control_invalid", "control_timeout", "control_response_length"}
        diagnostic_allowed = not (boot_failure or control_failure)
        error_type = message if message in {
            "profile_decrypt_failed",
            "profile_schema_invalid",
            "wifi_association",
            "dhcp_timeout",
            "tcp_connect",
            "noise",
            "availability_timeout",
            "benchmark_gate_failed",
            "benchmark_timeout",
        } else None if boot_failure else "store_failed"
        print(message, file=sys.stderr)
    if not records:
        records = [action_record(args.action, "success" if exit_code == 0 else "error", error_type)]
    if args.recording == "on" and diagnostic_allowed:
        try:
            publish_diagnostic(root, run_id, "success" if exit_code == 0 else "error", records)
        except OSError:
            pass
    try:
        result_path = str(publish_result(root, args.action, result, run_id))
    except OSError:
        result_path = ""
    print(json.dumps({"result": result, "run_id": run_id, "result_path": result_path}, separators=(",", ":")))
    return exit_code


if __name__ == "__main__":
    raise SystemExit(main())
