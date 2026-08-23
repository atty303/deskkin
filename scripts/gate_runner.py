#!/usr/bin/env python3
"""Bounded, locally observable runner for Deskkin Phase 1 Gate 1A."""

from __future__ import annotations

import argparse
import contextlib
import fcntl
import getpass
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
import time
import uuid
from dataclasses import dataclass, field
from datetime import UTC, datetime, timedelta
from pathlib import Path
from typing import Callable

SCHEMA_VERSION = 1
TARGETS = ("qemu_cortex_m3", "qemu_riscv32")
WEST_REVISIONS = {
    "zephyr": "1f6485eca25431b5ff27ce9a754218c9e559bbbb",
    "zephyr-lang-rust": "dd73abc242e995784da62352fe8c70d9a6c7ac2e",
    "cmsis": "512cc7e895e8491696b61f7ba8066b4a182569b8",
    "cmsis_6": "30a859f44ef8ab4dc8f84b03ed586fd16ccf9d74",
}
RUN_LIMIT_BYTES = 32 * 1024 * 1024
STORE_LIMIT_BYTES = 256 * 1024 * 1024
STORE_LIMIT_RUNS = 20
NORMAL_MARKERS = (
    "event=boot",
    "DESKKIN_GATE_LOG schema=1 event=logging status=ok",
    "event=allocation value=42",
    "event=async_wakeup value=42",
    "DESKKIN_GATE_RESULT schema=1 result=pass",
)
ANSI_RE = re.compile(r"\x1b\[[0-9;]*m")
SERIAL_PATTERNS = {
    "boot": re.compile(r"^DESKKIN_GATE_EVENT schema=1 event=boot board=(?P<board>[a-z0-9_]+) clock_hz=(?P<clock_hz>[0-9]+) console_ord=(?P<console_ord>[0-9]+)$"),
    "allocation": re.compile(r"^DESKKIN_GATE_EVENT schema=1 event=allocation value=(?P<value>[0-9]+)$"),
    "panic_trigger": re.compile(r"^DESKKIN_GATE_EVENT schema=1 event=panic_trigger reason=deliberate$"),
    "async_wakeup": re.compile(r"^DESKKIN_GATE_EVENT schema=1 event=async_wakeup value=(?P<value>[0-9]+)$"),
    "result": re.compile(r"^DESKKIN_GATE_RESULT schema=1 result=pass$"),
}
LOG_PATTERN = re.compile(r"DESKKIN_GATE_LOG schema=1 event=logging status=ok board=(?P<board>[a-z0-9_]+)$")


def utc_now() -> str:
    return datetime.now(UTC).isoformat().replace("+00:00", "Z")


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def atomic_json(path: Path, value: object) -> None:
    path.parent.mkdir(parents=True, exist_ok=True, mode=0o700)
    os.chmod(path.parent, 0o700)
    temporary = path.with_name(f".{path.name}.{uuid.uuid4().hex}.tmp")
    with temporary.open("x", encoding="utf-8") as stream:
        os.chmod(temporary, 0o600)
        json.dump(value, stream, sort_keys=True, separators=(",", ":"))
        stream.write("\n")
        stream.flush()
        os.fsync(stream.fileno())
    temporary.replace(path)


def atomic_jsonl(path: Path, values: list[dict[str, object]]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True, mode=0o700)
    temporary = path.with_name(f".{path.name}.{uuid.uuid4().hex}.tmp")
    with temporary.open("x", encoding="utf-8") as stream:
        os.chmod(temporary, 0o600)
        for value in values:
            stream.write(json.dumps(value, sort_keys=True) + "\n")
        stream.flush()
        os.fsync(stream.fileno())
    temporary.replace(path)


FORBIDDEN_VALUE_RE = re.compile(r"SENSITIVE_FIXTURE|AKIA[0-9A-Z]{16}|gh[pousr]_[A-Za-z0-9_]{20,}|-----BEGIN [A-Z ]*PRIVATE KEY-----")
EMBEDDED_ABSOLUTE_PATH_RE = re.compile(r"(?<![:/A-Za-z0-9])/(?!/)|(?<![A-Za-z0-9])[A-Za-z]:[\\/]")
FORBIDDEN_NORMALIZED_KEYS = {
    "accesstoken",
    "accesskey",
    "apikey",
    "authorization",
    "builder",
    "clientsecret",
    "cookie",
    "credential",
    "host",
    "hostname",
    "password",
    "privatekey",
    "refreshtoken",
    "secret",
    "token",
    "user",
    "username",
}


def privacy_safe(value: object) -> bool:
    try:
        encoded = json.dumps(value, sort_keys=True)
    except (TypeError, ValueError, RecursionError):
        return False
    if len(encoded.encode()) > RUN_LIMIT_BYTES:
        return False

    local_identity = getpass.getuser()

    def safe_key(key: object) -> bool:
        if not isinstance(key, str):
            return False
        normalized = re.sub(r"[^a-z0-9]", "", key.lower())
        return normalized not in FORBIDDEN_NORMALIZED_KEYS

    def visit(item: object) -> bool:
        if item is None or isinstance(item, (bool, int)):
            return True
        if isinstance(item, str):
            contains_identity = bool(local_identity) and re.search(rf"(?<![A-Za-z0-9]){re.escape(local_identity)}(?![A-Za-z0-9])", item, re.IGNORECASE) is not None
            return not contains_identity and EMBEDDED_ABSOLUTE_PATH_RE.search(item) is None and FORBIDDEN_VALUE_RE.search(item) is None
        if isinstance(item, list):
            return all(visit(child) for child in item)
        if isinstance(item, dict):
            return all(safe_key(key) and visit(child) for key, child in item.items())
        return False

    try:
        return visit(value)
    except RecursionError:
        return False


def record_schema_safe(value: dict[str, object]) -> bool:
    record_type = value.get("type")
    common = {"schema_version", "type"}
    allowed = {
        "resource": common | {"run_id", "gate", "mode", "target", "west_revisions", "sdk_file_digests", "sdk_version", "tool_identities", "input_digests", "application_version", "build_type", "deskkin_revision", "deskkin_dirty"},
        "resource_verified": common | {"run_id", "gate", "mode", "target", "west_revisions", "sdk_file_digests", "sdk_version", "tool_identities", "input_digests", "application_version", "build_type", "deskkin_revision", "deskkin_dirty"},
        "operation": common | {"run_id", "operation", "status", "duration_ms", "error_type", "target"},
        "completeness": common | {"status", "reason", "result", "reason_code"},
        "result_published": common | {"run_id", "result"},
    }.get(record_type)
    required = {
        "resource": {"schema_version", "type", "run_id", "gate", "mode"},
        "resource_verified": {"schema_version", "type", "run_id", "gate", "mode"},
        "operation": {"schema_version", "type", "run_id", "operation", "status", "duration_ms"},
        "completeness": {"schema_version", "type", "status", "result", "reason_code"},
        "result_published": {"schema_version", "type", "run_id", "result"},
    }.get(record_type, set())
    if allowed is None or not required <= set(value) <= allowed or value.get("schema_version") != 1 or not privacy_safe(value):
        return False
    if record_type == "operation":
        operations = {
            "prepare", "configure", "rust-compile", "c-compile", "link",
            "boot", "probe", "render", "flash", "preflight", "postflash",
            "cleanup", "recover",
        }
        return value.get("status") in {"success", "error", "timeout", "cancel"} and value.get("operation") in operations and type(value.get("duration_ms")) is int and value["duration_ms"] >= 0 and all(key not in value or isinstance(value[key], str) for key in ("error_type", "target"))
    if record_type == "completeness":
        return value.get("status") in {"complete", "partial", "dropped"} and value.get("result") in {"pass", "fail", "inconclusive"} and isinstance(value.get("reason_code"), str) and (value.get("reason") is None or isinstance(value.get("reason"), str))
    if record_type == "result_published":
        return value.get("result") in {"pass", "fail", "inconclusive"} and isinstance(value.get("run_id"), str)
    mode = value.get("mode")
    if not (isinstance(value.get("run_id"), str) and value.get("gate") in {"1a", "1b", "1c", "1d"} and (mode == "default" or value.get("gate") in {"1c", "1d"} and mode == "recover")):
        return False
    target = value.get("target")
    if target is not None and not (isinstance(target, str) or isinstance(target, list) and all(isinstance(item, str) for item in target)):
        return False
    if record_type == "resource_verified":
        maps = ("west_revisions", "sdk_file_digests", "input_digests")
        if not all(isinstance(value.get(key), dict) and all(isinstance(name, str) and isinstance(digest, str) and re.fullmatch(r"[0-9a-f]{40}|[0-9a-f]{64}", digest) for name, digest in value[key].items()) for key in maps):
            return False
        if not isinstance(value.get("tool_identities"), dict) or not all(isinstance(name, str) and isinstance(identity, str) for name, identity in value["tool_identities"].items()):
            return False
        if not all(isinstance(value.get(key), str) for key in ("sdk_version", "application_version", "build_type", "deskkin_revision")) or type(value.get("deskkin_dirty")) is not bool:
            return False
    return True


def diagnostic_records_safe(records: list[dict[str, object]], run_id: str) -> bool:
    resources = [record for record in records if record.get("type") == "resource"]
    if len(resources) != 1 or resources[0].get("run_id") != run_id or resources[0].get("gate") not in {"1a", "1b", "1c", "1d"} or resources[0].get("mode") not in {"default", "recover"}:
        return False
    if resources[0].get("mode") == "recover" and resources[0].get("gate") not in {"1c", "1d"}:
        return False
    if records[0].get("type") != "resource" or not all(record_schema_safe(record) and ("run_id" not in record or record.get("run_id") == run_id) for record in records):
        return False
    completeness = [index for index, record in enumerate(records) if record.get("type") == "completeness"]
    published = [index for index, record in enumerate(records) if record.get("type") == "result_published"]
    verified = [record for record in records if record.get("type") == "resource_verified"]
    if len(completeness) > 1 or len(published) > 1 or len(verified) > 1:
        return False
    if published and (not completeness or published[0] < completeness[0] or records[published[0]]["result"] != records[completeness[0]]["result"]):
        return False
    if verified and (verified[0].get("gate") != resources[0].get("gate") or verified[0].get("mode") != resources[0].get("mode")):
        return False
    if verified and resources[0].get("gate") == "1b" and verified[0].get("target") != resources[0].get("target"):
        return False
    if completeness and completeness[0] != len(records) - 1 - bool(published):
        return False
    if published and published[0] != len(records) - 1:
        return False
    return True


def artifact_schema_safe(kind: str, value: object) -> bool:
    if not privacy_safe(value) or not isinstance(value, dict) or value.get("schema_version") != 1:
        return False
    if kind == "build":
        if set(value) != {"schema_version", "operations"} or not isinstance(value["operations"], list):
            return False
        allowed = {"operation", "status", "duration_ms", "error_type", "target"}
        return all(isinstance(item, dict) and set(item) <= allowed for item in value["operations"])
    if kind == "link":
        if set(value) != {"schema_version", "artifacts"} or not isinstance(value["artifacts"], list):
            return False
        allowed = {"target", "mode", "sha256", "bytes"}
        return all(isinstance(item, dict) and set(item) == allowed for item in value["artifacts"])
    return False


def publish_artifact(path: Path, value: object, kind: str, writer: Callable[[Path, object], None] = atomic_json) -> bool:
    if not artifact_schema_safe(kind, value):
        return False
    try:
        writer(path, value)
        valid = path.stat().st_size <= RUN_LIMIT_BYTES and json.loads(path.read_text(encoding="utf-8")) == value
        if not valid:
            path.unlink(missing_ok=True)
        return valid
    except json.JSONDecodeError:
        with contextlib.suppress(OSError):
            path.unlink(missing_ok=True)
        return False
    except OSError:
        with contextlib.suppress(OSError):
            path.unlink(missing_ok=True)
        raise


def publish_serial_artifact(path: Path, values: list[dict[str, object]]) -> bool:
    allowed = {
        "schema_version", "target", "mode", "event", "status", "board",
        "clock_hz", "console_ord", "value", "panic_type", "run_id",
        "firmware_digest", "c_to_rust", "rust_to_c", "nesting",
        "restoration", "freed", "reason", "result", "power", "gpio",
        "display", "touch", "flash", "i2c0", "i2c1", "spi2", "width",
        "height", "format", "bytes", "duration_us", "index", "x", "y",
        "pattern", "expected_index", "inside",
    }
    if not all(set(value) <= allowed and privacy_safe(value) for value in values):
        return False
    try:
        atomic_jsonl(path, values)
        loaded = [json.loads(line) for line in path.read_text(encoding="utf-8").splitlines()]
        valid = path.stat().st_size <= RUN_LIMIT_BYTES and loaded == values
        if not valid:
            path.unlink(missing_ok=True)
        return valid
    except json.JSONDecodeError:
        with contextlib.suppress(OSError):
            path.unlink(missing_ok=True)
        return False
    except OSError:
        with contextlib.suppress(OSError):
            path.unlink(missing_ok=True)
        raise


def private_directory(path: Path) -> None:
    if path.is_symlink():
        raise OSError(f"private state path must not be a symlink: {path.name}")
    path.mkdir(exist_ok=True, mode=0o700)
    if not path.is_dir():
        raise OSError(f"private state path is not a directory: {path.name}")
    os.chmod(path, 0o700)


def prepare_control_state(root: Path, gate: str = "1a") -> None:
    state = root / ".deskkin"
    private_directory(state)
    for relative in (
        "results",
        f"results/{gate}",
        f"results/{gate}/default",
        "locks",
    ):
        private_directory(state / relative)


def clear_result(path: Path) -> bool:
    try:
        path.unlink(missing_ok=True)
        return True
    except OSError:
        return False


def publish_result(path: Path, value: object, writer: Callable[[Path, object], None] = atomic_json) -> bool:
    try:
        writer(path, value)
        return True
    except OSError:
        return False


def read_result_safe(path: Path, gate: str, mode: str) -> dict[str, object] | None:
    descriptor: int | None = None
    try:
        descriptor = os.open(path, os.O_RDONLY | getattr(os, "O_NOFOLLOW", 0))
        metadata = os.fstat(descriptor)
        if not stat.S_ISREG(metadata.st_mode) or metadata.st_size > RUN_LIMIT_BYTES:
            return None
        with os.fdopen(descriptor, "r", encoding="utf-8") as stream:
            descriptor = None
            value = json.load(stream)
        required = {
            "schema_version", "gate", "mode", "run_id", "result", "reason_code",
            "cleanup_status", "criteria", "started_at", "ended_at",
        }
        if (
            not isinstance(value, dict)
            or not required <= set(value)
            or value.get("schema_version") != 1
            or value.get("gate") != gate
            or value.get("mode") != mode
            or value.get("result") not in {"pass", "fail", "inconclusive"}
            or not isinstance(value.get("run_id"), str)
            or not re.fullmatch(r"[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}", value["run_id"])
            or not isinstance(value.get("reason_code"), str)
            or value.get("cleanup_status") not in {"success", "failed"}
            or not isinstance(value.get("started_at"), str)
            or not isinstance(value.get("ended_at"), str)
            or not privacy_safe(value)
        ):
            return None
        criteria = value.get("criteria")
        if not isinstance(criteria, list) or not all(
            isinstance(item, dict)
            and set(item) == {"name", "value", "unit", "threshold", "passed"}
            and isinstance(item["name"], str)
            and type(item["value"]) in {int, float}
            and isinstance(item["unit"], str)
            and type(item["threshold"]) in {int, float}
            and type(item["passed"]) is bool
            for item in criteria
        ):
            return None
        return value
    except (OSError, UnicodeError, json.JSONDecodeError, RecursionError):
        return None
    finally:
        if descriptor is not None:
            os.close(descriptor)


def read_diagnostic_records(base: Path, run_id: str) -> list[dict[str, object]] | None:
    if not re.fullmatch(r"[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}", run_id):
        return None
    base_fd = run_fd = file_fd = None
    try:
        flags = os.O_RDONLY | getattr(os, "O_DIRECTORY", 0) | getattr(os, "O_NOFOLLOW", 0)
        base_fd = os.open(base, flags)
        run_fd = os.open(run_id, flags, dir_fd=base_fd)
        file_fd = os.open("diagnostic.jsonl", os.O_RDONLY | getattr(os, "O_NOFOLLOW", 0), dir_fd=run_fd)
        metadata = os.fstat(file_fd)
        if not stat.S_ISREG(metadata.st_mode) or metadata.st_size > RUN_LIMIT_BYTES:
            return None
        with os.fdopen(file_fd, "r", encoding="utf-8") as stream:
            file_fd = None
            records = [json.loads(line) for line in stream.read().splitlines()]
        if not all(isinstance(record, dict) for record in records) or not diagnostic_records_safe(records, run_id):
            return None
        return records
    except (OSError, UnicodeError, json.JSONDecodeError, RecursionError):
        return None
    finally:
        for descriptor in (file_fd, run_fd, base_fd):
            if descriptor is not None:
                os.close(descriptor)


def classify_error(output: str) -> str:
    lowered = output.lower()
    if "failed to run custom build command" in lowered or "could not compile" in lowered:
        return "rust_compile_failed"
    if "kconfig" in lowered and "error" in lowered:
        return "configure_failed"
    if "linking c executable" in lowered or "undefined reference" in lowered:
        return "link_failed"
    if "qemu" in lowered:
        return "qemu_failed"
    return "command_failed"


@dataclass
class Recorder:
    directory: Path | None
    run_id: str
    events: list[dict[str, object]] = field(default_factory=list)
    summaries: list[dict[str, object]] = field(default_factory=list)
    serial: list[dict[str, object]] = field(default_factory=list)
    health: str = "complete"
    health_reason: str | None = None
    started: bool = False

    def _append(self, value: dict[str, object]) -> None:
        if self.directory is None or not self.started:
            return
        if not record_schema_safe(value):
            self.health = "partial"
            self.health_reason = "privacy_filter_failed"
            return
        try:
            path = self.directory / "diagnostic.jsonl"
            with path.open("a", encoding="utf-8") as stream:
                stream.write(json.dumps(value, sort_keys=True) + "\n")
                stream.flush()
                os.fsync(stream.fileno())
        except OSError as error:
            self.health = "partial"
            self.health_reason = f"recording_io_{error.errno}"

    def start(self, resources: dict[str, object]) -> None:
        if self.directory is None or self.started:
            return
        try:
            resource = {"schema_version": 1, "type": "resource", **resources}
            if not record_schema_safe(resource):
                self.health = "dropped"
                self.health_reason = "privacy_filter_failed"
                self.directory = None
                return
            self.directory.mkdir(parents=True, mode=0o700)
            os.chmod(self.directory, 0o700)
            diagnostic = self.directory / "diagnostic.jsonl"
            with diagnostic.open("x", encoding="utf-8") as stream:
                os.chmod(diagnostic, 0o600)
                stream.write(json.dumps(resource, sort_keys=True) + "\n")
                stream.flush()
                os.fsync(stream.fileno())
            self.started = True
        except OSError as error:
            self.health = "dropped"
            self.health_reason = f"recording_io_{error.errno}"
            self.directory = None

    def event(self, operation: str, status: str, duration_ms: int, error_type: str | None = None, target: str | None = None) -> None:
        item: dict[str, object] = {
            "schema_version": SCHEMA_VERSION,
            "run_id": self.run_id,
            "operation": operation,
            "status": status,
            "duration_ms": duration_ms,
        }
        if error_type:
            item["error_type"] = error_type
        if target:
            item["target"] = target
        self.events.append(item)
        self.summaries.append({key: item[key] for key in item if key in {"operation", "status", "duration_ms", "error_type", "target"}})
        self._append({"type": "operation", **item})

    def add_serial(self, target: str, mode: str, line: str) -> None:
        clean = ANSI_RE.sub("", line).strip()
        for event, pattern in SERIAL_PATTERNS.items():
            match = pattern.fullmatch(clean)
            if match:
                groups = match.groupdict()
                valid = target in TARGETS and mode in {"normal", "panic"}
                valid = valid and (groups.get("board") in (None, target))
                valid = valid and (groups.get("value") in (None, "42"))
                valid = valid and (event != "panic_trigger" or mode == "panic")
                valid = valid and (event != "result" or mode == "normal")
                if not valid:
                    self.health = "partial"
                    self.health_reason = "privacy_filter_failed"
                    return
                record: dict[str, object] = {"schema_version": 1, "target": target, "mode": mode, "event": event}
                for key, value in groups.items():
                    record[key] = int(value) if value.isdigit() else value
                self.serial.append(record)
                return
        log_match = LOG_PATTERN.search(clean)
        if log_match and log_match.group("board") == target:
            self.serial.append({"schema_version": 1, "target": target, "mode": mode, "event": "logging", "status": "ok"})
        elif mode == "panic" and clean.startswith("panic: panicked at"):
            self.serial.append({"schema_version": 1, "target": target, "mode": mode, "panic_type": "rust_panic"})
        elif clean.startswith(("DESKKIN_GATE_EVENT ", "DESKKIN_GATE_RESULT ")) or "DESKKIN_GATE_LOG " in clean:
            self.health = "partial"
            self.health_reason = "privacy_filter_failed"

    def finalize(self, resources: dict[str, object], links: list[dict[str, object]], outcome: str, reason_code: str) -> None:
        if self.directory is None:
            return
        try:
            self.start(resources)
            if self.directory is None:
                return
            build_summary = {"schema_version": 1, "operations": self.summaries}
            link_summary = {"schema_version": 1, "artifacts": links}
            if not publish_artifact(self.directory / "build-summary.json", build_summary, "build"):
                self.health = "partial"
                self.health_reason = "privacy_filter_failed"
            if not publish_artifact(self.directory / "link-summary.json", link_summary, "link"):
                self.health = "partial"
                self.health_reason = "privacy_filter_failed"
            if self.health_reason != "privacy_filter_failed":
                if not publish_serial_artifact(self.directory / "serial.jsonl", self.serial):
                    self.health = "partial"
                    self.health_reason = "privacy_filter_failed"
            size = sum(item.stat().st_size for item in self.directory.rglob("*") if item.is_file())
            if size > RUN_LIMIT_BYTES:
                self.health = "partial"
                self.health_reason = "run_capacity_exceeded"
                for artifact in (self.directory / "serial.jsonl", self.directory / "link-summary.json"):
                    artifact.unlink(missing_ok=True)
            self._append({"schema_version": 1, "type": "completeness", "status": self.health, "reason": self.health_reason, "result": outcome, "reason_code": reason_code})
        except OSError as error:
            self.health = "partial"
            self.health_reason = f"recording_io_{error.errno}"
            self._append({"schema_version": 1, "type": "completeness", "status": self.health, "reason": self.health_reason, "result": outcome, "reason_code": reason_code})


class GateFailure(RuntimeError):
    def __init__(self, reason: str):
        super().__init__(reason)
        self.reason = reason


class GateTimeout(GateFailure):
    pass


class GateCancelled(GateFailure):
    pass


class GateInconclusive(GateFailure):
    pass


class Runner:
    def __init__(self, root: Path, recording: bool, run_id: str, deadline_seconds: int = 900):
        self.root = root.resolve()
        self.state = self.root / ".deskkin"
        self.run_id = run_id
        self.started = utc_now()
        self.deadline = time.monotonic() + deadline_seconds
        self.cancelled = False
        self.recorder = Recorder(self.state / "diagnostics" / self.run_id if recording else None, self.run_id)
        self.links: list[dict[str, object]] = []
        self.env = os.environ.copy()
        self.verified_resources: dict[str, object] = {
            "run_id": self.run_id,
            "gate": "1a",
            "mode": "default",
            "target": list(TARGETS),
        }
        self.cleanup_status = "success"
        self.supported_targets: set[str] = set()
        self.clean_rebuilds: set[str] = set()
        self.deliberate_panics: set[str] = set()
        self.west_paths: dict[str, Path] = {}

    def _environment(self) -> dict[str, str]:
        sdk = self.state / "sdk"
        zephyr = self.state / "west" / "zephyr"
        clang_resource = self.command("prepare", ["clang", "-print-resource-dir"], "host").strip()
        libclang = self.command("prepare", ["mise", "where", "conda:libclang"], "host").strip()
        env = os.environ.copy()
        env.update(
            {
                "ZEPHYR_SDK_INSTALL_DIR": str(sdk),
                "ZEPHYR_TOOLCHAIN_VARIANT": "zephyr",
                "ZEPHYR_BASE": str(zephyr),
                "QEMU_BIN_PATH": str(sdk / "sysroots/x86_64-pokysdk-linux/usr/bin"),
                "LIBCLANG_PATH": str(Path(libclang) / "lib"),
                "BINDGEN_EXTRA_CLANG_ARGS_thumbv7m_none_eabi": f"-isystem{clang_resource}/include",
                "BINDGEN_EXTRA_CLANG_ARGS_riscv32i_unknown_none_elf": f"-isystem{clang_resource}/include",
                "SOURCE_DATE_EPOCH": "0",
            }
        )
        return env

    def prepare(self) -> None:
        start = time.monotonic()
        try:
            self.env = self._environment()
            west = self.state / "venv/bin/west"
            if not west.is_file():
                raise GateInconclusive("west_environment_missing")
            actual_revisions: dict[str, str] = {}
            for name, expected in WEST_REVISIONS.items():
                path_text = self.command(
                    "prepare",
                    [str(west), "list", name, "-f", "{abspath}"],
                    "host",
                ).strip()
                project = Path(path_text)
                self.west_paths[name] = project
                actual = self.command(
                    "prepare",
                    ["git", "-C", str(project), "rev-parse", "HEAD"],
                    "host",
                ).strip()
                dirty = self.command(
                    "prepare",
                    ["git", "-C", str(project), "status", "--porcelain"],
                    "host",
                ).strip()
                if actual != expected or dirty:
                    raise GateInconclusive(f"west_project_mismatch_{name.replace('-', '_')}")
                actual_revisions[name] = actual
            sdk_manifest_path = self.root / "requirements/gate1a-sdk.json"
            sdk_manifest = json.loads(sdk_manifest_path.read_text(encoding="utf-8"))
            sdk = self.state / "sdk"
            sdk_digests: dict[str, str] = {}
            for relative, expected in sdk_manifest["files"].items():
                path = sdk / relative
                actual = sha256(path)
                if actual != expected:
                    raise GateInconclusive("sdk_digest_mismatch")
                sdk_digests[relative] = actual
            tool_probes = {
                "rust": self.command("prepare", ["rustc", "--version"], "host").strip(),
                "clang": self.command("prepare", ["clang", "--version"], "host").splitlines()[0],
                "cmake": self.command("prepare", ["cmake", "--version"], "host").splitlines()[0],
                "ninja": self.command("prepare", ["ninja", "--version"], "host").strip(),
                "python": self.command("prepare", [str(self.state / "venv/bin/python"), "--version"], "host").strip(),
            }
            expected_fragments = {
                "rust": "1.95.0",
                "clang": "15.0.7",
                "cmake": "3.28.6",
                "ninja": "1.13.2",
                "python": "3.12.14",
            }
            for name, fragment in expected_fragments.items():
                if fragment not in tool_probes[name]:
                    raise GateInconclusive(f"host_tool_mismatch_{name}")
            gate_inputs = [
                "west.yml",
                "mise.toml",
                "mise.lock",
                "requirements/gate1a.in",
                "requirements/gate1a.lock",
                "requirements/gate1a-sdk.json",
                "scripts/bootstrap_gate1a.sh",
                "scripts/gate_runner.py",
                "gates/gate1a/CMakeLists.txt",
                "gates/gate1a/Cargo.toml",
                "gates/gate1a/Cargo.lock",
                "gates/gate1a/Kconfig",
                "gates/gate1a/prj.conf",
                "gates/gate1a/panic.conf",
                "gates/gate1a/src/lib.rs",
            ]
            deskkin_revision = self.command("prepare", ["git", "-C", str(self.root), "rev-parse", "HEAD"], "host").strip()
            deskkin_dirty = bool(self.command("prepare", ["git", "-C", str(self.root), "status", "--porcelain"], "host").strip())
            self.verified_resources.update(
                {
                    "application_version": "0.1.0",
                    "build_type": "dev",
                    "deskkin_revision": deskkin_revision,
                    "deskkin_dirty": deskkin_dirty,
                    "west_revisions": actual_revisions,
                    "sdk_file_digests": sdk_digests,
                    "sdk_version": sdk_manifest["sdk_version"],
                    "tool_identities": tool_probes,
                    "input_digests": {relative: sha256(self.root / relative) for relative in gate_inputs},
                }
            )
            self.publish_verified_resources()
            self.recorder.event("prepare", "success", round((time.monotonic() - start) * 1000))
        except (GateCancelled, GateTimeout):
            raise
        except GateInconclusive:
            self.recorder.event("prepare", "error", round((time.monotonic() - start) * 1000), "provenance_mismatch")
            raise
        except GateFailure as error:
            self.recorder.event("prepare", "error", round((time.monotonic() - start) * 1000), "setup_probe_failed")
            raise GateInconclusive("setup_probe_failed") from error
        except (OSError, subprocess.SubprocessError, json.JSONDecodeError, KeyError) as error:
            self.recorder.event("prepare", "error", round((time.monotonic() - start) * 1000), "setup_probe_failed")
            raise GateInconclusive("setup_probe_failed") from error

    def publish_verified_resources(self) -> None:
        self.recorder._append({"schema_version": 1, "type": "resource_verified", **self.verified_resources})

    def remaining(self) -> float:
        if self.cancelled:
            raise GateCancelled("cancelled")
        remaining = self.deadline - time.monotonic()
        if remaining <= 0:
            raise GateTimeout("deadline_exceeded")
        return remaining

    def command(self, operation: str, command: list[str], target: str) -> str:
        start = time.monotonic()
        process = subprocess.Popen(
            command,
            cwd=self.state,
            env=self.env,
            stdout=subprocess.PIPE,
            stderr=subprocess.STDOUT,
            start_new_session=True,
            bufsize=0,
        )
        assert process.stdout is not None
        selector: selectors.BaseSelector | None = None
        output = bytearray()
        interrupted: str | None = None
        try:
            selector = selectors.DefaultSelector()
            selector.register(process.stdout, selectors.EVENT_READ)
            while process.poll() is None:
                for key, _ in selector.select(timeout=0.1):
                    chunk = os.read(key.fileobj.fileno(), 65536)
                    if chunk:
                        output.extend(chunk)
                        if len(output) > 2_000_000:
                            del output[:-2_000_000]
                if self.cancelled:
                    interrupted = "cancel"
                    break
                if time.monotonic() >= self.deadline:
                    interrupted = "timeout"
                    break
            for key, _ in selector.select(timeout=0):
                output.extend(os.read(key.fileobj.fileno(), 65536))
            if interrupted:
                duration = round((time.monotonic() - start) * 1000)
                if interrupted == "cancel":
                    self.recorder.event(operation, "cancel", duration, "cancelled", target)
                    raise GateCancelled("cancelled")
                self.recorder.event(operation, "timeout", duration, "deadline_exceeded", target)
                raise GateTimeout("deadline_exceeded")
            process.wait()
            duration = round((time.monotonic() - start) * 1000)
            output_text = output.decode("utf-8", errors="replace")
            if process.returncode != 0:
                error_type = classify_error(output_text)
                self.recorder.event(operation, "error", duration, error_type, target)
                raise GateFailure(error_type)
            self.recorder.event(operation, "success", duration, target=target)
            return output_text
        finally:
            if not self._terminate_group(process):
                self.cleanup_status = "failed"
            if selector is not None:
                selector.close()
            process.stdout.close()

    def _terminate_group(self, process: subprocess.Popen[bytes]) -> bool:
        process_group = process.pid
        if self._group_alive(process_group):
            with contextlib.suppress(ProcessLookupError):
                os.killpg(process_group, signal.SIGTERM)
        deadline = time.monotonic() + 5
        while self._group_alive(process_group) and time.monotonic() < deadline:
            process.poll()
            time.sleep(0.05)
        if self._group_alive(process_group):
            with contextlib.suppress(ProcessLookupError):
                os.killpg(process_group, signal.SIGKILL)
            deadline = time.monotonic() + 5
            while self._group_alive(process_group) and time.monotonic() < deadline:
                process.poll()
                time.sleep(0.05)
        with contextlib.suppress(subprocess.TimeoutExpired):
            process.wait(timeout=0)
        return not self._group_alive(process_group)

    @staticmethod
    def _group_alive(process_group: int) -> bool:
        try:
            os.killpg(process_group, 0)
            return True
        except ProcessLookupError:
            return False
        except PermissionError:
            return True

    def configure(self, target: str, build: Path, panic: bool = False) -> None:
        command = [
            str(self.state / "venv/bin/west"),
            "build",
            "--cmake-only",
            "--board",
            target,
            "--build-dir",
            str(build),
            str(self.root / "gates/gate1a"),
        ]
        if panic:
            command += ["--", "-DEXTRA_CONF_FILE=panic.conf"]
        self.command("configure", command, target)

    def build(self, target: str, build: Path) -> str:
        cmake = "cmake"
        self.command("rust-compile", [cmake, "--build", str(build), "--target", "librustapp"], target)
        self.command("c-compile", [cmake, "--build", str(build), "--target", "zephyr_pre0"], target)
        self.command("link", [cmake, "--build", str(build), "--target", "zephyr_final"], target)
        elf = build / "zephyr/zephyr.elf"
        digest = sha256(elf)
        self.links.append({"target": target, "mode": build.name, "sha256": digest, "bytes": elf.stat().st_size})
        return digest

    def boot(self, target: str, build: Path, mode: str) -> None:
        start = time.monotonic()
        command = ["cmake", "--build", str(build), "--target", "run"]
        process = subprocess.Popen(
            command,
            cwd=self.state,
            env=self.env,
            stdout=subprocess.PIPE,
            stderr=subprocess.STDOUT,
            start_new_session=True,
            bufsize=0,
        )
        assert process.stdout is not None
        selector = selectors.DefaultSelector()
        selector.register(process.stdout, selectors.EVENT_READ)
        seen: list[str] = []
        pending = b""
        success: Callable[[list[str]], bool]
        if mode == "normal":
            success = lambda lines: all(any(marker in line for line in lines) for marker in NORMAL_MARKERS)
        else:
            success = lambda lines: any("event=panic_trigger reason=deliberate" in line for line in lines) and any("panic: panicked at" in line for line in lines)
        try:
            boot_deadline = min(self.deadline, time.monotonic() + 30)
            while time.monotonic() < boot_deadline:
                if self.cancelled:
                    self.recorder.event("boot", "cancel", round((time.monotonic() - start) * 1000), "cancelled", target)
                    raise GateCancelled("cancelled")
                ready = selector.select(timeout=0.2)
                for key, _ in ready:
                    chunk = os.read(key.fileobj.fileno(), 65536)
                    if not chunk:
                        break
                    pending += chunk
                    lines = pending.split(b"\n")
                    pending = lines.pop()
                    for raw in lines:
                        line = raw.decode("utf-8", errors="replace")
                        clean = ANSI_RE.sub("", line)
                        seen.append(clean)
                        self.recorder.add_serial(target, mode, clean)
                if success(seen):
                    self.recorder.event("boot", "success", round((time.monotonic() - start) * 1000), target=target)
                    return
                if process.poll() is not None:
                    break
            if time.monotonic() >= self.deadline:
                self.recorder.event("boot", "timeout", round((time.monotonic() - start) * 1000), "deadline_exceeded", target)
                raise GateTimeout("deadline_exceeded")
            self.recorder.event("boot", "error", round((time.monotonic() - start) * 1000), "expected_marker_missing", target)
            raise GateFailure("expected_marker_missing")
        finally:
            if not self._terminate_group(process):
                self.cleanup_status = "failed"
            selector.close()
            process.stdout.close()

    def target(self, target: str) -> None:
        target_root = self.state / "build/gate1a" / self.run_id / target
        normal = target_root / "normal"
        self.configure(target, normal)
        first = self.build(target, normal)
        self.boot(target, normal, "normal")
        self.supported_targets.add(target)
        shutil.rmtree(normal)
        self.configure(target, normal)
        second = self.build(target, normal)
        if first != second:
            raise GateFailure("clean_rebuild_digest_mismatch")
        self.clean_rebuilds.add(target)
        self.recorder.event("probe", "success", 0, target=target)
        panic = target_root / "panic"
        self.configure(target, panic, panic=True)
        self.build(target, panic)
        self.boot(target, panic, "panic")
        self.deliberate_panics.add(target)

    def resources(self) -> dict[str, object]:
        return self.verified_resources

    def run(self) -> tuple[int, dict[str, object]]:
        result = "pass"
        reason = "all_criteria_passed"
        exit_code = 0
        cleanup = "success"
        try:
            self.prepare()
            for target in TARGETS:
                self.target(target)
        except GateCancelled:
            result, reason, exit_code = "inconclusive", "cancelled", 130
        except GateTimeout:
            result, reason, exit_code = "inconclusive", "deadline_exceeded", 124
        except GateInconclusive as error:
            result, reason, exit_code = "inconclusive", error.reason, 2
        except GateFailure as error:
            result, reason, exit_code = "fail", error.reason, 1
        except (OSError, subprocess.SubprocessError) as error:
            result, reason, exit_code = "inconclusive", f"setup_{type(error).__name__.lower()}", 2
        cleanup = self.cleanup_status
        if cleanup != "success" and exit_code not in (124, 130):
            result, reason, exit_code = "inconclusive", "cleanup_failed", 2
        self.recorder.finalize(self.resources(), self.links, result, reason)
        value = {
            "schema_version": SCHEMA_VERSION,
            "gate": "1a",
            "mode": "default",
            "run_id": self.run_id,
            "result": result,
            "reason_code": reason,
            "cleanup_status": cleanup,
            "criteria": [
                {"name": "supported_targets", "value": len(self.supported_targets), "unit": "targets", "threshold": 2, "passed": len(self.supported_targets) == 2},
                {"name": "clean_rebuilds", "value": len(self.clean_rebuilds), "unit": "targets", "threshold": 2, "passed": len(self.clean_rebuilds) == 2},
                {"name": "deliberate_panics", "value": len(self.deliberate_panics), "unit": "targets", "threshold": 2, "passed": len(self.deliberate_panics) == 2},
            ],
            "started_at": self.started,
            "ended_at": utc_now(),
        }
        return exit_code, value


def acquire_lock(state: Path, run_id: str):
    locks = state / "locks"
    private_directory(locks)
    path = locks / "1a.lock"
    stream = path.open("a+", encoding="utf-8")
    os.chmod(path, 0o600)
    try:
        fcntl.flock(stream, fcntl.LOCK_EX | fcntl.LOCK_NB)
    except BlockingIOError:
        stream.seek(0)
        owner = stream.read().strip() or "unknown"
        stream.close()
        raise GateFailure(f"gate_locked owner={owner}") from None
    stream.seek(0)
    stream.truncate()
    json.dump({"gate": "1a", "run_id": run_id, "pid": os.getpid(), "started_at": utc_now()}, stream, sort_keys=True)
    stream.write("\n")
    stream.flush()
    os.fsync(stream.fileno())
    return stream


def diagnostics_list(root: Path) -> int:
    base = root / ".deskkin/diagnostics"
    if not base.exists():
        return 0
    for directory in sorted((item for item in base.iterdir() if item.is_dir() and not item.is_symlink()), key=lambda item: item.stat().st_mtime, reverse=True):
        files = [item for item in directory.rglob("*") if item.is_file()]
        completeness = "partial"
        result = "unknown"
        gate = "unknown"
        mode = "unknown"
        published = False
        diagnostic = directory / "diagnostic.jsonl"
        records = read_diagnostic_records(base, directory.name)
        if records is not None:
            for item in records:
                if item.get("type") == "resource":
                    gate = str(item.get("gate", gate))
                    mode = str(item.get("mode", mode))
                if item.get("type") == "completeness":
                    completeness = str(item.get("status"))
                    result = str(item.get("result", result))
                if item.get("type") == "result_published" and item.get("run_id") == directory.name:
                    published = True
        result_path = root / f".deskkin/results/{gate}/{mode}/result.json"
        result_matches = False
        if result_path.exists():
            value = read_result_safe(result_path, gate, mode)
            if value is not None and value.get("run_id") == directory.name:
                result = str(value.get("result"))
                result_matches = True
        if gate == "1b" and completeness == "complete" and not (result_matches or published):
            completeness = "partial"
            result = "unknown"
        print(f"{directory.name} {gate} {mode} {datetime.fromtimestamp(directory.stat().st_mtime, UTC).isoformat()} {result} {completeness} {sum(item.stat().st_size for item in files)}")
    return 0


def diagnostic_store_usage(root: Path) -> tuple[int, int]:
    base = root / ".deskkin/diagnostics"
    if not base.exists():
        return 0, 0
    directories = [item for item in base.iterdir() if item.is_dir()]
    return len(directories), sum(
        item.stat().st_size
        for directory in directories
        for item in directory.rglob("*")
        if item.is_file()
    )


def prune_diagnostics(root: Path, reserve_runs: int = 0, reserve_bytes: int = 0) -> None:
    base = root / ".deskkin/diagnostics"
    if not base.exists():
        return
    now = datetime.now(UTC)
    records: list[dict[str, object]] = []
    for directory in (item for item in base.iterdir() if item.is_dir() and not item.is_symlink()):
        modified = datetime.fromtimestamp(directory.stat().st_mtime, UTC)
        size = sum(item.stat().st_size for item in directory.rglob("*") if item.is_file())
        outcome = "unknown"
        reason = "unknown"
        gate = "unknown"
        published = False
        values = read_diagnostic_records(base, directory.name)
        if values is not None:
            for value in values:
                if value.get("type") == "completeness":
                    outcome = str(value.get("result", outcome))
                    reason = str(value.get("reason_code", reason))
                elif value.get("type") == "resource":
                    gate = str(value.get("gate", gate))
                elif value.get("type") == "result_published" and value.get("run_id") == directory.name:
                    published = True
        if gate == "1b" and outcome == "pass" and not published:
            outcome = "unknown"
            reason = "result_not_published"
        records.append({"path": directory, "modified": modified, "size": size, "outcome": outcome, "reason": reason, "gate": gate, "frozen": (directory / ".frozen").exists()})
    records.sort(key=lambda item: item["modified"], reverse=True)
    active_runs: set[str] = set()
    lock_scan_safe = True
    locks = root / ".deskkin/locks"
    if locks.is_symlink() or (locks.exists() and not locks.is_dir()):
        return
    if locks.is_dir():
        lock_base_fd: int | None = None
        try:
            lock_base_fd = os.open(locks, os.O_RDONLY | getattr(os, "O_DIRECTORY", 0) | getattr(os, "O_NOFOLLOW", 0))
            for name in os.listdir(lock_base_fd):
                if not name.endswith(".lock"):
                    continue
                stream = None
                try:
                    metadata = os.stat(name, dir_fd=lock_base_fd, follow_symlinks=False)
                    if not stat.S_ISREG(metadata.st_mode) or metadata.st_size > 4096:
                        raise OSError("ambiguous lock entry")
                    descriptor = os.open(name, os.O_RDONLY | os.O_NONBLOCK | getattr(os, "O_NOFOLLOW", 0), dir_fd=lock_base_fd)
                    stream = os.fdopen(descriptor, "r", encoding="utf-8")
                    try:
                        fcntl.flock(stream, fcntl.LOCK_EX | fcntl.LOCK_NB)
                        fcntl.flock(stream, fcntl.LOCK_UN)
                    except BlockingIOError:
                        try:
                            value = json.load(stream)
                            owner = str(value["run_id"])
                            if not re.fullmatch(r"[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}", owner):
                                raise ValueError("invalid owner")
                            active_runs.add(owner)
                        except (UnicodeError, json.JSONDecodeError, KeyError, RecursionError, ValueError):
                            lock_scan_safe = False
                except (OSError, UnicodeError, json.JSONDecodeError, KeyError, RecursionError):
                    lock_scan_safe = False
                finally:
                    if stream is not None:
                        stream.close()
                if not lock_scan_safe:
                    break
        except OSError:
            lock_scan_safe = False
        finally:
            if lock_base_fd is not None:
                os.close(lock_base_fd)
    if not lock_scan_safe:
        return
    protected: set[Path] = set()
    for gate in {str(item["gate"]) for item in records}:
        successes = [item for item in records if item["gate"] == gate and item["outcome"] == "pass"]
        failures = [item for item in records if item["gate"] == gate and item["outcome"] != "pass" and item["reason"] != "cancelled"]
        if successes:
            protected.add(successes[0]["path"])
        protected.update(item["path"] for item in failures[:3])
    total = sum(int(item["size"]) for item in records)
    count = len(records)
    for item in list(reversed(records)):
        if bool(item["frozen"]):
            continue
        if item["path"].name in active_runs:
            continue
        if now - item["modified"] > timedelta(days=14):
            shutil.rmtree(item["path"])
            count -= 1
            total -= int(item["size"])
            records.remove(item)
    def priority(item: dict[str, object]) -> tuple[int, datetime]:
        path = item["path"]
        if path in protected:
            group = 3
        elif item["outcome"] == "pass":
            group = 0
        elif item["reason"] == "cancelled":
            group = 1
        else:
            group = 2
        return group, item["modified"]
    candidates = sorted((item for item in records if not bool(item["frozen"]) and item["path"].name not in active_runs), key=priority)
    for item in candidates:
        if count + reserve_runs <= STORE_LIMIT_RUNS and total + reserve_bytes <= STORE_LIMIT_BYTES:
            break
        shutil.rmtree(item["path"])
        count -= 1
        total -= int(item["size"])


def recording_capacity(root: Path) -> bool:
    count, size = diagnostic_store_usage(root)
    return count < STORE_LIMIT_RUNS and size + 64 * 1024 <= STORE_LIMIT_BYTES


def publish_recording_health(root: Path, run_id: str, status: str, reason: str) -> None:
    atomic_json(
        root / ".deskkin/diagnostics/.recording-health.json",
        {"schema_version": 1, "run_id": run_id, "status": status, "reason": reason, "timestamp": utc_now()},
    )


def initialize_recording(root: Path, recorder: Recorder, gate: str = "1a", target: list[str] | None = None, mode: str = "default") -> None:
    if recorder.directory is None:
        return
    try:
        private_directory(root / ".deskkin/diagnostics")
        prune_diagnostics(root, reserve_runs=1, reserve_bytes=64 * 1024)
        if not recording_capacity(root):
            recorder.health = "dropped"
            recorder.health_reason = "store_capacity_exceeded"
            recorder.directory = None
        else:
            resource: dict[str, object] = {"run_id": recorder.run_id, "gate": gate, "mode": mode}
            if target is not None:
                resource["target"] = target
            recorder.start(resource)
    except (OSError, json.JSONDecodeError) as error:
        recorder.health = "dropped"
        recorder.health_reason = f"recording_io_{getattr(error, 'errno', None) or 'invalid'}"
        recorder.directory = None
    if recorder.directory is None:
        with contextlib.suppress(OSError):
            publish_recording_health(root, recorder.run_id, "dropped", recorder.health_reason or "recording_start_failed")


def diagnostics_delete(root: Path, run_id: str) -> int:
    if not re.fullmatch(r"[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}", run_id):
        print("invalid diagnostic run ID", file=sys.stderr)
        return 2
    base = root / ".deskkin/diagnostics"
    if base.is_symlink() or not base.is_dir():
        print("diagnostic run not found", file=sys.stderr)
        return 2
    flags = os.O_RDONLY | getattr(os, "O_DIRECTORY", 0) | getattr(os, "O_NOFOLLOW", 0)
    descriptor: int | None = None
    try:
        descriptor = os.open(base, flags)
        metadata = os.stat(run_id, dir_fd=descriptor, follow_symlinks=False)
        if stat.S_ISLNK(metadata.st_mode) or not stat.S_ISDIR(metadata.st_mode):
            raise OSError("diagnostic run is not a real directory")
        shutil.rmtree(run_id, dir_fd=descriptor)
    except OSError:
        print("diagnostic run not found", file=sys.stderr)
        return 2
    finally:
        if descriptor is not None:
            os.close(descriptor)
    return 0


def main() -> int:
    parser = argparse.ArgumentParser()
    subparsers = parser.add_subparsers(dest="command", required=True)
    run_parser = subparsers.add_parser("run")
    run_parser.add_argument("--recording", choices=("on", "off"), default="on")
    subparsers.add_parser("list")
    delete_parser = subparsers.add_parser("delete")
    delete_parser.add_argument("run_id")
    args = parser.parse_args()
    root = Path(__file__).resolve().parents[1]
    if args.command == "list":
        return diagnostics_list(root)
    if args.command == "delete":
        return diagnostics_delete(root, args.run_id)
    run_id = str(uuid.uuid4())
    print(run_id, flush=True)
    try:
        prepare_control_state(root)
        runner = Runner(root, args.recording == "on", run_id)
    except OSError:
        print(f"Gate 1A setup failed; run_id={run_id}; private state is unavailable", file=sys.stderr)
        return 2
    try:
        lock = acquire_lock(runner.state, run_id)
    except GateFailure as error:
        print(f"Gate 1A could not start: {error.reason}", file=sys.stderr)
        return 2
    result_path = runner.state / "results/1a/default/result.json"

    def cancel(_signum, _frame):
        runner.cancelled = True

    signal.signal(signal.SIGINT, cancel)
    with lock:
        if not clear_result(result_path):
            print(f"Gate 1A setup failed; run_id={run_id}; stale result could not be removed", file=sys.stderr)
            return 2
        initialize_recording(root, runner.recorder)
        exit_code, result = runner.run()
        if not publish_result(result_path, result):
            print(f"Gate 1A setup failed; run_id={run_id}; current result could not be published", file=sys.stderr)
            return 2
        if runner.recorder.started:
            with contextlib.suppress(OSError, json.JSONDecodeError):
                prune_diagnostics(root)
    if exit_code == 2:
        print(f"Gate 1A setup inconclusive: {result['reason_code']}; run_id={run_id}", file=sys.stderr)
    print(f"Gate 1A {result['result']} ({result['reason_code']})")
    print(result_path.relative_to(root))
    return exit_code


if __name__ == "__main__":
    raise SystemExit(main())
