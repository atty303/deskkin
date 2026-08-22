# ADR-0001: Assign distinct roles to Zephyr, Rust, Embassy, and Slint

- Status: Accepted
- Date: 2026-08-22

## Context

Deskkin needs a rich portable UI, safe application logic, asynchronous
workflows, and hardware portability across more than one board or silicon
vendor. The first target, StackChan on M5Stack CoreS3, must not determine the
long-term application structure.

Slint is selected as the UI toolkit. The remaining question is how to divide
responsibility among the device OS, Rust application, and async runtime without
duplicating abstractions.

## Decision

Use these roles:

- Slint owns declarative UI, binding, animation, and rendering.
- Pure `no_std` Rust owns portable application state and policy.
- Embassy owns Rust async orchestration above the portable core when a device
  path needs async concurrency.
- Zephyr owns hardware topology, drivers, system services, and outer thread
  scheduling.
- Narrow adapters translate between these layers.

Embassy runs within one or more Zephyr threads for asynchronous device paths.
Zephyr remains the system scheduler; Embassy cooperatively polls Rust futures
within its host thread. Work with a distinct priority or blocking behavior may
use a separate Zephyr thread. Portable core transitions and synchronous paths
remain runtime-neutral and do not require an executor.

Application code does not introduce a second hardware abstraction. It uses
semantic ports for external effects, while device adapters use Zephyr's device
and subsystem APIs.

## Consequences

- The application and UI can run on desktop without emulating Zephyr.
- Device support scales through Zephyr boards, devicetree, drivers, and
  modules.
- Rust wrappers are needed where `zephyr-lang-rust` does not yet expose a safe
  subsystem API.
- Slint must have one owner and receive messages from concurrent tasks.
- Runtime-neutral time and effect contracts are needed for deterministic
  simulation.
- An ESP32-S3/Xtensa feasibility gate is required before claiming CoreS3
  support.

## Alternatives

### ESP-IDF as the primary platform

ESP-IDF has strong Espressif enablement and component tooling, but application
and service code more readily acquire Espressif and FreeRTOS-specific types.
It remains a hardware reference, not Deskkin's primary portability boundary.

### Bare-metal `esp-hal` with Embassy

This is a clean Rust-first solution and Slint already has CoreS3 reference
code using `esp-hal`. It would require Deskkin to own more system-wide driver,
product configuration, lifecycle, and service integration policy.

### NuttX

NuttX offers a stronger POSIX-like environment, but Slint and Rust integration
are less directly aligned with the selected application architecture. Revisit
it only if POSIX and VFS semantics become more important than Zephyr's board
and subsystem model.
