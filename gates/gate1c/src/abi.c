// SPDX-License-Identifier: MIT

#include <stdint.h>
#include <zephyr/kernel.h>

extern uint32_t deskkin_rust_add(uint32_t left, uint32_t right);

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
	k_msleep(60000);
}
