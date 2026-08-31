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
Pet-only rendering benchmark with bounded timing and counter diagnostics. A
separate atlas-free AMP harness now boots MCUboot, a PROCPU USB supervisor, and
an APPCPU renderer stub from non-overlapping flash partitions. Its physical
fault-injection gate confirms that PROCPU status remains responsive after the
APPCPU enters an infinite loop.

No external provider connector is implemented. The deterministic availability
connector does not access an external service.

## Active work

The first physical CoreS3 Pet benchmark completed only 487 of 1,200 requested
frames in 60.138 seconds. Rather than optimize that single-core dirty-transfer
baseline, current work is establishing the AMP ownership boundary first. The
physical device currently runs the bounded fault harness: APPCPU emits a 100 ms
heartbeat for 10 seconds and then stalls permanently; PROCPU reports the same
generation as stale while continuing to answer USB status requests.

## Next work

Move LCD power/reset/backlight initialization to the PROCPU supervisor and
move exclusive SPI2/display ownership to APPCPU. Then add double framebuffers
and full-screen RGB565 transfer without restoring the atlas yet. The next
physical gate must inject render, display-transfer, and infinite-loop faults
independently while PROCPU USB status remains responsive. Fixed-world,
parallax, Notice, IMU, servo, and atlas work remain behind that foundation.
