# Deskkin

Deskkin is a modular platform for embodied desktop companions. It connects a
portable device application and Slint user interface to desktop-hosted
integrations through a capability-oriented protocol.

The first device target is StackChan on M5Stack CoreS3. Unraid control is the
first planned integration, not a special part of the platform. Future
integrations may include desktop notifications, conversational AI, calendars,
and home automation.

## Status

Deskkin has completed its Phase 0 approval checkpoint. The ordered Phase 1
foundation gates are authorized, and Gate 1A on supported Zephyr targets is the
next implementation slice. The repository currently contains no application
code or application dependencies. The agreed product boundary, architecture,
decisions, risks, and phased implementation plan are recorded in
[`docs/implementation-plan.md`](docs/implementation-plan.md), and the bounded
foundation proposal is in
[`docs/phase-0-feasibility-proposal.md`](docs/phase-0-feasibility-proposal.md).

Implement the approved gates in order and stop at each gate's pass/fail
boundary. Do not start a later gate until its prerequisites pass.

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

## License

MIT
