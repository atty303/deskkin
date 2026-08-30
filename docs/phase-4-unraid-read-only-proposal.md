# Phase 4 proposal: Unraid read-only infrastructure status

Status: Deferred; not the current approval boundary

Date: 2026-08-29

This historical proposal records one possible first provider-backed feature.
It is not the current approval boundary and records no accepted decision or
authority for implementation, dependency, credential creation, provider
access, or device mutation. It must be revised against the then-current
foundations before any future provider approval.

## Goal and observable result

Add one provider-neutral, read-only infrastructure-status feature. A Linux
desktop host reads the state of one Unraid array, normalizes it to a closed
Deskkin view, and sends only that semantic result to an authenticated simulator
or CoreS3. A source or session failure immediately displays `Unknown`; a later
successful read recovers without retaining or fabricating provider state.

The first acceptance boundary is an isolated fake GraphQL service. Live Unraid
access, credential provisioning against a real server, and physical-device
qualification are later, separately authorized checkpoints. Array mutation,
metrics, notifications, parity control, disk control, and provider payloads on
the device are out of scope.

Unraid documents a built-in GraphQL API beginning with Unraid 7.2 and states
that its local API does not require an Unraid Connect login. Programmatic access
uses API keys with roles and permissions, supplied in the `x-api-key` header.
The published schema exposes `UnraidArray.state` and separates data disks from
parity disks. Their status fields are the only provider health fields proposed
for this slice.

References:

- <https://docs.unraid.net/API/>
- <https://docs.unraid.net/API/how-to-use-the-api/>
- <https://github.com/unraid/api/blob/97f639fc1dc7484d9f7c4ed0dd2f9e0cd345d38f/api/generated-schema.graphql>

## Provider-neutral application contract

Add a dependency-free `#![no_std]` feature crate rather than placing Unraid
types in `application-core`. Its public view is closed:

- `Unknown`: no current trustworthy read exists;
- `Operational`: the array is started and every returned data and parity disk
  is present and healthy;
- `Degraded`: the array reports a non-started/non-stopped transitional or fault
  state, or any returned disk is missing, invalid, wrong, disabled, or new;
- `Stopped`: the array explicitly reports `STOPPED`.

The provider adapter maps only an exact successful response. `STARTED` with all
data and parity disk statuses `DISK_OK` maps to `Operational`; `STOPPED` maps
to `Stopped`; every other recognized array or disk state maps to `Degraded`.
Network, TLS, HTTP, authorization, rate-limit, GraphQL, schema, timeout,
oversize, cancellation, and connector-availability failures map to `Unknown`,
never `Stopped` or `Degraded`.

The feature follows the existing typed periodic-effect model. It starts with a
read, arms a five-second refresh only after completion, retries a failed read on
the next period, invalidates immediately when its authenticated session or
connector becomes unavailable, and rejects stale or mismatched completions.
Timer-arm failure stops the loop at `Unknown`. Provider data and connector
errors do not enter the public view model.

The shared Slint status surface gains text and color for these four states. The
CoreS3 shell, pairing UI, connection state, and touch behavior do not change.
The previous availability feature remains a separately negotiated Phase 3/3P
capability and is not silently reinterpreted as infrastructure health.

## Protocol and compatibility

Extend protocol major 1 with an optional
`infrastructure.status.read.v1` feature and independent
`infrastructure.status.read` permission. Add closed request/result messages and
canonical unused tags only after the wire vectors and maximum sizes are
accepted in the ADR. Existing Phase 3 availability bytes, negotiation,
pairing, identity, reconnect, framing, and Noise behavior remain unchanged.

Peers without the new optional feature continue to establish a Phase 3
session. A device requests infrastructure status only when support and
permission are both present. Permission denial is a terminal authorization
result for that feature and does not grant or revoke unrelated availability
permission.

## Hosted connector boundary

Implement the Unraid adapter as a statically registered, hosted connector. It
depends on the provider-neutral feature contract but portable crates never
depend on the connector, JSON, HTTP, TLS, filesystem, Tokio, or Unraid types.
The exact GraphQL query is read-only and requests only:

```graphql
query DeskkinInfrastructureStatus {
  array {
    state
    disks {
      status
    }
    parities {
      status
    }
  }
}
```

The connector sends one bounded POST to one explicitly configured HTTPS
endpoint, does not follow redirects, limits response bytes, requires one
complete GraphQL result without errors, and rejects missing, null, unknown, or
malformed state. DNS, connect, TLS, request write, response headers, and body
read receive explicit deadlines. Cancellation completes the current operation
once and cannot publish a late result.

No HTTP/TLS/URL dependency is approved by this proposal. Before the
implementation checkpoint, propose exact versions and features for a maintained
Rust client and TLS stack, compare the principal alternatives, inspect resolved
licenses and advisories, and obtain dependency approval. The implementation
must not add an invalid-certificate bypass. Trust uses the system roots or an
explicitly configured CA bundle; choosing between those modes remains an open
decision below.

## Credentials and owner control

The desktop host alone owns the Unraid endpoint, trust configuration, and API
key. The key is never accepted in an argument or environment variable and
never enters a result, diagnostic, protocol message, fixture, or repository
file. A hidden prompt or bounded stdin/file-descriptor input may provision it
through the existing private owner-control boundary only after the storage
decision is approved.

Credential storage is intentionally unresolved. The approval review must
choose either an OS secret service with an exact dependency and availability
contract, or a strict private local store whose plaintext-at-rest limitation is
explicit. An encrypted ignored profile may be considered only if startup and
key-unlock behavior are also specified. Failure to retrieve a credential is a
closed connector-unavailable result and does not broaden identity, permission,
or storage scope.

The Unraid API key must be created with only the permissions required for the
read query. Deskkin does not create, rotate, expand, or revoke permissions in
this slice. An authorization denial stops the operation and is not retried with
another identity or a wider key.

## Observability and privacy

Reuse the hosted recorder. One `connector.unraid.read` run spans request
acceptance, provider operation, normalization, feature completion, view
application, and next-timer arm. Closed operations distinguish connector
availability, DNS/connect, TLS, HTTP write/read, GraphQL decode, normalization,
protocol delivery, and presenter application. Closed outcomes distinguish
success, authorization denied, rate limited, timeout, cancelled, malformed,
oversize, source unavailable, recording degraded, partial, and dropped.

Allow only opaque context IDs, closed stage/outcome, duration, virtual time,
normalized view, render size, and RGB565 digest. Forbid API keys, authorization
headers, endpoint/address, certificates, query/response bytes, GraphQL errors,
array/disk names, paths, host/user/process identity, and provider metrics.
Recording remains default-on and locally bounded; recording-off or storage
failure cannot change the feature result, frame sequence, retry behavior, or
exit meaning. Remote export remains absent.

## Reproducible acceptance

The fake boundary must require the exact query and `x-api-key` header while
ensuring its value is absent from process output and artifacts. Its schema and
fixtures are derived from the exact schema commit accepted in the ADR, not a
moving branch. Deterministic fixtures cover every recognized array, data-disk,
and parity-disk state plus HTTP authorization, rate limit, GraphQL errors,
missing/null/unknown fields, malformed and oversize JSON, timeout,
cancellation, late response, disconnect, and reconnect.

Acceptance also covers:

- portable-core dependency boundaries and `no_std` target checks;
- canonical protocol vectors, unknown/trailing/oversize rejection, optional
  feature negotiation, and independent permission grant;
- fake-host, simulator, and device-fake-worker view equivalence;
- two fresh deterministic replays with byte-identical semantic records,
  virtual timestamps, views, and RGB565 frames;
- recording on/off/failure equivalence and a privacy scan proving the key,
  endpoint, query response, and provider identifiers are absent;
- a mutation oracle that rejects any GraphQL mutation and verifies that none is
  attempted;
- `mise run fix`, `mise run test`, portable dependency inspection, and fresh
  durable review.

No reproducible test reads a real credential, uses a physical device, or
connects to an Unraid server.

## Ordered checkpoints

1. Review and approve or revise this proposal, including the domain mapping,
   minimum supported and qualification Unraid OS/API versions, exact upstream
   schema commit, version/schema mismatch behavior, TLS trust, credential
   storage, exact dependency set, timeouts, size bounds, and protocol tags.
   Record the result in a new immutable ADR and synchronize architecture and
   implementation-plan documents in one local commit.
2. In a separate commit, add the portable feature/protocol/UI changes and fake
   connector with deterministic acceptance. Do not add real credential state
   or access a provider.
3. After separate approval, add the selected credential/control storage and
   hosted connector integration, still verified only against the isolated
   fake.
4. Show the exact Unraid target, endpoint trust mode, credential source and
   permissions, device target if any, retained state, and proposed read before
   requesting live qualification authority. Record evidence separately.

Push, release, artifact publication, daemon/autostart installation, firewall
changes, provider mutation, and Phase 5 actions are outside all four
checkpoints unless separately requested.

## Open decisions for approval

- Whether `Operational | Degraded | Stopped | Unknown` is the desired user
  vocabulary and whether all non-started/non-stopped array states belong in
  `Degraded`.
- Whether disk status belongs in the first query or array state alone is the
  correct minimal observable.
- Minimum supported and qualification Unraid OS/API versions, the exact schema
  commit used to generate fixtures, and the closed failure returned for an
  incompatible or unrecognized schema.
- Exact protocol tags, maximum message/response sizes, and hosted timeouts.
- Exact HTTP/TLS/URL dependencies and whether explicit private CA bundles are
  needed in addition to system trust.
- OS secret service versus a documented private local credential store, plus
  its recovery, clear, and owner-generation behavior.
- Whether live qualification should use only the simulator first or include
  the already paired CoreS3 after a separate physical approval.
