# Phase 3 proposal: paired host protocol vertical slice

Status: Approved  
Date: 2026-08-23

Approved on 2026-08-24. The user approved all five decision groups and the
ordered two-checkpoint execution: documentation and ADR first, followed by
implementation and local verification. The approved Snow direct dependency is
`snow = 0.10.0`; the `1.0.0` version in the conversational implementation
summary was corrected before dependency adoption because that release does not
exist. This approval does not authorize a live demonstration, provider access,
physical-device operation, non-loopback exposure, distribution, push, or
release.

## Goal and observable result

Prove the smallest provider-neutral path from one paired Linux desktop host to
one hosted device simulator. After an authenticated session and capability
negotiation, the simulator requests availability, receives one semantic
`Available` result from a fake host port, and applies it through the existing
application core, presenter, and Slint status surface.

The same saved identities must reconnect without a new pairing confirmation.
A disconnected or rejected session must not fabricate availability: the core
shows `Unknown` and a later authenticated retry can recover.

This is a durable protocol foundation, not a disposable spike. Its first
transport is deliberately loopback-only so the slice can validate framing,
identity, authentication, negotiation, reconnection, backpressure, and
diagnostics without claiming that a physical device or LAN deployment is
ready.

## Scope and hard stops

Phase 3 adds:

- a portable `no_std` protocol message and codec crate;
- one Linux desktop-host application with a fake availability port;
- a protocol client adapter and deterministic scenarios in the hosted
  simulator;
- explicit local pairing, pinned reconnect, exact unpair, and private identity
  persistence;
- a provider-neutral typed core availability-invalidation event;
- bounded protocol diagnostics and loopback end-to-end verification.

Phase 3 does not add:

- Unraid or another provider connector, credential, payload, or live access;
- a physical-device build, Zephyr adapter, Wi-Fi discovery, or LAN listener;
- mutation capabilities, actions, confirmation of provider mutations, or
  replay semantics for mutations;
- dynamic connector loading, multi-device hosting, background service
  installation, autostart, package, release, or artifact publication;
- compatibility aliases or migration for a pre-existing protocol or pairing
  store, because neither exists.

The listener binds only an explicit IPv4 or IPv6 loopback address. Non-loopback
binding is rejected rather than hidden behind an unsafe flag. Physical-device
transport and deployment require a later approved checkpoint.

## Workspace boundary

The implementation checkpoint may add:

```text
crates/deskkin-protocol/       no_std messages, bounded codec, frame contract
crates/local-run-recorder/     hosted reusable form of the Phase 2 recorder
apps/desktop-host/             Linux host, pairing store, session runtime
apps/simulator/                existing UI plus protocol client and scenarios
```

`application-core` remains dependency-free and unaware of transport,
serialization, identity, authentication, or the desktop host. The protocol
crate does not depend on Tokio, Snow, Slint, filesystem APIs, sockets, or the
application core. Adapters map versioned protocol availability to and from the
core domain types.

Extracting the Phase 2 recorder must preserve its paths, schema behavior,
retention, CLI controls, and tests. It is a hosted library: binaries own its
configuration and storage, and the library does not install a global provider
or choose a remote exporter.

## Session and message contract

The stable encrypted bootstrap envelope carries:

- bootstrap schema major `1`;
- supported protocol-major bitset, initially only protocol `1`;
- separate required and optional feature bitsets, initially only
  `availability.read.v1` as required;
- a requested-permission bitset, initially `availability.read`;
- selected protocol major, selected feature bitset, and independently granted
  permission bitset;
- a 128-bit session context identity and a session-local monotonically
  increasing request identity;
- a 128-bit operation context identity on every availability request, echoed
  unchanged by its result.

After Noise completes, the encrypted bootstrap schema owns pairing and
negotiation control. Its canonical pairing messages are:

- `pairing_begin { pairing_transaction_id }` from the initiator;
- `pairing_decision { pairing_transaction_id, confirmed | rejected }` from
  each peer after its own local confirmation;
- `pairing_prepared { pairing_transaction_id }` from each peer after durable
  pending publication;
- `pairing_commit { pairing_transaction_id }` from the initiator after both
  prepared messages;
- `pairing_committed { pairing_transaction_id }` from each peer after durable
  committing publication;
- `pairing_complete { pairing_transaction_id }` from the responder after both
  committed messages while it remains committing;
- typed pairing close with rejected, expired, incomplete, or store-failed
  reason.

The transaction identity is a fresh 16-byte operational identifier generated
by the initiator. The only valid role-specific transition sequence is:

| Boundary | Initiator durable state and action | Responder durable state and action | Failure state |
| --- | --- | --- | --- |
| begin/decision | send begin; after responder decision, send local decision | after begin, send local decision | rejection, expiry, or I/O leaves both unpaired |
| initiator prepare | compare generation/key/transaction, publish pending, send prepared | remain unpaired | initiator pending, responder unpaired |
| responder prepare | remain pending | receive prepared, compare and publish pending, send prepared | both pending once responder publication occurred; otherwise initiator pending only |
| commit | after responder prepared, send commit while remaining pending | receive commit, compare and publish committing, send committed | send/receive failure preserves each side's actual pending or committing publication |
| initiator committed | receive responder committed, compare and publish committing, send committed | remain committing and await committed | both committing after initiator publication, even if its send is ambiguous |
| responder complete | remain committing and await complete | receive committed, remain committing, and send complete | both committing if complete is lost |
| initiator complete | receive complete, compare and publish paired, send hello, and await hello ack | remain committing and await valid hello | complete or hello loss leaves initiator paired and responder committing |
| responder paired | remain paired and await hello outcome | receive structurally valid hello with matching session context, compare and publish paired, then negotiate capabilities | malformed/context-mismatched hello leaves responder committing; version/capability mismatch does not undo paired trust |
| session outcome | receive hello ack or reject; report pairing success in either case and session success only for ack | send hello ack or typed hello reject, then report pairing success | outcome loss leaves both paired; application incompatibility remains a distinct session result |

Both confirmed decisions are exchanged before either pending publication.
Every publication is compare-and-publish against the expected generation,
transaction, remote key, and prior state. Duplicate, out-of-order,
mismatched-transaction, or stale-generation control fails typed incomplete
without publishing. A clean interruption reports each side's actual unpaired,
pending, committing, or paired state; it never claims that all failures remain
pending. `committing` and any asymmetric result deny application sessions and
require exact unpair. Only when both stores list paired may result-delivery loss
converge through an ordinary pinned reconnect without fresh confirmation. No
single peer infers the remote durable state merely from its own paired value.
The contract distinguishes result delivery from durable trust state instead of
fabricating global atomic command success.
Pairing control is not a protocol-major application feature and is unavailable
after bootstrap enters an application session.

After successful pairing or pinned-peer authentication, protocol-major
negotiation selects the highest common supported major. No overlap is a typed
terminal error. Every required feature must be supported and selected or
negotiation fails; optional features are intersected. Separately, the host
evaluates requested permissions against peer policy and must not grant an
unrequested permission. This slice requires the requested `availability.read`
permission for a status session, so denial is distinct typed
`permission_denied`, not unsupported feature. Unknown optional feature and
permission bits are ignored and never granted. Protocol `1` contains only:

- `hello` and `hello_ack`;
- `hello_reject` with a closed negotiation reason;
- `read_availability { request_id, operation_context_id }`;
- `availability_result { request_id, operation_context_id, available |
  unavailable | read_failed }`;
- `ping`, `pong`, and typed `close`.

The canonical plaintext encoding is an outer one-byte tag followed by the
listed fields in order. Fixed byte arrays are emitted verbatim; no integer
serializer encoding is involved for them. There are no omitted/default fields
and trailing bytes are rejected.

| Tag | Message and ordered payload |
| --- | --- |
| `0x01` | pairing begin: transaction `[u8; 16]` |
| `0x02` | pairing decision: transaction `[u8; 16]`, decision `u8` (`0` confirmed, `1` rejected) |
| `0x03` | pairing prepared: transaction `[u8; 16]` |
| `0x04` | pairing commit: transaction `[u8; 16]` |
| `0x05` | pairing committed: transaction `[u8; 16]` |
| `0x06` | pairing close: transaction `[u8; 16]`, reason `u8` (`0` rejected, `1` expired, `2` incomplete, `3` store failed, `4` pairing busy) |
| `0x07` | pairing complete: transaction `[u8; 16]` |
| `0x10` | hello: session context `[u8; 16]`, protocol-major bits `[u8; 8]`, required-feature bits `[u8; 8]`, optional-feature bits `[u8; 8]`, requested-permission bits `[u8; 8]` |
| `0x11` | hello ack: session context `[u8; 16]`, selected major `u8`, selected-feature bits `[u8; 8]`, granted-permission bits `[u8; 8]` |
| `0x12` | hello reject: session context `[u8; 16]`, reason `u8` (`0` no common version, `1` required feature unsupported, `2` session busy, `3` permission denied) |
| `0x20` | availability request: request ID `[u8; 4]`, operation context `[u8; 16]` |
| `0x21` | availability result: request ID `[u8; 4]`, operation context `[u8; 16]`, result `u8` (`0` available, `1` unavailable, `2` read failed) |
| `0x30` | ping: empty |
| `0x31` | pong: empty |
| `0x32` | application close: reason `u8` (`0` normal, `1` protocol, `2` timeout, `3` cancelled, `4` unpaired) |

Protocol bit `n` means protocol major `n`; protocol 1 is byte 0 bit 1.
Feature byte 0 bit 0 is `availability.read.v1`; all other bits are currently
unknown. Permission byte 0 bit 0 is `availability.read`; permission bits are a
host policy decision, never inferred from feature support. A request identity
is a big-endian `u32` stored in its four-byte field,
starts at 1, and increments without wrap. Exhaustion closes the session with a
typed terminal error before another request is emitted, leaving the core state
unchanged until the adapter's ordinary disconnect invalidation.

Initiator sends hello and responder sends exactly one hello ack or hello reject
that echoes the session context. A fresh-pairing connection performs the
role-fixed pairing exchange first; a pinned connection proceeds directly to
hello. The first canonical vectors include ping `30`, pong `31`,
and a request with ID 1 and an all-zero test operation context:
`20 00000001 00000000000000000000000000000000`. These vectors, a complete
zero-filled vector for every tag, and rejection vectors are copied unchanged
into the ADR and conformance tests before implementation code is accepted.

Messages use fixed-capacity fields and closed enums. They contain no provider
name, provider payload, UI property, filesystem path, human text, credential,
or host command. A request and operation-context identity pair is unique within
one authenticated session. Exactly one availability request may be in flight
per device; a duplicate, missing, or mismatched identity is rejected without
completing the wrong core effect.

The host fake returns `Available` for the accepted vertical slice. A fake-port
failure maps to `read_failed`, which the client adapter maps to the core's
existing read failure and therefore `Unknown`, never `Unavailable`.

Authenticated session loss invalidates a previously displayed value
immediately. If a core read is active, the adapter completes that effect exactly
once as a read failure. Otherwise it sends a new typed
`AvailabilityInvalidated::SourceUnavailable` core input, which changes the view
to `Unknown` without changing the current state, armed refresh timer, or effect
identity. `HostDisconnected` remains an adapter/diagnostic closed reason and
does not enter the portable domain. A stopped core ignores invalidation. Frames
arriving from the closed session cannot complete an effect; the connection FSM
reconnects independently, and a successful read at a later scheduled refresh
restores the semantic view.

## Transport, framing, and bounds

The transport is TCP over loopback. The connection begins with the exact six
bytes `44 53 4b 4e 00 01`: ASCII `DSKN` followed by bootstrap schema major 1.
Those exact six bytes, not a descriptive string, are passed as the Noise
prologue and are therefore bound into the authenticated handshake. Handshake
frames and encrypted transport frames use a two-byte big-endian length followed
by exactly that many bytes.

Immediately after TCP connect, the initiator sends the six-byte prelude exactly
once. Before constructing or advancing its Noise responder, the responder reads
and validates exactly those six bytes under the partial-frame deadline and
sends no prelude in return. A mismatch closes as typed bootstrap-version error;
prelude bytes are not a length-framed Noise message.

Bounds are fixed in source:

- handshake frame: at most 1024 bytes;
- encrypted application frame: at most 16 KiB including authentication data;
- at most four concurrent pre-authentication connections/tasks;
- one device session and one in-flight availability request;
- internal channel capacity: eight messages per direction;
- connect timeout: 2 seconds;
- partial-frame completion and complete-frame write timeout: 2 seconds;
- Noise handshake and pinned-session bootstrap timeout: 5 seconds each;
- each expected encrypted pairing-control response: 5 seconds;
- availability-result timeout after request write: 2 seconds;
- explicit pairing-confirmation window: 60 seconds;
- idle timeout: 30 seconds;
- reconnect delays: 250, 500, 1000, 2000, then 5000 ms capped.

The host's accept wait has no per-accept timeout; it ends on explicit host
shutdown, and an unpaired connection is still constrained by the pairing
window. Waiting for the first frame-prefix byte is governed by the 30-second
idle timeout only when no pairing exchange or availability request is active.
Receiving the first prefix byte starts the two-second partial-frame deadline;
beginning a frame write starts the two-second complete-write deadline. A
complete frame read or write resets idle. Pairing has its own 60-second overall
window/confirmation deadline described below; Noise must finish within both its
five-second deadline and the remaining window. A successfully
written availability request starts its two-second result deadline, which
completes the core read once as failure and closes the session on expiry.
Normal five-second refresh silence therefore does not close a session.

After both decisions, each expected prepared, commit, or committed response
must begin within five seconds; its partial frame still has the two-second
completion deadline. A non-cancellable store publication suspends that step
deadline only while the local actor is inside its publication region, then
restarts a fresh five-second step deadline. Expiry terminates
`pairing_incomplete` with the actual durable state. Thus only an entered
filesystem sync has the separately documented unbounded residual.

The listener owns a four-permit pre-auth task set. Accept beyond capacity is
immediately closed and recorded locally as typed `preauth_capacity`; because no
authenticated context exists, the initiator observes typed EOF-before-Noise
rather than a trusted remote reason. Each admitted task owns one bounded
handshake buffer and its declared deadlines. Host shutdown cancels and joins
the complete task set before releasing listener/control ownership.

After Noise, one mutex-protected admission step handles the first encrypted
message. A valid pairing begin atomically reserves an open window and binds its
transaction/key; a pinned peer's valid hello atomically claims the sole
authenticated-session slot. A concurrent unknown-key loser receives encrypted
pairing close reason `pairing_busy` using its submitted transaction; a
concurrent pinned-key loser receives hello reject reason `session_busy` using
its submitted session context. Moving a task into the single session slot
releases its pre-auth permit. Session teardown releases the slot only after its
reader, writer, and request tasks join. The same pinned identity racing two
reconnects can therefore receive at most one hello ack.

Explicit shutdown/cancellation wins a simultaneous deadline; otherwise a
specific malformed/authentication/protocol failure wins over a generic timeout
already observed in the same operation. The simulator client adapter alone
owns reconnect scheduling. The host returns to accept after session teardown;
neither the core nor the recorder owns reconnect.

The client connection FSM is exactly `disconnected | backoff | connecting |
authenticated | stopped`, with at most one connection attempt. Session loss
invalidates the view and enters the next backoff immediately; the delay series
advances on each connect, Noise, bootstrap, or pre-success session failure and
resets only after a valid availability result. Successful TCP or hello alone
does not reset it. Backoff expiry starts one connect independently of the core's
five-second timer. If a core refresh becomes due while not authenticated, that
read effect fails immediately and arms its ordinary next timer; it neither
starts nor duplicates a connection attempt. An authenticated reconnect leaves
the view `Unknown` until the next core refresh succeeds. Core stop, exact
unpair, application shutdown, or terminal identity failure cancels pending
backoff/connect work and enters stopped; pairing a new exact peer explicitly
starts a fresh disconnected FSM with its backoff counter reset.

Hello reject is reason-specific. No-common-version and required-feature
unsupported preserve paired identity but enter terminal `incompatible` stopped
state. Permission denied preserves paired identity but enters terminal
`authorization_denied` stopped state; a later host-policy change requires an
explicit new connect command rather than background retry. Session busy alone
enters ordinary reconnect backoff. Virtual-time tests prove that terminal
reasons never retry and session busy follows and resets the declared series.

Length is validated before allocation or decode. EOF mid-frame, oversize,
malformed serialization, queue overflow, timeout, cancellation, and peer close
remain distinct typed outcomes. No queue, retry series, diagnostic flush, or
read buffer grows without a declared bound. Retry uses virtual time in
deterministic scenarios and does not add random jitter in this loopback-only
slice.

Exactly one writer actor owns the Noise transport send state, nonce sequence,
frame encoding, and TCP write half for a session. No other task encodes or
writes a frame. `read_availability` and `availability_result` use its bounded
eight-item application mailbox with a two-second submit-through-complete-write
deadline. A reserved single control slot accepts at most one close, pong, or
ping; priority is close, pong, ping, then application FIFO. Ping is suppressed
unless both slots and the writer are idle, duplicate pong is rejected as
protocol failure, and close is best effort. Pairing/bootstrap use this same
writer exclusively before the application tasks start. Their existing
operation deadlines include waiting for writer ownership and complete write.
On shutdown, cancellation closes both mailboxes, the writer finishes or aborts
its current bounded write, releases the Noise state, and is joined before the
session task terminates.

On the simulator, request-mailbox failure records typed `queue_full`, completes
its local active core read exactly once as failure, closes the session, and then
enters reconnect backoff. On the host, result-mailbox failure records
`queue_full` and closes the session; the simulator converts that observed loss
into exactly one core failure before entering backoff. The host never claims to
complete a remote core effect. Control-slot occupancy and writer-I/O failures
retain their specific suppressed, timeout, cancellation, EOF, or peer-close
type rather than becoming application queue failures. Shutdown never depends
on sending a close frame. Recorder failure never delays any completion or
termination action.

## Pairing, authentication, and persistence

Each peer has one X25519 static identity. A connection uses
`Noise_XX_25519_ChaChaPoly_BLAKE2s`; the fixed Deskkin prelude is supplied as
the Noise prologue. XX exchanges static public keys and establishes encrypted
transport keys, while Deskkin pairing decides whether the learned key is
trusted.

Each CLI exposes exact `identity init`. Pairing and connect fail typed
`identity_missing` rather than generating a key. Explicit init against a valid
canonical identity returns typed `already_initialized` without changing it;
against the strict virgin state defined below it discards only a recognized
private regular temporary, generates one local static key, and publishes it.
This makes recovery from an init crash explicit and permits deliberate
reinitialization only after the canonical identity was externally removed; a
pinned remote will still reject the changed key until exact unpair. Init reports
success only after canonical rename and parent-directory sync. Restart reuses
the identity. Pair and unpair never rotate or remove it. Every fresh-state
scenario and loopback E2E explicitly initializes both peers before opening a
pairing window.

An unknown device is accepted only while the host owns an explicit single-use
pairing window whose 60-second deadline starts when open succeeds. The first
unknown peer to complete Noise and send valid `pairing_begin` before that
deadline atomically reserves the window while binding its transaction and
remote static key; concurrent unknown peers receive typed `pairing_busy`.
Prelude, Noise, or malformed-begin failure before reservation does not consume
the window. After reservation, the original deadline is also the local-confirmation
deadline and is never extended. Explicit reject, disconnect, or expiry consumes
the window and leaves the durable states dictated by the transition table; a
new attempt requires a new window. Once both decisions are confirmed before the
deadline, the window is consumed and an already-entered non-cancellable store
publication may finish after it.

Both CLIs display the same six-digit short authentication string and require
explicit local confirmation. The string is
the first four handshake-hash bytes interpreted as a big-endian unsigned
integer, reduced modulo 1,000,000, and zero-padded to six decimal digits. It is
never accepted as remote input and is not recorded. A rejection or expiry
persists no peer.

After both confirmed decisions, pairing follows only the canonical transition
table above. Pending or asymmetric state is visible through `list` on both
peers and grants no application session. A failed pairing step is never retried
and Phase 3 performs no reconnect/resume of a pairing transaction. Connection
loss, command exit, crash, or ambiguous final send terminates typed
`pairing_incomplete`. If either peer reports pending or committing, both peers
are listed: exact unpair is run on each side that retains that transaction/key,
while a side already listing unpaired is confirmed clean and receives no
fabricated mutation. A new window opens only after both list unpaired. If both
report paired, pinned reconnect is the defined safe convergence path even when
a prior CLI result was incomplete. There is no silent rollback, automatic trust
reset, or first-connection fallback.

Once both sides are paired, later XX handshakes must match the pinned key and
do not ask for confirmation. A changed key, missing identity, identity-store
corruption, symlink, ambiguous state, or connection outside the pairing window
fails closed. Replacement requires exact `unpair PEER-ID` control followed by
a new pairing window. `PEER-ID` is exactly the full 64-character lowercase hex
encoding of the 32-byte static public key; an abbreviated display value is
never accepted by a mutating command.

The persisted peer state includes a monotonically increasing `u64` generation;
each authenticated application session captures it. Exact unpair is a
two-publication transaction. First it increments generation, publishes and
parent-syncs `revoking { remote_public_key, previous_pairing_transaction_id }`;
that completed sync is the durable trust-revocation linearization point.
Overflow or failure before the revoking rename returns failure without session
cancellation. Immediately after successful rename, even before parent sync,
the current process invalidates the generation and cancels matching pairing and
authenticated session tasks. If parent sync then fails, it reports
`revoked_recovery_required`; the current namespace remains fail-closed although
restart may reveal the prior paired or new revoking state. Once revoking is
durable, the actor publishes and parent-syncs `unpaired` at the same generation.
A stale task's later publication fails typed `pairing_incomplete`; it cannot
recreate trust.

The local session cancels an active request and closes, and a local simulator
core completes its read once as failure or applies disconnect invalidation. A
remote core observes eventual authenticated connection loss and is not part of
the unpair success boundary. Every availability request rechecks live
generation before invoking the fake host port and again before writing a
result. Every pairing publication compares expected generation, transaction,
remote key, and state.

After revoking rename, matching tasks get two seconds to join, then are
force-aborted and awaited. The terminal result has closed task termination
`joined | forced`. If final unpaired publication or sync fails, the result is
typed `revoked_recovery_required`; after restart the only possible canonical
states are durable revoking or unpaired, neither of which grants trust. Exact
unpair of the retained peer ID resumes revoking-to-unpaired publication. No
old-generation result is accepted after the revoking linearization point.

Default identity roots are exactly `.deskkin/phase3/host/identity/` and
`.deskkin/phase3/device-simulator/identity/`; tests and explicit disposable
runs must provide an isolated `--state-root`. Roots and files use modes 0700
and 0600, and existing non-private roots are rejected without chmod. The only
allowed root entries are the never-renamed `.identity.lock`, the fixed
`.identity.tmp`, and canonical `identity-v1.json`; symlinks including broken
links and every unknown entry fail closed.

When an allowed root path is absent, init creates exactly one path component at
a time with mode 0700, opens and syncs the newly created directory, then opens
and syncs its existing parent before creating the next component. Every
existing component is revalidated as a private real directory without following
symlinks. Init cannot report success until creation of `.deskkin`, `phase3`, the
role directory, and `identity` is durable through each parent boundary as well
as the canonical-file transaction below. Tests inject crashes after every
mkdir, child sync, and parent sync.

Canonical JSON has closed schema major 1 and exactly: the 32-byte local private
and public keys as 64 lowercase hex characters, the `u64` generation, and one
closed peer value of `unpaired`, `pending { remote_public_key,
pairing_transaction_id }`, `committing { remote_public_key,
pairing_transaction_id }`, `paired { remote_public_key,
pairing_transaction_id }`, or `revoking { remote_public_key,
previous_pairing_transaction_id }`. Only paired grants an application session.
A strict virgin store is an absent root or a private root containing only
`.identity.lock` and optional private regular `.identity.tmp`, with no canonical
file. Only `identity init` may accept that state and remove the recognized
temporary; all other operations report missing identity. Canonical validation
derives and constant-time compares the public key from the private key and
rejects generation overflow or malformed peer fields.
Once canonical state exists, a leftover private regular non-symlink temporary
represents an unpublished transaction and is removed under lock after canonical
validation. A non-private or non-regular temporary is never deleted. Thus
recognized crash residue is bounded to one file.

Each process has one dedicated blocking identity-store actor on a standard
thread with a one-request bounded mailbox; async runtime tasks never perform
filesystem or locking work. Before touching `.identity.tmp`, the actor uses
`File::try_lock()` with ten-millisecond bounded polling for at most two seconds,
checking cancellation between attempts. Cancellation or lock timeout in this
pre-publication region returns without mutation.

After lock acquisition, the actor revalidates root and entries, rereads current
state, and compares the expected transition. Immediately before creating or
truncating `.identity.tmp` it enters a non-cancellable publication region:
write and sync the temporary, rename it atomically to `identity-v1.json`, sync
the parent directory, and unlock. The requesting command remains active and
joins the actor result even after its network/confirmation deadline or shutdown
is requested, so no background mutation can publish after the command has
returned. A publication error returns its exact stage and actual durable state.

Linux does not provide a safe hard-cancel guarantee for a filesystem sync
already in progress. The actor emits `store_stalled` diagnostic health after
two seconds, while protocol I/O and presentation remain responsive on the async
thread, but the mutating control result remains pending until the syscall
returns or the whole process is externally terminated. The 60-second pairing
window bounds confirmation and pre-publication work; it does not falsely claim
to bound an entered filesystem publication. This residual local-filesystem
limit is part of the approved contract and is fault-tested with a controllable
store port rather than by abandoning an in-flight actor.

Raw private keys are never shown, logged, placed in diagnostics or results, or
committed. Deskkin-owned buffers used to read the persisted private key are
zeroized on drop; copies inside Snow, the compiler, swap, core dumps, or kernel
buffers are explicitly outside this guarantee. Public peer identity is an
operational identifier and appears only where exact list/unpair control
requires it.

This pairing proves the hosted loopback implementation only. It does not claim
physical-device key storage, recovery, hardware-backed identity, network
discovery, certificate interoperability, or resistance to an attacker who
already controls the user's local account.

## Result and control surfaces

The desktop-host control surface is limited to:

- explicitly initialize a missing local identity;
- start a loopback host with an explicit address;
- open one bounded pairing window;
- list exact pending, committing, paired, or revoking peer identities;
- unpair one exact pending, committing, paired, or revoking peer identity;
- request graceful shutdown.

The simulator can pair, list and exactly unpair pending, committing, paired, or
revoking host identities, explicitly initialize a missing local identity,
connect with a pinned identity, run one status session, and disconnect.
Automated scenarios publish one atomic result JSON per scenario.
Stable result fields contain the scenario result, scenario run ID, result path,
selected protocol/features, granted permissions, view sequence, and semantic
timestamps. Ephemeral ports, cryptographic keys, ciphertext, wall-clock values,
and process identity are excluded from replay comparison.

Pairing confirmation is an operation input, not an environment-variable bypass
or a stored universal approval. Test drivers inject a typed confirmation port;
production CLI input and tests exercise the same state transition.

Each long-running host or simulator process exclusively locks
`.deskkin/phase3/<role>/control/owner.lock` and owns its session/pairing registry.
It listens on the Unix socket
`.deskkin/phase3/<role>/control/owner.sock` inside that mode-0700 directory; the
socket is mode 0600, accepts only four-byte big-endian length-prefixed,
closed-enum JSON control requests and responses of at most 4 KiB, and has no
remote transport. Closed requests are host `pairing_window_open`, simulator
`pair_start { loopback_address }`, `pairing_decide { parent_command_id,
pairing_transaction_id, confirmed | rejected }`, identity init, exact unpair,
shutdown, `owner_info`, and command-result query. Every mutating request carries
an opaque 128-bit command ID and the owner generation obtained before mutation
from one-shot `owner_info`; stale or mismatched generation is rejected before
acceptance. Decision and result query carry the same generation and parent ID.
There is no pairing-recovery command; pending or ambiguous state uses exact
unpair on both peers followed by a fresh window. A random per-start owner
generation is echoed in each response to reject a stale socket. The threat
model already excludes an attacker controlling the same local account;
directory and socket modes prevent access by other accounts.

The owner listener has exactly four connection permits. Each connection serves
exactly one request and one response, then closes. Prefix read, complete payload
read, and complete response write each have a two-second deadline; timeout or
malformed input closes only that connection and releases its permit. An
accepted mutation returns its accepted state promptly, and the CLI polls with
one-shot result queries rather than holding a socket through store publication.
Owner shutdown stops accept, cancels all partial control I/O, and joins every
connection task before socket removal.

An accepted host window or simulator pair command owns the operation through
its terminal result. Its query state is a closed enum: `waiting_for_peer`,
`waiting_for_local_decision { transaction_id, authentication_string }`,
`publishing`, or terminal result. The authentication string exists only in
owner memory and the private control response and is not persisted or recorded.
`pairing_decide` must match the owner generation, parent command, current
transaction, and waiting state; exactly one decision is accepted. CLI
disconnect does not decide or cancel pairing. Requery restores the waiting
display, while the original pairing-window deadline continues and produces
typed expired if no decision arrives. Decision and query requests are correlated
inputs to an existing command, do not allocate command-table slots, and remain
available when the table is otherwise full. Shutdown also has a reserved
slot-free path: duplicate shutdown requests coalesce idempotently and are
accepted even when all command records are occupied.

A mutating CLI first obtains owner info. If no owner is live, it must obtain
`owner.lock` exclusively before direct store mutation. If the lock is
held but connection and acceptance acknowledgement do not complete within two
seconds, the command returns typed `owner_busy`; it never falls back to direct
mutation. Once accepted, operation ownership transfers to the runtime and the
CLI applies no operation-completion timeout while that owner remains live. It
polls the terminal state with the same command ID and owner generation, so loss
of the acceptance response cannot lose correlation. The owner keeps at most 16 command
records. A nonterminal accepted
operation is never evicted; a terminal record is removed exactly ten minutes
after completion, freeing its slot. A new command is rejected before acceptance
when the table is full. A duplicate retained ID returns the existing state and
never repeats mutation; a query after expiry returns typed `unknown_command`.
After expiry, reuse of an old ID in a new mutating request is caller error and
is treated as a new command because the owner retains no unbounded tombstone.
Accepted publication continues if the CLI process exits, but no CLI failure
result has been returned; a later query observes the terminal result.

After acceptance, connection refusal plus observed owner-lock release, or an
`owner_info` response with a different generation, terminates the CLI as typed
`owner_lost_result_unknown`. The CLI never resubmits the mutation implicitly.
The user inspects the canonical list and uses explicit init, exact unpair, or a
new pairing window appropriate to the durable state as a separate command.
Temporary or publication recovery remains owned by the new process's store
actor.

The owner routes pair and unpair through its registry, so exact unpair cancels
and joins matching tasks before replying. Shutdown is the sole mutating-command
lifecycle exception: successful complete write of `shutdown_accepted` is its
terminal CLI result, and it allocates no command record or later result query.
The shutdown handler first completes that response on its existing socket,
transfers ownership to a coordinator, and excludes itself from the cancellation set. The
coordinator then closes the listener, cancels every other cancellable command
and session/pairing/control task, and joins them before joining the already
completed handler. It must still join an identity actor already inside
the explicitly non-cancellable filesystem publication region, so the documented
sync-stall residual applies to shutdown completion but never to shutdown
admission. Shutdown removes the socket only while retaining the lock, then
releases the lock last. Startup rejects a symlink or non-socket at the exact
socket path and safely removes a stale socket only after acquiring the owner
lock. Read-only list takes `.identity.lock` and does not require owner routing.

## Observation contract

The network, cryptographic handshake, persistence, asynchronous I/O, timeout,
retry, and cross-process context make this path subject to the program
observation contract.

Diagnostic runs are:

- `protocol.pairing`: pairing window through confirmed/rejected/expired store
  publication;
- `identity.control`: init or exact unpair through store publication, active
  session revocation, and atomic control-result publication;
- `protocol.session`: TCP connect/accept through authenticated close,
  cancellation, timeout, or crash;
- `availability.read`: request through correlated result or terminal failure,
  linked to its session.

The connection initiator obtains each opaque 128-bit session context identity
from the OS cryptographic random source; an in-process collision is regenerated
before use. Tests inject a deterministic source. It is carried inside the
encrypted hello and echoed by the host. Each read obtains a separate opaque
128-bit operation context identity from the same source and carries and echoes
it with the request. Both processes record these operational identifiers, so
separate private recorder roots can correlate one session and operation without
exporting diagnostics or recording keys, addresses, process identity, or
payloads.

The initiator's pairing transaction identity is also the cross-process pairing
context identity. It is exchanged in `pairing_begin` and included in every
pairing control message and both recorders. Each process starts local
accept/connect and handshake operations under its own diagnostic run ID; once
the encrypted pairing transaction or session context becomes known, the run
emits a typed link to that shared context. A pre-authentication failure has only
its local parentage and never invents a remote correlation.

When session loss occurs without an active read, the session event, typed core
invalidation, and resulting `presenter.apply_view` operation remain linked to
the closing session context. When exact unpair revokes a session, the
`identity.control` run links to that same session and publishes its control
result only after the revocation sequence terminates. Recording-off and
recorder-failure tests preserve both semantic sequences.

The minimum operation allowlist is:

- `transport.connect`, `transport.accept`, `transport.frame_read`, and
  `transport.frame_write`;
- `noise.handshake`;
- `pairing.confirm` and `pairing.persist`;
- `protocol.negotiate`;
- `availability.read` and `presenter.apply_view`;
- `control.route`;
- `identity.init`;
- `identity.unpair`.

Each operation has start/end time, status, parent or link, and a closed error
type. Error types distinguish DNS-not-applicable invalid address, connection
refused, EOF, frame oversize, malformed frame, handshake timeout, handshake
authentication failure, pairing closed, pairing rejected, pairing expired,
pinned-key mismatch, identity missing, identity-store failure and stage,
generation exhausted, store lock timeout, store stalled, owner busy, version
mismatch, required feature unsupported, permission denied, pre-auth capacity,
pairing busy, session busy, owner lost/result unknown, revocation recovery
required, queue full, request timeout, cancellation, and crash.

Resource fields are program name, version, role (`host | device_simulator`),
protocol major, and recording health. Allowlisted dynamic fields are diagnostic
run/operation/link IDs, pairing transaction/context ID, session and operation
context IDs, request ID, message kind, frame byte count, duration, retry count,
selected protocol, required/optional/selected feature bits, requested/granted
permission bits, and completeness. Socket address, raw peer key, short authentication string,
handshake bytes, ciphertext, decoded payload, environment, path, hostname,
username, raw PID, and provider data are not recorded.

The reusable local recorder remains default-on, opt-out, remote-off, bounded,
private, crash-recoverable, and non-interfering. Phase 3 uses exact separate
roots `.deskkin/phase3/host/diagnostics/` and
`.deskkin/phase3/device-simulator/diagnostics/`. Each root independently
retains ten successful and twenty non-success runs plus explicit pins under a
16 MiB cap, so their aggregate maximum is 32 MiB without cross-process parent
locking. Each has its own never-renamed lock and offers list, retain, unretain,
and exact delete. Scenario stdout/result meaning, protocol outcome, pairing
state, and availability must be identical with recording on or off and under
recorder failure.

## Proposed dependencies

No dependency is added until this proposal is explicitly approved. The exact
Phase 3 direct set proposed on 2026-08-23 is:

| Dependency | Exact version and features | Purpose and boundary |
| --- | --- | --- |
| `serde` | `=1.0.229`, default off, `derive` | fixed protocol types; already locked for Phase 2 but this is a new portable use |
| `serde_json` | `=1.0.151` | hosted identity, owner-control, result, and diagnostic JSON; never used by portable crates |
| `postcard` | `=1.1.3`, default off | bounded `no_std` serialization into caller-provided slices |
| `tokio` | `=1.53.1`, default off, `io-util`, `macros`, `net`, `rt`, `sync`, `time`; `test-util` additionally in dev/test targets only | hosted single-thread session runtime, bounded channels, I/O and virtual-time tests |
| `snow` | `=0.10.0`, default off, `default-resolver`, `use-blake2`, `use-chacha20poly1305`, `use-curve25519`, `use-getrandom` | hosted Noise XX handshake and transport state; no Snow type crosses the protocol crate |
| `zeroize` | `=1.9.0`, default off, `alloc` | clear owned hosted private-key buffers on drop |

A repository-external temporary compile pilot on Rust 1.95.0 confirmed this
exact resolution, the `no_std` postcard slice codec, the current-thread Tokio
runtime, Snow key generation with the proposed algorithm set, and zeroizing
ownership. The pilot and its lockfile were removed afterward; it did not alter
the Deskkin workspace or approve adoption.

Adoption also accepts these maintenance, security, and licensing boundaries:

| Component group | Ownership and update gate | Distribution impact |
| --- | --- | --- |
| `serde`, derive macros, and `serde_json` | stable wire and hosted JSON schema dependencies; exact pin changes require explicit upgrade review, and derive executes build-time procedural macros | MIT OR Apache-2.0; no new copyleft term |
| `postcard` and `cobs` | enum order and canonical vectors become wire obligations; upgrades rerun the full codec conformance suite | MIT OR Apache-2.0 |
| `tokio`, `mio`, `socket2`, and `bytes` | hosted scheduling and I/O only; selected features exclude filesystem, process, signal, and multithread runtime support; upgrades rerun timeout, cancellation, and backpressure conformance | permissive MIT or MIT OR Apache-2.0 |
| `snow`, RustCrypto primitives, `curve25519-dalek`, `subtle`, and `getrandom` | cryptography is delegated rather than reimplemented; the compile pilot is not a security audit and no independent Snow audit is asserted; implementation and every upgrade review current RustSec and upstream advisories | permissive MIT, Apache-2.0, or BSD-3-Clause |
| `zeroize` | best-effort clearing of owned buffers only; it does not promise removal of compiler copies, swap, core dumps, or kernel buffers | MIT OR Apache-2.0 |

Maintainers own the exact direct pins and the complete root-lockfile resolution.
An upgrade does not implicitly authorize a wire, Noise pattern, feature, MSRV,
or license change. The implementation checkpoint records resolved licenses and
the advisory review. Deskkin source remains MIT; the simulator binary containing
Slint remains GPLv3 under the existing policy. Phase 3 still performs no binary
distribution, and non-loopback or release use requires a separate security and
distribution review.

`serde_json = 1.0.151` remains hosted-only but expands from Phase 2
result/diagnostic JSON to the identity and owner-control schemas above. The
implementation checkpoint will record the approved complete resolution in the
root lockfile. The current Phase 2 lockfile already contains `serde 1.0.229`,
`bytes 1.12.1`, and `getrandom 0.3.4` and `0.4.3` through existing hosted
dependencies; this does not approve their new Phase 3 roles. `postcard`,
`tokio`, `snow`, and `zeroize` are new proposed direct packages, while the
approved implementation resolution determines which already-present transitive
versions can be reused. No TLS, certificate, protobuf, WebSocket, QUIC, OTel
SDK, database, keyring, discovery, CLI framework, or logging dependency is
proposed.

Alternatives considered:

- loopback plus unauthenticated plaintext is smaller but does not validate the
  accepted paired-peer and session boundary;
- a custom HMAC or key-exchange protocol would reduce dependency count while
  making Deskkin own cryptographic protocol design;
- TLS provides a mature secure channel but requires certificate issuance,
  trust-store, and constrained-device compatibility decisions not otherwise
  needed by this slice;
- JSON is easy to inspect but brings text, allocation, and weak bounded-frame
  incentives into a future device boundary;
- manual threads and nonblocking sockets avoid Tokio but add scheduling,
  timeout, cancellation, and test-runtime machinery that Tokio already owns.

The proposed libraries are adopted components, not proof that a compatible
physical-device implementation exists. A later device checkpoint must select
and verify a Noise-compatible Zephyr/no_std implementation or revise the
transport through a superseding ADR before device code is added.

## Approval and ADR checkpoint

Because this slice establishes a durable cross-cutting wire, authentication,
pairing, persistence, and compatibility contract, a conversational approval is
not itself permission to begin code. The first post-approval checkpoint adds a
new immutable ADR containing the accepted contract and approval date, records
the proposal as approved without rewriting its history, synchronizes
`docs/architecture.md`, this implementation plan, and the open-decision list,
and commits only those documentation changes locally. Implementation and
dependency adoption begin only after that documentation checkpoint is complete.
Any later incompatible security or wire decision requires a superseding ADR.

## Test and acceptance plan

The implementation checkpoint passes only when:

1. protocol tests cover canonical bootstrap, pairing, and application-v1
   vectors, including matching request and operation context identities, exact
   decode, every size boundary, unknown kind, malformed length and payload, and
   allocation bounds;
2. pairing tests fault-inject before and after every prepare, commit, and
   publication boundary; cover matching confirmation, reject, expiry,
   asymmetric/pending recovery, pinned reconnect, changed-key rejection,
   competing pair/unpair processes including stale-task resurrection, exact
   list/unpair on both peers, corrupt and unknown-entry state, strict-virgin
   identity init and crash resume, key reuse, forbidden implicit rotation,
   private modes, symlink rejection, and restart after every temporary, rename,
   directory creation, child/parent sync, canonical publication boundary, and
   post-pair hello ack/reject outcome;
3. deterministic in-memory scenarios run twice from fresh state and compare
   semantic records, selected protocol/features, granted permissions, view sequence, virtual
   timestamps, and RGB565 frames byte-identically; adapter scenarios include
   wire unavailable to `StatusView::Unavailable` and wire read failed to
   `StatusView::Unknown` followed by next-cycle available recovery;
4. the accepted scenario reaches `Unknown -> Available`; disconnect both with
   and without an active read reaches `Available -> Unknown -> Available`
   without re-pairing, and stale closed-session frames cannot complete an
   effect;
5. real loopback TCP E2E exercises the same framing, Noise, negotiation, and
   availability adapters as the deterministic driver;
6. version mismatch, required-feature mismatch, optional-feature intersection,
   permission denial, oversize/malformed frame, pinned-key mismatch, saturation of
   both application-queue directions, prioritized control-message timeout,
   request timeout, cancel, clean shutdown, and crash have distinct results and
   cross-process correlated diagnostic runs;
7. recording on/off and injected recorder/storage failure preserve result,
   pairing, protocol, and frame semantics; retention and privacy fixtures pass;
8. `cargo tree` confirms `application-core` remains dependency-free and the
   protocol crate contains no hosted/runtime/UI dependency;
9. target-limited `mise run fix`, final `mise run test`, and a fresh durable
   subagent review pass;
10. no non-loopback listener, live provider access, device build, package,
    release, push, or artifact publication occurs.

The suite also races exact unpair against an active request and an idle
authenticated session, proving the store-generation linearization, single core
failure/invalidation, stale pairing/result rejection, normal and forced local
task termination, session termination, and `identity.control`
result/diagnostic ordering. The same races run through a separate CLI process,
proving owner-channel routing, owner-busy refusal, stale-socket recovery, and
that no standalone mutation bypasses a live registry. Control tests also cover
acceptance followed by slow publication, dropped CLI connection, idempotent
result query, pairing decision after requery, duplicate command/decision ID,
bounded-table refusal, terminal-result expiry, four stalled partial control
connections with bounded cleanup, and slot-free idempotent shutdown while the
command table is full. They also cover acceptance-response loss after mutation,
stale owner generation, owner crash before and after each publication boundary,
typed result-unknown without implicit replay, and shutdown response delivery
without self-cancel/join. Pairing tests cover window
open/expiry boundary timestamps, pre-reservation handshake failure, atomic
single-use reservation under simultaneous unknown peers, pre-auth capacity
saturation, rejection consumption, and no deadline extension. Session tests
race the same pinned identity through two hellos and prove exactly one ack plus
one session-busy reject and complete task-set join. Writer tests prove that
application and prioritized control frames never interleave ciphertext or
reuse/reorder a Noise nonce; queue tests assert the distinct simulator-request
and host-result failure ordering.
Virtual-time tests separately exercise connect, Noise handshake, pinned
bootstrap, pairing window, partial-frame read, complete-frame write,
each pairing-control step, availability-result, and idle deadlines, including
ordinary five-second refresh silence and every reconnect-FSM
transition/reset/cancellation. Store
actor tests cover mailbox saturation, cancellable lock timeout, cancellation
before publication, non-cancellable join after publication starts, every
publication-stage error, durable revoking linearization, restart to either
revoking or unpaired after final-sync failure, exact revoking recovery, stalled
health after each publication stage, and no post-return mutation.

Before item 1 begins, the approved ADR/documentation checkpoint above must be
complete as a separate local commit.

Any live local pairing demonstration is a separate explicit launch checkpoint.
It must use disposable Phase 3 state and must not replace the reproducible
acceptance suite.

## Approval record

On 2026-08-24 the user explicitly approved all of:

1. the loopback-only TCP, bounded framing, protocol-v1, request/reply, timeout,
   retry, and backpressure contract;
2. Noise XX pairing, six-digit local confirmation, exact pinned-peer state, and
   fail-closed unpair/re-pair policy;
3. the workspace changes, exact six direct dependencies, transitive component
   groups, and maintenance, security-review, licensing, and distribution impact
   above;
4. the default-on bounded diagnostic contract and conformance plan;
5. the explicit exclusion of protocol LAN exposure, physical-device support,
   Unraid, mutation, packaging, release, and publication.

The approval authorizes the ADR/documentation checkpoint and, only after that
separate local commit is complete, source implementation and local reproducible
verification. It does not authorize a live provider, non-loopback listener,
physical device operation, system installation, push, release, or artifact
publication.
