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

            def execute(self):
                self.builds = self.rebuilds = self.panic_builds = 1
                self.linker_checks = self.abi_symbols = self.builtins_checks = 1
                raise gate.common.GateInconclusive("physical_device_required")

        with tempfile.TemporaryDirectory() as directory:
            runner = HostOnly(Path(directory), False, str(uuid.uuid4()))
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
            runner = gate.Runner(root, False, str(uuid.uuid4()))
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

    def test_result_publication_is_private(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "result.json"
            value = {"schema_version": 1, "gate": "1c", "mode": "default", "run_id": str(uuid.uuid4()), "result": "inconclusive", "reason_code": "physical_device_required", "cleanup_status": "success", "criteria": [], "started_at": gate.common.utc_now(), "ended_at": gate.common.utc_now()}
            self.assertTrue(gate.common.publish_result(path, value))
            self.assertEqual(stat.S_IMODE(path.stat().st_mode), 0o600)


if __name__ == "__main__":
    unittest.main()
