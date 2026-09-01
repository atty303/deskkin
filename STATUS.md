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
control, boot supervision, LCD power/reset/backlight, and two internal-DRAM
RGB565 30-line bands. APPCPU owns Slint software rendering, SPI2, GDMA, and
full-screen LCD transfer. QIO flash and LCD SPI run at 80 MHz and 40 MHz
respectively while PROCPU status remains responsive.

No external provider connector is implemented. The deterministic availability
connector does not access an external service.

## Active work

The renderer uses a safe ping-pong eight-band pipeline. Slint
renders directly into alternating 320×30 RGB565 internal-DRAM bands while GDMA
transfers the other. Completion ownership prevents reuse while in flight. Every
band is 19,200 bytes, below the 32,736-byte GDMA transaction limit; 240 lines
divide evenly with no final-band special case. No bounce copy or PSRAM buffer
is present. Reserved pixel SRAM is 38,400 bytes, one quarter of a full-screen
RGB565 buffer. A ten-second physical run completed 285 frames in 9.625 seconds
at 29.61 FPS, with 31.8 ms last transfer and 8.0 ms observed render work. Since
boot, the maxima were 31.9 ms transfer and 8.6 ms render. Copy time, allocation
failures, and transfer failures were zero, and PROCPU status remained live. The
user-observed zero-clear line from concurrent reuse is structurally excluded
because Slint and GDMA never own the same band; visual confirmation remains
external to automated diagnostics.

## Next work

Resume the presentation pipeline on the measured ping-pong band baseline. Panel
scanout synchronization remains deferred. Fixed world, parallax, Notice, IMU,
and servo work remain behind that foundation.
