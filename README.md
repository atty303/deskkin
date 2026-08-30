# Deskkin

Deskkin is a platform for embodied desktop companions. A portable Rust
application and shared Slint UI run on a Linux simulator and on StackChan with
an M5Stack CoreS3. A desktop host owns integrations, credentials, policy, and
the authenticated semantic protocol exposed to companion devices.

The repository currently provides:

- allocation-free `no_std` application, feature-composition, protocol, and
  CoreS3 support crates;
- a deterministic Linux simulator with virtual time, scenario replay, and
  bounded local diagnostics;
- an authenticated Linux host and hosted device simulator with pairing,
  identity lifecycle, reconnect, and availability reads;
- repeatable, secret-free physical-host profiles; and
- reproducible CoreS3 product and inert-recovery firmware builds.

The only application capability is provider-neutral availability. The current
host connector is deterministic and accesses no external provider. Packaging,
background services, OTA, and a general compatibility or migration policy are
not implemented.

See [the architecture](docs/architecture.md) for component and security
boundaries and [CoreS3 operation](docs/core-s3.md) for firmware tooling,
persistent state, and live-device constraints.

## Setup

Install [mise](https://mise.jdx.dev/), then prepare the locked development
tools:

```sh
mise trust --yes
mise install --yes
```

If Codex was already running, restart it after the first install so the
project-local hk MCP server can start.

Prepare the pinned CoreS3 Zephyr SDK, Xtensa compiler, Rust toolchain, and west
workspace before the first firmware build:

```sh
mise run core-s3:bootstrap
mise run core-s3:build
```

## Development

```sh
mise run check
mise run fix
mise run test:host
mise run test:core-s3
mise run test
```

`mise run fix` applies available formatter and linter fixes without staging
files. Pass selected files after `--`, for example
`mise run check -- README.md`.

`mise run test:host` covers host, portable, protocol, simulator, Python, and
dependency-boundary checks without requiring a CoreS3 workspace.
`mise run test:core-s3` verifies the prepared toolchain and performs clean
product and inert firmware builds. Both are feedback lanes; `mise run test`
is the complete reproducible acceptance command.

No reproducible test flashes, provisions, erases, power-cycles, or otherwise
mutates a physical device.

## Simulator

Run the native UI or deterministic headless scenarios with:

```sh
mise run simulator:desktop
mise run simulator:scenario -- periodic-success
mise run simulator:scenario -- periodic-read-failure
mise run simulator:scenario -- protocol-disconnect-recovery
mise run simulator:scenario -- multi-feature-composition
```

Scenario results and bounded default-on diagnostics are atomically written
below `.deskkin/phase2/`. Pass `--recording-off` to disable recording without
changing scenario semantics or frames.

Diagnostic administration accepts only an exact operation and run ID:

```sh
mise run simulator:diagnostics -- list
mise run simulator:diagnostics -- retain RUN-ID
mise run simulator:diagnostics -- unretain RUN-ID
mise run simulator:diagnostics -- delete RUN-ID
```

## Hosted protocol loopback

The hosted host and device simulator use repository-local, ignored identity
roots by default. The roots persist across invocations; they are not reset or
removed automatically. Initialize both identities, start the long-running host
and simulator in separate terminals, and then run the pairing commands in two
more terminals:

```sh
mise run protocol:host -- identity-init
mise run protocol:simulator -- identity-init
mise run protocol:host -- run 127.0.0.1:39032 available
mise run protocol:simulator -- run 127.0.0.1:39032
mise run protocol:host -- pairing-window-open
mise run protocol:simulator -- pair-start 127.0.0.1:39032
```

Confirm the matching locally derived authentication string at both ends.
Pairing and runtime commands reject non-loopback scope. Identity mutation is
serialized through the live owner; exact unpair requires the peer public-key
ID reported by `identity-list`. For a genuinely fresh pairing trial, use new
paths with the CLI's distinct contracts: host identity commands take the
`identity` directory, host `run` takes its parent role directory, and simulator
commands take the identity directory throughout. For example:

```sh
mise run protocol:host -- identity-init .deskkin/trials/fresh-host/identity
mise run protocol:simulator -- identity-init .deskkin/trials/fresh-device/identity
mise run protocol:host -- run 127.0.0.1:39032 available .deskkin/trials/fresh-host
mise run protocol:simulator -- run 127.0.0.1:39032 .deskkin/trials/fresh-device/identity
mise run protocol:host -- pairing-window-open .deskkin/trials/fresh-host/identity
mise run protocol:simulator -- pair-start 127.0.0.1:39032 .deskkin/trials/fresh-device/identity
```

Otherwise inspect the retained default roots and obtain approval before
unpairing or removing them.

Role-local diagnostics remain private and bounded:

```sh
mise run protocol:diagnostics -- --root .deskkin/phase3/host list
mise run protocol:diagnostics -- --root .deskkin/phase3/device-simulator list
```

## Physical host profile

A named ignored profile stores only a secret-free launch selection. Its role
root references an existing identity; profile creation never creates, copies,
or changes identity state.

```sh
mise run deskkin:profile -- set core-s3 --role-root phase3/physical-demo/host --bind-mode private_lan --address 192.168.1.10:39042 --availability available --recording on
mise run deskkin:profile -- show core-s3
mise run deskkin:profile -- list
```

Use the machine's exact assigned RFC1918 IPv4 address. Wildcard, public,
link-local, unassigned, IPv6, and non-39042 private-LAN binds are rejected.
Never put Wi-Fi material, Noise keys, pairing state, or provider credentials in
a host profile.

The host remains foreground-only. Status matches the exact launch metadata and
stop is accepted only for the observed owner generation:

```sh
mise run deskkin:host -- --profile core-s3
mise run deskkin:status -- --profile core-s3
mise run deskkin:stop -- --profile core-s3
```

Creating a retained profile or launching against retained identity state is an
operational state change. Inspect the target first and obtain explicit live
approval before performing it.

## CoreS3

Build and read-only status are separate from all device mutation:

```sh
mise run core-s3:bootstrap
mise run core-s3:build
mise run core-s3:status
```

Flashing, identity mutation, Wi-Fi provisioning, application runs, and recovery
use separate tasks and require explicit live approval. See
[CoreS3 operation](docs/core-s3.md).

## License

Deskkin-authored source and documentation are MIT. Slint-bearing simulator
binaries and CoreS3 product firmware are GPL-3.0-only combined works; the inert
recovery firmware does not link Slint and is outside that statement.
Distribution requires the corresponding-source and delivery obligations
described in [the licensing reference](docs/licensing.md).
