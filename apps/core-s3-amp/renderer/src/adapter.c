// SPDX-License-Identifier: GPL-3.0-only

#include <errno.h>
#include <stdbool.h>
#include <stdint.h>
#include <string.h>
#include <esp_cpu.h>
#include <esp_clk_tree.h>
#include <soc/clk_tree_defs.h>
#include <soc/spi_struct.h>
#include <zephyr/device.h>
#include <zephyr/drivers/display.h>
#include <zephyr/kernel.h>
#include "../../shared.h"

#define FRAMEBUFFER_BYTES (320U * 240U * sizeof(uint16_t))
#define BOOT_MARKER ((volatile uint32_t *)(DT_REG_ADDR(DT_NODELABEL(shm0)) + 0x3f0U))
#define AMP_SHARED ((volatile struct deskkin_amp_shared *)DT_REG_ADDR(DT_NODELABEL(shm0)))

enum renderer_stage {
	RENDERER_WAITING_FOR_DISPLAY = 1,
	RENDERER_RENDERING = 2,
	RENDERER_TRANSFERRING = 3,
	RENDERER_PRESENTED = 4,
	RENDERER_FAILED = 5,
};

struct display_request {
	uint8_t buffer_index;
};

struct display_completion {
	uint8_t buffer_index;
	int8_t result;
	uint32_t duration_us;
};

K_MSGQ_DEFINE(display_requests, sizeof(struct display_request), 1, 4);
K_MSGQ_DEFINE(display_completions, sizeof(struct display_completion), 1, 4);
K_THREAD_STACK_DEFINE(display_stack, 4096);
static struct k_thread display_thread;

static const struct device *const display = DEVICE_DT_GET(DT_CHOSEN(zephyr_display));
static uint16_t *framebuffers[2];
static uint16_t *scanout;
static atomic_t generation;
static atomic_t completed_frames;
static atomic_t allocation_failures;
static atomic_t transfer_failures;
static atomic_t render_max_us;
static atomic_t transfer_max_us;
static atomic_t render_last_us;
static atomic_t transfer_last_us;

extern void rust_main(void);

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

void deskkin_renderer_observe(uint8_t stage, uint32_t render_us, uint32_t transfer_us)
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
	const struct deskkin_renderer_heartbeat heartbeat = {
		.magic = DESKKIN_HEARTBEAT_MAGIC,
		.generation = (uint32_t)atomic_inc(&generation) + 1U,
		.completed_frames = (uint32_t)atomic_get(&completed_frames),
		.render_us = (uint32_t)atomic_get(&render_last_us),
		.transfer_us = (uint32_t)atomic_get(&transfer_last_us),
		.render_max_us = (uint32_t)atomic_get(&render_max_us),
		.transfer_max_us = (uint32_t)atomic_get(&transfer_max_us),
		.stage = stage,
		.fault = 0,
		.allocation_failures = (uint8_t)atomic_get(&allocation_failures),
		.transfer_failures = (uint8_t)atomic_get(&transfer_failures),
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
	if (index >= ARRAY_SIZE(framebuffers)) {
		return NULL;
	}
	if (framebuffers[index] == NULL) {
		const uint32_t address = AMP_SHARED->display.framebuffer[index];
		if (address == 0U || (address & 31U) != 0U) {
			atomic_inc(&allocation_failures);
		} else {
			framebuffers[index] = (uint16_t *)(uintptr_t)address;
		}
	}
	if (scanout == NULL) {
		const uint32_t address = AMP_SHARED->display.scanout;
		if (address != 0U && (address & 31U) == 0U) {
			scanout = (uint16_t *)(uintptr_t)address;
		}
	}
	return framebuffers[index];
}

int deskkin_display_submit(uint8_t buffer_index)
{
	if (buffer_index >= ARRAY_SIZE(framebuffers) || framebuffers[buffer_index] == NULL) {
		return -EINVAL;
	}
	const struct display_request request = {.buffer_index = buffer_index};
	return k_msgq_put(&display_requests, &request, K_NO_WAIT);
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
	atomic_inc(&completed_frames);
	return 1;
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
	const struct display_buffer_descriptor descriptor = {
		.buf_size = FRAMEBUFFER_BYTES,
		.pitch = 320,
		.width = 320,
		.height = 240,
	};
	for (;;) {
		struct display_request request;
		(void)k_msgq_get(&display_requests, &request, K_FOREVER);
		const int64_t started = k_uptime_ticks();
		if (scanout == NULL) {
			const struct display_completion completion = {
				.buffer_index = request.buffer_index,
				.result = -1,
				.duration_us = 0,
			};
			(void)k_msgq_put(&display_completions, &completion, K_FOREVER);
			continue;
		}
		memcpy(scanout, framebuffers[request.buffer_index], FRAMEBUFFER_BYTES);
		const int result = display_write(display, 0, 0, &descriptor,
					   scanout);
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
		deskkin_renderer_observe(RENDERER_WAITING_FOR_DISPLAY, 0, 0);
		k_msleep(100);
	}
	*BOOT_MARKER = 7U;
	if (device_init(display) != 0 || !device_is_ready(display)) {
		deskkin_renderer_observe(RENDERER_FAILED, 0, 0);
		return 1;
	}
	__atomic_store_n(&AMP_SHARED->display_spi_hz, display_spi_frequency_hz(), __ATOMIC_RELEASE);
	*BOOT_MARKER = 8U;
	k_thread_create(&display_thread, display_stack, K_THREAD_STACK_SIZEOF(display_stack),
			display_entry, NULL, NULL, NULL, -1, 0, K_NO_WAIT);
	rust_main();
	return 0;
}
