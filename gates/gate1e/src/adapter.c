// SPDX-License-Identifier: GPL-3.0-only

#include <errno.h>
#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>
#include <string.h>
#include <zephyr/device.h>
#include <zephyr/drivers/display.h>
#include <zephyr/drivers/uart.h>
#include <zephyr/input/input.h>
#include <zephyr/kernel.h>
#include <zephyr/multi_heap/shared_multi_heap.h>
#include <zephyr/sys/printk.h>

#define RUN_ID_LENGTH 36
#define COMMAND_CAPACITY 160
#define FRAMEBUFFER_BYTES (320U * 240U * sizeof(uint16_t))

static const struct device *const console = DEVICE_DT_GET(DT_CHOSEN(zephyr_console));
static const struct device *const display = DEVICE_DT_GET(DT_CHOSEN(zephyr_display));
static const struct device *const touch = DEVICE_DT_GET(DT_CHOSEN(zephyr_touch));

static struct k_spinlock touch_lock;
static int32_t touch_x;
static int32_t touch_y;
static uint32_t touch_generation;
static uint32_t consumed_generation;

extern void rust_main(void);

static void touch_callback(struct input_event *event, void *user_data)
{
	ARG_UNUSED(user_data);
	k_spinlock_key_t key = k_spin_lock(&touch_lock);
	if (event->code == INPUT_ABS_X) {
		touch_x = event->value;
	} else if (event->code == INPUT_ABS_Y) {
		touch_y = event->value;
	} else if (event->code == INPUT_BTN_TOUCH && event->value != 0 && event->sync) {
		touch_generation++;
	}
	k_spin_unlock(&touch_lock, key);
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
	static const char qualification_prefix[] =
		"DESKKIN_GATE_COMMAND schema=1 action=run mode=qualification run_id=";
	static const char conformance_prefix[] =
		"DESKKIN_GATE_COMMAND schema=1 action=run mode=conformance run_id=";
	const char *value;
	int action;

	if (strncmp(line, status_prefix, sizeof(status_prefix) - 1) == 0) {
		value = line + sizeof(status_prefix) - 1;
		action = 1;
	} else if (strncmp(line, qualification_prefix, sizeof(qualification_prefix) - 1) == 0) {
		value = line + sizeof(qualification_prefix) - 1;
		action = 2;
	} else if (strncmp(line, conformance_prefix, sizeof(conformance_prefix) - 1) == 0) {
		value = line + sizeof(conformance_prefix) - 1;
		action = 3;
	} else {
		return 0;
	}
	if (!valid_run_id(value)) {
		return 0;
	}
	memcpy(run_id, value, RUN_ID_LENGTH + 1);
	return action;
}

int deskkin_wait_command(char *run_id)
{
	char line[COMMAND_CAPACITY];
	size_t length = 0;
	uint8_t character;

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
			if (action == 1) {
				printk("DESKKIN_GATE_EVENT schema=1 event=idle run_id=%s firmware_digest=%s\n",
				       run_id, DESKKIN_FIRMWARE_DIGEST);
			} else if (action != 0) {
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

bool deskkin_devices_ready(void)
{
	struct display_capabilities capabilities;
	if (!device_is_ready(console) || !device_is_ready(display) || !device_is_ready(touch)) {
		return false;
	}
	display_get_capabilities(display, &capabilities);
	return capabilities.x_resolution == 320 && capabilities.y_resolution == 240 &&
	       capabilities.current_pixel_format == PIXEL_FORMAT_RGB_565;
}

void deskkin_print_boot(const char *run_id, const char *mode)
{
	printk("DESKKIN_GATE_EVENT schema=1 event=boot run_id=%s mode=%s firmware_digest=%s "
	       "workload_digest=%s\n",
	       run_id, mode, DESKKIN_FIRMWARE_DIGEST, DESKKIN_WORKLOAD_DIGEST);
}

void deskkin_print_idle(const char *run_id)
{
	printk("DESKKIN_GATE_EVENT schema=1 event=idle run_id=%s firmware_digest=%s\n", run_id,
	       DESKKIN_FIRMWARE_DIGEST);
}

uint16_t *deskkin_framebuffer_alloc(void)
{
	uint16_t *buffer = shared_multi_heap_aligned_alloc(SMH_REG_ATTR_EXTERNAL, 32,
							FRAMEBUFFER_BYTES);
	if (buffer != NULL) {
		memset(buffer, 0, FRAMEBUFFER_BYTES);
	}
	return buffer;
}

uint16_t *deskkin_staging_alloc(void)
{
	return shared_multi_heap_aligned_alloc(SMH_REG_ATTR_EXTERNAL, 32,
					       FRAMEBUFFER_BYTES);
}

uint32_t deskkin_now_cycles(void)
{
	return k_cycle_get_32();
}

uint32_t deskkin_elapsed_us(uint32_t started)
{
	return (uint32_t)k_cyc_to_us_floor64((uint32_t)(k_cycle_get_32() - started));
}

int deskkin_display_write(uint16_t x, uint16_t y, uint16_t width, uint16_t height,
			  uint16_t pitch, const uint16_t *pixels, uint64_t *duration_us)
{
	const struct display_buffer_descriptor descriptor = {
		.buf_size = (size_t)pitch * height * sizeof(uint16_t),
		.pitch = pitch,
		.width = width,
		.height = height,
	};
	const uint32_t started = deskkin_now_cycles();
	const int result = display_write(display, x, y, &descriptor, pixels);
	*duration_us = deskkin_elapsed_us(started);
	return result;
}

int deskkin_display_enable(void)
{
	const int result = display_blanking_off(display);
	return result == -ENOSYS ? 0 : result;
}

int deskkin_inject_touch(void)
{
	int result = input_report_abs(touch, INPUT_ABS_X, 160, false, K_FOREVER);
	result = result == 0 ? input_report_abs(touch, INPUT_ABS_Y, 120, false, K_FOREVER) : result;
	result = result == 0 ? input_report_key(touch, INPUT_BTN_TOUCH, 1, true, K_FOREVER) : result;
	return result;
}

bool deskkin_take_touch(int32_t *x, int32_t *y)
{
	bool available = false;
	k_spinlock_key_t key = k_spin_lock(&touch_lock);
	if (touch_generation != consumed_generation) {
		consumed_generation = touch_generation;
		*x = touch_x;
		*y = touch_y;
		available = true;
	}
	k_spin_unlock(&touch_lock, key);
	return available;
}

int main(void)
{
	rust_main();
	return 0;
}
