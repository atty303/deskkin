#!/usr/bin/env python3
"""Bounded Gate 1B Slint software-renderer feasibility runner."""

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
import struct
import subprocess
import sys
import time
import uuid
import zlib
from pathlib import Path

import gate_runner as common

WIDTH = 320
HEIGHT = 240
TARGET = "qemu_cortex_m3"
FRAME_RE = re.compile(
    r"^DESKKIN_GATE1B_FRAME schema=1 stage=(?P<stage>[0-9]+) line=(?P<line>[0-9]+) "
    r"start=(?P<start>[0-9]+) end=(?P<end>[0-9]+) rgb565=(?P<pixels>[0-9a-f]+)$"
)
STATIC_RE = re.compile(r"^DESKKIN_GATE1B_EVENT schema=1 event=static_render ranges=(?P<ranges>[0-9]+)$")
INPUT_RE = re.compile(r"^DESKKIN_GATE1B_EVENT schema=1 event=input callback_count=(?P<count>[0-9]+) phase=(?P<phase>[0-9]+)$")
DIRTY_RE = re.compile(r"^DESKKIN_GATE1B_EVENT schema=1 event=input_dirty ranges=(?P<ranges>[0-9]+)$")
ANIMATION_RE = re.compile(r"^DESKKIN_GATE1B_EVENT schema=1 event=animation timer_waits=(?P<waits>[0-9]+) busy_polls=(?P<polls>[0-9]+)$")
RESULT_LINE = "DESKKIN_GATE1B_RESULT schema=1 result=pass"
HOST_RE = re.compile(r"^DESKKIN_GATE1B_HOST schema=1 callback_count=(?P<count>[0-9]+) phase=(?P<phase>[0-9]+) timer_waits=(?P<waits>[0-9]+) busy_polls=(?P<polls>[0-9]+)$")
GATE_INPUTS = (
    "scripts/gate_runner.py", "scripts/gate1b_runner.py", "gates/gate1b/CMakeLists.txt",
    "gates/gate1b/Cargo.toml", "gates/gate1b/Cargo.lock", "gates/gate1b/Kconfig",
    "gates/gate1b/prj.conf", "gates/gate1b/build.rs", "gates/gate1b/ui/gate.slint",
    "gates/gate1b/src/lib.rs", "gates/gate1b/src/shared.rs", "gates/gate1b/src/bin/host.rs",
)


class FrameCapture:
    def __init__(self) -> None:
        self.frame = [0] * (WIDTH * HEIGHT)
        self.initial = [0] * (WIDTH * HEIGHT)
        self.after_input = [0] * (WIDTH * HEIGHT)
        self.stage_one_ranges: list[tuple[int, int, int]] = []
        self.stage_ranges: dict[int, list[tuple[int, int, int]]] = {}
        self.stage_frames: dict[int, list[int]] = {}
        self.initial_lines: set[int] = set()
        self.static_ranges = 0
        self.input_ranges = 0
        self.callback_count = 0
        self.phase = 0
        self.timer_waits = 0
        self.busy_polls = -1
        self.result = False
        self.last_stage = -1
        self.static_seen = self.input_seen = self.dirty_seen = self.animation_seen = False

    def add(self, line: str) -> None:
        clean = common.ANSI_RE.sub("", line).strip()
        match = FRAME_RE.fullmatch(clean)
        if match:
            values = {key: int(match[key]) for key in ("stage", "line", "start", "end")}
            stage, row, start, end = (values[key] for key in ("stage", "line", "start", "end"))
            pixels = match["pixels"]
            if self.result or self.animation_seen or stage < self.last_stage or stage == 0 and self.static_seen or stage == 1 and not self.static_seen or stage >= 2 and not self.dirty_seen:
                raise common.GateFailure("invalid_frame_sequence")
            self.last_stage = stage
            if stage > 11 or row >= HEIGHT or start >= end or end > WIDTH or len(pixels) != (end - start) * 4:
                raise common.GateFailure("invalid_frame_record")
            decoded = [int(pixels[index:index + 4], 16) for index in range(0, len(pixels), 4)]
            offset = row * WIDTH + start
            self.frame[offset:offset + len(decoded)] = decoded
            if stage == 0:
                self.initial_lines.add(row)
            elif stage == 1:
                self.stage_one_ranges.append((row, start, end))
            self.stage_ranges.setdefault(stage, []).append((row, start, end))
            if stage == 0:
                self.initial = self.frame.copy()
            elif stage == 1:
                self.after_input = self.frame.copy()
            self.stage_frames[stage] = self.frame.copy()
            return
        for pattern, assign in (
            (STATIC_RE, self._static),
            (INPUT_RE, self._input),
            (DIRTY_RE, self._dirty),
            (ANIMATION_RE, self._animation),
        ):
            match = pattern.fullmatch(clean)
            if match:
                assign(match)
                return
        if clean == RESULT_LINE:
            if self.result or not self.animation_seen:
                raise common.GateFailure("invalid_gate_protocol")
            self.result = True
        elif clean.startswith(("DESKKIN_GATE1B_FRAME ", "DESKKIN_GATE1B_EVENT ", "DESKKIN_GATE1B_RESULT ")):
            raise common.GateFailure("invalid_gate_protocol")

    def _static(self, match: re.Match[str]) -> None:
        if self.static_seen or self.last_stage != 0:
            raise common.GateFailure("invalid_gate_protocol")
        self.static_seen = True
        self.static_ranges = int(match["ranges"])

    def _input(self, match: re.Match[str]) -> None:
        if self.input_seen or self.last_stage != 1:
            raise common.GateFailure("invalid_gate_protocol")
        self.input_seen = True
        self.callback_count, self.phase = int(match["count"]), int(match["phase"])

    def _dirty(self, match: re.Match[str]) -> None:
        if self.dirty_seen or not self.input_seen:
            raise common.GateFailure("invalid_gate_protocol")
        self.dirty_seen = True
        self.input_ranges = int(match["ranges"])

    def _animation(self, match: re.Match[str]) -> None:
        if self.animation_seen or self.last_stage < 2:
            raise common.GateFailure("invalid_gate_protocol")
        self.animation_seen = True
        self.timer_waits, self.busy_polls = int(match["waits"]), int(match["polls"])

    def validate(self) -> dict[str, int]:
        changed = {index for index, pair in enumerate(zip(self.initial, self.after_input)) if pair[0] != pair[1]}
        dirty = {
            row * WIDTH + column
            for row, start, end in self.stage_one_ranges
            for column in range(start, end)
        }
        animation_changed = 0
        animation_stages = sorted(stage for stage in self.stage_frames if 2 <= stage <= 11)
        animation_valid = bool(animation_stages)
        previous = self.stage_frames.get(1, self.after_input)
        for stage in animation_stages:
            stage_changed = {index for index, pair in enumerate(zip(previous, self.stage_frames[stage])) if pair[0] != pair[1]}
            stage_dirty = {row * WIDTH + column for row, start, end in self.stage_ranges[stage] for column in range(start, end)}
            animation_changed += len(stage_changed)
            animation_valid = animation_valid and stage_changed <= stage_dirty
            previous = self.stage_frames[stage]
        if (
            self.initial_lines != set(range(HEIGHT))
            or self.static_ranges != HEIGHT
            or self.callback_count != 1
            or self.phase != 1
            or self.input_ranges != len(self.stage_one_ranges)
            or not dirty
            or not changed
            or changed != dirty
            or not animation_valid
            or animation_changed == 0
            or self.timer_waits != 10
            or self.busy_polls != 0
            or not self.result
        ):
            raise common.GateFailure("render_contract_failed")
        return {"static_ranges": self.static_ranges, "input_ranges": self.input_ranges, "dirty_pixels": len(dirty), "changed_pixels": len(changed), "animation_changed_pixels": animation_changed, "animation_stages": len(animation_stages)}


def rgb565_bytes(pixels: list[int]) -> bytes:
    result = bytearray()
    for pixel in pixels:
        result.extend(((pixel >> 11) * 255 // 31, ((pixel >> 5) & 0x3F) * 255 // 63, (pixel & 0x1F) * 255 // 31))
    return bytes(result)


def decode_host(path: Path) -> list[int]:
    raw = path.read_bytes()
    if len(raw) != WIDTH * HEIGHT * 2:
        raise common.GateFailure("invalid_host_framebuffer")
    return list(struct.unpack(f"<{WIDTH * HEIGHT}H", raw))


def png_bytes(rgb: bytes) -> bytes:
    if len(rgb) != WIDTH * HEIGHT * 3:
        raise ValueError("invalid RGB framebuffer")
    def chunk(kind: bytes, data: bytes) -> bytes:
        return struct.pack(">I", len(data)) + kind + data + struct.pack(">I", zlib.crc32(kind + data) & 0xFFFFFFFF)
    scanlines = b"".join(b"\0" + rgb[row * WIDTH * 3:(row + 1) * WIDTH * 3] for row in range(HEIGHT))
    return b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", WIDTH, HEIGHT, 8, 2, 0, 0, 0)) + chunk(b"IDAT", zlib.compress(scanlines, 9)) + chunk(b"IEND", b"")


def publish_bytes(path: Path, data: bytes) -> bool:
    temporary = path.with_name(f".{path.name}.{uuid.uuid4().hex}.tmp")
    try:
        path.parent.mkdir(parents=True, exist_ok=True, mode=0o700)
        with temporary.open("xb") as stream:
            os.chmod(temporary, 0o600)
            stream.write(data)
            stream.flush()
            os.fsync(stream.fileno())
        temporary.replace(path)
        valid = path.stat().st_size <= common.RUN_LIMIT_BYTES and path.read_bytes() == data
        if not valid:
            path.unlink(missing_ok=True)
        return valid
    except OSError:
        with contextlib.suppress(OSError):
            temporary.unlink(missing_ok=True)
        raise


def valid_png(data: bytes) -> bool:
    if not data.startswith(b"\x89PNG\r\n\x1a\n"):
        return False
    offset, kinds = 8, []
    try:
        while offset < len(data):
            length = struct.unpack(">I", data[offset:offset + 4])[0]
            kind = data[offset + 4:offset + 8]
            payload = data[offset + 8:offset + 8 + length]
            checksum = struct.unpack(">I", data[offset + 8 + length:offset + 12 + length])[0]
            if checksum != zlib.crc32(kind + payload) & 0xFFFFFFFF:
                return False
            kinds.append(kind)
            if kind == b"IHDR" and payload != struct.pack(">IIBBBBB", WIDTH, HEIGHT, 8, 2, 0, 0, 0):
                return False
            offset += 12 + length
    except (struct.error, IndexError):
        return False
    return offset == len(data) and kinds[0] == b"IHDR" and kinds[-1] == b"IEND" and set(kinds) <= {b"IHDR", b"IDAT", b"IEND"}


def publish_measurements(path: Path, value: dict[str, object]) -> bool:
    if set(value) != {"schema_version", "metrics", "framebuffer_sha256"} or value.get("schema_version") != 1 or not common.privacy_safe(value):
        return False
    metrics = value.get("metrics")
    allowed = {"name", "value", "unit", "samples", "threshold", "passed"}
    if not isinstance(metrics, list) or not all(isinstance(item, dict) and set(item) == allowed for item in metrics):
        return False
    try:
        common.atomic_json(path, value)
        valid = path.stat().st_size <= common.RUN_LIMIT_BYTES and json.loads(path.read_text(encoding="utf-8")) == value
        if not valid:
            path.unlink(missing_ok=True)
        return valid
    except (OSError, json.JSONDecodeError):
        with contextlib.suppress(OSError):
            path.unlink(missing_ok=True)
        return False


class Runner(common.Runner):
    def __init__(self, root: Path, recording: bool, run_id: str, deadline_seconds: int = 900):
        super().__init__(root, recording, run_id, deadline_seconds)
        self.verified_resources.update({"gate": "1b", "target": [TARGET]})
        self.target_builds = 0
        self.frame_matches = 0
        self.no_cpp_app_layer = 0
        self.measurements: dict[str, int] = {}

    def prepare(self) -> None:
        super().prepare()
        if list((self.root / "gates/gate1b").rglob("*.cpp")) or "CXX" in (self.root / "gates/gate1b/CMakeLists.txt").read_text(encoding="utf-8"):
            raise common.GateFailure("cpp_application_layer_present")
        self.no_cpp_app_layer = 1
        digests = dict(self.verified_resources["input_digests"])
        digests.update({name: common.sha256(self.root / name) for name in GATE_INPUTS})
        self.verified_resources["input_digests"] = digests
        self.verified_resources["application_version"] = "gate1b-0.1.0"
        self.recorder._append({"schema_version": 1, "type": "resource_verified", **self.verified_resources})

    def publish_verified_resources(self) -> None:
        pass

    def verify_inputs(self) -> None:
        expected = self.verified_resources["input_digests"]
        if any(common.sha256(self.root / name) != digest for name, digest in expected.items()):
            raise common.GateInconclusive("input_changed")
        try:
            for name, revision in common.WEST_REVISIONS.items():
                project = self.west_paths[name]
                actual = subprocess.run(["git", "-C", str(project), "rev-parse", "HEAD"], cwd=self.state, env=self.env, check=True, capture_output=True, text=True, timeout=min(10, self.remaining())).stdout.strip()
                dirty = subprocess.run(["git", "-C", str(project), "status", "--porcelain"], cwd=self.state, env=self.env, check=True, capture_output=True, text=True, timeout=min(10, self.remaining())).stdout.strip()
                if actual != revision or dirty:
                    raise common.GateInconclusive("input_changed")
            sdk = self.state / "sdk"
            for relative, digest in self.verified_resources["sdk_file_digests"].items():
                if common.sha256(sdk / relative) != digest:
                    raise common.GateInconclusive("input_changed")
        except (KeyError, OSError, subprocess.SubprocessError) as error:
            raise common.GateInconclusive("input_changed") from error

    def configure(self, target: str, build: Path, panic: bool = False) -> None:
        self.command("configure", [str(self.state / "venv/bin/west"), "build", "--cmake-only", "--board", target, "--build-dir", str(build), str(self.root / "gates/gate1b")], target)

    def _cargo_config(self) -> list[str]:
        modules = self.state / "west/modules/lang/rust"
        return [
            "--config", f"patch.crates-io.zephyr.path='{modules / 'zephyr'}'",
            "--config", f"patch.crates-io.zephyr-build.path='{modules / 'zephyr-build'}'",
            "--config", f"patch.crates-io.zephyr-sys.path='{modules / 'zephyr-sys'}'",
        ]

    def _boot(self, build: Path) -> FrameCapture:
        start = time.monotonic()
        process: subprocess.Popen[bytes] | None = None
        selector: selectors.BaseSelector | None = None
        capture = FrameCapture()
        pending = b""
        terminal_recorded = False
        try:
            process = subprocess.Popen(["cmake", "--build", str(build), "--target", "run"], cwd=self.state, env=self.env, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, start_new_session=True, bufsize=0)
            assert process.stdout is not None
            selector = selectors.DefaultSelector()
            selector.register(process.stdout, selectors.EVENT_READ)
            operation_deadline = min(self.deadline, time.monotonic() + 45)
            while time.monotonic() < operation_deadline and not capture.result:
                if self.cancelled:
                    self.recorder.event("render", "cancel", round((time.monotonic() - start) * 1000), "cancelled", TARGET)
                    terminal_recorded = True
                    raise common.GateCancelled("cancelled")
                for key, _ in selector.select(timeout=0.2):
                    chunk = os.read(key.fileobj.fileno(), 65536)
                    if chunk:
                        pending += chunk
                        lines = pending.split(b"\n")
                        pending = lines.pop()
                        for raw in lines:
                            capture.add(raw.decode("utf-8", errors="replace"))
                if process.poll() is not None:
                    break
            if not capture.result:
                if time.monotonic() >= operation_deadline:
                    self.recorder.event("render", "timeout", round((time.monotonic() - start) * 1000), "deadline_exceeded", TARGET)
                    terminal_recorded = True
                    raise common.GateTimeout("deadline_exceeded")
                self.recorder.event("render", "error", round((time.monotonic() - start) * 1000), "expected_marker_missing", TARGET)
                terminal_recorded = True
                raise common.GateFailure("expected_marker_missing")
            self.recorder.event("render", "success", round((time.monotonic() - start) * 1000), target=TARGET)
            terminal_recorded = True
            return capture
        except common.GateFailure as error:
            if not terminal_recorded:
                self.recorder.event("render", "error", round((time.monotonic() - start) * 1000), error.reason, TARGET)
            raise
        finally:
            if process is not None and not self._terminate_group(process):
                self.cleanup_status = "failed"
            if selector is not None:
                selector.close()
            if process is not None and process.stdout is not None:
                process.stdout.close()

    def target(self, target: str) -> None:
        root = self.state / "build/gate1b" / self.run_id
        build = root / "target"
        self.verify_inputs()
        self.configure(target, build)
        first = self.build(target, build)
        self.verify_inputs()
        capture = self._boot(build)
        self.verify_inputs()
        self.measurements = capture.validate()
        self.target_builds = 1
        shutil.rmtree(build)
        self.configure(target, build)
        second = self.build(target, build)
        self.verify_inputs()
        if first != second:
            raise common.GateFailure("clean_rebuild_digest_mismatch")
        self.clean_rebuilds.add(target)

        host_frame = root / "host.rgb565"
        cargo_target = root / "cargo-host"
        self.verify_inputs()
        output = self.command("render", ["cargo", "run", "--quiet", "--locked", "--manifest-path", str(self.root / "gates/gate1b/Cargo.toml"), "--bin", "gate1b-host", "--features", "host", "--target-dir", str(cargo_target), *self._cargo_config(), "--", str(host_frame)], "host")
        host_match = next((HOST_RE.fullmatch(line.strip()) for line in output.splitlines() if HOST_RE.fullmatch(line.strip())), None)
        if not host_match or tuple(int(host_match[key]) for key in ("count", "phase", "waits", "polls")) != (1, 1, 10, 0):
            raise common.GateFailure("host_render_contract_failed")
        host = decode_host(host_frame)
        target_rgb = rgb565_bytes(capture.frame)
        if target_rgb != rgb565_bytes(host):
            raise common.GateFailure("framebuffer_mismatch")
        self.frame_matches = 1
        digest = hashlib.sha256(target_rgb).hexdigest()
        self.measurements.update({"width": WIDTH, "height": HEIGHT, "timer_waits": capture.timer_waits, "busy_polls": capture.busy_polls, "callback_count": capture.callback_count})
        if self.recorder.directory is not None:
            measurements = {"schema_version": 1, "metrics": [{"name": key, "value": value, "unit": "count", "samples": 1, "threshold": 0, "passed": True} for key, value in sorted(self.measurements.items())], "framebuffer_sha256": digest}
            try:
                image = png_bytes(target_rgb)
                if not publish_measurements(self.recorder.directory / "measurements.json", measurements) or not valid_png(image) or not publish_bytes(self.recorder.directory / "framebuffer.png", image):
                    self.recorder.health, self.recorder.health_reason = "partial", "privacy_filter_failed"
            except OSError:
                self.recorder.health, self.recorder.health_reason = "partial", "recording_io_failure"
        host_frame.unlink(missing_ok=True)
        self.verify_inputs()

    def run(self) -> tuple[int, dict[str, object]]:
        result, reason, exit_code = "pass", "all_criteria_passed", 0
        try:
            self.prepare()
            self.target(TARGET)
        except common.GateCancelled:
            result, reason, exit_code = "inconclusive", "cancelled", 130
        except common.GateTimeout:
            result, reason, exit_code = "inconclusive", "deadline_exceeded", 124
        except common.GateInconclusive as error:
            result, reason, exit_code = "inconclusive", error.reason, 2
        except common.GateFailure as error:
            result, reason, exit_code = "fail", error.reason, 1
        except (OSError, subprocess.SubprocessError) as error:
            result, reason, exit_code = "inconclusive", f"setup_{type(error).__name__.lower()}", 2
        if self.cleanup_status != "success" and exit_code not in (124, 130):
            result, reason, exit_code = "inconclusive", "cleanup_failed", 2
        criteria = [
            {"name": "target_builds", "value": self.target_builds, "unit": "targets", "threshold": 1, "passed": self.target_builds == 1},
            {"name": "clean_rebuilds", "value": len(self.clean_rebuilds), "unit": "targets", "threshold": 1, "passed": len(self.clean_rebuilds) == 1},
            {"name": "framebuffer_matches", "value": self.frame_matches, "unit": "matches", "threshold": 1, "passed": self.frame_matches == 1},
            {"name": "static_render_lines", "value": self.measurements.get("static_ranges", 0), "unit": "lines", "threshold": HEIGHT, "passed": self.measurements.get("static_ranges") == HEIGHT},
            {"name": "dirty_pixels", "value": self.measurements.get("dirty_pixels", 0), "unit": "pixels", "threshold": 1, "passed": self.measurements.get("dirty_pixels", 0) > 0 and self.measurements.get("dirty_pixels") == self.measurements.get("changed_pixels")},
            {"name": "animation_stages", "value": self.measurements.get("animation_stages", 0), "unit": "stages", "threshold": 1, "passed": self.measurements.get("animation_stages", 0) > 0 and self.measurements.get("animation_changed_pixels", 0) > 0},
            {"name": "timer_waits", "value": self.measurements.get("timer_waits", 0), "unit": "waits", "threshold": 10, "passed": self.measurements.get("timer_waits") == 10},
            {"name": "busy_polls", "value": self.measurements.get("busy_polls", -1), "unit": "polls", "threshold": 0, "passed": self.measurements.get("busy_polls") == 0},
            {"name": "input_callbacks", "value": self.measurements.get("callback_count", 0), "unit": "callbacks", "threshold": 1, "passed": self.measurements.get("callback_count") == 1},
            {"name": "no_cpp_app_layer", "value": self.no_cpp_app_layer, "unit": "boolean", "threshold": 1, "passed": self.no_cpp_app_layer == 1},
        ]
        return exit_code, {"schema_version": 1, "gate": "1b", "mode": "default", "run_id": self.run_id, "result": result, "reason_code": reason, "cleanup_status": self.cleanup_status, "criteria": criteria, "started_at": self.started, "ended_at": common.utc_now()}


def acquire_lock(state: Path, run_id: str):
    locks = state / "locks"
    common.private_directory(locks)
    stream = None
    transferred = False
    directory_fd: int | None = None
    file_fd: int | None = None
    try:
        directory_fd = os.open(locks, os.O_RDONLY | getattr(os, "O_DIRECTORY", 0) | getattr(os, "O_NOFOLLOW", 0))
        file_fd = os.open("1b.lock", os.O_RDWR | os.O_CREAT | getattr(os, "O_NOFOLLOW", 0), 0o600, dir_fd=directory_fd)
        if not stat.S_ISREG(os.fstat(file_fd).st_mode):
            raise OSError("lock is not a regular file")
        os.fchmod(file_fd, 0o600)
        stream = os.fdopen(file_fd, "r+", encoding="utf-8")
        file_fd = None
        fcntl.flock(stream, fcntl.LOCK_EX | fcntl.LOCK_NB)
    except BlockingIOError:
        assert stream is not None
        stream.seek(0)
        try:
            value = json.load(stream)
            owner = str(value.get("run_id", "unknown"))
            if not re.fullmatch(r"[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}", owner):
                owner = "unknown"
        except (UnicodeError, json.JSONDecodeError, AttributeError, RecursionError):
            owner = "unknown"
        stream.close()
        raise common.GateFailure(f"gate_locked owner={owner}") from None
    except OSError as error:
        if file_fd is not None:
            os.close(file_fd)
        if stream is not None:
            stream.close()
        raise common.GateFailure("lock_unavailable") from error
    finally:
        if directory_fd is not None:
            os.close(directory_fd)
    try:
        stream.seek(0); stream.truncate()
        json.dump({"gate": "1b", "run_id": run_id, "pid": os.getpid(), "started_at": common.utc_now()}, stream, sort_keys=True)
        stream.write("\n"); stream.flush(); os.fsync(stream.fileno())
        transferred = True
        return stream
    except OSError as error:
        raise common.GateFailure("lock_unavailable") from error
    finally:
        if not transferred:
            stream.close()


def acknowledge_existing_result(root: Path, result_path: Path, required: bool = True) -> bool:
    if not required:
        return True
    if not result_path.exists():
        return True
    base_fd: int | None = None
    run_fd: int | None = None
    file_fd: int | None = None
    try:
        result = common.read_result_safe(result_path, "1b", "default")
        if result is None:
            return False
        run_id = str(result["run_id"])
        expected = {"schema_version": 1, "type": "result_published", "run_id": run_id, "result": str(result["result"])}
        if not common.record_schema_safe(expected):
            return False
        if not re.fullmatch(r"[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}", run_id):
            return False
        base = root / ".deskkin/diagnostics"
        if not base.exists():
            return True
        if base.is_symlink() or not base.is_dir():
            return False
        flags = os.O_RDONLY | getattr(os, "O_DIRECTORY", 0) | getattr(os, "O_NOFOLLOW", 0)
        base_fd = os.open(base, flags)
        try:
            metadata = os.stat(run_id, dir_fd=base_fd, follow_symlinks=False)
        except FileNotFoundError:
            return True
        if not stat.S_ISDIR(metadata.st_mode):
            return False
        run_fd = os.open(run_id, flags, dir_fd=base_fd)
        file_fd = os.open("diagnostic.jsonl", os.O_RDWR | getattr(os, "O_NOFOLLOW", 0), dir_fd=run_fd)
        diagnostic_metadata = os.fstat(file_fd)
        if not stat.S_ISREG(diagnostic_metadata.st_mode) or diagnostic_metadata.st_size > common.RUN_LIMIT_BYTES:
            return False
        stream = os.fdopen(file_fd, "r+", encoding="utf-8")
        file_fd = None
        with stream:
                records = [json.loads(line) for line in stream.read().splitlines()]
                if not all(isinstance(record, dict) for record in records) or not common.diagnostic_records_safe(records, run_id):
                    return False
                completeness = [record for record in records if record.get("type") == "completeness"]
                if len(completeness) != 1 or completeness[0].get("result") != expected["result"]:
                    return False
                if expected not in records:
                    stream.seek(0, os.SEEK_END)
                    stream.write(json.dumps(expected, sort_keys=True) + "\n")
                    stream.flush()
                    os.fsync(stream.fileno())
                    stream.seek(0)
                    records = [json.loads(line) for line in stream.read().splitlines()]
                return expected in records and common.diagnostic_records_safe(records, run_id)
    except (OSError, UnicodeError, json.JSONDecodeError, KeyError, RecursionError):
        return False
    finally:
        if file_fd is not None:
            os.close(file_fd)
        if run_fd is not None:
            os.close(run_fd)
        if base_fd is not None:
            os.close(base_fd)


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--recording", choices=("on", "off"), default="on")
    args = parser.parse_args()
    root = Path(__file__).resolve().parents[1]
    run_id = str(uuid.uuid4())
    print(run_id, flush=True)
    try:
        common.prepare_control_state(root, "1b")
        runner = Runner(root, args.recording == "on", run_id)
    except OSError:
        print(f"Gate 1B setup failed; run_id={run_id}; private state is unavailable", file=sys.stderr)
        return 2
    try:
        lock = acquire_lock(runner.state, run_id)
    except common.GateFailure as error:
        print(f"Gate 1B could not start: {error.reason}", file=sys.stderr)
        return 2
    result_path = runner.state / "results/1b/default/result.json"
    pending_path = result_path.with_name(".result.pending")
    signal.signal(signal.SIGINT, lambda _signum, _frame: setattr(runner, "cancelled", True))
    with lock:
        if not acknowledge_existing_result(root, result_path, runner.recorder.directory is not None):
            with contextlib.suppress(OSError):
                common.publish_recording_health(root, run_id, "partial", "result_acknowledgement_failed")
        if not common.clear_result(pending_path):
            print(f"Gate 1B setup failed; run_id={run_id}; stale result could not be removed", file=sys.stderr)
            return 2
        common.initialize_recording(root, runner.recorder, "1b", [TARGET])
        exit_code, result = runner.run()
        if not common.publish_result(pending_path, result):
            print(f"Gate 1B setup failed; run_id={run_id}; current result could not be published", file=sys.stderr)
            runner.recorder.health = "partial"
            runner.recorder.health_reason = "result_publication_failed"
            runner.recorder.finalize(runner.resources(), runner.links, "inconclusive", "result_publication_failed")
            return 2
        runner.recorder.finalize(runner.resources(), runner.links, result["result"], result["reason_code"])
        try:
            pending_path.replace(result_path)
        except OSError:
            with contextlib.suppress(OSError):
                common.publish_recording_health(root, run_id, "partial", "result_publication_failed")
            pending_path.unlink(missing_ok=True)
            print(f"Gate 1B setup failed; run_id={run_id}; current result could not be published", file=sys.stderr)
            return 2
        if runner.recorder.directory is not None:
            if not acknowledge_existing_result(root, result_path):
                with contextlib.suppress(OSError):
                    common.publish_recording_health(root, run_id, "partial", "result_acknowledgement_failed")
    if exit_code == 2:
        print(f"Gate 1B setup inconclusive: {result['reason_code']}; run_id={run_id}", file=sys.stderr)
    print(f"Gate 1B {result['result']} ({result['reason_code']})")
    print(result_path.relative_to(root))
    return exit_code


if __name__ == "__main__":
    raise SystemExit(main())
