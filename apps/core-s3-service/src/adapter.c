// SPDX-License-Identifier: GPL-3.0-only

#include <errno.h>
#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>
#include <stdlib.h>
#include <string.h>
#include <zephyr/device.h>
#include <zephyr/drivers/flash.h>
#include <zephyr/drivers/uart.h>
#include <zephyr/kvss/nvs.h>
#include <zephyr/kernel.h>
#include <zephyr/kernel/thread_stack.h>
#include <zephyr/net/net_if.h>
#include <zephyr/net/dhcpv4.h>
#include <zephyr/net/socket.h>
#include <zephyr/net/wifi_mgmt.h>
#include <zephyr/random/random.h>
#include <zephyr/storage/flash_map.h>
#include <zephyr/sys/atomic.h>
#include <zephyr/sys/byteorder.h>
#include <zephyr/multi_heap/shared_multi_heap.h>
#include <esp_attr.h>
#include <esp_memory_utils.h>
#include <rom/ets_sys.h>

#include "adapter.h"
#include "dhcp_wait.h"

#define CONTROL_FRAME_MAX 188
#define COMPLETION_FRAME_MAX 204
#define DIAGNOSTIC_EVENT_SIZE 24
#define WIFI_ASSOCIATION_TIMEOUT_MS 15000
#define DHCP_TIMEOUT_MS 10000

struct bounded_frame {
	uint16_t length;
	uint8_t bytes[CONTROL_FRAME_MAX];
};

struct bounded_completion {
	uint16_t length;
	uint8_t bytes[COMPLETION_FRAME_MAX];
};

K_MSGQ_DEFINE(application_commands, sizeof(struct bounded_frame), 4, 4);
K_MSGQ_DEFINE(reserved_control, sizeof(struct bounded_frame), 1, 4);
K_MSGQ_DEFINE(worker_completions, sizeof(struct bounded_completion), 8, 4);
#define DESKKIN_SRAM2_NOINIT __attribute__((section(".sram2.noinit")))

struct z_thread_stack_element DESKKIN_SRAM2_NOINIT __aligned(ARCH_STACK_PTR_ALIGN)
	service_stack[K_THREAD_STACK_LEN(DESKKIN_SERVICE_STACK_SIZE)];
static struct k_thread service_thread;
extern struct z_thread_stack_element deskkin_control_stack[K_THREAD_STACK_LEN(3072)];
static struct k_thread control_thread;
static const struct device *const console = DEVICE_DT_GET(DT_CHOSEN(zephyr_console));
static struct nvs_fs storage;
static bool storage_ready;
static atomic_t nvs_failure;

enum deskkin_nvs_failure_stage {
	DESKKIN_NVS_FAILURE_NONE = 0,
	DESKKIN_NVS_FAILURE_AREA_OPEN = 1,
	DESKKIN_NVS_FAILURE_PAGE_INFO = 2,
	DESKKIN_NVS_FAILURE_MOUNT = 3,
	DESKKIN_NVS_FAILURE_INTENT_WRITE = 4,
	DESKKIN_NVS_FAILURE_RECORD_WRITE = 5,
	DESKKIN_NVS_FAILURE_INTENT_READBACK = 6,
	DESKKIN_NVS_FAILURE_RECORD_READBACK = 7,
	DESKKIN_NVS_FAILURE_INTENT_DELETE = 8,
	DESKKIN_NVS_FAILURE_INTENT_READ = 9,
	DESKKIN_NVS_FAILURE_RECORD_READ = 10,
};

static void control_entry(void *first, void *second, void *third);

extern void deskkin_rust_service_worker(void);
extern void deskkin_rust_set_boot_status(uint8_t stage, uint8_t error);
extern size_t deskkin_rust_control_snapshot(const uint8_t *input, size_t input_length,
						   uint8_t *output);
extern void deskkin_flash_guard_enter(void);
extern void deskkin_flash_guard_exit(void);
extern void deskkin_debug_pair_request(void);
extern int deskkin_diagnostic_read(uint32_t after_sequence, uint8_t *output, size_t capacity);
extern bool deskkin_runtime_internal_owns(const void *block);

static void set_nvs_failure(enum deskkin_nvs_failure_stage stage, int result)
{
	const uint32_t code = result < 0 ? (uint32_t)(-(int64_t)result) : (uint32_t)result;
	atomic_set(&nvs_failure,
		   (atomic_val_t)(((uint32_t)stage << 8) | MIN(code, UINT8_MAX)));
}

static void clear_nvs_failure(void)
{
	atomic_set(&nvs_failure, DESKKIN_NVS_FAILURE_NONE);
}

uint16_t deskkin_nvs_last_failure(void)
{
	return (uint16_t)atomic_get(&nvs_failure);
}

void *malloc(size_t size)
{
	return shared_multi_heap_alloc(SMH_REG_ATTR_EXTERNAL, MAX(size, 1U));
}

void *calloc(size_t count, size_t size)
{
	size_t bytes;
	if (__builtin_mul_overflow(count, size, &bytes)) {
		return NULL;
	}
	void *const block = malloc(bytes);
	if (block != NULL) {
		memset(block, 0, bytes);
	}
	return block;
}

void *realloc(void *block, size_t size)
{
	if (block == NULL) {
		return malloc(size);
	}
	if (size == 0U) {
		free(block);
		return NULL;
	}
	if (esp_ptr_in_dram(block)) {
		return k_realloc(block, size);
	}
	return shared_multi_heap_realloc(SMH_REG_ATTR_EXTERNAL, block, size);
}

void free(void *block)
{
	if (block == NULL) {
		return;
	}
	if (deskkin_runtime_internal_owns(block)) {
		shared_multi_heap_free(block);
		return;
	}
	if (esp_ptr_in_dram(block)) {
		k_free(block);
	} else if (esp_ptr_external_ram(block)) {
		shared_multi_heap_free(block);
	} else {
		ets_printf("deskkin rejected invalid free %p\n", block);
	}
}

void deskkin_boot_trace(uint8_t stage, uint8_t error)
{
	deskkin_rust_set_boot_status(stage, error);
}


int deskkin_csrand(uint8_t *output, size_t length)
{
	if (output == NULL || length == 0 || length > CONTROL_FRAME_MAX) {
		return -EINVAL;
	}
	return sys_csrand_get(output, length);
}

static void service_entry(void *first, void *second, void *third)
{
	ARG_UNUSED(first);
	ARG_UNUSED(second);
	ARG_UNUSED(third);
	deskkin_rust_service_worker();
}

int deskkin_start_service_worker(void)
{
	k_tid_t thread = k_thread_create(&service_thread, service_stack,
					 K_THREAD_STACK_SIZEOF(service_stack), service_entry, NULL,
					 NULL, NULL, 5, 0, K_NO_WAIT);
	return thread == NULL ? -EIO : 0;
}

int deskkin_start_control_worker(void)
{
	if (!device_is_ready(console)) {
		return -ENODEV;
	}
	k_tid_t thread = k_thread_create(&control_thread, deskkin_control_stack,
					 K_THREAD_STACK_SIZEOF(deskkin_control_stack), control_entry, NULL,
					 NULL, NULL, -1, 0, K_NO_WAIT);
	return thread == NULL ? -EIO : 0;
}

int deskkin_service_take_command(uint8_t *output, size_t capacity)
{
	struct bounded_frame frame;
	int result = k_msgq_get(&reserved_control, &frame, K_NO_WAIT);
	if (result != 0) {
		result = k_msgq_get(&application_commands, &frame, K_MSEC(10));
	}
	if (result != 0 || output == NULL || frame.length > capacity) {
		memset(&frame, 0, sizeof(frame));
		return -EMSGSIZE;
	}
	memcpy(output, frame.bytes, frame.length);
	const int length = frame.length;
	memset(&frame, 0, sizeof(frame));
	return length;
}

void deskkin_sleep_ms(uint32_t delay_ms)
{
	k_msleep(delay_ms);
}

int deskkin_service_publish_completion(const uint8_t *input, size_t length)
{
	if (input == NULL || length > COMPLETION_FRAME_MAX) {
		return -EMSGSIZE;
	}
	struct bounded_completion completion = {.length = length};
	memcpy(completion.bytes, input, length);
	return k_msgq_put(&worker_completions, &completion, K_NO_WAIT);
}

int deskkin_control_submit(const uint8_t *input, size_t length, bool reserved)
{
	if (input == NULL || length > CONTROL_FRAME_MAX) {
		return -EMSGSIZE;
	}
	struct bounded_frame frame = {.length = length};
	memcpy(frame.bytes, input, length);
	return k_msgq_put(reserved ? &reserved_control : &application_commands, &frame,
			  K_NO_WAIT);
}

int deskkin_control_take_completion(uint8_t *output, size_t capacity)
{
	struct bounded_completion completion;
	const int result = k_msgq_get(&worker_completions, &completion, K_MSEC(5000));
	if (result != 0 || output == NULL || completion.length > capacity) {
		return -EMSGSIZE;
	}
	memcpy(output, completion.bytes, completion.length);
	return completion.length;
}

static int uart_read_byte(uint8_t *byte, int64_t deadline)
{
	while (k_uptime_get() < deadline) {
		if (uart_poll_in(console, byte) == 0) {
			return 0;
		}
		k_msleep(1);
	}
	return -ETIMEDOUT;
}

static int uart_write_completion(const struct bounded_completion *completion)
{
	const int64_t deadline = k_uptime_get() + 2000;
	if (k_uptime_get() >= deadline) {
		return -ETIMEDOUT;
	}
	uart_poll_out(console, (uint8_t)(completion->length >> 8));
	uart_poll_out(console, (uint8_t)completion->length);
	for (size_t index = 0; index < completion->length; ++index) {
		if (k_uptime_get() >= deadline) {
			return -ETIMEDOUT;
		}
		uart_poll_out(console, completion->bytes[index]);
	}
	return 0;
}

static int uart_read_frame(struct bounded_frame *frame)
{
	uint8_t prefix[3] = {0};
	size_t prefix_length = 0;
	const int64_t prefix_deadline = k_uptime_get() + 2000;
	while (k_uptime_get() < prefix_deadline) {
		uint8_t byte;
		if (uart_read_byte(&byte, prefix_deadline) != 0) {
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
		const size_t length = ((size_t)prefix[0] << 8) | prefix[1];
		if (length < 28 || length > CONTROL_FRAME_MAX || prefix[2] != 1U) {
			continue;
		}
		frame->length = length;
		frame->bytes[0] = prefix[2];
		const int64_t payload_deadline = k_uptime_get() + 2000;
		for (size_t index = 1; index < length; ++index) {
			if (uart_read_byte(&frame->bytes[index], payload_deadline) != 0) {
				memset(frame, 0, sizeof(*frame));
				return -ETIMEDOUT;
			}
		}
		return 0;
	}
	return -ETIMEDOUT;
}

static void uart_write_closed_error(const struct bounded_frame *frame, uint8_t status)
{
	struct bounded_completion completion = {.length = 18};
	completion.bytes[0] = 1;
	completion.bytes[1] = status;
	memcpy(&completion.bytes[2], &frame->bytes[2], 16);
	(void)uart_write_completion(&completion);
}

static int take_matching_completion(const struct bounded_frame *frame,
				    struct bounded_completion *completion)
{
	const int64_t deadline = k_uptime_get() + 5000;
	while (k_uptime_get() < deadline) {
		const int64_t remaining = deadline - k_uptime_get();
		if (k_msgq_get(&worker_completions, completion, K_MSEC(remaining)) != 0) {
			return -ETIMEDOUT;
		}
		if (completion->length >= 18 &&
		    memcmp(&completion->bytes[2], &frame->bytes[2], 16) == 0) {
			return 0;
		}
		memset(completion, 0, sizeof(*completion));
	}
	return -ETIMEDOUT;
}

static void control_entry(void *first, void *second, void *third)
{
	ARG_UNUSED(first);
	ARG_UNUSED(second);
	ARG_UNUSED(third);
	for (;;) {
		struct bounded_frame frame = {0};
		if (uart_read_frame(&frame) != 0) {
			continue;
		}
		if (frame.bytes[1] == 11U && (frame.length == 30U || frame.length == 31U)) {
			const uint16_t seconds = ((uint16_t)frame.bytes[28] << 8) | frame.bytes[29];
			const bool auto_pair = frame.length == 31U && frame.bytes[30] == 1U;
			if (seconds == 0U || seconds > 300U ||
			    (frame.length == 31U && frame.bytes[30] > 1U)) {
				uart_write_closed_error(&frame, 4);
				continue;
			}
			struct bounded_completion acknowledgement = {.length = 18};
			acknowledgement.bytes[0] = 1U;
			acknowledgement.bytes[1] = 0U;
			memcpy(&acknowledgement.bytes[2], &frame.bytes[2], 16);
			(void)uart_write_completion(&acknowledgement);
			k_msleep(100);
			if (auto_pair) {
				deskkin_debug_pair_request();
			}
			uint32_t cursor = 0U;
			const int64_t deadline = k_uptime_get() + (int64_t)seconds * 1000;
			while (k_uptime_get() < deadline) {
				struct bounded_completion event = {0};
				const int length = deskkin_diagnostic_read(
					cursor, event.bytes, sizeof(event.bytes));
				if (length <= 0) {
					k_msleep(5);
					continue;
				}
				event.length = (uint16_t)length;
				cursor = sys_get_be32(&event.bytes[4]);
				(void)uart_write_completion(&event);
			}
			continue;
		}
		struct k_msgq *queue = frame.bytes[1] == 9 ? &reserved_control : &application_commands;
		if (frame.bytes[1] == 2 || frame.bytes[1] == 5 || frame.bytes[1] == 8) {
			struct bounded_completion completion;
			completion.length = deskkin_rust_control_snapshot(frame.bytes, frame.length,
								   completion.bytes);
			if (completion.length == 0 || completion.length > sizeof(completion.bytes)) {
				continue;
			}
			(void)uart_write_completion(&completion);
			memset(&frame, 0, sizeof(frame));
			memset(&completion, 0, sizeof(completion));
			continue;
		}
		if (k_msgq_put(queue, &frame, K_MSEC(2000)) != 0) {
			uart_write_closed_error(&frame, 8);
			memset(&frame, 0, sizeof(frame));
			continue;
		}
		struct bounded_completion completion;
		if (take_matching_completion(&frame, &completion) != 0) {
			k_msgq_purge(queue);
			uart_write_closed_error(&frame, 8);
			memset(&frame, 0, sizeof(frame));
			continue;
		}
		(void)uart_write_completion(&completion);
		memset(&frame, 0, sizeof(frame));
		memset(&completion, 0, sizeof(completion));
	}
}

uint64_t deskkin_uptime_ms(void)
{
	return (uint64_t)k_uptime_get();
}

static int ensure_storage(void)
{
	if (storage_ready) {
		return 0;
	}
	deskkin_flash_guard_enter();
	const struct flash_area *area;
	int result = flash_area_open(PARTITION_ID(storage_partition), &area);
	if (result != 0) {
		set_nvs_failure(DESKKIN_NVS_FAILURE_AREA_OPEN, result);
		deskkin_flash_guard_exit();
		return result;
	}
	storage.flash_device = area->fa_dev;
	storage.offset = area->fa_off;
	struct flash_pages_info page;
	result = flash_get_page_info_by_offs(storage.flash_device, storage.offset, &page);
	if (result == 0) {
		storage.sector_size = page.size;
		storage.sector_count = area->fa_size / page.size;
		result = nvs_mount(&storage);
		if (result != 0) {
			set_nvs_failure(DESKKIN_NVS_FAILURE_MOUNT, result);
		}
	} else {
		set_nvs_failure(DESKKIN_NVS_FAILURE_PAGE_INFO, result);
	}
	flash_area_close(area);
	storage_ready = result == 0;
	deskkin_flash_guard_exit();
	return result;
}

int deskkin_nvs_read(uint16_t record_id, uint8_t *output, size_t capacity)
{
	const int result = ensure_storage();
	if (result != 0 || output == NULL || capacity > CONTROL_FRAME_MAX) {
		return result != 0 ? result : -EINVAL;
	}
	deskkin_flash_guard_enter();
	const int length = nvs_read(&storage, record_id, output, capacity);
	deskkin_flash_guard_exit();
	if (length < 0 && length != -ENOENT) {
		set_nvs_failure(record_id == 0x102 || record_id == 0x202
					? DESKKIN_NVS_FAILURE_INTENT_READ
					: DESKKIN_NVS_FAILURE_RECORD_READ,
				length);
	}
	return length == -ENOENT ? 0 : length < 0 ? length : length + 1;
}

int deskkin_nvs_write_readback(uint16_t record_id, const uint8_t *input, size_t length)
{
	uint8_t readback[CONTROL_FRAME_MAX];
	clear_nvs_failure();
	int result = ensure_storage();
	if (result != 0 || input == NULL || length > sizeof(readback)) {
		result = result != 0 ? result : -EINVAL;
		goto cleanup;
	}
	deskkin_flash_guard_enter();
	result = nvs_write(&storage, record_id, input, length);
	if (result < 0 || (size_t)result != length) {
		result = result < 0 ? result : -EIO;
		set_nvs_failure(record_id == 0x102 || record_id == 0x202
					? DESKKIN_NVS_FAILURE_INTENT_WRITE
					: DESKKIN_NVS_FAILURE_RECORD_WRITE,
				result);
		goto guarded_cleanup;
	}
	result = nvs_read(&storage, record_id, readback, sizeof(readback));
	if (result < 0 || (size_t)result != length || memcmp(input, readback, length) != 0) {
		if (result >= 0) {
			result = -EIO;
		}
		set_nvs_failure(record_id == 0x102 || record_id == 0x202
					? DESKKIN_NVS_FAILURE_INTENT_READBACK
					: DESKKIN_NVS_FAILURE_RECORD_READBACK,
				result);
		goto guarded_cleanup;
	}
	result = 0;
guarded_cleanup:
	deskkin_flash_guard_exit();
cleanup:
	memset(readback, 0, sizeof(readback));
	return result;
}

int deskkin_nvs_delete(uint16_t record_id)
{
	clear_nvs_failure();
	const int result = ensure_storage();
	if (result != 0) {
		return result;
	}
	deskkin_flash_guard_enter();
	const int delete_result = nvs_delete(&storage, record_id);
	deskkin_flash_guard_exit();
	if (delete_result != 0) {
		set_nvs_failure(DESKKIN_NVS_FAILURE_INTENT_DELETE, delete_result);
	}
	return delete_result;
}

static int wifi_connection_state(struct net_if *iface)
{
	struct wifi_iface_status status = {0};
	const int result =
		net_mgmt(NET_REQUEST_WIFI_IFACE_STATUS, iface, &status, sizeof(status));
	return result == 0 ? status.state : result;
}

int deskkin_wifi_disconnect(void)
{
	struct net_if *iface = net_if_get_wifi_sta();
	if (iface == NULL) {
		return -ENODEV;
	}
	net_dhcpv4_stop(iface);
	const int result = net_mgmt(NET_REQUEST_WIFI_DISCONNECT, iface, NULL, 0);
	return result == -EALREADY ? 0 : result;
}

int deskkin_wifi_associate(const uint8_t *ssid, uint8_t ssid_length, const uint8_t *psk,
			   uint8_t psk_length)
{
	if (ssid == NULL || psk == NULL || ssid_length == 0 || ssid_length > 32 ||
	    psk_length < 8 || psk_length > 63) {
		return -EINVAL;
	}
	struct net_if *iface = net_if_get_wifi_sta();
	if (iface == NULL) {
		return -ENODEV;
	}
	const struct device *const wifi = net_if_get_device(iface);
	if (wifi == NULL) {
		return -ENODEV;
	}
	if (!device_is_ready(wifi)) {
		return wifi->state->initialized && wifi->state->init_res != 0U
			       ? -(int)wifi->state->init_res
			       : -ENXIO;
	}
	const int up_result = net_if_up(iface);
	if (up_result != 0 && up_result != -EALREADY) {
		return up_result;
	}
	const int64_t deadline = k_uptime_get() + WIFI_ASSOCIATION_TIMEOUT_MS;
	if (wifi_connection_state(iface) == WIFI_STATE_COMPLETED) {
		const int result = deskkin_wifi_disconnect();
		if (result != 0) {
			return result;
		}
		while (k_uptime_get() < deadline) {
			const int state = wifi_connection_state(iface);
			if (state < 0) {
				return state;
			}
			if (state <= WIFI_STATE_INACTIVE) {
				break;
			}
			k_msleep(50);
		}
		if (wifi_connection_state(iface) > WIFI_STATE_INACTIVE) {
			return -ETIMEDOUT;
		}
	}
	struct wifi_connect_req_params parameters = {
		.ssid = ssid,
		.ssid_length = ssid_length,
		.psk = psk,
		.psk_length = psk_length,
		.band = WIFI_FREQ_BAND_2_4_GHZ,
		.channel = WIFI_CHANNEL_ANY,
		.security = WIFI_SECURITY_TYPE_PSK,
		.mfp = WIFI_MFP_OPTIONAL,
		.timeout = WIFI_ASSOCIATION_TIMEOUT_MS,
	};
	bool connect_requested = false;
	while (k_uptime_get() < deadline) {
		if (k_msgq_num_used_get(&reserved_control) > 0 ||
		    k_msgq_num_used_get(&application_commands) > 0) {
			return -EINTR;
		}
		const int state = wifi_connection_state(iface);
		if (connect_requested && state == WIFI_STATE_COMPLETED) {
			return 0;
		}
		if (state == -ENETDOWN || state == -EINPROGRESS || state == -EAGAIN ||
		    state == -EBUSY) {
			k_msleep(50);
			continue;
		}
		if (state < 0) {
			return state;
		}
		if (connect_requested && state < WIFI_STATE_SCANNING) {
			connect_requested = false;
		}
		if (!connect_requested && state >= WIFI_STATE_SCANNING) {
			(void)deskkin_wifi_disconnect();
			k_msleep(50);
			continue;
		}
		if (!connect_requested && state < WIFI_STATE_SCANNING) {
			const int result = net_mgmt(NET_REQUEST_WIFI_CONNECT, iface, &parameters,
						   sizeof(parameters));
			if (result == 0 || result == -EALREADY || result == -EINPROGRESS) {
				connect_requested = true;
			} else if (result != -EAGAIN && result != -EIO && result != -EBUSY) {
				return result;
			}
		}
		k_msleep(50);
	}
	return -ETIMEDOUT;
}

int deskkin_wait_dhcp(void)
{
	struct net_if *iface = net_if_get_wifi_sta();
	if (iface == NULL) {
		return -ENODEV;
	}
	if (iface->config.dhcpv4.state != NET_DHCPV4_BOUND) {
		net_dhcpv4_start(iface);
	}
	const int64_t deadline = k_uptime_get() + DHCP_TIMEOUT_MS;
	for (;;) {
		const enum deskkin_dhcp_wait_decision decision =
			deskkin_dhcp_wait_decide(
				k_msgq_num_used_get(&reserved_control) > 0 ||
					k_msgq_num_used_get(&application_commands) > 0,
				net_if_ipv4_get_global_addr(iface, NET_ADDR_PREFERRED) != NULL,
				k_uptime_get() >= deadline);
		if (decision == DESKKIN_DHCP_WAIT_CANCELLED) {
			return -EINTR;
		}
		if (decision == DESKKIN_DHCP_WAIT_READY) {
			return 0;
		}
		if (decision == DESKKIN_DHCP_WAIT_TIMED_OUT) {
			return -ETIMEDOUT;
		}
		k_msleep(50);
	}
}

int deskkin_tcp_connect(const uint8_t host[4], uint16_t port)
{
	if (host == NULL || port != 39042) {
		return -EINVAL;
	}
	const int descriptor = zsock_socket(AF_INET, SOCK_STREAM, IPPROTO_TCP);
	if (descriptor < 0) {
		return -errno;
	}
	struct timeval timeout = {.tv_sec = 2, .tv_usec = 0};
	(void)zsock_setsockopt(descriptor, SOL_SOCKET, SO_RCVTIMEO, &timeout, sizeof(timeout));
	(void)zsock_setsockopt(descriptor, SOL_SOCKET, SO_SNDTIMEO, &timeout, sizeof(timeout));
	struct sockaddr_in address = {
		.sin_family = AF_INET,
		.sin_port = htons(port),
	};
	memcpy(&address.sin_addr.s_addr, host, sizeof(address.sin_addr.s_addr));
	if (zsock_connect(descriptor, (struct sockaddr *)&address, sizeof(address)) != 0) {
		const int error = -errno;
		(void)zsock_close(descriptor);
		return error;
	}
	return descriptor;
}

int deskkin_tcp_set_timeout(int descriptor, uint32_t timeout_ms)
{
	if (descriptor < 0 || timeout_ms == 0) {
		return -EINVAL;
	}
	struct timeval timeout = {
		.tv_sec = timeout_ms / 1000,
		.tv_usec = (timeout_ms % 1000) * 1000,
	};
	int result = zsock_setsockopt(descriptor, SOL_SOCKET, SO_RCVTIMEO, &timeout,
				      sizeof(timeout));
	if (result == 0) {
		result = zsock_setsockopt(descriptor, SOL_SOCKET, SO_SNDTIMEO, &timeout,
					  sizeof(timeout));
	}
	return result == 0 ? 0 : -errno;
}

int deskkin_tcp_read(int descriptor, uint8_t *output, size_t length)
{
	return zsock_recv(descriptor, output, length, 0);
}

int deskkin_tcp_write(int descriptor, const uint8_t *input, size_t length)
{
	return zsock_send(descriptor, input, length, 0);
}

int deskkin_tcp_close(int descriptor)
{
	return zsock_close(descriptor);
}
