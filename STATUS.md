# Current status

This file is the current working-state handoff, not a history or activity log.
Replace stale entries as work starts, changes, or completes; use version-control
history for prior states and rationale.

## Baseline

The portable application and protocol crates, Linux host and simulator, CoreS3
product and inert-recovery firmware, physical-host profiles, and reproducible
tooling are implemented. The current source and tests define the executable
baseline. See [`docs/architecture.md`](docs/architecture.md) and
[`docs/core-s3.md`](docs/core-s3.md) for the maintained contracts.

The shared Pet-presentation baseline includes a normalized embedded Koyori
atlas, a pure `no_std` animation model for `Idle`, `MoveLeft`, `MoveRight`, and
`Attend`, and one Slint Pet surface used by the simulator and CoreS3 firmware.
The CoreS3 product firmware and host runner include a fixed 60-second, 20 FPS
Pet-only rendering benchmark with bounded timing and counter diagnostics.

The atlas-free replacement runtime is a dual-Zephyr AMP build. PROCPU owns USB
control, boot supervision, LCD power/reset/backlight, and two full-screen
internal-SRAM RGB565 framebuffers. APPCPU owns Slint software rendering, SPI2, GDMA, and
full-screen LCD transfer. QIO flash and LCD SPI run at 80 MHz and 40 MHz
respectively while PROCPU status remains responsive. PROCPU initializes a
bounded 1 MiB Quad-PSRAM heap for APPCPU Slint allocations; framebuffer pixels
and the LCD DMA path remain entirely in internal SRAM.

No external provider connector is implemented. The deterministic availability
connector does not access an external service.

## Validated rendering baseline

The renderer now uses Slint `SwappedBuffers` with two 320×240 RGB565
framebuffers in internal SRAM. Slint renders a complete back buffer while GDMA
transfers the other complete buffer; explicit completion ownership prevents a
buffer from being rendered again while its transfer remains in flight. Each
153,600-byte transfer is descriptor-chained by the display driver. There is no
bounce copy, band clear, or PSRAM framebuffer. PROCPU owns the static 307,200
pixel bytes, while APPCPU exclusively owns their runtime contents.

The AMP control block is isolated at the top of SRAM1 and the PROCPU linker
asserts that its image cannot reach the APPCPU region. The APPCPU does not
reinitialize PSRAM: PROCPU initializes and maps it once before starting the
second core, enables the APPCPU cache bus, and publishes the bounded heap
address and size. APPCPU places only Slint dynamic allocations there and owns
its heap metadata locally. The published region is the dedicated high-end 1
MiB of the mapping, disjoint from PROCPU's registered low-end external heap;
PROCPU's unbounded common-libc allocation arena is disabled.

The physical 60-second full-screen benchmark completed 1,871 frames at 31.438
FPS. The last render and transfer took 7 ms and 32 ms; observed maxima were 8
ms and 33 ms. Copy time, allocation failures, and transfer failures were zero,
and PROCPU answered 229 bounded status requests during the unthrottled run.
The 30.72 ms RGB565 wire time at 40 MHz remains the principal limit.
The benchmark scene uses a fixed background color so content-driven full-screen
luminance changes do not obscure panel flicker and tearing observations.

## Next work

Resume the presentation pipeline on the validated full-frame double-buffer
runtime. Panel scanout synchronization remains deferred. Fixed world,
parallax, Notice, IMU, and servo work remain behind that foundation.
