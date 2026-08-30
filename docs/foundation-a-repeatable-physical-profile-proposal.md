# Foundation A proposal: repeatable physical profile

Status: Accepted, implemented, and physically qualified

Date: 2026-08-30

This proposal was the approval boundary for Foundation A. Its accepted
architecture is recorded in
[`ADR-0006`](decisions/0006-repeatable-physical-profile.md), and its isolated
profile and host-lifecycle path is implemented. The separately authorized
retained-CoreS3 qualification completed on 2026-08-30; its privacy-safe evidence
is recorded in
[`foundation-a-repeatable-physical-profile-qualification.md`](foundation-a-repeatable-physical-profile-qualification.md).
This proposal's acceptance did not itself authorize that live host launch,
identity mutation, device access, or private network exposure.

## Goal and observable result

Replace the manually reconstructed physical-host command line with one named,
secret-free local profile. Given an exact profile name, Deskkin resolves the
same host role root, bind mode and address, fake availability result, and
recording mode on every run. The same profile selects foreground host launch,
read-only owner status, and exact generation-bound stop.

The first physical profile points to the already retained Phase 3P host role
root; it does not move, copy, recreate, or reinterpret that identity. After the
isolated implementation passes, a separately approved live qualification must
show that the current CoreS3 reconnects to that exact profile without pairing,
provisioning, flashing, NVS mutation, or power cycling.

The resulting operator surface is:

```text
mise run deskkin:profile -- list
mise run deskkin:profile -- show PROFILE
mise run deskkin:profile -- set PROFILE OPTIONS...
mise run deskkin:profile -- delete PROFILE
mise run deskkin:host -- --profile PROFILE
mise run deskkin:status -- --profile PROFILE
mise run deskkin:stop -- --profile PROFILE
```

`deskkin:host` remains a foreground process. Foundation A adds no daemon,
autostart, background launcher, service manager, discovery, or multi-device
supervisor.

## Observed problem

The qualified physical host identity is held below an explicitly selected
qualification role root rather than the disposable default root. During Phase
3P qualification, launching the host through the default root failed at the
identity boundary even though the retained identity was valid. The operator
had to reconstruct the correct role root, exact assigned private-LAN address,
fake availability result, and recording choice across separate commands.

The existing runtime already owns the hard parts: fail-closed identity stores,
exact loopback and RFC1918 bind validation, one process owner, stale socket
cleanup under the owner lock, generation-bound mutations, bounded diagnostics,
and joined shutdown. Foundation A should select those existing boundaries as a
unit rather than add a second runtime or configuration system.

## Local profile contract

Profiles are ignored local state under `.deskkin/profiles/`. One profile is one
regular JSON file named `<profile>.json`. A profile name is 1 through 32 ASCII
lowercase letters, digits, and single hyphens, begins and ends with an
alphanumeric character, and cannot contain consecutive hyphens. At most 32
profiles and 4 KiB per file are accepted.

The profile directory is mode 0700 and files are mode 0600. Symlink profile
roots, profile files, and role-root path components are rejected. Unknown JSON
fields, duplicate fields, trailing data, invalid UTF-8, unsupported schema
versions, and noncanonical values fail closed.

Schema 1 is:

```json
{
  "schema_version": 1,
  "role_root": "phase3/physical-demo/host",
  "bind": {
    "mode": "private_lan",
    "address": "192.168.0.10:39042"
  },
  "availability": "available",
  "recording": "on"
}
```

`role_root` is a normalized relative path below `.deskkin/`. It cannot be
empty, absolute, contain `.` or `..`, escape the state root, or begin with the
reserved `profiles` or `profile-control` component. The selected identity root
is exactly `<state-root>/<role_root>/identity`; control and diagnostics remain
siblings owned by the existing host runtime.

`bind.mode` is closed:

- `loopback` accepts one explicitly selected IPv4 or IPv6 loopback socket;
- `private_lan` accepts one assigned RFC1918 IPv4 address on fixed port 39042.

Wildcard, unspecified, public, link-local, multicast, broadcast, unassigned,
and private-LAN IPv6 addresses are rejected. Profile loading does not bind;
`deskkin:host` reuses the existing bind validation immediately before launch.
It never changes an interface, route, DNS, or firewall.

`availability` is exactly `available`, `unavailable`, or `read_failed` and
selects only the existing deterministic fake capability. `recording` is exactly
`on` or `off` and selects the existing local host recorder.

The schema has no extension map or arbitrary attributes. It cannot contain a
Wi-Fi password, Noise private key, pairing authentication string, provider
credential, environment expansion, command, shell fragment, URL, or embedded
file content. The role-root path and bind address are operational identifiers,
not credentials; they remain in the private profile but are forbidden from
diagnostic records.

## Profile management

Implement profile parsing and lifecycle in the existing desktop-host package,
using its approved `serde` and `serde_json` dependencies. Add no crate or direct
dependency.

A store-wide operation lock at `.deskkin/profile-control/operation.lock`
serializes every profile set, delete, host startup, status, and stop resolution,
including different profile names that select the same role root. Host startup
holds it from profile read through owner readiness; set and delete hold it
through owner checks and durable filesystem publication. The foreground host
does not retain this lock after readiness. Consequently a profile cannot be
replaced between launch resolution and owner publication, while a later
mutation still observes and refuses the live owner. The private
`profile-control` directory is not a profile store and is never enumerated by
`list`.

Every existing desktop-host entrypoint that can acquire a role owner lock below
the same `.deskkin` state root, including the low-level `owner`, `run`, and
`run-private-lan` paths, participates in this store-wide startup barrier from
before role-owner acquisition through owner readiness. It then releases the
barrier and retains only the existing role owner lock. This closes the
check-to-publication race without routing low-level commands through a profile
or changing their listener, identity, protocol, or recording arguments. Owner
paths outside the managed `.deskkin` state root retain their current behavior.

`profile set` accepts only the closed schema fields as explicit arguments,
writes a sibling temporary file with mode 0600, syncs it, renames it atomically,
syncs the profile directory, and reads the stored profile back before reporting
success. It refuses to replace a profile when either the previously stored or
newly proposed role root has a live owner. A failure before rename leaves the
prior profile intact. A directory-sync or readback failure after rename returns
the closed result `publication_unknown`; the exact stored-file readback on the
next locked operation is authority, and no success or rollback is fabricated.
The role root need not exist at profile-write time; host launch still requires
the existing runtime's valid initialized identity and private state boundaries
before readiness.

`profile delete` validates the exact name and current file, refuses when its
role root has a live owner, removes exactly that regular file, and syncs the
profile directory. `list` returns sorted valid names and fails on any unknown,
unsafe, or malformed directory entry rather than silently omitting it. `show`
returns the canonical secret-free JSON for one exact name.

These operations are short, synchronous, deterministic, use atomic readback,
and expose a closed error at the failing boundary. They are excluded from the
Diagnostic Run requirement; their result and filesystem readback are the
result and verification surfaces. They never start a listener or owner.

## Foreground host lifecycle

`deskkin:host -- --profile NAME` performs these steps in order:

1. load and validate the exact profile;
2. resolve the role, identity, control, and diagnostic roots below `.deskkin`;
3. acquire or prove exclusive ownership through the existing owner lock;
4. validate and bind the exact configured socket;
5. start the identity actor, owner control, protocol worker, and diagnostics;
6. publish owner readiness only after the listener and control surface are
   usable;
7. serve the existing availability contract until owner stop, `SIGINT`, or
   `SIGTERM`; and
8. close the listener, terminalize active runs, join workers and the identity
   actor, remove the owner socket, and release the lock.

Any failure before readiness cleans up every resource already acquired and
reports `start_failed`; it does not leave a status-visible running owner.
The foreground entrypoint handles `SIGINT` and `SIGTERM` by requesting the same
joined shutdown used by owner stop and exits only after it completes or reaches
the same bounded failure result. Uncatchable process death is recovered as the
existing partial diagnostic run and stale owner state is cleaned only while
holding the owner lock.

Extend the private owner-info response with the profile name and exact closed
launch metadata: relative role root, bind mode and address, fake availability,
and recording mode. This data remains on the private Unix control surface and
is not a Deskkin protocol message. It lets status and stop compare the running
owner with the currently stored profile field by field; a digest collision or
file timestamp is never used as authority.

`deskkin:status` loads the profile, discovers the owner through its exact
control root, and returns one closed state:

- `stopped`: no owner holds the exact role lock;
- `running`: the live owner metadata exactly matches the profile;
- `profile_mismatch`: an owner is live at that role root with different launch
  metadata;
- `owner_unknown`: liveness or generation cannot be determined safely.

`deskkin:stop` first requires `running`, then sends a shutdown command carrying
the observed owner generation. The owner rejects a stale generation. Stop
reports success only after the runtime shutdown coordinator has closed the
listener, terminalized operations, joined workers and the identity actor,
removed its socket, and released ownership. It never signals a PID, deletes a
lock, or stops a mismatched owner.

Existing low-level `phase3:host` commands remain available and retain their
accepted semantics. The new tasks are profile-resolving entrypoints over the
same runtime, not compatibility aliases or a second implementation.

## Program observation contract

The asynchronous lifecycle path is subject to the program observation
contract. One `host.profile.lifecycle` Diagnostic Run corresponds to one
foreground launch and links these closed operations in causal order:

- `profile.resolve`;
- `host.owner.acquire`;
- `host.bind`;
- `host.runtime.start`;
- the existing pairing, session, availability, and identity child runs;
- `host.runtime.stop`; and
- `host.owner.release`.

Status uses `host.profile.status`; stop uses `host.profile.stop` and links the
observed and accepted owner generations without retaining their raw values.
Closed outcomes distinguish success, publication unknown, invalid profile,
unsafe profile store, identity unavailable, owner busy, profile mismatch,
invalid or unassigned address, bind failure, startup failure, shutdown rejected,
stale generation, shutdown timeout, worker failure, interrupted, recording
degraded, partial, and dropped.

Run outcome, operation status, completeness, missing reason, error type, and
recording health remain separate existing fields. Only opaque run/operation
identities, closed profile identity, bind mode, fake availability, recording
mode, lifecycle stage/outcome, duration, completeness, and health are allowed.
Forbid role and filesystem paths, addresses, ports, owner generation, PID,
hostname, username, environment, keys, authentication strings, protocol bytes,
Wi-Fi material, provider data, and arbitrary error text.

Recording follows the profile's `on` or `off` selection, remains local and
bounded below the role root, and has no remote export. Recording disabled,
storage failure, capacity exhaustion, partial recovery, and dropped records
cannot change profile resolution, listener scope, owner lifecycle, protocol
results, or exit meaning.

## Reproducible acceptance

The implementation checkpoint passes only when all of the following hold:

1. Schema tests cover every accepted bind, availability and recording value;
   name/path/size/count bounds; unknown, duplicate and trailing fields; unsafe
   permissions and symlinks; reserved profile namespaces; and secret-shaped
   unknown fields.
2. Profile set, replacement, list, show and delete prove atomic write/readback,
   deterministic sorting, exact deletion, pre-publication rollback,
   post-rename `publication_unknown`, interrupted temporary cleanup, refusal
   while the selected role has a live owner, and store-wide serialization
   against same-name and cross-name launch between profile resolution and owner
   readiness. The same race fixture covers each existing low-level role-owner
   startup path below `.deskkin` against profile set and delete.
3. Isolated temporary state roots prove profile-selected loopback launch,
   readiness, status, one authenticated fake availability read, generation-
   bound stop, complete joins, socket cleanup, and a second identical launch.
4. Tests cover concurrent launch, stale socket, partial startup at every
   acquired resource, profile mismatch, owner restart between status and stop,
   stale shutdown generation, shutdown timeout, and worker failure without
   stopping an unrelated owner.
5. Existing private-LAN validation proves the profile path cannot widen bind
   scope, use a different port, or mutate host networking. No reproducible test
   opens a non-loopback listener.
6. Recording on, off, storage failure and capacity exhaustion preserve public
   lifecycle and availability results. Crash recovery produces a correlated
   partial run without fabricating successful shutdown.
7. A privacy fixture proves profile paths, addresses, owner generations,
   credentials, arbitrary error text, and injected environment values are
   absent from diagnostics and public single-line lifecycle results.
8. Existing low-level host, simulator, protocol bytes, identity stores,
   pairing, reconnect, CoreS3 firmware, device profile, and recovery tests
   remain unchanged in behavior.
9. A new ADR records the approved profile, owner metadata, lifecycle, signal,
   and observation boundaries without rewriting accepted decisions.
10. `mise run fix`, `mise run test`, portable dependency inspection, and a
    fresh durable review succeed.

The reproducible checkpoint uses only loopback and isolated temporary state. It
does not read the retained host identity, bind the private LAN, contact the
CoreS3, or mutate a device.

## Dependencies, licensing, and distribution

Foundation A adds no crate, dependency version, tool, or generated lockfile
change. Profile JSON reuses the exact approved `serde 1.0.229` and
`serde_json 1.0.151` contracts already owned by the desktop host. Foreground
signal handling enables Tokio's existing `signal` feature for the desktop-host
normal and development dependency declarations; this expands compiled desktop
runtime code but introduces no new package or version. It adds no Slint code
and does not change the MIT source or existing GPL-bearing binary boundaries.
It creates no service, release, or published artifact.

## Ordered checkpoints

1. Review and approve or revise this proposal. Approval authorizes only the
   local profile, foreground host lifecycle, isolated tests, task entrypoints,
   and documentation defined above.
2. Record the approved long-lived architecture in a new ADR, then implement
   Foundation A without creating a real profile for retained state, launching a
   private-LAN host, or accessing a device. Commit the reviewed, reproducibly
   verified ADR and implementation as one local checkpoint.
3. Show the proposed exact profile name, role root, bind address/mode, fake
   result, recording mode, retained identity state, expected listener, stop
   behavior, and diagnostic root. Request separate live authority.
4. After approval, create the ignored local profile and launch it foreground.
   Verify status and the already paired CoreS3 reconnect, then stop the host and
   record privacy-safe qualification evidence. Do not pair, provision, flash,
   unpair, erase, or power-cycle implicitly.

Push, release, package installation, daemon/autostart configuration, firewall
or interface changes, provider access, device mutation, Foundation B, and
Foundation C remain outside all checkpoints unless separately requested.

## Approval choices fixed by this proposal

- Profiles are private ignored JSON under `.deskkin/profiles`, not committed
  machine configuration and not environment-variable bundles.
- The existing desktop-host package owns parsing and lifecycle; no new crate or
  dependency version is added, while Tokio's existing `signal` feature is
  enabled for controlled foreground shutdown.
- Host launch is foreground-only. Status and generation-bound stop use the
  existing private owner control surface.
- One profile selects one host role and listener. Multi-device, discovery,
  daemonization, and dynamic capability registration remain deferred.
- The only configured capability is the existing closed fake availability.
- Existing identity and Wi-Fi state are referenced but never moved or embedded.
