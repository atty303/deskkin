from __future__ import annotations

import json
import os
import sys
import tempfile
import unittest
import uuid
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
SCRIPTS = ROOT / "scripts"
sys.path.insert(0, str(SCRIPTS))
import gate1e_runner as gate
import device_recover

common = gate.common


class Gate1ERunnerTests(unittest.TestCase):
    def test_serial_protocol_accepts_only_fixed_records(self):
        run_id = str(uuid.uuid4())
        summary = (
            f"DESKKIN_GATE1E_SUMMARY schema=1 run_id={run_id} phase=disabled "
            "frames=1800 samples=1740 render_p95_us=1000 transfer_p95_us=4000 "
            "combined_p95_us=5000 combined_p99_us=6000 touch_p95_us=7000 "
            "missed_frames=0 max_dirty_pixels=5000 post_initial_full_frames=0 "
            "touches=60 semantic_digest=0123456789abcdef framebuffer_digest=fedcba9876543210"
        )
        record = gate.parse_serial_record(summary, gate.TARGET, "qualification")
        self.assertIsNotNone(record)
        assert record is not None
        self.assertEqual(record["render_p95_us"], 1000)
        self.assertEqual(record["phase"], "disabled")
        self.assertIsNone(gate.parse_serial_record(summary + " arbitrary=value", gate.TARGET, "qualification"))

    def test_frame_protocol_is_numeric_and_run_bound(self):
        run_id = str(uuid.uuid4())
        line = (
            f"DESKKIN_GATE1E_FRAME schema=1 run_id={run_id} phase=enabled frame=60 "
            "render_us=1000 transfer_us=4000 combined_us=5000 dirty_pixels=3000 "
            "touch_latency_us=7000 missed=no"
        )
        record = gate.parse_serial_record(line, gate.TARGET, "qualification")
        self.assertIsNotNone(record)
        assert record is not None
        self.assertEqual(record["frame"], 60)
        self.assertEqual(record["missed"], "no")

    def test_runtime_error_protocol_retains_phase_and_frame(self):
        run_id = str(uuid.uuid4())
        line = (
            f"DESKKIN_GATE1E_ERROR schema=1 run_id={run_id} phase=enabled "
            "error_type=initial_frame_incomplete frame=0"
        )
        record = gate.parse_serial_record(line, gate.TARGET, "qualification")
        self.assertIsNotNone(record)
        assert record is not None
        self.assertEqual(record["error_type"], "initial_frame_incomplete")
        self.assertEqual(record["frame"], 0)

    def test_gate1e_resource_modes_are_allowlisted(self):
        for mode in ("qualification", "conformance"):
            value = {
                "schema_version": 1,
                "type": "resource",
                "run_id": str(uuid.uuid4()),
                "gate": "1e",
                "mode": mode,
                "target": [gate.TARGET],
            }
            self.assertTrue(common.record_schema_safe(value))

    def test_measurement_writer_is_private_and_bounded(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "frames.jsonl"
            writer = gate.MeasurementWriter(path)
            writer.start()
            writer.submit({"schema_version": 1, "frame": 60, "render_us": 1})
            self.assertTrue(writer.stop())
            self.assertEqual(os.stat(path).st_mode & 0o777, 0o600)
            self.assertEqual(json.loads(path.read_text(encoding="utf-8"))["frame"], 60)

    def test_thresholds_cover_each_phase_and_recording_overhead(self):
        runner = gate.Runner(ROOT, True, str(uuid.uuid4()), "qualification")
        base = {
            "render_p95_us": 10_000,
            "transfer_p95_us": 10_000,
            "combined_p95_us": 20_000,
            "combined_p99_us": 30_000,
            "touch_p95_us": 80_000,
            "missed_frames": 10,
            "max_dirty_pixels": 10_000,
            "post_initial_full_frames": 0,
        }
        criteria = runner._criteria({"disabled": dict(base), "enabled": dict(base)})
        self.assertEqual(len(criteria), 17)
        self.assertTrue(all(item["passed"] for item in criteria))
        failing = dict(base, combined_p99_us=33_301)
        criteria = runner._criteria({"disabled": dict(base), "enabled": failing})
        self.assertFalse(next(item for item in criteria if item["name"] == "enabled_combined_p99")["passed"])

    def test_conformance_rejects_any_frame_recording(self):
        runner = gate.Runner(ROOT, False, str(uuid.uuid4()), "conformance")
        runner._validate_frame_records()
        runner.frames.append({"frame": 60})
        with self.assertRaisesRegex(common.GateFailure, "recording_opt_out_failed"):
            runner._validate_frame_records()

    def test_workload_identity_binds_firmware_and_ui(self):
        firmware = gate.firmware_digest(ROOT)
        first = gate.workload_digest(ROOT, firmware)
        second = gate.workload_digest(ROOT, firmware)
        self.assertRegex(first, r"^[0-9a-f]{64}$")
        self.assertEqual(first, second)
        self.assertNotEqual(first, gate.workload_digest(ROOT, "0" * 64))

    def test_device_recovery_dispatches_gate1e_digest(self):
        runner = device_recover.runner_for_digest(ROOT, gate.firmware_digest(ROOT))
        self.assertIsNotNone(runner)
        assert runner is not None
        self.assertEqual(runner.name, "gate1e_runner.py")

    def test_timeout_reason_is_bounded_to_parsed_event_names(self):
        self.assertEqual(set(gate.SERIAL_PATTERNS), {"idle", "boot", "summary", "frame", "runtime_error", "result"})


if __name__ == "__main__":
    unittest.main()
