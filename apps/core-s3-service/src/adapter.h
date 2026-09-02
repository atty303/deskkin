// SPDX-License-Identifier: GPL-3.0-only

#ifndef DESKKIN_CORE_S3_SERVICE_ADAPTER_H
#define DESKKIN_CORE_S3_SERVICE_ADAPTER_H

#include <zephyr/kernel/thread_stack.h>

#define DESKKIN_SERVICE_STACK_SIZE 21504U

extern struct z_thread_stack_element
	service_stack[K_THREAD_STACK_LEN(DESKKIN_SERVICE_STACK_SIZE)];

#endif
