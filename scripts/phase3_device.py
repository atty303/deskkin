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
    "world-benchmark-start": 10,
    "diagnostic-subscribe": 11,
}

WORLD_BENCHMARK_DURATION_SECONDS = 60.0
WORLD_BENCHMARK_MAX_OBSERVATION_AGE_SECONDS = 1.0
WORLD_BENCHMARK_MIN_COVERAGE_MILLI = 800
WORLD_BENCHMARK_MAX_STATUS_RESPONSE_MS = 1_000
WORLD_BENCHMARK_MIN_STATUS_RESPONSES = 20
WORLD_DEMO_ENTITY_COUNT = 71
STATUS_RESPONSE_SIZE = 204

BOOT_ERRORS = {
    1: "boot_devices_unavailable",
    2: "boot_noise_resolver",
    3: "boot_service_worker",
    4: "boot_ui_platform",
    5: "boot_ui_component",
    6: "boot_framebuffer",
    7: "boot_display_transfer",
    8: "boot_display_enable",
    9: "boot_shared_channel",
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


class WorldBenchmarkError(DeviceError):
    def __init__(self, error_type: str, summary: dict[str, object]) -> None:
        super().__init__(error_type)
        self.error_type = error_type
        self.summary = summary


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
            if not 18 <= length <= STATUS_RESPONSE_SIZE or start + 3 > len(buffered):
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


def exchange_once(
    device: Path,
    frame: bytearray,
    startup_delay: float,
    response_timeout: float,
    total_timeout: float | None = None,
) -> bytes:
    transaction_deadline = None if total_timeout is None else time.monotonic() + total_timeout
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
        write_timeout = (
            2.0
            if transaction_deadline is None
            else max(0.0, transaction_deadline - time.monotonic())
        )
        write_all(descriptor, frame, write_timeout)
        read_timeout = (
            response_timeout
            if transaction_deadline is None
            else max(0.0, transaction_deadline - time.monotonic())
        )
        return read_control_response(descriptor, frame, read_timeout)
    finally:
        # USB Serial/JTAG can retain unsent host output when device RX is not
        # draining.  Discard it so closing a diagnostic/control attempt cannot
        # block behind the tty driver's drain timeout.
        try:
            termios.tcflush(descriptor, termios.TCIOFLUSH)
        except termios.error:
            pass
        os.close(descriptor)


def watch_diagnostics(device_arg: str | None, duration_seconds: float, auto_pair: bool) -> None:
    duration = max(1, min(300, int(duration_seconds or 60)))
    device = discover_device(device_arg)
    payload = duration.to_bytes(2, "big") + (b"\x01" if auto_pair else b"")
    frame = control_frame("diagnostic-subscribe", 0, payload)
    descriptor = os.open(device, os.O_RDWR | os.O_NOCTTY | os.O_NONBLOCK)
    try:
        attributes = termios.tcgetattr(descriptor)
        attributes[0] = attributes[1] = attributes[3] = 0
        attributes[2] = termios.CS8 | termios.CREAD | termios.CLOCAL
        attributes[4] = attributes[5] = termios.B115200
        attributes[6][termios.VMIN] = attributes[6][termios.VTIME] = 0
        termios.tcsetattr(descriptor, termios.TCSANOW, attributes)
        termios.tcflush(descriptor, termios.TCIOFLUSH)
        write_all(descriptor, frame, 2.0)
        response = read_control_response(descriptor, frame, 2.0)
        if len(response) != 18 or response[1] != 0:
            raise DeviceError("control_invalid")
        names = {
            1: "boot",
            2: "renderer",
            3: "shell",
            4: "touch",
            5: "ui_command",
            6: "service",
            7: "memory",
        }
        deadline = time.monotonic() + duration + 1
        while time.monotonic() < deadline:
            try:
                prefix = read_exact(descriptor, 2, max(0.1, deadline - time.monotonic()))
            except DeviceError as error:
                if str(error) == "control_timeout":
                    break
                raise
            length = int.from_bytes(prefix, "big")
            payload = read_exact(descriptor, length, max(0.1, deadline - time.monotonic()))
            if length != 24 or payload[0] != SCHEMA or payload[1] != 0x80:
                continue
            sequence = int.from_bytes(payload[4:8], "big")
            x = int.from_bytes(payload[12:14], "big", signed=True)
            y = int.from_bytes(payload[14:16], "big", signed=True)
            event = {
                "sequence": sequence,
                "uptime_ms": int.from_bytes(payload[8:12], "big"),
                "event": names.get(payload[2], "unknown"),
                "flags": payload[3],
                "value": int.from_bytes(payload[16:20], "big"),
                "dropped_before": int.from_bytes(payload[20:24], "big"),
            }
            if payload[2] == 4:
                event["x"] = x
                event["y"] = y
                event["pressed"] = bool(payload[3] & 1)
            elif payload[2] == 2 and payload[3] == 0x80:
                event["renderer_progress_stage"] = x
                event["display_progress_stage"] = y
                event["renderer_progress_sequence"] = event["value"] >> 16
                event["display_progress_sequence"] = event["value"] & 0xFFFF
            elif payload[2] == 6 and payload[3] & 0x80:
                event["stage"] = payload[3] & 0x7F
                event["result_code"] = int.from_bytes(payload[16:20], "big", signed=True)
            print(json.dumps(event, separators=(",", ":")), flush=True)
    finally:
        try:
            termios.tcflush(descriptor, termios.TCIOFLUSH)
        except termios.error:
            pass
        os.close(descriptor)


def exchange(
    device: Path,
    frame: bytearray,
    recover_status_transport: bool = True,
    status_timeout: float | None = None,
) -> bytes:
    if frame[3] == COMMANDS["status"] and not recover_status_transport:
        timeout = 2.0 if status_timeout is None else status_timeout
        return exchange_once(device, frame, 0.0, timeout, total_timeout=status_timeout)
    if frame[3] != COMMANDS["status"]:
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
    if len(status) != STATUS_RESPONSE_SIZE or status[69] == 0:
        return None
    return BOOT_ERRORS.get(status[69], "boot_unknown")


def report_status(status: bytes) -> None:
    if len(status) != STATUS_RESPONSE_SIZE:
        raise DeviceError("control_invalid")
    decoded = decode_world_status(status)
    print(
        json.dumps(
            {
                "shell_state": status[26],
                "availability": decoded["semantic_availability"],
                "heartbeat_freshness": decoded["heartbeat_freshness"],
                "renderer_stage": decoded["stage"],
                "renderer_fault": decoded["fault"],
                "completed_frames": decoded["completed_frames"],
                "render_us": decoded["render_us"],
                "transfer_us": decoded["transfer_us"],
                "render_max_us": decoded["render_max_us"],
                "transfer_max_us": decoded["transfer_max_us"],
                "display_ready": decoded["display_ready"],
                "display_spi_hz": decoded["display_spi_hz"],
                "pixel_dma_batches": decoded["pixel_dma_batches"],
                "allocation_failures": decoded["allocation_failures"],
                "transfer_failures": decoded["transfer_failures"],
                "nvs_failure_stage": decoded["nvs_failure_stage"],
                "nvs_failure_code": decoded["nvs_failure_code"],
                "view_generation": decoded["view_generation"],
                "renderer_shell": decoded["visible_billboards"] if status[26] != 4 else None,
                "shell_property_matches": decoded["culled_billboards"] if status[26] != 4 else None,
                "pixel_transfer_count": decoded["nearest_samples"] if status[26] != 4 else None,
                "pixel_transfer_last_us": decoded["bilinear_samples"] if status[26] != 4 else None,
                "frame_difference_last": decoded["projection_us"] if status[26] != 4 else None,
                "frame_difference_max": decoded["projection_max_us"] if status[26] != 4 else None,
                "pose_generation": decoded["pose_generation"],
                "input_generation": decoded["input_generation"],
                "stale_snapshots": decoded["stale_snapshots"],
                "touch_drops": decoded["touch_drops"],
                "renderer_progress_sequence": decoded["renderer_progress"] >> 8,
                "renderer_progress_stage": decoded["renderer_progress"] & 0xFF,
                "display_progress_sequence": decoded["display_progress"] >> 8,
                "display_progress_stage": decoded["display_progress"] & 0xFF,
                "boot_stage": decoded["procpu_boot_stage"],
                "boot_error": status_boot_error(status),
            },
            separators=(",", ":"),
        ),
        file=sys.stderr,
    )


def await_boot_complete(status: bytes, device_arg: str | None, timeout: float = 15.0) -> bytes:
    deadline = time.monotonic() + timeout
    while True:
        if len(status) != STATUS_RESPONSE_SIZE:
            raise DeviceError("control_invalid")
        if status_boot_error(status) is not None or status[68] == BOOT_COMPLETE_STAGE:
            return status
        if status[68] > BOOT_COMPLETE_STAGE:
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
    status_timeout: float | None = None,
) -> bytes:
    device = discover_device(device_arg)
    generation = 0
    if command in {"identity-init", "identity-unpair", "wifi-provision", "wifi-clear", "run", "shutdown", "world-benchmark-start"}:
        status_frame = control_frame("status", 0)
        try:
            status = exchange(device, status_frame, recover_status_transport)
            if len(status) != STATUS_RESPONSE_SIZE:
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
        result = exchange(device, frame, recover_status_transport, status_timeout)
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
    build_amp(root)
    state, environment = device_environment(root)
    environment["DESKKIN_STATE_DIR"] = str(state)
    device_state = state / "phase3-device"
    ensure_private_directory(device_state)
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


def decode_world_status(status: bytes) -> dict[str, int]:
    if len(status) != STATUS_RESPONSE_SIZE or status[0] != SCHEMA or status[1] != 0:
        raise DeviceError("control_invalid")

    def unsigned(start: int, end: int) -> int:
        return int.from_bytes(status[start:end], "big")

    decoded = {
        "heartbeat_freshness": status[27],
        "generation": unsigned(28, 32),
        "heartbeat_received_ms": unsigned(32, 40),
        "completed_frames": unsigned(40, 44),
        "render_us": unsigned(44, 48),
        "transfer_us": unsigned(48, 52),
        "stage": status[52],
        "fault": status[53],
        "allocation_failures": status[54],
        "transfer_failures": status[55],
        "display_ready": status[56],
        "render_max_us": unsigned(57, 61),
        "transfer_max_us": unsigned(61, 65),
        "nvs_failure_stage": status[65],
        "nvs_failure_code": status[66],
        "heartbeat_memory_side": status[67],
        "procpu_boot_stage": status[68],
        "procpu_boot_error": status[69],
        "display_spi_hz": unsigned(70, 74),
        "copy_us": unsigned(74, 78),
        "deadline_misses": unsigned(78, 80),
        "benchmark_state": status[81],
        "requested_updates": unsigned(84, 88),
        "semantic_availability": status[80] & 0x7F,
        "valid_availability_result": (status[80] & 0x80) != 0,
        "valid_view_generation": unsigned(88, 92),
    }
    decoded.update(
        {
            "pixel_dma_batches": unsigned(82, 84),
            "view_generation": unsigned(92, 96),
            "pose_generation": unsigned(96, 100) if len(status) >= 160 else 0,
            "input_generation": unsigned(100, 104) if len(status) >= 160 else 0,
            "stale_snapshots": unsigned(104, 108) if len(status) >= 160 else 0,
            "touch_drops": unsigned(108, 112) if len(status) >= 160 else 0,
            "atlas_cache_hits": unsigned(112, 114) if len(status) >= 160 else 0,
            "atlas_cache_misses": unsigned(114, 116) if len(status) >= 160 else 0,
            "atlas_cache_failures": unsigned(116, 118) if len(status) >= 160 else 0,
            "visible_billboards": status[118] if len(status) >= 160 else 0,
            "culled_billboards": status[119] if len(status) >= 160 else 0,
            "nearest_samples": unsigned(120, 124) if len(status) >= 160 else 0,
            "bilinear_samples": unsigned(124, 128) if len(status) >= 160 else 0,
            "projection_us": unsigned(128, 132) if len(status) >= 160 else 0,
            "projection_max_us": unsigned(132, 136) if len(status) >= 160 else 0,
            "sort_us": unsigned(136, 140) if len(status) >= 160 else 0,
            "sort_max_us": unsigned(140, 144) if len(status) >= 160 else 0,
            "texture_us": unsigned(144, 148) if len(status) >= 160 else 0,
            "texture_max_us": unsigned(148, 152) if len(status) >= 160 else 0,
            "world_raster_us": unsigned(152, 156) if len(status) >= 160 else 0,
            "world_raster_max_us": unsigned(156, 160) if len(status) >= 160 else 0,
            "renderer_progress": unsigned(160, 164) if len(status) >= 168 else 0,
            "display_progress": unsigned(164, 168) if len(status) >= 168 else 0,
            "profile_generation": unsigned(168, 172),
            "coverage_us": unsigned(172, 176),
            "background_us": unsigned(176, 180),
            "scaler_setup_us": unsigned(180, 184),
            "pixel_raster_us": unsigned(184, 188),
            "coverage_tests": unsigned(188, 192),
            "scaler_preparations": unsigned(192, 196),
            "profile_nearest_samples": unsigned(196, 200),
            "profile_bilinear_samples": unsigned(200, 204),
        }
    )
    return decoded


PROFILE_FIELDS = ("coverage_us", "background_us", "scaler_setup_us", "pixel_raster_us",
                  "coverage_tests", "scaler_preparations", "profile_nearest_samples", "profile_bilinear_samples")


def measure_raster(device_arg: str | None, duration: float) -> dict[str, object]:
    if not 1 <= duration <= 120:
        raise DeviceError("renderer_profile_failed")
    started = time.monotonic()
    samples: list[dict[str, int]] = []
    previous = 0
    last_progress = started
    responses = 0
    while time.monotonic() - started < duration and len(samples) < 600:
        raw = run_control("status", device_arg)
        responses += 1
        sample = decode_world_status(raw)
        if raw[26] != 4 or sample["heartbeat_freshness"] != 1 or any(
            sample[key] for key in ("fault", "allocation_failures", "transfer_failures", "stale_snapshots")
        ):
            raise DeviceError("renderer_profile_failed")
        generation = sample["profile_generation"]
        if generation != 0 and generation != previous:
            samples.append(sample)
            previous = generation
            last_progress = time.monotonic()
        if time.monotonic() - last_progress > 2:
            raise DeviceError("renderer_profile_failed")
        time.sleep(0.2)
    if len(samples) < 2 or samples[-1]["completed_frames"] == samples[0]["completed_frames"]:
        raise DeviceError("renderer_profile_failed")
    result = action_record("raster-profile", "success")
    result.update(duration_ms=int((time.monotonic() - started) * 1000),
                  status_responses=responses, profile_samples=len(samples), first_generation=samples[0]["profile_generation"],
                  last_generation=samples[-1]["profile_generation"])
    result["end_virtual_time_ms"] = result["duration_ms"]
    for field in PROFILE_FIELDS:
        values = [sample[field] for sample in samples]
        result[field + "_min"] = min(values)
        result[field + "_mean"] = sum(values) // len(values)
        result[field + "_max"] = max(values)
    print(json.dumps(result, separators=(",", ":")), file=sys.stderr)
    return result


def world_benchmark(
    device_arg: str | None, duration_seconds: float = WORLD_BENCHMARK_DURATION_SECONDS
) -> dict[str, object]:
    run_control("world-benchmark-start", device_arg, recover_status_transport=True)
    benchmark_started = time.monotonic()
    deadline = benchmark_started + duration_seconds
    responses = 0
    max_response_ms = 0
    first_request = True
    first: dict[str, int] | None = None
    last: dict[str, int] | None = None
    first_observed_at: float | None = None
    last_observed_at: float | None = None
    maximum_observation_gap_ms = 0
    last_sample: dict[str, int] | None = None
    benchmark_scene_sample: dict[str, int] | None = None
    while time.monotonic() < deadline:
        started = time.monotonic()
        try:
            raw_status = run_control("status", device_arg, recover_status_transport=first_request)
        except DeviceError:
            first_request = False
            continue
        first_request = False
        response_ms = int((time.monotonic() - started) * 1000)
        max_response_ms = max(max_response_ms, response_ms)
        responses += 1
        sample = decode_world_status(raw_status)
        last_sample = sample
        if (
            benchmark_scene_sample is None
            or sample["visible_billboards"] + sample["culled_billboards"]
            > benchmark_scene_sample["visible_billboards"]
            + benchmark_scene_sample["culled_billboards"]
        ):
            benchmark_scene_sample = sample
        if sample["heartbeat_freshness"] == 1 and sample["generation"] > 0:
            observed_at = time.monotonic()
            if first is None:
                first = sample
                first_observed_at = observed_at
            if last_observed_at is not None:
                maximum_observation_gap_ms = max(
                    maximum_observation_gap_ms,
                    int((observed_at - last_observed_at) * 1000),
                )
            last = sample
            last_observed_at = observed_at
        time.sleep(0.25)
    # A terminal observation is mandatory: it proves that the device, rather
    # than the host's wall clock, completed the 1,200-update schedule. Allow a
    # bounded grace period for a status request racing the final 20 Hz publish.
    terminal = None
    terminal_deadline = time.monotonic() + 1.0
    while terminal is None or terminal["benchmark_state"] != 2:
        remaining_grace = terminal_deadline - time.monotonic()
        if remaining_grace <= 0:
            break
        try:
            raw_status = run_control(
                "status",
                device_arg,
                recover_status_transport=False,
                status_timeout=min(1.0, remaining_grace),
            )
            responses += 1
            terminal = decode_world_status(raw_status)
            last_sample = terminal
            observed_at = time.monotonic()
            if terminal["heartbeat_freshness"] == 1 and terminal["generation"] > 0:
                if last_observed_at is not None:
                    maximum_observation_gap_ms = max(
                        maximum_observation_gap_ms,
                        int((observed_at - last_observed_at) * 1000),
                    )
                last = terminal
                last_observed_at = observed_at
        except DeviceError:
            pass
        if terminal is not None and terminal["benchmark_state"] == 2:
            break
        if time.monotonic() >= terminal_deadline:
            break
        time.sleep(0.05)
    if first is None or last is None:
        summary: dict[str, object] = {
            "operation": "world_benchmark",
            "operation_id": 1,
            "parent_operation_id": None,
            "status": "error",
            "error_type": "world_not_observed",
            "effect_id": None,
            "virtual_time_ms": 0,
            "end_virtual_time_ms": int((time.monotonic() - benchmark_started) * 1000),
            "duration_ms": int((time.monotonic() - benchmark_started) * 1000),
            "value": "unavailable",
            "status_responses": responses,
            "max_status_response_ms": max_response_ms,
            "last_availability": last_sample["semantic_availability"] if last_sample is not None else 0,
            "renderer_stage": last_sample["stage"] if last_sample is not None else 0,
            "allocation_failures": last_sample["allocation_failures"] if last_sample is not None else 0,
            "transfer_failures": last_sample["transfer_failures"] if last_sample is not None else 0,
        }
        print(json.dumps(summary, separators=(",", ":")), file=sys.stderr)
        raise WorldBenchmarkError("world_not_observed", summary)
    benchmark_finished = time.monotonic()
    duration_ms = int((benchmark_finished - benchmark_started) * 1000)
    assert first_observed_at is not None
    assert last_observed_at is not None
    observation_duration_ms = max(1, int((last_observed_at - first_observed_at) * 1000))
    measurement_duration_ms = max(
        1,
        (last["heartbeat_received_ms"] - first["heartbeat_received_ms"]) & 0xFFFFFFFF,
    )
    last_observation_age_ms = max(0, int((benchmark_finished - last_observed_at) * 1000))
    measurement_coverage_milli = observation_duration_ms * 1000 // max(1, duration_ms)
    completed_frames = last["completed_frames"] - first["completed_frames"]
    measured_fps_milli = completed_frames * 1_000_000 // measurement_duration_ms
    passed = (
        responses >= WORLD_BENCHMARK_MIN_STATUS_RESPONSES
        and max_response_ms <= WORLD_BENCHMARK_MAX_STATUS_RESPONSE_MS
        and last_sample is not None
        and last_sample["heartbeat_freshness"] == 1
        and last_observation_age_ms <= int(WORLD_BENCHMARK_MAX_OBSERVATION_AGE_SECONDS * 1000)
        and measurement_coverage_milli >= WORLD_BENCHMARK_MIN_COVERAGE_MILLI
        and maximum_observation_gap_ms <= int(WORLD_BENCHMARK_MAX_OBSERVATION_AGE_SECONDS * 1000)
        and terminal is not None
        and terminal["benchmark_state"] == 2
        and terminal["requested_updates"] == 1_200
        and last["generation"] > first["generation"]
        and completed_frames > 0
        and last["stage"] != 5
        and last["display_ready"] == 1
        and last["pixel_dma_batches"] > 0
        and last["fault"] == 0
        and last["allocation_failures"] == 0
        and last["transfer_failures"] == 0
        and last["stale_snapshots"] == 0
        and last["atlas_cache_failures"] == 0
        and last["view_generation"] > first["view_generation"]
        and last["pose_generation"] > first["pose_generation"]
        and benchmark_scene_sample is not None
        and benchmark_scene_sample["visible_billboards"]
        + benchmark_scene_sample["culled_billboards"]
        == WORLD_DEMO_ENTITY_COUNT
    )
    summary: dict[str, object] = {
        "operation": "world_benchmark",
        "operation_id": 1,
        "parent_operation_id": None,
        "status": "success" if passed else "error",
        "error_type": None if passed else "world_benchmark_failed",
        "effect_id": None,
        "virtual_time_ms": 0,
        "end_virtual_time_ms": duration_ms,
        "duration_ms": duration_ms,
        "measurement_duration_ms": measurement_duration_ms,
        "observation_duration_ms": observation_duration_ms,
        "measurement_coverage_milli": measurement_coverage_milli,
        "maximum_observation_gap_ms": maximum_observation_gap_ms,
        "last_observation_age_ms": last_observation_age_ms,
        "last_availability": last_sample["semantic_availability"] if last_sample is not None else 0,
        "render_width": 320,
        "render_height": 240,
        "value": "observed" if passed else "unavailable",
        "status_responses": responses,
        "completed_frames": completed_frames,
        "requested_updates": terminal["requested_updates"] if terminal is not None else 0,
        "deadline_misses": (last["deadline_misses"] - first["deadline_misses"]) & 0xFFFF,
        "measured_fps_milli": measured_fps_milli,
        "first_generation": first["generation"],
        "last_generation": last["generation"],
        "render_last_us": last["render_us"],
        "transfer_last_us": last["transfer_us"],
        "render_max_us": last["render_max_us"],
        "transfer_max_us": last["transfer_max_us"],
        "renderer_stage": last["stage"],
        "display_ready": last["display_ready"],
        "max_status_response_ms": max_response_ms,
        "allocation_failures": last["allocation_failures"],
        "transfer_failures": last["transfer_failures"],
        "mailbox_published": last["heartbeat_memory_side"],
        "procpu_boot_stage": last["procpu_boot_stage"],
        "procpu_boot_error": last["procpu_boot_error"],
        "display_spi_hz": last["display_spi_hz"],
        "copy_last_us": last["copy_us"],
        "wire_last_us": max(0, last["transfer_us"] - last["copy_us"]),
        "pixel_dma_batches": last["pixel_dma_batches"],
        "view_generation": last["view_generation"],
        "pose_generation": last["pose_generation"],
        "input_generation": last["input_generation"],
        "stale_snapshots": last["stale_snapshots"],
        "touch_drops": last["touch_drops"],
        "atlas_cache_hits": last["atlas_cache_hits"],
        "atlas_cache_misses": last["atlas_cache_misses"],
        "atlas_cache_failures": last["atlas_cache_failures"],
        "visible_billboards": (
            benchmark_scene_sample["visible_billboards"]
            if benchmark_scene_sample is not None
            else 0
        ),
        "culled_billboards": (
            benchmark_scene_sample["culled_billboards"]
            if benchmark_scene_sample is not None
            else 0
        ),
        "nearest_samples": last["nearest_samples"],
        "bilinear_samples": last["bilinear_samples"],
        "projection_last_us": last["projection_us"],
        "projection_max_us": last["projection_max_us"],
        "sort_last_us": last["sort_us"],
        "sort_max_us": last["sort_max_us"],
        "texture_last_us": last["texture_us"],
        "texture_max_us": last["texture_max_us"],
        "world_raster_last_us": last["world_raster_us"],
        "world_raster_max_us": last["world_raster_max_us"],
    }
    print(json.dumps(summary, separators=(",", ":")), file=sys.stderr)
    if not passed:
        raise WorldBenchmarkError("world_benchmark_failed", summary)
    return summary


def flash(root: Path, device_arg: str | None) -> None:
    flash_amp(root, device_arg)


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


DIAGNOSTIC_RECORD_KEYS = frozenset(
    {
        "operation", "operation_id", "parent_operation_id", "status", "error_type",
        "effect_id", "virtual_time_ms", "end_virtual_time_ms", "duration_ms",
        "measurement_duration_ms", "observation_duration_ms", "measurement_coverage_milli",
        "maximum_observation_gap_ms", "last_observation_age_ms", "last_availability",
        "render_width", "render_height", "value", "status_responses", "completed_frames",
        "requested_updates", "deadline_misses", "measured_fps_milli", "first_generation",
        "last_generation", "generation", "render_last_us", "transfer_last_us",
        "render_max_us", "transfer_max_us", "renderer_stage", "renderer_fault",
        "display_ready", "max_status_response_ms", "allocation_failures", "transfer_failures",
        "mailbox_published", "procpu_boot_stage",
        "procpu_boot_error", "display_spi_hz", "copy_last_us", "wire_last_us",
        "pixel_dma_batches", "view_generation", "valid_view_generation",
        "pose_generation", "input_generation", "stale_snapshots", "touch_drops",
        "atlas_cache_hits", "atlas_cache_misses", "atlas_cache_failures",
        "visible_billboards", "culled_billboards", "nearest_samples", "bilinear_samples",
        "projection_last_us", "projection_max_us", "sort_last_us", "sort_max_us",
        "texture_last_us", "texture_max_us", "world_raster_last_us", "world_raster_max_us",
        "shell_state", "availability", "heartbeat_freshness", "valid_availability_result",
        "profile_samples",
    } | {field + suffix for field in PROFILE_FIELDS for suffix in ("_min", "_mean", "_max")}
)


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
    records = [{key: value for key, value in record.items() if key in DIAGNOSTIC_RECORD_KEYS} for record in records]
    value = {
        "schema_version": 1,
        "resource": {"program": "deskkin-core-s3-runner", "version": "0.1.0", "role": "physical_device"},
        "run_id": run_id,
        "scenario_run_id": run_id,
        "session_context_id": None,
        "operation_context_id": None,
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
    del expected_attempt
    records: list[dict[str, object]] = []
    started = time.monotonic()
    last_elapsed = 0
    previous: tuple[int, ...] | None = None
    try:
        while duration_seconds <= 0 or time.monotonic() - started < duration_seconds:
            result = run_control("status", device_arg, recover_status_transport=False)
            status = decode_world_status(result)
            value = (
                result[26], status["semantic_availability"], status["generation"],
                status["completed_frames"], status["stage"], status["fault"],
                status["allocation_failures"], status["transfer_failures"],
                status["stale_snapshots"], int(status["valid_availability_result"]),
                status["valid_view_generation"], status["view_generation"],
            )
            if value != previous:
                elapsed = int((time.monotonic() - started) * 1000)
                error_type = "renderer_fault" if value[5] != 0 else None
                records.append(
                    {
                        "operation": "device_ui",
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
                        "shell_state": value[0],
                        "availability": value[1],
                        "generation": value[2],
                        "completed_frames": value[3],
                        "renderer_stage": value[4],
                        "renderer_fault": value[5],
                        "allocation_failures": value[6],
                        "transfer_failures": value[7],
                        "stale_snapshots": value[8],
                        "valid_availability_result": bool(value[9]),
                        "valid_view_generation": value[10],
                        "view_generation": value[11],
                        "heartbeat_freshness": status["heartbeat_freshness"],
                    }
                )
                last_elapsed = elapsed
                previous = value
            time.sleep(0.25)
    except KeyboardInterrupt:
        pass
    return records


def run_succeeded(records: list[dict[str, object]], expected_attempt: int) -> bool:
    del expected_attempt
    return any(
        record.get("shell_state") == 4
        and record.get("availability") in {1, 2}
        and record.get("valid_availability_result") is True
        and 0 < int(record.get("valid_view_generation", 0)) <= int(record.get("view_generation", 0))
        and int(record.get("generation", 0)) > 0
        and int(record.get("completed_frames", 0)) > 0
        and record.get("renderer_stage") != 5
        and record.get("renderer_fault") == 0
        and record.get("allocation_failures") == 0
        and record.get("transfer_failures") == 0
        and int(record.get("pixel_dma_batches", 0)) > 0
        and record.get("stale_snapshots") == 0
        for record in records
    )


def action_record(action: str, status: str, error_type: str | None = None) -> dict[str, object]:
    operation = {
        "profile": "control_route",
        "build": "control_route",
        "flash": "device_ui",
        "identity": "identity_init",
        "provision": "nvs_publication",
        "status": "device_ui",
        "run": "device_ui",
        "benchmark": "world_benchmark",
        "raster-profile": "raster_profile",
        "watch": "diagnostic_stream",
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
    parser.add_argument("action", choices=("profile", "build", "flash", "identity", "provision", "status", "watch", "run", "benchmark", "raster-profile", "recover"))
    parser.add_argument("identity_action", nargs="?")
    parser.add_argument("--profile", type=Path)
    parser.add_argument("--age-identity", type=Path)
    parser.add_argument("--device")
    parser.add_argument("--peer-id")
    parser.add_argument("--erase-storage", action="store_true")
    parser.add_argument("--duration-seconds", type=float, default=0)
    parser.add_argument("--auto-pair", action="store_true")
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
        elif args.action == "watch":
            diagnostic_allowed = False
            watch_diagnostics(args.device, args.duration_seconds, args.auto_pair)
        elif args.action == "run":
            accepted = run_control("run", args.device)
            if len(accepted) < 30:
                raise DeviceError("control_invalid")
            expected_attempt = int.from_bytes(accepted[26:30], "big")
            records = monitor_device(args.device, args.duration_seconds, expected_attempt)
            if not run_succeeded(records, expected_attempt):
                raise DeviceError("availability_timeout")
        elif args.action == "benchmark":
            records = [world_benchmark(args.device, WORLD_BENCHMARK_DURATION_SECONDS)]
        elif args.action == "raster-profile":
            records = [measure_raster(args.device, args.duration_seconds or 60)]
        result = "success"
        exit_code = 0
    except (DeviceError, OSError, subprocess.SubprocessError, UnicodeError, ValueError) as error:
        message = str(error) if isinstance(error, DeviceError) else "device_operation_failed"
        if isinstance(error, WorldBenchmarkError):
            records = [error.summary]
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
            "world_not_observed",
            "world_benchmark_failed",
            "renderer_profile_failed",
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
