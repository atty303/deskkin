// SPDX-License-Identifier: MIT

#include <stdint.h>
#include <zephyr/device.h>
#include <zephyr/drivers/ipm.h>
#include <zephyr/kernel.h>

#define HEARTBEAT_MAGIC 0x44534b4eU
#define FAULT_AFTER_MS 10000

struct renderer_heartbeat {
	uint32_t magic;
	uint32_t generation;
	uint64_t uptime_ms;
};

int main(void)
{
	const struct device *const ipm = DEVICE_DT_GET(DT_NODELABEL(ipm0));
	if (!device_is_ready(ipm)) {
		return 1;
	}
	uint32_t generation = 0;
	while (k_uptime_get() < FAULT_AFTER_MS) {
		struct renderer_heartbeat heartbeat = {
			.magic = HEARTBEAT_MAGIC,
			.generation = ++generation,
			.uptime_ms = (uint64_t)k_uptime_get(),
		};
		(void)ipm_send(ipm, -1, sizeof(heartbeat), &heartbeat, sizeof(heartbeat));
		k_msleep(100);
	}
	for (;;) {
		/* Fault injection: APPCPU deliberately stops yielding. */
	}
	return 0;
}
