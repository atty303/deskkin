# Deskkin

Deskkin is a modular platform for embodied desktop companions. It connects a
portable device application and Slint user interface to desktop-hosted
integrations through a capability-oriented protocol.

The first device target is StackChan on M5Stack CoreS3. Unraid control is the
first planned integration, not a special part of the platform. Future
integrations may include desktop notifications, conversational AI, calendars,
and home automation.

## Status

Deskkin has completed Phase 0 and all Phase 1 foundation gates. The bounded
evidence covers Rust and Slint on supported Zephyr QEMU targets, the
ESP32-S3/Xtensa Rust toolchain, upstream CoreS3 board and driver behavior, and
the combined CoreS3 Slint touch-to-dirty-display path. The repository still
contains feasibility applications and tooling rather than the portable product
application.

The approved next checkpoint is the Phase 2 provider-neutral periodic
availability status: a pure portable core and a deterministic Linux simulator
described in [`docs/phase-2-slice-proposal.md`](docs/phase-2-slice-proposal.md).
The agreed product boundary, architecture, decisions, evidence, and phased plan remain in
[`docs/implementation-plan.md`](docs/implementation-plan.md), with the Phase 1
foundation baseline in
[`docs/phase-0-feasibility-proposal.md`](docs/phase-0-feasibility-proposal.md).

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

## License

Deskkin source is MIT. Slint-bearing Gate 1B and Phase 2 combined binaries use
Slint under GPL-3.0-only and are GPLv3 as a whole. Phase 2 authorizes that
distribution model but adds no release or publication; actual distribution
requires a separately approved corresponding-source bundle and delivery method.
