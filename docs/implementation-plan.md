# Deskkin implementation plan

## Current checkpoint

```text
Status: Phase 0 and Phase 1 Gates 1A-1B complete; Gate 1C ready
Product name: Deskkin
First device: StackChan on M5Stack CoreS3
First connector: Unraid
Selected UI: Slint
Selected device platform: Zephyr
Selected application language: no_std Rust
Selected async role: Embassy above the portable core, hosted by Zephyr threads
Implementation: bounded Gate 1A and Gate 1B feasibility applications and local gate runners
Application dependencies: approved Gate 1A set plus exact Slint 1.17.1 Gate 1B set
Next action: implement only the approved Gate 1C ESP32-S3/Xtensa Zephyr Rust
             toolchain spike; do not begin Gate 1D or a physical CoreS3 gate
             until its ordered prerequisites pass
```

This document is the source of truth for resuming development. Accepted
cross-cutting decisions are immutable ADRs under `docs/decisions/`. Update this
checkpoint when a phase is completed, but do not duplicate detailed contracts
from the architecture or ADRs here.

The proposed foundation pins, dependency effects, spike matrix, observation
contract, patch boundary, and removal criteria are in
[`phase-0-feasibility-proposal.md`](phase-0-feasibility-proposal.md). It is the
approved baseline for the ordered Phase 1 foundation gates.

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

## Explicit non-goals for Gate 1A

- product application features, UI, connectors, or the product simulator;
- Slint source files, Slint dependencies, or generated bindings;
- Xtensa/CoreS3 Rust integration, board or driver changes, and physical
  flashing;
- any `zephyr-lang-rust` patch; the approved patch policy applies first at Gate
  1C if observed evidence requires it;
- transport, serialization, pairing, or authentication selection;
- dynamic plugin ABI or public feature marketplace;
- Unraid, ChatGPT, or desktop notification credentials;
- host daemon configuration, autostart, or deployment;
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

Phase 0 completed on 2026-08-22 when all four approval items in that proposal
were accepted.

## Phase 1: prove the foundation in independent vertical spikes

Keep the following uncertainties separate so failure identifies one boundary.

### Gate 1A: Rust application on supported Zephyr targets

Build and run the smallest official-style Rust application through
`zephyr-lang-rust` on one emulator and one supported physical architecture when
practical. Confirm Cargo integration, panic behavior, allocation when enabled,
Kconfig access, typed devicetree access, logging, and a clean reproducible
build.

This gate proves the baseline only. It does not prove Slint or ESP32-S3.

Gate 1A passed on 2026-08-22. Local diagnostic run
`c7fd0a3d-a090-4516-aedf-8c1c57617900` recorded complete evidence for both
`qemu_cortex_m3` and `qemu_riscv32`:

- each target configured through the pinned west manifest, compiled the Rust
  static library, compiled and linked Zephyr, and booted under SDK 1.0.1 QEMU;
- typed Kconfig and devicetree reads, logging, allocation value 42, and an
  Embassy timer/channel wakeup emitted their fixed semantic markers;
- each target emitted the final pass record and the deliberate Rust panic was
  captured separately;
- rebuilding each normal image from a removed build directory at the same path
  reproduced its ELF digest: Cortex-M3
  `5307cb81a7e6d0a79a9b39fa8f160b365bd2290514980f2f628908dda2ef4fcf`
  and RISC-V
  `0014f14da6c26aabf911d002e81c38decc9e62eb61d06d3f60e52f2f0cb8baac`;
- the atomic result and diagnostic files were mode 0600, the diagnostic
  directory was mode 0700, completeness was `complete`, and the privacy scan
  found no absolute home path, username, or credential-pattern value.
- the 24-test conformance suite covered observation on/off equivalence,
  timeout/panic/cancellation separation, recording-storage degradation,
  truncated and sensitive artifacts, remote-connection exclusion, concurrent
  ownership, process-tree cleanup, retention, and deletion safety.

The gate remains a feasibility boundary. These results do not establish Slint,
Xtensa, CoreS3 board support, display behavior, or product application
architecture.

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

Gate 1B passed on 2026-08-22. Complete local diagnostic run
`73a3cc38-06dd-4581-ad1b-75bb81b8e221` recorded the bounded Slint 1.17.1
software-renderer slice on `qemu_cortex_m3` and the host:

- the exact `thumbv7m-none-eabi` Zephyr Rust target built, linked, booted, and
  reproduced ELF digest
  `5e28156cc39db148288d932a6e084548950ed1edb87cff93c482cb737e964133`
  after removal and clean reconstruction of its build directory;
- the shared `.slint` source rendered all 240 initial RGB565 lines, dispatched
  one mock pointer callback through the typed UI callback, and returned 34
  dirty line ranges covering exactly the 2,720 changed pixels;
- the animation advanced through ten Embassy timer waits with zero busy polls;
  eight dirty-render stages changed 2,352 pixels after input;
- QEMU and host rendering normalized to the same 320x240 RGB image, digest
  `ee51bbe3ed7662d999ee50c27985c30bcb34e253e04d8c00f18905a7f6f47416`;
- the selected application layer is Rust and Slint with no C++ source, and the
  exact direct dependency/version/feature review matched the approved Gate 1B
  boundary;
- the 48-test conformance suite passed, and the mode-0600 artifacts were
  recorded with completeness `complete` and no forbidden path, identity, or
  credential fixture.

This proves only the bounded software-renderer integration on an already
supported QEMU architecture. It does not establish Xtensa, CoreS3 display or
touch support, physical-device performance, or a distribution license for
embedded Slint firmware. The result remains a local, non-distributed
GPL-3.0-only feasibility spike.

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

- production Rust wrapper ownership beyond the approved Phase 1 gates;
- production Embassy executor and thread topology beyond Gate 1A's single
  executor;
- desktop runtime and supported desktop operating systems;
- protocol transport, schema language, serialization, and compatibility rule;
- pairing, device identity storage, and transport security;
- host process and connector isolation;
- UI navigation and shared feature-surface contracts;
- local behavior while the host is disconnected;
- OTA, rollback, and firmware compatibility policy;
- packaging, repository split, and release strategy.

The first-spike Zephyr and Rust revisions are fixed by the approved Phase 0
baseline. If drift makes a pin unusable, revise and reapprove the baseline
before changing it.

Resolve an item only when its first vertical slice needs it. Record a
cross-cutting decision as a new ADR.

## Resume instructions

Start a future development session with:

> Read `AGENTS.md`, `docs/architecture.md`, the accepted ADRs,
> `docs/implementation-plan.md`, and
> `docs/phase-0-feasibility-proposal.md`. Confirm that the repository is at the
> Phase 1 Gate 1C checkpoint and refresh any drift-prone upstream pins or
> evidence before relying on them. All Phase 0 approval items are accepted and
> Gates 1A and 1B passed with their results recorded above. Implement only the
> approved Gate 1C ESP32-S3/Xtensa Zephyr Rust toolchain spike, record its
> specified evidence, and stop at its pass/fail boundary. Do not begin Gate 1D
> or any physical CoreS3 gate until the ordered prerequisites are satisfied.
> Record actual pass/fail evidence only when executing the approved Phase 1
> gates.
