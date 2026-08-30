# ADR-0008: Host capability and connector composition

Status: Accepted

Date: 2026-08-30

## Context

Foundation B established portable multi-feature composition, but the desktop
host still converted `ReadAvailability` directly into one configured result.
There was no owner for connector lifecycle, semantic capability routing,
connector identity, completion correlation, or provider-independent failure
classification.

Putting these responsibilities in the portable application would give devices
knowledge of host authority and provider execution. Putting them directly in
protocol handling would couple connectors to current wire messages and make
future connector changes protocol changes.

## Decision

Use `deskkin-host-capabilities` as the sole host capability and connector
composition root. It owns a closed compile-time registry, connector lifecycle,
semantic requests and results, fully namespaced effect identities, and exact
completion validation.

The crate is host-only but independent of protocol, provider, runtime,
filesystem, credential, and diagnostic implementations. The desktop host maps
between its semantic results and protocol major 1 at the outer adapter.

The current registry contains availability read and a deterministic
availability connector. Real provider connectors, dynamic loading, credential
persistence, and new protocol features require later accepted checkpoints.

## Consequences

- Portable application crates remain independent of connectors and provider
  authority.
- Connector failures remain distinct inside the host even when the current wire
  contract collapses them to `ReadFailed`.
- Stale or mismatched connector completions cannot update host capability state.
- Host diagnostic runs identify routing and connector execution separately.
- Adding a provider requires explicit semantic mapping and connector execution,
  not a change to the composition ownership boundary.
- Static registration must be edited and rebuilt to add a connector until a
  demonstrated need justifies dynamic loading.

