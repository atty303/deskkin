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
framebuffer removes PSRAM and bounce-copy traffic. After GDMA starts reading a
frame, Slint immediately renders the next frame into that same buffer; frame
coherence is intentionally not guaranteed. The display and render threads
share priority and yield at the SPI polling boundary. A ten-second physical
run completed 298 frames at 30.92 FPS with 31.0 ms last full-screen transfer
and 4.3 ms last render. Since boot, the observed maxima were 32.2 ms transfer
and 7.6 ms render. Copy time, allocation failures, and transfer failures were
zero. The 40 MHz wire-only ceiling is 32.55 FPS, so the retained path reaches
95.0% of that ceiling. The separate PROCPU remained responsive throughout the
run.

## Next work

Resume the presentation pipeline on this measured baseline. Streaming line or
band rendering and tear synchronization remain deferred. Fixed world,
parallax, Notice, IMU, and servo work remain behind that foundation.
