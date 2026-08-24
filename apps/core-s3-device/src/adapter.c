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
#include <zephyr/net/socket.h>
#include <zephyr/net/wifi_mgmt.h>
#include <zephyr/random/random.h>
#include <zephyr/storage/flash_map.h>

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
K_THREAD_STACK_DEFINE(service_stack, 12288);
static struct k_thread service_thread;
K_THREAD_STACK_DEFINE(control_stack, 4096);
static struct k_thread control_thread;

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

extern void rust_main(void);
extern void deskkin_rust_service_worker(void);
extern size_t deskkin_rust_control_snapshot(const uint8_t *input, size_t input_length,
					   uint8_t *output);

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
	return shared_multi_heap_aligned_alloc(SMH_REG_ATTR_EXTERNAL, 32, FRAMEBUFFER_BYTES);
}

int deskkin_display_write(uint16_t x, uint16_t y, uint16_t width, uint16_t height,
			  uint16_t pitch, const uint16_t *pixels)
{
	const struct display_buffer_descriptor descriptor = {
		.buf_size = (size_t)pitch * height * sizeof(uint16_t),
		.pitch = pitch,
		.width = width,
		.height = height,
	};
	return display_write(display, x, y, &descriptor, pixels);
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

static void uart_flush_input(void)
{
	uint8_t ignored;
	while (uart_poll_in(console, &ignored) == 0) {
	}
}

static void uart_write_closed_error(const struct bounded_frame *frame, uint8_t status)
{
	struct bounded_completion completion = {.length = 18};
	completion.bytes[0] = 1;
	completion.bytes[1] = status;
	memcpy(&completion.bytes[2], &frame->bytes[2], 16);
	(void)uart_write_completion(&completion);
}

static void control_entry(void *first, void *second, void *third)
{
	ARG_UNUSED(first);
	ARG_UNUSED(second);
	ARG_UNUSED(third);
	for (;;) {
		uint8_t prefix[2];
		const int64_t prefix_deadline = k_uptime_get() + 2000;
		if (uart_read_byte(&prefix[0], prefix_deadline) != 0 ||
		    uart_read_byte(&prefix[1], prefix_deadline) != 0) {
			continue;
		}
		const size_t length = ((size_t)prefix[0] << 8) | prefix[1];
		if (length < 28 || length > CONTROL_FRAME_MAX) {
			uart_flush_input();
			continue;
		}
		struct bounded_frame frame = {.length = length};
		const int64_t payload_deadline = k_uptime_get() + 2000;
		for (size_t index = 0; index < length; ++index) {
			if (uart_read_byte(&frame.bytes[index], payload_deadline) != 0) {
				frame.length = 0;
				break;
			}
		}
		if (frame.length == 0) {
			memset(&frame, 0, sizeof(frame));
			uart_flush_input();
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
		if (k_msgq_get(&worker_completions, &completion, K_MSEC(5000)) != 0) {
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

int deskkin_wifi_associate(const uint8_t *ssid, uint8_t ssid_length, const uint8_t *psk,
			   uint8_t psk_length)
{
	if (ssid == NULL || psk == NULL || ssid_length == 0 || ssid_length > 32 ||
	    psk_length < 8 || psk_length > 63) {
		return -EINVAL;
	}
	struct net_if *iface = net_if_get_wifi_sta();
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
	return net_mgmt(NET_REQUEST_WIFI_CONNECT, iface, &parameters, sizeof(parameters));
}

int deskkin_wait_dhcp(void)
{
	struct net_if *iface = net_if_get_wifi_sta();
	const int64_t deadline = k_uptime_get() + DHCP_TIMEOUT_MS;
	while (k_uptime_get() < deadline) {
		if (k_msgq_num_used_get(&reserved_control) > 0 ||
		    k_msgq_num_used_get(&application_commands) > 0) {
			return -EINTR;
		}
		if (net_if_ipv4_get_global_addr(iface, NET_ADDR_PREFERRED) != NULL) {
			return 0;
		}
		k_msleep(50);
	}
	return -ETIMEDOUT;
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
		.sin_addr = {.s_addr = htonl(((uint32_t)host[0] << 24) | ((uint32_t)host[1] << 16) |
						 ((uint32_t)host[2] << 8) | host[3])},
	};
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
	       capabilities.current_pixel_format == PIXEL_FORMAT_RGB_565;
}

int main(void)
{
	if (!deskkin_devices_ready()) {
		return 1;
	}
	if (k_thread_create(&control_thread, control_stack, K_THREAD_STACK_SIZEOF(control_stack),
			    control_entry, NULL, NULL, NULL, 4, 0, K_NO_WAIT) == NULL) {
		return 1;
	}
	rust_main();
	return 0;
}
