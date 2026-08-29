import importlib.util
import json
import subprocess
import stat
import sys
import tempfile
import unittest
from pathlib import Path
from unittest import mock
import uuid

SCRIPTS = Path(__file__).resolve().parents[1] / "scripts"
sys.path.insert(0, str(SCRIPTS))
SPEC = importlib.util.spec_from_file_location("gate1b_runner", SCRIPTS / "gate1b_runner.py")
assert SPEC and SPEC.loader
gate = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = gate
SPEC.loader.exec_module(gate)


class Gate1BRunnerTests(unittest.TestCase):
    @staticmethod
    def valid_capture():
        capture = gate.FrameCapture()
        capture.initial_lines = set(range(gate.HEIGHT))
        capture.static_ranges = gate.HEIGHT
        capture.callback_count = capture.phase = 1
        capture.input_ranges = 1
        capture.stage_one_ranges = [(0, 0, 1)]
        capture.initial = [0] * (gate.WIDTH * gate.HEIGHT)
        frame = capture.initial.copy()
        frame[0] = 1
        capture.after_input = frame.copy()
        capture.stage_frames[0] = capture.initial.copy()
        capture.stage_frames[1] = frame.copy()
        capture.stage_ranges[1] = [(0, 0, 1)]
        for stage in range(2, 12):
            frame = frame.copy()
            frame[0] = 1 if frame[0] == 2 else 2
            capture.stage_frames[stage] = frame
            capture.stage_ranges[stage] = [(0, 0, 1)]
        capture.frame = frame
        capture.timer_waits, capture.busy_polls, capture.result = 10, 0, True
        return capture

    def test_rgb565_normalization_is_stable(self):
        self.assertEqual(gate.rgb565_bytes([0x0000, 0xF800, 0x07E0, 0x001F, 0xFFFF]), bytes((0, 0, 0, 255, 0, 0, 0, 255, 0, 0, 0, 255, 255, 255, 255)))

    def test_png_is_exact_rgb_without_metadata(self):
        image = gate.png_bytes(bytes(gate.WIDTH * gate.HEIGHT * 3))
        self.assertTrue(gate.valid_png(image))
        self.assertNotIn(b"tEXt", image)
        self.assertNotIn(b"iTXt", image)

    def test_frame_parser_rejects_bounds_and_unknown_schema(self):
        capture = gate.FrameCapture()
        with self.assertRaises(gate.common.GateFailure):
            capture.add("DESKKIN_GATE1B_FRAME schema=1 stage=0 line=240 start=0 end=1 rgb565=0000")
        with self.assertRaises(gate.common.GateFailure):
            capture.add("DESKKIN_GATE1B_EVENT schema=1 accessToken=SENSITIVE_FIXTURE")

    def test_frame_parser_requires_changed_pixels_inside_dirty_ranges(self):
        capture = gate.FrameCapture()
        capture.initial_lines = set(range(gate.HEIGHT))
        capture.static_ranges = gate.HEIGHT
        capture.callback_count = capture.phase = 1
        capture.input_ranges = 1
        capture.stage_one_ranges = [(0, 0, 1)]
        capture.after_input[1] = 1
        capture.timer_waits, capture.busy_polls, capture.result = 10, 0, True
        with self.assertRaises(gate.common.GateFailure):
            capture.validate()

    def test_frame_parser_rejects_conservative_dirty_superset(self):
        capture = gate.FrameCapture()
        capture.initial_lines = set(range(gate.HEIGHT))
        capture.static_ranges = gate.HEIGHT
        capture.callback_count = capture.phase = 1
        capture.input_ranges = 1
        capture.stage_one_ranges = [(0, 0, 2)]
        capture.after_input[0] = 1
        capture.timer_waits, capture.busy_polls, capture.result = 10, 0, True
        with self.assertRaises(gate.common.GateFailure):
            capture.validate()

    def test_animation_requires_rendered_stage_and_real_pixel_change(self):
        capture = self.valid_capture()
        self.assertEqual(capture.validate()["animation_stages"], 10)
        for stage in range(2, 12):
            del capture.stage_frames[stage]
        with self.assertRaises(gate.common.GateFailure):
            capture.validate()

    def test_measurements_are_private_and_schema_validated(self):
        value = {"schema_version": 1, "metrics": [{"name": "waits", "value": 10, "unit": "count", "samples": 1, "threshold": 10, "passed": True}], "framebuffer_sha256": "0" * 64}
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "measurements.json"
            self.assertTrue(gate.publish_measurements(path, value))
            self.assertEqual(stat.S_IMODE(path.stat().st_mode), 0o600)
            self.assertEqual(json.loads(path.read_text()), value)
            unsafe = dict(value, accessToken="SENSITIVE_FIXTURE")
            self.assertFalse(gate.publish_measurements(path, unsafe))

    def test_host_framebuffer_has_exact_size(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "host.rgb565"
            path.write_bytes(b"\0" * 2)
            with self.assertRaises(gate.common.GateFailure):
                gate.decode_host(path)

    def test_target_rejects_source_changed_after_provenance(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            for name in gate.GATE_INPUTS:
                path = root / name
                path.parent.mkdir(parents=True, exist_ok=True)
                path.write_text("original")
            runner = gate.Runner(root, False, str(uuid.uuid4()))
            runner.verified_resources["input_digests"] = {name: gate.common.sha256(root / name) for name in gate.GATE_INPUTS}
            (root / "gates/gate1b/ui/gate.slint").write_text("changed")
            with self.assertRaises(gate.common.GateInconclusive):
                runner.target(gate.TARGET)

    def test_verify_inputs_rejects_changed_host_tool_identity(self):
        with tempfile.TemporaryDirectory() as directory:
            runner = gate.Runner(Path(directory), False, str(uuid.uuid4()))
            runner.verified_resources.update(
                {
                    "input_digests": {},
                    "sdk_file_digests": {},
                    "sdk_host_tools": {"host-tool": "expected version"},
                }
            )
            completed = subprocess.CompletedProcess([], 0, stdout="changed version\n", stderr="")
            with (
                mock.patch.object(gate.common, "WEST_REVISIONS", {}),
                mock.patch.object(gate.subprocess, "run", return_value=completed),
                self.assertRaisesRegex(gate.common.GateInconclusive, "input_changed"),
            ):
                runner.verify_inputs()

    def test_concurrent_lock_reports_owner_without_changing_it(self):
        with tempfile.TemporaryDirectory() as directory:
            state = Path(directory) / ".deskkin"
            state.mkdir()
            owner = str(uuid.uuid4())
            first = gate.acquire_lock(state, owner)
            try:
                with self.assertRaisesRegex(gate.common.GateFailure, owner):
                    gate.acquire_lock(state, str(uuid.uuid4()))
                first.seek(0)
                self.assertEqual(json.load(first)["run_id"], owner)
            finally:
                first.close()

    def test_lock_rejects_symlink_leaf_without_changing_target(self):
        with tempfile.TemporaryDirectory() as directory:
            state = Path(directory) / ".deskkin"
            locks = state / "locks"
            locks.mkdir(parents=True)
            outside = Path(directory) / "outside"
            outside.write_text("unchanged")
            (locks / "1b.lock").symlink_to(outside)
            with self.assertRaises(gate.common.GateFailure):
                gate.acquire_lock(state, str(uuid.uuid4()))
            self.assertEqual(outside.read_text(), "unchanged")

    def test_failed_acknowledgement_keeps_authoritative_result(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            run_id = str(uuid.uuid4())
            result = root / ".deskkin/results/1b/default/result.json"
            gate.common.atomic_json(result, {"schema_version": 1, "gate": "1b", "mode": "default", "run_id": run_id, "result": "pass"})
            diagnostics = root / ".deskkin/diagnostics"
            diagnostics.mkdir(parents=True)
            outside = root / "outside"
            outside.mkdir()
            (diagnostics / run_id).symlink_to(outside, target_is_directory=True)
            self.assertFalse(gate.acknowledge_existing_result(root, result))
            self.assertTrue(result.exists())

    def test_result_ack_rejects_symlinked_result(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            outside = root / "outside.json"
            outside.write_text(json.dumps({"schema_version": 1, "gate": "1b", "mode": "default", "run_id": str(uuid.uuid4()), "result": "pass"}))
            result = root / ".deskkin/results/1b/default/result.json"
            result.parent.mkdir(parents=True)
            result.symlink_to(outside)
            self.assertFalse(gate.acknowledge_existing_result(root, result))

    def test_recording_off_result_needs_no_diagnostic_ack(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            result = root / ".deskkin/results/1b/default/result.json"
            gate.common.atomic_json(result, {"run_id": str(uuid.uuid4()), "result": "pass"})
            diagnostics = root / ".deskkin/diagnostics"
            diagnostics.mkdir(parents=True)
            marker = diagnostics / "unchanged"
            marker.write_text("unchanged")
            self.assertTrue(gate.acknowledge_existing_result(root, result, required=False))
            self.assertEqual(marker.read_text(), "unchanged")

    def test_boot_deadline_and_cancel_have_distinct_terminal_events(self):
        original_popen = subprocess.Popen

        def sleeper(*_args, **_kwargs):
            return original_popen(
                [sys.executable, "-c", "import time; time.sleep(30)"],
                stdout=subprocess.PIPE,
                stderr=subprocess.STDOUT,
                start_new_session=True,
                bufsize=0,
            )

        with tempfile.TemporaryDirectory() as directory, mock.patch.object(gate.subprocess, "Popen", side_effect=sleeper):
            timeout = gate.Runner(Path(directory), False, str(uuid.uuid4()), deadline_seconds=0)
            with self.assertRaises(gate.common.GateTimeout):
                timeout._boot(Path(directory))
            self.assertEqual((timeout.recorder.events[-1]["status"], timeout.recorder.events[-1]["error_type"]), ("timeout", "deadline_exceeded"))

            cancelled = gate.Runner(Path(directory), False, str(uuid.uuid4()))
            cancelled.cancelled = True
            with self.assertRaises(gate.common.GateCancelled):
                cancelled._boot(Path(directory))
            self.assertEqual((cancelled.recorder.events[-1]["status"], cancelled.recorder.events[-1]["error_type"]), ("cancel", "cancelled"))

    def test_selector_registration_failure_still_reaps_process(self):
        original_popen = subprocess.Popen
        spawned = []

        def sleeper(*_args, **_kwargs):
            process = original_popen([sys.executable, "-c", "import time; time.sleep(30)"], stdout=subprocess.PIPE, stderr=subprocess.STDOUT, start_new_session=True, bufsize=0)
            spawned.append(process)
            return process

        selector = mock.Mock()
        selector.register.side_effect = OSError("descriptor exhaustion")
        with tempfile.TemporaryDirectory() as directory, mock.patch.object(gate.subprocess, "Popen", side_effect=sleeper), mock.patch.object(gate.selectors, "DefaultSelector", return_value=selector):
            runner = gate.Runner(Path(directory), False, str(uuid.uuid4()))
            with self.assertRaises(OSError):
                runner._boot(Path(directory))
        self.assertIsNotNone(spawned[0].poll())
        selector.close.assert_called_once()


if __name__ == "__main__":
    unittest.main()
