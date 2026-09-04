// SPDX-License-Identifier: GPL-3.0-only

#include <errno.h>
#include <stdint.h>
#include <string.h>
#include <esp_clk_tree.h>
#include <hal/rtc_timer_ll.h>
#include <soc/clk_tree_defs.h>
#include <soc/spi_struct.h>
#include <rom/ets_sys.h>
#include <zephyr/device.h>
#include <zephyr/drivers/display.h>
#include <zephyr/kernel.h>
#include <zephyr/kernel/thread_stack.h>
#include <zephyr/sys/sys_heap.h>
#include "../../shared.h"

#define DISPLAY_WIDTH 320U
#define DISPLAY_HEIGHT 240U
#define FRAME_PIXELS (DISPLAY_WIDTH * DISPLAY_HEIGHT)
#define PIXEL_DMA_CHUNK_BYTES (4092U * 8U)
#define FULL_FRAME_DMA_BATCHES DIV_ROUND_UP(FRAME_PIXELS * sizeof(uint16_t), PIXEL_DMA_CHUNK_BYTES)
#define FRAMEBUFFER_COUNT 2U
#define MAX_DIRTY_RECTS 3U
#define RENDERER_TIME_SLICE_TICKS 1
#define AMP_SHARED                                                                                 \
	((volatile struct deskkin_amp_shared *)(DT_REG_ADDR(DT_NODELABEL(shm0)) +                  \
					       DESKKIN_CHANNEL_OFFSET))

void deskkin_renderer_entry_probe(void)
{
	const struct deskkin_renderer_heartbeat heartbeat = {
		.magic = DESKKIN_HEARTBEAT_MAGIC,
		.generation = 1U,
		.schema = DESKKIN_CHANNEL_SCHEMA,
		.stage = 6U,
	};
	deskkin_shared_copy_to(&AMP_SHARED->renderer, &heartbeat, sizeof(heartbeat));
	deskkin_shared_store(&AMP_SHARED->renderer_publication, heartbeat.generation);
}

static inline atomic_val_t renderer_counter_get(const atomic_t *counter)
{
	return *(const volatile atomic_val_t *)counter;
}

static inline void renderer_counter_set(atomic_t *counter, atomic_val_t value)
{
	*(volatile atomic_val_t *)counter = value;
}

static inline atomic_val_t renderer_counter_inc(atomic_t *counter)
{
	const atomic_val_t previous = renderer_counter_get(counter);
	renderer_counter_set(counter, previous + 1);
	return previous;
}

static inline bool renderer_counter_cas(atomic_t *counter, atomic_val_t old_value,
					atomic_val_t new_value)
{
	if (renderer_counter_get(counter) != old_value) {
		return false;
	}
	renderer_counter_set(counter, new_value);
	return true;
}

#define atomic_get renderer_counter_get
#define atomic_set renderer_counter_set
#define atomic_inc renderer_counter_inc
#define atomic_cas renderer_counter_cas

enum renderer_stage {
	RENDERER_WAITING_FOR_DISPLAY = 1,
	RENDERER_RENDERING = 2,
	RENDERER_TRANSFERRING = 3,
	RENDERER_PRESENTED = 4,
	RENDERER_FAILED = 5,
	RENDERER_ENTERED = 6,
	RENDERER_APPCPU_STARTED = 7,
	RENDERER_INITIALIZING_HEAP = 8,
	RENDERER_INITIALIZING_DISPLAY = 9,
	RENDERER_STARTING_THREADS = 10,
	RENDERER_INITIALIZING_DMA = 11,
	RENDERER_INITIALIZING_SPI = 12,
	RENDERER_INITIALIZING_MIPI_DBI = 13,
	RENDERER_INITIALIZING_PANEL = 14,
};

enum renderer_fault {
	RENDERER_FAULT_NONE = 0,
	RENDERER_FAULT_HEAP_EXHAUSTED = 11,
	RENDERER_FAULT_DISPLAY_INIT = 12,
	RENDERER_FAULT_HEAP_INIT = 13,
	RENDERER_FAULT_DMA_INIT = 14,
	RENDERER_FAULT_SPI_INIT = 15,
	RENDERER_FAULT_MIPI_DBI_INIT = 16,
};

struct deskkin_dirty_rect {
	uint16_t x;
	uint16_t y;
	uint16_t width;
	uint16_t height;
};

struct display_request {
	uint8_t buffer_index;
	uint8_t dirty_rect_count;
	struct deskkin_dirty_rect dirty_rects[MAX_DIRTY_RECTS];
};

struct display_completion {
	uint8_t buffer_index;
	int8_t result;
	uint32_t duration_us;
};

K_MSGQ_DEFINE(display_requests, sizeof(struct display_request), 2, 4);
K_MSGQ_DEFINE(display_completions, sizeof(struct display_completion), 2, 4);
static k_thread_stack_t *display_stack;
static struct k_thread display_thread;
static k_thread_stack_t *renderer_stack;
static struct k_thread renderer_thread;
extern struct k_thread z_main_thread;
K_THREAD_STACK_DECLARE(z_main_stack, CONFIG_MAIN_STACK_SIZE);

static const struct device *const display = DEVICE_DT_GET(DT_CHOSEN(zephyr_display));
static const struct device *const display_dma = DEVICE_DT_GET(DT_NODELABEL(dma));
static const struct device *const mipi_dbi = DEVICE_DT_GET(DT_NODELABEL(mipi_dbi));
static const struct device *const display_spi = DEVICE_DT_GET(DT_NODELABEL(spi2));
static const struct device *const display_gpio0 = DEVICE_DT_GET(DT_NODELABEL(gpio0));
static const struct device *const display_gpio = DEVICE_DT_GET(DT_NODELABEL(gpio1));
static uint16_t *framebuffer;
static struct sys_heap renderer_heap;
static bool renderer_heap_ready;
static uint32_t generation;
static uint32_t renderer_progress_sequence;
static uint32_t display_progress_sequence;
static atomic_t completed_frames;
static atomic_t allocation_failures;
static atomic_t transfer_failures;
static atomic_t render_max_us;
static atomic_t transfer_max_us;
static atomic_t render_last_us;
static atomic_t transfer_last_us;
static atomic_t copy_last_us;
static atomic_t dirty_rect_count;
static atomic_t pixel_dma_batches;
static atomic_t dirty_pixels;
static atomic_t transferred_bytes;
static atomic_t pixel_transfer_count;
static atomic_t pixel_transfer_last_us;
static atomic_t frame_difference_last;
static atomic_t frame_difference_max;
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
static atomic_t observed_shell;
static atomic_t shell_property_matches;
static atomic_t nearest_samples;
static atomic_t bilinear_samples;
static atomic_t projection_last_us;
static atomic_t projection_max_us;
static atomic_t sort_last_us;
static atomic_t sort_max_us;
static atomic_t texture_last_us;
static atomic_t texture_max_us;
static atomic_t world_raster_last_us;
static atomic_t world_raster_max_us;
static atomic_t deadline_misses;

extern void rust_main(void);

static void progress_store(volatile uint32_t *target, uint32_t *sequence, uint8_t stage)
{
	*sequence = (*sequence + 1U) & 0x00ffffffU;
	if (*sequence == 0U) {
		*sequence = 1U;
	}
	deskkin_shared_store(target, (*sequence << 8U) | stage);
}

void deskkin_renderer_progress(uint8_t stage)
{
	progress_store(&AMP_SHARED->renderer_progress, &renderer_progress_sequence, stage);
}

static void display_progress(uint8_t stage)
{
	progress_store(&AMP_SHARED->display_progress, &display_progress_sequence, stage);
}

void deskkin_renderer_observe(uint8_t stage, uint8_t fault, uint32_t render_us,
			      uint32_t transfer_us);
uint64_t deskkin_uptime_us(void);

static void observe_max(atomic_t *maximum, uint32_t value)
{
	atomic_val_t current = atomic_get(maximum);
	while (value > (uint32_t)current &&
	       !atomic_cas(maximum, current, (atomic_val_t)value)) {
		current = atomic_get(maximum);
	}
}

void deskkin_world_observe(uint32_t generation, uint32_t input, uint32_t drops,
			   uint16_t cache_hits, uint16_t cache_misses, uint16_t cache_failures,
			   uint8_t visible, uint8_t culled, uint32_t nearest,
			   uint32_t bilinear, uint32_t projection_us, uint32_t sort_us,
			   uint32_t texture_us, uint32_t raster_us)
{
	atomic_set(&view_generation, (atomic_val_t)generation);
	atomic_set(&pose_generation, (atomic_val_t)generation);
	atomic_set(&input_generation, (atomic_val_t)input);
	atomic_set(&touch_drops, (atomic_val_t)drops);
	atomic_set(&atlas_cache_hits, cache_hits);
	atomic_set(&atlas_cache_misses, cache_misses);
	atomic_set(&atlas_cache_failures, cache_failures);
	atomic_set(&visible_billboards, visible);
	atomic_set(&culled_billboards, culled);
	atomic_set(&nearest_samples, (atomic_val_t)nearest);
	atomic_set(&bilinear_samples, (atomic_val_t)bilinear);
	atomic_set(&projection_last_us, (atomic_val_t)projection_us);
	atomic_set(&sort_last_us, (atomic_val_t)sort_us);
	atomic_set(&texture_last_us, (atomic_val_t)texture_us);
	atomic_set(&world_raster_last_us, (atomic_val_t)raster_us);
	observe_max(&projection_max_us, projection_us);
	observe_max(&sort_max_us, sort_us);
	observe_max(&texture_max_us, texture_us);
	observe_max(&world_raster_max_us, raster_us);
}

void deskkin_shell_observe(uint8_t shell, uint8_t property_matches)
{
	atomic_set(&observed_shell, shell);
	atomic_set(&shell_property_matches, property_matches);
}

void deskkin_raster_profile(const uint32_t *values)
{
	static uint32_t generation;
	generation++;
	if (generation == 0U) { generation = 1U; }
	deskkin_shared_store(&AMP_SHARED->raster_profile_publication, 0U);
	deskkin_shared_copy_to(AMP_SHARED->raster_profile, values, sizeof(AMP_SHARED->raster_profile));
	deskkin_shared_store(&AMP_SHARED->raster_profile_publication, generation);
}

void *malloc(size_t size)
{
	if (!renderer_heap_ready) {
		return NULL;
	}
	const size_t allocation_size = MAX(size, 1U);
	void *const block = sys_heap_alloc(&renderer_heap, allocation_size);
	if (block == NULL) {
		atomic_inc(&allocation_failures);
		deskkin_renderer_observe(RENDERER_FAILED, RENDERER_FAULT_HEAP_EXHAUSTED, 0, 0);
	}
	return block;
}

void free(void *block)
{
	if (renderer_heap_ready && block != NULL) {
		sys_heap_free(&renderer_heap, block);
	}
}

static int initialize_renderer_heap(void)
{
	const uint32_t address = AMP_SHARED->display.renderer_heap;
	const uint32_t size = AMP_SHARED->display.renderer_heap_size;
	deskkin_shared_store(&AMP_SHARED->renderer.render_us, address);
	deskkin_shared_store(&AMP_SHARED->renderer.transfer_us, size);
	if (address == 0U || size == 0U || (address & 31U) != 0U) {
		return -ENOMEM;
	}
	for (uint32_t offset = 0U; offset < size; offset += 64U * 1024U) {
		volatile uint32_t *const word = (volatile uint32_t *)(uintptr_t)(address + offset);
		const uint32_t expected = 0x5a5a0000U ^ offset;
		*word = expected;
		__asm__ volatile("memw" ::: "memory");
		if (*word != expected) {
			return -EIO;
		}
	}
	volatile uint32_t *const last_word =
		(volatile uint32_t *)(uintptr_t)(address + size - sizeof(uint32_t));
	*last_word = 0xa5a55a5aU;
	__asm__ volatile("memw" ::: "memory");
	if (*last_word != 0xa5a55a5aU) {
		return -EIO;
	}
	sys_heap_init(&renderer_heap, (void *)(uintptr_t)address, size);
	renderer_heap_ready = true;
	return 0;
}

static void initialize_output_only_gpio(const struct device *gpio)
{
	/* PROCPU owns GPIO interrupts. The renderer only uses output operations for
	 * display chip-select and data/command, so core1 must not allocate the SoC's
	 * shared GPIO interrupt while bringing up its local device model. */
	gpio->state->init_res = 0U;
	gpio->state->initialized = true;
}

void deskkin_renderer_observe(uint8_t stage, uint8_t fault, uint32_t render_us,
			      uint32_t transfer_us)
{
	if (stage == RENDERER_PRESENTED) {
		atomic_inc(&completed_frames);
	}
	atomic_set(&render_last_us, (atomic_val_t)render_us);
	atomic_set(&transfer_last_us, (atomic_val_t)transfer_us);
	observe_max(&render_max_us, render_us);
	observe_max(&transfer_max_us, transfer_us);
	generation += 1U;
	if (generation == 0U) {
		generation = 1U;
	}
	const uint64_t copy_started = deskkin_uptime_us();
	deskkin_shared_store(&AMP_SHARED->renderer_publication, 0U);
	volatile struct deskkin_renderer_heartbeat *const heartbeat = &AMP_SHARED->renderer;
	heartbeat->magic = DESKKIN_HEARTBEAT_MAGIC;
	heartbeat->generation = generation;
	heartbeat->completed_frames = (uint32_t)atomic_get(&completed_frames);
	heartbeat->render_us = (uint32_t)atomic_get(&render_last_us);
	heartbeat->transfer_us = (uint32_t)atomic_get(&transfer_last_us);
	heartbeat->copy_us = (uint32_t)atomic_get(&copy_last_us);
	heartbeat->render_max_us = (uint32_t)atomic_get(&render_max_us);
	heartbeat->transfer_max_us = (uint32_t)atomic_get(&transfer_max_us);
	heartbeat->stage = stage;
	heartbeat->fault = fault;
	heartbeat->allocation_failures = (uint8_t)atomic_get(&allocation_failures);
	heartbeat->transfer_failures = (uint8_t)atomic_get(&transfer_failures);
	heartbeat->dirty_rect_count = (uint8_t)atomic_get(&dirty_rect_count);
	heartbeat->schema = DESKKIN_CHANNEL_SCHEMA;
	heartbeat->pixel_dma_batches = (uint16_t)atomic_get(&pixel_dma_batches);
	heartbeat->dirty_pixels = (uint32_t)atomic_get(&dirty_pixels);
	heartbeat->transferred_bytes = (uint32_t)atomic_get(&transferred_bytes);
	heartbeat->view_generation = (uint32_t)atomic_get(&view_generation);
	heartbeat->pose_generation = (uint32_t)atomic_get(&pose_generation);
	heartbeat->input_generation = (uint32_t)atomic_get(&input_generation);
	heartbeat->stale_snapshots = (uint32_t)atomic_get(&stale_snapshots);
	heartbeat->touch_drops = (uint32_t)atomic_get(&touch_drops);
	heartbeat->atlas_cache_hits = (uint16_t)atomic_get(&atlas_cache_hits);
	heartbeat->atlas_cache_misses = (uint16_t)atomic_get(&atlas_cache_misses);
	heartbeat->atlas_cache_failures = (uint16_t)atomic_get(&atlas_cache_failures);
	heartbeat->visible_billboards = (uint8_t)atomic_get(&visible_billboards);
	heartbeat->culled_billboards = (uint8_t)atomic_get(&culled_billboards);
	heartbeat->observed_shell = (uint8_t)atomic_get(&observed_shell);
	heartbeat->shell_property_matches = (uint8_t)atomic_get(&shell_property_matches);
	if ((uint8_t)atomic_get(&observed_shell) == DESKKIN_SHELL_PAIRED) {
		heartbeat->nearest_samples = (uint32_t)atomic_get(&nearest_samples);
		heartbeat->bilinear_samples = (uint32_t)atomic_get(&bilinear_samples);
		heartbeat->projection_us = (uint32_t)atomic_get(&projection_last_us);
		heartbeat->projection_max_us = (uint32_t)atomic_get(&projection_max_us);
	} else {
		heartbeat->nearest_samples = (uint32_t)atomic_get(&pixel_transfer_count);
		heartbeat->bilinear_samples = (uint32_t)atomic_get(&pixel_transfer_last_us);
		heartbeat->projection_us = (uint32_t)atomic_get(&frame_difference_last);
		heartbeat->projection_max_us = (uint32_t)atomic_get(&frame_difference_max);
	}
	heartbeat->sort_us = (uint32_t)atomic_get(&sort_last_us);
	heartbeat->sort_max_us = (uint32_t)atomic_get(&sort_max_us);
	heartbeat->texture_us = (uint32_t)atomic_get(&texture_last_us);
	heartbeat->texture_max_us = (uint32_t)atomic_get(&texture_max_us);
	heartbeat->world_raster_us = (uint32_t)atomic_get(&world_raster_last_us);
	heartbeat->world_raster_max_us = (uint32_t)atomic_get(&world_raster_max_us);
	heartbeat->deadline_misses = (uint32_t)atomic_get(&deadline_misses);
	deskkin_shared_fence();
	deskkin_shared_store(&AMP_SHARED->renderer_publication, generation);
	atomic_set(&copy_last_us, (atomic_val_t)(deskkin_uptime_us() - copy_started));
}

uint64_t deskkin_uptime_us(void)
{
	/* RC_SLOW is approximately 136 kHz. Seven us/tick is a conservative
	 * monotonic approximation used only for renderer pacing and diagnostics.
	 */
	const uint64_t ticks = rtc_timer_ll_get_cycle_count(0);
	return (ticks << 3U) - ticks;
}

void deskkin_sleep_ms(uint32_t delay_ms)
{
	k_busy_wait(delay_ms * 1000U);
}

uint16_t *deskkin_framebuffer_alloc(uint8_t index)
{
	if (index >= FRAMEBUFFER_COUNT) {
		return NULL;
	}
	if (framebuffer == NULL) {
		const uint32_t address = AMP_SHARED->display.framebuffer;
		if (address == 0U || (address & 31U) != 0U) {
			atomic_inc(&allocation_failures);
		} else {
			framebuffer = (uint16_t *)(uintptr_t)address;
		}
	}
	return framebuffer == NULL ? NULL : framebuffer + (size_t)index * FRAME_PIXELS;
}

int deskkin_display_submit(uint8_t buffer_index,
			   const struct deskkin_dirty_rect *dirty_rects,
			   uint8_t dirty_rect_count)
{
	if (framebuffer == NULL || buffer_index >= FRAMEBUFFER_COUNT ||
	    dirty_rect_count > MAX_DIRTY_RECTS ||
	    (dirty_rect_count != 0U && dirty_rects == NULL)) {
		return -EINVAL;
	}
	struct display_request request = {
		.buffer_index = buffer_index,
		.dirty_rect_count = dirty_rect_count,
	};
	for (size_t index = 0; index < dirty_rect_count; ++index) {
		const struct deskkin_dirty_rect rect = dirty_rects[index];
		if (rect.width == 0U || rect.height == 0U || rect.x >= DISPLAY_WIDTH ||
		    rect.y >= DISPLAY_HEIGHT || rect.width > DISPLAY_WIDTH - rect.x ||
		    rect.height > DISPLAY_HEIGHT - rect.y) {
			return -EINVAL;
		}
		request.dirty_rects[index] = rect;
	}
	const int result = k_msgq_put(&display_requests, &request, K_NO_WAIT);
	if (result == 0) {
		k_yield();
	}
	return result;
}

int deskkin_display_take_completion(uint8_t *buffer_index, uint32_t *duration_us)
{
	struct display_completion completion;
	if (k_msgq_get(&display_completions, &completion, K_NO_WAIT) != 0) {
		return 0;
	}
	*buffer_index = completion.buffer_index;
	*duration_us = completion.duration_us;
	if (completion.result != 0) {
		atomic_inc(&transfer_failures);
		return -EIO;
	}
	return 1;
}

void deskkin_yield(void)
{
	k_yield();
}

int deskkin_world_snapshot(struct deskkin_world_snapshot *output)
{
	if (output == NULL) {
		return -EINVAL;
	}
	for (size_t attempt = 0; attempt < 3U; ++attempt) {
		const uint32_t publication = deskkin_shared_load(&AMP_SHARED->world_publication);
		if (publication == 0U) {
			return -EAGAIN;
		}
		deskkin_shared_copy_from(output, &AMP_SHARED->world, sizeof(*output));
		const uint32_t after = deskkin_shared_load(&AMP_SHARED->world_publication);
		if (publication == after && output->generation == publication) {
			if (output->magic != DESKKIN_WORLD_MAGIC ||
			    output->schema != DESKKIN_WORLD_SCHEMA ||
			    output->shell > DESKKIN_SHELL_PAIRED || output->availability > 3U ||
			    output->notice > 1U ||
			    (output->sas != UINT32_MAX && output->sas > 999999U)) {
				return -EPROTO;
			}
			return 1;
		}
	}
	atomic_inc(&stale_snapshots);
	return -EAGAIN;
}

int deskkin_touch_read(uint32_t after_generation, struct deskkin_touch_sample *output,
		       uint32_t *drop_count)
{
	if (output == NULL || drop_count == NULL) {
		return -EINVAL;
	}
	const uint32_t latest = deskkin_shared_load(&AMP_SHARED->touch.write_generation);
	uint32_t cumulative_drops = deskkin_shared_load(&AMP_SHARED->touch.drop_count);
	*drop_count = cumulative_drops;
	if (latest == 0U || latest == after_generation) {
		return 0;
	}
	uint32_t wanted = after_generation + 1U;
	uint32_t skipped = 0U;
	if (latest - wanted >= DESKKIN_TOUCH_CAPACITY) {
		const uint32_t oldest = latest - DESKKIN_TOUCH_CAPACITY + 1U;
		skipped = oldest - wanted;
		wanted = oldest;
	}
	const uint32_t index = (wanted - 1U) % DESKKIN_TOUCH_CAPACITY;
	volatile struct deskkin_touch_sample *slot = &AMP_SHARED->touch.samples[index];
	const uint32_t before = deskkin_shared_load(&slot->publication);
	if (before == 0U || before != wanted) {
		return -EAGAIN;
	}
	deskkin_shared_copy_from(output, slot, sizeof(*output));
	const uint32_t after = deskkin_shared_load(&slot->publication);
	if (before != after) {
		return -EAGAIN;
	}
	if (output->generation != wanted || output->schema != DESKKIN_CHANNEL_SCHEMA ||
	    output->pressed > 1U || output->x < 0 || output->x >= 320 || output->y < 0 ||
	    output->y >= 240) {
		return -EPROTO;
	}
	if (skipped != 0U) {
		cumulative_drops = skipped > UINT32_MAX - cumulative_drops
					   ? UINT32_MAX
					   : cumulative_drops + skipped;
		deskkin_shared_store(&AMP_SHARED->touch.drop_count, cumulative_drops);
		*drop_count = cumulative_drops;
	}
	return 1;
}

void deskkin_publish_target_yaw(int64_t value)
{
	const uint32_t generation = AMP_SHARED->target_yaw.generation + 1U;
	deskkin_shared_store(&AMP_SHARED->target_yaw_publication, 0U);
	const struct deskkin_target_yaw target = {
		.generation = generation == 0U ? 1U : generation,
		.schema = DESKKIN_CHANNEL_SCHEMA,
		.value = value,
	};
	deskkin_shared_copy_to(&AMP_SHARED->target_yaw, &target, sizeof(target));
	deskkin_shared_store(&AMP_SHARED->target_yaw_publication, target.generation);
}

void deskkin_publish_ui_command(uint8_t command)
{
	const uint32_t generation = AMP_SHARED->command.generation + 1U;
	deskkin_shared_store(&AMP_SHARED->command_publication, 0U);
	const struct deskkin_ui_command message = {
		.generation = generation == 0U ? 1U : generation,
		.command = command,
		.schema = DESKKIN_CHANNEL_SCHEMA,
	};
	deskkin_shared_copy_to(&AMP_SHARED->command, &message, sizeof(message));
	deskkin_shared_store(&AMP_SHARED->command_publication, message.generation);
}

void deskkin_deadline_missed(void)
{
	atomic_inc(&deadline_misses);
}

int deskkin_display_enable(void)
{
	const int result = display_blanking_off(display);
	return result == -ENOSYS ? 0 : result;
}

static uint32_t display_spi_frequency_hz(void)
{
	uint32_t source_hz;
	if (esp_clk_tree_src_get_freq_hz(SPI_CLK_SRC_DEFAULT,
					 ESP_CLK_TREE_SRC_FREQ_PRECISION_APPROX, &source_hz) != ESP_OK) {
		return 0U;
	}
	if (GPSPI2.clock.clk_equ_sysclk != 0U) {
		return source_hz;
	}
	return source_hz /
	       ((GPSPI2.clock.clkdiv_pre + 1U) * (GPSPI2.clock.clkcnt_n + 1U));
}

static void display_entry(void *first, void *second, void *third)
{
	ARG_UNUSED(first);
	ARG_UNUSED(second);
	ARG_UNUSED(third);
	uint8_t previous_buffer_index = 0U;
	bool have_previous_buffer = false;
	for (;;) {
		struct display_request request;
		display_progress(DESKKIN_DISPLAY_PROGRESS_WAITING);
		(void)k_msgq_get(&display_requests, &request, K_FOREVER);
		display_progress(DESKKIN_DISPLAY_PROGRESS_REQUEST);
		const uint64_t started = deskkin_uptime_us();
		const uint16_t *pixels =
			framebuffer + (size_t)request.buffer_index * FRAME_PIXELS;
		uint32_t request_pixels = 0U;
		for (size_t index = 0; index < request.dirty_rect_count; ++index) {
			const struct deskkin_dirty_rect *rect = &request.dirty_rects[index];
			request_pixels += (uint32_t)rect->width * rect->height;
		}
		atomic_set(&dirty_rect_count, request.dirty_rect_count);
		if (request.dirty_rect_count != 0U) {
			atomic_set(&pixel_dma_batches, FULL_FRAME_DMA_BATCHES);
		}
		atomic_set(&dirty_pixels, (atomic_val_t)request_pixels);
		atomic_set(&transferred_bytes, request.dirty_rect_count == 0U
						 ? 0
						 : (atomic_val_t)(FRAME_PIXELS * sizeof(uint16_t)));
		int result = 0;
		if (request.dirty_rect_count != 0U) {
			const struct display_buffer_descriptor descriptor = {
				.buf_size = FRAME_PIXELS * sizeof(uint16_t),
				.pitch = DISPLAY_WIDTH,
				.width = DISPLAY_WIDTH,
				.height = DISPLAY_HEIGHT,
			};
			display_progress(DESKKIN_DISPLAY_PROGRESS_TRANSFER);
			result = display_write(display, 0, 0, &descriptor, pixels);
		}
		display_progress(DESKKIN_DISPLAY_PROGRESS_COMPLETION);
		const uint64_t elapsed = deskkin_uptime_us() - started;
		if (request.dirty_rect_count != 0U && result == 0) {
			uint32_t difference = 0U;
			if (have_previous_buffer) {
				const uint16_t *previous = framebuffer +
					(size_t)previous_buffer_index * FRAME_PIXELS;
				for (size_t index = 0; index < FRAME_PIXELS; ++index) {
					difference += pixels[index] != previous[index] ? 1U : 0U;
				}
			}
			previous_buffer_index = request.buffer_index;
			have_previous_buffer = true;
			atomic_set(&frame_difference_last, (atomic_val_t)difference);
			observe_max(&frame_difference_max, difference);
			atomic_inc(&pixel_transfer_count);
			atomic_set(&pixel_transfer_last_us,
				   (atomic_val_t)MIN(elapsed, UINT32_MAX));
		}
		const struct display_completion completion = {
			.buffer_index = request.buffer_index,
			.result = result == 0 ? 0 : -1,
			.duration_us = (uint32_t)MIN(elapsed, UINT32_MAX),
		};
		(void)k_msgq_put(&display_completions, &completion, K_FOREVER);
	}
}

static void renderer_entry(void *first, void *second, void *third)
{
	ARG_UNUSED(first);
	ARG_UNUSED(second);
	ARG_UNUSED(third);
	const int64_t deadline = k_uptime_get() + 5000;
	while ((z_main_thread.base.thread_state & _THREAD_DEAD) == 0U) {
		if (k_uptime_get() >= deadline) {
			deskkin_renderer_observe(RENDERER_FAILED, RENDERER_FAULT_HEAP_INIT, 0, 0);
			return;
		}
		k_msleep(1);
	}
	const uintptr_t main_stack = (uintptr_t)K_KERNEL_STACK_BUFFER(z_main_stack);
	const size_t main_stack_size = K_KERNEL_STACK_SIZEOF(z_main_stack);
	memset((void *)main_stack, 0, main_stack_size);
	const struct deskkin_runtime_sram_handoff handoff = {
		.magic = DESKKIN_RUNTIME_SRAM_MAGIC,
		.generation = 1U,
		.address = (uint32_t)main_stack,
		.size = (uint32_t)main_stack_size,
		.used = 0U,
	};
	deskkin_shared_store(&AMP_SHARED->runtime_sram_publication, 0U);
	deskkin_shared_copy_to(&AMP_SHARED->runtime_sram, &handoff, sizeof(handoff));
	deskkin_shared_store(&AMP_SHARED->runtime_sram_publication, handoff.generation);
	rust_main();
}

int main(void)
{
	deskkin_renderer_observe(RENDERER_APPCPU_STARTED, RENDERER_FAULT_NONE, 0, 0);
	deskkin_renderer_observe(RENDERER_WAITING_FOR_DISPLAY, RENDERER_FAULT_NONE, 0, 0);
	while (deskkin_shared_load(&AMP_SHARED->display_publication) == 0U ||
	       AMP_SHARED->display.magic != DESKKIN_DISPLAY_MAGIC || AMP_SHARED->display.ready != 1U) {
		k_busy_wait(100000U);
	}
	deskkin_renderer_observe(RENDERER_INITIALIZING_HEAP, RENDERER_FAULT_NONE, 0, 0);
	if (initialize_renderer_heap() != 0) {
		atomic_inc(&allocation_failures);
		deskkin_renderer_observe(RENDERER_FAILED, RENDERER_FAULT_HEAP_INIT, 0, 0);
		return 1;
	}
	display_stack = sys_heap_aligned_alloc(&renderer_heap, ARCH_STACK_PTR_ALIGN,
					       K_THREAD_STACK_LEN(4096));
	renderer_stack = sys_heap_aligned_alloc(&renderer_heap, ARCH_STACK_PTR_ALIGN,
					        K_THREAD_STACK_LEN(32768));
	if (display_stack == NULL || renderer_stack == NULL) {
		atomic_inc(&allocation_failures);
		deskkin_renderer_observe(RENDERER_FAILED, RENDERER_FAULT_HEAP_EXHAUSTED, 0, 0);
		return 1;
	}
	initialize_output_only_gpio(display_gpio0);
	initialize_output_only_gpio(display_gpio);
	deskkin_renderer_observe(RENDERER_INITIALIZING_DISPLAY, RENDERER_FAULT_NONE, 0, 0);
	const struct device *const dependencies[] = {
		display_dma,
		display_spi,
		mipi_dbi,
	};
	for (size_t index = 0; index < ARRAY_SIZE(dependencies); ++index) {
		deskkin_renderer_observe((uint8_t)(RENDERER_INITIALIZING_DMA + index),
					 RENDERER_FAULT_NONE, 0, 0);
		const int result = device_init(dependencies[index]);
		if ((result != 0 && result != -EALREADY) ||
		    !device_is_ready(dependencies[index])) {
			const uint8_t fault = (uint8_t)(RENDERER_FAULT_DMA_INIT + index);
			deskkin_renderer_observe(RENDERER_FAILED, fault, 0, 0);
			return 1;
		}
	}
	deskkin_renderer_observe(RENDERER_INITIALIZING_PANEL, RENDERER_FAULT_NONE, 0, 0);
	const int display_result = device_init(display);
	if (display_result == -ENOMEM) {
		atomic_inc(&allocation_failures);
	}
	if (display_result != 0 || !device_is_ready(display)) {
		deskkin_renderer_observe(RENDERER_FAILED, RENDERER_FAULT_DISPLAY_INIT, 0, 0);
		return 1;
	}
	deskkin_shared_store(&AMP_SHARED->display_spi_hz, display_spi_frequency_hz());
	deskkin_renderer_observe(RENDERER_STARTING_THREADS, RENDERER_FAULT_NONE, 0, 0);
	k_tid_t display_tid = k_thread_create(&display_thread, display_stack, 4096,
					    display_entry, NULL, NULL, NULL, 0, 0, K_NO_WAIT);
	k_tid_t renderer_tid = k_thread_create(&renderer_thread, renderer_stack, 32768,
					     renderer_entry, NULL, NULL, NULL, 0, 0, K_FOREVER);
	k_thread_time_slice_set(display_tid, RENDERER_TIME_SLICE_TICKS, NULL, NULL);
	k_thread_time_slice_set(renderer_tid, RENDERER_TIME_SLICE_TICKS, NULL, NULL);
	k_thread_start(renderer_tid);
	return 0;
}

/* The current Zephyr context does not save CP3. Preserve q0 in the leaf and
 * exclude preemption only for this bounded span (at most 40 vector stores). */
extern void deskkin_background_pie(uint16_t *destination, const uint16_t *pattern,
                                  size_t vectors);

void deskkin_background_vectors(uint16_t *destination, const uint16_t *pattern,
                                size_t vectors)
{
    const unsigned int key = irq_lock();
    uint32_t saved;
    __asm__ volatile("rsr.cpenable %0" : "=r"(saved));
    const uint32_t enabled = saved | (1U << 3);
    __asm__ volatile("wsr.cpenable %0; rsync" :: "r"(enabled) : "memory");
    deskkin_background_pie(destination, pattern, vectors);
    __asm__ volatile("wsr.cpenable %0; rsync" :: "r"(saved) : "memory");
    irq_unlock(key);
}

uint32_t deskkin_blit_cycles(void)
{
    uint32_t cycles;
    __asm__ volatile("rsr.ccount %0" : "=r"(cycles));
    return cycles;
}
extern void deskkin_copy_pie(uint16_t *, const uint16_t *, size_t, uint32_t);
void deskkin_copy_vectors(uint16_t *dst, const uint16_t *src, size_t vectors, uint32_t wire)
{
    unsigned int key = irq_lock();
    uint32_t saved;
    __asm__ volatile("rsr.cpenable %0" : "=r"(saved));
    uint32_t enabled = saved | (1U << 3);
    __asm__ volatile("wsr.cpenable %0; rsync" :: "r"(enabled) : "memory");
    deskkin_copy_pie(dst, src, vectors, wire);
    __asm__ volatile("wsr.cpenable %0; rsync" :: "r"(saved) : "memory");
    irq_unlock(key);
}

/* Single renderer owner; access is serialized by the bounded IRQ lock. The
 * ordinary data section is internal SRAM, unlike the renderer's PSRAM stack. */
static struct {
    uint32_t saved_q[32];
    uint16_t masks[4][8];
} __attribute__((aligned(16))) alpha_pie_state = {
    .masks = {
        {1, 1, 1, 1, 1, 1, 1, 1},
        {31, 31, 31, 31, 31, 31, 31, 31},
        {0x07e0, 0x07e0, 0x07e0, 0x07e0, 0x07e0, 0x07e0, 0x07e0, 0x07e0},
        {0x7c00, 0x7c00, 0x7c00, 0x7c00, 0x7c00, 0x7c00, 0x7c00, 0x7c00},
    },
};
extern void deskkin_alpha_pie(uint16_t *, const uint16_t *, const uint8_t *, size_t,
                              uint32_t, void *);
void deskkin_alpha_vectors(uint16_t *dst, const uint16_t *src, const uint8_t *alpha,
                           size_t vectors, uint32_t wire)
{
    unsigned int key = irq_lock();
    uint32_t saved;
    __asm__ volatile("rsr.cpenable %0" : "=r"(saved));
    uint32_t enabled = saved | (1U << 3);
    __asm__ volatile("wsr.cpenable %0; rsync" :: "r"(enabled) : "memory");
    deskkin_alpha_pie(dst, src, alpha, vectors, wire, &alpha_pie_state);
    __asm__ volatile("wsr.cpenable %0; rsync" :: "r"(saved) : "memory");
    irq_unlock(key);
}
