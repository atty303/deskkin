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
control, boot supervision, LCD power/reset/backlight, and one internal-DRAM
RGB565 render buffer. APPCPU owns Slint software rendering, SPI2, GDMA, and
full-screen LCD transfer. QIO flash and LCD SPI run at 80 MHz and 40 MHz
respectively while PROCPU status remains responsive.

No external provider connector is implemented. The deterministic availability
connector does not access an external service.

## Active work

The physical 40 MHz full-screen speed baseline is complete. One internal-DRAM
framebuffer removes PSRAM and bounce-copy traffic, and rendering and GDMA
transfer remain sequential so their costs stay independently observable. A
ten-second physical run completed 255 frames at 26.50 FPS with 31.1 ms maximum
full-screen transfer, zero copy time, and no allocation or transfer failure.
The 153,600-byte pixel payload alone requires 30.72 ms at 40 MHz, so the
retained path reaches 98.8% of the wire-only limit. Busy polling is retained on
the isolated APPCPU because a one-millisecond sleep in each SPI transaction
raised transfer time to 33.2 ms. The Rust release profile now favors execution
speed with optimization level 3; observed render samples were 4.2--7.6 ms.

## Next work

Resume the presentation pipeline on this measured baseline. Streaming line or
band rendering and tear synchronization remain deferred. Fixed world,
parallax, Notice, IMU, and servo work remain behind that foundation.
