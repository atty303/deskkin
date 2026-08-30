# Deskkin

Deskkin is a modular platform for embodied desktop companions. It connects a
portable device application and Slint user interface to desktop-hosted
integrations through a capability-oriented protocol.

The first device target is StackChan on M5Stack CoreS3. Provider connectors are
deliberately deferred while the shared application, protocol, device, and
development foundations mature. Future candidates include Unraid, desktop
notifications, conversational AI, calendars, and home automation.

## Status

Deskkin has completed its foundation gates, provider-neutral availability
surface, authenticated host protocol, and the physically qualified CoreS3
paired-availability slice. The maintained product paths now include portable
`no_std` multi-feature application and protocol crates, a host capability and
connector composition crate, the Linux host and simulator, and the CoreS3
firmware. Completed feasibility harnesses are no
longer part of the active development surface; their contracts and evidence
remain in `docs/`.

Foundation D separates fast host feedback from clean CoreS3 conformance without
weakening the complete verification boundary. Its accepted contract and remote
qualification are in
[`docs/foundation-d-repeatable-development-loop-proposal.md`](docs/foundation-d-repeatable-development-loop-proposal.md)
and
[`docs/foundation-d-repeatable-development-loop-qualification.md`](docs/foundation-d-repeatable-development-loop-qualification.md).
Foundation E evaluated a standard CoreS3 SDK download cache without changing
the ordinary bootstrap or conformance boundary. Comparable remote miss and hit
runs showed no bootstrap speedup, so the cache was removed. Its accepted
contract and measured result are in
[`docs/foundation-e-core-s3-download-cache-proposal.md`](docs/foundation-e-core-s3-download-cache-proposal.md)
and
[`docs/foundation-e-core-s3-download-cache-qualification.md`](docs/foundation-e-core-s3-download-cache-qualification.md).

The agreed product boundary, architecture, decisions, evidence, and phased plan
remain in [`docs/implementation-plan.md`](docs/implementation-plan.md). Phase 2
is specified by
[`docs/phase-2-slice-proposal.md`](docs/phase-2-slice-proposal.md), and the
approved Phase 3 contract is in
[`docs/phase-3-slice-proposal.md`](docs/phase-3-slice-proposal.md) and
[`ADR-0004`](docs/decisions/0004-paired-host-protocol.md).

## Architecture

At a high level:

```text
external services
        |
        v
desktop host  <---- semantic protocol ---->  companion device
connectors                                  application + Slint UI
credentials                                Zephyr hardware platform
policy                                     Embassy async runtime
```

The portable application is split into a feature-neutral `application-core`,
an `application-features` crate containing availability and synthetic notice,
and the `deskkin-application` composition root. Zephyr owns hardware and system
services, Embassy provides optional async orchestration, and Slint owns the
declarative UI. Desktop and deterministic simulator adapters run the same
application composition and UI without emulating Zephyr.

The desktop host routes semantic requests through the closed
`deskkin-host-capabilities` registry before adapting results to protocol major
1. The current connector is deterministic availability only; it owns no
provider payload, credential, or external authority.

See [`docs/architecture.md`](docs/architecture.md) for component boundaries
and the accepted ADRs under [`docs/decisions/`](docs/decisions/).

## Setup

Install [mise](https://mise.jdx.dev/), then bootstrap the repository:

```sh
mise trust --yes
mise install --yes
```

If Codex was already running, restart it after the first install so the
project-local hk MCP server can start.

Prepare the pinned CoreS3 Zephyr SDK, Xtensa compiler, Rust toolchain, and west
workspace before the first firmware build:

```sh
mise run phase3:device:bootstrap
mise run phase3:device:build
```

## Development

```sh
mise run check
mise run fix
mise run test:host
mise run test:core-s3
mise run test
```

`mise run fix` applies available fixes without staging the changed files.

`mise run test:host` is the fast feedback lane for host, portable, protocol,
simulator, Python, and dependency-boundary checks. It does not require the
CoreS3 SDK or Zephyr workspace. After the explicit CoreS3 bootstrap,
`mise run test:core-s3` verifies the pinned toolchain and performs clean product
and inert firmware builds. These child tasks are feedback entrypoints;
`mise run test` remains the complete reproducible acceptance command and runs
both lanes in order.

Pass selected files after `--`, for example `mise run check -- README.md`.

Run the Linux simulator and deterministic scenarios with:

```sh
mise run simulator:desktop
mise run simulator:scenario -- periodic-success
mise run simulator:scenario -- periodic-read-failure
mise run simulator:scenario -- multi-feature-composition
```

The native fake repeats `Available`, `Unavailable`, and read failure after an
initial 250 ms read and five-second refresh timers. Headless results and bounded
default-on diagnostics are atomically written below `.deskkin/phase2/`.
Recording can be disabled by passing `--recording-off` to either binary.
Refresh runs publish a private in-progress marker and replace it on completion;
the next store access recovers a marker left by a crash as a partial run. The
multi-feature scenario also proves deterministic notice preemption, underlying
availability progress, session invalidation, and restoration without exposing
a production notice command.

Diagnostic administration accepts only an exact operation and run ID:

```sh
mise run simulator:diagnostics -- list
mise run simulator:diagnostics -- retain RUN-ID
mise run simulator:diagnostics -- unretain RUN-ID
mise run simulator:diagnostics -- delete RUN-ID
```

Phase 3 uses disposable identity roots by default. Initialize both identities,
start the long-running host and simulator in separate local terminals, then run
the owner-routed pairing commands in two more terminals. Confirm the matching
authentication string at both ends; the running simulator reconnects with the
pinned identity after the pairing operation completes:

```sh
mise run phase3:host -- identity-init
mise run phase3:simulator -- identity-init
mise run phase3:host -- run 127.0.0.1:39032 available
mise run phase3:simulator -- run 127.0.0.1:39032
mise run phase3:host -- pairing-window-open
mise run phase3:simulator -- pair-start 127.0.0.1:39032
```

Pairing and runtime commands reject non-loopback network scope. The fake host
also accepts `unavailable` and `read_failed`. Identity mutation is serialized
through the live process owner; exact unpair requires the peer public-key ID
reported by `identity-list`. Role-local diagnostics remain private and bounded:

```sh
mise run phase3:diagnostics -- --root .deskkin/phase3/host list
mise run phase3:diagnostics -- --root .deskkin/phase3/device-simulator list
```

For repeatable physical-host operation, store only the secret-free launch
selection in an ignored named profile. The role root references an existing
identity; profile creation never initializes, copies, or changes it:

```sh
mise run deskkin:profile -- set core-s3 --role-root phase3/physical-demo/host --bind-mode private_lan --address 192.168.1.10:39042 --availability available --recording on
mise run deskkin:profile -- show core-s3
mise run deskkin:profile -- list
```

Launch remains foreground-only. Status compares the live owner's exact launch
metadata, and stop is accepted only for the matching owner generation:

```sh
mise run deskkin:host -- --profile core-s3
mise run deskkin:status -- --profile core-s3
mise run deskkin:stop -- --profile core-s3
```

Use the machine's exact assigned RFC1918 address; wildcard, public, link-local,
unassigned, IPv6 private-LAN, and non-39042 physical binds are rejected. Never
place Wi-Fi, Noise, pairing, or provider credentials in a host profile. Profile
creation and a private-LAN launch for retained state are operational changes and
must follow their explicit live checkpoint.

CoreS3 development uses explicit, separate bootstrap, build, device-mutation,
and read-only status tasks. Mutation tasks do not run as part of tests:

```sh
mise run phase3:device:bootstrap
mise run phase3:device:build
mise run phase3:device:status
```

See [`docs/phase-3p-physical-slice-proposal.md`](docs/phase-3p-physical-slice-proposal.md)
for the separately authorized profile, flash, identity, provisioning, run, and
recovery operations and their retained-state constraints.

## License

Deskkin source is MIT. Slint-bearing simulator binaries and CoreS3 firmware use
Slint under GPL-3.0-only and are GPLv3 as a whole. The accepted phases authorize
that distribution model but add no release or publication; actual distribution
requires a separately approved corresponding-source bundle and delivery
method.
See [`docs/licensing.md`](docs/licensing.md) for the exact source and combined
binary boundary.
