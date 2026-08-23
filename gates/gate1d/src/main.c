// SPDX-License-Identifier: MIT

#include <errno.h>
#include <stdbool.h>
#include <stdint.h>
#include <string.h>
#include <soc/soc_memory_layout.h>
#include <zephyr/device.h>
#include <zephyr/drivers/display.h>
#include <zephyr/drivers/flash.h>
#include <zephyr/drivers/gpio.h>
#include <zephyr/drivers/uart.h>
#include <zephyr/input/input.h>
#include <zephyr/kernel.h>
#include <zephyr/multi_heap/shared_multi_heap.h>
#include <zephyr/sys/printk.h>

#define RUN_ID_LENGTH 36
#define COMMAND_CAPACITY 128
#define PSRAM_PROBE_BYTES 32768
#define FLASH_PROBE_BYTES 32
#define RECT_COUNT 3
#define MAX_RECT_BYTES (80 * 60 * 2)

struct gate_rect {
	uint16_t x;
	uint16_t y;
	uint16_t width;
	uint16_t height;
	uint16_t color;
};

struct touch_point {
	int32_t x;
	int32_t y;
};

static const struct gate_rect rectangles[RECT_COUNT] = {
	{.x = 20, .y = 20, .width = 80, .height = 60, .color = 0xf800},
	{.x = 120, .y = 90, .width = 80, .height = 60, .color = 0x07e0},
	{.x = 220, .y = 160, .width = 80, .height = 60, .color = 0x001f},
};

static const struct device *const console = DEVICE_DT_GET(DT_CHOSEN(zephyr_console));
static const struct device *const display = DEVICE_DT_GET(DT_CHOSEN(zephyr_display));
static const struct device *const touch = DEVICE_DT_GET(DT_CHOSEN(zephyr_touch));
static const struct device *const flash = DEVICE_DT_GET(DT_CHOSEN(zephyr_flash_controller));
static const struct device *const i2c0 = DEVICE_DT_GET(DT_NODELABEL(i2c0));
static const struct device *const i2c1 = DEVICE_DT_GET(DT_NODELABEL(i2c1));
static const struct device *const spi2 = DEVICE_DT_GET(DT_NODELABEL(spi2));
static const struct device *const wifi = DEVICE_DT_GET(DT_NODELABEL(wifi));
static const struct device *const gpio_expander = DEVICE_DT_GET(DT_NODELABEL(aw9523b_gpio));
static const struct device *const power_regulator =
	DEVICE_DT_GET(DT_PARENT(DT_NODELABEL(regulator)));

K_MSGQ_DEFINE(touch_points, sizeof(struct touch_point), 4, 4);

static int32_t touch_x;
static int32_t touch_y;
static uint16_t pixel_buffer[MAX_RECT_BYTES / sizeof(uint16_t)];

static void touch_callback(struct input_event *event, void *user_data)
{
	ARG_UNUSED(user_data);
	if (event->code == INPUT_ABS_X) {
		touch_x = event->value;
	} else if (event->code == INPUT_ABS_Y) {
		touch_y = event->value;
	} else if (event->code == INPUT_BTN_TOUCH && event->value != 0 && event->sync) {
		const struct touch_point point = {.x = touch_x, .y = touch_y};
		(void)k_msgq_put(&touch_points, &point, K_NO_WAIT);
	}
}
INPUT_CALLBACK_DEFINE(touch, touch_callback, NULL);

static bool valid_run_id(const char *value)
{
	for (size_t index = 0; index < RUN_ID_LENGTH; ++index) {
		const char character = value[index];
		const bool hyphen = index == 8 || index == 13 || index == 18 || index == 23;
		if (hyphen ? character != '-' : !((character >= '0' && character <= '9') ||
					      (character >= 'a' && character <= 'f'))) {
			return false;
		}
	}
	return value[RUN_ID_LENGTH] == '\0';
}

static int parse_command(const char *line, char *run_id)
{
	static const char status_prefix[] = "DESKKIN_GATE_COMMAND schema=1 action=status run_id=";
	static const char run_prefix[] = "DESKKIN_GATE_COMMAND schema=1 action=run run_id=";
	const char *value;
	int action;

	if (strncmp(line, status_prefix, sizeof(status_prefix) - 1) == 0) {
		value = line + sizeof(status_prefix) - 1;
		action = 1;
	} else if (strncmp(line, run_prefix, sizeof(run_prefix) - 1) == 0) {
		value = line + sizeof(run_prefix) - 1;
		action = 2;
	} else {
		return 0;
	}
	if (!valid_run_id(value)) {
		return 0;
	}
	memcpy(run_id, value, RUN_ID_LENGTH + 1);
	return action;
}

static int wait_command(char *run_id)
{
	char line[COMMAND_CAPACITY];
	size_t length = 0;
	uint8_t character;

	if (!device_is_ready(console)) {
		return -1;
	}
	for (;;) {
		if (uart_poll_in(console, &character) != 0) {
			k_msleep(10);
			continue;
		}
		if (character == '\r') {
			continue;
		}
		if (character == '\n') {
			line[length] = '\0';
			const int action = parse_command(line, run_id);
			length = 0;
			if (action != 0) {
				return action;
			}
			continue;
		}
		if (length + 1 < sizeof(line)) {
			line[length++] = (char)character;
		} else {
			length = 0;
		}
	}
}

static bool probe_devices(const char *run_id)
{
	const bool power_ok = device_is_ready(power_regulator);
	const bool gpio_ok = device_is_ready(gpio_expander);
	const bool display_ok = device_is_ready(display);
	const bool touch_ok = device_is_ready(touch);
	const bool flash_ok = device_is_ready(flash);
	const bool i2c0_ok = device_is_ready(i2c0);
	const bool i2c1_ok = device_is_ready(i2c1);
	const bool spi2_ok = device_is_ready(spi2);
	struct display_capabilities capabilities;

	if (!(power_ok && gpio_ok && display_ok && touch_ok && flash_ok && i2c0_ok && i2c1_ok &&
	      spi2_ok)) {
		return false;
	}
	display_get_capabilities(display, &capabilities);
	if (capabilities.x_resolution != 320 || capabilities.y_resolution != 240 ||
	    capabilities.current_pixel_format != PIXEL_FORMAT_RGB_565) {
		return false;
	}
	printk("DESKKIN_GATE_EVENT schema=1 event=devices run_id=%s power=ok gpio=ok display=ok "
	       "touch=ok flash=ok i2c0=ok i2c1=ok spi2=ok width=320 height=240 format=rgb565\n",
	       run_id);
	return true;
}

static bool probe_psram(const char *run_id)
{
	uint32_t *memory = shared_multi_heap_aligned_alloc(SMH_REG_ATTR_EXTERNAL, 32,
							 PSRAM_PROBE_BYTES);
	if (memory == NULL || !esp_ptr_external_ram(memory)) {
		return false;
	}
	for (size_t index = 0; index < PSRAM_PROBE_BYTES / sizeof(*memory); ++index) {
		memory[index] = (uint32_t)index ^ 0xa55aa55aU;
	}
	for (size_t index = 0; index < PSRAM_PROBE_BYTES / sizeof(*memory); ++index) {
		if (memory[index] != ((uint32_t)index ^ 0xa55aa55aU)) {
			shared_multi_heap_free(memory);
			return false;
		}
	}
	shared_multi_heap_free(memory);
	printk("DESKKIN_GATE_EVENT schema=1 event=psram run_id=%s bytes=%u status=ok\n", run_id,
	       PSRAM_PROBE_BYTES);
	return true;
}

static bool probe_wifi(const char *run_id)
{
	const bool ready = device_is_ready(wifi);
	printk("DESKKIN_GATE_EVENT schema=1 event=wifi run_id=%s status=%s\n", run_id,
	       ready ? "ready" : "not_ready");
	return ready;
}

static bool probe_flash(const char *run_id)
{
	uint8_t bytes[FLASH_PROBE_BYTES];
	if (flash_read(flash, 0, bytes, sizeof(bytes)) != 0) {
		return false;
	}
	uint32_t nonzero = 0;
	for (size_t index = 0; index < sizeof(bytes); ++index) {
		nonzero |= bytes[index];
	}
	if (nonzero == 0) {
		return false;
	}
	printk("DESKKIN_GATE_EVENT schema=1 event=flash_read run_id=%s bytes=%u status=ok\n",
	       run_id, FLASH_PROBE_BYTES);
	return true;
}

static bool draw_rectangles(const char *run_id)
{
	for (size_t index = 0; index < RECT_COUNT; ++index) {
		const struct gate_rect *rect = &rectangles[index];
		const size_t pixels = rect->width * rect->height;
		for (size_t pixel = 0; pixel < pixels; ++pixel) {
			pixel_buffer[pixel] = rect->color;
		}
		const struct display_buffer_descriptor descriptor = {
			.buf_size = pixels * sizeof(uint16_t),
			.pitch = rect->width,
			.width = rect->width,
			.height = rect->height,
		};
		const uint32_t started = k_cycle_get_32();
		if (display_write(display, rect->x, rect->y, &descriptor, pixel_buffer) != 0) {
			return false;
		}
		const uint64_t duration_us =
			k_cyc_to_us_floor64((uint32_t)(k_cycle_get_32() - started));
		printk("DESKKIN_GATE_EVENT schema=1 event=display_rect run_id=%s index=%u x=%u y=%u "
		       "width=%u height=%u bytes=%u duration_us=%llu status=ok\n",
		       run_id, (unsigned int)index + 1, rect->x, rect->y, rect->width, rect->height,
		       (unsigned int)descriptor.buf_size, (unsigned long long)duration_us);
	}
	const int result = display_blanking_off(display);
	return result == 0 || result == -ENOSYS;
}

static bool point_in_rect(const struct touch_point *point, const struct gate_rect *rect)
{
	return point->x >= rect->x && point->x < rect->x + rect->width && point->y >= rect->y &&
	       point->y < rect->y + rect->height;
}

static void verify_touches(const char *run_id)
{
	k_msgq_purge(&touch_points);
	for (size_t index = 0; index < RECT_COUNT; ++index) {
		struct touch_point point;
		for (;;) {
			k_msgq_get(&touch_points, &point, K_FOREVER);
			const bool inside = point_in_rect(&point, &rectangles[index]);
			printk("DESKKIN_GATE_EVENT schema=1 event=touch_sample run_id=%s "
			       "expected_index=%u x=%d y=%d inside=%s\n",
			       run_id, (unsigned int)index + 1, point.x, point.y,
			       inside ? "yes" : "no");
			if (inside) {
				break;
			}
		}
		printk("DESKKIN_GATE_EVENT schema=1 event=touch run_id=%s index=%u x=%d y=%d "
		       "status=ok\n",
		       run_id, (unsigned int)index + 1, point.x, point.y);
	}
}

int main(void)
{
	for (;;) {
		char run_id[RUN_ID_LENGTH + 1] = {0};
		const int action = wait_command(run_id);
		if (action < 0) {
			return 1;
		}
		if (action == 1) {
			printk("DESKKIN_GATE_EVENT schema=1 event=idle run_id=%s firmware_digest=%s\n",
			       run_id, DESKKIN_FIRMWARE_DIGEST);
			continue;
		}

		printk("DESKKIN_GATE_EVENT schema=1 event=boot run_id=%s board=m5stack_cores3 "
		       "firmware_digest=%s\n",
		       run_id, DESKKIN_FIRMWARE_DIGEST);
		if (!probe_devices(run_id) || !probe_wifi(run_id) || !probe_psram(run_id) ||
		    !probe_flash(run_id) ||
		    !draw_rectangles(run_id)) {
			printk("DESKKIN_GATE_RESULT schema=1 run_id=%s result=fail\n", run_id);
			printk("DESKKIN_GATE_EVENT schema=1 event=idle run_id=%s firmware_digest=%s\n",
			       run_id, DESKKIN_FIRMWARE_DIGEST);
			continue;
		}
		printk("DESKKIN_GATE_EVENT schema=1 event=panel run_id=%s pattern=rgb_rectangles "
		       "status=ready\n",
		       run_id);
		verify_touches(run_id);
		printk("DESKKIN_GATE_RESULT schema=1 run_id=%s result=pass\n", run_id);
		printk("DESKKIN_GATE_EVENT schema=1 event=idle run_id=%s firmware_digest=%s\n",
		       run_id, DESKKIN_FIRMWARE_DIGEST);
	}
}
