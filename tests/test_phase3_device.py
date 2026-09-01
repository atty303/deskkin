import importlib.util
import io
import json
import os
import stat
import subprocess
import tempfile
import unittest
from unittest import mock
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
SPEC = importlib.util.spec_from_file_location("phase3_device", ROOT / "scripts/phase3_device.py")
device = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(device)


class Phase3DeviceTests(unittest.TestCase):
    def profile(self):
        return {"schema_version": 1, "ssid": "fixture", "password": "fake-password", "host_ipv4": "192.168.10.2"}

    def pet_benchmark_status(self):
        status = bytearray(80)
        status[0] = 1
        status[26] = 2
        for start, length, value in (
            (30, 4, 60_010),
            (34, 4, 1_200),
            (38, 4, 1_200),
            (42, 4, 12_000_000),
            (46, 4, 24_000_000),
            (50, 4, 20_000),
            (54, 4, 30_000),
            (58, 4, 1_140),
            (68, 4, 288_000),
            (72, 4, 184_320_000),
            (76, 4, 1_200),
        ):
            status[start : start + length] = value.to_bytes(length, "big")
        return bytes(status)

    def test_build_verifies_the_same_state_directory_it_uses(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            with mock.patch.dict(os.environ, {"DESKKIN_STATE_DIR": "/tmp/unverified-state"}), mock.patch.object(
                device.subprocess, "run"
            ) as run:
                device.build(root)

        self.assertEqual(run.call_count, 3)
        expected_state = str(root / ".deskkin")
        for call in run.call_args_list:
            self.assertEqual(call.kwargs["env"]["DESKKIN_STATE_DIR"], expected_state)

    def test_amp_build_is_one_pristine_sysbuild(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            state = root / ".deskkin"
            with mock.patch.object(device, "device_environment", return_value=(state, {})), mock.patch.object(
                device.subprocess, "run"
            ) as run:
                device.build_amp(root)

        self.assertEqual(run.call_count, 2)
        build_command = run.call_args_list[1].args[0]
        self.assertIn("--sysbuild", build_command)
        self.assertEqual(build_command.count("--pristine"), 1)
        self.assertIn(str(root / "apps/core-s3-amp"), build_command)

    def test_amp_flash_uses_sysbuild_flash_order(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            state = root / ".deskkin"
            build = state / "phase3-device/amp-build"
            build.mkdir(parents=True)
            (build / "domains.yaml").write_text("default: core-s3-amp\n", encoding="utf-8")
            with mock.patch.object(device, "discover_device", return_value=Path("/dev/fake")), mock.patch.object(
                device, "device_environment", return_value=(state, {})
            ), mock.patch.object(device.subprocess, "run") as run:
                device.flash_amp(root, "/dev/fake")

        run.assert_called_once()
        command = run.call_args.args[0]
        self.assertNotIn("--domain", command)
        self.assertIn(str(build), command)

    def test_amp_pipeline_benchmark_observes_progress_and_bounded_timings(self):
        clock = mock.Mock()
        clock.now = 0.0
        clock.monotonic.side_effect = lambda: clock.now
        clock.sleep.side_effect = lambda seconds: setattr(clock, "now", clock.now + seconds)

        generation = 10

        def status(*_args, **_kwargs):
            nonlocal generation
            generation += 3
            response = bytearray(80)
            response[0] = 1
            response[27] = 1
            response[28:32] = generation.to_bytes(4, "big")
            response[32:40] = int(clock.now * 1000).to_bytes(8, "big")
            response[40:44] = (generation * 2).to_bytes(4, "big")
            response[44:48] = (12_000).to_bytes(4, "big")
            response[48:52] = (42_000).to_bytes(4, "big")
            response[52] = 4
            response[56] = 1
            response[57:61] = (14_000).to_bytes(4, "big")
            response[61:65] = (44_000).to_bytes(4, "big")
            response[74:78] = (3_000).to_bytes(4, "big")
            return bytes(response)

        with mock.patch.object(device, "time", clock), mock.patch.object(
            device, "run_control", side_effect=status
        ) as run_control:
            summary = device.amp_render_pipeline_benchmark("/dev/fake")

        self.assertEqual(summary["value"], "observed")
        self.assertGreater(summary["completed_frames"], 0)
        self.assertGreaterEqual(summary["measured_fps_milli"], 20_000)
        self.assertLess(summary["observation_duration_ms"], summary["duration_ms"])
        self.assertGreaterEqual(summary["measurement_coverage_milli"], 800)
        self.assertLessEqual(summary["last_observation_age_ms"], 1_000)
        self.assertEqual(summary["last_availability"], 1)
        self.assertEqual(summary["render_max_us"], 14_000)
        self.assertEqual(summary["transfer_max_us"], 44_000)
        self.assertEqual(summary["copy_last_us"], 3_000)
        self.assertEqual(summary["wire_last_us"], 39_000)
        self.assertTrue(run_control.call_args_list[0].kwargs["recover_status_transport"])
        self.assertFalse(run_control.call_args_list[1].kwargs["recover_status_transport"])

    def test_amp_pipeline_status_rejects_allocation_failure(self):
        status = bytearray(80)
        status[0] = 1
        status[27] = 1
        status[28:32] = (3).to_bytes(4, "big")
        status[40:44] = (2).to_bytes(4, "big")
        status[54] = 1
        status[56] = 1
        decoded = device.decode_amp_pipeline_status(bytes(status))

        self.assertEqual(decoded["allocation_failures"], 1)
        self.assertEqual(decoded["completed_frames"], 2)

    def test_amp_pipeline_measurement_reports_less_than_twenty_fps(self):
        clock = mock.Mock()
        clock.now = 0.0
        clock.monotonic.side_effect = lambda: clock.now
        clock.sleep.side_effect = lambda seconds: setattr(clock, "now", clock.now + seconds)
        generation = 0

        def status(*_args, **_kwargs):
            nonlocal generation
            generation += 1
            response = bytearray(80)
            response[0] = 1
            response[27] = 1
            response[28:32] = generation.to_bytes(4, "big")
            response[32:40] = int(clock.now * 1000).to_bytes(8, "big")
            response[40:44] = generation.to_bytes(4, "big")
            response[44:48] = (10_000).to_bytes(4, "big")
            response[48:52] = (40_000).to_bytes(4, "big")
            response[52] = 4
            response[56] = 1
            response[57:61] = (10_000).to_bytes(4, "big")
            response[61:65] = (40_000).to_bytes(4, "big")
            return bytes(response)

        with mock.patch.object(device, "time", clock), mock.patch.object(
            device, "run_control", side_effect=status
        ):
            summary = device.amp_render_pipeline_benchmark("/dev/fake")

        self.assertEqual(summary["value"], "observed")
        self.assertLess(summary["measured_fps_milli"], 20_000)

    def test_amp_pipeline_benchmark_rejects_renderer_that_stops_before_deadline(self):
        clock = mock.Mock()
        clock.now = 0.0
        clock.monotonic.side_effect = lambda: clock.now
        clock.sleep.side_effect = lambda seconds: setattr(clock, "now", clock.now + seconds)
        generation = 0

        def status(*_args, **_kwargs):
            nonlocal generation
            response = bytearray(80)
            response[0] = 1
            if clock.now < device.AMP_BENCHMARK_DURATION_SECONDS / 2:
                generation += 5
                response[27] = 1
                response[28:32] = generation.to_bytes(4, "big")
                response[32:40] = int(clock.now * 1000).to_bytes(8, "big")
                response[40:44] = generation.to_bytes(4, "big")
                response[44:48] = (10_000).to_bytes(4, "big")
                response[48:52] = (40_000).to_bytes(4, "big")
                response[52] = 4
                response[56] = 1
                response[57:61] = (10_000).to_bytes(4, "big")
                response[61:65] = (40_000).to_bytes(4, "big")
            else:
                response[27] = 2
            return bytes(response)

        with mock.patch.object(device, "time", clock), mock.patch.object(
            device, "run_control", side_effect=status
        ), self.assertRaises(device.DeviceError):
            device.amp_render_pipeline_benchmark("/dev/fake")

    def test_amp_pipeline_benchmark_rejects_fresh_renderer_failure(self):
        clock = mock.Mock()
        clock.now = 0.0
        clock.monotonic.side_effect = lambda: clock.now
        clock.sleep.side_effect = lambda seconds: setattr(clock, "now", clock.now + seconds)
        generation = 0

        def status(*_args, **_kwargs):
            nonlocal generation
            generation += 1
            response = bytearray(80)
            response[0] = 1
            response[27] = 1
            response[28:32] = generation.to_bytes(4, "big")
            response[32:40] = int(clock.now * 1000).to_bytes(8, "big")
            response[40:44] = generation.to_bytes(4, "big")
            response[44:48] = (10_000).to_bytes(4, "big")
            response[48:52] = (40_000).to_bytes(4, "big")
            response[52] = 5 if clock.now >= device.AMP_BENCHMARK_DURATION_SECONDS - 0.25 else 4
            response[56] = 1
            return bytes(response)

        with mock.patch.object(device, "time", clock), mock.patch.object(
            device, "run_control", side_effect=status
        ), self.assertRaises(device.AmpBenchmarkError) as raised:
            device.amp_render_pipeline_benchmark("/dev/fake")

        self.assertEqual(raised.exception.error_type, "amp_pipeline_benchmark_failed")
        self.assertEqual(raised.exception.summary["renderer_stage"], 5)
        self.assertEqual(raised.exception.summary["status"], "error")

    def test_amp_benchmark_cli_records_measurement_failure_summary(self):
        summary = {
            "operation": "amp_render_pipeline",
            "status": "error",
            "error_type": "amp_pipeline_benchmark_failed",
            "duration_ms": 10_000,
            "renderer_stage": 5,
            "allocation_failures": 0,
            "transfer_failures": 0,
        }
        failure = device.AmpBenchmarkError("amp_pipeline_benchmark_failed", summary)
        with mock.patch.object(device.sys, "argv", ["phase3_device.py", "amp-benchmark"]), mock.patch.object(
            device, "amp_render_pipeline_benchmark", side_effect=failure
        ), mock.patch.object(device, "publish_diagnostic") as publish_diagnostic, mock.patch.object(
            device, "publish_result", return_value=Path("/tmp/result.json")
        ):
            exit_code = device.main()

        self.assertEqual(exit_code, 2)
        self.assertEqual(publish_diagnostic.call_args.args[2], "error")
        self.assertEqual(publish_diagnostic.call_args.args[3], [summary])

    def test_amp_supervisor_dram_ends_at_renderer_origin(self):
        linker = (ROOT / "apps/core-s3-amp/amp-dram-boundary.ld").read_text(encoding="utf-8")
        self.assertIn("ASSERT(_end <= 0x3fcc5000", linker)

    def test_amp_supervisor_uses_one_internal_framebuffer_without_psram(self):
        config = (ROOT / "apps/core-s3-amp/prj.conf").read_text(encoding="utf-8")
        source = (ROOT / "apps/core-s3-amp/src/main.c").read_text(encoding="utf-8")
        self.assertNotIn("CONFIG_ESP_SPIRAM", config)
        self.assertIn("internal_framebuffer[320U * 240U]", source)
        self.assertNotIn("external_framebuffer", source)
        self.assertNotIn("psram_ready", source)

    def test_amp_renderer_overlaps_next_render_with_single_buffer_transfer(self):
        renderer = (ROOT / "apps/core-s3-amp/renderer/src/lib.rs").read_text(encoding="utf-8")
        adapter = (ROOT / "apps/core-s3-amp/renderer/src/adapter.c").read_text(encoding="utf-8")
        spi_patch = (ROOT / "patches/zephyr-core-s3/0003-yield-while-polling-esp32-spi.patch").read_text(
            encoding="utf-8"
        )
        bootstrap = (ROOT / "scripts/bootstrap_core_s3.sh").read_text(encoding="utf-8")
        submit = renderer.index("deskkin_display_submit(buffer)")
        next_render = renderer.index("let Ok(next_render_us)")
        completion = renderer.index("let Ok(transfer_us)")
        self.assertLess(submit, next_render)
        self.assertLess(next_render, completion)
        self.assertIn("k_yield();", adapter)
        self.assertIn("display_entry, NULL, NULL, NULL, 0, 0, K_NO_WAIT", adapter)
        self.assertIn("while (!spi_hal_usr_is_done(hal))", spi_patch)
        self.assertIn("+\t\t\tk_yield();", spi_patch)
        self.assertIn("543fd300e1237cb09a41e4e7f443f9392370dc470e9eb89de7e8706a2bbe8abb", bootstrap)

    def test_profile_schema_is_exact_and_rfc1918(self):
        self.assertEqual(device.validate_profile(self.profile()), self.profile())
        for change in ({"extra": 1}, {"host_ipv4": "8.8.8.8"}, {"password": "short"}, {"schema_version": 2}):
            value = self.profile() | change
            with self.assertRaises(device.DeviceError):
                device.validate_profile(value)

    def test_dhcp_wait_decision_covers_ready_timeout_and_cancel(self):
        source = r'''
#include <assert.h>
#include "dhcp_wait.h"

int main(void) {
    assert(deskkin_dhcp_wait_decide(false, true, false) == DESKKIN_DHCP_WAIT_READY);
    assert(deskkin_dhcp_wait_decide(false, true, true) == DESKKIN_DHCP_WAIT_READY);
    assert(deskkin_dhcp_wait_decide(false, false, false) == DESKKIN_DHCP_WAIT_CONTINUE);
    assert(deskkin_dhcp_wait_decide(false, false, true) == DESKKIN_DHCP_WAIT_TIMED_OUT);
    assert(deskkin_dhcp_wait_decide(true, false, false) == DESKKIN_DHCP_WAIT_CANCELLED);
    assert(deskkin_dhcp_wait_decide(true, true, true) == DESKKIN_DHCP_WAIT_CANCELLED);
    return 0;
}
'''
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            harness = root / "dhcp_wait_test.c"
            executable = root / "dhcp_wait_test"
            harness.write_text(source, encoding="utf-8")
            subprocess.run(
                [
                    "clang",
                    "-std=c11",
                    "-Wall",
                    "-Wextra",
                    "-Werror",
                    "-I",
                    str(ROOT / "apps/core-s3-device/src"),
                    str(harness),
                    "-o",
                    str(executable),
                ],
                check=True,
            )
            subprocess.run([str(executable)], check=True)

    def test_control_frame_is_bounded_and_canonical(self):
        payload = device.wifi_payload(self.profile())
        frame = device.control_frame("wifi-provision", 7, payload)
        self.assertEqual(int.from_bytes(frame[:2], "big"), len(frame) - 2)
        self.assertEqual(frame[2:4], bytes([1, 4]))
        self.assertEqual(int.from_bytes(frame[20:28], "big"), 7)
        self.assertEqual(int.from_bytes(frame[28:30], "big"), len(payload))
        device.zeroize(payload)
        device.zeroize(frame)
        self.assertEqual(set(payload), {0})
        self.assertEqual(set(frame), {0})

    def test_control_io_handles_partial_reads_and_writes(self):
        writes = []

        def partial_write(_descriptor, value):
            length = min(2, len(value))
            writes.append(bytes(value[:length]))
            return length

        chunks = [b"a", b"bc", b"d"]
        with mock.patch.object(device.select, "select", side_effect=lambda reads, writes, errors, timeout: (reads, writes, errors)), mock.patch.object(device.os, "write", side_effect=partial_write), mock.patch.object(device.os, "read", side_effect=lambda _descriptor, _length: chunks.pop(0)):
            device.write_all(7, b"abcdef", 1)
            self.assertEqual(b"".join(writes), b"abcdef")
            self.assertEqual(device.read_exact(7, 4, 1), b"abcd")

    def test_control_response_framing_failures_are_distinct(self):
        frame = device.control_frame("status", 0)
        response = bytearray(18)
        response[0] = 1
        response[2:18] = frame[4:20]
        stream = bytearray(b"deskkin.boot stage=3 error=0\n")
        stream.extend(len(response).to_bytes(2, "big"))
        stream.extend(response)
        def consume(_descriptor, length):
            value = bytes(stream[:length])
            del stream[:length]
            return value

        with mock.patch.object(
            device.select,
            "select",
            side_effect=lambda reads, writes, errors, timeout: (reads, writes, errors),
        ), mock.patch.object(device.os, "read", side_effect=consume):
            self.assertEqual(device.read_control_response(7, frame, 1), bytes(response))

    def test_status_retries_but_mutation_does_not(self):
        attributes = [0, 0, 0, 0, 0, 0, [0] * 32]
        response = bytes([1, 0]) + bytes(16)
        common = mock.patch.multiple(
            device.os,
            open=mock.DEFAULT,
            close=mock.DEFAULT,
        )
        with common as patched, mock.patch.object(device.time, "sleep"), mock.patch.object(
            device.termios, "tcgetattr", return_value=attributes
        ), mock.patch.object(device.termios, "tcsetattr"), mock.patch.object(
            device.termios, "tcflush"
        ), mock.patch.object(device, "write_all") as write, mock.patch.object(
            device,
            "read_control_response",
            side_effect=[device.DeviceError("control_timeout"), response],
        ):
            patched["open"].return_value = 7
            self.assertEqual(device.exchange(Path("/dev/fake"), device.control_frame("status", 0)), response)
            self.assertEqual(write.call_count, 2)
            self.assertEqual(patched["open"].call_count, 2)
            self.assertEqual(patched["close"].call_count, 2)

        with common as patched, mock.patch.object(device.time, "sleep"), mock.patch.object(
            device.termios, "tcgetattr", return_value=attributes
        ), mock.patch.object(device.termios, "tcsetattr"), mock.patch.object(
            device.termios, "tcflush"
        ), mock.patch.object(device, "write_all") as write, mock.patch.object(
            device,
            "read_control_response",
            side_effect=device.DeviceError("control_timeout"),
        ):
            patched["open"].return_value = 7
            with self.assertRaisesRegex(device.DeviceError, "control_timeout"):
                device.exchange(Path("/dev/fake"), device.control_frame("run", 0))
            self.assertEqual(write.call_count, 1)

    def test_monitor_status_does_not_wait_for_transport_recovery(self):
        status = bytearray(80)
        attributes = [0, 0, 0, 0, 0, 0, [0] * 32]
        with mock.patch.object(device, "discover_device", return_value=Path("/dev/fake")), mock.patch.object(
            device.os, "open", return_value=7
        ), mock.patch.object(device.os, "close"), mock.patch.object(
            device.termios, "tcgetattr", return_value=attributes
        ), mock.patch.object(device.termios, "tcsetattr"), mock.patch.object(
            device.termios, "tcflush"
        ), mock.patch.object(device.time, "sleep") as sleep, mock.patch.object(
            device, "write_all"
        ), mock.patch.object(device, "read_control_response", return_value=bytes(status)):
            device.run_control("status", "/dev/fake", recover_status_transport=False)
        sleep.assert_called_once_with(0.0)

    def test_mutation_stops_at_closed_boot_failure(self):
        status = bytearray(80)
        status[0] = 1
        status[79] = 2
        with mock.patch.object(device, "discover_device", return_value=Path("/dev/fake")), mock.patch.object(
            device, "exchange", return_value=bytes(status)
        ) as exchange:
            with self.assertRaisesRegex(device.DeviceError, "boot_noise_resolver"):
                device.run_control("wifi-provision", "/dev/fake", b"payload")
        self.assertEqual(exchange.call_count, 1)

    def test_mutation_rejects_short_status_preflight(self):
        status = bytes([1, 0]) + bytes(16)
        with mock.patch.object(
            device, "discover_device", return_value=Path("/dev/fake")
        ), mock.patch.object(device, "exchange", return_value=status) as exchange:
            with self.assertRaisesRegex(device.DeviceError, "control_invalid"):
                device.run_control("wifi-provision", "/dev/fake", b"payload")
        self.assertEqual(exchange.call_count, 1)

    def test_unknown_boot_failure_remains_closed(self):
        status = bytearray(80)
        status[79] = 255
        self.assertEqual(device.status_boot_error(bytes(status)), "boot_unknown")

    def test_status_waits_until_boot_is_complete(self):
        starting = bytearray(80)
        starting[78] = 8
        complete = bytearray(starting)
        complete[78] = device.BOOT_COMPLETE_STAGE
        with mock.patch.object(device, "run_control", return_value=bytes(complete)) as run_control, mock.patch.object(
            device.time, "sleep"
        ) as sleep:
            result = device.await_boot_complete(bytes(starting), "/dev/fake")
        self.assertEqual(result, bytes(complete))
        sleep.assert_called_once_with(0.25)
        run_control.assert_called_once_with("status", "/dev/fake", recover_status_transport=False)

    def test_status_boot_wait_is_bounded(self):
        starting = bytearray(80)
        starting[78] = 8
        with mock.patch.object(device.time, "monotonic", side_effect=[0.0, 15.0]):
            with self.assertRaisesRegex(device.DeviceError, "boot_not_ready"):
                device.await_boot_complete(bytes(starting), "/dev/fake")

    def test_status_rejects_unknown_completed_boot_stage(self):
        status = bytearray(80)
        status[78] = 255
        with self.assertRaisesRegex(device.DeviceError, "boot_unknown"):
            device.await_boot_complete(bytes(status), "/dev/fake")

    def test_boot_failure_is_not_persisted_as_a_complete_diagnostic(self):
        status = bytearray(80)
        status[79] = 2
        with mock.patch.object(device.sys, "argv", ["phase3_device.py", "status"]), mock.patch.object(
            device, "run_control", return_value=bytes(status)
        ), mock.patch.object(device, "publish_diagnostic") as publish_diagnostic, mock.patch.object(
            device, "publish_result", return_value=Path("/tmp/status.json")
        ), mock.patch.object(device.sys, "stdout", io.StringIO()), mock.patch.object(
            device.sys, "stderr", io.StringIO()
        ):
            self.assertEqual(device.main(), 2)
        publish_diagnostic.assert_not_called()

    def test_control_failure_is_not_persisted_outside_closed_error_set(self):
        with mock.patch.object(device.sys, "argv", ["phase3_device.py", "status"]), mock.patch.object(
            device, "run_control", side_effect=device.DeviceError("control_timeout")
        ), mock.patch.object(device, "publish_diagnostic") as publish_diagnostic, mock.patch.object(
            device, "publish_result", return_value=Path("/tmp/status.json")
        ), mock.patch.object(device.sys, "stdout", io.StringIO()), mock.patch.object(
            device.sys, "stderr", io.StringIO()
        ):
            self.assertEqual(device.main(), 2)
        publish_diagnostic.assert_not_called()

    def test_successful_status_publishes_diagnostic(self):
        status = bytearray(80)
        status[78] = 9
        with mock.patch.object(device.sys, "argv", ["phase3_device.py", "status"]), mock.patch.object(
            device, "run_control", return_value=bytes(status)
        ), mock.patch.object(device, "publish_diagnostic") as publish_diagnostic, mock.patch.object(
            device, "publish_result", return_value=Path("/tmp/status.json")
        ), mock.patch.object(device.sys, "stdout", io.StringIO()), mock.patch.object(
            device.sys, "stderr", io.StringIO()
        ):
            self.assertEqual(device.main(), 0)
        publish_diagnostic.assert_called_once()

    def test_status_report_is_bounded_and_contains_no_payload(self):
        status = bytearray(80)
        status[26] = 4
        status[27] = 1
        status[78] = 7
        with mock.patch.object(device.sys, "stderr") as stderr:
            device.report_status(bytes(status))
        reported = json.loads("".join(call.args[0] for call in stderr.write.call_args_list))
        self.assertEqual(
            reported,
            {
                "shell_state": 4,
                "availability": 1,
                "last_stage": "idle",
                "last_error": None,
                "boot_stage": 7,
                "boot_error": None,
            },
        )

    def test_device_state_directory_is_hardened_without_following_symlinks(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            state = root / "phase3-device"
            state.mkdir(mode=0o755)
            device.ensure_private_directory(state)
            self.assertEqual(stat.S_IMODE(state.stat().st_mode), 0o700)
            target = root / "target"
            target.mkdir()
            linked = root / "linked"
            linked.symlink_to(target, target_is_directory=True)
            with self.assertRaises(device.DeviceError):
                device.ensure_private_directory(linked)

    def test_run_success_requires_valid_result_and_rendered_correlations(self):
        record = {
            "shell_state": 4,
            "valid_availability_result": True,
            "session_context_id": "11" * 16,
            "operation_context_id": "22" * 16,
            "rgb565_digest": "12345678",
            "run_attempt": 7,
            "result_attempt": 7,
            "frame_attempt": 7,
        }
        self.assertTrue(device.run_succeeded([record], 7))
        for key, invalid in (
            ("valid_availability_result", False),
            ("session_context_id", "00" * 16),
            ("operation_context_id", "00" * 16),
            ("rgb565_digest", "00000000"),
        ):
            self.assertFalse(device.run_succeeded([record | {key: invalid}], 7))
        for key in ("run_attempt", "result_attempt", "frame_attempt"):
            self.assertFalse(device.run_succeeded([record | {key: 6}], 7))

    def test_pet_benchmark_gate_decodes_only_bounded_timing_and_counters(self):
        summary = device.decode_pet_benchmark(self.pet_benchmark_status())
        self.assertTrue(device.pet_benchmark_passed(summary))
        self.assertEqual(summary["animation_update_requests"], 1_200)
        self.assertEqual(summary["completed_frames"], 1_200)
        self.assertEqual(summary["frames_within_50ms"], 1_140)
        self.assertEqual(summary["frame_digest_updates"], 1_200)
        self.assertNotIn("rgb565_digest", summary)
        self.assertNotIn("asset_path", summary)

        for key, value in (
            ("state", 3),
            ("duration_ms", 60_501),
            ("animation_update_requests", 1_199),
            ("completed_frames", 1_199),
            ("frames_within_50ms", 1_139),
            ("stalls_over_250ms", 1),
            ("allocation_failures", 1),
            ("display_transfer_failures", 1),
            ("frame_digest_updates", 0),
        ):
            self.assertFalse(device.pet_benchmark_passed(summary | {key: value}), key)

    def test_pet_benchmark_wait_avoids_usb_polling_during_measurement(self):
        with mock.patch.object(device.time, "sleep") as sleep, mock.patch.object(
            device, "run_control", return_value=self.pet_benchmark_status()
        ) as run_control:
            summary = device.await_pet_benchmark("/dev/fake")
        self.assertTrue(device.pet_benchmark_passed(summary))
        sleep.assert_called_once_with(60.5)
        run_control.assert_called_once_with(
            "pet-benchmark-status", "/dev/fake", recover_status_transport=False
        )

    def test_pet_benchmark_action_stops_application_before_start(self):
        calls = []

        def control(command, *args, **kwargs):
            calls.append(command)
            return bytes(80)

        summary = device.decode_pet_benchmark(self.pet_benchmark_status())
        with mock.patch.object(device.sys, "argv", ["phase3_device.py", "benchmark", "--device", "/dev/fake"]), mock.patch.object(
            device, "run_control", side_effect=control
        ), mock.patch.object(device, "await_pet_benchmark", return_value=summary), mock.patch.object(
            device, "publish_diagnostic"
        ) as publish_diagnostic, mock.patch.object(
            device, "publish_result", return_value=Path("/tmp/benchmark.json")
        ), mock.patch.object(device.sys, "stdout", io.StringIO()), mock.patch.object(
            device.sys, "stderr", io.StringIO()
        ):
            self.assertEqual(device.main(), 0)
        self.assertEqual(calls, ["shutdown", "pet-benchmark-start"])
        record = publish_diagnostic.call_args.args[3][0]
        self.assertEqual(record["operation"], "pet_render_benchmark")
        self.assertEqual(record["status"], "success")
        for forbidden in ("rgb565_digest", "asset_path", "pixel", "raw_packet"):
            self.assertNotIn(forbidden, record)

    def test_hosted_profile_parser_is_exact_and_bounded(self):
        plaintext = bytearray(json.dumps(self.profile(), separators=(",", ":")).encode())
        try:
            self.assertEqual(device.profile_payload_from_json(ROOT, plaintext), device.wifi_payload(self.profile()))
            plaintext[:] = json.dumps(self.profile() | {"extra": 1}, separators=(",", ":")).encode()
            with self.assertRaises(device.DeviceError):
                device.profile_payload_from_json(ROOT, plaintext)
        finally:
            device.zeroize(plaintext)

    @unittest.skipUnless(os.environ.get("DESKKIN_TEST_AGE") == "1", "age round trip is run by the locked task")
    def test_age_round_trip_has_no_plaintext_file(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            os.chmod(root, 0o700)
            identity = root / "identity.txt"
            subprocess.run(
                ["age-keygen", "-o", str(identity)],
                check=True,
                stdout=subprocess.DEVNULL,
                stderr=subprocess.DEVNULL,
            )
            profile = root / "wifi.age"
            device.create_profile(profile, identity, self.profile())
            self.assertEqual(stat.S_IMODE(profile.stat().st_mode), 0o600)
            plaintext = device.decrypt_profile(profile, identity)
            self.assertEqual(json.loads(plaintext), self.profile())
            device.zeroize(plaintext)
            self.assertEqual(sorted(path.name for path in root.iterdir()), ["identity.txt", "wifi.age"])

    def test_atomic_result_is_private_and_replaced(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            first = device.publish_result(root, "run", "success", "first")
            second = device.publish_result(root, "run", "error", "second")
            self.assertEqual(first, second)
            self.assertEqual(stat.S_IMODE(first.stat().st_mode), 0o600)
            self.assertEqual(json.loads(first.read_text())["run_id"], "second")

    def test_device_diagnostics_are_private_bounded_and_allowlisted(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            records = [
                {
                    "operation": "device_ui",
                    "operation_id": 1,
                    "parent_operation_id": None,
                    "status": "success",
                    "error_type": None,
                    "effect_id": None,
                    "virtual_time_ms": 0,
                    "end_virtual_time_ms": 0,
                    "duration_ms": 0,
                    "render_width": 320,
                    "render_height": 240,
                    "value": "unknown",
                    "session_context_id": "00" * 16,
                    "operation_context_id": "11" * 16,
                    "rgb565_digest": "12345678",
                    "shell_state": 1,
                }
            ]
            for index in range(12):
                device.publish_diagnostic(root, f"run-{index:02}", "success", records)
            diagnostics = root / ".deskkin/phase3/device/diagnostics"
            self.assertEqual(len(list(diagnostics.glob("*.json"))), 10)
            self.assertTrue(all(stat.S_IMODE(path.stat().st_mode) == 0o600 for path in diagnostics.glob("*.json")))
            persisted = b"".join(path.read_bytes() for path in diagnostics.glob("*.json"))
            for forbidden in (b"password", b"ssid", b"socket_address", b"authentication_string", b"private_key"):
                self.assertNotIn(forbidden, persisted)

    def test_device_diagnostics_reject_symlink_root(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            target = root / "target"
            target.mkdir()
            (root / ".deskkin").symlink_to(target, target_is_directory=True)
            with self.assertRaises(OSError):
                device.publish_diagnostic(root, "run", "success", [])


if __name__ == "__main__":
    unittest.main()
