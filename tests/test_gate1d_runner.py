import sys
import json
import tempfile
import unittest
import uuid
from pathlib import Path

SCRIPTS = Path(__file__).resolve().parents[1] / "scripts"
sys.path.insert(0, str(SCRIPTS))
import device_recover
import gate1d_runner as gate


class Gate1DRunnerTests(unittest.TestCase):
    def test_missing_device_preserves_host_evidence(self):
        class HostOnly(gate.Runner):
            def prepare(self):
                pass

            def execute(self, requested_device=None):
                self.builds = self.rebuilds = self.linker_checks = 1
                raise gate.common.GateInconclusive("physical_device_required")

        with tempfile.TemporaryDirectory() as directory:
            runner = HostOnly(Path(directory), False, str(uuid.uuid4()), "a" * 64)
            code, result = runner.run()
        self.assertEqual((code, result["result"], result["reason_code"]), (2, "inconclusive", "physical_device_required"))
        criteria = {item["name"]: item for item in result["criteria"]}
        self.assertTrue(criteria["xtensa_builds"]["passed"])
        self.assertFalse(criteria["physical_boots"]["passed"])
        self.assertEqual(result["device_state"], "unchanged")

    def test_serial_protocol_is_fixed(self):
        run_id = str(uuid.uuid4())
        line = (
            f"DESKKIN_GATE_EVENT schema=1 event=display_rect run_id={run_id} "
            "index=1 x=20 y=20 width=80 height=60 bytes=9600 duration_us=123 status=ok"
        )
        record = gate.parse_serial_record(line, gate.TARGET, "normal")
        self.assertEqual(record["duration_us"], 123)
        self.assertEqual(record["bytes"], 9600)
        self.assertIsNone(gate.parse_serial_record(line + " arbitrary=payload", gate.TARGET, "normal"))

    def test_zero_duration_is_not_accepted_as_transfer_evidence(self):
        source = Path(gate.__file__).read_text(encoding="utf-8")
        self.assertIn('record.get("duration_us", 0) <= 0', source)

    def test_firmware_fail_path_reports_idle_for_classification(self):
        source = (Path(__file__).resolve().parents[1] / "gates/gate1d/src/main.c").read_text(
            encoding="utf-8"
        )
        fail = source.index('result=fail')
        idle = source.index('event=idle', fail)
        self.assertGreater(idle, fail)

    def test_dirty_west_driver_tree_fails_closed(self):
        class Dirty(gate.Runner):
            def command(self, operation, command, target):
                return " M drivers/display/display_ili9xxx.c\n"

        with tempfile.TemporaryDirectory() as directory:
            runner = Dirty(Path(directory), False, str(uuid.uuid4()), "a" * 64)
            runner.west_paths = {"zephyr": Path(directory) / "zephyr"}
            with self.assertRaisesRegex(gate.common.GateInconclusive, "input_changed"):
                runner._verify_west_trees("input_changed")

    def test_rejected_touch_sample_is_run_bound_and_structured(self):
        run_id = str(uuid.uuid4())
        line = (
            f"DESKKIN_GATE_EVENT schema=1 event=touch_sample run_id={run_id} "
            "expected_index=2 x=57 y=50 inside=no"
        )
        record = gate.parse_serial_record(line, gate.TARGET, "normal")
        self.assertEqual(record["expected_index"], 2)
        self.assertEqual(record["inside"], "no")

    def test_preflight_accepts_only_recognized_firmware_without_touch(self):
        class Observing(gate.Runner):
            def serial_exchange(self, action, mode, required, timeout, allowed_digests=None):
                self.observed = (action, mode, required, allowed_digests)
                return []

        root = Path(__file__).resolve().parents[1]
        runner = Observing(root, False, str(uuid.uuid4()))
        runner.preflight()
        self.assertEqual(runner.observed[:3], ("status", "preflight", {"idle"}))
        self.assertTrue(
            {runner.firmware_digest, gate.gate1c.firmware_digest(root)}
            <= runner.observed[3]
        )
        self.assertLessEqual(len(runner.observed[3]), 3)
        self.assertFalse(runner.device_touched)

    def test_preflight_accepts_last_confirmed_idle_digest(self):
        class Observing(gate.Runner):
            def serial_exchange(self, action, mode, required, timeout, allowed_digests=None):
                self.allowed = allowed_digests
                return []

        root = Path(__file__).resolve().parents[1]
        with tempfile.TemporaryDirectory() as directory:
            runner = Observing(root, False, str(uuid.uuid4()))
            runner.state = Path(directory)
            result_path = runner.state / "results/1d/default/result.json"
            result_path.parent.mkdir(parents=True)
            previous = {
                "schema_version": 1,
                "gate": "1d",
                "mode": "default",
                "run_id": str(uuid.uuid4()),
                "result": "inconclusive",
                "reason_code": "serial_protocol_timeout",
                "cleanup_status": "success",
                "device_state": "test_firmware_idle",
                "firmware_digest": "b" * 64,
                "criteria": [],
                "started_at": gate.common.utc_now(),
                "ended_at": gate.common.utc_now(),
            }
            result_path.write_text(json.dumps(previous), encoding="utf-8")
            runner.preflight()
        self.assertIn("b" * 64, runner.allowed)

    def test_unknown_preflight_never_touches_device(self):
        class Unknown(gate.Runner):
            def serial_exchange(self, action, mode, required, timeout, allowed_digests=None):
                raise gate.common.GateInconclusive("firmware_digest_mismatch")

        root = Path(__file__).resolve().parents[1]
        runner = Unknown(root, False, str(uuid.uuid4()), "a" * 64)
        with self.assertRaisesRegex(gate.common.GateInconclusive, "device_state_unknown"):
            runner.preflight()
        self.assertFalse(runner.device_touched)

    def test_cleanup_failure_overrides_apparent_pass(self):
        class Unsafe(gate.Runner):
            def prepare(self):
                pass

            def execute(self, requested_device=None):
                self.builds = self.rebuilds = self.linker_checks = 1
                self.physical_boots = self.device_checks = self.psram_checks = 1
                self.flash_checks = self.panel_checks = self.idle_checks = 1
                self.display_rect_checks = self.touch_checks = 3
                self.cleanup_status = "failed"
                self.device_state = "unknown"

        with tempfile.TemporaryDirectory() as directory:
            runner = Unsafe(Path(directory), False, str(uuid.uuid4()), "a" * 64)
            code, result = runner.run()
        self.assertEqual((code, result["result"], result["reason_code"]), (2, "inconclusive", "device_cleanup_failed"))

    def test_cleanup_has_fresh_deadline_when_cancelled(self):
        class Recovering(gate.Runner):
            def flash(self, build):
                self.device_touched = True

            def serial_exchange(self, action, mode, required, timeout, allowed_digests=None):
                self.observed = (self.cancelled, mode, timeout)
                return []

        with tempfile.TemporaryDirectory() as directory:
            runner = Recovering(Path(directory), False, str(uuid.uuid4()), "a" * 64)
            runner.device_touched = True
            runner.cancelled = True
            runner.deadline = 0
            runner.cleanup_device(Path(directory) / "normal")
        self.assertFalse(runner.observed[0])
        self.assertEqual(runner.observed[1], "cleanup")
        self.assertGreater(runner.observed[2], 9.0)
        self.assertTrue(runner.cancelled)
        self.assertEqual(runner.device_state, "test_firmware_idle")

    def test_recovery_dispatches_only_exact_recognized_digest(self):
        root = Path(__file__).resolve().parents[1]
        gate1c_digest = gate.gate1c.firmware_digest(root)
        gate1d_digest = gate.firmware_digest(root)
        self.assertEqual(device_recover.runner_for_digest(root, gate1c_digest).name, "gate1c_runner.py")
        self.assertEqual(device_recover.runner_for_digest(root, gate1d_digest).name, "gate1d_runner.py")
        self.assertIsNone(device_recover.runner_for_digest(root, "0" * 64))

    def test_gate1d_records_conform_to_shared_schema(self):
        run_id = str(uuid.uuid4())
        resource = {"schema_version": 1, "type": "resource", "run_id": run_id, "gate": "1d", "mode": "recover", "target": [gate.TARGET]}
        completeness = {"schema_version": 1, "type": "completeness", "status": "complete", "reason": None, "result": "inconclusive", "reason_code": "serial_protocol_timeout"}
        self.assertTrue(gate.common.diagnostic_records_safe([resource, completeness], run_id))


if __name__ == "__main__":
    unittest.main()
