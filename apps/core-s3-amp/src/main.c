// SPDX-License-Identifier: MIT

#include <errno.h>
#include <stdint.h>
#include <string.h>
#include <zephyr/device.h>
#include <zephyr/drivers/ipm.h>
#include <zephyr/drivers/uart.h>
#include <zephyr/kernel.h>
#include <zephyr/sys/byteorder.h>

#define CONTROL_FRAME_MAX 188
#define STATUS_RESPONSE_SIZE 80
#define HEARTBEAT_STALE_MS 500
#define HEARTBEAT_MAGIC 0x44534b4eU

struct renderer_heartbeat {
	uint32_t magic;
	uint32_t generation;
	uint64_t uptime_ms;
};

static const struct device *const console = DEVICE_DT_GET(DT_CHOSEN(zephyr_console));
static const struct device *const ipm = DEVICE_DT_GET(DT_NODELABEL(ipm0));
static atomic_t heartbeat_generation;
static atomic_t heartbeat_received_ms;

static void receive_heartbeat(const struct device *device, void *user_data, uint32_t id,
			      volatile void *data)
{
	ARG_UNUSED(device);
	ARG_UNUSED(user_data);
	ARG_UNUSED(id);
	struct renderer_heartbeat heartbeat;
	memcpy(&heartbeat, (const void *)data, sizeof(heartbeat));
	if (heartbeat.magic != HEARTBEAT_MAGIC) {
		return;
	}
	atomic_set(&heartbeat_received_ms, (atomic_val_t)k_uptime_get_32());
	atomic_set(&heartbeat_generation, (atomic_val_t)heartbeat.generation);
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
	if (!device_is_ready(console) || !device_is_ready(ipm)) {
		return 1;
	}
	ipm_register_callback(ipm, receive_heartbeat, NULL);
	uint8_t frame[CONTROL_FRAME_MAX];
	for (;;) {
		memset(frame, 0, sizeof(frame));
		if (read_status_request(frame) == 0) {
			write_status(frame);
		}
	}
	return 0;
}
