# ADR-0002: Keep external integrations in a desktop host

- Status: Accepted
- Date: 2026-08-22

## Context

Deskkin will begin with Unraid status and control, then may add desktop
notifications, conversational AI, calendars, home automation, and other
integrations. Embedding each provider API and credential in companion firmware
would couple releases, expand device authority, and make every device reproduce
desktop integration work.

## Decision

Split Deskkin into a desktop host and one or more companion devices.

The desktop host owns connectors, provider credentials, persistent integration
state, authorization and confirmation policy, and device sessions. A device
owns local interaction, character presentation, UI, input, bounded offline
behavior, and device health.

Peers exchange versioned semantic messages through a capability-oriented
Deskkin protocol. Provider payloads, UI properties, Zephyr events, and direct
hardware commands do not cross this boundary.

Unraid is the first connector and infrastructure status is the corresponding
application feature. Conversational AI providers are connectors behind a
conversation feature. Desktop notification APIs are connectors behind a
notification feature.

## Consequences

- Provider changes usually do not require device firmware changes.
- External credentials and broad authority remain off removable devices.
- The protocol must define identity, capability negotiation, authorization,
  confirmation, request correlation, failure, reconnection, and evolution.
- Useful local behavior must be declared explicitly for host disconnection.
- A desktop simulator can exercise the same protocol as a physical device.

## Alternatives

### Direct provider clients on every device

This reduces the first host dependency but duplicates credentials, provider
SDKs, persistence, and policy on constrained devices. It does not support the
intended multi-device platform.

### One desktop process per integration

This isolates connectors but leaves session, authorization, protocol, and
state coordination fragmented. Connector process isolation may be added later
behind one logical host boundary if evidence justifies it.
