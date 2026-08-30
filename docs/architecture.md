# Deskkin architecture

## Purpose

Deskkin is a modular platform for embodied desktop companions. A companion
device presents character, status, notifications, conversation, and user
actions. A desktop host integrates external services and grants each paired
device bounded capabilities.

StackChan on M5Stack CoreS3 is the first device target. It is a proving ground,
not an architectural boundary. Unraid is the first planned connector, not a
platform dependency.

## System context

```text
                         external services
                  Unraid / AI / notifications / future
                                  |
                                  v
+----------------------+   +-----------------------------+
| desktop applications |-->| Deskkin desktop host        |
+----------------------+   | connectors / policy / state |
                           +--------------+--------------+
                                          |
                                  Deskkin protocol
                                          |
                           +--------------v--------------+
                           | companion device             |
                           | application / Slint / input  |
                           +--------------+--------------+
                                          |
                           +--------------v--------------+
                           | Zephyr / drivers / hardware  |
                           +-----------------------------+
```

The desktop host is the external-authority boundary. Provider credentials,
desktop sessions, durable connector state, authorization policy, and broad
network access remain there. A companion device holds only its paired identity,
the minimum transport configuration needed to reach its host, and the
capabilities needed for its current session. Provider credentials remain on
the host; the transport configuration may include a Wi-Fi credential.

## Dependency direction

```text
device platform adapters ---+
desktop platform adapters ---+--> application runtime --> application core
simulator adapters ----------+

Slint presenter -----------------------------------------> application core
connectors --> desktop host --> protocol ----------------> application core
```

The portable core must not depend on a platform adapter, runtime, UI toolkit,
transport, connector, or board.

## Logical components

### Application core

The application core is pure `no_std` Rust. It owns domain values, state
transitions, commands, events, effect requests, and feature-independent policy.
It does not perform I/O or read ambient time, randomness, environment, or
global runtime state.

A useful target shape is an event-driven state machine:

```text
current state + input event --> new state + requested effects
```

This shape supports synchronous unit tests, virtual time, record and replay,
and alternate device and desktop runtimes without emulating Zephyr.

### Application runtime

The application runtime executes requested effects, schedules work, delivers
completion events, and owns concurrency. On a device, async orchestration uses
Embassy in one or more Zephyr threads. Synchronous paths do not need to become
Embassy tasks. On desktop, the runtime may use a hosted executor or a
deterministic scenario driver.

Embassy is not part of the portable core contract. In particular, direct calls
to Embassy timers, task macros, executor state, and platform drivers remain in
the runtime adapter.

### Device platform

Zephyr owns hardware topology, device discovery, drivers, system services,
thread scheduling, networking, storage, power management, logging, and
firmware-management facilities selected for the product.

Rust adapters expose the narrow application semantics needed above Zephyr.
Unsafe Rust, C FFI, and raw Zephyr types end at this boundary. Board-specific
initialization belongs in devicetree, board support, or drivers rather than in
application conditionals.

### User interface

Slint is the shared declarative UI. The Slint presenter maps application view
models to Slint properties and models, and maps Slint callbacks to typed
application commands.

One owner controls a Slint instance. Runtime tasks, drivers, and callbacks do
not mutate it directly. They send messages to the owner. The same UI and
presenter run with a Zephyr software-renderer adapter and a native desktop
backend.

The device adapter must preserve Slint dirty rendering through the display
boundary. Rendering less while transferring a full frame does not satisfy the
performance contract. Future work must measure dirty pixels, render duration,
transfer duration, and missed frame deadlines.

### Desktop host

The desktop host owns paired device sessions, connector lifecycle, provider
credentials, persistent integration state, authorization, confirmation policy,
and translation from provider-specific data into Deskkin semantics.

The host is Linux-only and accepts one authenticated device session. Its
default command binds an explicitly selected IPv4 or IPv6 loopback address. A
separate physical-slice command binds one exact assigned RFC1918 IPv4 address
on fixed port `39042`; it refuses wildcard, public, link-local, IPv6, and
unassigned addresses and never changes firewall or interface configuration. A
single session writer owns encrypted framing and TCP writes; bounded
application and reserved control queues make overload and shutdown explicit.
Identity filesystem work is isolated in a blocking store actor. Mutations while
the runtime is alive pass through a private, generation-bound Unix owner
control socket rather than racing a second standalone store writer.

Connectors do not send provider payloads to a device. For example, an Unraid
connector converts an Unraid response into infrastructure status and declared
actions. A future conversational connector converts provider streaming output
into conversation events.

Host semantic requests pass through the compile-time capability and connector
composition defined by
[`ADR-0008`](decisions/0008-host-capability-connector-composition.md). The
composition root owns lifecycle, routing, connector identity, and completion
correlation without depending on protocol or provider types. Protocol mapping
remains at the desktop-host adapter, and provider execution remains behind the
selected connector.

### Simulator

The simulator runs the application core, presenter, and Slint UI with fake
ports and virtual time. It must support deterministic scenarios, injected
failures, and recorded semantic inputs. A desktop device simulator may also
connect to a real desktop host for end-to-end integration development.

## Feature model

Features own their state and exchange typed commands, events, view models, and
effect requests. They do not call connectors, Zephyr APIs, or Slint objects.

The device initially uses a compile-time feature registry. Modularity does not
require a stable dynamic Rust ABI. Dynamic desktop connector loading remains a
separate future decision.

Shared UI surfaces should cover common cases such as notifications, status,
progress, actions, confirmation, and conversation. A feature-specific Slint
component is allowed when a common surface would erase important semantics,
but navigation and global shell policy remain outside the feature.

## Protocol boundary

The Deskkin protocol carries semantic messages rather than implementation
details. Its conceptual families are:

- session establishment and peer identity;
- protocol and feature capability negotiation;
- application commands and completion events;
- notifications, conversation updates, and status views;
- proposed actions and explicit confirmations;
- device health and bounded diagnostic summaries.

Protocol major 1 uses a fixed prelude, Noise XX with
X25519, ChaChaPoly, and BLAKE2s, and encrypted bootstrap schema 1. A portable
`no_std` codec owns canonical closed messages and two-byte big-endian bounded
framing without owning sockets, authentication state, persistence, runtime, or
application types. The exact protocol-major-1 wire contract is fixed by
[ADR-0004](decisions/0004-paired-host-protocol.md).
The original loopback-only transport consequence is superseded only for the
exact RFC1918 physical mode by
[ADR-0005](decisions/0005-core-s3-paired-availability.md).

Bootstrap negotiates supported protocol majors, required and optional feature
bits, requested peer permission bits, selected features, and granted
permissions independently. The first feature is `availability.read.v1`; its
permission is `availability.read`. Session and operation context identities
correlate requests and diagnostics without making a closed session's late
result valid.

Hardware capability, application capability, protocol capability, and granted
permission are distinct:

```text
hardware capability     what the board physically provides
application capability  what this firmware can do
protocol capability     what this peer can exchange
permission              what this paired device may request
```

## Security boundary

External service credentials must not be provisioned to a companion device.
Read capabilities and mutation capabilities are separate. A mutation request
must carry an action identity and the policy-required confirmation result; the
desktop host remains responsible for final authorization immediately before
calling a connector.

Each peer has an explicitly initialized X25519 static identity. Noise XX
pairing derives the same six-digit local authentication string on both peers;
the string is never accepted from the remote peer or persisted. Only the
durable `paired` state permits an application session. Pairing publication and
exact unpair use generation-bound, crash-recoverable state machines and
fail-closed validation. Hosted peers use private filesystem modes and atomic
replacement. The CoreS3 uses separate two-slot NVS records with publication
sequence, CRC, write/readback verification, and conflict rejection.

The CoreS3 physical slice stores its pairing identity and one Wi-Fi profile in
separate two-slot Zephyr NVS namespaces. This state is intentionally plaintext:
the slice does not enable flash encryption, secure boot, or eFuse mutation.
Provider credentials and provider authority still never enter the device.

Disconnect invalidates current availability to `Unknown`; it never fabricates
`Unavailable`. Terminal version, required-feature, and authorization failures
require an explicit new connect, while transient busy or transport failures
use the bounded reconnect policy. Provider mutations and replay protection for
them remain future decisions.

## Observation boundary

Hosted external and asynchronous paths expose separate result, owner-control,
and local diagnostic surfaces. The reusable recorder publishes bounded atomic
runs and never changes semantic results when disabled, full, or unhealthy.
Phase 3 correlates pairing, session, availability read, and identity control by
opaque transaction, session, and operation identities.

Diagnostics contain only the accepted closed operation, outcome, error,
duration, protocol/feature/permission, queue, completeness, and health fields.
They exclude authentication strings, raw keys, addresses, wire data, payloads,
paths, machine/user/process identity, environment, and provider data. Host and
simulator stores are separately locked and capped at 16 MiB each, with no
remote exporter.

For the CoreS3 slice, a USB-connected host runner consumes only allowlisted
device records and uses the same opaque correlations. Device and host roots are
independently capped at 16 MiB. Wi-Fi credentials, pairing authentication
strings, keys, LAN addresses, serial payloads, and NVS contents are never
diagnostic fields; absence or failure of the runner cannot alter device
behavior.

## Expected workspace shape

This is a boundary map, not authorization to create the crates now:

```text
crates/
  application-core/
  application-embassy/
  feature-api/
  protocol/
  slint-presenter/
  platform-zephyr/
  platform-desktop/
  desktop-core/
  simulator/
features/
  character/
  notifications/
  conversation/
  infrastructure/
connectors/
  unraid/
  notifications/
  conversational-ai/
apps/
  device/
  desktop-host/
  desktop-device-simulator/
  scenario-runner/
boards/
  m5stack_cores3/
```

Crates and directories should be added only when their first accepted vertical
slice needs them. The final source layout may become smaller than this map.

## Accepted decisions

- [ADR-0001](decisions/0001-platform-roles.md): Platform roles
- [ADR-0002](decisions/0002-host-device-boundary.md): Desktop host and device
  boundary
- [ADR-0003](decisions/0003-portable-application.md): Portable application and
  feature model
- [ADR-0004](decisions/0004-paired-host-protocol.md): Paired loopback host
  protocol
- [ADR-0005](decisions/0005-core-s3-paired-availability.md): Exact private-LAN
  CoreS3 transport, runtime, and persistent state

The current implementation checkpoint and unresolved questions are maintained
in [`implementation-plan.md`](implementation-plan.md).
