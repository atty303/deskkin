# Phase 2 portable interaction proposal

## Status and scope

This proposal defines the first durable product slice after the completed
Phase 1 feasibility gates. It is an approval checkpoint, not authorization to
add the workspace or application code.

The slice proves one portable character interaction on a native desktop window
and in a deterministic headless simulator:

```text
activate character
  -> portable state transition
  -> request one bounded wait effect
  -> publish a delighted view
  -> receive success or injected failure
  -> publish the corresponding final view
```

It does not add a device build, Embassy runtime, Zephyr adapter, host process,
protocol, connector, provider data, credential, application or provider
persistence, networking, audio, conversation, or infrastructure-status feature.
The bounded local scenario results and diagnostic records defined below are the
only persistent runtime state in scope. The Phase 1 gate applications remain
independent feasibility evidence and are not promoted into product crates.

The deliverable is durable. The trusted control domain is the user's local
development machine. Crates fetched through the approved Cargo lock are
adopted build inputs. There is no input, account, resource, or executable code
controlled by an external service at runtime, and no external state change.

## Interaction contract

The portable core owns only closed, allocation-free domain values:

- `CharacterState`: `Resting`, `WaitingForRest`, or `NeedsAttention`;
- `Command::Activate`;
- completion events carrying the exact effect identity and either success or
  `WaitError::Unavailable`;
- `EffectRequest::Wait` with an effect identity and an explicit duration in
  integer milliseconds;
- `CharacterView` with a closed expression and whether activation is accepted.

An activation while resting transitions to `WaitingForRest`, publishes a
delighted view, and requests one 800 ms wait. The matching successful
completion returns to the resting view. The matching failed completion enters
`NeedsAttention` and publishes a concerned view without retrying or silently
claiming success. A stale or mismatched completion is a typed transition error
and cannot mutate state. A command received while an interaction is active is
rejected explicitly and does not create another effect.

The core transition is synchronous and total over its accepted inputs:

```text
current state + typed input -> new state + optional effect + view
```

It reads no ambient clock, randomness, environment, global runtime, filesystem,
Slint object, or platform API. Effect identities come from core-owned bounded
state rather than a runtime-generated UUID.

## Smallest workspace shape

Only these product paths are authorized by the proposed slice:

```text
Cargo.toml
Cargo.lock
crates/
  application-core/
  slint-presenter/
apps/
  simulator/
ui/
  character.slint
```

The implementation may also make the minimum changes to `mise.toml`, `hk.pkl`,
and `.gitignore` needed to connect the new workspace to the existing
`mise run check`, `mise run fix`, and `mise run test` entrypoints and to keep
root Cargo build output untracked. CI already invokes `mise run test`, so a CI
workflow change is not authorized unless that verified premise changes. No
other repository tooling or configuration is authorized by this proposal.

`application-core` is `#![no_std]` and contains the state machine and domain
types. `slint-presenter` owns the typed mapping between `CharacterView` and the
shared Slint component. `apps/simulator` owns two hosted binaries and their
adapters:

- a native desktop binary with the Slint winit event loop and a monotonic wait;
- a scenario binary with virtual time, scripted effects, headless software
  rendering, deterministic replay, and local diagnostic recording.

The simulator package may contain private modules for the shared hosted runtime
and recorder. Separate runtime, feature API, diagnostics, desktop-platform, or
scenario crates are not justified by this slice and must not be added merely to
match the architecture map.

One Slint owner receives typed commands and applies typed views. Neither the
core nor effect adapter mutates Slint properties directly. The native and
headless binaries compile the same `ui/character.slint` source and use the same
presenter.

## Virtual time and replay

The scenario binary uses a single-threaded deterministic queue ordered by
virtual due time and effect identity. Advancing virtual time is an explicit
control operation; the simulator never sleeps or reads wall-clock time.

Two fixed scenarios are included:

1. `activate-success`: activation at 0 ms, matching wait completion at 800 ms,
   and a final resting view;
2. `activate-wait-failure`: activation at 0 ms, injected
   `WaitError::Unavailable` at 800 ms, and a final needs-attention view.

Each scenario runs twice from a fresh initial state. After excluding diagnostic
run identity and real recording timestamps, the semantic input sequence,
effects, transitions, views, virtual timestamps, final state, and normalized
RGB framebuffer bytes must match exactly between the two runs. The failure
scenario must differ from the success scenario at the completion event,
transition, final view, and final state.

Scenario files are repository-owned test fixtures with a closed schema. This
slice does not accept arbitrary user scripts or deserialize provider payloads.

## Observation contract

This path is subject to the repository's program observation contract because
it crosses a UI/runtime boundary, schedules an asynchronous effect, and has
multiple failure stages.

### Interfaces

| Surface | Native desktop binary | Scenario binary |
| --- | --- | --- |
| Result | Character view in the window | One concise stdout summary and an atomic structured result file |
| Control | Window activation and close | Scenario selection, recording opt-out, exit status, and bounded cancellation |
| Diagnostic | Bounded local interaction runs | Bounded local scenario runs |

The public view and scenario result contain semantic outcome only. Internal
operation detail is not written to the UI, stdout, or stderr. A failed public
result may include the diagnostic run ID needed to locate its out-of-band
record.

### Diagnostic run and operations

One activation through its terminal view is one diagnostic run. The resource
allowlist contains program name and version, binary role, build identity, and
schema version. It contains no username, home path, hostname, environment dump,
command line, or process environment.

The minimum operations are:

- `interaction.run` for the complete interaction;
- `core.transition` for typed state transition and validation;
- `effect.wait` for scheduling and completion;
- `presenter.apply_view` for publishing the view;
- `renderer.frame` only when a headless framebuffer artifact is requested.

The minimum events are `command.accepted`, `command.rejected`,
`state.changed`, `effect.requested`, `effect.completed`, `effect.failed`, and
`view.published`. Operation status distinguishes `success`, `error`, `cancel`,
and `timeout`. Stable error types are `interaction_busy`,
`effect_identity_mismatch`, `wait_unavailable`, `scenario_invalid`, and
`render_failed`. Recorder failures are recording degradation, not interaction
errors.

Operation and event attributes are a source-side allowlist of enum names,
effect identity, integer virtual time or duration with units, and bounded
numeric render dimensions. There is no free-form text field. Raw pointer
coordinates, screen contents outside the Deskkin window, paths, secrets,
confidential content, and privacy-sensitive input are not recorded.

### Recording and retention

Instrumentation and local recording are on by default and can be disabled for
either binary. There is no remote exporter or network destination. The host
application owns recording configuration and storage. `application-core`
remains a pure value transformation with no diagnostic sink or context; the
hosted runtime records its typed input and returned transition at the same call
boundary. `slint-presenter` may receive host-provided diagnostic context but
does not choose a provider, exporter, or path.

Records live below ignored `.deskkin/phase2/diagnostics/` with directory mode
0700 and file mode 0600. A run is published atomically with completeness
`complete`, `partial`, or `dropped`. Partial records include a stable reason and
known missing range or count. If the run itself cannot be stored, a bounded
recorder-health record reports degradation without changing the application
outcome.

Retain at most ten recent successful runs and twenty failed, timed-out,
cancelled, or explicitly retained runs, subject to a 32 MiB total cap. Evict
the oldest unretained success first and then the oldest unretained failure.
Explicitly retained runs count toward the cap and cannot prevent the recorder
from degrading safely when the cap is exhausted. Provide list, retain,
unretain, and exact-run delete controls; never accept a broad directory as a
delete target.

All buffers, flushes, file retries, and shutdown work are bounded. Recording
disabled, consumer absent, capacity exhausted, or storage failure must not
change state transitions, views, effects, stdout contract, exit meaning, or
framebuffer output. Remote export is out of scope and remains absent.

### Conformance scenarios

The reproducible test entrypoint must cover:

- success and injected wait failure with their distinct terminal states;
- exact two-run replay equality for both fixed scenarios;
- mismatched completion identity and interaction-busy rejection;
- recording on/off equivalence for semantic results and framebuffer bytes;
- recording storage failure and capacity exhaustion without semantic change;
- complete, partial, and dropped classification;
- success grace retention, failure-priority retention, explicit retention, and
  exact-run deletion;
- no remote connection attempt;
- a privacy fixture proving the allowlist excludes paths, environment values,
  arbitrary text, and credential-pattern values;
- one Slint owner and the same presenter/UI source in native and headless paths.

The native window remains a user-visible boundary. The implementation
checkpoint must include an observed launch, activation, delighted view, and
return to resting; automated headless replay is not a substitute for claiming
that native window behavior.

## Dependencies and pins

The current Rust stable release was rechecked as 1.98.0 on 2026-08-23. This
slice intentionally retains the repository's already installed and verified
Rust 1.95.0 pin: no accepted requirement needs 1.98.0, and changing the shared
pin would add Phase 1 compatibility work without improving this slice.

| Package | Exact selection and first use | Main alternative | Impact |
| --- | --- | --- | --- |
| Rust | Existing `1.95.0`; workspace build, `no_std` core, hosted binaries | Upgrade the repository to current `1.98.0` | No new tool or artifact; retaining the verified pin avoids unrelated gate drift. |
| `slint` | Existing `=1.17.1`, default features off; `std`, `backend-winit`, `renderer-software`, and `compat-1-2` for the native binary; software renderer for headless replay | Default Slint feature set; a second UI toolkit; custom framebuffer UI | Reuses the proved UI version while adding only the native backend needed by the slice. No Qt, Skia, FemtoVG, accessibility, system tray, or live-preview feature is selected. |
| `slint-build` | Existing `=1.17.1`; compile the one shared `.slint` source | Runtime interpreter | Reuses the proved compile-time path and avoids runtime UI source loading. |
| `serde` | New `=1.0.229`, default features off, `derive` enabled only in the hosted simulator package | Handwritten serializers or an ad hoc text format | Adds a maintained typed serialization layer for scenario, result, and diagnostic schemas. It is not a core or protocol dependency. MIT OR Apache-2.0. |
| `serde_json` | New `=1.0.151`, default `std` support, hosted simulator package only | Handwritten JSON or a new binary format | Adds deterministic structured local artifacts and parser-backed tests. It is not used by `application-core`, on a device, or over a network. MIT OR Apache-2.0. |

Cargo resolves the full transitive set once into the committed root
`Cargo.lock`. Implementation must review that resolution for duplicate runtime
owners, build scripts, licenses, and features before accepting it. No async
executor, timer crate, UUID generator, CLI parser, tracing SDK, OTel SDK,
exporter, hashing crate, test package, or device dependency is authorized.

Using Serde rather than handwritten encoding adds two direct hosted
dependencies but removes a custom schema encoder/decoder and makes malformed
fixtures and forward-incompatible fields observable as typed errors. Reusing
Slint is required by the accepted architecture; a custom UI would duplicate
the already selected platform and make the shared-presenter claim weaker.

## Licensing boundary

Slint 1.17.1 remains available under GPL-3.0-only, the Slint royalty-free
desktop/mobile/web license, or a paid Slint software license. The royalty-free
license does not cover embedded systems. For this local Phase 2 checkpoint,
the recommendation is the already used GPL-3.0-only option, with no binary,
firmware, package, or release distribution. Deskkin's MIT source may remain
MIT, while any combined local Slint binary is treated as GPLv3.

Before any distribution or before promoting this UI into a product device
build, choose and record one compatible distribution model: GPLv3 for combined
binaries, or an applicable paid embedded license. The royalty-free desktop
license must not be assumed to authorize CoreS3 distribution. This is a project
decision, not legal advice.

## Acceptance and stop conditions

The slice passes only when all of the following are observed:

1. `application-core` builds for a repository-selected `no_std` target and its
   dependency graph contains no `std`, Slint, runtime, platform, serialization,
   filesystem, or network package;
2. transition tests prove the interaction contract, including stale identity
   and busy-command rejection;
3. both fixed scenarios replay twice with byte-identical semantic records and
   normalized framebuffer output after excluded run metadata;
4. the injected failure reaches `NeedsAttention` without retry or fabricated
   success;
5. native and headless paths compile the same UI and presenter, and the native
   interaction is visibly observed;
6. the applicable observation conformance scenarios pass;
7. `mise run fix` and the final `mise run test` pass from the locked workspace.

Stop without extending the slice if any of these require a new dependency,
protocol message, connector, provider access, device adapter, platform-specific
core type, second UI owner, ambient time in the core, unbounded recording, or
remote export. Revise this proposal and request approval instead.

## Approval checkpoint

Approval of this proposal authorizes only the product workspace, the listed
minimum repository-tooling integration, and the Phase 2 slice described above,
using the exact direct dependencies and bounded local result/diagnostic state
listed here. It does not authorize a device build or flash, distribution, push,
release, protocol, connector, live service access, credential use, or external
state change.

Approval is required for these four items before implementation:

1. the one-interaction state, effect, failure, view, smallest-workspace, and
   standard verification-entrypoint contract;
2. the exact Rust, Slint, Serde, and serde_json selections and feature bounds;
3. local non-distributed GPLv3 Slint use with the later device/distribution
   license stop retained;
4. the diagnostic schema, default-on bounded local recording, retention, and
   conformance contract.

## Upstream evidence refreshed on 2026-08-23

- <https://doc.rust-lang.org/releases.html>
- <https://github.com/slint-ui/slint/releases/tag/v1.17.1>
- <https://slint.dev/terms-and-conditions>
- <https://slint.dev/pricing>
- <https://docs.rs/crate/serde/1.0.229>
- <https://docs.rs/crate/serde_json/1.0.151>
