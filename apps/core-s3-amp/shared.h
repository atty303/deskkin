// SPDX-License-Identifier: MIT

#ifndef DESKKIN_CORE_S3_AMP_SHARED_H
#define DESKKIN_CORE_S3_AMP_SHARED_H

#include <stdint.h>

#define DESKKIN_HEARTBEAT_MAGIC 0x44534b4eU
#define DESKKIN_DISPLAY_MAGIC 0x4453504cU

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
};

struct deskkin_display_ready {
	uint32_t magic;
	uint32_t generation;
	uint32_t ready;
	uint32_t framebuffer[2];
};

struct deskkin_amp_shared {
	struct deskkin_renderer_heartbeat renderer;
	uint32_t renderer_publication;
	struct deskkin_display_ready display;
	uint32_t display_publication;
	uint32_t display_spi_hz;
};

#endif
