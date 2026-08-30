# ADR-0006: Select the physical host through a repeatable local profile

- Status: Accepted
- Date: 2026-08-30

## Context

The qualified CoreS3 path depends on one retained desktop-host identity, one
exact assigned listener, one deterministic availability source, and one local
recording choice. Those values previously had to be reconstructed across
low-level Phase 3 commands. Selecting the default disposable role root instead
of the retained physical role fails correctly at the identity boundary, but the
operator has no single repeatable way to select the complete working host.

The approved contract and its authority boundaries are recorded in
[`foundation-a-repeatable-physical-profile-proposal.md`](../foundation-a-repeatable-physical-profile-proposal.md).

## Decision

Store named, ignored, secret-free schema-1 JSON profiles under
`.deskkin/profiles/`. A profile selects one normalized role root below
`.deskkin`, one explicit loopback or exact RFC1918 bind address, one closed fake
availability result, and local recording on or off. Reject unknown fields,
unsafe permissions, symlinks, noncanonical paths, reserved profile namespaces,
invalid addresses, files above 4 KiB, and stores above 32 profiles.

Keep Wi-Fi material, Noise private keys, pairing state, and provider authority
out of profiles. The selected role root references the existing identity and
diagnostic stores without moving, copying, or recreating them.

Use a store-wide private operation lock to serialize profile mutation and
owner resolution. Every desktop-host path that acquires a role owner below the
same `.deskkin` root participates from before owner acquisition through
listener and control readiness. It then retains only the existing role owner
lock. Atomic profile publication distinguishes failures before rename from an
unknown publication result after rename.

Keep the host foreground-only. Extend private owner information with optional
profile launch metadata. Profile status compares that metadata field by field;
profile stop sends the observed owner generation and refuses a stale or
mismatched owner. `SIGINT` and `SIGTERM` enter the same joined shutdown path.
The existing low-level commands remain available and do not acquire profile
semantics.

Record profile host lifecycle, status, and stop through the existing bounded
local recorder. Child protocol diagnostics link to the lifecycle run through a
scenario context. The source allowlist contains only closed operations,
outcomes, error types, durations, completeness, and recording health. Profile
paths, listener addresses, generations, PIDs, environment values, credentials,
protocol bytes, and arbitrary error text are not recorded. Recording failure
does not change host or control results.

Once resolution succeeds, lifecycle records are stored below the selected role
root and child protocol runs link to them. A failure that prevents resolving a
safe role root records only its closed operation and error type in the bounded
`.deskkin/profile-control` diagnostic store; it never copies the invalid name,
path, JSON, or error text into that fallback record.

## Consequences

- `deskkin:profile`, `deskkin:host`, `deskkin:status`, and `deskkin:stop` become
  the repeatable physical-host entrypoints.
- Profile writes and starts for the same state root are intentionally
  serialized for a short readiness interval.
- Owner shutdown is generation-bound for both profile and existing simulator
  lifecycle callers.
- Tokio's existing `signal` feature is enabled for the desktop host; no new
  crate or dependency version is introduced.
- Daemonization, autostart, discovery, multiple listeners, dynamic capability
  registration, provider connectors, and device mutation remain outside this
  decision.

## Alternatives

### Environment-variable bundles

They are easy to launch but do not provide bounded schema validation, atomic
publication, exact status comparison, or a safe persistent operator surface.

### A separate launcher or daemon

It would duplicate owner, lifecycle, diagnostics, and shutdown responsibility
before background operation is required.

### Embed credentials in the profile

It would collapse desktop identity, device provisioning, and provider
authority into a convenience file and broaden both failure and disclosure
scope.
