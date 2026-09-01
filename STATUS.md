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
control, boot supervision, LCD power/reset/backlight, one internal-DRAM RGB565
render buffer, and one PSRAM render buffer. APPCPU owns Slint software
rendering, SPI2, GDMA, and full-screen LCD transfer. QIO flash and Quad PSRAM
run at 80 MHz and LCD SPI at 40 MHz while PROCPU status remains responsive.

No external provider connector is implemented. The deterministic availability
connector does not access an external service.

## Active work

No implementation slice is active. The full-screen benchmark is unthrottled
and reports performance rather than enforcing a 20 FPS gate. Its hybrid
internal/PSRAM buffers transfer directly through GDMA with zero bounce-copy
bytes. On the physical CoreS3, the prior capped PSRAM-to-DRAM-copy baseline
reported 20.05 FPS, 18.0 ms last render, 39.7 ms last copy-plus-transfer, and
6.2 ms last copy. The final QIO hybrid path reported 27.27 FPS with 19.9 ms
maximum render, 41.0 ms maximum transfer, and zero copy, with no allocation or
transfer failure. A 64-byte D-cache-line trial had no measurable benefit and
was not retained.

## Next work

Restore the normalized Pet asset through the hybrid double-buffered full-screen
path and measure the real scene without turning its animation cadence into a
hardware performance gate. Fixed world, parallax, Notice, IMU, and servo work
remain behind that foundation.
