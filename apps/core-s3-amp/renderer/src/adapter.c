// SPDX-License-Identifier: GPL-3.0-only

#include <errno.h>
#include <stdint.h>
#include <string.h>
#include <esp_cpu.h>
#include <esp_clk_tree.h>
#include <soc/clk_tree_defs.h>
#include <soc/spi_struct.h>
#include <zephyr/device.h>
#include <zephyr/drivers/display.h>
#include <zephyr/kernel.h>
#include <zephyr/sys/sys_heap.h>
#include "../../shared.h"

#define DISPLAY_WIDTH 320U
#define DISPLAY_HEIGHT 240U
#define BYTES_PER_LINE (DISPLAY_WIDTH * sizeof(uint16_t))
#define FRAME_PIXELS (DISPLAY_WIDTH * DISPLAY_HEIGHT)
#define FRAMEBUFFER_COUNT 2U
#define MAX_DIRTY_RECTS 3U
#define FULL_WIDTH_CHUNK_LINES 30U
#define BOOT_MARKER ((volatile uint32_t *)(DT_REG_ADDR(DT_NODELABEL(shm0)) + 0x3f0U))
#define AMP_SHARED ((volatile struct deskkin_amp_shared *)DT_REG_ADDR(DT_NODELABEL(shm0)))

enum renderer_stage {
	RENDERER_WAITING_FOR_DISPLAY = 1,
	RENDERER_RENDERING = 2,
	RENDERER_TRANSFERRING = 3,
	RENDERER_PRESENTED = 4,
	RENDERER_FAILED = 5,
};

enum renderer_fault {
	RENDERER_FAULT_NONE = 0,
	RENDERER_FAULT_HEAP_EXHAUSTED = 11,
	RENDERER_FAULT_DISPLAY_INIT = 12,
	RENDERER_FAULT_HEAP_INIT = 13,
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
K_THREAD_STACK_DEFINE(display_stack, 4096);
static struct k_thread display_thread;

static const struct device *const display = DEVICE_DT_GET(DT_CHOSEN(zephyr_display));
static uint16_t *framebuffer;
static struct sys_heap renderer_heap;
static bool renderer_heap_ready;
static atomic_t generation;
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

extern void rust_main(void);
void deskkin_renderer_observe(uint8_t stage, uint8_t fault, uint32_t render_us,
			      uint32_t transfer_us);

void *malloc(size_t size)
{
	if (!renderer_heap_ready) {
		return NULL;
	}
	void *block = sys_heap_alloc(&renderer_heap, size);
	if (block == NULL) {
		atomic_inc(&allocation_failures);
		deskkin_renderer_observe(RENDERER_FAILED, RENDERER_FAULT_HEAP_EXHAUSTED, 0, 0);
	}
	return block;
}

void free(void *block)
{
	if (block != NULL) {
		sys_heap_free(&renderer_heap, block);
	}
}

static int initialize_renderer_heap(void)
{
	const uint32_t address = AMP_SHARED->display.renderer_heap;
	const uint32_t size = AMP_SHARED->display.renderer_heap_size;
	if (address == 0U || size == 0U || (address & 31U) != 0U) {
		return -ENOMEM;
	}
	sys_heap_init(&renderer_heap, (void *)(uintptr_t)address, size);
	renderer_heap_ready = true;
	return 0;
}

void deskkin_renderer_boot_stage(uint8_t stage)
{
	*BOOT_MARKER = stage;
}

static int renderer_early_marker(void)
{
	*BOOT_MARKER = 1U;
	return 0;
}

SYS_INIT(renderer_early_marker, EARLY, 0);

void deskkin_renderer_observe(uint8_t stage, uint8_t fault, uint32_t render_us,
			      uint32_t transfer_us)
{
	*BOOT_MARKER = 5U;
	if (render_us != 0U) {
		atomic_set(&render_last_us, (atomic_val_t)render_us);
	}
	if (transfer_us != 0U) {
		atomic_set(&transfer_last_us, (atomic_val_t)transfer_us);
	}
	atomic_val_t maximum = atomic_get(&render_max_us);
	while (render_us > (uint32_t)maximum &&
	       !atomic_cas(&render_max_us, maximum, (atomic_val_t)render_us)) {
		maximum = atomic_get(&render_max_us);
	}
	maximum = atomic_get(&transfer_max_us);
	while (transfer_us > (uint32_t)maximum &&
	       !atomic_cas(&transfer_max_us, maximum, (atomic_val_t)transfer_us)) {
		maximum = atomic_get(&transfer_max_us);
	}
	if (stage == RENDERER_PRESENTED) {
		atomic_inc(&completed_frames);
	}
	const struct deskkin_renderer_heartbeat heartbeat = {
		.magic = DESKKIN_HEARTBEAT_MAGIC,
		.generation = (uint32_t)atomic_inc(&generation) + 1U,
		.completed_frames = (uint32_t)atomic_get(&completed_frames),
		.render_us = (uint32_t)atomic_get(&render_last_us),
		.transfer_us = (uint32_t)atomic_get(&transfer_last_us),
		.copy_us = (uint32_t)atomic_get(&copy_last_us),
		.render_max_us = (uint32_t)atomic_get(&render_max_us),
		.transfer_max_us = (uint32_t)atomic_get(&transfer_max_us),
		.stage = stage,
		.fault = fault,
		.allocation_failures = (uint8_t)atomic_get(&allocation_failures),
		.transfer_failures = (uint8_t)atomic_get(&transfer_failures),
		.dirty_rect_count = (uint8_t)atomic_get(&dirty_rect_count),
		.pixel_dma_batches = (uint16_t)atomic_get(&pixel_dma_batches),
		.dirty_pixels = (uint32_t)atomic_get(&dirty_pixels),
		.transferred_bytes = (uint32_t)atomic_get(&transferred_bytes),
	};
	/* Zero marks the payload unstable while the next snapshot is copied. */
	__atomic_store_n(&AMP_SHARED->renderer_publication, 0U, __ATOMIC_SEQ_CST);
	memcpy((void *)&AMP_SHARED->renderer, &heartbeat, sizeof(heartbeat));
	__atomic_store_n(&AMP_SHARED->renderer_publication, heartbeat.generation,
			 __ATOMIC_SEQ_CST);
	*BOOT_MARKER = 6U;
}

uint64_t deskkin_uptime_us(void)
{
	return k_ticks_to_us_floor64(k_uptime_ticks());
}

void deskkin_sleep_ms(uint32_t delay_ms)
{
	k_msleep(delay_ms);
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

static int display_write_rect(const uint16_t *pixels,
			      const struct deskkin_dirty_rect *rect)
{
	if (rect->width == DISPLAY_WIDTH) {
		uint16_t line = rect->y;
		const uint16_t end = rect->y + rect->height;
		while (line < end) {
			const uint16_t height = MIN(FULL_WIDTH_CHUNK_LINES, end - line);
			const struct display_buffer_descriptor descriptor = {
				.buf_size = DISPLAY_WIDTH * height * sizeof(uint16_t),
				.pitch = DISPLAY_WIDTH,
				.width = DISPLAY_WIDTH,
				.height = height,
			};
			const int result =
				display_write(display, 0, line, &descriptor,
					      pixels + (size_t)line * DISPLAY_WIDTH);
			if (result != 0) {
				return result;
			}
			line += height;
		}
		return 0;
	}

	const struct display_buffer_descriptor descriptor = {
		.buf_size =
			(((uint32_t)rect->height - 1U) * DISPLAY_WIDTH + rect->width) *
			sizeof(uint16_t),
		.pitch = DISPLAY_WIDTH,
		.width = rect->width,
		.height = rect->height,
	};
	return display_write(display, rect->x, rect->y, &descriptor,
			     pixels + (size_t)rect->y * DISPLAY_WIDTH + rect->x);
}

static void display_entry(void *first, void *second, void *third)
{
	ARG_UNUSED(first);
	ARG_UNUSED(second);
	ARG_UNUSED(third);
	for (;;) {
		struct display_request request;
		(void)k_msgq_get(&display_requests, &request, K_FOREVER);
		const int64_t started = k_uptime_ticks();
		const uint16_t *pixels =
			framebuffer + (size_t)request.buffer_index * FRAME_PIXELS;
		uint32_t request_pixels = 0U;
		uint32_t request_batches = 0U;
		for (size_t index = 0; index < request.dirty_rect_count; ++index) {
			const struct deskkin_dirty_rect *rect = &request.dirty_rects[index];
			request_pixels += (uint32_t)rect->width * rect->height;
			request_batches += rect->width == DISPLAY_WIDTH
					   ? DIV_ROUND_UP(rect->height, FULL_WIDTH_CHUNK_LINES)
					   : DIV_ROUND_UP(rect->height,
							  CONFIG_DMA_ESP32_MAX_DESCRIPTOR_NUM);
		}
		atomic_set(&dirty_rect_count, request.dirty_rect_count);
		atomic_set(&pixel_dma_batches, (atomic_val_t)request_batches);
		atomic_set(&dirty_pixels, (atomic_val_t)request_pixels);
		atomic_set(&transferred_bytes,
			   (atomic_val_t)(request_pixels * sizeof(uint16_t)));
		int result = 0;
		for (size_t index = 0; index < request.dirty_rect_count; ++index) {
			result = display_write_rect(pixels, &request.dirty_rects[index]);
			if (result != 0) {
				break;
			}
		}
		const uint64_t elapsed = k_ticks_to_us_floor64(k_uptime_ticks() - started);
		const struct display_completion completion = {
			.buffer_index = request.buffer_index,
			.result = result == 0 ? 0 : -1,
			.duration_us = (uint32_t)MIN(elapsed, UINT32_MAX),
		};
		(void)k_msgq_put(&display_completions, &completion, K_FOREVER);
	}
}

int main(void)
{
	*BOOT_MARKER = 2U;
	BOOT_MARKER[1] = esp_cpu_get_core_id();
	*BOOT_MARKER = 4U;
	while (__atomic_load_n(&AMP_SHARED->display_publication, __ATOMIC_ACQUIRE) == 0U ||
	       AMP_SHARED->display.magic != DESKKIN_DISPLAY_MAGIC || AMP_SHARED->display.ready != 1U) {
		deskkin_renderer_observe(RENDERER_WAITING_FOR_DISPLAY, RENDERER_FAULT_NONE, 0, 0);
		k_msleep(100);
	}
	*BOOT_MARKER = 7U;
	if (initialize_renderer_heap() != 0) {
		atomic_inc(&allocation_failures);
		deskkin_renderer_observe(RENDERER_FAILED, RENDERER_FAULT_HEAP_INIT, 0, 0);
		return 1;
	}
	const int display_result = device_init(display);
	if (display_result == -ENOMEM) {
		atomic_inc(&allocation_failures);
	}
	if (display_result != 0 || !device_is_ready(display)) {
		deskkin_renderer_observe(RENDERER_FAILED, RENDERER_FAULT_DISPLAY_INIT, 0, 0);
		return 1;
	}
	__atomic_store_n(&AMP_SHARED->display_spi_hz, display_spi_frequency_hz(), __ATOMIC_RELEASE);
	*BOOT_MARKER = 8U;
	k_thread_create(&display_thread, display_stack, K_THREAD_STACK_SIZEOF(display_stack),
			display_entry, NULL, NULL, NULL, 0, 0, K_NO_WAIT);
	rust_main();
	return 0;
}
