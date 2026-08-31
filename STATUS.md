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
The CoreS3 firmware and host runner now include a fixed 60-second, 20 FPS
Pet-only rendering benchmark with bounded timing and counter diagnostics. The
clean CoreS3 build and host conformance tests pass, but no physical-device
animation or display-transfer performance has been established.

No external provider connector is implemented. The deterministic availability
connector does not access an external service.

## Active work

None.

## Next work

After naming and inspecting the target device, obtain explicit approval to
flash the benchmark-capable firmware. Obtain a separate explicit approval to
run the 60-second Pet benchmark. Fixed-world, parallax, Notice, IMU, and servo
work remain behind the physical performance gate.
