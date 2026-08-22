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
network access remain there. A companion device holds only its paired identity
and the capabilities needed for its current session.

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

Connectors do not send provider payloads to a device. For example, an Unraid
connector converts an Unraid response into infrastructure status and declared
actions. A future conversational connector converts provider streaming output
into conversation events.

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

Transport, serialization, framing, authentication, schema evolution, and
reconnection semantics are intentionally undecided. They require a dedicated
protocol design and dependency approval before implementation.

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

Transport security and pairing are not selected yet. Their later design must
bind messages to a peer and session, prevent replay of mutations, fail closed
on ambiguous authorization, and expose enough state to diagnose rejection
without disclosing credentials.

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

The current implementation checkpoint and unresolved questions are maintained
in [`implementation-plan.md`](implementation-plan.md).
