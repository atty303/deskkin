// SPDX-License-Identifier: MIT

#include <errno.h>
#include <stdint.h>
#include <string.h>
#include <esp_cpu.h>
#include <esp_attr.h>
#include <esp_heap_caps.h>
#include <esp_psram.h>
#include <hal/cache_ll.h>
#include <rom/ets_sys.h>
#include <zephyr/device.h>
#include <zephyr/drivers/gpio.h>
#include <zephyr/input/input.h>
#include <zephyr/drivers/regulator.h>
#include <zephyr/drivers/uart.h>
#include <zephyr/kernel.h>
#include <zephyr/kernel/thread_stack.h>
#include <zephyr/multi_heap/shared_multi_heap.h>
#include <zephyr/sys/printk.h>
#include <zephyr/sys/byteorder.h>
#include "../../core-s3-service/src/adapter.h"
#include "../shared.h"

#define CONTROL_FRAME_MAX 188
#define STATUS_RESPONSE_SIZE 168
#define HEARTBEAT_STALE_MS 500
#define DIAGNOSTIC_EVENT_CAPACITY 64U
#define DIAGNOSTIC_EVENT_SIZE 24U
#define AMP_SHARED                                                                                 \
	((volatile struct deskkin_amp_shared *)(DT_REG_ADDR(DT_NODELABEL(shm0)) +                  \
					       DESKKIN_CHANNEL_OFFSET))

static const struct device *const console = DEVICE_DT_GET(DT_CHOSEN(zephyr_console));
static const struct device *const lcd_reset_gpio = DEVICE_DT_GET(DT_NODELABEL(aw9523b_gpio));
static const struct device *const lcd_supply = DEVICE_DT_GET(DT_NODELABEL(vcc_bl));
static atomic_t heartbeat_generation;
static atomic_t heartbeat_received_ms;
static atomic_t completed_frames;
static atomic_t render_us;
static atomic_t transfer_us;
static atomic_t copy_us;
static atomic_t render_max_us;
static atomic_t transfer_max_us;
static atomic_t renderer_stage;
static atomic_t renderer_fault;
static atomic_t allocation_failures;
static atomic_t transfer_failures;
static atomic_t dirty_rect_count;
static atomic_t pixel_dma_batches;
static atomic_t dirty_pixels;
static atomic_t transferred_bytes;
static atomic_t view_generation;
static atomic_t pose_generation;
static atomic_t input_generation;
static atomic_t stale_snapshots;
static atomic_t touch_drops;
static atomic_t atlas_cache_hits;
static atomic_t atlas_cache_misses;
static atomic_t atlas_cache_failures;
static atomic_t visible_billboards;
static atomic_t culled_billboards;
static atomic_t renderer_shell;
static atomic_t renderer_shell_property_matches;
static atomic_t nearest_samples;
static atomic_t bilinear_samples;
static atomic_t projection_us;
static atomic_t projection_max_us;
static atomic_t sort_us;
static atomic_t sort_max_us;
static atomic_t texture_us;
static atomic_t texture_max_us;
static atomic_t world_raster_us;
static atomic_t world_raster_max_us;
static atomic_t deadline_misses;
static atomic_t display_ready;

static atomic_t boot_stage;
static atomic_t boot_error;
static __aligned(32) uint16_t internal_framebuffer[2U][320U * 240U];
#define RENDERER_HEAP_SIZE (4U * 1024U * 1024U)
static uintptr_t renderer_heap;
static size_t renderer_heap_size;
static const struct device *const touch = DEVICE_DT_GET(DT_CHOSEN(zephyr_touch));
static atomic_t touch_x;
static atomic_t touch_y;
static atomic_t touch_pressed;
static uint32_t world_generation;
static int64_t observed_yaw;
static int64_t target_yaw;
static uint32_t touch_generation;
static uint16_t observed_rate_remainder;
static uint32_t command_generation;
static atomic_t world_benchmark_active;
static atomic_t world_benchmark_complete;
static uint32_t world_benchmark_started_generation;
static atomic_t valid_view_generation;
static bool valid_snapshot_published;
static uint32_t valid_snapshot_attempt;
K_MUTEX_DEFINE(appcpu_flash_mutex);
static bool appcpu_running;

enum deskkin_diagnostic_kind {
	DESKKIN_DIAGNOSTIC_BOOT = 1,
	DESKKIN_DIAGNOSTIC_RENDERER = 2,
	DESKKIN_DIAGNOSTIC_SHELL = 3,
	DESKKIN_DIAGNOSTIC_TOUCH = 4,
	DESKKIN_DIAGNOSTIC_UI_COMMAND = 5,
	DESKKIN_DIAGNOSTIC_SERVICE = 6,
	DESKKIN_DIAGNOSTIC_MEMORY = 7,
};

struct deskkin_diagnostic_event {
	uint32_t sequence;
	uint32_t uptime_ms;
	int16_t x;
	int16_t y;
	uint32_t value;
	uint8_t kind;
	uint8_t flags;
	uint8_t reserved[2];
};

static struct deskkin_diagnostic_event diagnostic_events[DIAGNOSTIC_EVENT_CAPACITY]
	__attribute__((section(".ext_ram.bss")));
static uint32_t diagnostic_sequence;
extern void deskkin_service_ui_command(uint8_t command);

static void diagnostic_record(uint8_t kind, uint8_t flags, int16_t x, int16_t y,
			      uint32_t value)
{
	unsigned int key = irq_lock();
	diagnostic_sequence++;
	if (diagnostic_sequence == 0U) {
		diagnostic_sequence = 1U;
	}
	diagnostic_events[(diagnostic_sequence - 1U) % DIAGNOSTIC_EVENT_CAPACITY] =
		(struct deskkin_diagnostic_event){
			.sequence = diagnostic_sequence,
			.uptime_ms = k_uptime_get_32(),
			.x = x,
			.y = y,
			.value = value,
			.kind = kind,
			.flags = flags,
		};
	irq_unlock(key);
}

void deskkin_service_trace(uint8_t stage, uint8_t error)
{
	diagnostic_record(DESKKIN_DIAGNOSTIC_SERVICE, stage, 0, 0, error);
}

void deskkin_service_result(uint8_t stage, int32_t result)
{
	diagnostic_record(DESKKIN_DIAGNOSTIC_SERVICE, stage | 0x80U, 0, 0,
			  (uint32_t)result);
}

void deskkin_debug_pair_request(void)
{
	diagnostic_record(DESKKIN_DIAGNOSTIC_UI_COMMAND, 1U, 0, 0, 1U);
	deskkin_service_ui_command(1U);
}

static void allocation_failed_probe(size_t requested_size, uint32_t caps,
				    const char *function_name)
{
	ARG_UNUSED(caps);
	ARG_UNUSED(function_name);
	diagnostic_record(DESKKIN_DIAGNOSTIC_SERVICE, 0x88U, 0, 0,
			  requested_size > INT32_MAX ? INT32_MAX : (uint32_t)requested_size);
}

void deskkin_install_allocation_failed_probe(void)
{
	(void)heap_caps_register_failed_alloc_callback(allocation_failed_probe);
}

int deskkin_diagnostic_read(uint32_t after_sequence, uint8_t *output, size_t capacity)
{
	if (output == NULL || capacity < DIAGNOSTIC_EVENT_SIZE) {
		return -EINVAL;
	}
	unsigned int key = irq_lock();
	const uint32_t newest = diagnostic_sequence;
	if (newest == 0U || after_sequence == newest) {
		irq_unlock(key);
		return 0;
	}
	const uint32_t oldest = newest >= DIAGNOSTIC_EVENT_CAPACITY
				? newest - DIAGNOSTIC_EVENT_CAPACITY + 1U
				: 1U;
	const uint32_t wanted = after_sequence + 1U < oldest ? oldest : after_sequence + 1U;
	const struct deskkin_diagnostic_event event =
		diagnostic_events[(wanted - 1U) % DIAGNOSTIC_EVENT_CAPACITY];
	irq_unlock(key);
	memset(output, 0, DIAGNOSTIC_EVENT_SIZE);
	output[0] = 1U;
	output[1] = 0x80U;
	output[2] = event.kind;
	output[3] = event.flags;
	sys_put_be32(event.sequence, &output[4]);
	sys_put_be32(event.uptime_ms, &output[8]);
	sys_put_be16((uint16_t)event.x, &output[12]);
	sys_put_be16((uint16_t)event.y, &output[14]);
	sys_put_be32(event.value, &output[16]);
	sys_put_be32(wanted - (after_sequence + 1U), &output[20]);
	return DIAGNOSTIC_EVENT_SIZE;
}

static void set_boot_stage(uint8_t stage)
{
	if ((uint8_t)atomic_get(&boot_stage) == stage) {
		return;
	}
	atomic_set(&boot_stage, stage);
	diagnostic_record(DESKKIN_DIAGNOSTIC_BOOT, stage, 0, 0,
			  (uint32_t)(uint8_t)atomic_get(&boot_error));
}

static void set_boot_error(uint8_t error)
{
	if ((uint8_t)atomic_get(&boot_error) == error) {
		return;
	}
	atomic_set(&boot_error, error);
	diagnostic_record(DESKKIN_DIAGNOSTIC_BOOT, (uint8_t)atomic_get(&boot_stage), 0, 0,
			  error);
}

extern uint8_t deskkin_service_shell(void);
extern uint32_t deskkin_service_sas(void);
extern uint8_t deskkin_service_availability(void);
extern uint8_t deskkin_service_notice(void);
extern uint8_t deskkin_service_valid_result(void);
extern uint32_t deskkin_service_result_attempt(void);
extern uint16_t deskkin_nvs_last_failure(void);

void deskkin_flash_guard_enter(void)
{
	k_mutex_lock(&appcpu_flash_mutex, K_FOREVER);
	if (appcpu_running) {
		esp_cpu_stall(1);
	}
}

void deskkin_flash_guard_exit(void)
{
	if (appcpu_running) {
		esp_cpu_unstall(1);
	}
	k_mutex_unlock(&appcpu_flash_mutex);
}

static void receive_ui_command(void)
{
	struct deskkin_ui_command command;
	const uint32_t before = deskkin_shared_load(&AMP_SHARED->command_publication);
	if (before == 0U || before == command_generation) {
		return;
	}
	deskkin_shared_copy_from(&command, &AMP_SHARED->command, sizeof(command));
	const uint32_t after = deskkin_shared_load(&AMP_SHARED->command_publication);
	if (before != after) {
		return;
	}
	if (command.generation != before || command.schema != DESKKIN_CHANNEL_SCHEMA ||
	    command.command < 1U || command.command > 3U) {
		set_boot_error(9);
		return;
	}
	command_generation = before;
	diagnostic_record(DESKKIN_DIAGNOSTIC_UI_COMMAND, command.command, 0, 0,
			  command.generation);
	deskkin_service_ui_command(command.command);
}

static void publish_touch(void)
{
	touch_generation++;
	const uint32_t index = (touch_generation - 1U) % DESKKIN_TOUCH_CAPACITY;
	const struct deskkin_touch_sample sample = {
		.publication = 0U,
		.generation = touch_generation,
		.x = (int16_t)atomic_get(&touch_x),
		.y = (int16_t)atomic_get(&touch_y),
		.pressed = atomic_get(&touch_pressed) != 0 ? 1U : 0U,
		.schema = DESKKIN_CHANNEL_SCHEMA,
	};
	volatile struct deskkin_touch_sample *slot = &AMP_SHARED->touch.samples[index];
	deskkin_shared_store(&slot->publication, 0U);
	deskkin_shared_copy_to(slot, &sample, sizeof(sample));
	deskkin_shared_store(&slot->publication, touch_generation);
	deskkin_shared_store(&AMP_SHARED->touch.write_generation, touch_generation);
}

static void touch_callback(struct input_event *event, void *user_data)
{
	ARG_UNUSED(user_data);
	if (event->code == INPUT_ABS_X) {
		atomic_set(&touch_x, event->value);
	} else if (event->code == INPUT_ABS_Y) {
		atomic_set(&touch_y, event->value);
	} else if (event->code == INPUT_BTN_TOUCH) {
		atomic_set(&touch_pressed, event->value != 0 ? 1 : 0);
	}
	if (event->sync) {
		publish_touch();
		diagnostic_record(DESKKIN_DIAGNOSTIC_TOUCH,
				  atomic_get(&touch_pressed) != 0 ? 1U : 0U,
				  (int16_t)atomic_get(&touch_x), (int16_t)atomic_get(&touch_y),
				  touch_generation);
	}
}
INPUT_CALLBACK_DEFINE(touch, touch_callback, NULL);

static void publish_world_snapshot(void)
{
	static uint8_t last_diagnostic_shell = UINT8_MAX;
	bool benchmark = atomic_get(&world_benchmark_active) != 0;
	const bool valid_result = deskkin_service_valid_result() != 0U;
	const uint32_t result_attempt = deskkin_service_result_attempt();
	const bool benchmark_completes =
		benchmark && world_generation + 1U - world_benchmark_started_generation >= 1200U;
	world_generation++;
	deskkin_shared_store(&AMP_SHARED->world_publication, 0U);
	const struct deskkin_world_snapshot snapshot = {
		.magic = DESKKIN_WORLD_MAGIC,
		.generation = world_generation,
		.observed_yaw = observed_yaw,
		.sas = deskkin_service_sas(),
		.schema = DESKKIN_WORLD_SCHEMA,
		.shell = deskkin_service_shell(),
		.availability = benchmark ? 2U : deskkin_service_availability(),
		.notice = benchmark ? 1U : deskkin_service_notice(),
	};
	if (snapshot.shell != last_diagnostic_shell) {
		last_diagnostic_shell = snapshot.shell;
		diagnostic_record(DESKKIN_DIAGNOSTIC_SHELL, snapshot.shell, 0, 0,
				  snapshot.generation);
	}
	deskkin_shared_copy_to(&AMP_SHARED->world, &snapshot, sizeof(snapshot));
	deskkin_shared_store(&AMP_SHARED->world_publication, world_generation);
	if (valid_result &&
	    (!valid_snapshot_published || result_attempt != valid_snapshot_attempt)) {
		atomic_set(&valid_view_generation, (atomic_val_t)world_generation);
		valid_snapshot_published = true;
		valid_snapshot_attempt = result_attempt;
	} else if (!valid_result) {
		atomic_set(&valid_view_generation, 0);
		valid_snapshot_published = false;
		valid_snapshot_attempt = 0U;
	}
	if (benchmark_completes) {
		atomic_set(&world_benchmark_active, 0);
		atomic_set(&world_benchmark_complete, 1);
	}
}

int deskkin_amp_world_benchmark_start(void)
{
	if (deskkin_service_shell() != DESKKIN_SHELL_PAIRED) {
		return -EACCES;
	}
	target_yaw = observed_yaw + 65536;
	world_benchmark_started_generation = world_generation;
	atomic_set(&world_benchmark_complete, 0);
	atomic_set(&world_benchmark_active, 1);
	return 0;
}

static void update_observed_yaw(void)
{
	struct deskkin_target_yaw target;
	const uint32_t before = deskkin_shared_load(&AMP_SHARED->target_yaw_publication);
	if (before != 0U) {
		deskkin_shared_copy_from(&target, &AMP_SHARED->target_yaw, sizeof(target));
		const uint32_t after = deskkin_shared_load(&AMP_SHARED->target_yaw_publication);
		if (before == after && target.generation == before &&
		    target.schema == DESKKIN_CHANNEL_SCHEMA) {
			target_yaw = target.value;
		} else if (before == after) {
			set_boot_error(9);
		}
	}
	/* Preserve the exact 0.5 turn/s bound across the 1 kHz supervisor ticks. */
	const uint32_t numerator = (uint32_t)observed_rate_remainder + 32768U;
	const int64_t maximum_step = (int64_t)(numerator / 1000U);
	observed_rate_remainder = (uint16_t)(numerator % 1000U);
	const int64_t difference = target_yaw - observed_yaw;
	const int64_t step = difference > maximum_step ? maximum_step :
			     difference < -maximum_step ? -maximum_step : difference;
	observed_yaw += step;
}

static struct z_thread_stack_element EXT_RAM_NOINIT_ATTR __aligned(ARCH_STACK_PTR_ALIGN)
	supervisor_stack[K_THREAD_STACK_LEN(2048)];
static struct k_thread supervisor_thread;
static struct k_thread wifi_boot_thread;
struct z_thread_stack_element __attribute__((section(".sram2.noinit")))
	__aligned(ARCH_STACK_PTR_ALIGN)
	deskkin_control_stack[K_THREAD_STACK_LEN(3072)];

extern int esp_appcpu_init(void);
extern int esp32_wifi_runtime_init(void);
extern int deskkin_amp_prepare_renderer(void);
extern int deskkin_start_control_worker(void);
extern int deskkin_start_service_after_runtime_handoff(void);
extern void deskkin_amp_service_failed(void);
extern struct k_thread z_main_thread;
#define WIFI_BOOT_STACK_SIZE 1536U
BUILD_ASSERT(WIFI_BOOT_STACK_SIZE <= DESKKIN_SERVICE_STACK_SIZE);
static struct shared_multi_heap_region runtime_sram_regions[3];
static atomic_t runtime_sram_ready;
static uintptr_t wifi_boot_stack_start;
static int wifi_boot_result;
bool deskkin_runtime_internal_owns(const void *block);

BUILD_ASSERT(DT_REG_SIZE(DT_NODELABEL(shm0)) == DESKKIN_SHARED_SIZE,
	     "AMP shared SRAM must match the bounded wire contract");
BUILD_ASSERT(DT_REG_ADDR(DT_NODELABEL(shm0)) == 0x3fcee400U,
	     "AMP shared SRAM must retain the proven APPCPU layout anchor");
BUILD_ASSERT((DT_REG_ADDR(DT_NODELABEL(shm0)) + DESKKIN_CHANNEL_OFFSET +
	      sizeof(struct deskkin_amp_shared)) <= 0x3fcf0000U,
	     "AMP channels must end before the cache-reserved SRAM2 range");

static int initialize_runtime_sram(void)
{
	const int join_result = k_thread_join(&z_main_thread, K_FOREVER);
	if (join_result != 0) {
		return join_result;
	}

	size_t main_unused = 0U;
	const int stack_result = k_thread_stack_space_get(&z_main_thread, &main_unused);
	if (stack_result != 0) {
		return stack_result;
	}
	const uintptr_t main_start = (uintptr_t)z_main_thread.stack_info.start;
	const size_t main_size = z_main_thread.stack_info.size;
	const uintptr_t shared_start = DT_REG_ADDR(DT_NODELABEL(shm0));
	const size_t prefix_size = DESKKIN_CHANNEL_OFFSET;
	if (main_start == 0U || main_size == 0U || prefix_size == 0U ||
	    (main_start % sizeof(uintptr_t)) != 0U ||
	    (shared_start % sizeof(uintptr_t)) != 0U) {
		return -EINVAL;
	}
	diagnostic_record(DESKKIN_DIAGNOSTIC_MEMORY, 1U, 0, 0,
			  (uint32_t)(main_size - MIN(main_unused, main_size)));
	memset((void *)main_start, 0, main_size);
	memset((void *)shared_start, 0, prefix_size);
	struct deskkin_runtime_sram_handoff app_handoff = {0};
	const int64_t handoff_deadline = k_uptime_get() + 5000;
	for (;;) {
		const uint32_t before =
			deskkin_shared_load(&AMP_SHARED->runtime_sram_publication);
		if (before != 0U) {
			deskkin_shared_copy_from(&app_handoff, &AMP_SHARED->runtime_sram,
						 sizeof(app_handoff));
			const uint32_t after =
				deskkin_shared_load(&AMP_SHARED->runtime_sram_publication);
			if (before == after && app_handoff.generation == before &&
			    app_handoff.magic == DESKKIN_RUNTIME_SRAM_MAGIC) {
				break;
			}
		}
		if (k_uptime_get() >= handoff_deadline) {
			diagnostic_record(DESKKIN_DIAGNOSTIC_MEMORY, 5U, 0, 0,
					  *(volatile uint32_t *)(shared_start + prefix_size - 16U));
			return -ETIMEDOUT;
		}
		k_msleep(1);
	}
	const uintptr_t app_start = app_handoff.address;
	const size_t app_size = app_handoff.size;
	if (app_start <= main_start + main_size || app_size == 0U ||
	    app_start + app_size < app_start || app_start + app_size > shared_start ||
	    (app_start % sizeof(uintptr_t)) != 0U) {
		return -EINVAL;
	}
	runtime_sram_regions[0] = (struct shared_multi_heap_region){
		.attr = SMH_REG_ATTR_CACHEABLE,
		.addr = shared_start,
		.size = prefix_size,
	};
	wifi_boot_stack_start = (uintptr_t)service_stack;
	runtime_sram_regions[1] = (struct shared_multi_heap_region){
		.attr = SMH_REG_ATTR_CACHEABLE,
		.addr = main_start,
		.size = main_size,
	};
	runtime_sram_regions[2] = (struct shared_multi_heap_region){
		.attr = SMH_REG_ATTR_CACHEABLE,
		.addr = app_start,
		.size = app_size,
	};
	for (size_t index = 0U; index < 3U; ++index) {
		const int result = shared_multi_heap_add(&runtime_sram_regions[index], NULL);
		if (result != 0) {
			return result;
		}
	}
	atomic_set(&runtime_sram_ready, 1);
	diagnostic_record(DESKKIN_DIAGNOSTIC_MEMORY, 2U, (int16_t)prefix_size,
			  (int16_t)main_size,
			  (uint32_t)(prefix_size + main_size + app_size));
	diagnostic_record(DESKKIN_DIAGNOSTIC_MEMORY, 4U, 0, 0, app_handoff.used);
	return 0;
}

static void wifi_boot_entry(void *first, void *second, void *third)
{
	ARG_UNUSED(first);
	ARG_UNUSED(second);
	ARG_UNUSED(third);
	wifi_boot_result = esp32_wifi_runtime_init();
}

static int complete_wifi_boot_phase(void)
{
	k_tid_t thread =
		k_thread_create(&wifi_boot_thread, (k_thread_stack_t *)wifi_boot_stack_start,
				WIFI_BOOT_STACK_SIZE, wifi_boot_entry, NULL, NULL, NULL, 2, 0,
				K_NO_WAIT);
	if (thread == NULL) {
		memset((void *)wifi_boot_stack_start, 0, WIFI_BOOT_STACK_SIZE);
		return -EIO;
	}
	const int join_result = k_thread_join(&wifi_boot_thread, K_FOREVER);
	memset((void *)wifi_boot_stack_start, 0, WIFI_BOOT_STACK_SIZE);
	if (join_result != 0 || wifi_boot_result != 0) {
		return join_result != 0 ? join_result : wifi_boot_result;
	}
	return 0;
}

void *deskkin_runtime_internal_calloc(size_t count, size_t size)
{
	size_t bytes;
	if (__builtin_mul_overflow(count, size, &bytes) || bytes == 0U) {
		return NULL;
	}
	void *const block = atomic_get(&runtime_sram_ready) != 0
				    ? shared_multi_heap_alloc(SMH_REG_ATTR_CACHEABLE, bytes)
				    : k_calloc(1U, bytes);
	if (block != NULL) {
		memset(block, 0, bytes);
	} else if (atomic_get(&runtime_sram_ready) != 0) {
		diagnostic_record(DESKKIN_DIAGNOSTIC_MEMORY, 3U, 0, 0, (uint32_t)bytes);
	}
	return block;
}

void deskkin_runtime_internal_free(void *block)
{
	if (block == NULL) {
		return;
	}
	if (deskkin_runtime_internal_owns(block)) {
		shared_multi_heap_free(block);
	} else {
		k_free(block);
	}
}

bool deskkin_runtime_internal_owns(const void *block)
{
	if (block == NULL || atomic_get(&runtime_sram_ready) == 0) {
		return false;
	}
	const uintptr_t address = (uintptr_t)block;
	for (size_t index = 0U; index < ARRAY_SIZE(runtime_sram_regions); ++index) {
		const uintptr_t start = runtime_sram_regions[index].addr;
		if (address >= start && address < start + runtime_sram_regions[index].size) {
			return true;
		}
	}
	return false;
}

static void receive_heartbeat(void)
{
	static uint32_t received_publication;
	static uint32_t recorded_stage_mask;
	struct deskkin_renderer_heartbeat heartbeat = {0};
	uint32_t publication = 0U;
	bool stable = false;
	for (size_t attempt = 0; attempt < 3U; ++attempt) {
		publication = deskkin_shared_load(&AMP_SHARED->renderer_publication);
		if (publication == 0U || publication == received_publication) {
			return;
		}
		deskkin_shared_copy_from(&heartbeat, &AMP_SHARED->renderer, sizeof(heartbeat));
		const uint32_t after = deskkin_shared_load(&AMP_SHARED->renderer_publication);
		if (publication != after) {
			continue;
		}
		if (heartbeat.magic != DESKKIN_HEARTBEAT_MAGIC ||
		    heartbeat.generation != publication ||
		    heartbeat.schema != DESKKIN_CHANNEL_SCHEMA) {
			set_boot_error(9);
			return;
		}
		stable = true;
		break;
	}
	if (!stable) {
		return;
	}
	received_publication = publication;
	const uint8_t previous_fault = (uint8_t)atomic_get(&renderer_fault);
	atomic_set(&heartbeat_received_ms, (atomic_val_t)k_uptime_get_32());
	atomic_set(&heartbeat_generation, (atomic_val_t)heartbeat.generation);
	atomic_set(&completed_frames, (atomic_val_t)heartbeat.completed_frames);
	atomic_set(&render_us, (atomic_val_t)heartbeat.render_us);
	atomic_set(&transfer_us, (atomic_val_t)heartbeat.transfer_us);
	atomic_set(&copy_us, (atomic_val_t)heartbeat.copy_us);
	atomic_set(&render_max_us, (atomic_val_t)heartbeat.render_max_us);
	atomic_set(&transfer_max_us, (atomic_val_t)heartbeat.transfer_max_us);
	atomic_set(&renderer_stage, heartbeat.stage);
	atomic_set(&renderer_fault, heartbeat.fault);
	const uint32_t stage_bit = heartbeat.stage < 32U ? BIT(heartbeat.stage) : 0U;
	const bool first_stage = stage_bit != 0U && (recorded_stage_mask & stage_bit) == 0U;
	if (first_stage) {
		recorded_stage_mask |= stage_bit;
	}
	if (first_stage || heartbeat.fault != previous_fault) {
		diagnostic_record(DESKKIN_DIAGNOSTIC_RENDERER, heartbeat.stage, 0, 0,
				  heartbeat.fault);
	}
	atomic_set(&allocation_failures, heartbeat.allocation_failures);
	atomic_set(&transfer_failures, heartbeat.transfer_failures);
	atomic_set(&dirty_rect_count, heartbeat.dirty_rect_count);
	atomic_set(&pixel_dma_batches, heartbeat.pixel_dma_batches);
	atomic_set(&dirty_pixels, (atomic_val_t)heartbeat.dirty_pixels);
	atomic_set(&transferred_bytes, (atomic_val_t)heartbeat.transferred_bytes);
	atomic_set(&view_generation, (atomic_val_t)heartbeat.view_generation);
	atomic_set(&pose_generation, (atomic_val_t)heartbeat.pose_generation);
	atomic_set(&input_generation, (atomic_val_t)heartbeat.input_generation);
	atomic_set(&stale_snapshots, (atomic_val_t)heartbeat.stale_snapshots);
	atomic_set(&touch_drops, (atomic_val_t)heartbeat.touch_drops);
	atomic_set(&atlas_cache_hits, heartbeat.atlas_cache_hits);
	atomic_set(&atlas_cache_misses, heartbeat.atlas_cache_misses);
	atomic_set(&atlas_cache_failures, heartbeat.atlas_cache_failures);
	atomic_set(&visible_billboards, heartbeat.visible_billboards);
	atomic_set(&culled_billboards, heartbeat.culled_billboards);
	atomic_set(&renderer_shell, heartbeat.observed_shell);
	atomic_set(&renderer_shell_property_matches, heartbeat.shell_property_matches);
	atomic_set(&nearest_samples, (atomic_val_t)heartbeat.nearest_samples);
	atomic_set(&bilinear_samples, (atomic_val_t)heartbeat.bilinear_samples);
	atomic_set(&projection_us, (atomic_val_t)heartbeat.projection_us);
	atomic_set(&projection_max_us, (atomic_val_t)heartbeat.projection_max_us);
	atomic_set(&sort_us, (atomic_val_t)heartbeat.sort_us);
	atomic_set(&sort_max_us, (atomic_val_t)heartbeat.sort_max_us);
	atomic_set(&texture_us, (atomic_val_t)heartbeat.texture_us);
	atomic_set(&texture_max_us, (atomic_val_t)heartbeat.texture_max_us);
	atomic_set(&world_raster_us, (atomic_val_t)heartbeat.world_raster_us);
	atomic_set(&world_raster_max_us, (atomic_val_t)heartbeat.world_raster_max_us);
	atomic_set(&deadline_misses, (atomic_val_t)heartbeat.deadline_misses);
}

static void observe_renderer_stale(bool *reported)
{
	const uint32_t heartbeat = (uint32_t)atomic_get(&heartbeat_generation);
	const uint32_t received_ms = (uint32_t)atomic_get(&heartbeat_received_ms);
	const uint32_t now = k_uptime_get_32();
	const bool fresh = heartbeat != 0U && now - received_ms <= HEARTBEAT_STALE_MS;
	if (fresh) {
		*reported = false;
		return;
	}
	if (heartbeat == 0U || *reported) {
		return;
	}
	const uint32_t renderer = deskkin_shared_load(&AMP_SHARED->renderer_progress);
	const uint32_t display = deskkin_shared_load(&AMP_SHARED->display_progress);
	const uint32_t sequences = (((renderer >> 8U) & 0xffffU) << 16U) |
				   ((display >> 8U) & 0xffffU);
	diagnostic_record(DESKKIN_DIAGNOSTIC_RENDERER, 0x80U,
			  (int16_t)(renderer & 0xffU), (int16_t)(display & 0xffU),
			  sequences);
	*reported = true;
}

static int initialize_display_power(void)
{
	if (!device_is_ready(lcd_reset_gpio) || !device_is_ready(lcd_supply)) {
		return -ENODEV;
	}
	int result = regulator_enable(lcd_supply);
	if (result != 0 && result != -EALREADY) {
		return result;
	}
	result = gpio_pin_configure(lcd_reset_gpio, 9, GPIO_OUTPUT_LOW);
	if (result != 0) {
		return result;
	}
	k_msleep(20);
	result = gpio_pin_set_raw(lcd_reset_gpio, 9, 1);
	k_msleep(120);
	return result;
}

static void publish_display_ready(void)
{
	const struct deskkin_display_ready message = {
		.magic = DESKKIN_DISPLAY_MAGIC,
		.generation = 1U,
		.ready = 1U,
		.framebuffer = (uint32_t)(uintptr_t)internal_framebuffer,
		.renderer_heap = (uint32_t)renderer_heap,
		.renderer_heap_size = (uint32_t)renderer_heap_size,
	};
	deskkin_shared_copy_to(&AMP_SHARED->display, &message, sizeof(message));
	deskkin_shared_store(&AMP_SHARED->display_publication, 1U);
}

static void supervisor_entry(void *first, void *second, void *third)
{
	ARG_UNUSED(first);
	ARG_UNUSED(second);
	ARG_UNUSED(third);
	int result = initialize_runtime_sram();
	if (result != 0) {
		diagnostic_record(DESKKIN_DIAGNOSTIC_MEMORY, 0x80U, 0, 0, (uint32_t)result);
		set_boot_error(10);
		return;
	}
	set_boot_stage(10);
	result = complete_wifi_boot_phase();
	if (result != 0) {
		diagnostic_record(DESKKIN_DIAGNOSTIC_MEMORY, 0x81U, 0, 0, (uint32_t)result);
		set_boot_error(11);
		return;
	}
	set_boot_stage(11);
	if (deskkin_start_service_after_runtime_handoff() != 0) {
		deskkin_amp_service_failed();
		return;
	}
	set_boot_stage(8);
	uint32_t next_world_ms = k_uptime_get_32();
	bool renderer_stale_reported = false;
	for (;;) {
		receive_heartbeat();
		observe_renderer_stale(&renderer_stale_reported);
		receive_ui_command();
		update_observed_yaw();
		const uint32_t now = k_uptime_get_32();
		if ((int32_t)(now - next_world_ms) >= 0) {
			publish_world_snapshot();
			set_boot_stage(9);
			next_world_ms += 50U;
			if ((int32_t)(now - next_world_ms) >= 0) {
				next_world_ms = now + 50U;
			}
		}
		if (atomic_get(&display_ready) != 0 &&
		    deskkin_shared_load(&AMP_SHARED->display_publication) == 0U) {
			publish_display_ready();
		}
		k_msleep(1);
	}
}

int deskkin_amp_prepare_renderer(void)
{
	set_boot_stage(1);
	intptr_t mapped_heap = 0;
	size_t mapped_heap_size = 0;
	if (esp_psram_get_mapped_region(&mapped_heap, &mapped_heap_size) != 0 ||
	    mapped_heap == 0 || mapped_heap_size < RENDERER_HEAP_SIZE) {
		set_boot_error(5);
		return -ENOMEM;
	}
	renderer_heap_size = RENDERER_HEAP_SIZE;
	renderer_heap = (uintptr_t)mapped_heap + mapped_heap_size - renderer_heap_size;
	if (renderer_heap <= (uintptr_t)mapped_heap) {
		set_boot_error(5);
		return -ENOMEM;
	}
	const cache_bus_mask_t appcpu_bus =
		cache_ll_l1_get_bus(1, (uint32_t)renderer_heap, renderer_heap_size);
	cache_ll_l1_enable_bus(1, appcpu_bus);
	memset((void *)AMP_SHARED, 0, sizeof(*AMP_SHARED));
	set_boot_stage(2);
	if (initialize_display_power() != 0) {
		set_boot_error(4);
		return -EIO;
	}
	atomic_set(&display_ready, 1);
	publish_display_ready();
	set_boot_stage(3);
	const int appcpu_result = esp_appcpu_init();
	if (appcpu_result == 0) {
		appcpu_running = true;
		esp_cpu_unstall(1);
	}
	if (appcpu_result != 0) {
		set_boot_error(3);
		return -EIO;
	}
	const int64_t renderer_deadline = k_uptime_get() + 5000;
	while (deskkin_shared_load(&AMP_SHARED->display_spi_hz) == 0U &&
	       k_uptime_get() < renderer_deadline) {
		receive_heartbeat();
		if (atomic_get(&renderer_stage) == 5) {
			set_boot_error(7);
			return -EIO;
		}
		k_msleep(1);
	}
	if (deskkin_shared_load(&AMP_SHARED->display_spi_hz) == 0U) {
		diagnostic_record(DESKKIN_DIAGNOSTIC_MEMORY, 5U, 0, 0,
				  (uint32_t)atomic_get(&renderer_stage));
		set_boot_error(7);
		return -ETIMEDOUT;
	}
	set_boot_stage(4);
	return 0;
}

size_t deskkin_amp_status_snapshot(const uint8_t *command_id, uint8_t *response)
{
	if (command_id == NULL || response == NULL) {
		return 0U;
	}
	memset(response, 0, STATUS_RESPONSE_SIZE);
	response[0] = 1;
	memcpy(&response[2], command_id, 16);
	const uint32_t generation = (uint32_t)atomic_get(&heartbeat_generation);
	const uint32_t received_ms = (uint32_t)atomic_get(&heartbeat_received_ms);
	sys_put_be32(generation, &response[28]);
	sys_put_be64(received_ms, &response[32]);
	sys_put_be32((uint32_t)atomic_get(&completed_frames), &response[40]);
	sys_put_be32((uint32_t)atomic_get(&render_us), &response[44]);
	sys_put_be32((uint32_t)atomic_get(&transfer_us), &response[48]);
	response[52] = (uint8_t)atomic_get(&renderer_stage);
	response[53] = (uint8_t)atomic_get(&renderer_fault);
	response[54] = (uint8_t)atomic_get(&allocation_failures);
	response[55] = (uint8_t)atomic_get(&transfer_failures);
	response[56] = (uint8_t)atomic_get(&display_ready);
	sys_put_be32((uint32_t)atomic_get(&render_max_us), &response[57]);
	sys_put_be32((uint32_t)atomic_get(&transfer_max_us), &response[61]);
	const uint16_t nvs_failure = deskkin_nvs_last_failure();
	response[65] = (uint8_t)(nvs_failure >> 8);
	response[66] = (uint8_t)nvs_failure;
	response[67] = deskkin_shared_load(&AMP_SHARED->renderer_publication) != 0U ? 1U : 0U;
	response[68] = (uint8_t)atomic_get(&boot_stage);
	response[69] = (uint8_t)atomic_get(&boot_error);
	sys_put_be32(deskkin_shared_load(&AMP_SHARED->display_spi_hz), &response[70]);
	sys_put_be32((uint32_t)atomic_get(&copy_us), &response[74]);
	const uint32_t now = k_uptime_get_32();
	response[27] = generation != 0U && now - received_ms <= HEARTBEAT_STALE_MS ? 1U : 2U;
	response[78] = (uint8_t)(((uint32_t)atomic_get(&deadline_misses) >> 8) & 0xffU);
	response[79] = (uint8_t)((uint32_t)atomic_get(&deadline_misses) & 0xffU);
	response[80] = (uint8_t)atomic_get(&dirty_rect_count);
	response[81] = atomic_get(&world_benchmark_active) != 0 ? 1U :
		       atomic_get(&world_benchmark_complete) != 0 ? 2U : 0U;
	sys_put_be16((uint16_t)atomic_get(&pixel_dma_batches), &response[82]);
	uint32_t benchmark_updates = 0U;
	if (response[81] != 0U) {
		benchmark_updates = MIN(world_generation - world_benchmark_started_generation, 1200U);
	}
	sys_put_be32(benchmark_updates, &response[84]);
	sys_put_be32((uint32_t)atomic_get(&valid_view_generation), &response[88]);
	sys_put_be32((uint32_t)atomic_get(&view_generation), &response[92]);
	sys_put_be32((uint32_t)atomic_get(&pose_generation), &response[96]);
	sys_put_be32((uint32_t)atomic_get(&input_generation), &response[100]);
	sys_put_be32((uint32_t)atomic_get(&stale_snapshots), &response[104]);
	sys_put_be32((uint32_t)atomic_get(&touch_drops), &response[108]);
	sys_put_be16((uint16_t)atomic_get(&atlas_cache_hits), &response[112]);
	sys_put_be16((uint16_t)atomic_get(&atlas_cache_misses), &response[114]);
	sys_put_be16((uint16_t)atomic_get(&atlas_cache_failures), &response[116]);
	if (deskkin_service_shell() == DESKKIN_SHELL_PAIRED) {
		response[118] = (uint8_t)atomic_get(&visible_billboards);
		response[119] = (uint8_t)atomic_get(&culled_billboards);
	} else {
		response[118] = (uint8_t)atomic_get(&renderer_shell);
		response[119] = (uint8_t)atomic_get(&renderer_shell_property_matches);
	}
	sys_put_be32((uint32_t)atomic_get(&nearest_samples), &response[120]);
	sys_put_be32((uint32_t)atomic_get(&bilinear_samples), &response[124]);
	sys_put_be32((uint32_t)atomic_get(&projection_us), &response[128]);
	sys_put_be32((uint32_t)atomic_get(&projection_max_us), &response[132]);
	sys_put_be32((uint32_t)atomic_get(&sort_us), &response[136]);
	sys_put_be32((uint32_t)atomic_get(&sort_max_us), &response[140]);
	sys_put_be32((uint32_t)atomic_get(&texture_us), &response[144]);
	sys_put_be32((uint32_t)atomic_get(&texture_max_us), &response[148]);
	sys_put_be32((uint32_t)atomic_get(&world_raster_us), &response[152]);
	sys_put_be32((uint32_t)atomic_get(&world_raster_max_us), &response[156]);
	sys_put_be32(deskkin_shared_load(&AMP_SHARED->renderer_progress), &response[160]);
	sys_put_be32(deskkin_shared_load(&AMP_SHARED->display_progress), &response[164]);
	return STATUS_RESPONSE_SIZE;
}

void deskkin_amp_service_failed(void)
{
	set_boot_error(6);
}

void deskkin_amp_supervisor_main(void)
{
	if (!device_is_ready(console) || !device_is_ready(touch) || !appcpu_running ||
	    atomic_get(&display_ready) == 0 || atomic_get(&boot_error) != 0 ||
	    deskkin_shared_load(&AMP_SHARED->display_spi_hz) == 0U) {
		return;
	}
	k_thread_create(&supervisor_thread, supervisor_stack, K_THREAD_STACK_SIZEOF(supervisor_stack),
			supervisor_entry, NULL, NULL, NULL, 3, 0, K_NO_WAIT);
}
