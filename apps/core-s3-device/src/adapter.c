// SPDX-License-Identifier: GPL-3.0-only

#include <errno.h>
#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>
#include <string.h>
#include <zephyr/device.h>
#include <zephyr/drivers/display.h>
#include <zephyr/drivers/flash.h>
#include <zephyr/drivers/uart.h>
#include <zephyr/kvss/nvs.h>
#include <zephyr/input/input.h>
#include <zephyr/kernel.h>
#include <zephyr/multi_heap/shared_multi_heap.h>
#include <zephyr/net/net_if.h>
#include <zephyr/net/dhcpv4.h>
#include <zephyr/net/socket.h>
#include <zephyr/net/wifi_mgmt.h>
#include <zephyr/random/random.h>
#include <zephyr/storage/flash_map.h>
#include <zephyr/sys/atomic.h>
#include <zephyr/sys/byteorder.h>

#include "dhcp_wait.h"

#define CONTROL_FRAME_MAX 188
#define COMPLETION_FRAME_MAX 80
#define WIFI_ASSOCIATION_TIMEOUT_MS 15000
#define DHCP_TIMEOUT_MS 10000
#define FRAMEBUFFER_BYTES (320U * 240U * sizeof(uint16_t))

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
K_THREAD_STACK_DEFINE(service_stack, 24576);
static struct k_thread service_thread;
K_THREAD_STACK_DEFINE(control_stack, 4096);
static struct k_thread control_thread;
K_THREAD_STACK_DEFINE(display_stack, 4096);
static struct k_thread display_thread;

struct display_request {
	uint8_t buffer_index;
};

struct display_completion {
	uint8_t buffer_index;
	int8_t result;
	uint32_t duration_us;
	uint64_t completed_at_us;
};

K_MSGQ_DEFINE(display_requests, sizeof(struct display_request), 1, 4);
K_MSGQ_DEFINE(display_completions, sizeof(struct display_completion), 1, 4);

static const struct device *const display = DEVICE_DT_GET(DT_CHOSEN(zephyr_display));
static const struct device *const console = DEVICE_DT_GET(DT_CHOSEN(zephyr_console));
static const struct device *const touch = DEVICE_DT_GET(DT_CHOSEN(zephyr_touch));
static struct nvs_fs storage;
static bool storage_ready;
static struct k_spinlock touch_lock;
static int32_t touch_x;
static int32_t touch_y;
static uint32_t touch_generation;
static uint32_t consumed_touch_generation;
static uint16_t *framebuffers[2];
static atomic_t allocation_failures;
static atomic_t control_trace;

extern void rust_main(void);
extern void deskkin_rust_service_worker(void);
extern void deskkin_rust_set_boot_status(uint8_t stage, uint8_t error);
extern size_t deskkin_rust_control_snapshot(const uint8_t *input, size_t input_length,
						   uint8_t *output);
uint16_t *deskkin_framebuffer_alloc(uint8_t index);
uint8_t deskkin_allocation_failures(void);
static void display_boot_stage(uint8_t stage, uint8_t error);

static void record_allocation_failure(void)
{
	atomic_val_t current = atomic_get(&allocation_failures);
	while (current < UINT8_MAX &&
	       !atomic_cas(&allocation_failures, current, current + 1)) {
		current = atomic_get(&allocation_failures);
	}
}

static void publish_boot_status(uint8_t stage, uint8_t error)
{
	deskkin_rust_set_boot_status(stage, error);
}

void deskkin_boot_trace(uint8_t stage, uint8_t error)
{
	display_boot_stage(stage, error);
}

static void display_boot_color(uint16_t color)
{
	if (!device_is_ready(display)) {
		return;
	}
	uint16_t *pixels = deskkin_framebuffer_alloc(0);
	if (pixels == NULL) {
		return;
	}
	for (size_t index = 0; index < 320U * 240U; ++index) {
		pixels[index] = sys_cpu_to_be16(color);
	}
	const struct display_buffer_descriptor descriptor = {
		.buf_size = FRAMEBUFFER_BYTES,
		.width = 320,
		.height = 240,
		.pitch = 320,
	};
	if (display_write(display, 0, 0, &descriptor, pixels) != 0) {
		return;
	}
	(void)display_blanking_off(display);
}

static void display_boot_stage(uint8_t stage, uint8_t error)
{
	if (error != 0U) {
		display_boot_color(0xF800);
		return;
	}
	if (stage < 4U || stage > 8U || !device_is_ready(display)) {
		return;
	}
	uint16_t *pixels = deskkin_framebuffer_alloc(0);
	if (pixels == NULL) {
		return;
	}
	for (size_t y = 0; y < 240U; ++y) {
		for (size_t x = 0; x < 320U; ++x) {
			const size_t block = x / 32U;
			const bool filled = block < stage;
			const bool separator = x % 32U >= 28U;
			pixels[y * 320U + x] = sys_cpu_to_be16(filled && !separator ? 0xFFFF : 0x0000);
		}
	}
	const struct display_buffer_descriptor descriptor = {
		.buf_size = FRAMEBUFFER_BYTES,
		.width = 320,
		.height = 240,
		.pitch = 320,
	};
	(void)display_write(display, 0, 0, &descriptor, pixels);
	(void)display_blanking_off(display);
}

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

uint16_t *deskkin_framebuffer_alloc(uint8_t index)
{
	if (index >= ARRAY_SIZE(framebuffers)) {
		return NULL;
	}
	if (framebuffers[index] == NULL) {
		framebuffers[index] = shared_multi_heap_aligned_alloc(SMH_REG_ATTR_EXTERNAL, 32,
								     FRAMEBUFFER_BYTES);
		if (framebuffers[index] != NULL) {
			memset(framebuffers[index], 0, FRAMEBUFFER_BYTES);
		} else {
			record_allocation_failure();
		}
	}
	return framebuffers[index];
}

uint8_t deskkin_allocation_failures(void)
{
	return (uint8_t)atomic_get(&allocation_failures);
}

uint8_t deskkin_control_trace(void)
{
	return (uint8_t)atomic_get(&control_trace);
}

static int display_full_frame(const uint16_t *pixels)
{
	const struct display_buffer_descriptor descriptor = {
		.buf_size = FRAMEBUFFER_BYTES,
		.pitch = 320,
		.width = 320,
		.height = 240,
	};
	return display_write(display, 0, 0, &descriptor, pixels);
}

static void display_entry(void *first, void *second, void *third)
{
	ARG_UNUSED(first);
	ARG_UNUSED(second);
	ARG_UNUSED(third);
	for (;;) {
		struct display_request request;
		if (k_msgq_get(&display_requests, &request, K_FOREVER) != 0) {
			continue;
		}
		const uint64_t started_cycles = k_cycle_get_64();
		const int result = display_full_frame(framebuffers[request.buffer_index]);
		const uint64_t completed_cycles = k_cycle_get_64();
		const struct display_completion completion = {
			.buffer_index = request.buffer_index,
			.result = result == 0 ? 0 : -1,
			.duration_us = (uint32_t)MIN(k_cyc_to_us_floor64(completed_cycles - started_cycles),
						    UINT32_MAX),
			.completed_at_us = k_ticks_to_us_floor64(k_uptime_ticks()),
		};
		(void)k_msgq_put(&display_completions, &completion, K_FOREVER);
	}
}

int deskkin_display_submit(uint8_t buffer_index)
{
	if (buffer_index >= ARRAY_SIZE(framebuffers) || framebuffers[buffer_index] == NULL) {
		return -EINVAL;
	}
	const struct display_request request = {.buffer_index = buffer_index};
	return k_msgq_put(&display_requests, &request, K_NO_WAIT);
}

int deskkin_display_take_completion(uint8_t *buffer_index, uint32_t *duration_us,
				    uint64_t *completed_at_us)
{
	struct display_completion completion;
	if (k_msgq_get(&display_completions, &completion, K_NO_WAIT) != 0) {
		return 0;
	}
	*buffer_index = completion.buffer_index;
	*duration_us = completion.duration_us;
	*completed_at_us = completion.completed_at_us;
	return completion.result == 0 ? 1 : -EIO;
}

int deskkin_display_enable(void)
{
	const int result = display_blanking_off(display);
	return result == -ENOSYS ? 0 : result;
}

bool deskkin_take_touch(int32_t *x, int32_t *y)
{
	bool available = false;
	k_spinlock_key_t key = k_spin_lock(&touch_lock);
	if (touch_generation != consumed_touch_generation) {
		consumed_touch_generation = touch_generation;
		*x = touch_x;
		*y = touch_y;
		available = true;
	}
	k_spin_unlock(&touch_lock, key);
	return available;
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
		atomic_set(&control_trace, 1);
		struct k_msgq *queue = frame.bytes[1] == 9 ? &reserved_control : &application_commands;
		if (frame.bytes[1] == 2 || frame.bytes[1] == 5 || frame.bytes[1] == 8 ||
		    frame.bytes[1] == 11) {
			struct bounded_completion completion;
			completion.length = deskkin_rust_control_snapshot(frame.bytes, frame.length,
								   completion.bytes);
			if (completion.length == 0 || completion.length > sizeof(completion.bytes)) {
				continue;
			}
			atomic_set(&control_trace, 2);
			if (uart_write_completion(&completion) == 0) {
				atomic_set(&control_trace, 3);
			}
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
	const struct flash_area *area;
	int result = flash_area_open(PARTITION_ID(storage_partition), &area);
	if (result != 0) {
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
	}
	flash_area_close(area);
	storage_ready = result == 0;
	return result;
}

int deskkin_nvs_read(uint16_t record_id, uint8_t *output, size_t capacity)
{
	const int result = ensure_storage();
	if (result != 0 || output == NULL || capacity > CONTROL_FRAME_MAX) {
		return result != 0 ? result : -EINVAL;
	}
	const int length = nvs_read(&storage, record_id, output, capacity);
	return length == -ENOENT ? 0 : length < 0 ? length : length + 1;
}

int deskkin_nvs_write_readback(uint16_t record_id, const uint8_t *input, size_t length)
{
	uint8_t readback[CONTROL_FRAME_MAX];
	int result = ensure_storage();
	if (result != 0 || input == NULL || length > sizeof(readback)) {
		result = result != 0 ? result : -EINVAL;
		goto cleanup;
	}
	result = nvs_write(&storage, record_id, input, length);
	if (result < 0 || (size_t)result != length) {
		result = result < 0 ? result : -EIO;
		goto cleanup;
	}
	result = nvs_read(&storage, record_id, readback, sizeof(readback));
	if (result < 0 || (size_t)result != length || memcmp(input, readback, length) != 0) {
		result = -EIO;
		goto cleanup;
	}
	result = 0;
cleanup:
	memset(readback, 0, sizeof(readback));
	return result;
}

int deskkin_nvs_delete(uint16_t record_id)
{
	const int result = ensure_storage();
	return result == 0 ? nvs_delete(&storage, record_id) : result;
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
			if (result == 0 || result == -EALREADY) {
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

bool deskkin_devices_ready(void)
{
	struct display_capabilities capabilities;
	if (!device_is_ready(console) || !device_is_ready(display) || !device_is_ready(touch)) {
		return false;
	}
	display_get_capabilities(display, &capabilities);
	return capabilities.x_resolution == 320 && capabilities.y_resolution == 240 &&
	       capabilities.current_pixel_format == PIXEL_FORMAT_RGB_565X;
}

int main(void)
{
	if (!device_is_ready(console)) {
		display_boot_color(0xF800); /* red: USB control unavailable */
		return 1;
	}
	if (k_thread_create(&control_thread, control_stack, K_THREAD_STACK_SIZEOF(control_stack),
			    control_entry, NULL, NULL, NULL, -1, 0, K_NO_WAIT) == NULL) {
		display_boot_color(0xF800);
		return 1;
	}
	publish_boot_status(2, 0);
	display_boot_color(0xFD20); /* orange: USB control thread started */
	if (!deskkin_devices_ready()) {
		display_boot_color(0xF800);
		publish_boot_status(2, 1);
		for (;;) {
			k_sleep(K_FOREVER);
		}
	}
	publish_boot_status(3, 0);
	display_boot_color(0x001F); /* blue: platform devices ready */
	if (k_thread_create(&display_thread, display_stack, K_THREAD_STACK_SIZEOF(display_stack),
			    display_entry, NULL, NULL, NULL, 0, 0, K_NO_WAIT) == NULL) {
		display_boot_color(0xF800);
		return 1;
	}
	rust_main();
	return 0;
}
