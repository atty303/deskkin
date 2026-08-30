# Foundation C proposal: host capability and connector composition

Status: Accepted and implemented

Date: 2026-08-30

## Goal and observable result

Introduce one desktop-host composition owner that translates semantic Deskkin
capability requests into statically registered connector requests and accepts
only the exact typed completion. Provider payloads, credentials, transports,
and protocol messages remain outside the composition crate.

The checkpoint is observable when the existing availability request traverses
the same capability registry and deterministic connector on normal host,
profile-host, serve-once, and simulator E2E paths without changing protocol
major 1 bytes or physical-device state. Connector unavailable, timeout,
cancellation, stale completion, shutdown, and identity exhaustion remain
closed and deterministic.

Foundation C is durable. Its deterministic availability connector is the
current configured fake capability, not a provider connector.

## Scope

Foundation C includes:

- the host-only `deskkin-host-capabilities` crate;
- a closed compile-time capability-to-connector registry;
- explicit connector lifecycle, request routing, and typed completion;
- capability-, connector-, and local-effect-namespaced identities;
- one deterministic availability connector with closed outcomes;
- protocol adaptation only at the desktop-host boundary;
- host diagnostic operations for capability routing and connector execution;
- deterministic unit and existing loopback E2E verification; and
- static dependency inspection through the standard test entrypoint.

It does not include:

- Unraid or another provider, endpoint, payload, credential, HTTP, TLS, or JSON;
- credential or connector configuration persistence;
- new protocol messages, feature bits, permissions, or wire bytes;
- a new portable application feature or device-side capability registry;
- dynamic connector loading, a plugin ABI, scripting, or a connector SDK;
- daemonization, autostart, discovery, or multi-device management;
- private-LAN or physical-device qualification, flashing, provisioning, or NVS
  mutation; or
- push, release, publication, or provider access.

## Composition contract

`deskkin-host-capabilities` owns host semantic capability IDs, connector IDs,
effect identities, requests, completions, lifecycle, and routing errors. It is
independent of Deskkin protocol types and every provider-specific type.

The current closed registry contains exactly:

```text
availability.read -> deterministic availability connector
```

Routing allocates a nonzero local effect identity and combines it with the
selected capability and connector. Only the exact completion for the active
identity is accepted. Busy, stopped, duplicate, stale, mismatched, and exhausted
states return typed errors without publishing candidate state.

The deterministic connector can produce available, unavailable, read failure,
connector unavailable, timeout, or cancellation. It executes through the same
route/complete path used by the host adapter. A future real connector replaces
only connector execution after its own proposal is accepted; it does not move
provider authority into this crate or the device.

## Protocol boundary

The desktop host maps protocol `ReadAvailability` to
`CapabilityRequest::ReadAvailability`. Available and unavailable retain their
existing wire results. All closed connector failures map to the existing
`AvailabilityResult::ReadFailed` because Foundation C does not change protocol
major 1.

The collapse occurs only after connector completion. Host diagnostics retain
whether the semantic failure was read failure, connector unavailable, timeout,
or cancellation. Request IDs, session and operation contexts, feature
negotiation, permissions, pairing, and Noise identity remain unchanged.

## Observation contract

One existing host availability diagnostic run spans transport frame read,
host capability routing, connector execution, and transport frame write. The
closed operations are:

- `availability_read`;
- `transport_frame_read`;
- `host_capability_route`;
- `connector_availability_read`; and
- `transport_frame_write`.

Connector timeout and cancellation use distinct terminal statuses. Connector
unavailable and deterministic read failure use distinct stable error types.
Protocol/session/operation identities may be recorded as existing local
operational identifiers. Connector configuration, provider payload, credential,
request body, response body, and notice content are not recorded.

The host application continues to own local recording, retention, opt-out, and
non-interference through `local-run-recorder`. The composition library owns no
recorder, exporter, global provider, filesystem, or remote destination.

## Acceptance criteria

1. The registry routes availability only to the deterministic availability
   connector and publishes a fully namespaced nonzero effect identity.
2. Every configured success and failure outcome produces the exact closed
   semantic result.
3. Busy, stopped, stale, duplicate, mismatched, post-stop, and identity-exhausted
   transitions are transactional.
4. The desktop host invokes the registry on its normal availability session
   path before producing a protocol result.
5. Connector failures collapse to the existing wire `ReadFailed` only at the
   protocol adapter boundary.
6. Host diagnostics identify capability routing and the exact connector
   failure stage while the client-side availability diagnostic remains free of
   host-only operations.
7. Existing loopback, profile-host, simulator protocol E2E, recording on/off,
   and CoreS3 build behavior remain successful.
8. Static dependency inspection proves the host capability crate has no
   protocol, application, provider, runtime, filesystem, or recorder dependency.
9. `mise run fix`, `mise run test`, and fresh durable review succeed.

No reproducible test reads a real credential, contacts a provider, binds the
private LAN, or mutates a physical device.

## Implementation result

Foundation C completed on 2026-08-30. ADR-0008 records the long-lived boundary.
The deterministic availability configuration formerly passed directly to the
session writer now enters the host capability registry, executes through the
registered connector, and returns through an identity-checked completion.

No external dependency, provider schema, credential state, protocol byte,
feature permission, device firmware behavior, or retained physical state was
added or changed.

## Accepted decisions

- Host connector composition is a separate host-only crate, not part of
  `application-core`, `application-features`, or `deskkin-application`.
- Capability and connector registration is closed and compile-time for this
  checkpoint.
- The current deterministic availability source is a connector conformance
  implementation, not a provider connector.
- Provider-specific mapping and dependencies require a later provider proposal.
- Dynamic loading remains deferred until a demonstrated operational need.
- Protocol routing for a real second wire feature remains a separate decision.

