# Phase 0 feasibility and dependency proposal

- Status: Proposed; implementation and dependency approval required
- Evidence checked: 2026-08-22
- Scope: Phase 1 foundation spikes only

## Recommendation

Approve a bounded Phase 1 experiment based on Zephyr 4.4.1, the
`zephyr-lang-rust` revision selected by that Zephyr release, Rust 1.95, and
Slint 1.17.1. Keep the five feasibility gates independent and stop after the
first failing boundary rather than repairing later layers around it.

This proposal does not authorize a Rust workspace, source code, toolchain
configuration, dependency installation, board changes, flashing, or release.
Those changes begin only after the approval items at the end of this document
are accepted.

The largest uncertainty is narrower than originally expected. Zephyr 4.4.1
already contains a CoreS3 board definition with ILI9342C display, FT6336U
touch, AXP2101 power, AW9523 GPIO expansion, PSRAM, flash, and buses. The
remaining critical path is whether the Rust integration can use the
`xtensa-esp32s3-none-elf` target with Zephyr's ABI, linker, compiler builtins,
bindings, allocator, and atomic assumptions.

## Pinned foundation

| Component | Proposed pin | First use | Reason for the pin |
| --- | --- | --- | --- |
| Zephyr | `v4.4.1`, commit `1f6485eca25431b5ff27ce9a754218c9e559bbbb` | Gates 1A-1E | Current supported 4.4 patch release; includes CoreS3 and selects SDK 1.0.1. |
| `zephyr-lang-rust` | commit `dd73abc242e995784da62352fe8c70d9a6c7ac2e` | Gate 1A | Exact optional-module revision in Zephyr 4.4.1's manifest; avoids an untested main-branch combination. |
| Rust | `1.95.0` with Cargo `1.95.0` | Gates 1A-1C and 1E | Satisfies `zephyr-lang-rust`'s Rust 1.85 minimum and Slint 1.17.1's Rust 1.92 minimum while matching the latest non-prerelease ESP Xtensa toolchain line. Gate 1D deliberately has no Rust dependency. |
| ESP Rust Xtensa toolchain | `esp-rs/rust-build` `v1.95.0.0`, commit `dc1dba6a3e0da1fe254a2ea5ba47c00c6544b402` | Gate 1C | Provides `xtensa-esp32s3-none-elf`; upstream Rust classifies the target as Tier 3 and does not distribute it through the standard stable channel. |
| Slint | `1.17.1`, commit `cf62c975c311e7036d599ed8ed0b7e6a8386a934` | Gate 1B | Current patch release, `no_std` software renderer, Rust 1.92 MSRV, and an upstream CoreS3 `esp-hal` implementation as a hardware reference. |
| Zephyr SDK | `1.0.1` | Gates 1A, 1C, 1D | Version selected by Zephyr 4.4.1; includes the ESP32-S3 Xtensa GCC fix and target/host tools. |
| Python | `3.12.14` | Zephyr host tooling | Zephyr 4.4 requires Python 3.12 and warns that newer feature releases can be incompatible. |
| CMake | `3.28.6` | Zephyr configuration | Last 3.28 patch line; matches the 3.28 minimum in current Zephyr guidance and `zephyr-lang-rust` samples. |
| Ninja | `1.13.2` | Zephyr builds | Pinned current patch release; no project-specific customization. |
| west | `1.5.0` | Workspace/module/SDK fetch | Stable west release, installed inside the project Python environment rather than globally. |
| `espup` | `0.17.1` | Xtensa toolchain install | Official installer for the pinned ESP Rust toolchain; it is not used by non-Xtensa gates. |

Tags are convenience labels only. Repository manifests and Cargo lockfiles
must retain immutable commits, exact package versions, and artifact checksums.

## Proposed direct dependencies

Only dependencies first exercised by a Phase 1 gate are in scope. The union of
the two planned Cargo manifests is fixed below. The baseline manifest contains
`zephyr`, `log`, `static_cell`, `embassy-executor`, `embassy-sync`,
`embassy-time`, and `critical-section`. The Slint manifest reuses that set and
adds `slint` plus the `slint-build` build dependency. `zephyr-build`,
`zephyr-sys`, and `zephyr-macros` are transitive path dependencies of
`zephyr`, not Deskkin direct dependencies.

| Direct dependency | Version/source and purpose | Alternative and rejection | Maintenance | License | Safety | Reproducibility |
| --- | --- | --- | --- | --- | --- | --- |
| `zephyr` | `0.1.0` from the pinned `zephyr-lang-rust` commit; 1A integration for Kconfig, devicetree, logging, allocation, panic, raw bindings, and the Zephyr-hosted Embassy adapter | Hand-written C ABI duplicates an early upstream integration; a C++ application layer violates the selected Rust path. | High while the upstream API is pre-1.0; upgrades require rerunning 1A-1C. | Apache-2.0 | Generated/raw bindings and FFI remain behind the platform adapter; causes and panic state must cross the boundary intact. | Patched to the immutable module path selected by the west manifest; Cargo lock records all transitive crates. |
| `log` | `=0.4.22`; facade used by the pinned Zephyr/Embassy sample and captured by Zephyr's logger | Direct `printk` couples library code to Zephyr and turns public output into diagnostics. | Low; stable facade, no logger implementation owned by portable crates. | MIT/Apache-2.0 | Message fields use a static allowlist; no credential or arbitrary input formatting. | Exact version in Cargo lock; logger implementation comes from the pinned Zephyr adapter. |
| `static_cell` | `=2.1.0`; statically initialize the one executor without `static mut` | Heap allocation makes executor creation depend on allocator availability; handwritten unsafe static initialization adds no value. | Low; narrow single use that can be removed if upstream owns executor storage. | MIT/Apache-2.0 | Encapsulates one-time static initialization; the UI/executor ownership invariant is still enforced by Deskkin types. | Exact version and features in Cargo lock; no build-time download beyond crates.io. |
| `embassy-executor` | `=0.7.0` with `log` and `task-arena-size-2048`; one executor hosted by one Zephyr thread | A Zephyr-only synchronous loop does not prove the accepted async role; the generic Embassy thread executor duplicates Zephyr scheduling. | Medium; feature flags and task arena size are reviewed on upgrade. | MIT/Apache-2.0 | Fixed task arena bounds memory; no interrupt executor or global application-core executor. | Exact version/features in Cargo lock; Gate 1A records the arena configuration. |
| `embassy-sync` | `=0.6.2`; one bounded typed channel/signal in 1A | Custom queues or raw Zephyr synchronization leak platform types and require bespoke wakeup safety. | Medium; version remains aligned with the pinned `zephyr` crate. | MIT/Apache-2.0 | Bounded capacity, no unbounded queue, and one declared mutex strategy; overflow is a typed failure. | Exact version in Cargo lock; no independently floating Embassy version. |
| `embassy-time` | `=0.4.0` with `tick-hz-10_000`; Zephyr-backed timer and Slint animation wakeup | Direct Zephyr timers in presenter/UI code break the runtime boundary and virtual-time design; busy polling is forbidden. | Medium; tick-rate and driver compatibility are remeasured on change. | MIT/Apache-2.0 | One time driver, bounded timer queue, and explicit timeout/cancel status; no ambient clock in the core. | Exact version/features in Cargo lock; build record includes tick rate and Zephyr timer configuration. |
| `critical-section` | `=1.2.0`; the upstream Embassy integration's critical-section contract | Raw interrupt masking is target-specific unsafe code; relying only on a transitive version obscures the selected implementation. | Low, but each new architecture reruns the atomic/critical-section evidence. | MIT/Apache-2.0 | Gate 1C verifies nesting, restoration, and `portable-atomic` assumptions on Xtensa. | Exact direct version in Cargo lock and one selected implementation per binary. |
| `slint` | `=1.17.1`, default features off; `compat-1-2`, `renderer-software`, `unsafe-single-threaded`, and `libm`; shared UI and `no_std` renderer in 1B | LVGL would not preserve the selected shared Slint UI; embedded-graphics or a custom renderer would recreate UI/runtime behavior; C++ Slint violates the Rust-path gate. | Medium-high; pre-2.0 feature/MSRV changes and renderer behavior require rerunning 1B/1E. | GPL-3.0-only or commercial for the embedded product; the royalty-free desktop/mobile/web license excludes embedded systems. | `unsafe-single-threaded` is permitted only with one statically enforced owner; dirty ranges and allocator/panic behavior are tested. | Exact version/features and all transitive crates in Cargo lock; device and host compile the same `.slint` source. |
| `slint-build` | `=1.17.1`; build dependency that compiles the shared `.slint` source in 1B | Inline macro weakens shared source/tooling; committed generated bindings create drift. | Medium; kept exactly equal to `slint`. | Same Slint license choice | Runs only on the host; generated output is not trusted as source and is rebuilt from the pinned compiler. | Exact version in Cargo lock; generated bindings are uncommitted and reproducible from source. |

The planned manifests intentionally do not directly depend on `heapless` or
`embassy-futures`, even though the broader upstream samples use them. The
bounded spike uses Embassy's fixed-capacity channel and no join/select helper.
Adding either package changes this approval table and requires approval first.

Transitive Cargo packages are not independently selected architecture. After
approval, Cargo must resolve them once into a committed `Cargo.lock`; the lock
is reviewed for duplicate runtime versions, license exceptions, source URLs,
and target-specific packages before the first build is accepted. No
application feature, protocol, transport, connector, serialization, network,
storage, or provider crate is in scope.

## Fetch and reproducibility boundary

### Fetched by mise

The repository's mise configuration will pin and provide:

- Rust/Cargo 1.95.0 and the standard targets `thumbv7m-none-eabi` and
  `riscv32i-unknown-none-elf`;
- Python 3.12.14, CMake 3.28.6, Ninja 1.13.2, and `espup` 0.17.1;
- tasks that create a repository-local Python environment and invoke west,
  Cargo, and the existing validation entrypoints.

West 1.5.0 and Zephyr's Python requirements will be installed into the local
environment from a hash-pinned requirements lock. The lock is generated from
the pinned Zephyr and module requirements after approval, before any build.
Range-resolved `pip install` output is not a reproducibility artifact.

### Fetched by west

Use an application-owned manifest with a name allowlist. It fetches:

- Zephyr at the pinned commit;
- `zephyr-lang-rust` at the revision selected by that Zephyr commit;
- only the module projects required by the accepted targets, initially
  `cmsis`, `cmsis_6`, `hal_espressif`, and `hal_xtensa`;
- Zephyr SDK 1.0.1 host tools and only
  `arm-zephyr-eabi`, `riscv64-zephyr-elf`, and
  `xtensa-espressif_esp32s3_zephyr-elf` target toolchains.

If west reports another project as required, add it to the proposal with its
first use before expanding the allowlist. Do not fetch every vendor HAL merely
because it is present in Zephyr's upstream manifest.

### Fetched by Cargo or espup

Cargo fetches only the locked dependency graph of the accepted spike crates.
`espup` fetches the pinned ESP Rust 1.95.0.0 toolchain for Gate 1C. Build
records must include the toolchain identity, target specification digest,
Cargo lock digest, west manifest digest, and Zephyr configuration digest.

No fetched source, SDK, toolchain, build directory, generated binding, or
diagnostic artifact is committed. Only source manifests, locks, small approved
patches, and reproducible tasks are committed.

## Gate matrix

Each gate starts from a clean build directory and produces an isolated
diagnostic run. A later gate may begin only after its prerequisites pass.

| Gate | Target and slice | Pass criteria | Failure boundary |
| --- | --- | --- | --- |
| 1A | `qemu_cortex_m3` and `qemu_riscv32`; optional `m2gl025_miv` only if hardware is available | The pinned upstream-style Rust app configures, compiles, links, boots, emits structured test completion, exercises Kconfig and typed devicetree access, logs, deterministic panic capture, allocation when enabled, Embassy wakeup, and a clean rebuild. | Any unsupported official target, non-reproducible build, lost panic/cause, or direct platform call above the adapter fails 1A. |
| 1B | `qemu_cortex_m3` with a mock RGB565 framebuffer plus a host process using the same `.slint` file and software renderer | Slint compiles for the exact target; static and animated surfaces render; timer wakeups do not busy-poll; mock input reaches a typed callback; returned dirty line ranges match changed pixels; host and target image artifacts match after explicit pixel-format normalization. | A C++ application layer, full-frame-only result, multiple Slint owners, target-incompatible allocator/panic behavior, or target-specific UI source fails 1B. |
| 1C | `m5stack_cores3/esp32s3/procpu` with `xtensa-esp32s3-none-elf` | A minimal Rust static library compiles, links with Zephyr, boots on CoreS3, logs a build identity, exercises one C-to-Rust and Rust-to-C call with checked values, and reaches a deterministic panic record. Linker map and symbol/section reports demonstrate the expected ABI, compiler builtins, target features, and memory placement. | More than the bounded integration patch below, unexplained duplicate builtins, ABI mismatch, incorrect atomics, uninspectable panic, or toolchain-only success without device boot fails 1C. |
| 1D | Existing upstream CoreS3 Zephyr board and drivers, without Slint or Rust | CoreS3 power/GPIO expansion, RGB565 display, FT6336U touch, PSRAM, flash, and required buses initialize independently. Display writes multiple non-fullscreen rectangles at arbitrary valid coordinates; readback where available or a deterministic panel test proves location and extent. Transfer bytes and duration are recorded. | A full-frame-only display path, board-specific application conditional, or required unsupported subsystem fails 1D and is isolated from Rust/Slint. |
| 1E | CoreS3 combined touch-to-Slint and dirty-render-to-display slice | Meet every numeric workload and latency criterion below; typed touch events update the face and only returned dirty regions are transferred. | Any numeric miss, prerequisite regression, hidden full-frame transfer, direct driver access from UI/application code, or missing timing evidence fails 1E. |

Gate 1D should first test the upstream board exactly as shipped. Board or
driver changes are permitted only for an observed failed criterion and become
a separate approval if they exceed a narrow bug fix or missing devicetree
description.

### Gate 1E performance workload

Use the 320x240 RGB565 display at a 30 Hz animation cadence. One `qualification`
invocation, for which local diagnostic recording is enabled, runs two
60-second, 1,800-frame phases in fixed order: the recorder disabled, then the
actual local recorder enabled. Reset the application and animation clock
between phases and replay the same precomputed animation seed and touch
schedule. The build, firmware, target, clock, UI, workload, input schedule, and
configuration digests must match. After one allowed initial full-frame paint
per phase, animate two eyes and one mouth and inject one touch per second.
Changed regions may be split but their union must be at most 19,200 pixels
(25% of the display) per animation frame. The touch changes expression for at
least ten frames so its response is visible in both semantic and framebuffer
evidence.

Discard the first 60 warm-up frames of each phase. Use all remaining scheduled
frame slots as the sample population. Sort completed durations and use the
nearest-rank definition `sample[ceil(p * N) - 1]` for p95 and p99. A frame with
no completion is an infinite-duration sample and a deadline miss; a touch with
no corresponding physical transfer is an infinite-latency sample. Both phases
must meet every non-overhead criterion. The invocation passes only if:

- render duration p95 is at most 12 ms;
- physical display-transfer duration p95 is at most 12 ms;
- render plus transfer duration p95 is at most 25 ms and p99 is at most
  33.3 ms;
- touch-to-first-corresponding-physical-transfer completion p95 is at most
  100 ms;
- at most 1% of measured frames miss the 33.3 ms deadline;
- no post-warm-up transfer exceeds the current frame's dirty-pixel union, and
  no post-initialization full-frame transfer occurs;
- enabled-phase render-plus-transfer p95 is no more than disabled-phase p95
  plus the larger of 5% of disabled-phase p95 or 1 ms.

The enabled phase exercises the production diagnostic path, including
allowlisted encoding, the bounded queue, concurrent filesystem writes, capacity
enforcement, and bounded final flush. The disabled phase performs none of those
operations. The runner retains both phase aggregates and their shared identity
digest in memory, then publishes only the allowlisted aggregates after both
phases.

Top-level `--recording=off` selects `mode=conformance`. That mode runs only the
recorder-disabled 60-second phase, creates no diagnostic directory or record,
and evaluates every non-overhead criterion. It cannot qualify Gate 1E or claim
a recording-overhead result. Its semantic-event and framebuffer digests must
match the disabled phase of a `qualification` run with the same workload
identity. The conformance result links that qualification run explicitly. Gate
1E adoption requires both results to pass and their workload identity,
disabled-phase semantic-event digest, and disabled-phase framebuffer digest to
match.

These are Phase 1 feasibility thresholds, not final animation-quality or
product performance promises. Changing the workload or thresholds changes the
approval baseline.

## Observation and evidence contract

All five gates cross external, asynchronous, multi-stage, or hardware
boundaries and therefore require three separate surfaces.

| Surface | Contract |
| --- | --- |
| Result | After acquiring the gate lock and before execution, remove the previous result for that gate and mode. Each invocation atomically renames a temporary file to `.deskkin/results/<gate>/<mode>/result.json` only after final classification. `.deskkin/results` is mode 0700 and result files are 0600. The stable schema contains `schema_version`, `gate`, `mode`, `run_id`, `result` (`pass`, `fail`, or `inconclusive`), one stable `reason_code`, `cleanup_status`, optional `device_state` and firmware digest, evaluated criteria with numeric value/unit/threshold, and start/end timestamps. Both Gate 1E modes also contain `workload_identity_digest`, `disabled_semantic_event_digest`, and `disabled_framebuffer_digest`; conformance contains its matched `qualification_run_id`. A missing file or mismatched run ID is not success. stdout contains only a one-line human summary, run ID, and result path. At most one result file per gate and mode is retained. |
| Control | `mise run gate:1a` through `gate:1e` invoke one common runner with a gate and target. Gate 1E defaults to `mode=qualification`; `--recording=off` selects its non-qualifying `mode=conformance`. For other gates the flag only opts out of diagnostics. Before touching a result, the supervisor atomically acquires an OS-released exclusive lock for that gate under `.deskkin/locks` and holds it through result publication and cleanup. A competitor exits 2 and reports the owning run ID without deleting or overwriting state. Defaults are 15 minutes for 1A/1B, 20 minutes for 1C, 10 minutes for 1D, and 5 minutes for either 1E mode. SIGINT requests cancellation and triggers bounded cleanup. Exit status is 0 for pass, 1 for gate fail, 2 for invalid setup/invocation or inconclusive cleanup, 124 for timeout, and 130 for cancellation; diagnostic recording failure never changes it. stderr contains only actionable control/setup errors plus the run ID. |
| Diagnostic | Out-of-band records live under `.deskkin/diagnostics/<run-id>/`, separate from `result.json`. `diagnostic.jsonl`, recording health, and allowlisted artifacts describe causal stages. The runner prints the run ID before the first operation so an incomplete live run can be inspected. No diagnostic payload is put in the result schema, stdout summary, serial test protocol, or UI. |

Result publication follows diagnostic finalization: when recording is enabled,
artifacts and the final diagnostic completeness marker are closed first, then
`result.json` is renamed into place. A crash before that rename leaves an
inspectable partial diagnostic run and no result for the current run ID.
Cancellation gives the owned process group and serial device five seconds to
close before forced cleanup, and records cleanup failure without converting the
cancellation into pass.

The lock file stores only gate, run ID, supervisor process ID, and start time,
with mode 0600. The operating-system lock, not file existence, is authoritative
and is released if the supervisor crashes. A new invocation may replace stale
metadata only after it acquires the lock, so it cannot infer that a process is
dead from the PID alone.

One command invocation is a **Diagnostic Run**. Its resource identifies the
Deskkin revision, gate, target, application version, Zephyr and module commits,
Rust and C toolchains, lock/manifest/config digests, build type, and physical
board revision when applicable. Host identity, usernames, absolute paths, and
serial numbers are excluded.

Use stable operations only where they distinguish a failure stage:

```text
prepare -> configure -> rust-compile -> c-compile -> link
        -> flash -> boot -> probe -> render -> transfer -> input
```

Every operation records `success`, `error`, `timeout`, or `cancel` and a stable
error type. Retry attempts are linked rather than overwritten. Build summaries,
validated linker maps, symbol/section summaries, normalized framebuffer images,
schema-validated serial records, and measurement tables are artifacts
referenced by the run; their existence alone is not a pass result.

Recording is enabled and local by default. Remote export does not exist. The
root directory is `.deskkin/diagnostics`, created with mode 0700; files are
0600. Each run is limited to 32 MiB, and the store is limited to 256 MiB, 20
runs, and 14 days for non-frozen runs. Keep the newest success per gate and the
newest three error/timeout runs per gate when capacity permits. Evict oldest
non-frozen surplus successes first, then surplus cancellations, then surplus
error/timeouts. Explicitly frozen runs are never automatically evicted, but
still count toward 256 MiB; if they consume the limit, new recording becomes
`partial` or `dropped` rather than exceeding it or changing the gate result.

`mise run diagnostics:list` shows run ID, gate, mode, time, result,
completeness, and bytes. `mise run diagnostics:delete -- RUN_ID` deletes exactly
one resolved run directory. `--recording=off` creates no diagnostic directory
or record. For Gates 1A-1D that is the only behavior change; for Gate 1E it also
selects the explicitly non-qualifying `conformance` mode defined above.

A run is `complete`, `partial`, or `dropped`. Source-side artifact allowlists
are fixed as follows:

| Artifact | Allowed content |
| --- | --- |
| `diagnostic.jsonl` | Stable operation/event names, status, error type, parent/link IDs, monotonic duration, approved numeric counters, and resource pins/digests listed above. |
| `build-summary.json` | Named build phase, target, exit status, duration, compiler identity, and stable parsed error classifier; no raw stdout/stderr. |
| `link-summary.json` | Section names/sizes, memory-region names/sizes, expected test-symbol names/addresses, and compiler-builtins classification. |
| `linker-map.txt` | Relative source/object identifiers and section/symbol data only, produced with prefix remapping and accepted only after forbidden-path/token scanning. |
| `serial.jsonl` | Only the spike protocol's fixed record types, numeric measurements, build digest, expected test values, panic type, and timestamps; unknown/free-text lines are discarded. |
| `framebuffer.png` | Exactly 320x240 RGB pixels with no text metadata. |
| `measurements.json` | Named numeric metrics, units, sample count, quantiles, thresholds, and pass booleans. |

Credentials, environment values, unrestricted command lines, raw build logs,
absolute paths, host/user identity, device serial numbers, arbitrary serial
text, raw memory, and unallowlisted metadata are forbidden. Before atomic
artifact publication, validation scans schema, size, path form, forbidden key
names, and credential fixtures. Validation or redaction failure fails closed:
the artifact is omitted, the run is marked `partial` with
`privacy_filter_failed`, and the gate result remains independent.

### Physical device residual state

Gates 1C-1E deliberately replace CoreS3 flash contents with approved test
firmware. There is no prior Deskkin product firmware to restore. After pass,
fail, timeout, or cancellation, the runner gives device cleanup ten seconds to
reset into that same test firmware's inert idle mode. Idle mode runs no
workload, starts no network or external service, holds no credential, performs
no persistent mutation, and reports its firmware digest and idle status over
the fixed serial schema. The final result records
`device_state=test_firmware_idle` and that digest.

For pass or fail, inability to prove idle state changes the result to
`inconclusive`, reason `device_cleanup_failed`, and exit 2. Timeout retains exit
124 and cancellation retains exit 130, with cleanup failure recorded in
`cleanup_status` and the residual state marked `unknown`. A supervisor crash
cannot claim cleanup. The next physical-gate invocation must preflight the
device before flashing or running: reset a recognized test firmware to idle,
or stop with exit 2 and `device_state_unknown`.

`mise run device:recover -- --expected-firmware DIGEST` is the explicit manual
recovery surface. It may reset or reflash only that approved test firmware and
must re-read the idle digest. If recovery fails, it reports the remaining
unknown state and the required power-cycle or approved reflash step; it does
not restore an unknown prior image or report success. The Phase 1 approval
therefore also accepts that CoreS3 remains flashed with inert test firmware
until the user chooses a later firmware.

The first implementation must verify these proportional conformance cases:

- for the same requested non-overhead behavior, observation enabled and
  disabled preserve semantic results and exit-status meaning; Gate 1E compares
  its conformance result only with the recorder-disabled phase of a matching
  qualification run, never with the qualification classification itself;
- build or serial-capture storage failure degrades the recording, not the
  product operation;
- timeout and panic are distinct from cancellation;
- a deliberately truncated artifact marks the run partial;
- no remote destination is contacted;
- a sensitive fixture is absent from records and artifacts;
- a concurrent invocation of the same gate is rejected without changing the
  owner's result or diagnostic state;
- a Gate 1E phase-identity mismatch is `inconclusive` rather than a performance
  pass;
- a simulated supervisor crash after flash requires device preflight and
  cannot reuse the prior run's result;
- host-side libraries accept a diagnostic sink and do not own a global
  exporter or storage location.

## Temporary patch boundary and upstream strategy

Do not create a remote fork initially. Keep an ordered, checksum-verified local
patch series against the pinned `zephyr-lang-rust` commit.

The accepted Gate 1C patch may contain only:

1. the Xtensa Zephyr-to-Rust target mapping;
2. target-specific compiler/link arguments required to match the pinned
   Zephyr ESP32-S3 build;
3. narrowly demonstrated `cfg`, atomic, binding, or compiler-builtins fixes;
4. the smallest upstreamable test or sample proving each change.

Stop and request a new architecture decision if success requires changes to
Zephyr scheduling, the Rust calling convention, Rust compiler source, a
general driver subsystem, Slint, or more than one maintained module boundary.
Any retained patch must carry its upstream issue or pull-request reference,
rationale, affected pins, verification, and removal condition. Creating an
upstream fork or pull request is a separate explicit remote-state action.

## Removal and stop criteria

- A failed gate removes its application, generated artifacts, target-specific
  tasks, and dependencies that have no earlier passing gate.
- A Gate 1C failure removes the ESP Rust toolchain, `espup`, and local
  `zephyr-lang-rust` patches; it does not invalidate Gates 1A or 1B.
- A Gate 1D failure removes local board/driver experiments while retaining the
  untouched upstream board evidence.
- A Gate 1B failure removes Slint spike code and Slint dependencies; it does
  not leave a compatibility wrapper or C++ fallback.
- No experimental code is promoted to the application-core or protocol
  architecture. Passing spike code is still deleted or simplified if its only
  value was proving feasibility.
- The combined path is not adopted unless 1B, 1C, and 1D pass independently and
  both Gate 1E modes pass: `qualification` meets every numeric criterion,
  including measured recording overhead, and `conformance` meets every
  non-overhead criterion without producing diagnostics. The adoption check
  additionally requires conformance's `qualification_run_id` to equal the
  retained qualification result's run ID and all three identity/digest fields
  in the two results to match.

## Licensing and safety decision

Zephyr, `zephyr-lang-rust`, and Zephyr SDK are Apache-2.0. Rust and the proposed
Embassy crates are MIT/Apache-2.0. Slint 1.17.1 is offered under
GPL-3.0-only, a royalty-free license that excludes embedded systems, or a
commercial software license.

For the local, non-distributed feasibility spike, the recommendation is to use
Slint under GPL-3.0-only and create no firmware or binary release. Before any
distribution or product adoption, explicitly choose either a GPL-compatible
distribution model or a commercial Slint embedded license. The repository's
MIT source license does not by itself resolve the license obligations of a
combined binary. This is a project decision, not legal advice.

The main safety obligations are:

- keep all generated/raw Zephyr bindings and C FFI behind a narrow platform
  adapter;
- permit Slint's `unsafe-single-threaded` feature only with one statically
  enforced UI owner and message-based access;
- verify `portable-atomic` and critical-section assumptions on Xtensa rather
  than inheriting them from a successful link;
- preserve panic causes and typed failures across the C/Rust boundary;
- do not put credentials, provider access, network services, or external
  mutation in any Phase 1 gate.

## Approval checkpoint

Approval of this proposal authorizes only the Phase 1 foundation work and its
listed dependencies, pins, local artifacts, and bounded patch series. It does
not authorize release, push, remote fork/PR creation, live external services,
or application features.

The approval must explicitly accept:

1. the pinned foundation and direct dependencies above;
2. the local, non-distributed GPL-3.0 Slint feasibility use and the later
   distribution-license stop;
3. the bounded `zephyr-lang-rust` patch policy;
4. physical CoreS3 flashing and observation for Gates 1C-1E when those gates
   are reached.

## Primary evidence

- [Zephyr supported releases](https://github.com/zephyrproject-rtos/zephyr/blob/main/doc/releases/index.rst)
- [Zephyr 4.4.1 optional module manifest](https://github.com/zephyrproject-rtos/zephyr/blob/v4.4.1/submanifests/optional.yaml)
- [Zephyr 4.4.1 CoreS3 board](https://github.com/zephyrproject-rtos/zephyr/tree/v4.4.1/boards/m5stack/m5stack_cores3)
- [Zephyr Rust integration](https://github.com/zephyrproject-rtos/zephyr-lang-rust/tree/dd73abc242e995784da62352fe8c70d9a6c7ac2e)
- [Rust Xtensa target support](https://doc.rust-lang.org/rustc/platform-support/xtensa.html)
- [ESP Rust Xtensa toolchain releases](https://github.com/esp-rs/rust-build/releases)
- [Slint 1.17.1 release](https://github.com/slint-ui/slint/releases/tag/v1.17.1)
- [Slint 1.17.1 Cargo workspace and MSRV](https://github.com/slint-ui/slint/blob/v1.17.1/Cargo.toml)
- [Slint CoreS3 reference](https://github.com/slint-ui/slint/tree/v1.17.1/examples/mcu-board-support/m5stack_cores3)
- [Slint licensing options](https://github.com/slint-ui/slint/tree/v1.17.1/LICENSES)
- [Zephyr SDK 1.0.1](https://github.com/zephyrproject-rtos/sdk-ng/releases/tag/v1.0.1)
