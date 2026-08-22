# Deskkin

Deskkin is a modular platform for embodied desktop companions. It connects a
portable device application and Slint user interface to desktop-hosted
integrations through a capability-oriented protocol.

The first device target is StackChan on M5Stack CoreS3. Unraid control is the
first planned integration, not a special part of the platform. Future
integrations may include desktop notifications, conversational AI, calendars,
and home automation.

## Status

Deskkin is at a documentation-only checkpoint. The repository contains no
application code or application dependencies. The agreed product boundary,
architecture, decisions, risks, and phased implementation plan are recorded in
[`docs/implementation-plan.md`](docs/implementation-plan.md).

Do not begin implementation by selecting crates or adding a Rust workspace.
The next checkpoint is a bounded feasibility and dependency proposal described
in the implementation plan.

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
