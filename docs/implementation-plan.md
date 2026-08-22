# Deskkin implementation plan

## Current checkpoint

```text
Status: Phase 0 proposal prepared; dependency approval pending
Product name: Deskkin
First device: StackChan on M5Stack CoreS3
First connector: Unraid
Selected UI: Slint
Selected device platform: Zephyr
Selected application language: no_std Rust
Selected async role: Embassy above the portable core, hosted by Zephyr threads
Implementation: none
Application dependencies: none
Next action: review and approve or revise docs/phase-0-feasibility-proposal.md;
             do not add a Rust workspace, toolchain, crate, board, binding, or
             code before approval
```

This document is the source of truth for resuming development. Accepted
cross-cutting decisions are immutable ADRs under `docs/decisions/`. Update this
checkpoint when a phase is completed, but do not duplicate detailed contracts
from the architecture or ADRs here.

The proposed foundation pins, dependency effects, spike matrix, observation
contract, patch boundary, and removal criteria are in
[`phase-0-feasibility-proposal.md`](phase-0-feasibility-proposal.md). It is a
proposal, not authorization to install or implement its contents.

## Product goal

Deskkin is a modular platform for embodied desktop companions. The first
physical companion is StackChan on CoreS3. The platform is expected to support
other companion devices without changing portable application behavior.

A desktop host integrates external services. Unraid status and control are the
first feature slice. Future candidates include desktop notifications,
conversational AI, calendars, music, home automation, and system status. These
are connectors and features, not dependencies of the device platform.

## Required properties

- One portable application and Slint UI run on device and desktop.
- A deterministic simulator runs scenarios with virtual time and fake effects.
- Board and silicon changes stay below Zephyr and Rust platform adapters.
- Provider changes stay in desktop connectors.
- External credentials and broad mutation authority stay off devices.
- Features have explicit state, commands, events, views, effects, and granted
  capabilities.
- Device and host versions can negotiate protocol and feature capabilities.
- Slint dirty rendering remains a partial display transfer on constrained
  devices and is measured at that boundary.
- External and asynchronous paths are observable without placing diagnostics
  in public protocol results or UI models.

## Explicit non-goals at this checkpoint

- application, UI, firmware, connector, or simulator implementation;
- Rust workspace or crate selection;
- Zephyr workspace, fork, module, board, or driver creation;
- Slint source files or generated bindings;
- transport, serialization, pairing, or authentication selection;
- dynamic plugin ABI or public feature marketplace;
- Unraid, ChatGPT, or desktop notification credentials;
- installation, flashing, host daemon configuration, autostart, or deployment;
- release, remote repository creation, push, or publication.

## Known upstream state

These observations were checked while defining the scaffold on 2026-08-22.
They are starting evidence, not permanent compatibility guarantees.

### Slint

- Slint exposes custom platform and window-adapter interfaces for MCU and OS
  integration.
- Its software renderer tracks dirty regions. `render_by_line` exposes changed
  line ranges, and `RepaintBufferType::ReusedBuffer` permits retained display
  memory and partial updates.
- Slint has upstream bare-metal Rust board support for M5Stack CoreS3 using
  `esp-hal`, an ILI9342C display, FT6336U touch, AXP2101 power management, and
  AW9523 GPIO expansion.
- Slint continuously builds a Zephyr integration for `native_sim` and an NXP
  i.MX RT1170 board, currently through its C++ integration.
- No upstream example was found combining the Slint Rust API,
  `zephyr-lang-rust`, ESP32-S3, and CoreS3.

References:

- <https://docs.slint.dev/latest/docs/slint/guide/backends-and-renderers/backends_and_renderers/>
- <https://docs.slint.dev/latest/docs/rust/slint/platform/software_renderer/>
- <https://github.com/slint-ui/slint/tree/master/examples/mcu-board-support/m5stack_cores3>
- <https://github.com/slint-ui/slint/tree/master/demos/zephyr-common>

### Zephyr and Rust

- Zephyr supports Rust applications through the official optional
  `zephyr-lang-rust` module and `CONFIG_RUST`.
- The module integrates Cargo applications into `west build` and provides
  Rust-facing time, synchronization, thread, timer, work queue, logging,
  allocator, Kconfig, devicetree, GPIO, flash, raw binding, and Embassy support.
- Safe Rust coverage is not complete for display, input, SPI, I2C, audio,
  networking, storage, and other subsystems needed by Deskkin. Narrow wrappers
  or C shims will be required where the module lacks an accepted API.
- The module's pinned platform list contains QEMU Cortex-M and RISC-V targets
  but no Xtensa target. CoreS3 therefore needs a language/toolchain gate in
  addition to board and driver verification.
- The Embassy integration can host an executor in a Zephyr thread and use a
  Zephyr timer as the Embassy time driver. Async Zephyr driver interfaces remain
  incomplete.

References:

- <https://docs.zephyrproject.org/latest/develop/languages/rust/>
- <https://github.com/zephyrproject-rtos/zephyr-lang-rust>
- <https://zephyrproject-rtos.github.io/zephyr-lang-rust/nostd/zephyr/>
- <https://zephyrproject-rtos.github.io/zephyr-lang-rust/nostd/zephyr/embassy/>

## Phase 0: freeze the first feasibility matrix

Before adding any dependency or implementation, produce one proposal that
contains:

1. exact pinned revisions or versions for Zephyr, `zephyr-lang-rust`, Rust,
   Slint, and build tools;
2. the purpose and first use of every proposed dependency;
3. the principal alternatives and why they are not selected for the spike;
4. maintenance, licensing, safety, and reproducibility impact;
5. the host tools fetched by mise and artifacts fetched by west or Cargo;
6. the target matrix, pass/fail criteria for each vertical spike, and the
   evidence that Phase 1 will collect;
7. the boundary for any temporary fork or patch;
8. removal criteria for unsuccessful experimental integration work.

Request approval before changing repository files to add those dependencies.
Do not propose application feature crates yet; Phase 0 is limited to proving
the selected foundation.

The current proposal is
[`phase-0-feasibility-proposal.md`](phase-0-feasibility-proposal.md). Phase 0 is
complete only when that proposal is approved or revised into an approved
baseline.

## Phase 1: prove the foundation in independent vertical spikes

Keep the following uncertainties separate so failure identifies one boundary.

### Gate 1A: Rust application on supported Zephyr targets

Build and run the smallest official-style Rust application through
`zephyr-lang-rust` on one emulator and one supported physical architecture when
practical. Confirm Cargo integration, panic behavior, allocation when enabled,
Kconfig access, typed devicetree access, logging, and a clean reproducible
build.

This gate proves the baseline only. It does not prove Slint or ESP32-S3.

### Gate 1B: Slint Rust software renderer on Zephyr

Add Slint as a dependency of the Rust Zephyr application on an already
supported target. Implement only the platform functions needed to render a
static and animated test surface into a mock or supported framebuffer.

Verify:

- Slint compilation under the exact Zephyr Rust target;
- allocator and panic compatibility;
- timer and animation wakeup without busy polling;
- input event dispatch through a mock source;
- dirty-region rendering and returned physical ranges;
- desktop rendering of the exact same `.slint` UI;
- no C++ application layer is required for this selected Rust path.

### Gate 1C: ESP32-S3/Xtensa Zephyr Rust toolchain

Prove that a minimal `zephyr-lang-rust` application can compile, link, boot,
log, and panic predictably on an ESP32-S3 Zephyr target. Treat compiler target,
C ABI, linker, compiler builtins, target features, and allocator as explicit
evidence boundaries.

Do not combine CoreS3 display or Slint work into this gate. If an upstream
change or maintained module patch is required, document its scope and upstream
strategy before expanding it.

### Gate 1D: CoreS3 Zephyr board and drivers

Start with Zephyr 4.4.1's existing CoreS3 board and independently prove power
sequencing, GPIO expansion, display, touch, memory, and required buses through
Zephyr. Add or change board support only for an observed missing or incorrect
boundary. Use the Slint `esp-hal` board support and M5Stack sources as
behavioral references, not as application architecture.

The display gate must verify partial RGB565 writes at arbitrary supported
rectangles. Record transfer size and duration. Do not claim smooth dirty
rendering from a successful full-frame update.

### Gate 1E: combined CoreS3 Slint slice

Only after Gates 1B, 1C, and 1D pass, combine them into one vertical slice:

```text
touch --> Zephyr input --> Rust adapter --> Slint event
Slint dirty render --> Rust adapter --> Zephyr partial display write
```

Exercise a StackChan-scale face animation and interaction. Measure dirty area,
render time, transfer time, end-to-end input latency, and missed deadlines.

## Phase 2: portable application and deterministic simulator

After the foundation passes:

1. introduce only the application core, message types, effect model, and
   runtime ports needed by one character interaction;
2. keep the core `no_std` and runtime-neutral;
3. add a desktop Slint application using the same UI and presenter;
4. add virtual time and scripted fake effects;
5. record semantic input, requested effects, state transitions, and UI views;
6. prove deterministic replay of one interaction and one injected failure.

Do not add the host protocol or Unraid connector merely to fill the proposed
workspace shape.

## Phase 3: host and protocol vertical slice

Design the smallest semantic protocol slice for one paired desktop host and one
simulated device. Before dependencies, approve transport, serialization,
framing, identity, pairing, authentication, versioning, reconnection,
backpressure, and observation contracts together.

The first slice should negotiate capabilities, publish host availability,
deliver one read-only status update, and survive a disconnect without
fabricating state. Mutation and external credentials remain out of this slice.

## Phase 4: Unraid read-only feature

Add an infrastructure-status feature and an Unraid connector. Keep Unraid
payloads and credentials in the host. Normalize only the semantics needed by
the accepted UI, and prove the connector against an isolated fake or test
boundary before any live system access.

Live Unraid inspection and control require a separate explicit authorization.
Do not broaden credentials or mutation permissions after an access denial.

## Phase 5: confirmed Unraid actions

Introduce action identity, expiration, confirmation, authorization, replay
protection, completion, and failure as one end-to-end contract. Read and
mutation capabilities remain separate. A desktop host rechecks authorization
immediately before invoking the connector.

## Later feature slices

Notifications, conversation, and other integrations each begin with one
semantic vertical slice. They reuse the feature and protocol boundaries but do
not need a universal provider schema.

Conversational AI integration must keep provider credentials and provider tool
execution in the host. A device receives conversation and proposed-action
semantics rather than a provider SDK surface.

## Cross-cutting verification

Each implemented external, asynchronous, state-changing, or multi-stage path
must define its result, control, and out-of-band diagnostic surfaces before
implementation. Record correlation, stage outcome, timing, backpressure, and
degraded coverage without placing private payloads or credentials in logs.

The repository entrypoints remain:

```text
mise run check   non-mutating fast validation
mise run fix     safe formatting and lint repair
mise run test    complete reproducible validation
```

Add target-specific live tasks separately. `mise run test` must not require
physical hardware, real provider accounts, host mutation, or secret material.

## Open decisions

The following are deliberately unresolved:

- exact Zephyr and Rust revisions for the first spike;
- how ESP32-S3 support enters or extends `zephyr-lang-rust`;
- safe Rust wrapper ownership for missing Zephyr subsystems;
- Embassy executor count and thread priorities;
- desktop runtime and supported desktop operating systems;
- protocol transport, schema language, serialization, and compatibility rule;
- pairing, device identity storage, and transport security;
- host process and connector isolation;
- UI navigation and shared feature-surface contracts;
- local behavior while the host is disconnected;
- OTA, rollback, and firmware compatibility policy;
- packaging, repository split, and release strategy.

Resolve an item only when its first vertical slice needs it. Record a
cross-cutting decision as a new ADR.

## Resume instructions

Start a future development session with:

> Read `AGENTS.md`, `docs/architecture.md`, the accepted ADRs,
> `docs/implementation-plan.md`, and
> `docs/phase-0-feasibility-proposal.md`. Confirm that the repository is at the
> Phase 0 dependency-approval checkpoint and refresh any drift-prone upstream
> pins or evidence before relying on them. Do not implement yet. Ask for the
> proposal to be approved or revised. Before approval, do not add toolchains,
> dependencies, generated bindings, board files, a Rust workspace, or code.
> Record actual pass/fail evidence only when executing the approved Phase 1
> gates.
