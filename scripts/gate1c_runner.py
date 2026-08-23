#!/usr/bin/env python3
"""Bounded Gate 1C ESP32-S3/Xtensa Zephyr Rust feasibility runner."""

from __future__ import annotations

import argparse
import contextlib
import fcntl
import hashlib
import json
import os
import re
import selectors
import shutil
import signal
import stat
import subprocess
import sys
import termios
import time
import uuid
from pathlib import Path

import gate_runner as common

TARGET = "m5stack_cores3/esp32s3/procpu"
PATCH_DIFF_SHA256 = "3a16ecd15058a4ceb80245fcc0ba5ef89087183b428f718c8ce8890e5559f186"
RUSTC_COMMIT = "95e5bda868c960c607597bc03ed9e8f0ad26226d"
TOOL_DIGESTS = {
    "rustc": "fb6469add601520c44d68115ad4ff137985b1ff320e001a471ef35d786df51fd",
    "cargo": "f9f150db83d4b06a9da0a0dd1c1736048efba7edc5ada70c1b7972efe2539983",
    "libclang": "12e2f4e8e3fb62ce00e0cba30ddd9cef84935a899f040beb0c0a226940d73d8b",
    "xtensa_gcc": "6c37a821e8f20d8d53c536460e5c2f292f22d923ae29655bc9c054278e352a84",
}
WEST_REVISIONS = {
    **common.WEST_REVISIONS,
    "hal_espressif": "19f979cfe66bcab09abe3b0b3aa419a664c1606c",
    "hal_xtensa": "0495a1afd300b644d3ec8dd2c3bd11007e69a892",
}
PATCHES = tuple(f"patches/gate1c-zephyr-lang-rust/{name}" for name in (
    "0001-map-esp32s3-xtensa-target.patch",
    "0002-enable-esp32s3-xtensa-kconfig.patch",
    "0003-build-xtensa-core-from-source.patch",
    "0004-recognize-esp32-flash-controller.patch",
    "0005-use-fixed-width-kconfig-integers.patch",
))
INPUTS = (
    "west.yml", "mise.toml", "mise.lock", "requirements/gate1c.in",
    "requirements/gate1c.lock", "scripts/bootstrap_gate1c.sh",
    "scripts/gate1c_runner.py", "gates/gate1c/CMakeLists.txt",
    "gates/gate1c/Cargo.toml", "gates/gate1c/Cargo.lock",
    "gates/gate1c/Kconfig", "gates/gate1c/prj.conf",
    "gates/gate1c/panic.conf", "gates/gate1c/src/abi.c",
    "gates/gate1c/src/lib.rs", *PATCHES,
)
FIRMWARE_INPUTS = (
    "gates/gate1c/CMakeLists.txt", "gates/gate1c/Cargo.lock",
    "gates/gate1c/Cargo.toml", "gates/gate1c/Kconfig",
    "gates/gate1c/prj.conf", "gates/gate1c/panic.conf",
    "gates/gate1c/src/abi.c", "gates/gate1c/src/lib.rs",
)
UUID_PATTERN = r"[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}"
DIGEST_PATTERN = r"[0-9a-f]{64}"
SERIAL_PATTERNS = {
    "idle": re.compile(rf"^DESKKIN_GATE_EVENT schema=1 event=idle run_id=(?P<run_id>{UUID_PATTERN}) firmware_digest=(?P<firmware_digest>{DIGEST_PATTERN})$"),
    "boot": re.compile(rf"^DESKKIN_GATE_EVENT schema=1 event=boot run_id=(?P<run_id>{UUID_PATTERN}) board=(?P<board>[a-z0-9_]+) firmware_digest=(?P<firmware_digest>{DIGEST_PATTERN})$"),
    "abi": re.compile(rf"^DESKKIN_GATE_EVENT schema=1 event=abi run_id=(?P<run_id>{UUID_PATTERN}) c_to_rust=(?P<c_to_rust>[0-9]+) rust_to_c=(?P<rust_to_c>[0-9]+)$"),
    "atomic": re.compile(rf"^DESKKIN_GATE_EVENT schema=1 event=atomic run_id=(?P<run_id>{UUID_PATTERN}) value=(?P<value>[0-9]+) nesting=(?P<nesting>ok) restoration=(?P<restoration>ok)$"),
    "allocation": re.compile(rf"^DESKKIN_GATE_EVENT schema=1 event=allocation run_id=(?P<run_id>{UUID_PATTERN}) value=(?P<value>[0-9]+) freed=(?P<freed>ok)$"),
    "panic_trigger": re.compile(rf"^DESKKIN_GATE_EVENT schema=1 event=panic_trigger run_id=(?P<run_id>{UUID_PATTERN}) reason=(?P<reason>deliberate)$"),
    "result": re.compile(rf"^DESKKIN_GATE_RESULT schema=1 run_id=(?P<run_id>{UUID_PATTERN}) result=(?P<result>pass)$"),
}


def firmware_digest(root: Path) -> str:
    digest = hashlib.sha256()
    for relative in FIRMWARE_INPUTS:
        encoded = relative.encode()
        digest.update(len(encoded).to_bytes(4, "big"))
        digest.update(encoded)
        payload = (root / relative).read_bytes()
        digest.update(len(payload).to_bytes(8, "big"))
        digest.update(payload)
    return digest.hexdigest()


def parse_serial_record(line: str, target: str, mode: str) -> dict[str, object] | None:
    for event, pattern in SERIAL_PATTERNS.items():
        match = pattern.fullmatch(line)
        if match is None:
            continue
        record: dict[str, object] = {"schema_version": 1, "target": target, "mode": mode, "event": event}
        for key, value in match.groupdict().items():
            record[key] = int(value) if key in {"value", "c_to_rust", "rust_to_c"} else value
        return record
    return None


def acquire_lock(state: Path, run_id: str):
    locks = state / "locks"
    common.private_directory(locks)
    directory_fd = os.open(locks, os.O_RDONLY | getattr(os, "O_DIRECTORY", 0) | getattr(os, "O_NOFOLLOW", 0))
    try:
        file_fd = os.open("1c.lock", os.O_RDWR | os.O_CREAT | getattr(os, "O_NOFOLLOW", 0), 0o600, dir_fd=directory_fd)
    finally:
        os.close(directory_fd)
    stream = os.fdopen(file_fd, "r+", encoding="utf-8")
    try:
        if not stat.S_ISREG(os.fstat(stream.fileno()).st_mode):
            raise OSError("lock is not a regular file")
        fcntl.flock(stream, fcntl.LOCK_EX | fcntl.LOCK_NB)
    except BlockingIOError:
        stream.seek(0)
        try:
            owner = str(json.load(stream).get("run_id", "unknown"))
        except (json.JSONDecodeError, AttributeError, UnicodeError):
            owner = "unknown"
        stream.close()
        raise common.GateInconclusive(f"gate_locked owner={owner}") from None
    except OSError as error:
        stream.close()
        raise common.GateInconclusive("lock_unavailable") from error
    stream.seek(0)
    stream.truncate()
    json.dump({"gate": "1c", "run_id": run_id, "pid": os.getpid(), "started_at": common.utc_now()}, stream, sort_keys=True)
    stream.write("\n")
    stream.flush()
    os.fsync(stream.fileno())
    return stream


class Runner(common.Runner):
    def __init__(self, root: Path, recording: bool, run_id: str, firmware_identity: str | None = None):
        super().__init__(root, recording, run_id, deadline_seconds=1200)
        self.verified_resources.update({"gate": "1c", "target": [TARGET]})
        self.builds = 0
        self.rebuilds = 0
        self.panic_builds = 0
        self.linker_checks = 0
        self.abi_symbols = 0
        self.builtins_checks = 0
        self.physical_boots = 0
        self.c_abi_runtime_checks = 0
        self.atomic_runtime_checks = 0
        self.allocator_runtime_checks = 0
        self.idle_checks = 0
        self.deliberate_panics_count = 0
        self.device: Path | None = None
        self.device_touched = False
        self.device_state = "unchanged"
        self.firmware_digest = firmware_identity if firmware_identity is not None else firmware_digest(root)
        self.normal_image_sha256: str | None = None

    def _environment(self) -> dict[str, str]:
        toolchain = self.state / "rustup/toolchains/deskkin-esp"
        clang = toolchain / "xtensa-esp32-elf-clang/esp-20.1.1_20250829/esp-clang"
        cmake = Path(subprocess.run(["mise", "where", "cmake"], check=True, capture_output=True, text=True).stdout.strip()) / "bin"
        ninja = Path(subprocess.run(["mise", "where", "ninja"], check=True, capture_output=True, text=True).stdout.strip())
        env = os.environ.copy()
        env.update({
            "PATH": os.pathsep.join((str(toolchain / "bin"), str(cmake), str(ninja), str(self.state / "venv/bin"), env["PATH"])),
            "LIBCLANG_PATH": str(clang / "lib"),
            "ZEPHYR_SDK_INSTALL_DIR": str(self.state / "sdk"),
            "ZEPHYR_TOOLCHAIN_VARIANT": "zephyr",
            "ZEPHYR_BASE": str(self.state / "west/zephyr"),
            "SOURCE_DATE_EPOCH": "0",
        })
        return env

    def prepare(self) -> None:
        start = time.monotonic()
        try:
            self.env = self._environment()
            west = self.state / "venv/bin/west"
            actual: dict[str, str] = {}
            for name, expected in WEST_REVISIONS.items():
                path = Path(self.command("prepare", [str(west), "list", name, "-f", "{abspath}"], "host").strip())
                self.west_paths[name] = path
                revision = self.command("prepare", ["git", "-C", str(path), "rev-parse", "HEAD"], "host").strip()
                if revision != expected:
                    raise common.GateInconclusive(f"west_project_mismatch_{name.replace('-', '_')}")
                actual[name] = revision
            module = self.west_paths["zephyr-lang-rust"]
            for patch in PATCHES:
                check = subprocess.run(["git", "-C", str(module), "apply", "--reverse", "--check", str(self.root / patch)], capture_output=True)
                if check.returncode != 0:
                    raise common.GateInconclusive("patch_series_mismatch")
            status = self.command("prepare", ["git", "-C", str(module), "status", "--porcelain"], "host").splitlines()
            if status != [" M CMakeLists.txt", " M Kconfig", " M dt-rust.yaml", " M zephyr-build/src/lib.rs"]:
                raise common.GateInconclusive("patched_tree_mismatch")
            patch_diff = subprocess.run(["git", "-C", str(module), "diff", "--binary", "HEAD"], check=True, capture_output=True).stdout
            if hashlib.sha256(patch_diff).hexdigest() != PATCH_DIFF_SHA256:
                raise common.GateInconclusive("patched_tree_mismatch")
            probes = {
                "rust": self.command("prepare", ["rustc", "-vV"], "host").splitlines()[0],
                "gcc": self.command("prepare", [str(self.state / "sdk/gnu/xtensa-espressif_esp32s3_zephyr-elf/bin/xtensa-espressif_esp32s3_zephyr-elf-gcc"), "--version"], "host").splitlines()[0],
                "cmake": self.command("prepare", ["cmake", "--version"], "host").splitlines()[0],
                "ninja": self.command("prepare", ["ninja", "--version"], "host").strip(),
                "python": self.command("prepare", [str(self.state / "venv/bin/python"), "--version"], "host").strip(),
            }
            expected = {"rust": "1.95.0", "gcc": "14.3.0", "cmake": "3.28.6", "ninja": "1.13.2", "python": "3.12"}
            if any(fragment not in probes[name] for name, fragment in expected.items()):
                raise common.GateInconclusive("host_tool_mismatch")
            verbose_rust = self.command("prepare", ["rustc", "-vV"], "host")
            if f"commit-hash: {RUSTC_COMMIT}" not in verbose_rust:
                raise common.GateInconclusive("rust_toolchain_mismatch")
            tool_paths = self._tool_paths()
            if any(common.sha256(tool_paths[name]) != digest for name, digest in TOOL_DIGESTS.items()):
                raise common.GateInconclusive("tool_digest_mismatch")
            revision = self.command("prepare", ["git", "-C", str(self.root), "rev-parse", "HEAD"], "host").strip()
            self.verified_resources.update({
                "application_version": "gate1c-0.1.0", "build_type": "dev",
                "deskkin_revision": revision,
                "deskkin_dirty": bool(self.command("prepare", ["git", "-C", str(self.root), "status", "--porcelain"], "host").strip()),
                "west_revisions": actual, "sdk_file_digests": dict(TOOL_DIGESTS), "sdk_version": "1.0.1",
                "tool_identities": probes,
                "input_digests": {name: common.sha256(self.root / name) for name in INPUTS},
            })
            self.recorder._append({"schema_version": 1, "type": "resource_verified", **self.verified_resources})
            self.recorder.event("prepare", "success", round((time.monotonic() - start) * 1000))
        except common.GateFailure:
            self.recorder.event("prepare", "error", round((time.monotonic() - start) * 1000), "provenance_mismatch")
            raise
        except (OSError, subprocess.SubprocessError, KeyError) as error:
            self.recorder.event("prepare", "error", round((time.monotonic() - start) * 1000), "setup_probe_failed")
            raise common.GateInconclusive("setup_probe_failed") from error

    def verify_inputs(self) -> None:
        if any(common.sha256(self.root / name) != digest for name, digest in self.verified_resources["input_digests"].items()):
            raise common.GateInconclusive("input_changed")
        module = self.west_paths["zephyr-lang-rust"]
        patch_diff = subprocess.run(["git", "-C", str(module), "diff", "--binary", "HEAD"], check=True, capture_output=True).stdout
        if hashlib.sha256(patch_diff).hexdigest() != PATCH_DIFF_SHA256:
            raise common.GateInconclusive("input_changed")
        for name, expected in WEST_REVISIONS.items():
            path = self.west_paths[name]
            if self.command("prepare", ["git", "-C", str(path), "rev-parse", "HEAD"], "host").strip() != expected:
                raise common.GateInconclusive("input_changed")
            if name != "zephyr-lang-rust" and self.command("prepare", ["git", "-C", str(path), "status", "--porcelain"], "host").strip():
                raise common.GateInconclusive("input_changed")
        if any(common.sha256(self._tool_paths()[name]) != digest for name, digest in TOOL_DIGESTS.items()):
            raise common.GateInconclusive("input_changed")

    def _tool_paths(self) -> dict[str, Path]:
        toolchain = self.state / "rustup/toolchains/deskkin-esp"
        return {
            "rustc": toolchain / "bin/rustc",
            "cargo": toolchain / "bin/cargo",
            "libclang": toolchain / "xtensa-esp32-elf-clang/esp-20.1.1_20250829/esp-clang/lib/libclang.so.20.1.1",
            "xtensa_gcc": self.state / "sdk/gnu/xtensa-espressif_esp32s3_zephyr-elf/bin/xtensa-espressif_esp32s3_zephyr-elf-gcc",
        }

    def configure(self, build: Path, panic: bool = False) -> None:
        command = [str(self.state / "venv/bin/west"), "build", "--cmake-only", "--board", TARGET, "--build-dir", str(build), str(self.root / "gates/gate1c")]
        command += ["--", f"-DDESKKIN_FIRMWARE_DIGEST={self.firmware_digest}"]
        if panic:
            command += ["-DEXTRA_CONF_FILE=panic.conf"]
        self.command("configure", command, TARGET)

    def build_image(self, build: Path) -> str:
        self.command("rust-compile", ["cmake", "--build", str(build), "--target", "librustapp"], TARGET)
        self.command("c-compile", ["cmake", "--build", str(build), "--target", "zephyr_pre0"], TARGET)
        self.command("link", ["cmake", "--build", str(build), "--target", "zephyr_final"], TARGET)
        elf = build / "zephyr/zephyr.elf"
        digest = common.sha256(elf)
        self.links.append({"target": TARGET, "mode": build.name, "sha256": digest, "bytes": elf.stat().st_size})
        self.verify_linker(elf, build / "zephyr/zephyr.map")
        return digest

    def discover_device(self, requested: str | None) -> Path:
        if requested is not None:
            candidates = [Path(requested)]
        else:
            base = Path("/dev/serial/by-id")
            candidates = sorted(base.glob("usb-Espressif_USB_JTAG_serial_debug_unit_*-if00")) if base.is_dir() else []
        if not candidates:
            raise common.GateInconclusive("physical_device_required")
        if len(candidates) != 1:
            raise common.GateInconclusive("device_selection_ambiguous")
        candidate = candidates[0]
        try:
            resolved = candidate.resolve(strict=True)
            metadata = resolved.stat()
        except OSError as error:
            raise common.GateInconclusive("device_unavailable") from error
        if not stat.S_ISCHR(metadata.st_mode) or not self._is_espressif_tty(resolved):
            raise common.GateInconclusive("device_not_recognized")
        if not os.access(resolved, os.R_OK | os.W_OK):
            raise common.GateInconclusive("device_permission_denied")
        return candidate

    @staticmethod
    def _is_espressif_tty(device: Path) -> bool:
        sysfs = Path("/sys/class/tty") / device.name / "device"
        try:
            current = sysfs.resolve(strict=True)
        except OSError:
            return False
        for parent in (current, *current.parents):
            uevent = parent / "uevent"
            try:
                if "PRODUCT=303a/1001/" in uevent.read_text(encoding="utf-8"):
                    return True
            except OSError:
                continue
        return False

    def _configure_serial(self, descriptor: int) -> None:
        attributes = termios.tcgetattr(descriptor)
        attributes[0] = 0
        attributes[1] = 0
        attributes[2] = termios.CS8 | termios.CREAD | termios.CLOCAL
        attributes[3] = 0
        attributes[4] = termios.B115200
        attributes[5] = termios.B115200
        attributes[6][termios.VMIN] = 0
        attributes[6][termios.VTIME] = 0
        termios.tcsetattr(descriptor, termios.TCSANOW, attributes)
        termios.tcflush(descriptor, termios.TCIOFLUSH)

    def serial_exchange(self, action: str, mode: str, required: set[str], timeout: float) -> list[dict[str, object]]:
        if self.device is None:
            raise common.GateInconclusive("device_unavailable")
        started = time.monotonic()
        descriptor: int | None = None
        selector: selectors.BaseSelector | None = None
        records: list[dict[str, object]] = []
        pending = b""
        panic_observed = False
        operation = "boot" if mode in {"normal", "panic"} else mode
        try:
            descriptor = os.open(self.device, os.O_RDWR | os.O_NOCTTY | os.O_NONBLOCK)
            self._configure_serial(descriptor)
            selector = selectors.DefaultSelector()
            selector.register(descriptor, selectors.EVENT_READ)
            command = f"DESKKIN_GATE_COMMAND schema=1 action={action} run_id={self.run_id}\n".encode()
            os.write(descriptor, command)
            next_status_retry = time.monotonic() + 0.25
            deadline = min(self.deadline, time.monotonic() + timeout)
            while time.monotonic() < deadline:
                if self.cancelled:
                    raise common.GateCancelled("cancelled")
                if action == "status" and time.monotonic() >= next_status_retry:
                    os.write(descriptor, command)
                    next_status_retry = time.monotonic() + 0.25
                for key, _ in selector.select(timeout=0.05):
                    try:
                        chunk = os.read(key.fd, 65536)
                    except BlockingIOError:
                        continue
                    if not chunk:
                        continue
                    pending += chunk
                    lines = pending.split(b"\n")
                    pending = lines.pop()
                    for raw in lines:
                        clean = common.ANSI_RE.sub("", raw.decode("utf-8", errors="replace").strip())
                        panic_observed = panic_observed or "panicked at" in clean
                        record = parse_serial_record(clean, TARGET, mode)
                        if record is None or record.get("run_id") != self.run_id:
                            continue
                        if "firmware_digest" in record and record["firmware_digest"] != self.firmware_digest:
                            raise common.GateInconclusive("firmware_digest_mismatch")
                        records.append(record)
                        self.recorder.serial.append(record)
                events = {str(record["event"]) for record in records}
                if required <= events and ("panic_trigger" not in required or panic_observed):
                    if panic_observed:
                        panic_record = {"schema_version": 1, "target": TARGET, "mode": mode, "event": "panic", "panic_type": "rust_panic", "run_id": self.run_id}
                        records.append(panic_record)
                        self.recorder.serial.append(panic_record)
                    self.recorder.event(operation, "success", round((time.monotonic() - started) * 1000), target=TARGET)
                    return records
            if time.monotonic() >= self.deadline:
                self.recorder.event(operation, "timeout", round((time.monotonic() - started) * 1000), "deadline_exceeded", TARGET)
                raise common.GateTimeout("deadline_exceeded")
            self.recorder.event(operation, "error", round((time.monotonic() - started) * 1000), "serial_protocol_timeout", TARGET)
            raise common.GateInconclusive("serial_protocol_timeout")
        except PermissionError as error:
            raise common.GateInconclusive("device_permission_denied") from error
        finally:
            if selector is not None:
                selector.close()
            if descriptor is not None:
                os.close(descriptor)

    def flash(self, build: Path) -> None:
        if self.device is None:
            raise common.GateInconclusive("device_unavailable")
        self.device_touched = True
        west = self.state / "venv/bin/west"
        self.command("flash", [str(west), "flash", "--skip-rebuild", "--build-dir", str(build), "--runner", "esp32", "--", "--esp-device", str(self.device)], TARGET)

    def preflight(self) -> None:
        try:
            self.serial_exchange("status", "preflight", {"idle"}, 3)
        except (common.GateCancelled, common.GateTimeout):
            raise
        except (common.GateFailure, OSError, termios.error) as error:
            raise common.GateInconclusive("device_state_unknown") from error

    def run_normal(self, build: Path) -> None:
        self.flash(build)
        self.serial_exchange("status", "postflash", {"idle"}, 5)
        records = self.serial_exchange("run", "normal", {"boot", "abi", "atomic", "allocation", "result", "idle"}, 10)
        by_event = {str(record["event"]): record for record in records}
        if by_event["abi"].get("c_to_rust") != 42 or by_event["abi"].get("rust_to_c") != 42:
            raise common.GateFailure("c_abi_runtime_failed")
        if by_event["atomic"].get("value") != 42 or by_event["atomic"].get("nesting") != "ok" or by_event["atomic"].get("restoration") != "ok":
            raise common.GateFailure("atomic_runtime_failed")
        if by_event["allocation"].get("value") != 42 or by_event["allocation"].get("freed") != "ok":
            raise common.GateFailure("allocator_runtime_failed")
        self.physical_boots = self.c_abi_runtime_checks = self.atomic_runtime_checks = self.allocator_runtime_checks = self.idle_checks = 1

    def run_panic(self, build: Path) -> None:
        self.flash(build)
        self.serial_exchange("status", "postflash", {"idle"}, 5)
        self.serial_exchange("run", "panic", {"boot", "panic_trigger"}, 10)
        self.deliberate_panics_count = 1

    def cleanup_device(self, normal: Path) -> None:
        if not self.device_touched:
            return
        saved_cancelled = self.cancelled
        saved_deadline = self.deadline
        self.cancelled = False
        self.deadline = time.monotonic() + 10
        try:
            self.flash(normal)
            self.serial_exchange("status", "cleanup", {"idle"}, max(0.1, self.deadline - time.monotonic()))
            self.cleanup_status = "success"
            self.device_state = "test_firmware_idle"
            self.idle_checks = 1
        except (common.GateFailure, OSError, subprocess.SubprocessError, termios.error):
            self.cleanup_status = "failed"
            self.device_state = "unknown"
        finally:
            self.cancelled = saved_cancelled
            self.deadline = saved_deadline

    def verify_linker(self, elf: Path, linker_map: Path) -> None:
        prefix = self.state / "sdk/gnu/xtensa-espressif_esp32s3_zephyr-elf/bin/xtensa-espressif_esp32s3_zephyr-elf-"
        header = self.command("probe", [f"{prefix}readelf", "-h", str(elf)], TARGET)
        sections = self.command("probe", [f"{prefix}readelf", "-S", str(elf)], TARGET)
        symbols = self.command("probe", [f"{prefix}nm", "-g", "--defined-only", str(elf)], TARGET)
        required = ("deskkin_c_multiply", "deskkin_c_to_rust_check", "deskkin_rust_add", "rust_main")
        if "Class:                             ELF32" not in header or "Machine:                           Tensilica Xtensa Processor" not in header:
            raise common.GateFailure("target_attributes_failed")
        if not all(re.search(rf"\b{re.escape(name)}$", symbols, re.MULTILINE) for name in required):
            raise common.GateFailure("abi_symbols_missing")
        if len(re.findall(r"\b__muldi3$", symbols, re.MULTILINE)) != 1:
            raise common.GateFailure("compiler_builtins_ownership_failed")
        if not linker_map.is_file() or not all(re.search(rf"\]\s+{re.escape(name)}\s+", sections) for name in (".text", ".dram0.data", ".dram0.bss")):
            raise common.GateFailure("memory_placement_failed")
        self.linker_checks = self.abi_symbols = self.builtins_checks = 1

    def execute(self, requested_device: str | None) -> None:
        root = self.state / "build/gate1c" / self.run_id
        normal = root / "normal"
        self.configure(normal)
        first = self.build_image(normal)
        self.builds = 1
        self.verify_inputs()
        shutil.rmtree(normal)
        self.configure(normal)
        second = self.build_image(normal)
        if first != second:
            raise common.GateFailure("clean_rebuild_digest_mismatch")
        self.rebuilds = 1
        self.normal_image_sha256 = common.sha256(normal / "zephyr/zephyr.bin")
        panic = root / "panic"
        self.configure(panic, panic=True)
        self.build_image(panic)
        self.panic_builds = 1
        self.verify_inputs()
        self.device = self.discover_device(requested_device)
        self.preflight()
        try:
            self.run_normal(normal)
            self.run_panic(panic)
        finally:
            self.cleanup_device(normal)

    def run(self, requested_device: str | None = None) -> tuple[int, dict[str, object]]:
        result, reason, code = "pass", "all_criteria_passed", 0
        try:
            self.prepare()
            self.execute(requested_device)
        except common.GateCancelled:
            result, reason, code = "inconclusive", "cancelled", 130
        except common.GateTimeout:
            result, reason, code = "inconclusive", "deadline_exceeded", 124
        except common.GateInconclusive as error:
            result, reason, code = "inconclusive", error.reason, 2
        except common.GateFailure as error:
            result, reason, code = "fail", error.reason, 1
        except (OSError, subprocess.SubprocessError) as error:
            result, reason, code = "inconclusive", f"setup_{type(error).__name__.lower()}", 2
        if self.cleanup_status != "success" and code not in (124, 130):
            result, reason, code = "inconclusive", "device_cleanup_failed", 2
        values = (("xtensa_builds", self.builds), ("clean_rebuilds", self.rebuilds), ("panic_builds", self.panic_builds), ("linker_checks", self.linker_checks), ("abi_symbols", self.abi_symbols), ("compiler_builtins_checks", self.builtins_checks), ("physical_boots", self.physical_boots), ("c_abi_runtime_checks", self.c_abi_runtime_checks), ("atomic_runtime_checks", self.atomic_runtime_checks), ("allocator_runtime_checks", self.allocator_runtime_checks), ("idle_checks", self.idle_checks), ("deliberate_panics", self.deliberate_panics_count))
        value: dict[str, object] = {"schema_version": 1, "gate": "1c", "mode": "default", "run_id": self.run_id, "result": result, "reason_code": reason, "cleanup_status": self.cleanup_status, "device_state": self.device_state, "firmware_digest": self.firmware_digest, "criteria": [{"name": name, "value": value, "unit": "count", "threshold": 1, "passed": value == 1} for name, value in values], "started_at": self.started, "ended_at": common.utc_now()}
        if self.normal_image_sha256 is not None:
            value["firmware_image_sha256"] = self.normal_image_sha256
        return code, value

    def recover(self, expected_firmware: str, requested_device: str | None = None) -> tuple[int, str]:
        normal = self.state / "build/gate1c" / self.run_id / "normal"
        try:
            self.prepare()
            if expected_firmware != self.firmware_digest:
                raise common.GateInconclusive("expected_firmware_mismatch")
            self.configure(normal)
            self.build_image(normal)
            self.verify_inputs()
            self.device = self.discover_device(requested_device)
            self.flash(normal)
            self.serial_exchange("status", "recover", {"idle"}, 10)
            self.device_state = "test_firmware_idle"
            self.cleanup_status = "success"
            return 0, "recovered"
        except common.GateCancelled:
            return 130, "cancelled"
        except common.GateTimeout:
            return 124, "deadline_exceeded"
        except common.GateFailure as error:
            return 2, error.reason
        except (OSError, subprocess.SubprocessError, termios.error):
            return 2, "recovery_failed"
        finally:
            if self.device_touched and self.device_state != "test_firmware_idle":
                self.cleanup_device(normal)


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("action", choices=("run", "recover"), nargs="?", default="run")
    parser.add_argument("--recording", choices=("on", "off"), default="on")
    parser.add_argument("--device")
    parser.add_argument("--expected-firmware")
    args = parser.parse_args()
    root = Path(__file__).resolve().parents[1]
    run_id = str(uuid.uuid4())
    print(run_id, flush=True)
    try:
        common.prepare_control_state(root, "1c")
        runner = Runner(root, args.recording == "on", run_id)
        if args.action == "recover":
            runner.verified_resources["mode"] = "recover"
        lock = acquire_lock(runner.state, run_id)
    except (OSError, common.GateInconclusive) as error:
        print(f"Gate 1C could not start: {error}", file=sys.stderr)
        return 2
    result_path = runner.state / "results/1c/default/result.json"
    pending_path = result_path.with_name(".result.pending")
    signal.signal(signal.SIGINT, lambda _signal, _frame: setattr(runner, "cancelled", True))
    with lock:
        recording_mode = "recover" if args.action == "recover" else "default"
        common.initialize_recording(root, runner.recorder, "1c", [TARGET], recording_mode)
        if args.action == "recover":
            if args.expected_firmware is None or re.fullmatch(DIGEST_PATTERN, args.expected_firmware) is None:
                print(f"Device recovery could not start: expected_firmware_required; run_id={run_id}", file=sys.stderr)
                return 2
            code, reason = runner.recover(args.expected_firmware, args.device)
            outcome = "pass" if code == 0 else "inconclusive"
            runner.recorder.finalize(runner.resources(), runner.links, outcome, reason)
            next_action = "power_cycle_or_approved_reflash" if runner.device_state == "unknown" else "none"
            print(f"Device recovery {outcome} ({reason}); device_state={runner.device_state}; required_action={next_action}; run_id={run_id}")
            return code
        if not common.clear_result(pending_path):
            return 2
        code, result = runner.run(args.device)
        if not common.publish_result(pending_path, result):
            return 2
        runner.recorder.finalize(runner.resources(), runner.links, result["result"], result["reason_code"])
        try:
            pending_path.replace(result_path)
        except OSError:
            pending_path.unlink(missing_ok=True)
            return 2
    if code == 2:
        print(f"Gate 1C inconclusive: {result['reason_code']}; run_id={run_id}", file=sys.stderr)
    print(f"Gate 1C {result['result']} ({result['reason_code']})")
    print(result_path.relative_to(root))
    return code


if __name__ == "__main__":
    raise SystemExit(main())
