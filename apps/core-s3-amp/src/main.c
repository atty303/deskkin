// SPDX-License-Identifier: MIT

#include <errno.h>
#include <stdint.h>
#include <string.h>
#include <esp_cpu.h>
#include <esp_psram.h>
#include <hal/cache_ll.h>
#include <zephyr/device.h>
#include <zephyr/drivers/gpio.h>
#include <zephyr/input/input.h>
#include <zephyr/drivers/regulator.h>
#include <zephyr/drivers/uart.h>
#include <zephyr/kernel.h>
#include <zephyr/sys/printk.h>
#include <zephyr/sys/byteorder.h>
#include "../shared.h"

#define CONTROL_FRAME_MAX 188
#define STATUS_RESPONSE_SIZE 160
#define HEARTBEAT_STALE_MS 500
#define APPCPU_BOOT_MARKER                                                                        \
	((volatile uint32_t *)(DT_REG_ADDR(DT_NODELABEL(shm0)) + DESKKIN_BOOT_MARKER_OFFSET))
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
static uintptr_t renderer_framebuffer;
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

extern void deskkin_service_ui_command(uint8_t command);
extern uint8_t deskkin_service_shell(void);
extern uint32_t deskkin_service_sas(void);
extern uint8_t deskkin_service_availability(void);
extern uint8_t deskkin_service_notice(void);
extern uint8_t deskkin_service_valid_result(void);
extern uint32_t deskkin_service_result_attempt(void);

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
		atomic_set(&boot_error, 9);
		return;
	}
	command_generation = before;
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
	}
}
INPUT_CALLBACK_DEFINE(touch, touch_callback, NULL);

static void publish_world_snapshot(void)
{
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
			atomic_set(&boot_error, 9);
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

K_THREAD_STACK_DEFINE(supervisor_stack, 2048);
static struct k_thread supervisor_thread;
K_THREAD_STACK_DEFINE(boot_stack, 4096);
static struct k_thread boot_thread;

extern int esp_appcpu_init(void);

static void receive_heartbeat(void)
{
	static uint32_t received_publication;
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
			atomic_set(&boot_error, 9);
			return;
		}
		stable = true;
		break;
	}
	if (!stable) {
		return;
	}
	received_publication = publication;
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

static void supervisor_entry(void *first, void *second, void *third)
{
	ARG_UNUSED(first);
	ARG_UNUSED(second);
	ARG_UNUSED(third);
	uint32_t next_world_ms = k_uptime_get_32();
	for (;;) {
		receive_heartbeat();
		receive_ui_command();
		update_observed_yaw();
		const uint32_t now = k_uptime_get_32();
		if ((int32_t)(now - next_world_ms) >= 0) {
			publish_world_snapshot();
			next_world_ms += 50U;
			if ((int32_t)(now - next_world_ms) >= 0) {
				next_world_ms = now + 50U;
			}
		}
		if (atomic_get(&display_ready) != 0 &&
		    deskkin_shared_load(&AMP_SHARED->display_publication) == 0U) {
			const struct deskkin_display_ready message = {
				.magic = DESKKIN_DISPLAY_MAGIC,
				.generation = 1U,
				.ready = 1U,
				.framebuffer = (uint32_t)renderer_framebuffer,
				.renderer_heap = (uint32_t)renderer_heap,
				.renderer_heap_size = (uint32_t)renderer_heap_size,
			};
			deskkin_shared_copy_to(&AMP_SHARED->display, &message, sizeof(message));
			deskkin_shared_store(&AMP_SHARED->display_publication, 1U);
		}
		k_msleep(1);
	}
}

void deskkin_amp_boot_trace(uint8_t stage)
{
	printk("deskkin_amp:%u\n", stage);
}

static void boot_entry(void *first, void *second, void *third)
{
	ARG_UNUSED(first);
	ARG_UNUSED(second);
	ARG_UNUSED(third);
	atomic_set(&boot_stage, 1);
	atomic_set(&boot_stage, 2);
	atomic_set(&boot_stage, 3);
	k_mutex_lock(&appcpu_flash_mutex, K_FOREVER);
	if (esp_appcpu_init() != 0) {
		k_mutex_unlock(&appcpu_flash_mutex);
		atomic_set(&boot_error, 3);
		return;
	}
	appcpu_running = true;
	k_mutex_unlock(&appcpu_flash_mutex);
	atomic_set(&boot_stage, 4);
	if (initialize_display_power() == 0) {
		atomic_set(&display_ready, 1);
	} else {
		atomic_set(&boot_error, 4);
		return;
	}
	atomic_set(&boot_stage, 5);
	atomic_set(&boot_stage, 9);
}

static int read_byte(uint8_t *byte, int64_t deadline)
{
	while (k_uptime_get() < deadline) {
		if (uart_poll_in(console, byte) == 0) {
			return 0;
		}
		k_msleep(1);
	}
	return -ETIMEDOUT;
}

static int read_status_request(uint8_t *frame)
{
	uint8_t prefix[3] = {0};
	size_t prefix_length = 0;
	const int64_t deadline = k_uptime_get() + 2000;
	while (k_uptime_get() < deadline) {
		uint8_t byte;
		if (read_byte(&byte, deadline) != 0) {
			return -ETIMEDOUT;
		}
		if (prefix_length < sizeof(prefix)) {
			prefix[prefix_length++] = byte;
		} else {
			prefix[0] = prefix[1];
			prefix[1] = prefix[2];
			prefix[2] = byte;
		}
		if (prefix_length < sizeof(prefix)) {
			continue;
		}
		const size_t length = sys_get_be16(prefix);
		if (length != 28 || prefix[2] != 1U) {
			continue;
		}
		frame[0] = prefix[2];
		for (size_t index = 1; index < length; ++index) {
			if (read_byte(&frame[index], k_uptime_get() + 2000) != 0) {
				return -ETIMEDOUT;
			}
		}
		return frame[1] == 8U ? 0 : -ENOTSUP;
	}
	return -ETIMEDOUT;
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
	response[65] = (uint8_t)*APPCPU_BOOT_MARKER;
	response[66] = (uint8_t)APPCPU_BOOT_MARKER[1];
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
	response[118] = (uint8_t)atomic_get(&visible_billboards);
	response[119] = (uint8_t)atomic_get(&culled_billboards);
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
	return STATUS_RESPONSE_SIZE;
}

void deskkin_amp_service_failed(void)
{
	atomic_set(&boot_error, 6);
}

void deskkin_amp_supervisor_main(void)
{
	if (!device_is_ready(console) || !device_is_ready(touch)) {
		return;
	}
	intptr_t mapped_heap = 0;
	size_t mapped_heap_size = 0;
	if (esp_psram_get_mapped_region(&mapped_heap, &mapped_heap_size) != 0 ||
	    mapped_heap == 0 || mapped_heap_size / 2U < CONFIG_ESP_SPIRAM_HEAP_SIZE) {
		atomic_set(&boot_error, 5);
		return;
	}
	/* Keep APPCPU allocations disjoint from PROCPU's heap at the low end. */
	renderer_heap_size = CONFIG_ESP_SPIRAM_HEAP_SIZE;
	renderer_heap = (uintptr_t)mapped_heap + mapped_heap_size - renderer_heap_size;
	const size_t framebuffer_bytes = 2U * 320U * 240U * sizeof(uint16_t);
	if (renderer_heap_size <= framebuffer_bytes) {
		atomic_set(&allocation_failures, 1);
		atomic_set(&boot_error, 5);
		return;
	}
	renderer_framebuffer = renderer_heap;
	renderer_heap += framebuffer_bytes;
	renderer_heap_size -= framebuffer_bytes;
	const cache_bus_mask_t appcpu_bus =
		cache_ll_l1_get_bus(1, (uint32_t)renderer_framebuffer,
				    renderer_heap_size + framebuffer_bytes);
	cache_ll_l1_enable_bus(1, appcpu_bus);
	memset((void *)renderer_framebuffer, 0, framebuffer_bytes);
	memset((void *)AMP_SHARED, 0, sizeof(*AMP_SHARED));
	k_thread_create(&supervisor_thread, supervisor_stack, K_THREAD_STACK_SIZEOF(supervisor_stack),
			supervisor_entry, NULL, NULL, NULL, 3, 0, K_NO_WAIT);
	k_thread_create(&boot_thread, boot_stack, K_THREAD_STACK_SIZEOF(boot_stack), boot_entry, NULL,
			NULL, NULL, 4, 0, K_NO_WAIT);
	for (;;) {
		k_sleep(K_FOREVER);
	}
}
