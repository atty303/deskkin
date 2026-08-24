# ADR-0004: Establish the paired loopback host protocol

- Status: Accepted
- Date: 2026-08-24

## Context

Phase 2 proves provider-neutral availability behavior with deterministic local
effects, but it does not establish the trust, compatibility, persistence, or
backpressure boundary between a companion and the desktop host. Phase 3 needs
one durable vertical slice without prematurely exposing a LAN service, adding
a provider connector, or coupling the portable application core to transport.

The complete accepted field order, message tags, sizes, timeouts, persistence
transitions, diagnostic vocabulary, dependency features, and acceptance tests
are recorded in the approved
[`phase-3-slice-proposal.md`](../phase-3-slice-proposal.md). Those definitions
are normative for protocol major 1 and identity schema major 1.

## Decision

Use Linux loopback-only TCP for the first host transport. The initiator sends
the exact six-byte prelude `44 53 4b 4e 00 01`; the responder validates it and
both sides bind it as the prologue for
`Noise_XX_25519_ChaChaPoly_BLAKE2s`. Encrypted bootstrap schema 1 negotiates
protocol major, required and optional feature support, requested peer
permissions, and granted permissions as separate fields.

Add a dependency-free `no_std` protocol crate with a bounded caller-provided
slice codec, two-byte big-endian frame lengths, canonical one-byte message
tags, exact decode, and closed messages. The first feature is
`availability.read.v1`; the separately granted permission is
`availability.read`. Request, transaction, session-context, and
operation-context identities follow the exact widths and field order in the
approved proposal.

Each peer owns an explicitly initialized X25519 static identity. Pairing uses
Noise XX, a locally derived six-digit authentication string, explicit local
confirmation on both peers, and the role-fixed durable
`pending -> committing -> paired` publication sequence. Only `paired` permits
an application session. Exact unpair first durably publishes `revoking`,
invalidates the old generation and its sessions, then publishes `unpaired`.
Private identity stores fail closed on symlinks, unknown entries, invalid
modes, corrupt state, and ambiguous recovery.

The desktop host and hosted simulator use bounded single-owner session writers,
queues, timeouts, and the reconnect state machine defined in the proposal.
No automatic retry follows terminal incompatibility or authorization denial.
Session loss invalidates availability immediately; it never maps to
`Unavailable`. Feature support and peer permission remain independent.

Live identity mutation is serialized through each process's private Unix owner
control socket. Commands are generation-bound and idempotently queryable;
uncertain owner loss is reported rather than implicitly replayed.

Extract the hosted Phase 2 recorder without changing its public behavior.
Phase 3 records bounded `protocol.pairing`, `protocol.session`,
`availability.read`, and `identity.control` runs using opaque correlation
identities. Authentication strings, keys, addresses, protocol bytes, payloads,
paths, host/user/process identity, and provider data are forbidden diagnostic
fields. Recording remains default-on, local-only, bounded, and non-interfering.

Adopt only the six exact direct dependency contracts approved in the proposal:
`serde 1.0.229`, `serde_json 1.0.151`, `postcard 1.1.3`, `tokio 1.53.1`,
`snow 0.10.0`, and `zeroize 1.9.0`, with their stated default-off feature
boundaries. Deskkin source remains MIT. A simulator binary containing Slint
remains GPLv3; this checkpoint does not distribute binaries.

## Consequences

- Protocol major 1 and identity schema major 1 are compatibility contracts;
  incompatible changes require a superseding ADR.
- The application core stays unaware of transport, identity, Noise, sockets,
  persistence, and hosted runtimes.
- The protocol crate stays allocation-free and independent of application,
  runtime, filesystem, socket, and UI crates.
- Loopback E2E can prove the authenticated boundary without claiming LAN or
  physical-device readiness.
- Pairing and unpairing require more durable state transitions and fault tests
  than a replace-only identity file.
- A later physical-device slice must prove a compatible constrained Noise
  implementation or supersede this transport decision.

## Alternatives

### Unauthenticated loopback plaintext

This is smaller but cannot prove pinned identity, authenticated negotiation,
or reconnection semantics.

### TLS with certificates

TLS is mature, but certificate issuance, trust-store policy, and constrained
device compatibility add decisions not needed for the first slice.

### Custom key exchange or authentication

This reduces dependencies by making Deskkin own cryptographic protocol design,
which is not justified for this slice.

### LAN exposure in Phase 3

LAN exposure would add discovery, interface policy, firewall, and remote threat
boundaries before the local protocol and recovery behavior are proven.
