// SPDX-License-Identifier: MIT

#include <errno.h>
#include <stdint.h>
#include <string.h>
#include <zephyr/device.h>
#include <zephyr/drivers/gpio.h>
#include <zephyr/drivers/regulator.h>
#include <zephyr/drivers/uart.h>
#include <zephyr/kernel.h>
#include <zephyr/sys/byteorder.h>
#include "../shared.h"

#define CONTROL_FRAME_MAX 188
#define STATUS_RESPONSE_SIZE 80
#define HEARTBEAT_STALE_MS 500
#define APPCPU_BOOT_MARKER ((volatile uint32_t *)(DT_REG_ADDR(DT_NODELABEL(shm0)) + 0x3f0U))
#define AMP_SHARED ((volatile struct deskkin_amp_shared *)DT_REG_ADDR(DT_NODELABEL(shm0)))

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
static atomic_t display_ready;
static atomic_t boot_stage;
static atomic_t boot_error;
static __aligned(32) uint16_t internal_framebuffer[320U * 240U];

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
		publication =
			__atomic_load_n(&AMP_SHARED->renderer_publication, __ATOMIC_ACQUIRE);
		if (publication == 0U || publication == received_publication) {
			return;
		}
		memcpy(&heartbeat, (const void *)&AMP_SHARED->renderer, sizeof(heartbeat));
		const uint32_t after =
			__atomic_load_n(&AMP_SHARED->renderer_publication, __ATOMIC_ACQUIRE);
		if (publication == after && heartbeat.magic == DESKKIN_HEARTBEAT_MAGIC &&
		    heartbeat.generation == publication) {
			stable = true;
			break;
		}
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
	for (;;) {
		receive_heartbeat();
		if (atomic_get(&display_ready) != 0 && AMP_SHARED->display_publication == 0U) {
			const struct deskkin_display_ready message = {
				.magic = DESKKIN_DISPLAY_MAGIC,
				.generation = 1U,
				.ready = 1U,
				.framebuffer = (uint32_t)(uintptr_t)internal_framebuffer,
			};
			memcpy((void *)&AMP_SHARED->display, &message, sizeof(message));
			__atomic_store_n(&AMP_SHARED->display_publication, 1U, __ATOMIC_RELEASE);
		}
		k_msleep(1);
	}
}

static void boot_entry(void *first, void *second, void *third)
{
	ARG_UNUSED(first);
	ARG_UNUSED(second);
	ARG_UNUSED(third);
	atomic_set(&boot_stage, 1);
	atomic_set(&boot_stage, 2);
	atomic_set(&boot_stage, 3);
	if (esp_appcpu_init() != 0) {
		atomic_set(&boot_error, 3);
		return;
	}
	atomic_set(&boot_stage, 4);
	if (initialize_display_power() == 0) {
		atomic_set(&display_ready, 1);
	} else {
		atomic_set(&boot_error, 4);
		return;
	}
	atomic_set(&boot_stage, 5);
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

static void write_status(const uint8_t *request)
{
	uint8_t response[STATUS_RESPONSE_SIZE] = {0};
	response[0] = 1;
	memcpy(&response[2], &request[2], 16);
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
	response[67] =
		__atomic_load_n(&AMP_SHARED->renderer_publication, __ATOMIC_ACQUIRE) != 0U ? 1U : 0U;
	response[68] = (uint8_t)atomic_get(&boot_stage);
	response[69] = (uint8_t)atomic_get(&boot_error);
	sys_put_be32(__atomic_load_n(&AMP_SHARED->display_spi_hz, __ATOMIC_ACQUIRE), &response[70]);
	sys_put_be32((uint32_t)atomic_get(&copy_us), &response[74]);
	const uint32_t now = k_uptime_get_32();
	response[27] = generation != 0U && now - received_ms <= HEARTBEAT_STALE_MS ? 1U : 2U;
	response[78] = 9;
	uart_poll_out(console, 0);
	uart_poll_out(console, STATUS_RESPONSE_SIZE);
	for (size_t index = 0; index < sizeof(response); ++index) {
		uart_poll_out(console, response[index]);
	}
}

int main(void)
{
	if (!device_is_ready(console)) {
		return 1;
	}
	memset((void *)AMP_SHARED, 0, sizeof(*AMP_SHARED));
	k_thread_create(&supervisor_thread, supervisor_stack, K_THREAD_STACK_SIZEOF(supervisor_stack),
			supervisor_entry, NULL, NULL, NULL, 3, 0, K_NO_WAIT);
	k_thread_create(&boot_thread, boot_stack, K_THREAD_STACK_SIZEOF(boot_stack), boot_entry, NULL,
			NULL, NULL, 4, 0, K_NO_WAIT);
	uint8_t frame[CONTROL_FRAME_MAX];
	for (;;) {
		memset(frame, 0, sizeof(frame));
		if (read_status_request(frame) == 0) {
			write_status(frame);
		}
	}
	return 0;
}
