#!/usr/bin/env python3
"""Bounded Gate 1D CoreS3 Zephyr board-support feasibility runner."""

from __future__ import annotations

import argparse
import hashlib
import os
import re
import selectors
import shutil
import signal
import subprocess
import sys
import termios
import time
import uuid
from pathlib import Path

import gate1c_runner as gate1c
import gate_runner as common

TARGET = gate1c.TARGET
WEST_REVISIONS = gate1c.WEST_REVISIONS
TOOL_DIGESTS = {"xtensa_gcc": gate1c.TOOL_DIGESTS["xtensa_gcc"]}
RF_BLOB_DIGESTS = {
    "libcore.a": "01e5e7540db58e46d4d9144c5003a00588f146dcee786f22d5ba822b6727243d",
    "libnet80211.a": "ab462592d9c3533e67064fea500f6f8d54c5222bffbe7eaabd1902aeec5dab04",
    "libpp.a": "07c4eeb8198f35a600878e131bed20b43e7463fc29b0986acacd79ba4d46b655",
    "libphy.a": "29dcc18a035801bd41486d5d59f3caeb62f8ed153533c488828bffef1abb99fd",
    "libcoexist.a": "fc370f917302c0da9437063ba76113e9102bc8af4a6985aa9ba2d2c09231dfbd",
}
PREVIOUS_GATE1D_FIRMWARE_DIGEST = "994f4902ba0363a101314bd62128f931382dc0ef7210e15f65702cb1c3ff9758"
INPUTS = (
    "west.yml",
    "mise.toml",
    "mise.lock",
    "requirements/core-s3.lock",
    "scripts/bootstrap_gate1c.sh",
    "scripts/gate_runner.py",
    "scripts/gate1c_runner.py",
    "scripts/gate1d_runner.py",
    "gates/gate1d/CMakeLists.txt",
    "gates/gate1d/prj.conf",
    "gates/gate1d/src/main.c",
)
FIRMWARE_INPUTS = (
    "gates/gate1d/CMakeLists.txt",
    "gates/gate1d/prj.conf",
    "gates/gate1d/src/main.c",
)
UUID_PATTERN = gate1c.UUID_PATTERN
DIGEST_PATTERN = gate1c.DIGEST_PATTERN
SERIAL_PATTERNS = {
    "idle": re.compile(rf"^DESKKIN_GATE_EVENT schema=1 event=idle run_id=(?P<run_id>{UUID_PATTERN}) firmware_digest=(?P<firmware_digest>{DIGEST_PATTERN})$"),
    "boot": re.compile(rf"^DESKKIN_GATE_EVENT schema=1 event=boot run_id=(?P<run_id>{UUID_PATTERN}) board=(?P<board>m5stack_cores3) firmware_digest=(?P<firmware_digest>{DIGEST_PATTERN})$"),
    "devices": re.compile(rf"^DESKKIN_GATE_EVENT schema=1 event=devices run_id=(?P<run_id>{UUID_PATTERN}) power=(?P<power>ok) gpio=(?P<gpio>ok) display=(?P<display>ok) touch=(?P<touch>ok) flash=(?P<flash>ok) i2c0=(?P<i2c0>ok) i2c1=(?P<i2c1>ok) spi2=(?P<spi2>ok) width=(?P<width>[0-9]+) height=(?P<height>[0-9]+) format=(?P<format>rgb565)$"),
    "wifi": re.compile(rf"^DESKKIN_GATE_EVENT schema=1 event=wifi run_id=(?P<run_id>{UUID_PATTERN}) status=(?P<status>ready|not_ready)$"),
    "psram": re.compile(rf"^DESKKIN_GATE_EVENT schema=1 event=psram run_id=(?P<run_id>{UUID_PATTERN}) bytes=(?P<bytes>[0-9]+) status=(?P<status>ok)$"),
    "flash_read": re.compile(rf"^DESKKIN_GATE_EVENT schema=1 event=flash_read run_id=(?P<run_id>{UUID_PATTERN}) bytes=(?P<bytes>[0-9]+) status=(?P<status>ok)$"),
    "display_rect": re.compile(rf"^DESKKIN_GATE_EVENT schema=1 event=display_rect run_id=(?P<run_id>{UUID_PATTERN}) index=(?P<index>[0-9]+) x=(?P<x>[0-9]+) y=(?P<y>[0-9]+) width=(?P<width>[0-9]+) height=(?P<height>[0-9]+) bytes=(?P<bytes>[0-9]+) duration_us=(?P<duration_us>[0-9]+) status=(?P<status>ok)$"),
    "panel": re.compile(rf"^DESKKIN_GATE_EVENT schema=1 event=panel run_id=(?P<run_id>{UUID_PATTERN}) pattern=(?P<pattern>rgb_rectangles) status=(?P<status>ready)$"),
    "touch": re.compile(rf"^DESKKIN_GATE_EVENT schema=1 event=touch run_id=(?P<run_id>{UUID_PATTERN}) index=(?P<index>[0-9]+) x=(?P<x>[0-9]+) y=(?P<y>[0-9]+) status=(?P<status>ok)$"),
    "touch_sample": re.compile(rf"^DESKKIN_GATE_EVENT schema=1 event=touch_sample run_id=(?P<run_id>{UUID_PATTERN}) expected_index=(?P<expected_index>[0-9]+) x=(?P<x>[0-9]+) y=(?P<y>[0-9]+) inside=(?P<inside>yes|no)$"),
    "result": re.compile(rf"^DESKKIN_GATE_RESULT schema=1 run_id=(?P<run_id>{UUID_PATTERN}) result=(?P<result>pass|fail)$"),
}
INTEGER_FIELDS = {
    "width", "height", "bytes", "duration_us", "index", "expected_index", "x", "y"
}
RECTANGLES = (
    {"index": 1, "x": 20, "y": 20, "width": 80, "height": 60, "bytes": 9600},
    {"index": 2, "x": 120, "y": 90, "width": 80, "height": 60, "bytes": 9600},
    {"index": 3, "x": 220, "y": 160, "width": 80, "height": 60, "bytes": 9600},
)


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
        record: dict[str, object] = {
            "schema_version": 1,
            "target": target,
            "mode": mode,
            "event": event,
        }
        for key, value in match.groupdict().items():
            record[key] = int(value) if key in INTEGER_FIELDS else value
        return record
    return None


class Runner(gate1c.Runner):
    def __init__(self, root: Path, recording: bool, run_id: str, firmware_identity: str | None = None):
        super().__init__(root, recording, run_id, firmware_identity or firmware_digest(root))
        self.deadline = time.monotonic() + 600
        self.verified_resources.update({"gate": "1d", "target": [TARGET]})
        self.builds = self.rebuilds = self.linker_checks = 0
        self.physical_boots = self.device_checks = self.psram_checks = 0
        self.wifi_checks = 0
        self.flash_checks = self.display_rect_checks = self.touch_checks = 0
        self.panel_checks = self.idle_checks = 0

    def _environment(self) -> dict[str, str]:
        cmake = Path(subprocess.run(["mise", "where", "cmake"], check=True, capture_output=True, text=True).stdout.strip()) / "bin"
        ninja = Path(subprocess.run(["mise", "where", "ninja"], check=True, capture_output=True, text=True).stdout.strip())
        env = os.environ.copy()
        env.update({
            "PATH": os.pathsep.join((str(cmake), str(ninja), str(self.state / "venv/bin"), env["PATH"])),
            "ZEPHYR_SDK_INSTALL_DIR": str(self.state / "sdk"),
            "ZEPHYR_TOOLCHAIN_VARIANT": "zephyr",
            "ZEPHYR_BASE": str(self.state / "west/zephyr"),
            "SOURCE_DATE_EPOCH": "0",
        })
        return env

    def _tool_paths(self) -> dict[str, Path]:
        return {
            "xtensa_gcc": self.state / "sdk/gnu/xtensa-espressif_esp32s3_zephyr-elf/bin/xtensa-espressif_esp32s3_zephyr-elf-gcc",
        }

    def _verify_rf_blobs(self) -> None:
        directory = self.state / "west/modules/hal/espressif/zephyr/blobs/lib/esp32s3"
        if any(
            not (path := directory / name).is_file() or common.sha256(path) != digest
            for name, digest in RF_BLOB_DIGESTS.items()
        ):
            raise common.GateInconclusive("rf_blob_mismatch")

    def prepare(self) -> None:
        started = time.monotonic()
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
            self._verify_west_trees("west_project_dirty")
            probes = {
                "gcc": self.command("prepare", [str(self._tool_paths()["xtensa_gcc"]), "--version"], "host").splitlines()[0],
                "cmake": self.command("prepare", ["cmake", "--version"], "host").splitlines()[0],
                "ninja": self.command("prepare", ["ninja", "--version"], "host").strip(),
                "python": self.command("prepare", [str(self.state / "venv/bin/python"), "--version"], "host").strip(),
            }
            expected_tools = {"gcc": "14.3.0", "cmake": "3.28.6", "ninja": "1.13.2", "python": "3.12"}
            if any(fragment not in probes[name] for name, fragment in expected_tools.items()):
                raise common.GateInconclusive("host_tool_mismatch")
            if any(common.sha256(self._tool_paths()[name]) != digest for name, digest in TOOL_DIGESTS.items()):
                raise common.GateInconclusive("tool_digest_mismatch")
            self._verify_rf_blobs()
            revision = self.command("prepare", ["git", "-C", str(self.root), "rev-parse", "HEAD"], "host").strip()
            self.verified_resources.update({
                "application_version": "gate1d-0.1.0",
                "build_type": "dev",
                "deskkin_revision": revision,
                "deskkin_dirty": bool(self.command("prepare", ["git", "-C", str(self.root), "status", "--porcelain"], "host").strip()),
                "west_revisions": actual,
                "sdk_file_digests": dict(TOOL_DIGESTS),
                "rf_blob_digests": dict(RF_BLOB_DIGESTS),
                "sdk_version": "1.0.1",
                "tool_identities": probes,
                "input_digests": {name: common.sha256(self.root / name) for name in INPUTS},
            })
            self.recorder._append({"schema_version": 1, "type": "resource_verified", **self.verified_resources})
            self.recorder.event("prepare", "success", round((time.monotonic() - started) * 1000))
        except common.GateFailure:
            self.recorder.event("prepare", "error", round((time.monotonic() - started) * 1000), "provenance_mismatch")
            raise
        except (OSError, subprocess.SubprocessError, KeyError) as error:
            self.recorder.event("prepare", "error", round((time.monotonic() - started) * 1000), "setup_probe_failed")
            raise common.GateInconclusive("setup_probe_failed") from error

    def verify_inputs(self) -> None:
        if any(common.sha256(self.root / name) != digest for name, digest in self.verified_resources["input_digests"].items()):
            raise common.GateInconclusive("input_changed")
        for name, expected in WEST_REVISIONS.items():
            if self.command("prepare", ["git", "-C", str(self.west_paths[name]), "rev-parse", "HEAD"], "host").strip() != expected:
                raise common.GateInconclusive("input_changed")
        self._verify_west_trees("input_changed")
        if any(common.sha256(self._tool_paths()[name]) != digest for name, digest in TOOL_DIGESTS.items()):
            raise common.GateInconclusive("input_changed")
        self._verify_rf_blobs()

    def _verify_west_trees(self, reason: str) -> None:
        for name, path in self.west_paths.items():
            status = self.command(
                "prepare", ["git", "-C", str(path), "status", "--porcelain"], "host"
            ).splitlines()
            if name == "zephyr-lang-rust":
                expected_status = [
                    " M CMakeLists.txt", " M Kconfig", " M dt-rust.yaml",
                    " M zephyr-build/src/lib.rs",
                ]
                diff = subprocess.run(
                    ["git", "-C", str(path), "diff", "--binary", "HEAD"],
                    check=True,
                    capture_output=True,
                ).stdout
                if status != expected_status or hashlib.sha256(diff).hexdigest() != gate1c.PATCH_DIFF_SHA256:
                    raise common.GateInconclusive(reason)
            elif status:
                raise common.GateInconclusive(reason)

    def configure(self, build: Path, panic: bool = False) -> None:
        del panic
        self.command("configure", [
            str(self.state / "venv/bin/west"), "build", "--cmake-only", "--board", TARGET,
            "--build-dir", str(build), str(self.root / "gates/gate1d"), "--",
            f"-DDESKKIN_FIRMWARE_DIGEST={self.firmware_digest}",
        ], TARGET)

    def build_image(self, build: Path) -> str:
        self.command("c-compile", ["cmake", "--build", str(build), "--target", "zephyr_pre0"], TARGET)
        self.command("link", ["cmake", "--build", str(build), "--target", "zephyr_final"], TARGET)
        elf = build / "zephyr/zephyr.elf"
        digest = common.sha256(elf)
        self.links.append({"target": TARGET, "mode": build.name, "sha256": digest, "bytes": elf.stat().st_size})
        self.verify_linker(elf, build / "zephyr/zephyr.map", build / "zephyr/.config")
        return digest

    def verify_linker(self, elf: Path, linker_map: Path, config: Path) -> None:
        prefix = self.state / "sdk/gnu/xtensa-espressif_esp32s3_zephyr-elf/bin/xtensa-espressif_esp32s3_zephyr-elf-"
        header = self.command("probe", [f"{prefix}readelf", "-h", str(elf)], TARGET)
        sections = self.command("probe", [f"{prefix}readelf", "-S", str(elf)], TARGET)
        symbols = self.command("probe", [f"{prefix}nm", "--defined-only", str(elf)], TARGET)
        required_symbols = (
            "main",
            "flash_esp32_read",
            "ft5336_init",
            "gpio_aw9523b_init",
            "ili9xxx_write",
            "mfd_axp2101_init",
            "esp32_wifi_dev_init",
            "shared_multi_heap_aligned_alloc",
        )
        required_config = ("CONFIG_BOARD_M5STACK_CORES3=y", "CONFIG_DISPLAY=y", "CONFIG_ESP_SPIRAM=y", "CONFIG_INPUT=y", "CONFIG_WIFI_ESP32=y")
        if "Class:                             ELF32" not in header or "Machine:                           Tensilica Xtensa Processor" not in header:
            raise common.GateFailure("target_attributes_failed")
        if not all(re.search(rf"\b{re.escape(name)}$", symbols, re.MULTILINE) for name in required_symbols):
            raise common.GateFailure("board_symbols_missing")
        if not linker_map.is_file() or not all(re.search(rf"\]\s+{re.escape(name)}\s+", sections) for name in (".text", ".dram0.data", ".dram0.bss", ".ext_ram.data")):
            raise common.GateFailure("memory_placement_failed")
        configured = config.read_text(encoding="utf-8")
        if not all(value in configured for value in required_config):
            raise common.GateFailure("board_configuration_failed")
        self.linker_checks = 1

    def serial_exchange(self, action: str, mode: str, required: set[str], timeout: float, allowed_digests: set[str] | None = None) -> list[dict[str, object]]:
        if self.device is None:
            raise common.GateInconclusive("device_unavailable")
        started = time.monotonic()
        descriptor: int | None = None
        selector: selectors.BaseSelector | None = None
        records: list[dict[str, object]] = []
        pending = b""
        allowed = allowed_digests or {self.firmware_digest}
        operation = "boot" if mode == "normal" else mode
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
                        record = parse_serial_record(clean, TARGET, mode)
                        if record is None or record.get("run_id") != self.run_id:
                            continue
                        if "firmware_digest" in record and record["firmware_digest"] not in allowed:
                            raise common.GateInconclusive("firmware_digest_mismatch")
                        records.append(record)
                        self.recorder.serial.append(record)
                if required <= {str(record["event"]) for record in records}:
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

    def preflight(self) -> None:
        recognized = {
            self.firmware_digest,
            gate1c.firmware_digest(self.root),
            PREVIOUS_GATE1D_FIRMWARE_DIGEST,
        }
        previous = common.read_result_safe(
            self.state / "results/1d/default/result.json", "1d", "default"
        )
        if (
            previous is not None
            and previous.get("cleanup_status") == "success"
            and previous.get("device_state") == "test_firmware_idle"
            and isinstance(previous.get("firmware_digest"), str)
            and re.fullmatch(DIGEST_PATTERN, previous["firmware_digest"])
        ):
            recognized.add(previous["firmware_digest"])
        try:
            self.serial_exchange("status", "preflight", {"idle"}, 3, recognized)
        except (common.GateCancelled, common.GateTimeout):
            raise
        except (common.GateFailure, OSError, termios.error) as error:
            raise common.GateInconclusive("device_state_unknown") from error

    def run_board_probe(self, build: Path) -> None:
        self.flash(build)
        self.serial_exchange("status", "postflash", {"idle"}, 5)
        records = self.serial_exchange(
            "run", "normal",
            {"boot", "devices", "wifi", "psram", "flash_read", "display_rect", "panel", "touch", "result", "idle"},
            120,
        )
        result = [record for record in records if record["event"] == "result"]
        rects = [record for record in records if record["event"] == "display_rect"]
        touches = [record for record in records if record["event"] == "touch"]
        devices = next(record for record in records if record["event"] == "devices")
        wifi = next(record for record in records if record["event"] == "wifi")
        psram = next(record for record in records if record["event"] == "psram")
        flash = next(record for record in records if record["event"] == "flash_read")
        if len(result) != 1 or result[0].get("result") != "pass":
            raise common.GateFailure("board_runtime_failed")
        if devices.get("width") != 320 or devices.get("height") != 240:
            raise common.GateFailure("display_capabilities_failed")
        if wifi.get("status") != "ready":
            raise common.GateFailure("wifi_device_not_ready")
        if psram.get("bytes") != 32768 or flash.get("bytes") != 32:
            raise common.GateFailure("memory_runtime_failed")
        if len(rects) != 3 or any(any(record.get(key) != value for key, value in expected.items()) or record.get("duration_us", 0) <= 0 for record, expected in zip(rects, RECTANGLES, strict=True)):
            raise common.GateFailure("partial_display_failed")
        touches_valid = len(touches) == 3 and all(
            record.get("index") == rect["index"]
            and rect["x"] <= record.get("x", -1) < rect["x"] + rect["width"]
            and rect["y"] <= record.get("y", -1) < rect["y"] + rect["height"]
            for record, rect in zip(touches, RECTANGLES, strict=True)
        )
        if not touches_valid:
            raise common.GateFailure("touch_sequence_failed")
        self.physical_boots = self.device_checks = self.psram_checks = 1
        self.wifi_checks = 1
        self.flash_checks = self.panel_checks = self.idle_checks = 1
        self.display_rect_checks = self.touch_checks = 3

    def execute(self, requested_device: str | None) -> None:
        build = self.state / "build/gate1d" / self.run_id / "normal"
        self.configure(build)
        first = self.build_image(build)
        self.builds = 1
        self.verify_inputs()
        shutil.rmtree(build)
        self.configure(build)
        second = self.build_image(build)
        if first != second:
            raise common.GateFailure("clean_rebuild_digest_mismatch")
        self.rebuilds = 1
        self.normal_image_sha256 = common.sha256(build / "zephyr/zephyr.bin")
        self.verify_inputs()
        self.device = self.discover_device(requested_device)
        self.preflight()
        try:
            self.run_board_probe(build)
        finally:
            self.cleanup_device(build)

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
        values = (
            ("xtensa_builds", self.builds, 1),
            ("clean_rebuilds", self.rebuilds, 1),
            ("linker_checks", self.linker_checks, 1),
            ("physical_boots", self.physical_boots, 1),
            ("device_initialization_checks", self.device_checks, 1),
            ("wifi_readiness_checks", self.wifi_checks, 1),
            ("psram_checks", self.psram_checks, 1),
            ("flash_read_checks", self.flash_checks, 1),
            ("partial_display_rectangles", self.display_rect_checks, 3),
            ("touch_rectangles", self.touch_checks, 3),
            ("deterministic_panel_checks", self.panel_checks, 1),
            ("idle_checks", self.idle_checks, 1),
        )
        value: dict[str, object] = {
            "schema_version": 1,
            "gate": "1d",
            "mode": "default",
            "run_id": self.run_id,
            "result": result,
            "reason_code": reason,
            "cleanup_status": self.cleanup_status,
            "device_state": self.device_state,
            "firmware_digest": self.firmware_digest,
            "criteria": [
                {"name": name, "value": count, "unit": "count", "threshold": threshold, "passed": count == threshold}
                for name, count, threshold in values
            ],
            "started_at": self.started,
            "ended_at": common.utc_now(),
        }
        if self.normal_image_sha256 is not None:
            value["firmware_image_sha256"] = self.normal_image_sha256
        return code, value

    def recover(self, expected_firmware: str, requested_device: str | None = None) -> tuple[int, str]:
        build = self.state / "build/gate1d" / self.run_id / "normal"
        try:
            self.prepare()
            if expected_firmware != self.firmware_digest:
                raise common.GateInconclusive("expected_firmware_mismatch")
            self.configure(build)
            self.build_image(build)
            self.verify_inputs()
            self.device = self.discover_device(requested_device)
            self.flash(build)
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
                self.cleanup_device(build)


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
        common.prepare_control_state(root, "1d")
        runner = Runner(root, args.recording == "on", run_id)
        if args.action == "recover":
            runner.verified_resources["mode"] = "recover"
        lock = gate1c.acquire_lock(runner.state, run_id, "1d")
    except (OSError, common.GateInconclusive) as error:
        print(f"Gate 1D could not start: {error}", file=sys.stderr)
        return 2
    result_path = runner.state / "results/1d/default/result.json"
    pending_path = result_path.with_name(".result.pending")
    signal.signal(signal.SIGINT, lambda _signal, _frame: setattr(runner, "cancelled", True))
    with lock:
        recording_mode = "recover" if args.action == "recover" else "default"
        common.initialize_recording(root, runner.recorder, "1d", [TARGET], recording_mode)
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
        print(f"Gate 1D inconclusive: {result['reason_code']}; run_id={run_id}", file=sys.stderr)
    print(f"Gate 1D {result['result']} ({result['reason_code']})")
    print(result_path.relative_to(root))
    return code


if __name__ == "__main__":
    raise SystemExit(main())
