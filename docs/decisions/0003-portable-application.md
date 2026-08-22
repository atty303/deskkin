# ADR-0003: Keep the application core runtime-neutral

- Status: Accepted
- Date: 2026-08-22

## Context

Deskkin must support physical devices, a native desktop UI, a deterministic
simulator, and future boards. Direct dependencies on Zephyr, Embassy, Slint,
wall-clock time, or provider APIs would force tests and alternate platforms to
emulate implementation details.

Modularity also does not require dynamic code loading on a microcontroller.
Rust does not provide a stable dynamic plugin ABI suitable as the initial
device feature boundary.

## Decision

Implement the portable application core as pure `no_std` Rust. It owns typed
state, input events, commands, effect requests, view models, and feature policy.
It performs no I/O and reads no ambient clock, randomness, or runtime state.

Platform runtimes execute effects and return explicit completion events.
Embassy is the initial device runtime option, not a dependency of the portable
core. Slint is reached through a presenter and typed UI messages.

Register device features at compile time. A feature owns its state and
exchanges typed application messages. Dynamic desktop connector loading is a
separate decision and must not force a dynamic device ABI.

## Consequences

- State transitions can be tested synchronously.
- Virtual time and scripted failures can drive deterministic scenarios.
- Desktop and device builds can share application behavior and Slint UI.
- Ports must describe application semantics rather than mirror provider or OS
  APIs.
- Cross-feature coordination needs an explicit owner instead of shared mutable
  global state.
- The design should avoid trait proliferation by adding ports only at real
  effect boundaries.

## Alternatives

### Depend directly on Embassy in the core

This is simpler for the first device runtime but binds time, scheduling, and
channels to one execution model. Embassy remains available in the runtime,
where its benefits do not constrain deterministic simulation.

### Give each feature direct UI and service access

This minimizes message definitions initially but couples feature state to
Slint objects and provider lifecycles. It also prevents one feature from being
exercised consistently on device, desktop, and simulator.

### Dynamic plugins on the device

This adds ABI, loading, memory, versioning, and trust concerns before a need is
demonstrated. Compile-time composition provides module ownership without that
cost.
