# Deskkin

Deskkin is a modular platform for embodied desktop companions. It connects a
portable device application and Slint user interface to desktop-hosted
integrations through a capability-oriented protocol.

The first device target is StackChan on M5Stack CoreS3. Unraid control is the
first planned integration, not a special part of the platform. Future
integrations may include desktop notifications, conversational AI, calendars,
and home automation.

## Status

Deskkin has completed Phase 0, all Phase 1 foundation gates, and the Phase 2
provider-neutral periodic availability surface. The Phase 3 implementation now
adds an authenticated Linux loopback vertical slice: explicit host and
simulator identities, Noise XX pairing with a shared six-digit authentication
string, pinned reconnects, a bounded `no_std` wire codec, exact unpair, and a
fake-host availability read. The implementation passed the complete local
verification suite and a fresh independent review with no remaining P0/P1
finding; live pairing demonstration remains a separate launch checkpoint.

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

The application core is portable `no_std` Rust. Zephyr owns hardware and
system services, Embassy provides optional async orchestration, and Slint owns
the declarative UI. Desktop and deterministic simulator adapters run the same
application and UI without emulating Zephyr.

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
mise run test
```

`mise run fix` applies available fixes without staging the changed files.

Pass selected files after `--`, for example `mise run check -- README.md`.

Prepare and run the completed Gate 1A and Gate 1B evidence paths with:

```sh
mise run gate:1a:bootstrap
mise run gate:1a
mise run gate:1b
mise run diagnostics:list
```

Gate state, SDKs, fetched sources, builds, results, and bounded local
diagnostics remain under the ignored `.deskkin/` directory.

Run the Phase 2 Linux simulator and deterministic scenarios with:

```sh
mise run phase2:desktop
mise run phase2:scenario -- periodic-success
mise run phase2:scenario -- periodic-read-failure
```

The native fake repeats `Available`, `Unavailable`, and read failure after an
initial 250 ms read and five-second refresh timers. Headless results and bounded
default-on diagnostics are atomically written below `.deskkin/phase2/`.
Recording can be disabled by passing `--recording-off` to either binary.
Refresh runs publish a private in-progress marker and replace it on completion;
the next store access recovers a marker left by a crash as a partial run.

Diagnostic administration accepts only an exact operation and run ID:

```sh
mise run phase2:diagnostics -- list
mise run phase2:diagnostics -- retain RUN-ID
mise run phase2:diagnostics -- unretain RUN-ID
mise run phase2:diagnostics -- delete RUN-ID
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

## License

Deskkin source is MIT. Slint-bearing Gate 1B, Phase 2, and Phase 3 simulator
combined binaries use Slint under GPL-3.0-only and are GPLv3 as a whole. The
accepted phases authorize that distribution model but add no release or
publication; actual distribution requires a separately approved
corresponding-source bundle and delivery method.
See [`docs/licensing.md`](docs/licensing.md) for the exact source and combined
binary boundary.
