# First provider proposal: read-only Unraid array status

Status: Proposed; implementation and provider access are not authorized

Date: 2026-08-30

## Goal and observable result

Add the first provider-backed semantic feature as one read-only vertical slice:
the companion displays whether an Unraid array is started, stopped, or requires
attention. The desktop host obtains only the array state from the official
Unraid GraphQL API, converts it to Deskkin semantics, and sends no provider
payload or credential to the device.

The implementation checkpoint is observable when deterministic simulator,
loopback provider fixture, desktop-host protocol E2E, and CoreS3 build paths all
exercise the new infrastructure feature through the portable application,
protocol, host capability registry, and Unraid connector. It does not include a
connection to a real Unraid server. A retained live qualification is a separate
explicitly authorized checkpoint after the reproducible implementation passes.

This is a durable product slice. It is not an availability alias, a provider
connectivity indicator, or a general Unraid SDK.

## Why this slice

[`ADR-0002`](decisions/0002-host-device-boundary.md) already selects Unraid as
the first connector and infrastructure status as its application feature. The
official API currently exposes a GraphQL `array { state }` query, API-key
authentication, and read-only roles. Unraid 7.2 and later include the API in the
OS; earlier versions can expose it through the Unraid Connect plugin without a
cloud login.

Reading only `array.state` is the smallest provider query that produces useful
product semantics. Capacity, individual disks, parity progress, Docker, VMs,
notifications, and every mutation remain separate future slices. Reusing the
existing availability feature would erase the distinction between host
reachability and array state and would contradict the accepted infrastructure
feature boundary.

Official references checked for this proposal:

- [Unraid API overview](https://docs.unraid.net/API/)
- [Using the Unraid API](https://docs.unraid.net/API/how-to-use-the-api/)
- [Current generated GraphQL schema](https://github.com/unraid/api/blob/main/api/generated-schema.graphql)

These references describe a moving provider surface. The checked query and
fixture become Deskkin's reproducible compatibility contract; a future Unraid
schema change is not accepted silently.

## Scope

The proposed implementation includes:

- a portable `infrastructure` feature with a closed array-status surface;
- a required protocol-major-1 feature and separately granted read permission;
- one host capability, one statically registered Unraid connector, and exact
  completion routing through the Foundation C composition owner;
- an Unraid connector crate that performs exactly one bounded GraphQL query;
- an explicit low-level host launch accepting an endpoint and API-key file;
- deterministic provider response fixtures and a loopback HTTP server used
  only by tests;
- shared Slint presentation and CoreS3 compilation without device mutation;
- bounded local diagnostics owned by the desktop host and simulator; and
- a new accepted-on-implementation ADR recording the additive protocol-major-1
  extension without rewriting ADR-0004; and
- static dependency and privacy checks in `mise run test`.

It does not include:

- live Unraid access, credential creation, provider configuration changes, or
  enabling the Unraid API or GraphQL sandbox;
- an Unraid mutation, array start/stop, parity control, Docker or VM control,
  notification acknowledgement, or any other write authority;
- capacity, disk identity, temperature, error counts, parity status, Docker,
  VM, share, host, or registration data;
- profile schema migration, background service, autostart, provider discovery,
  multiple Unraid servers, or credential persistence owned by Deskkin;
- HTTP over a private LAN, disabled TLS validation, redirects, ambient proxy
  use, cookies, SSO/OIDC, Unraid Connect cloud access, or remote diagnostics;
- dynamic connector loading, a plugin ABI, a general GraphQL client surface,
  or a public connector SDK;
- flashing, provisioning, NVS mutation, power cycling, firewall or network
  changes, push, release, or publication.

## Portable infrastructure feature

The feature owns the provider-neutral semantic value:

```text
ArrayStatus
├── Started
├── Stopped
└── AttentionRequired
```

`Started` and `Stopped` are deliberate array states, not transport success and
failure. Every other currently documented `ArrayState` value maps to
`AttentionRequired`. A missing `data.array.state`, a GraphQL `errors` entry, a
null where the checked schema requires a value, or an unknown enum value is a
typed connector/schema failure and publishes no fabricated status.

The feature starts one immediate read and arms one 30-second refresh after
either success or failure. It permits one in-flight read and one timer, rejects
stale or mismatched completions transactionally, and withdraws its surface on
session invalidation or read failure. Withdrawal reveals the existing
availability surface rather than translating provider failure into an array
state.

This feature has an independent domain and expected evolution boundary, so it
is proposed as a separate `feature-infrastructure` crate depending only on
`application-core`. `deskkin-application` registers it before availability.
Both are `Ambient` surfaces; fixed registry order therefore makes a valid array
status visible, while availability remains the fallback. The existing
synthetic notice remains `Information` and still preempts both.

All portable crates remain allocation-free `no_std`, runtime-neutral, and free
of protocol, connector, provider, filesystem, network, Slint, and recorder
dependencies.

## Protocol contract

The protocol major number remains 1, while its negotiated schema gains an
additive extension. Implementation must add a new ADR that preserves ADR-0004
as historical authority and records why negotiation makes a major bump
unnecessary. That ADR becomes the current authority for these values:

- feature byte 0 bit 1: `infrastructure.array-status.v1`;
- permission byte 0 bit 1: `infrastructure.read`;
- tag `0x22`: array-status request, request ID `[u8; 4]`, operation context
  `[u8; 16]`;
- tag `0x23`: array-status result, the same IDs, and one result byte.

The result byte is closed:

```text
0 started
1 stopped
2 attention_required
3 read_failed
```

The new feature is required by the newly built Deskkin application and the read
permission is requested separately. A peer without the feature is rejected by
the existing required-feature negotiation. A peer denying the permission is
rejected by the existing permission contract. No provider name, endpoint,
GraphQL enum, HTTP status, response field, text, capacity, hostname, or
credential crosses the protocol boundary.

The compatibility outcomes are exact:

- new device to old host: the old host rejects the required unknown feature;
- old device to new host: availability-only negotiation remains valid and the
  host sends no infrastructure message;
- new device to new host: both features and both read permissions are selected;
  and
- either peer receiving an unnegotiated infrastructure message treats it as a
  protocol error.

The new ADR and canonical codec vectors must record those outcomes, both bits,
both tags, and every result byte. Existing ADRs and the Phase 3 proposal are not
rewritten to make their previously accepted closed schema appear broader.

Exactly one availability request and one infrastructure request may be active
at the same time. The session retains one strictly monotonic request-ID space;
the message family and operation context complete correlation. Session loss
invalidates both features atomically through the existing application
composition.

## Host capability and connector composition

Foundation C gains the closed mapping:

```text
infrastructure.array-status.read -> unraid array connector
```

`deskkin-host-capabilities` owns only semantic request/result types,
`CapabilityId`, `ConnectorId`, namespaced effect identity, lifecycle, and exact
completion acceptance. It remains independent of HTTP, GraphQL, Unraid types,
credentials, filesystem, Tokio, and recording.

The composition owner expands its single waiting state into one bounded slot
per registered capability so availability and infrastructure can be in flight
concurrently. A second request for the same capability remains `busy`.
Namespaced effect IDs remain globally unique within the owner, and completion
checks both the capability slot and the full capability/connector/local ID.

The desktop host performs the asynchronous connector dispatch between `route`
and `complete`. The provider-specific implementation is isolated in a new
`deskkin-connector-unraid` crate. That crate owns the fixed query, closed DTO,
response-size limit, HTTP/TLS client, and conversion from the provider enum to
the semantic `ArrayStatus`. It owns no recorder, global runtime, filesystem
credential loader, CLI, or protocol type.

The exact request body is equivalent to:

```graphql
query DeskkinArrayStatus {
  array {
    state
  }
}
```

GraphQL partial data accompanied by any `errors` entry is failure. The
connector accepts only the expected top-level shape and current closed
`ArrayState` vocabulary. It does not use introspection at runtime and never
falls back to WebGUI HTML, SSH, `/var/local/emhttp`, the Unraid Connect cloud,
or an undocumented endpoint.

## Endpoint and credential boundary

The low-level product entrypoint is proposed as:

```text
deskkin-desktop-host run-unraid ADDRESS --endpoint URL --api-key-file PATH [--ca-file PATH] [ROLE_ROOT]
```

The raw API key is never a command-line argument or environment variable.
Deskkin reads one explicitly selected regular file, rejects symlinks and files
with group or other permission bits, limits it to 4 KiB, accepts one nonempty
visible-ASCII value without surrounding whitespace, and keeps the value in a
zeroizing source buffer. The desktop host's `run-unraid` owner reads it once at
startup, retains it across every 30-second refresh, and zeroizes and drops it
when connector shutdown completes. Each request borrows that value to construct
a transient header rather than storing it in client-wide default headers.
Transient request and header buffers owned by `reqwest` are not claimed to be
overwritten on drop. Deskkin neither creates, rotates, lists, nor deletes the key
file. The user creates and revokes a read-only key in Unraid. The current
official schema names the read-only role `VIEWER`; actual permission for `array`
must be verified during the separately approved live qualification. An
administrative key is outside this slice.

`--endpoint` is the complete GraphQL URL. It is one explicit absolute URL whose
path is exactly `/graphql`, with no user-info, query, or fragment. It must use
HTTPS with normal hostname and certificate validation. An optional explicit CA
certificate file may extend trust for a private CA. Cleartext HTTP is accepted
only when the URL host is an exact loopback address, allowing the deterministic
test server or a separately managed encrypted tunnel; private-LAN HTTP is
rejected.

`--ca-file` accepts at most 64 KiB containing one or more PEM-encoded X.509 CA
certificates. It must be a regular non-symlink file. Empty input, non-certificate
PEM blocks, trailing non-PEM data, or a bundle containing no certificate fails
startup. Every parsed certificate is added to the normal trust roots; the file
cannot disable hostname, expiry, or signature validation.

The connector sends one HTTP `POST /graphql` with these headers:

```text
content-type: application/json
accept: application/graphql-response+json, application/json
x-api-key: <raw key from the selected file>
```

It sends no `Authorization`, cookie, referer, or provider-specific header. The
JSON request body is exactly this UTF-8 byte sequence, without a trailing
newline:

```text
{"query":"query DeskkinArrayStatus { array { state } }","operationName":"DeskkinArrayStatus","variables":{}}
```

The fixture compares the exact request bytes, method, path, required header
values, and absence of forbidden headers. It compares the key only inside a
zeroizing fixture value and never prints or records either value.

The HTTP client uses `redirect(Policy::none())`,
`retry(reqwest::retry::never())`, `no_proxy()`, and `referer(false)`; it has no
cookie store or client-wide default API-key header. Gzip, Brotli, Zstd, and
Deflate decoding are disabled. This prevents an API key from following a
redirect or ambient proxy configuration and keeps the response cap unambiguous.
The request has a three-second total deadline, the response body is read as
chunks and capped at 16 KiB before `serde_json::from_slice`, and no whole-body
JSON helper is used. Only HTTP 200 with
`application/json` or `application/graphql-response+json` is decoded.
Cancellation is propagated from host shutdown. The connector does not retry;
the portable feature owns the bounded refresh schedule.

Profile schema 1 remains secret-free and unchanged. Named provider
configuration and repeatable physical-profile selection are deferred until the
low-level connector and live read contract are qualified.

## Failure contract

Provider execution returns closed failures without provider text:

- `endpoint_invalid`;
- `credential_unavailable`;
- `tls_configuration_invalid`;
- `authentication_denied`;
- `rate_limited`;
- `provider_rejected`;
- `provider_unavailable`;
- `connection_failed`;
- `response_read_failed`;
- `response_oversize`;
- `content_type_invalid`;
- `schema_invalid`;
- `timeout`; and
- `cancelled`.

Every failure maps to protocol `read_failed` only after exact connector
completion. It withdraws the infrastructure surface and retains availability
as the fallback. No failure maps to `Stopped` or `AttentionRequired`.
Recording failure never changes these results.

Classification is deterministic and stops at the first owning stage:

| Stage or input | Closed failure | Owning operation |
| --- | --- | --- |
| endpoint parse or endpoint policy rejection | `endpoint_invalid` | `unraid_connector_start` |
| API-key file read or validation | `credential_unavailable` | `credential_file_read` |
| CA read/parse or TLS client construction | `tls_configuration_invalid` | `http_client_build` |
| explicit host shutdown before completion | `cancelled` | active connector operation |
| three-second deadline before completion | `timeout` | active connector operation |
| DNS, TCP, TLS handshake/certificate validation, or request-write failure before a response | `connection_failed` | `unraid_graphql_request` |
| HTTP 401 or 403 | `authentication_denied` | `unraid_graphql_request` |
| HTTP 429 | `rate_limited` | `unraid_graphql_request` |
| HTTP 500 through 599 | `provider_unavailable` | `unraid_graphql_request` |
| every non-200 status not classified above, including 1xx, other 2xx/3xx/4xx, and nonstandard 6xx or higher | `provider_rejected` | `unraid_graphql_request` |
| unsupported or missing 200 response content type | `content_type_invalid` | `unraid_response_decode` |
| accepted body exceeds 16 KiB | `response_oversize` | `unraid_response_decode` |
| body stream fails after response headers | `response_read_failed` | `unraid_response_decode` |
| invalid JSON, GraphQL errors, partial data, missing/null field, or unknown enum | `schema_invalid` | `unraid_response_decode` |

Cancellation and the total deadline cover connect, request write, headers, body,
and decode. One atomic terminal result is published. If shutdown cancellation
and the deadline become ready in the same poll, cancellation wins; otherwise
the first observed terminal condition wins. HTTP status is classified before
content type or body, body size is checked while reading chunks before JSON
decoding, and no provider response text participates in classification.

## Observation contract

This path crosses an external network and credential boundary and is therefore
covered by the program observation contract. The desktop host remains the sole
recording owner.

One startup Diagnostic Run covers:

- `unraid_connector_start`;
- `credential_file_read`; and
- `http_client_build`.

One array-status Diagnostic Run covers:

- `infrastructure_status_read`;
- `transport_frame_read`;
- `host_capability_route`;
- `unraid_graphql_request`;
- `unraid_response_decode`; and
- `transport_frame_write`.

Timeout and cancellation use distinct statuses. Authentication, rate limit,
TLS configuration, pre-response connection, response read, HTTP/provider
availability, response cap, content type, schema, queue, and transport failures
retain distinct stable error types.
Connector and downstream delivery failures can coexist in the same run and are
attached to their owning operations, following the Foundation C compound
failure contract.

The source allowlist records only resource/version, opaque run/session/
operation identities, selected protocol feature and permission bits, closed
operation/status/error values, duration, completeness, and recording health.
It excludes the API key and all derived representations, authorization header,
endpoint, URL, hostname, IP, port, certificate content, filesystem path,
GraphQL request or response body, provider error message, HTTP headers,
`ArrayState` provider spelling, and Unraid machine or account identity.

Existing bounded private local recording, opt-out, retention, deletion, and
non-interference remain authoritative. The connector library configures no SDK,
exporter, global provider, filesystem sink, or remote destination.

## Dependency proposal

Implementation requires one workspace-new direct dependency and two existing
workspace dependencies as new direct edges in `deskkin-connector-unraid`:

```toml
reqwest = { version = "=0.13.4", default-features = false, features = ["rustls"] }
serde = { version = "=1.0.229", default-features = false, features = ["derive"] }
serde_json = "=1.0.151"
```

`reqwest` matches the existing Tokio desktop runtime, provides bounded async
request execution, HTTPS through Rustls, explicit redirect policy, explicit
proxy disabling, retry suppression, and chunked response reads. `serde` and
`serde_json` provide the closed DTO and post-limit decoding; the reqwest JSON
helper is intentionally not enabled. Versions and transitive dependencies are
locked. All three remain host-only and do not enter the portable or device
dependency graph. `reqwest` is MIT/Apache-2.0 licensed; `serde` and `serde_json`
are already approved workspace dependencies under the same licensing family.

The principal alternatives are:

- handwritten HTTP/TLS: rejected because framing, redirects, certificate
  validation, cancellation, and body limiting would add security-sensitive
  product code;
- `hyper` plus separate TLS and JSON assembly: lower-level but larger Deskkin
  integration surface for no additional product value;
- blocking `ureq`: simple, but would require a dedicated blocking worker and
  cancellation bridge beside the existing Tokio runtime; and
- native TLS: rejected because it introduces host OpenSSL configuration and
  build variance instead of the repository-locked Rust path.

Approval of this proposal explicitly approves this dependency and feature set.
No additional package may be added without another dependency approval.

## Reproducible verification

The implementation checkpoint must add these tests to `mise run test`:

1. portable feature tests for start, all statuses, fixed refresh, failure
   withdrawal, invalidation, stale completion, stop, and ID exhaustion;
2. composition tests proving infrastructure precedes availability, notice
   still preempts it, failure reveals availability, and invalidation is atomic;
3. canonical protocol-byte tests for the two tags, feature/permission
   negotiation, malformed values, unknown bits, trailing bytes, and concurrent
   request-family correlation, plus the new protocol-extension ADR;
4. capability tests for exact Unraid routing, namespace isolation, lifecycle,
   mismatch, duplicate, cancellation, and compound failures;
5. connector DTO tests for every documented `ArrayState`, unknown/null/missing
   fields, GraphQL errors with and without partial data, invalid JSON, wrong
   content type, and oversized bodies;
6. loopback HTTP tests verifying exact POST path/body/header presence without
   emitting the key, redirect refusal, no proxy use, every HTTP-status row
   including representative 1xx and nonstandard 6xx, response cap/read failure,
   timeout/cancellation priority, and stable classification;
7. credential and endpoint tests for symlink, permissions, size, encoding,
   whitespace, HTTPS validation, explicit CA, loopback-only HTTP, URL user-info,
   query, fragment, and path rejection;
8. desktop-host and simulator protocol E2E for every semantic status, read
   failure, reconnect invalidation, recording on/off parity, compound provider
   and delivery failure, and privacy injection;
9. shared Slint presenter and deterministic frame assertions for all three
   statuses and fallback surfaces;
10. CoreS3 clean product and inert builds without flashing or changing NVS;
11. dependency inspection proving provider dependencies remain host-only; and
12. full `mise run fix` followed by one final `mise run test`.

The loopback provider fixture must not pre-create a product-owned credential,
profile, socket, or diagnostic artifact. It observes the real connector path,
not a diagnostic-only replacement.

## Separately authorized live qualification

After implementation and independent review pass, live qualification requires
a new explicit user instruction. Before that run, read-only checks identify the
exact endpoint, API availability/version, TLS trust path, and pre-existing
read-only key file without displaying its content.

The authorized live run is exactly one connector startup and bounded
`array.state` query through the normal desktop-host path. It must read back the
semantic result and local diagnostic run, verify that the request caused no
Unraid mutation, and leave API, key, firewall, DNS, certificate, profile,
pairing, Wi-Fi, device, and NVS state unchanged. Deskkin does not create or
broaden a key. Any permission failure stops at the existing identity and does
not switch to an administrator credential or change Unraid IAM.

Physical-device display qualification, named profile integration, credential
storage, and repeated operational use remain later checkpoints even if this
single live read succeeds.

## Acceptance criteria

The implementation is complete only when:

1. a provider-independent infrastructure feature presents exactly the three
   array statuses and never treats transport/provider failure as a status;
2. the application registry and surface arbitration behave identically in all
   portable runtimes;
3. protocol major 1 carries only the closed semantic request/result with exact
   feature and permission negotiation, canonical vectors, compatibility
   outcomes, and a new additive-extension ADR;
4. the host routes through the Foundation C owner and only the Unraid connector
   sees GraphQL, HTTP, TLS, endpoint, and credential values;
5. the connector issues only the fixed `array.state` query with bounded,
   non-redirecting, no-proxy, validated transport;
6. credentials remain host-only; the `run-unraid` owner retains the zeroizing
   source buffer across refreshes and zeroizes and releases it at connector
   shutdown, no persistent HTTP-client copy exists, and raw values remain absent
   from CLI arguments, diagnostics, provider fixtures, artifacts, protocol, and
   device state;
7. every reachable failure is closed, transactional, observable at its owning
   stage, and non-interfering when recording is unavailable or disabled;
8. all deterministic and clean-build verification passes from the standard
   entrypoint; and
9. no live provider, physical device, profile schema, external state, remote
   repository, release, or publication is changed.

## Approval boundary

Approval is required for the following as one implementation checkpoint:

1. the infrastructure feature, surface priority, refresh, protocol feature,
   permission, tags, result bytes, compatibility outcomes, and protocol ADR;
2. the host capability and separate Unraid connector crate with the fixed
   read-only GraphQL mapping;
3. the explicit endpoint/API-key-file boundary and the proposed `reqwest`
   dependency; and
4. the deterministic observation and verification contract.

Approval authorizes repository implementation, dependency locking, loopback
network tests, and clean CoreS3 builds only. It does not authorize live Unraid
access, API/key/configuration changes, private-LAN HTTP, profile migration,
physical-device use or mutation, push, release, or publication.
