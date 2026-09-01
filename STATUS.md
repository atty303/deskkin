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
control, boot supervision, LCD power/reset/backlight, two PSRAM RGB565 render
buffers, and one internal-DRAM scanout buffer. APPCPU owns Slint software
rendering, SPI2, GDMA, and full-screen LCD transfer. The physical 10-second
pipeline gate sustains 20 FPS with Quad PSRAM at 80 MHz and LCD SPI at 40 MHz
while retaining responsive PROCPU status.

No external provider connector is implemented. The deterministic availability
connector does not access an external service.

## Active work

The first physical CoreS3 Pet benchmark completed only 487 of 1,200 requested
frames in 60.138 seconds. Its single-core dirty-transfer path is superseded by
the AMP full-screen pipeline. The current physical gate completes 193 frames
over a 9.626-second device-heartbeat window (20.049 FPS); observed maxima are
20.8 ms for render and 40.3 ms for scanout copy plus transfer, with no
allocation or transfer failure. Octal PSRAM was rejected on the physical device
because it prevented the PROCPU USB surface from booting; Quad 80 MHz is the
validated mode.

## Next work

Turn the atlas-free 10-second pipeline gate into the planned bounded 60-second
benchmark, including schedule requests, frame completions, deadline misses,
maximum consecutive misses, stalls, and a bounded percentile representation.
Then restore the normalized Pet asset through the same double-buffered
full-screen path and repeat the physical 20 FPS gate. Fixed-world, parallax,
Notice, IMU, and servo work remain behind that foundation.
