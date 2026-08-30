# Foundation D proposal: repeatable development loop

Status: Accepted and implemented locally; remote CI qualification awaits an explicit push

Date: 2026-08-30

## Goal and observable result

Shorten the feedback loop for ordinary host, portable-core, protocol, and
simulator changes without weakening the complete reproducible verification
boundary. Keep clean CoreS3 product and inert firmware builds mandatory for
completion, but make them an independent conformance lane rather than a
prerequisite for seeing host failures.

The checkpoint is observable when:

- one host/portable command exercises every non-CoreS3-build check without a
  prepared Zephyr/Xtensa workspace;
- one CoreS3 command verifies the prepared toolchain and performs both clean
  firmware builds without rerunning host tests;
- `mise run test` executes the exact union of those lanes and remains the sole
  complete local acceptance entrypoint; and
- the CI definition starts both lanes independently and requires both lanes for
  workflow success; and
- after a separately requested push, one clean run confirms that a host result
  is available independently of CoreS3 bootstrap.

This is durable development infrastructure. It changes no product result,
protocol byte, UI, runtime state, device state, dependency, or release policy.

## Current evidence and problem

On 2026-08-30, the current warm local `mise run test` completed in about 50
seconds. The latest successful `main` CI run took about 13 minutes. CI performs
disk reclamation and the full CoreS3 toolchain bootstrap before it can report a
Rust format, lint, host test, protocol, or simulator failure.

Before the separately authorized cleanup, the local repository contained about
206 GiB below `.deskkin` and 35 GiB below `target`; about 196 GiB of `.deskkin`
was ignored output from removed Gate build paths. The filesystem was 93 percent
full at inspection. The explicit cleanup removed only `.deskkin/build`, leaving
about 11 GiB below `.deskkin` and increasing free space from 139 GiB to 333 GiB.
These figures are one-workstation observations, not portable thresholds.

The obsolete ignored Gate output was a one-time local cleanup concern. No active
task recreates it, so Foundation D adds no permanent cleanup framework for it.
That cleanup was separately authorized and executed only after the exact paths
and protected product state were read back. Profiles, identities, Wi-Fi
material, diagnostic records, qualification results, current firmware output,
and unknown `.deskkin` entries were not cleanup targets.

The warm local implementation run produced the host result in 15.7 seconds.
With `.deskkin/sdk`, `.deskkin/west`, `.deskkin/rustup`, and `.deskkin/venv`
temporarily absent, the same lane passed in 12.2 seconds and all four directories
were restored afterward. The independent clean CoreS3 conformance lane passed in
about 42 seconds. These observations replace one approximately 50-second serial
wait with an independently available host result; they are not performance
thresholds.

## Scope

Foundation D includes:

- objective-based host and CoreS3 verification tasks;
- one complete aggregate task retaining the existing `mise run test` name;
- two independent CI jobs with unchanged required coverage;
- a static assertion that each accepted verification step has exactly one lane
  owner and the aggregate includes both owners;
- README, implementation-plan, and agent workflow updates for the new entrypoints;
  and
- before/after timing evidence from the same warm local checkout, reported as
  observations rather than a hard performance guarantee; a clean CI comparison
  is recorded only after a separately requested push.

Foundation D does not include:

- product code, protocol, UI, connector, feature, profile, identity, device, or
  diagnostic-schema changes;
- new dependencies, action versions, cache services, prebuilt toolchain images,
  artifacts, releases, or remote destinations;
- path-based test skipping, change detection, flaky-test retries, reduced
  coverage, or a second definition of completion;
- parallel execution inside one checkout where Cargo, Python, Zephyr, or result
  paths may contend;
- automatic deletion, `cargo clean`, toolchain cleanup, state migration, or a
  general-purpose disk manager;
- flashing, provisioning, NVS mutation, power cycling, provider access, push,
  or publication.

## Verification lanes

### Host and portable feedback

The new entrypoint is:

```text
mise run test:host
```

It owns the current steps that do not require the repository-local CoreS3
toolchain:

1. `mise run check`;
2. locked workspace Clippy with warnings denied;
3. locked workspace tests and doc tests;
4. all deterministic simulator scenarios;
5. portable `thumbv7m-none-eabi` checks;
6. the complete Python unit suite, including the age round trip;
7. dependency-boundary inspection for every portable and host-composition
   crate currently checked by `mise run test`.

The lane may use normal Cargo incremental state. It must succeed from a clean
checkout after `mise install`, with `.deskkin/sdk`, `.deskkin/west`,
`.deskkin/rustup`, and `.deskkin/venv` absent. A test fixture may create only
its own temporary state; the lane must not bootstrap or inspect a physical
device.

### CoreS3 clean conformance

The new entrypoint is:

```text
mise run test:core-s3
```

It runs the existing CoreS3 input verification and then performs the product
and inert `--pristine` builds through `scripts/phase3_device.py build`. It owns
no host workspace test, simulator scenario, or live device operation.

The lane is offline once `mise run phase3:device:bootstrap` has prepared the
repository-local toolchain. A missing, changed, or incomplete toolchain fails
closed through the existing bootstrap verification. CI performs bootstrap in
the CoreS3 job before invoking this lane; the local lane does not silently
download dependencies.

### Complete acceptance

`mise run test` invokes `test:host` and then `test:core-s3` sequentially. It
remains the documented and agent-required complete reproducible verification
entrypoint. Calling either child lane alone is feedback, not completion.

Sequential local execution avoids shared-checkout contention and preserves
readable failure ordering. The aggregate returns the first nonzero child status
and never continues into CoreS3 work after a host-lane failure. It does not
duplicate any accepted check outside the child lanes.

## CI contract

The workflow has two jobs:

```text
host
core-s3
```

Both begin from the same commit in separate clean runners. `host` installs the
locked mise tools and runs `mise run test:host`; it does not reclaim disk or
bootstrap CoreS3. `core-s3` performs the existing runner disk reclamation,
installs the locked mise tools, bootstraps the pinned CoreS3 toolchain, and runs
`mise run test:core-s3`.

The jobs start independently and neither is conditional on changed paths. A
workflow is successful only when both jobs succeed. Existing concurrency
cancellation, read-only repository permission, pinned action revisions, and
push/pull-request triggers remain unchanged.

Foundation D intentionally adds no cache in this slice. The current local
toolchain and old build footprint show that caching the broad `.deskkin` tree
would mix protected runtime state, obsolete output, build products, and
rebuildable tools. A future cache proposal must first define a narrow immutable
input key, exact cached paths, maximum size, restore validation, eviction, and
the behavior of a corrupt or unavailable cache. Cache restore must never become
toolchain provenance.

## Task ownership and drift prevention

`mise.toml` remains the source of truth. The host and CoreS3 child tasks each
contain their owned commands once; the aggregate invokes only those two tasks.
CI invokes the same child tasks and contains no duplicate Cargo, Python, or
device-build command list.

A dependency-free repository test reads the resolved task definitions and
fails when:

- either child task is absent;
- `test` does anything except invoke both children in the accepted order;
- the host task references CoreS3 bootstrap/build paths;
- the CoreS3 task references host test/scenario commands; or
- an existing accepted verification command disappears from both owners.

The assertion checks stable command identities, not formatting or elapsed time.
Adding a future verification step requires assigning it to exactly one child
lane and keeping the aggregate unchanged.

## Observation contract

This checkpoint reorganizes development workflow rather than a product program.
It does not add a new runtime Diagnostic Run or recorder. Existing command exit
statuses, stage-specific stderr, Cargo/Python/Zephyr output, the CoreS3 atomic
build result, and GitHub job/step status already identify the owning failure
stage. Recording those results again below `.deskkin` would add disk usage and
another retention surface without improving failure discrimination.

The observable result surfaces are:

- child-command exit status for local feedback;
- the first failing owned command in local stdout/stderr;
- the existing CoreS3 build result JSON for firmware conformance; and
- separate `host` and `core-s3` GitHub job conclusions and logs.

No command prints environment dumps, identity/profile contents, Wi-Fi material,
diagnostic payloads, full filesystem inventories, or other secret or
privacy-sensitive state. CI uploads no artifact and sends no result to a new
remote service.

## Reproducible verification

Implementation must prove:

1. `test:host` passes with all CoreS3 state directories absent;
2. injecting a host compilation or unit-test failure prevents the aggregate
   from starting the CoreS3 lane;
3. `test:core-s3` rejects an absent or changed toolchain and passes after the
   existing bootstrap followed by clean product and inert builds;
4. lane-ownership tests reject missing, duplicated, or misplaced commands;
5. the workflow definition has independent unconditional jobs and a local
   workflow check rejects a missing lane or changed permissions/triggers;
6. the final local `mise run test` passes and demonstrates the exact union;
7. no physical-device path is opened and no ignored local state is deleted;
   and
8. dependency trees, protocol bytes, rendered frames, and CoreS3 firmware
   behavior are unchanged.

Before/after local elapsed times are retained in the implementation handoff.
After an explicitly requested push, the remote qualification also reads back
both job conclusions and elapsed times. Measurements detect a material
regression in the intended feedback path; they are not cross-machine pass
thresholds.

## Acceptance criteria

Foundation D is complete only when:

1. ordinary host and portable failures can be obtained without CoreS3 setup or
   build latency;
2. clean product and inert firmware conformance remains mandatory for complete
   local and CI success;
3. every pre-Foundation-D verification step has exactly one child-lane owner;
4. `mise run test` remains the only complete local acceptance claim;
5. CI always runs both lanes from the same commit without path-based skipping;
6. no new dependency, cache, product behavior, external authority, or remote
   state is introduced; and
7. implementation, tests, documentation, and fresh independent review pass.

## Approval boundary

Approval of this proposal authorized repository-only changes to `mise.toml`,
the CI workflow, dependency-free drift tests, README, implementation-plan, and
agent workflow documentation needed to implement the two lanes. It authorizes
local reproducible tests but not a push or remote CI run.

Approval did not authorize deletion of the observed legacy Gate build tree or
any other ignored state; the completed cleanup was a separate instruction after
exact readback of the target and protected siblings. It also did not authorize
dependencies, caches, product changes, physical-device operations, provider
access, push, release, or publication.
