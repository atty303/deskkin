import importlib.util
import contextlib
import io
import json
import os
import stat
import subprocess
import sys
import tempfile
import unittest
import uuid
from pathlib import Path
from unittest import mock

MODULE_PATH = Path(__file__).resolve().parents[1] / "scripts/gate_runner.py"
SPEC = importlib.util.spec_from_file_location("gate_runner", MODULE_PATH)
assert SPEC and SPEC.loader
gate_runner = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = gate_runner
SPEC.loader.exec_module(gate_runner)


class GateRunnerTests(unittest.TestCase):
    @staticmethod
    def deterministic_runner(root: Path, recording: bool):
        class DeterministicRunner(gate_runner.Runner):
            def prepare(self):
                return None

            def target(self, target):
                self.supported_targets.add(target)
                self.clean_rebuilds.add(target)
                self.deliberate_panics.add(target)

        runner = DeterministicRunner(root, recording, str(uuid.uuid4()))
        if recording:
            gate_runner.prepare_control_state(root)
            gate_runner.initialize_recording(root, runner.recorder)
        return runner

    def test_atomic_json_is_private_and_complete(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "result.json"
            gate_runner.atomic_json(path, {"result": "pass", "run_id": "example"})
            self.assertEqual(json.loads(path.read_text()), {"result": "pass", "run_id": "example"})
            self.assertEqual(stat.S_IMODE(path.stat().st_mode), 0o600)

    def test_error_classification_is_stable(self):
        self.assertEqual(gate_runner.classify_error("could not compile rustapp"), "rust_compile_failed")
        self.assertEqual(gate_runner.classify_error("Kconfig error"), "configure_failed")
        self.assertEqual(gate_runner.classify_error("unrelated"), "command_failed")

    def test_recording_is_visible_before_finalize(self):
        with tempfile.TemporaryDirectory() as directory:
            run = Path(directory) / "run"
            recorder = gate_runner.Recorder(run, "run-id")
            recorder.start({"run_id": "run-id", "gate": "1a"})
            recorder.event("prepare", "success", 1)
            records = [json.loads(line) for line in (run / "diagnostic.jsonl").read_text().splitlines()]
            self.assertEqual([record["type"] for record in records], ["resource", "operation"])
            self.assertEqual(stat.S_IMODE((run / "diagnostic.jsonl").stat().st_mode), 0o600)

    def test_control_state_fixes_every_owned_directory_mode(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            gate_runner.prepare_control_state(root)
            for relative in (".deskkin", ".deskkin/results", ".deskkin/results/1a", ".deskkin/results/1a/default", ".deskkin/locks"):
                self.assertEqual(stat.S_IMODE((root / relative).stat().st_mode), 0o700)
            self.assertFalse((root / ".deskkin/diagnostics").exists())

    def test_recording_failure_does_not_affect_control_state(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            gate_runner.prepare_control_state(root)
            diagnostics = root / ".deskkin/diagnostics"
            diagnostics.write_text("unavailable")
            recorder = gate_runner.Recorder(diagnostics / str(uuid.uuid4()), "run-id")
            gate_runner.initialize_recording(root, recorder)
            self.assertIsNone(recorder.directory)
            self.assertEqual(recorder.health, "dropped")

    def test_artifact_storage_failure_only_degrades_recording(self):
        with tempfile.TemporaryDirectory() as directory:
            run = Path(directory) / "run"
            recorder = gate_runner.Recorder(run, "run-id")
            recorder.start({"run_id": "run-id", "gate": "1a"})
            with mock.patch.object(gate_runner, "publish_artifact", side_effect=OSError("artifact storage unavailable")):
                recorder.finalize({"run_id": "run-id", "gate": "1a"}, [], "pass", "all_criteria_passed")
            self.assertEqual(recorder.health, "partial")
            self.assertIn('"result": "pass"', (run / "diagnostic.jsonl").read_text())

    def test_recording_off_does_not_touch_diagnostic_storage(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            gate_runner.prepare_control_state(root)
            diagnostics = root / ".deskkin/diagnostics"
            diagnostics.write_text("leave unchanged")
            recorder = gate_runner.Recorder(None, "run-id")
            gate_runner.initialize_recording(root, recorder)
            self.assertEqual(diagnostics.read_text(), "leave unchanged")

    def test_list_tolerates_a_truncated_final_record(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            run = root / ".deskkin/diagnostics" / str(uuid.uuid4())
            run.mkdir(parents=True)
            (run / "diagnostic.jsonl").write_text('{"type":"resource"}\n{"type":')
            output = io.StringIO()
            with contextlib.redirect_stdout(output):
                self.assertEqual(gate_runner.diagnostics_list(root), 0)
            self.assertIn(" partial ", output.getvalue())

    def test_frozen_runs_count_toward_store_run_limit(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            base = root / ".deskkin/diagnostics"
            for _ in range(gate_runner.STORE_LIMIT_RUNS):
                run = base / str(uuid.uuid4())
                run.mkdir(parents=True)
                (run / ".frozen").touch()
            self.assertFalse(gate_runner.recording_capacity(root))

    def test_cancelled_build_command_reaps_its_process_group(self):
        with tempfile.TemporaryDirectory() as directory:
            runner = gate_runner.Runner(Path(directory), False, str(uuid.uuid4()))
            runner.state.mkdir()
            runner.cancelled = True
            with self.assertRaises(gate_runner.GateCancelled):
                runner.command("configure", [sys.executable, "-c", "import time; time.sleep(10)"], "test")
            self.assertEqual(runner.cleanup_status, "success")

    def test_termination_reaps_descendant_that_ignores_term(self):
        with tempfile.TemporaryDirectory() as directory:
            runner = gate_runner.Runner(Path(directory), False, str(uuid.uuid4()))
            script = (
                "import subprocess,sys,time; "
                "child=subprocess.Popen([sys.executable,'-c','import signal,time; signal.signal(signal.SIGTERM, signal.SIG_IGN); time.sleep(30)']); "
                "print(child.pid, flush=True); time.sleep(30)"
            )
            process = subprocess.Popen(
                [sys.executable, "-c", script],
                stdout=subprocess.PIPE,
                start_new_session=True,
            )
            assert process.stdout is not None
            child_pid = int(process.stdout.readline())
            self.assertTrue(runner._terminate_group(process))
            with self.assertRaises(ProcessLookupError):
                os.kill(child_pid, 0)
            process.stdout.close()

    def test_result_helpers_report_io_failure(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "result.json"
            path.mkdir()
            self.assertFalse(gate_runner.clear_result(path))
            self.assertFalse(
                gate_runner.publish_result(
                    path,
                    {"result": "pass"},
                    writer=lambda _path, _value: (_ for _ in ()).throw(OSError("unavailable")),
                )
            )

    def test_recording_on_and_off_preserve_semantic_result(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            off_exit, off_result = self.deterministic_runner(root, False).run()
            on_exit, on_result = self.deterministic_runner(root, True).run()
            for result in (off_result, on_result):
                for key in ("run_id", "started_at", "ended_at"):
                    result.pop(key)
            self.assertEqual(on_exit, off_exit)
            self.assertEqual(on_result, off_result)

    def test_timeout_panic_and_cancel_have_distinct_meaning(self):
        cases = (
            (gate_runner.GateTimeout("deadline_exceeded"), 124, "deadline_exceeded"),
            (gate_runner.GateFailure("deliberate_panic"), 1, "deliberate_panic"),
            (gate_runner.GateCancelled("cancelled"), 130, "cancelled"),
        )
        with tempfile.TemporaryDirectory() as directory:
            for error, expected_exit, expected_reason in cases:
                runner = self.deterministic_runner(Path(directory), False)
                runner.prepare = lambda current=error: (_ for _ in ()).throw(current)
                exit_code, result = runner.run()
                self.assertEqual(exit_code, expected_exit)
                self.assertEqual(result["reason_code"], expected_reason)

    def test_sensitive_serial_fixture_fails_closed(self):
        with tempfile.TemporaryDirectory() as directory:
            run = Path(directory) / "run"
            recorder = gate_runner.Recorder(run, "run-id")
            recorder.start({"run_id": "run-id", "gate": "1a"})
            recorder.add_serial("qemu_cortex_m3", "normal", "DESKKIN_GATE_EVENT schema=1 token=SENSITIVE_FIXTURE")
            recorder.finalize({"run_id": "run-id", "gate": "1a"}, [], "pass", "all_criteria_passed")
            self.assertFalse((run / "serial.jsonl").exists())
            self.assertNotIn("SENSITIVE_FIXTURE", (run / "diagnostic.jsonl").read_text())
            self.assertIn('"reason": "privacy_filter_failed"', (run / "diagnostic.jsonl").read_text())

    def test_sensitive_resource_and_link_fixtures_fail_closed(self):
        with tempfile.TemporaryDirectory() as directory:
            dropped = gate_runner.Recorder(Path(directory) / "dropped", "run-id")
            dropped.start({"run_id": "run-id", "gate": "1a", "api_token": "SENSITIVE_FIXTURE"})
            self.assertIsNone(dropped.directory)
            self.assertEqual(dropped.health_reason, "privacy_filter_failed")

            run = Path(directory) / "partial"
            recorder = gate_runner.Recorder(run, "run-id")
            recorder.start({"run_id": "run-id", "gate": "1a"})
            recorder.finalize(
                {"run_id": "run-id", "gate": "1a"},
                [{"target": "qemu_cortex_m3", "mode": "normal", "sha256": "0" * 64, "bytes": 1, "credential": "SENSITIVE_FIXTURE"}],
                "pass",
                "all_criteria_passed",
            )
            self.assertFalse((run / "link-summary.json").exists())
            self.assertIn('"reason": "privacy_filter_failed"', (run / "diagnostic.jsonl").read_text())

    def test_privacy_filter_rejects_paths_identity_and_camel_case_credentials(self):
        fixtures = (
            {"tool_identities": {"rust": "rustc from /home/example/toolchain"}},
            {"tool_identities": {"rust": r"rustc from C:\\Users\\example\\toolchain"}},
            {"tool_identities": {"builder": "example-user"}},
            {"accessToken": "opaque-value"},
            {"tool_identities": {"rust": f"built by {gate_runner.getpass.getuser()}"}},
        )
        for fixture in fixtures:
            self.assertFalse(gate_runner.privacy_safe(fixture), fixture)
        self.assertTrue(gate_runner.privacy_safe({"tool_identities": {"rust": "rustc 1.95.0 (https://github.com/rust-lang/rust)"}}))

    def test_truncated_artifact_is_omitted(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "build-summary.json"

            def truncated_writer(destination, _value):
                destination.write_text('{"schema_version":1,"operations":')

            self.assertFalse(
                gate_runner.publish_artifact(
                    path,
                    {"schema_version": 1, "operations": []},
                    "build",
                    writer=truncated_writer,
                )
            )
            self.assertFalse(path.exists())

    def test_runner_does_not_open_remote_connections(self):
        with tempfile.TemporaryDirectory() as directory:
            runner = self.deterministic_runner(Path(directory), False)
            with mock.patch("socket.socket.connect", side_effect=AssertionError("remote connection attempted")):
                exit_code, _result = runner.run()
            self.assertEqual(exit_code, 0)

    def test_lock_contention_preserves_owner_state(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            gate_runner.prepare_control_state(root)
            result = root / ".deskkin/results/1a/default/result.json"
            diagnostic = root / ".deskkin/owner-diagnostic"
            result.write_text("owner-result")
            diagnostic.write_text("owner-diagnostic")
            owner = gate_runner.acquire_lock(root / ".deskkin", "owner-run")
            try:
                with self.assertRaises(gate_runner.GateFailure):
                    gate_runner.acquire_lock(root / ".deskkin", "contender-run")
                self.assertEqual(result.read_text(), "owner-result")
                self.assertEqual(diagnostic.read_text(), "owner-diagnostic")
            finally:
                owner.close()

    def test_partial_progress_is_reported_in_criteria(self):
        with tempfile.TemporaryDirectory() as directory:
            runner = self.deterministic_runner(Path(directory), False)
            runner.target = lambda target: (
                runner.supported_targets.add(target),
                runner.clean_rebuilds.add(target),
                (_ for _ in ()).throw(gate_runner.GateFailure("second_target_failed")) if target == gate_runner.TARGETS[1] else None,
            )
            exit_code, result = runner.run()
            self.assertEqual(exit_code, 1)
            criteria = {item["name"]: item["value"] for item in result["criteria"]}
            self.assertEqual(criteria, {"supported_targets": 2, "clean_rebuilds": 2, "deliberate_panics": 0})

    def test_delete_rejects_symlinked_run_and_root(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            base = root / ".deskkin/diagnostics"
            first = base / str(uuid.uuid4())
            second = base / str(uuid.uuid4())
            second.mkdir(parents=True)
            first.symlink_to(second, target_is_directory=True)
            with contextlib.redirect_stderr(io.StringIO()):
                self.assertEqual(gate_runner.diagnostics_delete(root, first.name), 2)
            self.assertTrue(second.is_dir())

        with tempfile.TemporaryDirectory() as directory, tempfile.TemporaryDirectory() as outside:
            root = Path(directory)
            external = Path(outside)
            run = external / str(uuid.uuid4())
            run.mkdir()
            (root / ".deskkin").mkdir()
            (root / ".deskkin/diagnostics").symlink_to(external, target_is_directory=True)
            with contextlib.redirect_stderr(io.StringIO()):
                self.assertEqual(gate_runner.diagnostics_delete(root, run.name), 2)
            self.assertTrue(run.is_dir())

    def test_setup_probe_failure_is_inconclusive(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            runner = gate_runner.Runner(root, False, str(uuid.uuid4()))
            runner._environment = lambda: (_ for _ in ()).throw(OSError("missing"))
            exit_code, result = runner.run()
            self.assertEqual(exit_code, 2)
            self.assertEqual(result["result"], "inconclusive")
            self.assertEqual(result["reason_code"], "setup_probe_failed")

    def test_delete_rejects_path_traversal(self):
        with tempfile.TemporaryDirectory() as directory:
            with contextlib.redirect_stderr(io.StringIO()):
                self.assertEqual(gate_runner.diagnostics_delete(Path(directory), "../outside"), 2)


if __name__ == "__main__":
    unittest.main()
