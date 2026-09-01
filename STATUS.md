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

The shared Pet-presentation baseline includes four normalized embedded Koyori
QOI loop atlases, a pure `no_std` animation model for `Idle`, `MoveLeft`,
`MoveRight`, and `Attend`, and one Slint Pet surface used by the simulator and
CoreS3 firmware.
The CoreS3 product firmware and host runner include a fixed 60-second, 20 FPS
Pet-only rendering benchmark with bounded timing and counter diagnostics.

The replacement runtime is a dual-Zephyr AMP build. PROCPU owns USB
control, boot supervision, LCD power/reset/backlight, and two full-screen
internal-SRAM RGB565 framebuffers. APPCPU owns Slint software rendering, SPI2,
GDMA, and dirty-rectangle LCD transfer. QIO flash and LCD SPI run at 80 MHz and 40 MHz
respectively while PROCPU status remains responsive. PROCPU initializes a
bounded 4 MiB Quad-PSRAM heap for APPCPU Slint allocations; framebuffer pixels
and the LCD DMA path remain entirely in internal SRAM.

The AMP renderer stores each Koyori loop as compressed QOI bytes in flash. It
keeps only the active decoded RGBA loop in PSRAM and synchronously releases it
before decoding the next loop. No Slint draw occurs between release and
installation, so the LCD retains its preceding completed frame during the
transition. Idle and Attend run at 10 FPS; MoveRight and MoveLeft run at 20 FPS.

No external provider connector is implemented. The deterministic availability
connector does not access an external service.

## Validated rendering baseline

The renderer now uses Slint `SwappedBuffers` with two 320×240 RGB565
framebuffers in internal SRAM. Slint renders the changed area into a complete
back buffer while GDMA transfers the dirty bounding rectangle from the other
buffer's full-frame stride; explicit completion ownership prevents a buffer
from being rendered again while its transfer remains in flight. The ESP32
MIPI-DBI driver gathers selected row spans directly into DMA descriptors, and
complete-frame dirty regions use bounded 30-line transactions. There is no
bounce copy, band clear, or PSRAM framebuffer. PROCPU owns the static 307,200
pixel bytes, while APPCPU exclusively owns their runtime contents.

The AMP control block is isolated at the top of SRAM1 and the PROCPU linker
asserts that its image cannot reach the APPCPU region. The APPCPU does not
reinitialize PSRAM: PROCPU initializes and maps it once before starting the
second core, enables the APPCPU cache bus, and publishes the bounded heap
address and size. APPCPU places only Slint dynamic allocations there and owns
its heap metadata locally. The published region is the dedicated high-end 4
MiB of the 8 MiB mapping, disjoint from PROCPU's registered low-end external heap;
PROCPU's unbounded common-libc allocation arena is disabled.

Before Koyori restoration, the physical 60-second complete-frame benchmark
completed 1,871 frames at 31.438
FPS. The last render and transfer took 7 ms and 32 ms; observed maxima were 8
ms and 33 ms. Copy time, allocation failures, and transfer failures were zero,
and PROCPU answered 229 bounded status requests during the unthrottled run.
The 30.72 ms RGB565 wire time at 40 MHz remains the principal limit.
The predecessor local-update benchmark used a fixed background and an 80×80 square moving
one pixel per frame throughout the 320×240 screen. Its steady-state
`SwappedBuffers` dirty bound is 82×82: one rectangle, 6,724 pixels, 13,448
transferred bytes, and six pixel DMA batches. The physical 60-second run
completed 9,177 frames at 153.803 FPS. Last render and transfer times were 2 ms
and 6 ms; observed maxima were 4 ms and 35 ms. Allocation and transfer failures
were zero.

The ILI9342 normal-mode frame rate is approximately 30.9 Hz
(`DIVA=fosc/2`, `RTNA=31`). On the physical panel this left the tear geometry in
a paused video frame effectively unchanged while substantially reducing the
multiple-edge persistence seen by eye. There is no wired TE signal or scan-line
readback, so the renderer and panel remain free-running and this setting does
not claim tear-free presentation.

## Next work

The restored QOI loop lifecycle is reproducibly build-tested but has not been
flashed or observed on the physical CoreS3. The next product slice remains the
approved fixed 2D world: implement the deterministic 320×240 viewport, three
parallax layers, separated target and observed pose, and Notice cues in the
simulator before enabling physical motion.
