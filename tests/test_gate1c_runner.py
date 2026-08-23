import importlib.util
import json
import stat
import sys
import tempfile
import unittest
import uuid
from pathlib import Path

SCRIPTS = Path(__file__).resolve().parents[1] / "scripts"
sys.path.insert(0, str(SCRIPTS))
SPEC = importlib.util.spec_from_file_location("gate1c_runner", SCRIPTS / "gate1c_runner.py")
assert SPEC and SPEC.loader
gate = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = gate
SPEC.loader.exec_module(gate)


class Gate1CRunnerTests(unittest.TestCase):
    def test_missing_device_preserves_host_evidence_as_inconclusive(self):
        class HostOnly(gate.Runner):
            def prepare(self):
                pass

            def execute(self, requested_device=None):
                self.builds = self.rebuilds = self.panic_builds = 1
                self.linker_checks = self.abi_symbols = self.builtins_checks = 1
                raise gate.common.GateInconclusive("physical_device_required")

        with tempfile.TemporaryDirectory() as directory:
            runner = HostOnly(Path(directory), False, str(uuid.uuid4()), "a" * 64)
            code, result = runner.run()
        self.assertEqual((code, result["result"], result["reason_code"]), (2, "inconclusive", "physical_device_required"))
        criteria = {item["name"]: item for item in result["criteria"]}
        self.assertTrue(criteria["xtensa_builds"]["passed"])
        self.assertTrue(criteria["clean_rebuilds"]["passed"])
        self.assertTrue(criteria["panic_builds"]["passed"])
        self.assertTrue(criteria["linker_checks"]["passed"])
        self.assertFalse(criteria["physical_boots"]["passed"])
        self.assertEqual(result["device_state"], "unchanged")

    def test_input_change_fails_closed(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            path = root / "input"
            path.write_text("before")
            runner = gate.Runner(root, False, str(uuid.uuid4()), "a" * 64)
            runner.verified_resources["input_digests"] = {"input": gate.common.sha256(path)}
            path.write_text("after")
            with self.assertRaises(gate.common.GateInconclusive):
                runner.verify_inputs()

    def test_concurrent_lock_reports_owner_without_mutation(self):
        with tempfile.TemporaryDirectory() as directory:
            state = Path(directory) / ".deskkin"
            state.mkdir()
            owner = str(uuid.uuid4())
            first = gate.acquire_lock(state, owner)
            try:
                with self.assertRaisesRegex(gate.common.GateInconclusive, owner):
                    gate.acquire_lock(state, str(uuid.uuid4()))
                first.seek(0)
                self.assertEqual(json.load(first)["run_id"], owner)
            finally:
                first.close()

    def test_lock_rejects_symlink_without_touching_target(self):
        with tempfile.TemporaryDirectory() as directory:
            state = Path(directory) / ".deskkin"
            locks = state / "locks"
            locks.mkdir(parents=True)
            outside = Path(directory) / "outside"
            outside.write_text("unchanged")
            (locks / "1c.lock").symlink_to(outside)
            with self.assertRaises(OSError):
                gate.acquire_lock(state, str(uuid.uuid4()))
            self.assertEqual(outside.read_text(), "unchanged")

    def test_gate1c_records_are_schema_and_privacy_safe(self):
        run_id = str(uuid.uuid4())
        resource = {"schema_version": 1, "type": "resource", "run_id": run_id, "gate": "1c", "mode": "default", "target": [gate.TARGET]}
        completeness = {"schema_version": 1, "type": "completeness", "status": "complete", "reason": None, "result": "inconclusive", "reason_code": "physical_device_required"}
        self.assertTrue(gate.common.diagnostic_records_safe([resource, completeness], run_id))
        unsafe = dict(resource, accessToken="SENSITIVE_FIXTURE")
        self.assertFalse(gate.common.diagnostic_records_safe([unsafe, completeness], run_id))

    def test_recovery_records_use_the_recover_mode(self):
        run_id = str(uuid.uuid4())
        resource = {"schema_version": 1, "type": "resource", "run_id": run_id, "gate": "1c", "mode": "recover", "target": [gate.TARGET]}
        verified = dict(resource, type="resource_verified", west_revisions={}, sdk_file_digests={}, input_digests={}, tool_identities={}, sdk_version="1.0.1", application_version="gate1c-0.1.0", build_type="dev", deskkin_revision="a" * 40, deskkin_dirty=True)
        completeness = {"schema_version": 1, "type": "completeness", "status": "complete", "reason": None, "result": "inconclusive", "reason_code": "serial_protocol_timeout"}
        self.assertTrue(gate.common.diagnostic_records_safe([resource, verified, completeness], run_id))

    def test_physical_operations_are_recordable(self):
        run_id = str(uuid.uuid4())
        for operation in ("flash", "preflight", "postflash", "cleanup", "recover"):
            record = {"schema_version": 1, "type": "operation", "run_id": run_id, "operation": operation, "status": "success", "duration_ms": 1, "target": gate.TARGET}
            self.assertTrue(gate.common.record_schema_safe(record))

    def test_result_publication_is_private(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "result.json"
            value = {"schema_version": 1, "gate": "1c", "mode": "default", "run_id": str(uuid.uuid4()), "result": "inconclusive", "reason_code": "physical_device_required", "cleanup_status": "success", "criteria": [], "started_at": gate.common.utc_now(), "ended_at": gate.common.utc_now()}
            self.assertTrue(gate.common.publish_result(path, value))
            self.assertEqual(stat.S_IMODE(path.stat().st_mode), 0o600)

    def test_gate1c_serial_protocol_accepts_only_fixed_records(self):
        run_id = str(uuid.uuid4())
        digest = "a" * 64
        line = f"DESKKIN_GATE_EVENT schema=1 event=idle run_id={run_id} firmware_digest={digest}"
        self.assertEqual(
            gate.parse_serial_record(line, gate.TARGET, "cleanup"),
            {
                "schema_version": 1,
                "target": gate.TARGET,
                "mode": "cleanup",
                "event": "idle",
                "run_id": run_id,
                "firmware_digest": digest,
            },
        )
        self.assertIsNone(gate.parse_serial_record(line + " arbitrary=payload", gate.TARGET, "cleanup"))

    def test_preflight_unknown_firmware_never_marks_device_touched(self):
        class Unknown(gate.Runner):
            def serial_exchange(self, action, mode, required, timeout):
                raise gate.common.GateInconclusive("serial_protocol_timeout")

        with tempfile.TemporaryDirectory() as directory:
            runner = Unknown(Path(directory), False, str(uuid.uuid4()), "a" * 64)
            with self.assertRaisesRegex(gate.common.GateInconclusive, "device_state_unknown"):
                runner.preflight()
            self.assertFalse(runner.device_touched)
            self.assertEqual(runner.device_state, "unchanged")

    def test_preflight_preserves_cancellation(self):
        class Cancelled(gate.Runner):
            def serial_exchange(self, action, mode, required, timeout):
                raise gate.common.GateCancelled("cancelled")

        with tempfile.TemporaryDirectory() as directory:
            runner = Cancelled(Path(directory), False, str(uuid.uuid4()), "a" * 64)
            with self.assertRaises(gate.common.GateCancelled):
                runner.preflight()

    def test_cleanup_failure_overrides_an_apparent_pass(self):
        class UnsafeResidualState(gate.Runner):
            def prepare(self):
                pass

            def execute(self, requested_device=None):
                self.builds = self.rebuilds = self.panic_builds = 1
                self.linker_checks = self.abi_symbols = self.builtins_checks = 1
                self.physical_boots = self.c_abi_runtime_checks = 1
                self.atomic_runtime_checks = self.allocator_runtime_checks = 1
                self.idle_checks = self.deliberate_panics_count = 1
                self.cleanup_status = "failed"
                self.device_state = "unknown"

        with tempfile.TemporaryDirectory() as directory:
            runner = UnsafeResidualState(Path(directory), False, str(uuid.uuid4()), "a" * 64)
            code, result = runner.run()
            self.assertEqual((code, result["result"], result["reason_code"]), (2, "inconclusive", "device_cleanup_failed"))
            self.assertEqual(result["device_state"], "unknown")

    def test_cleanup_restores_normal_idle_with_cancellation_pending(self):
        class Recovering(gate.Runner):
            def flash(self, build):
                self.device_touched = True

            def serial_exchange(self, action, mode, required, timeout):
                self.assertions.append((self.cancelled, action, mode))
                return []

        with tempfile.TemporaryDirectory() as directory:
            runner = Recovering(Path(directory), False, str(uuid.uuid4()), "a" * 64)
            runner.assertions = []
            runner.device_touched = True
            runner.cancelled = True
            runner.cleanup_device(Path(directory) / "normal")
            self.assertEqual(runner.assertions, [(False, "status", "cleanup")])
            self.assertTrue(runner.cancelled)
            self.assertEqual(runner.cleanup_status, "success")
            self.assertEqual(runner.device_state, "test_firmware_idle")

    def test_cleanup_gets_a_fresh_ten_second_deadline_after_timeout(self):
        class TimedOut(gate.Runner):
            def flash(self, build):
                self.device_touched = True

            def serial_exchange(self, action, mode, required, timeout):
                self.cleanup_timeout = timeout
                return []

        with tempfile.TemporaryDirectory() as directory:
            runner = TimedOut(Path(directory), False, str(uuid.uuid4()), "a" * 64)
            runner.device_touched = True
            runner.deadline = 0
            runner.cleanup_device(Path(directory) / "normal")
            self.assertGreater(runner.cleanup_timeout, 9.0)
            self.assertEqual(runner.deadline, 0)


if __name__ == "__main__":
    unittest.main()
