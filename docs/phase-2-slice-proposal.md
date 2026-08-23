# Phase 2 periodic infrastructure-status proposal

## Status and scope

Approved on 2026-08-23. This durable slice introduces a provider-neutral,
periodically refreshed availability status. It is limited to Linux and contains
only a dependency-free portable `application-core` crate and one hosted
`simulator` crate. It adds neither a protocol nor an Unraid connector, device
build, manual refresh, character UI, status metrics, or provider payload.

The trusted control domain is the user's local development machine. Locked
Cargo dependencies are adopted build inputs. Runtime input is repository-owned
fake data; no account, credential, remote service, or externally controlled
code is used.

## Portable contract

`application-core` is pure `#![no_std]` Rust with no dependencies. Its public
domain interface is fixed to:

- `Availability`: `Available` or `Unavailable`;
- `StatusView`: `Unknown`, `Available`, or `Unavailable`;
- `Command::Start`;
- `EffectRequest::ReadAvailability` and
  `EffectRequest::ArmRefreshTimer { delay_ms: 5000 }`, each with a core-owned
  bounded effect identity;
- typed read completion, timer-arm completion, and refresh-due events;
- explicit `Stopped`, `Reading`, `ArmingRefresh`, and `Waiting` states.

Start requests an immediate read. A successful read publishes its availability
and arms a five-second timer. A read failure publishes `Unknown` and arms the
same retry timer. A matching refresh-due event starts the next read. A timer-arm
failure publishes `Unknown` and stops periodic execution. Duplicate start,
mismatched or stale effect identity, and exhausted effect identities are typed
errors and do not mutate state.

The core reads no clock, filesystem, environment, Slint object, serializer,
runtime, or platform API. Its transition remains a synchronous typed value
transformation.

## Linux simulator and UI

The hosted simulator package owns a native desktop binary and a headless
scenario binary. Presenter, runtime adapter, and diagnostic recorder are
private shared modules. Both binaries compile the same Slint source and use the
same presenter.

The 320 x 240 surface contains only the status. Text and color distinguish
`Unknown`, `Available`, and `Unavailable`; it has no character representation
or operation button. The native runtime completes fake reads after 250 ms and
repeats `Available`, `Unavailable`, then read failure. After starting a Slint
timer it checks `running()` and converts failure to a typed timer-arm
completion.

The headless runtime uses Slint's software renderer and advances only explicit
virtual time. It provides two fixed scenarios:

- `periodic-success`: `Available` at 250 ms, refresh due at 5250 ms, and
  `Unavailable` at 5500 ms;
- `periodic-read-failure`: read failure and `Unknown` at 250 ms, refresh due at
  5250 ms, and `Available` at 5500 ms.

Each scenario runs twice from fresh state. Excluding diagnostic run identity
and wall-clock metadata, semantic records, view sequence, virtual timestamps,
and RGB565 frames must be byte-identical.

## Observation contract

The path crosses an asynchronous runtime and UI boundary. One refresh start
through read result, view application, and the terminal timer-arm completion is
one diagnostic run. Success, error, cancellation, and timeout all terminate and
publish that run. Each scenario execution has a separate scenario run identity
and records every child refresh run identity in its atomic result. The single
stdout run ID is the scenario run ID, from which all child refresh runs are
reachable. The source-side closed operation allowlist is:

- `status.refresh`;
- `core.transition`;
- `effect.read_status`;
- `effect.arm_refresh_timer`;
- `presenter.apply_view`.

Attributes are restricted to closed enum values, effect identity, integer
duration or virtual time, and render dimensions. No free-form payload, path,
environment, screen content, identity, credential, or provider data is
recorded.

Records live below ignored `.deskkin/phase2/` with directories mode 0700 and
files mode 0600. Recording is on by default, can be disabled, and has no remote
export. Publication is atomic and reports `complete`, `partial`, or `dropped`
plus bounded recording health. Retention keeps the ten most recent unretained
successful runs and twenty most recent unretained failed, cancelled, or
timed-out runs under a total 32 MiB cap. Explicitly retained runs are pinned
outside those outcome-count limits but still count toward the byte cap. Oldest
unretained successes are evicted before oldest unretained failures. Explicit
retention cannot turn capacity exhaustion into an application error.

List, retain, unretain, and exact-run delete are the only recorder
administration controls. Symlinks are rejected. Publication, retention, and
control operations are serialized with `File::lock()`. Scenario results are
atomically replaced per scenario. Scenario stdout is exactly one line
containing result, scenario run ID, and result path. Recorder failure never
changes semantic results, frames, or exit meaning. Driver outcomes distinguish
cancellation and timeout.

## Workspace, dependencies, and tooling

The authorized product shape is:

```text
Cargo.toml
Cargo.lock
crates/application-core/
apps/simulator/
```

Rust remains 1.95.0 with `clippy`, `rustfmt`, and the existing
`thumbv7m-none-eabi` target. Only the simulator directly depends on exact
`serde = 1.0.229`, `serde_json = 1.0.151`, and `slint`/`slint-build = 1.17.1`.
Slint disables defaults and enables only `std`, `backend-winit`,
`renderer-software`, and `compat-1-2`. No async executor, timer crate, UUID
crate, CLI parser, tracing stack, exporter, hashing crate, test dependency, or
device dependency is approved.

`hk.pkl` gains selected-file Rust formatting. `mise run test` performs locked
Clippy, workspace tests, both headless scenarios, and a
`thumbv7m-none-eabi` core check. Root `Cargo.lock` is committed.

## Licensing

Deskkin source remains MIT. Slint 1.17.1 is adopted under GPL-3.0-only for this
slice, and a combined binary containing Slint may be distributed only under
GPLv3. Phase 2 implementation must add the GPLv3 text and an explicit licensing
note. No package, release task, publication, or artifact distribution is added
here.

Before a binary is actually distributed, a separate checkpoint must approve
the corresponding-source bundle and its delivery method. Any future paid
Slint license switch must separately approve the covered Slint version, seats,
use, and embedded royalty terms; it does not change the license of binaries
already distributed under GPLv3. See <https://slint.dev/terms-and-conditions>.

## Acceptance criteria

The slice passes only when:

1. core tests cover startup, both availability values, read failure and retry,
   timer-arm failure, duplicate start, stale identities, identity exhaustion,
   and the rule that failure displays `Unknown`;
2. both headless scenarios replay twice byte-identically and recording on/off
   produces identical semantic results and frames;
3. native and headless binaries share the Slint source and presenter;
4. recorder tests cover storage and capacity failure, partial and dropped
   health, retention priority, retain/unretain, exact deletion, symlink
   rejection, privacy allowlisting, absence of remote connection, failure
   non-interference, cancellation, and timeout;
5. `cargo tree` shows no `application-core` dependencies;
6. targeted `mise run fix` and final `mise run test` pass;
7. the GPLv3 text and licensing note preserve the MIT source and GPLv3 combined
   binary boundary;
8. after explicit launch approval, a human observes the native Linux sequence
   `Unknown -> Available -> Unavailable -> Unknown`.

## Approval record

On 2026-08-23 the four revised items were approved: the periodic availability
contract and two-crate boundary; the exact dependency and feature selections;
MIT source plus GPLv3 combined-binary distribution; and the bounded default-on
diagnostic recording and conformance contract. This approval excludes push,
release, artifact publication, protocol, connector, provider access, device
adapter, and non-Linux desktop support.
