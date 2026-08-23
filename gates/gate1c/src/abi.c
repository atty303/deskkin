// SPDX-License-Identifier: MIT

#include <stdbool.h>
#include <stdint.h>
#include <string.h>
#include <zephyr/device.h>
#include <zephyr/drivers/uart.h>
#include <zephyr/irq.h>
#include <zephyr/kernel.h>

#define RUN_ID_LENGTH 36
#define COMMAND_CAPACITY 128

extern uint32_t deskkin_rust_add(uint32_t left, uint32_t right);

static const struct device *const console = DEVICE_DT_GET(DT_CHOSEN(zephyr_console));

static int valid_run_id(const char *value)
{
	for (size_t index = 0; index < RUN_ID_LENGTH; ++index) {
		const char character = value[index];
		const bool hyphen = index == 8 || index == 13 || index == 18 || index == 23;
		if (hyphen ? character != '-' : !((character >= '0' && character <= '9') ||
						      (character >= 'a' && character <= 'f'))) {
			return 0;
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

uint32_t deskkin_c_multiply(uint32_t left, uint32_t right)
{
	return left * right;
}

uint32_t deskkin_c_to_rust_check(void)
{
	return deskkin_rust_add(19, 23);
}

void deskkin_c_idle(void)
{
	k_msleep(10);
}

const char *deskkin_firmware_digest(void)
{
	return DESKKIN_FIRMWARE_DIGEST;
}

int deskkin_wait_command(char *run_id)
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

uint32_t deskkin_interrupt_state_probe(void)
{
	const unsigned int key = irq_lock();
	const uint32_t was_unlocked = arch_irq_unlocked(key) ? 1U : 0U;
	irq_unlock(key);
	return was_unlocked;
}
