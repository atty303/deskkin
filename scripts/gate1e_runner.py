#!/usr/bin/env python3
"""Bounded Gate 1E combined CoreS3 Slint feasibility runner."""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import queue
import re
import selectors
import shutil
import signal
import subprocess
import sys
import termios
import threading
import time
import uuid
from pathlib import Path

import gate1c_runner as gate1c
import gate1d_runner as gate1d
import gate_runner as common

TARGET = gate1c.TARGET
UUID_PATTERN = gate1c.UUID_PATTERN
DIGEST_PATTERN = gate1c.DIGEST_PATTERN
SHORT_DIGEST_PATTERN = r"[0-9a-f]{16}"
SAMPLES = 1_740
FRAMES = 1_800
TOUCHES = 60
MAX_DIRTY_PIXELS = 19_200

GATE_INPUTS = (
    "mise.toml",
    "scripts/gate_runner.py",
    "scripts/gate1c_runner.py",
    "scripts/gate1d_runner.py",
    "scripts/gate1e_runner.py",
    "gates/gate1e/CMakeLists.txt",
    "gates/gate1e/Cargo.toml",
    "gates/gate1e/Cargo.lock",
    "gates/gate1e/Kconfig",
    "gates/gate1e/prj.conf",
    "gates/gate1e/build.rs",
    "gates/gate1e/ui/gate.slint",
    "gates/gate1e/src/adapter.c",
    "gates/gate1e/src/lib.rs",
)
FIRMWARE_INPUTS = tuple(name for name in GATE_INPUTS if name.startswith("gates/gate1e/"))

SERIAL_PATTERNS = {
    "idle": re.compile(
        rf"^DESKKIN_GATE_EVENT schema=1 event=idle run_id=(?P<run_id>{UUID_PATTERN}) "
        rf"firmware_digest=(?P<firmware_digest>{DIGEST_PATTERN})$"
    ),
    "boot": re.compile(
        rf"^DESKKIN_GATE_EVENT schema=1 event=boot run_id=(?P<run_id>{UUID_PATTERN}) "
        rf"mode=(?P<run_mode>qualification|conformance) firmware_digest=(?P<firmware_digest>{DIGEST_PATTERN}) "
        rf"workload_digest=(?P<workload_digest>{DIGEST_PATTERN})$"
    ),
    "summary": re.compile(
        rf"^DESKKIN_GATE1E_SUMMARY schema=1 run_id=(?P<run_id>{UUID_PATTERN}) "
        r"phase=(?P<phase>disabled|enabled) frames=(?P<frames>[0-9]+) samples=(?P<samples>[0-9]+) "
        r"render_p95_us=(?P<render_p95_us>[0-9]+) transfer_p95_us=(?P<transfer_p95_us>[0-9]+) "
        r"combined_p95_us=(?P<combined_p95_us>[0-9]+) combined_p99_us=(?P<combined_p99_us>[0-9]+) "
        r"touch_p95_us=(?P<touch_p95_us>[0-9]+) missed_frames=(?P<missed_frames>[0-9]+) "
        r"max_dirty_pixels=(?P<max_dirty_pixels>[0-9]+) post_initial_full_frames=(?P<post_initial_full_frames>[0-9]+) "
        rf"touches=(?P<touches>[0-9]+) semantic_digest=(?P<semantic_digest>{SHORT_DIGEST_PATTERN}) "
        rf"framebuffer_digest=(?P<framebuffer_digest>{SHORT_DIGEST_PATTERN})$"
    ),
    "frame": re.compile(
        rf"^DESKKIN_GATE1E_FRAME schema=1 run_id=(?P<run_id>{UUID_PATTERN}) phase=(?P<phase>enabled) "
        r"frame=(?P<frame>[0-9]+) render_us=(?P<render_us>[0-9]+) transfer_us=(?P<transfer_us>[0-9]+) "
        r"combined_us=(?P<combined_us>[0-9]+) dirty_pixels=(?P<dirty_pixels>[0-9]+) "
        r"touch_latency_us=(?P<touch_latency_us>[0-9]+) missed=(?P<missed>yes|no)$"
    ),
    "runtime_error": re.compile(
        rf"^DESKKIN_GATE1E_ERROR schema=1 run_id=(?P<run_id>{UUID_PATTERN}) "
        r"phase=(?P<phase>disabled|enabled) "
        r"error_type=(?P<error_type>touch_inject_failed|touch_callback_timeout|display_write_failed|initial_frame_incomplete|display_enable_failed|dirty_pixel_limit_exceeded|workload_count_mismatch) "
        r"frame=(?P<frame>[0-9]+)$"
    ),
    "result": re.compile(
        rf"^DESKKIN_GATE_RESULT schema=1 run_id=(?P<run_id>{UUID_PATTERN}) result=(?P<result>pass|fail)$"
    ),
}
INTEGER_FIELDS = {
    "frames",
    "samples",
    "render_p95_us",
    "transfer_p95_us",
    "combined_p95_us",
    "combined_p99_us",
    "touch_p95_us",
    "missed_frames",
    "max_dirty_pixels",
    "post_initial_full_frames",
    "touches",
    "frame",
    "render_us",
    "transfer_us",
    "combined_us",
    "dirty_pixels",
    "touch_latency_us",
}


def digest_files(root: Path, paths: tuple[str, ...]) -> str:
    digest = hashlib.sha256()
    for relative in paths:
        encoded = relative.encode()
        payload = (root / relative).read_bytes()
        digest.update(len(encoded).to_bytes(4, "big"))
        digest.update(encoded)
        digest.update(len(payload).to_bytes(8, "big"))
        digest.update(payload)
    return digest.hexdigest()


def firmware_digest(root: Path) -> str:
    return digest_files(root, FIRMWARE_INPUTS)


def workload_digest(root: Path, firmware: str) -> str:
    identity = {
        "schema_version": 1,
        "target": TARGET,
        "firmware_digest": firmware,
        "clock_hz": 10_000,
        "width": 320,
        "height": 240,
        "pixel_format": "rgb565",
        "cadence_hz": 30,
        "frames": FRAMES,
        "warmup_frames": 60,
        "animation_seed": 0,
        "touch_period_frames": 30,
        "expression_behavior": "toggle_on_each_touch",
        "ui_sha256": common.sha256(root / "gates/gate1e/ui/gate.slint"),
        "config_sha256": common.sha256(root / "gates/gate1e/prj.conf"),
    }
    return hashlib.sha256(json.dumps(identity, sort_keys=True, separators=(",", ":")).encode()).hexdigest()


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


def nearest_rank(values: list[int], percentile: int) -> int:
    ordered = sorted(values)
    rank = (percentile * len(ordered) + 99) // 100
    return ordered[rank - 1]


class MeasurementWriter:
    def __init__(self, path: Path | None):
        self.path = path
        self.items: queue.Queue[dict[str, object] | None] = queue.Queue(maxsize=256)
        self.thread: threading.Thread | None = None
        self.error: str | None = None
        self.bytes_written = 0
        self.records_written = 0

    def start(self) -> None:
        if self.path is None:
            return
        self.thread = threading.Thread(target=self._run, name="gate1e-recorder", daemon=True)
        self.thread.start()

    def submit(self, value: dict[str, object]) -> None:
        if self.thread is None or self.error is not None:
            return
        try:
            self.items.put_nowait(value)
        except queue.Full:
            self.error = "recording_queue_full"

    def stop(self) -> bool:
        if self.thread is None:
            return self.path is None
        try:
            self.items.put(None, timeout=1)
        except queue.Full:
            self.error = "recording_queue_full"
        self.thread.join(timeout=5)
        if self.thread.is_alive():
            self.error = "recording_flush_timeout"
        return self.error is None

    def _run(self) -> None:
        assert self.path is not None
        try:
            with self.path.open("x", encoding="utf-8") as stream:
                os.chmod(self.path, 0o600)
                while True:
                    item = self.items.get()
                    if item is None:
                        break
                    encoded = json.dumps(item, sort_keys=True, separators=(",", ":")) + "\n"
                    size = len(encoded.encode())
                    if self.bytes_written + size > common.RUN_LIMIT_BYTES:
                        self.error = "recording_capacity_exceeded"
                        continue
                    stream.write(encoded)
                    self.bytes_written += size
                    self.records_written += 1
                    if self.records_written % 32 == 0:
                        stream.flush()
                    if self.records_written % 256 == 0:
                        os.fsync(stream.fileno())
                stream.flush()
                os.fsync(stream.fileno())
        except OSError:
            self.error = "recording_io_failure"


class Runner(gate1d.Runner):
    def __init__(self, root: Path, recording: bool, run_id: str, mode: str):
        identity = firmware_digest(root)
        super().__init__(root, recording, run_id, identity)
        self.deadline = time.monotonic() + 300
        self.mode = mode
        self.workload_identity = workload_digest(root, identity)
        self.verified_resources.update({"gate": "1e", "mode": mode, "target": [TARGET]})
        self.builds = self.rebuilds = self.linker_checks = 0
        self.physical_boots = self.idle_checks = 0
        self.normal_image_sha256: str | None = None
        self.summaries: list[dict[str, object]] = []
        self.frames: list[dict[str, object]] = []
        self.writer: MeasurementWriter | None = None

    def _environment(self) -> dict[str, str]:
        env = gate1c.Runner._environment(self)
        toolchain = self.state / "rustup/toolchains/deskkin-esp/bin"
        cmake = Path(
            subprocess.run(
                ["mise", "which", "cmake"], check=True, capture_output=True, text=True
            ).stdout.strip()
        ).parent
        ninja = Path(
            subprocess.run(
                ["mise", "which", "ninja"], check=True, capture_output=True, text=True
            ).stdout.strip()
        ).parent
        env["PATH"] = os.pathsep.join(
            (str(toolchain), str(cmake), str(ninja), str(self.state / "venv/bin"), env["PATH"])
        )
        return env

    def _tool_paths(self) -> dict[str, Path]:
        return gate1c.Runner._tool_paths(self)

    def prepare(self) -> None:
        gate1c.Runner.prepare(self)
        self.verified_resources.update(
            {
                "application_version": "gate1e-0.1.0",
                "input_digests": {
                    **self.verified_resources["input_digests"],
                    **{name: common.sha256(self.root / name) for name in GATE_INPUTS},
                },
            }
        )
        self.recorder._append({"schema_version": 1, "type": "resource_verified", **self.verified_resources})

    def verify_inputs(self) -> None:
        gate1c.Runner.verify_inputs(self)

    def configure(self, build: Path, panic: bool = False) -> None:
        del panic
        self.command(
            "configure",
            [
                str(self.state / "venv/bin/west"),
                "build",
                "--cmake-only",
                "--board",
                TARGET,
                "--build-dir",
                str(build),
                str(self.root / "gates/gate1e"),
                "--",
                f"-DDESKKIN_FIRMWARE_DIGEST={self.firmware_digest}",
                f"-DDESKKIN_WORKLOAD_DIGEST={self.workload_identity}",
            ],
            TARGET,
        )

    def build_image(self, build: Path) -> str:
        return gate1c.Runner.build_image(self, build)

    def verify_linker(self, elf: Path, linker_map: Path) -> None:
        prefix = self.state / "sdk/gnu/xtensa-espressif_esp32s3_zephyr-elf/bin/xtensa-espressif_esp32s3_zephyr-elf-"
        header = self.command("probe", [f"{prefix}readelf", "-h", str(elf)], TARGET)
        sections = self.command("probe", [f"{prefix}readelf", "-S", str(elf)], TARGET)
        symbols = self.command("probe", [f"{prefix}nm", "--defined-only", str(elf)], TARGET)
        config = (elf.parent / ".config").read_text(encoding="utf-8")
        required_symbols = (
            "rust_main",
            "deskkin_display_write",
            "deskkin_inject_touch",
            "ili9xxx_write",
            "input_report",
            "shared_multi_heap_aligned_alloc",
        )
        required_config = (
            "CONFIG_BOARD_M5STACK_CORES3=y",
            "CONFIG_DISPLAY=y",
            "CONFIG_ESP_SPIRAM=y",
            "CONFIG_INPUT=y",
            "CONFIG_RUST=y",
            "CONFIG_RUST_ALLOC=y",
        )
        if "Class:                             ELF32" not in header or "Machine:                           Tensilica Xtensa Processor" not in header:
            raise common.GateFailure("target_attributes_failed")
        if not all(re.search(rf"\b{re.escape(name)}$", symbols, re.MULTILINE) for name in required_symbols):
            raise common.GateFailure("combined_symbols_missing")
        if len(re.findall(r"\b__muldi3$", symbols, re.MULTILINE)) != 1:
            raise common.GateFailure("compiler_builtins_ownership_failed")
        if not linker_map.is_file() or not all(re.search(rf"\]\s+{re.escape(name)}\s+", sections) for name in (".text", ".dram0.data", ".dram0.bss", ".ext_ram.data")):
            raise common.GateFailure("memory_placement_failed")
        if not all(value in config for value in required_config):
            raise common.GateFailure("combined_configuration_failed")
        self.linker_checks = 1

    def preflight(self) -> None:
        recognized = {self.firmware_digest, gate1c.firmware_digest(self.root), gate1d.PREVIOUS_GATE1D_FIRMWARE_DIGEST}
        for gate, mode in (("1d", "default"), ("1e", "qualification"), ("1e", "conformance")):
            previous = common.read_result_safe(
                self.state / f"results/{gate}/{mode}/result.json", gate, mode
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

    def serial_exchange(
        self,
        action: str,
        mode: str,
        required: set[str],
        timeout: float,
        allowed_digests: set[str] | None = None,
    ) -> list[dict[str, object]]:
        if self.device is None:
            raise common.GateInconclusive("device_unavailable")
        started = time.monotonic()
        descriptor: int | None = None
        selector: selectors.BaseSelector | None = None
        records: list[dict[str, object]] = []
        pending = b""
        allowed = allowed_digests or {self.firmware_digest}
        try:
            descriptor = os.open(self.device, os.O_RDWR | os.O_NOCTTY | os.O_NONBLOCK)
            self._configure_serial(descriptor)
            selector = selectors.DefaultSelector()
            selector.register(descriptor, selectors.EVENT_READ)
            if action == "run":
                command = f"DESKKIN_GATE_COMMAND schema=1 action=run mode={mode} run_id={self.run_id}\n".encode()
            else:
                command = f"DESKKIN_GATE_COMMAND schema=1 action=status run_id={self.run_id}\n".encode()
            os.write(descriptor, command)
            next_retry = time.monotonic() + 0.25
            deadline = min(self.deadline, time.monotonic() + timeout)
            while time.monotonic() < deadline:
                if self.cancelled:
                    raise common.GateCancelled("cancelled")
                if action == "status" and time.monotonic() >= next_retry:
                    os.write(descriptor, command)
                    next_retry = time.monotonic() + 0.25
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
                        if record["event"] == "boot" and (
                            record.get("run_mode") != self.mode
                            or record.get("workload_digest") != self.workload_identity
                        ):
                            raise common.GateInconclusive("workload_identity_mismatch")
                        records.append(record)
                        if record["event"] == "frame":
                            self.frames.append(record)
                            if self.writer is not None:
                                self.writer.submit(record)
                        else:
                            self.recorder.serial.append(record)
                if required <= {str(record["event"]) for record in records}:
                    self.recorder.event("boot" if action == "run" else mode, "success", round((time.monotonic() - started) * 1000), target=TARGET)
                    return records
            if time.monotonic() >= self.deadline:
                self.recorder.event("boot" if action == "run" else mode, "timeout", round((time.monotonic() - started) * 1000), "deadline_exceeded", TARGET)
                raise common.GateTimeout("deadline_exceeded")
            last_event = str(records[-1]["event"]) if records else "none"
            reason = f"serial_protocol_timeout_after_{last_event}"
            self.recorder.event("boot" if action == "run" else mode, "error", round((time.monotonic() - started) * 1000), reason, TARGET)
            raise common.GateInconclusive(reason)
        except PermissionError as error:
            raise common.GateInconclusive("device_permission_denied") from error
        finally:
            if selector is not None:
                selector.close()
            if descriptor is not None:
                os.close(descriptor)

    def _validate_frame_records(self) -> None:
        if self.mode == "conformance":
            if self.frames:
                raise common.GateFailure("recording_opt_out_failed")
            return
        if len(self.frames) != SAMPLES or [record["frame"] for record in self.frames] != list(range(60, FRAMES)):
            raise common.GateFailure("recording_frame_sequence_failed")
        for record in self.frames:
            if record["combined_us"] != record["render_us"] + record["transfer_us"]:
                raise common.GateFailure("recording_measurement_invalid")
            if not 0 < record["dirty_pixels"] <= MAX_DIRTY_PIXELS:
                raise common.GateFailure("recording_dirty_range_invalid")

    def _validate_summaries(self, records: list[dict[str, object]]) -> dict[str, dict[str, object]]:
        runtime = [record for record in records if record["event"] == "result"]
        runtime_errors = [record for record in records if record["event"] == "runtime_error"]
        summaries = [record for record in records if record["event"] == "summary"]
        expected_phases = ["disabled", "enabled"] if self.mode == "qualification" else ["disabled"]
        if len(runtime) != 1 or runtime[0].get("result") != "pass" or [item.get("phase") for item in summaries] != expected_phases:
            if len(runtime_errors) == 1:
                raise common.GateFailure(f"runtime_{runtime_errors[0]['error_type']}")
            raise common.GateFailure("combined_runtime_failed")
        by_phase = {str(item["phase"]): item for item in summaries}
        for summary in summaries:
            if summary["frames"] != FRAMES or summary["samples"] != SAMPLES or summary["touches"] != TOUCHES:
                raise common.GateFailure("workload_execution_failed")
        if self.mode == "qualification":
            disabled, enabled = by_phase["disabled"], by_phase["enabled"]
            if disabled["semantic_digest"] != enabled["semantic_digest"] or disabled["framebuffer_digest"] != enabled["framebuffer_digest"]:
                raise common.GateFailure("recording_behavior_mismatch")
            self._validate_frame_records()
            if nearest_rank([int(item["render_us"]) for item in self.frames], 95) != enabled["render_p95_us"]:
                raise common.GateFailure("recording_aggregate_mismatch")
            if nearest_rank([int(item["transfer_us"]) for item in self.frames], 95) != enabled["transfer_p95_us"]:
                raise common.GateFailure("recording_aggregate_mismatch")
            if nearest_rank([int(item["combined_us"]) for item in self.frames], 95) != enabled["combined_p95_us"] or nearest_rank([int(item["combined_us"]) for item in self.frames], 99) != enabled["combined_p99_us"]:
                raise common.GateFailure("recording_aggregate_mismatch")
            if sum(item["missed"] == "yes" for item in self.frames) != enabled["missed_frames"]:
                raise common.GateFailure("recording_aggregate_mismatch")
        else:
            self._validate_frame_records()
        return by_phase

    @staticmethod
    def stable_digest(short_digest: object) -> str:
        return hashlib.sha256(f"fnv64:{short_digest}".encode()).hexdigest()

    def _criteria(self, phases: dict[str, dict[str, object]]) -> list[dict[str, object]]:
        criteria: list[dict[str, object]] = []
        for phase_name, phase in phases.items():
            missed_percent = float(phase["missed_frames"]) * 100 / SAMPLES
            values = (
                (f"{phase_name}_render_p95", phase["render_p95_us"], "us", 12_000, phase["render_p95_us"] <= 12_000),
                (f"{phase_name}_transfer_p95", phase["transfer_p95_us"], "us", 12_000, phase["transfer_p95_us"] <= 12_000),
                (f"{phase_name}_combined_p95", phase["combined_p95_us"], "us", 25_000, phase["combined_p95_us"] <= 25_000),
                (f"{phase_name}_combined_p99", phase["combined_p99_us"], "us", 33_300, phase["combined_p99_us"] <= 33_300),
                (f"{phase_name}_touch_p95", phase["touch_p95_us"], "us", 100_000, phase["touch_p95_us"] <= 100_000),
                (f"{phase_name}_deadline_misses", missed_percent, "percent", 1.0, missed_percent <= 1.0),
                (f"{phase_name}_max_dirty_pixels", phase["max_dirty_pixels"], "pixels", MAX_DIRTY_PIXELS, 0 < phase["max_dirty_pixels"] <= MAX_DIRTY_PIXELS),
                (f"{phase_name}_post_initial_full_frames", phase["post_initial_full_frames"], "frames", 0, phase["post_initial_full_frames"] == 0),
            )
            criteria.extend(
                {"name": name, "value": value, "unit": unit, "threshold": threshold, "passed": passed}
                for name, value, unit, threshold, passed in values
            )
        if self.mode == "qualification":
            disabled = phases["disabled"]
            enabled = phases["enabled"]
            threshold = int(disabled["combined_p95_us"] + max(disabled["combined_p95_us"] * 0.05, 1_000))
            criteria.append(
                {
                    "name": "recording_overhead_combined_p95",
                    "value": enabled["combined_p95_us"],
                    "unit": "us",
                    "threshold": threshold,
                    "passed": enabled["combined_p95_us"] <= threshold,
                }
            )
        return criteria

    def _foundation_criteria(self) -> list[dict[str, object]]:
        values = (
            ("xtensa_builds", self.builds, 1),
            ("clean_rebuilds", self.rebuilds, 1),
            ("combined_link_checks", self.linker_checks, 1),
            ("physical_boots", self.physical_boots, 1),
            ("idle_checks", self.idle_checks, 1),
        )
        return [
            {
                "name": name,
                "value": value,
                "unit": "count",
                "threshold": threshold,
                "passed": value == threshold,
            }
            for name, value, threshold in values
        ]

    def run_device(self, build: Path) -> tuple[dict[str, dict[str, object]], list[dict[str, object]]]:
        self.flash(build)
        self.serial_exchange("status", "postflash", {"idle"}, 5)
        artifact = self.recorder.directory / "frames.jsonl" if self.mode == "qualification" and self.recorder.directory is not None else None
        self.writer = MeasurementWriter(artifact)
        self.writer.start()
        try:
            records = self.serial_exchange("run", self.mode, {"boot", "summary", "result", "idle"}, 140 if self.mode == "qualification" else 75)
        finally:
            if not self.writer.stop():
                self.recorder.health = "partial"
                self.recorder.health_reason = self.writer.error or "recording_flush_failed"
        phases = self._validate_summaries(records)
        self.physical_boots = self.idle_checks = 1
        return phases, records

    def execute(self, requested_device: str | None) -> dict[str, dict[str, object]]:
        build = self.state / "build/gate1e" / self.run_id / "normal"
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
            phases, _ = self.run_device(build)
            return phases
        finally:
            self.cleanup_device(build)

    def run(self, requested_device: str | None = None) -> tuple[int, dict[str, object]]:
        result, reason, code = "pass", "all_criteria_passed", 0
        phases: dict[str, dict[str, object]] = {}
        try:
            self.prepare()
            phases = self.execute(requested_device)
            criteria = self._criteria(phases)
            if not all(item["passed"] for item in criteria):
                raise common.GateFailure("performance_criteria_failed")
            disabled = phases["disabled"]
            semantic = self.stable_digest(disabled["semantic_digest"])
            framebuffer = self.stable_digest(disabled["framebuffer_digest"])
            if self.mode == "conformance":
                qualification = common.read_result_safe(
                    self.state / "results/1e/qualification/result.json", "1e", "qualification"
                )
                if qualification is None or qualification.get("result") != "pass":
                    raise common.GateInconclusive("qualification_required")
                if any(
                    qualification.get(key) != value
                    for key, value in (
                        ("workload_identity_digest", self.workload_identity),
                        ("disabled_semantic_event_digest", semantic),
                        ("disabled_framebuffer_digest", framebuffer),
                    )
                ):
                    raise common.GateFailure("qualification_conformance_mismatch")
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

        criteria = self._foundation_criteria() + (self._criteria(phases) if phases else [])
        value: dict[str, object] = {
            "schema_version": 1,
            "gate": "1e",
            "mode": self.mode,
            "run_id": self.run_id,
            "result": result,
            "reason_code": reason,
            "cleanup_status": self.cleanup_status,
            "device_state": self.device_state,
            "firmware_digest": self.firmware_digest,
            "workload_identity_digest": self.workload_identity,
            "criteria": criteria,
            "started_at": self.started,
            "ended_at": common.utc_now(),
        }
        if phases:
            disabled = phases["disabled"]
            value["disabled_semantic_event_digest"] = self.stable_digest(disabled["semantic_digest"])
            value["disabled_framebuffer_digest"] = self.stable_digest(disabled["framebuffer_digest"])
        if self.normal_image_sha256 is not None:
            value["firmware_image_sha256"] = self.normal_image_sha256
        if self.mode == "conformance":
            qualification = common.read_result_safe(
                self.state / "results/1e/qualification/result.json", "1e", "qualification"
            )
            if qualification is not None:
                value["qualification_run_id"] = qualification["run_id"]
        return code, value

    def recover(self, expected_firmware: str, requested_device: str | None = None) -> tuple[int, str]:
        build = self.state / "build/gate1e" / self.run_id / "normal"
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
    mode = "qualification" if args.recording == "on" else "conformance"
    root = Path(__file__).resolve().parents[1]
    run_id = str(uuid.uuid4())
    print(run_id, flush=True)
    try:
        common.prepare_control_state(root, "1e")
        runner = Runner(root, args.recording == "on", run_id, mode)
        if args.action == "recover":
            runner.verified_resources["mode"] = "recover"
        lock = gate1c.acquire_lock(runner.state, run_id, "1e")
    except (OSError, common.GateInconclusive) as error:
        print(f"Gate 1E could not start: {error}", file=sys.stderr)
        return 2
    result_path = runner.state / f"results/1e/{mode}/result.json"
    pending_path = result_path.with_name(".result.pending")
    signal.signal(signal.SIGINT, lambda _signal, _frame: setattr(runner, "cancelled", True))
    with lock:
        recording_mode = "recover" if args.action == "recover" else mode
        common.initialize_recording(root, runner.recorder, "1e", [TARGET], recording_mode)
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
            runner.recorder._append(
                {
                    "schema_version": 1,
                    "type": "result_published",
                    "run_id": run_id,
                    "result": result["result"],
                }
            )
        except OSError:
            pending_path.unlink(missing_ok=True)
            return 2
    if code == 2:
        print(f"Gate 1E inconclusive: {result['reason_code']}; run_id={run_id}", file=sys.stderr)
    print(f"Gate 1E {mode} {result['result']} ({result['reason_code']})")
    print(result_path.relative_to(root))
    return code


if __name__ == "__main__":
    raise SystemExit(main())
