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
    def test_status_response_fits_every_ffi_completion_buffer(self):
        supervisor = (ROOT / "apps/core-s3-amp/src/main.c").read_text(encoding="utf-8")
        adapter = (ROOT / "apps/core-s3-service/src/adapter.c").read_text(encoding="utf-8")
        service = (ROOT / "apps/core-s3-service/src/lib.rs").read_text(encoding="utf-8")
        size = device.STATUS_RESPONSE_SIZE
        self.assertIn(f"#define STATUS_RESPONSE_SIZE {size}", supervisor)
        self.assertIn(f"#define COMPLETION_FRAME_MAX {size}", adapter)
        self.assertIn(f"const STATUS_RESPONSE_SIZE: usize = {size};", service)
        self.assertIn("uint8_t bytes[COMPLETION_FRAME_MAX];", adapter)
        self.assertIn("[0_u8; STATUS_RESPONSE_SIZE]", service)

    def test_raster_profile_is_coherent_bounded_and_scalar_only(self):
        clock = [0.0]
        generation = [0]
        def sample(*args, **kwargs):
            generation[0] += 1
            raw = bytearray(device.STATUS_RESPONSE_SIZE)
            raw[0] = 1
            raw[26] = 4
            raw[27] = 1
            raw[40:44] = generation[0].to_bytes(4, "big")
            raw[168:172] = generation[0].to_bytes(4, "big")
            for index in range(8):
                raw[172 + index * 4:176 + index * 4] = (100 + index).to_bytes(4, "big")
            return bytes(raw)
        with mock.patch.object(device, "run_control", side_effect=sample), mock.patch.object(
            device.time, "monotonic", side_effect=lambda: clock[0]
        ), mock.patch.object(device.time, "sleep", side_effect=lambda seconds: clock.__setitem__(0, clock[0] + seconds)), mock.patch.object(device.sys, "stderr", io.StringIO()):
            result = device.measure_raster(None, 1)
        self.assertEqual(result["profile_samples"], 5)
        self.assertEqual(result["coverage_us_mean"], 100)
        self.assertEqual(result["pixel_raster_us_max"], 103)
        self.assertEqual(result["profile_bilinear_samples_min"], 107)
        self.assertTrue(set(result) <= device.DIAGNOSTIC_RECORD_KEYS)
        with self.assertRaises(device.DeviceError):
            device.measure_raster(None, 121)

    def test_raster_profile_rejects_stopped_or_unpaired_input(self):
        for paired in (False, True):
            clock = [0.0]
            raw = bytearray(device.STATUS_RESPONSE_SIZE)
            raw[0] = raw[27] = 1
            raw[26] = 4 if paired else 0
            with mock.patch.object(device, "run_control", return_value=bytes(raw)), mock.patch.object(
                device.time, "monotonic", side_effect=lambda: clock[0]
            ), mock.patch.object(device.time, "sleep", side_effect=lambda seconds: clock.__setitem__(0, clock[0] + seconds)):
                with self.assertRaises(device.DeviceError):
                    device.measure_raster(None, 3)

    def test_device_build_inherits_cargo_network_policy(self):
        for offline in (None, "true", "false"):
            with self.subTest(offline=offline):
                environment = {"PATH": "/usr/bin"}
                if offline is not None:
                    environment["CARGO_NET_OFFLINE"] = offline
                with mock.patch.dict(os.environ, environment, clear=True):
                    _, actual = device.device_environment(ROOT)
                self.assertEqual(actual.get("CARGO_NET_OFFLINE"), offline)

    def test_dependency_patches_have_valid_unified_diff_structure(self):
        for patch in sorted((ROOT / "patches").glob("*/*.patch")):
            with self.subTest(patch=patch.relative_to(ROOT)):
                result = subprocess.run(
                    ["git", "apply", "--numstat", str(patch)],
                    cwd=ROOT,
                    capture_output=True,
                    text=True,
                    timeout=10,
                )
                self.assertEqual(result.returncode, 0, result.stderr)

    def profile(self):
        return {"schema_version": 1, "ssid": "fixture", "password": "fake-password", "host_ipv4": "192.168.10.2"}

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

    def test_world_benchmark_observes_progress_and_bounded_timings(self):
        clock = mock.Mock()
        clock.now = 0.0
        clock.monotonic.side_effect = lambda: clock.now
        clock.sleep.side_effect = lambda seconds: setattr(clock, "now", clock.now + seconds)

        generation = 10

        def status(*_args, **_kwargs):
            nonlocal generation
            generation += 3
            response = bytearray(device.STATUS_RESPONSE_SIZE)
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
            response[80] = 1
            benchmark_complete = clock.now >= device.WORLD_BENCHMARK_DURATION_SECONDS
            response[81] = 2 if benchmark_complete else 1
            response[82:84] = (10).to_bytes(2, "big")
            response[84:88] = (1_200).to_bytes(4, "big")
            response[92:96] = generation.to_bytes(4, "big")
            response[96:100] = generation.to_bytes(4, "big")
            response[100:104] = generation.to_bytes(4, "big")
            response[112:114] = (20).to_bytes(2, "big")
            response[114:116] = (6).to_bytes(2, "big")
            response[118] = device.WORLD_DEMO_ENTITY_COUNT - 1
            response[119] = 0 if benchmark_complete else 1
            response[120:124] = (2_000).to_bytes(4, "big")
            response[124:128] = (4_000).to_bytes(4, "big")
            response[128:132] = (100).to_bytes(4, "big")
            response[132:136] = (120).to_bytes(4, "big")
            response[136:140] = (20).to_bytes(4, "big")
            response[140:144] = (30).to_bytes(4, "big")
            response[144:148] = (2_000).to_bytes(4, "big")
            response[148:152] = (2_000).to_bytes(4, "big")
            response[152:156] = (8_000).to_bytes(4, "big")
            response[156:160] = (9_000).to_bytes(4, "big")
            return bytes(response)

        with mock.patch.object(device, "time", clock), mock.patch.object(
            device, "run_control", side_effect=status
        ) as run_control:
            summary = device.world_benchmark("/dev/fake")

        self.assertEqual(summary["value"], "observed")
        self.assertGreater(summary["completed_frames"], 0)
        self.assertGreaterEqual(summary["measured_fps_milli"], 20_000)
        self.assertLessEqual(summary["observation_duration_ms"], summary["duration_ms"])
        self.assertGreaterEqual(summary["measurement_coverage_milli"], 800)
        self.assertLessEqual(summary["last_observation_age_ms"], 1_000)
        self.assertEqual(summary["last_availability"], 1)
        self.assertEqual(summary["render_max_us"], 14_000)
        self.assertEqual(summary["transfer_max_us"], 44_000)
        self.assertEqual(summary["copy_last_us"], 3_000)
        self.assertEqual(summary["wire_last_us"], 39_000)
        self.assertEqual(summary["requested_updates"], 1_200)
        self.assertEqual(summary["visible_billboards"], device.WORLD_DEMO_ENTITY_COUNT - 1)
        self.assertEqual(summary["culled_billboards"], 1)
        self.assertEqual(summary["pixel_dma_batches"], 10)
        self.assertEqual(run_control.call_args_list[0].args[0], "world-benchmark-start")
        self.assertTrue(run_control.call_args_list[1].kwargs["recover_status_transport"])
        self.assertFalse(run_control.call_args_list[2].kwargs["recover_status_transport"])

    def test_world_status_rejects_allocation_failure(self):
        status = bytearray(device.STATUS_RESPONSE_SIZE)
        status[0] = 1
        status[27] = 1
        status[28:32] = (3).to_bytes(4, "big")
        status[40:44] = (2).to_bytes(4, "big")
        status[54] = 1
        status[56] = 1
        status[65] = 5
        status[66] = 28
        decoded = device.decode_world_status(bytes(status))

        self.assertEqual(decoded["allocation_failures"], 1)
        self.assertEqual(decoded["completed_frames"], 2)
        self.assertEqual(decoded["nvs_failure_stage"], 5)
        self.assertEqual(decoded["nvs_failure_code"], 28)

    def test_world_measurement_reports_less_than_twenty_fps(self):
        clock = mock.Mock()
        clock.now = 0.0
        clock.monotonic.side_effect = lambda: clock.now
        clock.sleep.side_effect = lambda seconds: setattr(clock, "now", clock.now + seconds)
        generation = 0

        def status(*_args, **_kwargs):
            nonlocal generation
            generation += 1
            response = bytearray(device.STATUS_RESPONSE_SIZE)
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
            response[80] = 1
            response[81] = 2
            response[82:84] = (1).to_bytes(2, "big")
            response[84:88] = (1_200).to_bytes(4, "big")
            response[92:96] = generation.to_bytes(4, "big")
            response[96:100] = generation.to_bytes(4, "big")
            response[112:114] = (20).to_bytes(2, "big")
            response[114:116] = (6).to_bytes(2, "big")
            response[118] = device.WORLD_DEMO_ENTITY_COUNT
            return bytes(response)

        with mock.patch.object(device, "time", clock), mock.patch.object(
            device, "run_control", side_effect=status
        ):
            summary = device.world_benchmark("/dev/fake")

        self.assertEqual(summary["value"], "observed")
        self.assertLess(summary["measured_fps_milli"], 20_000)

    def test_world_benchmark_rejects_renderer_that_stops_before_deadline(self):
        clock = mock.Mock()
        clock.now = 0.0
        clock.monotonic.side_effect = lambda: clock.now
        clock.sleep.side_effect = lambda seconds: setattr(clock, "now", clock.now + seconds)
        generation = 0

        def status(*_args, **_kwargs):
            nonlocal generation
            response = bytearray(device.STATUS_RESPONSE_SIZE)
            response[0] = 1
            if clock.now < device.WORLD_BENCHMARK_DURATION_SECONDS / 2:
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
            device.world_benchmark("/dev/fake")

    def test_world_benchmark_rejects_fresh_renderer_failure(self):
        clock = mock.Mock()
        clock.now = 0.0
        clock.monotonic.side_effect = lambda: clock.now
        clock.sleep.side_effect = lambda seconds: setattr(clock, "now", clock.now + seconds)
        generation = 0

        def status(*_args, **_kwargs):
            nonlocal generation
            generation += 1
            response = bytearray(device.STATUS_RESPONSE_SIZE)
            response[0] = 1
            response[27] = 1
            response[28:32] = generation.to_bytes(4, "big")
            response[32:40] = int(clock.now * 1000).to_bytes(8, "big")
            response[40:44] = generation.to_bytes(4, "big")
            response[44:48] = (10_000).to_bytes(4, "big")
            response[48:52] = (40_000).to_bytes(4, "big")
            response[52] = 5 if clock.now >= device.WORLD_BENCHMARK_DURATION_SECONDS - 0.25 else 4
            response[56] = 1
            response[80] = 1
            response[81] = 2
            response[84:88] = (1_200).to_bytes(4, "big")
            return bytes(response)

        with mock.patch.object(device, "time", clock), mock.patch.object(
            device, "run_control", side_effect=status
        ), self.assertRaises(device.WorldBenchmarkError) as raised:
            device.world_benchmark("/dev/fake")

        self.assertEqual(raised.exception.error_type, "world_benchmark_failed")
        self.assertEqual(raised.exception.summary["renderer_stage"], 5)
        self.assertEqual(raised.exception.summary["status"], "error")

    def test_amp_benchmark_cli_records_measurement_failure_summary(self):
        summary = {
            "operation": "world_benchmark",
            "status": "error",
            "error_type": "world_benchmark_failed",
            "duration_ms": 10_000,
            "renderer_stage": 5,
            "allocation_failures": 0,
            "transfer_failures": 0,
        }
        failure = device.WorldBenchmarkError("world_benchmark_failed", summary)
        with mock.patch.object(
            device.sys,
            "argv",
            ["phase3_device.py", "benchmark", "--duration-seconds", "10"],
        ), mock.patch.object(
            device, "world_benchmark", side_effect=failure
        ) as benchmark, mock.patch.object(device, "publish_diagnostic") as publish_diagnostic, mock.patch.object(
            device, "publish_result", return_value=Path("/tmp/result.json")
        ):
            exit_code = device.main()

        self.assertEqual(exit_code, 2)
        benchmark.assert_called_once_with(None, device.WORLD_BENCHMARK_DURATION_SECONDS)
        self.assertEqual(publish_diagnostic.call_args.args[2], "error")
        self.assertEqual(publish_diagnostic.call_args.args[3], [summary])

    def test_amp_supervisor_dram_ends_at_renderer_origin(self):
        linker = (ROOT / "apps/core-s3-amp/amp-dram-boundary.ld").read_text(encoding="utf-8")
        self.assertIn("ASSERT(_end <= 0x3fce4c00", linker)
        reservation = (
            ROOT / "patches/zephyr-core-s3/0011-reserve-appcpu-sram-on-both-cores.patch"
        ).read_text(encoding="utf-8")
        self.assertIn("#if defined(CONFIG_SOC_ENABLE_APPCPU)", reservation)
        self.assertIn("DRAM_ROM_BSS_DATA_START", reservation)
        self.assertIn("appcpu_dram_end = MIN", reservation)

    def test_amp_supervisor_keeps_framebuffers_internal_and_renderer_heap_in_psram(self):
        config = (ROOT / "apps/core-s3-amp/prj.conf").read_text(encoding="utf-8")
        source = (ROOT / "apps/core-s3-amp/src/main.c").read_text(encoding="utf-8")
        service_adapter = (ROOT / "apps/core-s3-service/src/adapter.c").read_text(
            encoding="utf-8"
        )
        self.assertIn("CONFIG_ESP_SPIRAM=y", config)
        self.assertIn("CONFIG_ESP_SPIRAM_HEAP_SIZE=3473408", config)
        self.assertIn("CONFIG_COMMON_LIBC_MALLOC_ARENA_SIZE=0", config)
        self.assertIn("CONFIG_INPUT_FT5336_INTERRUPT=y", config)
        self.assertIn("CONFIG_INPUT_MODE_SYNCHRONOUS=y", config)
        self.assertIn("shared_multi_heap_alloc(SMH_REG_ATTR_EXTERNAL", service_adapter)
        heap_patch = (ROOT / "patches/zephyr-core-s3/0009-start-spiram-heap-after-static-data.patch").read_text(
            encoding="utf-8"
        )
        self.assertIn("_ext_ram_heap_start", heap_patch)
        self.assertIn("_ext_ram_heap_end", heap_patch)
        self.assertIn("CONFIG_SPIRAM_MODE_QUAD=y", config)
        self.assertIn("CONFIG_SPIRAM_SPEED_80M=y", config)
        self.assertIn("static __aligned(32) uint16_t internal_framebuffer[2U][320U * 240U]", source)
        self.assertIn(".framebuffer = (uint32_t)(uintptr_t)internal_framebuffer", source)
        self.assertNotIn("renderer_framebuffer", source)
        self.assertIn("esp_psram_get_mapped_region", source)
        self.assertIn("#define RENDERER_HEAP_SIZE (4U * 1024U * 1024U)", source)
        self.assertIn("mapped_heap_size < RENDERER_HEAP_SIZE", source)
        self.assertIn("mapped_heap_size - renderer_heap_size", source)
        self.assertIn("renderer_heap <= (uintptr_t)mapped_heap", source)
        self.assertIn("cache_ll_l1_enable_bus(1, appcpu_bus)", source)

        sram2 = (ROOT / "apps/core-s3-amp/amp-sram2.ld").read_text(encoding="utf-8")
        hal_patch = (ROOT / "patches/hal-espressif-core-s3/0001-place-procpu-system-stacks-by-cache-requirement.patch").read_text(
            encoding="utf-8"
        )
        zephyr_patch = (ROOT / "patches/zephyr-core-s3/0006-place-wifi-network-state-in-spiram.patch").read_text(
            encoding="utf-8"
        )
        self.assertIn(".sram2.noinit 0x3fcf0000", sram2)
        self.assertIn("SIZEOF(.sram2.noinit) <= 0x8000", sram2)
        dram_boundary = (ROOT / "apps/core-s3-amp/amp-dram-boundary.ld").read_text(
            encoding="utf-8"
        )
        self.assertIn("_ext_ram_heap_end <= 0x3c400000", dram_boundary)
        self.assertIn('__attribute__((section(".sram2.noinit")))', service_adapter)
        self.assertIn('__attribute__((section(".sram2.noinit")))', source)
        self.assertIn(
            "service_stack[K_THREAD_STACK_LEN(DESKKIN_SERVICE_STACK_SIZE)]",
            service_adapter,
        )
        service_adapter_header = (
            ROOT / "apps/core-s3-service/src/adapter.h"
        ).read_text(encoding="utf-8")
        self.assertIn("#define DESKKIN_SERVICE_STACK_SIZE 21504U", service_adapter_header)
        self.assertIn("deskkin_control_stack[K_THREAD_STACK_LEN(3072)]", source)
        self.assertIn('__attribute__((section(".sram2.noinit")))', hal_patch)
        self.assertIn("_ext_ram_bss_start", zephyr_patch)
        self.assertIn("*libnet80211.a:(.bss .bss.*)", zephyr_patch)
        self.assertNotIn("*libsubsys__input.a:input.c.obj", zephyr_patch)
        self.assertIn("*libapp.a:adapter.c.obj", zephyr_patch)
        self.assertIn("CONFIG_MAIN_STACK_SIZE=4096", config)
        self.assertNotIn("renderer_boot_stack", source)

        amp_memory = (ROOT / "apps/core-s3-amp/amp-memory.overlay").read_text(
            encoding="utf-8"
        )
        self.assertIn("reg = <0x3fcee000 0x400>", amp_memory)
        self.assertIn("reg = <0x3fcee400 0x1000>", amp_memory)
        self.assertIn("#define DESKKIN_CHANNEL_OFFSET 0x400U", (ROOT / "apps/core-s3-amp/shared.h").read_text(encoding="utf-8"))
        self.assertIn("CONFIG_INIT_STACKS=y", config)
        renderer_config = (
            ROOT / "apps/core-s3-amp/renderer/prj.conf"
        ).read_text(encoding="utf-8")
        self.assertNotIn("CONFIG_INIT_STACKS=y", renderer_config)
        self.assertIn("k_thread_join(&z_main_thread, K_FOREVER)", source)
        self.assertIn("k_thread_stack_space_get(&z_main_thread", source)
        self.assertIn("memset((void *)main_start, 0, main_size)", source)
        self.assertIn("memset((void *)shared_start, 0, prefix_size)", source)
        self.assertNotIn("memset((void *)tail_start, 0, tail_size)", source)
        self.assertIn("shared_multi_heap_add(&runtime_sram_regions[index]", source)
        self.assertIn("wifi_boot_stack_start = (uintptr_t)service_stack", source)
        self.assertIn("memset((void *)wifi_boot_stack_start, 0, WIFI_BOOT_STACK_SIZE)", source)
        self.assertIn("if (thread == NULL)", source)
        self.assertLess(
            source.index("const int join_result = k_thread_join(&wifi_boot_thread"),
            source.rindex("memset((void *)wifi_boot_stack_start, 0, WIFI_BOOT_STACK_SIZE)"),
        )
        self.assertIn("atomic_set(&runtime_sram_ready, 1)", source)
        self.assertLess(
            source.index("atomic_set(&runtime_sram_ready, 1)"),
            source.index("wifi_boot_result = esp32_wifi_runtime_init()"),
        )
        supervisor_rust = (
            ROOT / "apps/core-s3-amp/src/lib.rs"
        ).read_text(encoding="utf-8")
        self.assertLess(
            supervisor_rust.index("if unsafe { deskkin_start_control_worker() }"),
            supervisor_rust.index("let _renderer_ready = unsafe { deskkin_amp_prepare_renderer() }"),
        )
        self.assertLess(
            source.index("wifi_boot_result = esp32_wifi_runtime_init()"),
            source.index("deskkin_start_service_after_runtime_handoff()"),
        )

        wifi_patch = (ROOT / "patches/zephyr-core-s3/0010-defer-wifi-until-runtime-sram-handoff.patch").read_text(
            encoding="utf-8"
        )
        self.assertIn("int esp32_wifi_runtime_init(void)", wifi_patch)
        self.assertIn("atomic_cas(&esp32_wifi_runtime_state, 0, 1)", wifi_patch)
        runtime_heap_patch = (ROOT / "patches/hal-espressif-core-s3/0003-use-phase-owned-internal-runtime-heap.patch").read_text(
            encoding="utf-8"
        )
        self.assertIn("deskkin_runtime_internal_calloc", runtime_heap_patch)
        self.assertIn("deskkin_runtime_internal_free", runtime_heap_patch)

        renderer_config = (ROOT / "apps/core-s3-amp/renderer/prj.conf").read_text(
            encoding="utf-8"
        )
        self.assertIn(
            'CONFIG_MCUBOOT_EXTRA_IMGTOOL_ARGS="--slot-size 0x300000"',
            renderer_config,
        )

    def test_amp_boot_control_precedes_phase_handoff_wifi_and_service(self):
        entry = (ROOT / "apps/core-s3-amp/src/lib.rs").read_text(encoding="utf-8")
        supervisor = (ROOT / "apps/core-s3-amp/src/main.c").read_text(encoding="utf-8")
        config = (ROOT / "apps/core-s3-amp/prj.conf").read_text(encoding="utf-8")
        rust_main = entry.split('extern "C" fn rust_main()', 1)[1].split("#[no_mangle]", 1)[0]
        self.assertLess(
            rust_main.index("deskkin_start_control_worker()"),
            rust_main.index("deskkin_amp_prepare_renderer()"),
        )
        self.assertLess(
            rust_main.index("deskkin_amp_prepare_renderer()"),
            rust_main.index("deskkin_amp_supervisor_main()"),
        )
        self.assertNotIn("deskkin_core_s3_service::start()", rust_main)
        self.assertIn("deskkin_start_service_after_runtime_handoff", entry)
        self.assertLess(
            supervisor.index("result = initialize_runtime_sram()"),
            supervisor.index("result = complete_wifi_boot_phase()"),
        )
        self.assertLess(
            supervisor.index("result = complete_wifi_boot_phase()"),
            supervisor.index("deskkin_start_service_after_runtime_handoff()"),
        )
        self.assertNotIn("deskkin_start_control_worker()", supervisor)
        self.assertNotIn("deskkin_allocator_init", entry)
        self.assertIn("set_boot_stage(9);", supervisor)
        self.assertIn("CONFIG_COMMON_LIBC_MALLOC_ARENA_SIZE=0", config)
        self.assertIn("CONFIG_NET_CONFIG_AUTO_INIT=n", config)
        self.assertIn("CONFIG_PRINTK=n", config)
        self.assertNotIn("deskkin_amp_boot_trace", entry)
        self.assertNotIn("deskkin_amp_boot_trace", supervisor)

    def test_amp_stalls_appcpu_while_flash_cache_can_be_disabled(self):
        supervisor = (ROOT / "apps/core-s3-amp/src/main.c").read_text(encoding="utf-8")
        service = (ROOT / "apps/core-s3-service/src/adapter.c").read_text(encoding="utf-8")

        guard_enter = supervisor.index("void deskkin_flash_guard_enter(void)")
        stall = supervisor.index("esp_cpu_stall(1);", guard_enter)
        guard_exit = supervisor.index("void deskkin_flash_guard_exit(void)", stall)
        unstall = supervisor.index("esp_cpu_unstall(1);", guard_exit)
        unlock = supervisor.index("k_mutex_unlock(&appcpu_flash_mutex);", unstall)
        self.assertLess(guard_enter, stall)
        self.assertLess(guard_exit, unstall)
        self.assertLess(unstall, unlock)

        prepare = supervisor.index("int deskkin_amp_prepare_renderer(void)")
        shared = supervisor.index("memset((void *)AMP_SHARED", prepare)
        display_publication = supervisor.index("publish_display_ready();", shared)
        appcpu_init = supervisor.index("esp_appcpu_init()", shared)
        running = supervisor.index("appcpu_running = true;", appcpu_init)
        platform_ready = supervisor.index("display_spi_hz) == 0U", running)
        self.assertLess(shared, display_publication)
        self.assertLess(display_publication, appcpu_init)
        self.assertLess(shared, appcpu_init)
        self.assertLess(appcpu_init, running)
        self.assertLess(running, platform_ready)

        self.assertIn("deskkin_flash_guard_enter();\n\tconst struct flash_area *area;", service)
        self.assertIn("nvs_mount(&storage);", service)
        self.assertIn("deskkin_flash_guard_enter();\n\tconst int length = nvs_read", service)
        self.assertIn("deskkin_flash_guard_enter();\n\tresult = nvs_write", service)
        self.assertIn("deskkin_flash_guard_enter();\n\tconst int delete_result = nvs_delete", service)
        self.assertIn("DESKKIN_NVS_FAILURE_INTENT_READ", service)
        self.assertIn("DESKKIN_NVS_FAILURE_RECORD_READBACK", service)
        self.assertIn("const uint16_t nvs_failure = deskkin_nvs_last_failure();", supervisor)
        self.assertIn("response[65] = (uint8_t)(nvs_failure >> 8);", supervisor)
        self.assertIn("response[66] = (uint8_t)nvs_failure;", supervisor)
        self.assertIn("#define STATUS_RESPONSE_SIZE 224", supervisor)
        self.assertIn("observe_renderer_stale(&renderer_stale_reported);", supervisor)
        self.assertIn("&AMP_SHARED->renderer_progress), &response[160]", supervisor)
        self.assertIn("&AMP_SHARED->display_progress), &response[164]", supervisor)

    def test_amp_renderer_swaps_full_frames_without_reuse_overlap(self):
        config = (ROOT / "apps/core-s3-amp/renderer/prj.conf").read_text(encoding="utf-8")
        renderer = (ROOT / "apps/core-s3-amp/renderer/src/lib.rs").read_text(encoding="utf-8")
        adapter = (ROOT / "apps/core-s3-amp/renderer/src/adapter.c").read_text(encoding="utf-8")
        bootstrap = (ROOT / "scripts/bootstrap_core_s3.sh").read_text(encoding="utf-8")
        self.assertIn("const BUFFER_COUNT: usize = 2;", renderer)
        self.assertIn("RepaintBufferType::SwappedBuffers", renderer)
        self.assertIn("renderer.render(framebuffer.pixels_mut(index), WIDTH)", renderer)
        self.assertNotIn("render_by_line", renderer)
        self.assertIn("deskkin_display_submit", renderer)
        self.assertIn("deskkin_display_take_completion", renderer)
        self.assertIn("framebuffer.wait_for_back_buffer()?", renderer)
        self.assertIn(".begin_render(index)", renderer)
        self.assertIn(".submit(index)", renderer)
        self.assertIn("self.back ^= 1;", renderer)
        self.assertIn("#define FRAME_PIXELS (DISPLAY_WIDTH * DISPLAY_HEIGHT)", adapter)
        self.assertNotIn("CONFIG_ESP_SPIRAM", config)
        self.assertIn("sys_heap_init(&renderer_heap", adapter)
        self.assertIn("sys_heap_alloc(&renderer_heap", adapter)
        self.assertIn("sys_heap_aligned_alloc(&renderer_heap", adapter)
        self.assertIn("CONFIG_TIMESLICE_PER_THREAD=y", config)
        self.assertIn("#define RENDERER_TIME_SLICE_TICKS 1", adapter)
        self.assertIn("k_thread_create(&display_thread, display_stack, 4096", adapter)
        self.assertIn("k_thread_create(&renderer_thread, renderer_stack, 32768", adapter)
        self.assertNotIn("K_THREAD_STACK_DEFINE(display_stack", adapter)
        self.assertNotIn("K_THREAD_STACK_DEFINE(renderer_stack", adapter)
        self.assertIn("atomic_inc(&allocation_failures)", adapter)
        self.assertIn("K_MSGQ_DEFINE(display_requests", adapter)
        self.assertIn("display_entry, NULL, NULL, NULL, 0, 0, K_NO_WAIT", adapter)
        self.assertIn("renderer_entry, NULL, NULL, NULL, 0, 0, K_FOREVER", adapter)
        self.assertIn(
            "k_thread_time_slice_set(display_tid, RENDERER_TIME_SLICE_TICKS, NULL, NULL)",
            adapter,
        )
        self.assertIn(
            "k_thread_time_slice_set(renderer_tid, RENDERER_TIME_SLICE_TICKS, NULL, NULL)",
            adapter,
        )
        self.assertIn("k_thread_start(renderer_tid)", adapter)
        self.assertIn("k_yield();", adapter)
        self.assertNotIn("DMA_STAGING_LINES", adapter)
        self.assertNotIn("dma_staging", adapter)
        self.assertIn("display_write(display, 0, 0, &descriptor, pixels)", adapter)
        self.assertIn("FULL_FRAME_DMA_BATCHES", adapter)
        self.assertIn("atomic_set(&pixel_dma_batches, FULL_FRAME_DMA_BATCHES)", adapter)
        self.assertIn("deskkin_renderer_progress(RendererProgress::Buffer as u8)", renderer)
        self.assertIn("display_progress(DESKKIN_DISPLAY_PROGRESS_TRANSFER)", adapter)
        self.assertIn("&AMP_SHARED->renderer_progress", adapter)
        self.assertIn("&AMP_SHARED->display_progress", adapter)
        spi_patch = (ROOT / "patches/zephyr-core-s3/0001-route-esp32-spi-pixel-payloads-through-dma.patch").read_text(
            encoding="utf-8"
        )
        self.assertIn("while (!spi_hal_usr_is_done(hal))", spi_patch)
        self.assertNotIn("k_sleep(K_TICKS(1))", spi_patch)
        self.assertIn("SPI_DMA_MAX_BUFFER_SIZE (4092 * 8)", spi_patch)
        self.assertIn("SPI_TRANSFER_TIMEOUT_MS 100", spi_patch)
        self.assertNotIn("esp_ptr_dma_ext_capable", spi_patch)
        gdma_patch = (
            ROOT
            / "patches/zephyr-core-s3/0012-disable-unused-gdma-completion-interrupts.patch"
        ).read_text(encoding="utf-8")
        self.assertIn("dma_channel->cb != NULL", gdma_patch)
        self.assertIn('" M drivers/dma/dma_esp32_gdma.c"', bootstrap)
        self.assertIn("69df8e02eee5caf343859f0bfad651ac1f3d0605c5b8c735bb1985cbc543bd6d", bootstrap)
        sleep_poll_migration = bootstrap.split(
            "ade32e58926d12ea981d512c6d4adb92812f55fda603d98a174fd146b6e4adf8", 1
        )[1].split(";;", 1)[0]
        self.assertIn("drivers/spi/spi_esp32_spim.c", sleep_poll_migration)
        legacy_migration = bootstrap.split(
            "2aa1a66261802c19f97df062bcff61b9781d4d42caa5599edb2f2ab7ebdf3dab", 1
        )[1].split(";;", 1)[0]
        for legacy_path in (
            "drivers/spi/spi_esp32_spim.c",
            "drivers/display/display_ili9xxx.c",
            "drivers/mipi_dbi/mipi_dbi_esp32.c",
            "include/zephyr/drivers/mipi_dbi.h",
        ):
            self.assertIn(legacy_path, legacy_migration)
        self.assertFalse(
            (ROOT / "patches/zephyr-core-s3/0004-write-strided-mipi-dbi-regions.patch").exists()
        )
        self.assertIn("CONFIG_TICKLESS_KERNEL=n", config)
        self.assertIn("CONFIG_SYS_CLOCK_TICKS_PER_SEC=1000", config)
        self.assertIn("CONFIG_HEAP_MEM_POOL_SIZE=1024", config)
        self.assertNotIn("CONFIG_HEAP_MEM_POOL_IGNORE_MIN=y", config)
        overlay = (ROOT / "apps/core-s3-amp/renderer/app.overlay").read_text(encoding="utf-8")
        self.assertIn('&dma {\n\tstatus = "okay";\n\tzephyr,deferred-init;', overlay)
        self.assertIn("DMA_IN_CH0_INTR_SOURCE", overlay)
        self.assertIn("DMA_OUT_CH0_INTR_SOURCE", overlay)
        self.assertIn("\tdma-channels = <2>;", overlay)
        self.assertIn("\tdma-enabled;", overlay)
        self.assertIn("\tdmas = <&dma 0>, <&dma 1>;", overlay)
        self.assertIn('\tdma-names = "rx", "tx";', overlay)
        self.assertIn("display_dma,\n\t\tdisplay_spi,\n\t\tmipi_dbi,", adapter)

    def test_amp_entry_trampoline_is_a_pinned_zephyr_patch(self):
        patch = (ROOT / "patches/zephyr-core-s3/0005-initialize-appcpu-window-state.patch").read_text(
            encoding="utf-8"
        )
        build = (ROOT / "scripts/phase3_device.py").read_text(encoding="utf-8")
        bootstrap = (ROOT / "scripts/bootstrap_core_s3.sh").read_text(encoding="utf-8")
        self.assertIn("wsr.windowstart a1", patch)
        self.assertIn('wsr.windowbase a0', patch)
        self.assertIn('STRINGIFY(CONFIG_ISR_STACK_SIZE) "\\n\\t"', patch)
        self.assertNotIn('STRINGIFY(CONFIG_ISR_STACK_SIZE) " + 16', patch)
        self.assertIn("vecbase; rsync", patch)
        self.assertIn("j __appcpu_start_c", patch)
        self.assertNotIn("call8 __appcpu_start_c", patch)
        self.assertIn('" M soc/espressif/esp32s3/soc_appcpu.c"', bootstrap)
        prior_migration = bootstrap.split(
            "bf0b6c23ddf842be5bc5bc82ebfa2a0632150ef71915acba81288047da52fc32",
            1,
        )[1].split(";;", 1)[0]
        self.assertIn("soc/espressif/esp32s3/soc_appcpu.c", prior_migration)
        self.assertNotIn("drivers/spi/spi_esp32_spim.c", prior_migration)
        self.assertNotIn("patched_appcpu_source", build)
        self.assertNotIn("appcpu_source.write_text", build)

    def test_amp_loader_preserves_live_procpu_cache_and_segment_identity(self):
        patch = (ROOT / "patches/zephyr-core-s3/0008-preserve-procpu-cache-while-mapping-appcpu.patch").read_text(
            encoding="utf-8"
        )
        bootstrap = (ROOT / "scripts/bootstrap_core_s3.sh").read_text(encoding="utf-8")
        self.assertIn("if (core == 0)", patch)
        self.assertIn(".irom_map_addr = image_header.irom_map_addr", patch)
        self.assertIn(".drom_map_addr = image_header.drom_map_addr", patch)
        self.assertIn("#if !defined(CONFIG_BOOTLOADER_MCUBOOT)", patch)
        self.assertIn('" M soc/espressif/common/loader.c"', bootstrap)
        self.assertIn('" M soc/espressif/esp32s3/esp32s3-mp.c"', bootstrap)

    def test_touch_overflow_counter_survives_followup_empty_reads(self):
        adapter = (ROOT / "apps/core-s3-amp/renderer/src/adapter.c").read_text(encoding="utf-8")
        read = adapter[adapter.index("int deskkin_touch_read("):adapter.index("void deskkin_publish_target_yaw", adapter.index("int deskkin_touch_read("))]
        self.assertIn("cumulative_drops = deskkin_shared_load(&AMP_SHARED->touch.drop_count)", read)
        self.assertIn("deskkin_shared_store(&AMP_SHARED->touch.drop_count, cumulative_drops)", read)
        self.assertIn("? UINT32_MAX", read)
        self.assertLess(read.index("*drop_count = cumulative_drops"), read.index("if (latest == 0U"))

    def test_koyori_qoi_loop_assets_have_closed_native_geometry(self):
        asset_root = ROOT / "assets/pets/koyori"
        expected = {
            "idle.qoi": (864, 156),
            "move-right.qoi": (1152, 156),
            "move-left.qoi": (1152, 156),
            "attend.qoi": (864, 156),
        }
        self.assertFalse((asset_root / "atlas.png").exists())
        for name, dimensions in expected.items():
            header = (asset_root / name).read_bytes()[:14]
            self.assertEqual(header[:4], b"qoif")
            self.assertEqual(
                (int.from_bytes(header[4:8], "big"), int.from_bytes(header[8:12], "big")),
                dimensions,
            )
            self.assertEqual(header[12], 4)

    def test_amp_renderer_releases_active_loop_before_direct_qoi_decode(self):
        renderer = (ROOT / "apps/core-s3-amp/renderer/src/lib.rs").read_text(encoding="utf-8")
        release = renderer.index("component.set_pet_atlas(Image::default())")
        decode = renderer.index("let next = decode_loop", release)
        install = renderer.index("component.set_pet_atlas(next.image.clone())", decode)
        redraw = renderer.index("component.set_pet_frame_index(0)", install)
        self.assertLess(release, decode)
        self.assertLess(decode, install)
        self.assertLess(install, redraw)
        self.assertIn("decode_to_buf(pixels.make_mut_bytes())", renderer)
        self.assertIn("RendererStage::AssetLoading", renderer)
        self.assertIn("RendererStage::AssetReady", renderer)
        self.assertIn("RendererFault::QoiHeader", renderer)
        self.assertIn("RendererFault::QoiMetadata", renderer)
        self.assertIn("RendererFault::QoiDecode", renderer)

    def test_amp_buffer_ownership_requires_completion_before_reuse(self):
        source = ROOT / "apps/core-s3-amp/renderer/src/buffer_ownership.rs"
        with tempfile.TemporaryDirectory() as temporary:
            executable = Path(temporary) / "band_ownership_test"
            subprocess.run(
                ["rustc", "--edition=2021", "--test", str(source), "-o", str(executable)],
                check=True,
            )
            subprocess.run([str(executable)], check=True)

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
                    str(ROOT / "apps/core-s3-service/src"),
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

    def test_control_response_accepts_extended_amp_status(self):
        frame = device.control_frame("status", 0)
        response = bytearray(device.STATUS_RESPONSE_SIZE)
        response[0] = 1
        response[2:18] = frame[4:20]
        stream = bytearray(len(response).to_bytes(2, "big"))
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
        status = bytearray(device.STATUS_RESPONSE_SIZE)
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
        status = bytearray(device.STATUS_RESPONSE_SIZE)
        status[0] = 1
        status[69] = 2
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
        status = bytearray(device.STATUS_RESPONSE_SIZE)
        status[69] = 255
        self.assertEqual(device.status_boot_error(bytes(status)), "boot_unknown")

    def test_status_waits_until_boot_is_complete(self):
        starting = bytearray(device.STATUS_RESPONSE_SIZE)
        starting[68] = 8
        complete = bytearray(starting)
        complete[68] = device.BOOT_COMPLETE_STAGE
        with mock.patch.object(device, "run_control", return_value=bytes(complete)) as run_control, mock.patch.object(
            device.time, "sleep"
        ) as sleep:
            result = device.await_boot_complete(bytes(starting), "/dev/fake")
        self.assertEqual(result, bytes(complete))
        sleep.assert_called_once_with(0.25)
        run_control.assert_called_once_with("status", "/dev/fake", recover_status_transport=False)

    def test_status_boot_wait_is_bounded(self):
        starting = bytearray(device.STATUS_RESPONSE_SIZE)
        starting[68] = 8
        with mock.patch.object(device.time, "monotonic", side_effect=[0.0, 15.0]):
            with self.assertRaisesRegex(device.DeviceError, "boot_not_ready"):
                device.await_boot_complete(bytes(starting), "/dev/fake")

    def test_status_rejects_unknown_completed_boot_stage(self):
        status = bytearray(device.STATUS_RESPONSE_SIZE)
        status[68] = 255
        with self.assertRaisesRegex(device.DeviceError, "boot_unknown"):
            device.await_boot_complete(bytes(status), "/dev/fake")

    def test_boot_failure_is_not_persisted_as_a_complete_diagnostic(self):
        status = bytearray(device.STATUS_RESPONSE_SIZE)
        status[69] = 2
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
        status = bytearray(device.STATUS_RESPONSE_SIZE)
        status[0] = 1
        status[68] = 9
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
        status = bytearray(device.STATUS_RESPONSE_SIZE)
        status[0] = 1
        status[26] = 4
        status[27] = 1
        status[80] = 1
        status[68] = 7
        status[92:96] = (5).to_bytes(4, "big")
        status[96:100] = (6).to_bytes(4, "big")
        status[100:104] = (7).to_bytes(4, "big")
        status[104:108] = (1).to_bytes(4, "big")
        status[108:112] = (2).to_bytes(4, "big")
        status[160:164] = ((11 << 8) | 6).to_bytes(4, "big")
        status[164:168] = ((9 << 8) | 3).to_bytes(4, "big")
        with mock.patch.object(device.sys, "stderr") as stderr:
            device.report_status(bytes(status))
        reported = json.loads("".join(call.args[0] for call in stderr.write.call_args_list))
        self.assertEqual(
            reported,
            {
                "shell_state": 4,
                "availability": 1,
                "heartbeat_freshness": 1,
                "renderer_stage": 0,
                "renderer_fault": 0,
                "completed_frames": 0,
                "render_us": 0,
                "transfer_us": 0,
                "render_max_us": 0,
                "transfer_max_us": 0,
                "display_ready": 0,
                "display_spi_hz": 0,
                "pixel_dma_batches": 0,
                "allocation_failures": 0,
                "transfer_failures": 0,
                "nvs_failure_stage": 0,
                "nvs_failure_code": 0,
                "view_generation": 5,
                "renderer_shell": None,
                "shell_property_matches": None,
                "pixel_transfer_count": None,
                "pixel_transfer_last_us": None,
                "frame_difference_last": None,
                "frame_difference_max": None,
                "pose_generation": 6,
                "input_generation": 7,
                "stale_snapshots": 1,
                "touch_drops": 2,
                "renderer_progress_sequence": 11,
                "renderer_progress_stage": 6,
                "display_progress_sequence": 9,
                "display_progress_stage": 3,
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
            "availability": 1,
            "generation": 7,
            "completed_frames": 1,
            "renderer_stage": 4,
            "renderer_fault": 0,
            "allocation_failures": 0,
            "transfer_failures": 0,
            "pixel_dma_batches": 1,
            "stale_snapshots": 0,
            "valid_availability_result": True,
            "valid_view_generation": 6,
            "view_generation": 7,
        }
        self.assertTrue(device.run_succeeded([record], 7))
        self.assertTrue(device.run_succeeded([record | {"availability": 2}], 7))
        for key, invalid in (
            ("availability", 0),
            ("generation", 0),
            ("completed_frames", 0),
            ("valid_availability_result", False),
            ("valid_view_generation", 0),
            ("renderer_fault", 1),
            ("pixel_dma_batches", 0),
            ("stale_snapshots", 1),
        ):
            self.assertFalse(device.run_succeeded([record | {key: invalid}], 7))

    def test_world_benchmark_uses_the_amp_product_observation_path(self):
        summary = {
            "operation": "world_benchmark",
            "status": "success",
            "error_type": None,
            "duration_ms": 60_000,
            "completed_frames": 1_200,
        }
        with mock.patch.object(device.sys, "argv", ["phase3_device.py", "benchmark", "--device", "/dev/fake"]), mock.patch.object(
            device, "world_benchmark", return_value=summary
        ) as benchmark, mock.patch.object(
            device, "publish_diagnostic"
        ) as publish_diagnostic, mock.patch.object(
            device, "publish_result", return_value=Path("/tmp/benchmark.json")
        ), mock.patch.object(device.sys, "stdout", io.StringIO()), mock.patch.object(
            device.sys, "stderr", io.StringIO()
        ):
            self.assertEqual(device.main(), 0)
        benchmark.assert_called_once_with("/dev/fake", device.WORLD_BENCHMARK_DURATION_SECONDS)
        record = publish_diagnostic.call_args.args[3][0]
        self.assertEqual(record["operation"], "world_benchmark")
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
                    "sas": "123456",
                    "asset_path": "/private/pet.qoi",
                    "touch_coordinates": [[1, 2]],
                    "shell_state": 1,
                }
            ]
            for index in range(12):
                device.publish_diagnostic(root, f"run-{index:02}", "success", records)
            diagnostics = root / ".deskkin/phase3/device/diagnostics"
            self.assertEqual(len(list(diagnostics.glob("*.json"))), 10)
            self.assertTrue(all(stat.S_IMODE(path.stat().st_mode) == 0o600 for path in diagnostics.glob("*.json")))
            persisted = b"".join(path.read_bytes() for path in diagnostics.glob("*.json"))
            for forbidden in (
                b"password", b"ssid", b"socket_address", b"authentication_string",
                b"private_key", b"rgb565_digest", b"12345678", b"sas",
                b"asset_path", b"pet.qoi", b"touch_coordinates",
            ):
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
