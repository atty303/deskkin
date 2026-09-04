// SPDX-License-Identifier: MIT

#ifndef DESKKIN_CORE_S3_AMP_SHARED_H
#define DESKKIN_CORE_S3_AMP_SHARED_H

#include <stdint.h>
#include <stddef.h>

#define DESKKIN_HEARTBEAT_MAGIC 0x44534b4eU
#define DESKKIN_DISPLAY_MAGIC 0x4453504cU
#define DESKKIN_WORLD_MAGIC 0x4453574cU
#define DESKKIN_RUNTIME_SRAM_MAGIC 0x4453524dU
#define DESKKIN_WORLD_SCHEMA 1U
#define DESKKIN_CHANNEL_SCHEMA 1U
#define DESKKIN_TOUCH_CAPACITY 16U
#define DESKKIN_SHARED_SIZE 0x1000U
#define DESKKIN_CHANNEL_OFFSET 0x400U

static inline void deskkin_shared_fence(void)
{
	__asm__ volatile("" ::: "memory");
}

static inline uint32_t deskkin_shared_load(const volatile uint32_t *value)
{
	const uint32_t loaded = *value;
	deskkin_shared_fence();
	return loaded;
}

static inline void deskkin_shared_store(volatile uint32_t *target, uint32_t value)
{
	deskkin_shared_fence();
	*target = value;
	deskkin_shared_fence();
}

static inline __attribute__((always_inline)) void
deskkin_shared_copy_to(volatile void *target, const void *source, size_t size)
{
	volatile uint8_t *output = target;
	const uint8_t *input = source;
	for (size_t index = 0; index < size; ++index) {
		output[index] = input[index];
	}
	deskkin_shared_fence();
}

static inline __attribute__((always_inline)) void
deskkin_shared_copy_from(void *target, const volatile void *source, size_t size)
{
	uint8_t *output = target;
	const volatile uint8_t *input = source;
	for (size_t index = 0; index < size; ++index) {
		output[index] = input[index];
	}
	deskkin_shared_fence();
}

struct deskkin_renderer_heartbeat {
	uint32_t magic;
	uint32_t generation;
	uint32_t completed_frames;
	uint32_t render_us;
	uint32_t transfer_us;
	uint32_t copy_us;
	uint32_t render_max_us;
	uint32_t transfer_max_us;
	uint8_t stage;
	uint8_t fault;
	uint8_t allocation_failures;
	uint8_t transfer_failures;
	uint8_t dirty_rect_count;
	uint8_t schema;
	uint16_t pixel_dma_batches;
	uint32_t dirty_pixels;
	uint32_t transferred_bytes;
	uint32_t view_generation;
	uint32_t pose_generation;
	uint32_t input_generation;
	uint32_t stale_snapshots;
	uint32_t touch_drops;
	uint16_t atlas_cache_hits;
	uint16_t atlas_cache_misses;
	uint16_t atlas_cache_failures;
	uint8_t visible_billboards;
	uint8_t culled_billboards;
	uint8_t observed_shell;
	uint8_t shell_property_matches;
	uint32_t nearest_samples;
	uint32_t bilinear_samples;
	uint32_t projection_us;
	uint32_t projection_max_us;
	uint32_t sort_us;
	uint32_t sort_max_us;
	uint32_t texture_us;
	uint32_t texture_max_us;
	uint32_t world_raster_us;
	uint32_t world_raster_max_us;
	uint32_t deadline_misses;
};

struct deskkin_display_ready {
	uint32_t magic;
	uint32_t generation;
	uint32_t ready;
	uint32_t framebuffer;
	uint32_t renderer_heap;
	uint32_t renderer_heap_size;
};

struct deskkin_runtime_sram_handoff {
	uint32_t magic;
	uint32_t generation;
	uint32_t address;
	uint32_t size;
	uint32_t used;
};

enum deskkin_shell_state {
	DESKKIN_SHELL_SETUP_REQUIRED = 0,
	DESKKIN_SHELL_READY_TO_PAIR = 1,
	DESKKIN_SHELL_CONNECTING = 2,
	DESKKIN_SHELL_PAIRING_CONFIRMATION = 3,
	DESKKIN_SHELL_PAIRED = 4,
};

enum deskkin_renderer_progress_stage {
	DESKKIN_RENDER_PROGRESS_LOOP = 1,
	DESKKIN_RENDER_PROGRESS_SNAPSHOT = 2,
	DESKKIN_RENDER_PROGRESS_TOUCH = 3,
	DESKKIN_RENDER_PROGRESS_TEXTURE = 4,
	DESKKIN_RENDER_PROGRESS_BUFFER = 5,
	DESKKIN_RENDER_PROGRESS_RASTER = 6,
	DESKKIN_RENDER_PROGRESS_SUBMIT = 7,
	DESKKIN_RENDER_PROGRESS_PACING = 8,
};

enum deskkin_display_progress_stage {
	DESKKIN_DISPLAY_PROGRESS_WAITING = 1,
	DESKKIN_DISPLAY_PROGRESS_REQUEST = 2,
	DESKKIN_DISPLAY_PROGRESS_TRANSFER = 3,
	DESKKIN_DISPLAY_PROGRESS_COMPLETION = 4,
};

struct deskkin_world_snapshot {
	uint32_t magic;
	uint32_t generation;
	int64_t observed_yaw;
	uint32_t sas;
	uint8_t schema;
	uint8_t shell;
	uint8_t availability;
	uint8_t notice;
};

struct deskkin_touch_sample {
	uint32_t publication;
	uint32_t generation;
	int16_t x;
	int16_t y;
	uint8_t pressed;
	uint8_t schema;
	uint8_t reserved[2];
};

struct deskkin_touch_ring {
	struct deskkin_touch_sample samples[DESKKIN_TOUCH_CAPACITY];
	uint32_t write_generation;
	uint32_t drop_count;
};

struct deskkin_ui_command {
	uint32_t generation;
	uint8_t command;
	uint8_t schema;
	uint8_t reserved[2];
};

struct deskkin_target_yaw {
	uint32_t generation;
	uint8_t schema;
	uint8_t reserved[3];
	int64_t value;
};

struct deskkin_amp_shared {
	struct deskkin_renderer_heartbeat renderer;
	uint32_t renderer_publication;
	struct deskkin_display_ready display;
	uint32_t display_publication;
	uint32_t display_spi_hz;
	struct deskkin_runtime_sram_handoff runtime_sram;
	uint32_t runtime_sram_publication;
	struct deskkin_world_snapshot world;
	uint32_t world_publication;
	struct deskkin_touch_ring touch;
	struct deskkin_ui_command command;
	uint32_t command_publication;
	struct deskkin_target_yaw target_yaw;
	uint32_t target_yaw_publication;
	uint32_t renderer_progress;
	uint32_t display_progress;
	uint32_t raster_profile[8];
	uint32_t raster_profile_publication;
};

_Static_assert(sizeof(struct deskkin_amp_shared) <= DESKKIN_SHARED_SIZE - DESKKIN_CHANNEL_OFFSET,
	       "AMP channels exceed the shared control region");

#endif
