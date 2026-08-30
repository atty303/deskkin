# Deskkin implementation plan

## Current checkpoint

```text
Status: Phase 0, Phase 1 Gates 1A-1E, Phase 2, Phase 3, and Phase 3P complete
Product name: Deskkin
First device: StackChan on M5Stack CoreS3
Current provider connector: none
Selected UI: Slint
Selected device platform: Zephyr
Selected application language: no_std Rust
Selected async role: Embassy above the portable core, hosted by Zephyr threads
Implementation: portable application/protocol crates, Linux host and simulator, repeatable physical-host profiles, and the physically qualified CoreS3 firmware/tooling slice
Application dependencies: approved product dependencies resolved in the root and device lockfiles
Next action: review the exact retained physical profile and separately approve its private-LAN host launch and CoreS3 reconnect qualification; provider connectors remain deferred
```

This document is the source of truth for resuming development. Accepted
cross-cutting decisions are immutable ADRs under `docs/decisions/`. Update this
checkpoint when a phase is completed, but do not duplicate detailed contracts
from the architecture or ADRs here.

The implemented Foundation A checkpoint is
[`foundation-a-repeatable-physical-profile-proposal.md`](foundation-a-repeatable-physical-profile-proposal.md).
It makes the already qualified host path repeatable through a named,
secret-free local profile and exact foreground owner lifecycle. Its architecture
is accepted in
[`ADR-0006`](decisions/0006-repeatable-physical-profile.md). Isolated loopback
implementation does not authorize creating the retained profile, launching its
private-LAN listener, or accessing the CoreS3; those remain the next live
approval checkpoint.

The proposed foundation pins, dependency effects, spike matrix, observation
contract, patch boundary, and removal criteria are in
[`phase-0-feasibility-proposal.md`](phase-0-feasibility-proposal.md). It is the
approved baseline for the ordered Phase 1 foundation gates.

## Product goal

Deskkin is a modular platform for embodied desktop companions. The first
physical companion is StackChan on CoreS3. The platform is expected to support
other companion devices without changing portable application behavior.

A desktop host integrates external services. Future connector candidates
include Unraid, desktop notifications, conversational AI, calendars, music,
home automation, and system status. No provider connector is the current
checkpoint; connectors and features are not dependencies of the device
platform.

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

## Historical foundation record

The remaining Phase 0 and Phase 1 sections preserve the approved contracts and
physical evidence that selected the current product foundation. Their
applications, runners, bootstrap scripts, and conformance tests have been
removed from the active repository surface. Current development uses the
product bootstrap and verification entrypoints documented in `README.md`.

### Explicit non-goals for Gate 1A

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

### Known upstream state

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

### Phase 0: freeze the first feasibility matrix

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

### Phase 1: prove the foundation in independent vertical spikes

Keep the following uncertainties separate so failure identifies one boundary.

#### Gate 1A: Rust application on supported Zephyr targets

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

#### Gate 1B: Slint Rust software renderer on Zephyr

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

#### Gate 1C: ESP32-S3/Xtensa Zephyr Rust toolchain

Prove that a minimal `zephyr-lang-rust` application can compile, link, boot,
log, and panic predictably on an ESP32-S3 Zephyr target. Treat compiler target,
C ABI, linker, compiler builtins, target features, and allocator as explicit
evidence boundaries.

Do not combine CoreS3 display or Slint work into this gate. If an upstream
change or maintained module patch is required, document its scope and upstream
strategy before expanding it.

Gate 1C passed on a physical CoreS3. Local diagnostic run
`015a39fd-7191-45ef-8ca5-4ea5681d8514` recorded complete evidence:

- the pinned ESP Rust 1.95.0.0 compiler built `core` and `alloc` from source for
  `xtensa-esp32s3-none-elf`;
- the Rust static library, C ABI shim, Zephyr C objects, Xtensa linker, and
  ESP32-S3 image generation completed for the existing
  `m5stack_cores3/esp32s3/procpu` board;
- a removed normal build directory reproduced ELF digest
  `c1b797b06ebd2d35e75c82c7155c17a66c8de626b842331377e841efc7ea5cab`,
  and a separate deliberate-panic image compiled and linked with digest
  `93b48a91fc0a1a67fdbbfc5cadfba0ffb19eabd863b4dc24b5a799e8c30ff32f`;
- the pinned Xtensa `readelf` and `nm` confirmed ELF32 Xtensa attributes,
  required bidirectional ABI symbols, one compiler-builtins owner for
  `__muldi3`, the linker map, and the expected text/data/BSS placements;
- the run-bound fixed serial protocol recorded normal boot, C-to-Rust and
  Rust-to-C values of 42, nested critical-section behavior with interrupt-state
  restoration, and one allocation/free probe;
- the deliberate Rust panic was observed from the separate panic image, after
  which cleanup reflashed the normal image and proved the same test firmware's
  inert idle state in 1,937 ms;
- the final result is `pass` with firmware-input digest
  `3df0774203345a7c96da4034d0f49dd755eaf48788679e954c90b1a1feeec840`,
  normal image SHA-256
  `7e7c9ffec1be69a5ee507761e49bc01b09436b4ebb91f3be345e8ff0d193c45a`,
  `cleanup_status=success`, and `device_state=test_firmware_idle`.

The retained local patch scope, upstream strategy, and removal conditions are
recorded in [`core-s3-zephyr-rust-patches.md`](core-s3-zephyr-rust-patches.md). The runner
enforces recognized-firmware preflight, a run-bound serial protocol, the
explicit `device:recover` surface, and fail/timeout/cancel cleanup. Gate 1D may
now start as its own checkpoint; this Gate 1C evidence does not establish any
display, touch, power, memory, or bus behavior.

#### Gate 1D: CoreS3 Zephyr board and drivers

Start with Zephyr 4.4.1's existing CoreS3 board and independently prove power
sequencing, GPIO expansion, display, touch, memory, and required buses through
Zephyr. Add or change board support only for an observed missing or incorrect
boundary. Use the Slint `esp-hal` board support and M5Stack sources as
behavioral references, not as application architecture.

The display gate must verify partial RGB565 writes at arbitrary supported
rectangles. Record transfer size and duration. Do not claim smooth dirty
rendering from a successful full-frame update.

Gate 1D passed on a physical CoreS3. Complete local diagnostic run
`33e4ab9c-4ecf-4149-a5d0-7d305796dca6` used Zephyr 4.4.1's existing
`m5stack_cores3/esp32s3/procpu` board without a board overlay or driver change:

- AXP2101 power management, AW9523 GPIO expansion, the ILI9342C RGB565
  display, FT6336 touch, flash, PSRAM, I2C0, I2C1, and SPI2 all initialized
  through their upstream Zephyr devices;
- a follow-up bounded readiness probe enables Zephyr's pinned ESP32-S3 Wi-Fi
  driver and RF blobs and requires its device to report ready, without scanning,
  joining an access point, acquiring an address, or handling credentials;
- a 32 KiB external-PSRAM allocation passed a bounded read/write probe, and a
  read-only 32-byte flash probe returned nonzero contents;
- three non-fullscreen 80x60 RGB565 rectangles at `(20,20)`, `(120,90)`, and
  `(220,160)` each transferred 9,600 bytes in 4,288, 4,211, and 4,211
  microseconds respectively;
- because the upstream display node is write-only, the deterministic panel
  oracle bound the displayed red, green, and blue rectangles to FT6336 touch
  coordinates `(63,59)`, `(159,124)`, and `(251,195)` in that order;
- removing and reconstructing the same build directory reproduced its ELF
  digest, while ELF/map/config evidence confirmed the Xtensa target, upstream
  CoreS3 board selection, required driver owners, and text/data/BSS/external
  RAM placement;
- the final result is `pass` with firmware-input digest
  `994f4902ba0363a101314bd62128f931382dc0ef7210e15f65702cb1c3ff9758`,
  image SHA-256
  `a6096965d4d84c139764f3e19e5baad76cee7cf60a73a63521773abd89e903a0`,
  `cleanup_status=success`, and `device_state=test_firmware_idle`; cleanup
  reflashed and re-read inert idle in 3,508 milliseconds, within the approved
  ten-second residual-state limit.

Complete follow-up diagnostic run
`07613046-2cb5-4259-b63b-a908a72b750e` added that Wi-Fi readiness criterion.
The pinned west manifest now includes the Mbed TLS and TF-PSA-Crypto revisions
required by Zephyr's ESP32-S3 Wi-Fi driver, and bootstrap fetches the
HAL-declared RF blob set that Zephyr verifies before configuring the driver.
The run retained `event=wifi status=ready`, with no scan, association, address
acquisition, or credential input. It passed every prior Gate 1D criterion with
firmware-input digest
`ad47d753cc40978656efa6eb649fc8f7fcc8668e0213557f24c2abea43b41912`,
image SHA-256
`aa1f06ed28ec2130dd078e15654c95feafcec5cf225360beaa92a0b762a07398`,
`cleanup_status=success`, and `device_state=test_firmware_idle`. Cleanup
reflashed in 4,381 milliseconds and re-read inert idle in 1,737 milliseconds,
remaining within the approved ten-second residual-state limit.

The runner also retains the run-bound transfer and raw touch samples needed to
distinguish a missing touch from a coordinate outside the expected rectangle.
This evidence proves the independent board/driver boundary only. It does not
establish Slint on CoreS3, dirty-render integration, animation performance, or
the Gate 1E combined path.

#### Gate 1E: combined CoreS3 Slint slice

Only after Gates 1B, 1C, and 1D pass, combine them into one vertical slice:

```text
touch --> Zephyr input --> Rust adapter --> Slint event
Slint dirty render --> Rust adapter --> Zephyr partial display write
```

Exercise a StackChan-scale face animation and interaction. Measure dirty area,
render time, transfer time, end-to-end input latency, and missed deadlines.

The physical qualification run
`a0401b75-31d3-4823-b0b7-1cda09f5e110` and required recording-off conformance
run `9c56aa9f-cf5c-4d19-ad13-10bf50bad99b` both passed with firmware digest
`fae8edd2254308dadfd6b15a10ee61338b6e6c2844067be1a158aa1a90d781aa`
and workload identity
`0f352ecc704c3122fa682b42399d2fd8840d8d708c01c6ee3d3cff38a3d1bdf2`.
Qualification disabled/enabled render p95 was 4,242/4,254 microseconds,
transfer p95 was 1,764/1,767 microseconds, and combined p95 was 6,010/6,033
microseconds. Both phases had zero deadline misses, all 1,740 enabled
post-warm-up frame records were retained, recording overhead passed, and
diagnostics completed. Conformance created no diagnostic directory, linked the
qualification run explicitly, and matched its disabled semantic-event and
framebuffer digests. Both runs returned the recognized firmware to inert idle
within ten seconds.

The recording-off control failure was caused by status retries remaining in
the serial path when the next command was sent without the incidental delay of
starting the recorder. Status completion now requires a bounded quiet period,
and the firmware emits a run-bound acceptance marker before starting the
workload. The result reader also accepts finite floating-point criterion values
already permitted by the result schema, while rejecting non-finite values.
These changes remove the timing dependency and the false
`qualification_required` classification. The physical display showed the
expected face colors and partial updates without noisy dirty rectangles.

## Phase 2: portable application and deterministic simulator

After the foundation passes:

1. add the pure `no_std` application core for a provider-neutral availability;
2. start a read immediately and refresh it every five seconds;
3. map read failure to `Unknown` and retry, but stop on timer-arm failure;
4. share one 320 x 240 Slint status surface and presenter across native Linux
   and deterministic headless runtimes;
5. record bounded semantic refresh runs with correlated operation lifecycle,
   typed failure, and crash recovery without changing results;
6. prove byte-identical replay of success and injected read failure.

Do not add the host protocol or Unraid connector merely to fill the proposed
workspace shape.

The exact domain contract, two-crate boundary, dependency and licensing
decisions, observation contract, acceptance criteria, and 2026-08-23 approval
record are in [`phase-2-slice-proposal.md`](phase-2-slice-proposal.md).

Phase 2 passed on 2026-08-23. The dependency-free `no_std` core, native Linux
simulator, two deterministic software-rendered scenarios, bounded diagnostic
recorder, control surface, tooling, and licensing artifacts passed the complete
`mise run test` entrypoint. The two scenario replays matched semantically and
at the RGB565 frame level, recording on/off preserved results, and a fresh
independent review found no actionable defect. The user then confirmed the
native 320 x 240 surface visibly progressed through
`Unknown -> Available -> Unavailable -> Unknown`. No protocol, connector,
device adapter, package, release, or published artifact was added.

## Phase 3: host and protocol vertical slice

The accepted contract and 2026-08-24 approval record are in
[`phase-3-slice-proposal.md`](phase-3-slice-proposal.md) and
[ADR-0004](decisions/0004-paired-host-protocol.md). The approved documentation
checkpoint must be committed separately before adding its workspace members,
dependencies, protocol code, identity state, listener, or host runtime. After
that commit, implement the exact loopback protocol, identity lifecycle, hosted
adapters, observation surfaces, and acceptance suite without expanding scope.

Implement the smallest semantic protocol slice for one paired desktop host and
one simulated device. Transport, serialization, framing, identity, pairing,
authentication, versioning, reconnection, backpressure, and observation are
fixed together by ADR-0004 and the approved proposal.

The first slice should negotiate capabilities, publish host availability,
deliver one read-only status update, and survive a disconnect without
fabricating state. Mutation and external credentials remain out of this slice.

The implementation passed the complete `mise run test` entrypoint on
2026-08-24. This covered locked Clippy, all workspace tests, byte-identical
headless replay, recording equivalence, the protocol disconnect recovery
scenario, `thumbv7m-none-eabi` checks for both portable crates, and the existing
89 repository conformance tests. Real loopback tests covered Noise XX pairing,
pinned reconnect and availability reads, disconnect invalidation, simultaneous
unknown and pinned peers, exact unpair during an active session, owner-control
response loss and capacity, identity recovery, and changed-key rejection. The
fresh independent review found no remaining reachable P0/P1 contract
violation after its required repairs and full reverification. The Phase 3
implementation is complete; its live local pairing demonstration remains a
separate explicit launch checkpoint.

## Phase 3P: CoreS3 paired availability demo

The accepted contract and 2026-08-24 approval are in
[`phase-3p-physical-slice-proposal.md`](phase-3p-physical-slice-proposal.md) and
[ADR-0005](decisions/0005-core-s3-paired-availability.md). Commit that contract
checkpoint before adding code, dependencies, device state, or LAN exposure.
Then implement the shared `no_std` client state machine, exact private-LAN host
mode, independent CoreS3 Zephyr application, fail-closed NVS stores, bounded
USB control, encrypted experimental profile tooling, and reproducible tests as
a separate local checkpoint.

Phase 3P keeps Phase 3 protocol bytes and pairing semantics unchanged. It
proves the same availability slice on the current CoreS3 before any provider
connector. Reproducible build and fake-boundary verification do not authorize
live device mutation. Flashing, real provisioning, identity initialization,
pairing, and power cycling require an immediate qualification approval after
showing the exact targets and retained plaintext-NVS state.

The approved physical qualification completed on 2026-08-29. Its observed
pairing, cancellation, availability mapping, disconnect recovery, pinned
reconnect, power-cycle recovery, repairs, verification limits, and retained
plaintext-NVS state are recorded in
[`phase-3p-physical-qualification.md`](phase-3p-physical-qualification.md).
The selected residual state retains demo firmware, Wi-Fi credentials, and Noise
identity in device flash; cleanup is never implicit.

## Deferred connector example: Unraid read-only feature

The unapproved example contract is
[`phase-4-unraid-read-only-proposal.md`](phase-4-unraid-read-only-proposal.md).
It is retained as a possible future connector, not the next checkpoint. Do not
add an ADR, source, dependencies, credentials, or provider access until its
domain mapping, TLS trust, credential storage, protocol additions, bounds, and
exact dependencies are reviewed and accepted.

After approval, add an infrastructure-status feature and an Unraid connector.
Keep Unraid payloads and credentials in the host. Normalize only the semantics
needed by the accepted UI, and prove the connector against an isolated fake or
test boundary before any live system access.

Live Unraid inspection and control require a separate explicit authorization.
Do not broaden credentials or mutation permissions after an access denial.

## Deferred connector extension: confirmed Unraid actions

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
- production Embassy executor and thread topology beyond Phase 3P's UI owner
  and service worker;
- desktop runtime and support beyond the approved Linux loopback, exact
  RFC1918 physical host mode, and simulator;
- protocol transport beyond those exact TCP modes and compatibility beyond
  protocol major 1 and bootstrap schema 1;
- physical-device identity storage beyond the approved CoreS3 NVS schema;
- host process and connector isolation;
- UI navigation and shared feature-surface contracts;
- application behavior beyond availability invalidation and bounded reconnect
  while the host is disconnected;
- OTA, rollback, and firmware compatibility policy;
- packaging, repository split, and release strategy.

The first-spike Zephyr and Rust revisions are fixed by the approved Phase 0
baseline. If drift makes a pin unusable, revise and reapprove the baseline
before changing it.

Resolve an item only when its first vertical slice needs it. Record a
cross-cutting decision as a new ADR.

## Resume instructions

Start a future development session with:

> Read `AGENTS.md`, `docs/architecture.md`, the accepted ADRs, and the current
> checkpoint in `docs/implementation-plan.md`. The maintained implementation is
> the portable core/protocol, Linux host and simulator, and physically
> qualified CoreS3 slice; completed Gate harnesses were removed while their
> contracts and evidence remain in `docs/`. Define and approve the next
> development-foundation slice before adding code or dependencies. Treat
> Unraid as a deferred example, not the next action. Do not flash, provision,
> mutate device state, power-cycle, access a provider, release, push, or publish
> without the corresponding explicit approval. Preserve and report the
> selected plaintext-NVS residual state after any approved device run.
