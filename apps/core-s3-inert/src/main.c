// SPDX-License-Identifier: MIT

#include <zephyr/device.h>
#include <zephyr/drivers/display.h>
#include <zephyr/kernel.h>

int main(void)
{
	const struct device *display = DEVICE_DT_GET(DT_CHOSEN(zephyr_display));
	if (device_is_ready(display)) {
		(void)display_blanking_on(display);
	}
	for (;;) {
		k_sleep(K_FOREVER);
	}
	return 0;
}
