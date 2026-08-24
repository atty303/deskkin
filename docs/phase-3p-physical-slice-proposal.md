# Phase 3P proposal: CoreS3 paired availability demo

Status: Approved  
Date: 2026-08-24

The user approved this durable physical slice before Phase 4. Approval covers
the ordered documentation and implementation checkpoints plus reproducible
local verification. Live flashing, real Wi-Fi provisioning, device storage
mutation, and power cycling remain separate execution approvals. It does not
authorize provider access, firewall changes, distribution, release, push, or
publication.

## Goal and observable result

Run the Phase 3 paired availability contract between one Linux desktop host and
the current M5Stack CoreS3 on a personally managed LAN. From explicitly
initialized identities, both peers show the same locally derived six-digit
authentication string and require local confirmation. A paired device then
reconnects without confirmation, maps host `available`, `unavailable`, and
`read_failed` to the shared status surface, immediately invalidates a lost
session to `Unknown`, and recovers on a later authenticated read.

Phase 3P changes transport placement, device persistence, and runtime adapters.
It does not change application-core behavior, protocol-major-1 bytes,
bootstrap schema 1, Noise algorithms, pairing publication, permission policy,
or exact unpair semantics.

## Scope and transport

The physical path is device-initiated TCP to one exact RFC1918 IPv4 address on
port `39042`. The device uses one WPA2-Personal 2.4 GHz profile and DHCPv4. The
host's new `run-private-lan` command accepts only an exact local RFC1918 address
that the kernel can bind. It rejects wildcard, public, link-local, IPv6, and
unassigned addresses. The existing `run` command remains loopback-only.

Deskkin does not discover the host, modify a firewall, configure an interface,
install a service, or accept a hostname in this slice. mDNS, IPv6, WPA3, open
networks, multiple profiles, and multiple devices remain out of scope.

## Shared client and UI

Move the simulator-private connection adapter to a dependency-bounded `no_std`
crate. It owns `ConnectionState`, `TerminalReason`, and `ProtocolAdapter`, an
authenticated session context, monotonic request identities, operation-context
matching, disconnect invalidation, terminal rejection, and reconnect delays of
250, 500, 1000, 2000, then 5000 milliseconds. Only a valid availability result
resets backoff. The crate depends only on `application-core` and
`deskkin-protocol`; wire bytes remain owned by the latter.

The simulator continues to use this state machine and its existing shared
Slint status surface. The CoreS3 shell adds `SetupRequired`, `ReadyToPair` with
an explicit Pair action, `Connecting`, a six-digit confirmation with Confirm
and Cancel, and paired status states. The host opens its existing explicit,
single-use 60-second pairing window and displays the string through its private
owner-control response and CLI. On the device, the string is delivered only to
the display owner. It never enters the application view model, serial output,
diagnostics, or persistent state.

## Device runtime

The independent CoreS3 Zephyr application has two owners: a Rust/Embassy UI
owner for the portable core, Slint, timers, touch, and typed worker messages;
and one Rust service worker for Wi-Fi, TCP, Snow state, session writes, and
identity/configuration mutation. The C boundary exposes only bounded Wi-Fi,
DHCP, blocking socket, NVS, `sys_csrand_get`, USB serial, display, and touch
operations. Unsafe Rust and Zephyr types do not escape the adapter.

The application queue has capacity 4, reserved control capacity 1, and
completion capacity 8. The worker alone advances the Noise send nonce and
writes TCP, prioritizing close, pong, ping, and application FIFO. Phase 3
timeouts remain: TCP and partial I/O 2 seconds, Noise/bootstrap 5 seconds,
availability 2 seconds, pairing control 5 seconds, pairing window 60 seconds,
and idle 30 seconds. Wi-Fi association adds 15 seconds and DHCPv4 10 seconds.

Device Snow is exactly `0.10.0`, default-off, with the approved Curve25519,
ChaCha20-Poly1305, BLAKE2, and default resolver but without `getrandom`. A
custom resolver obtains randomness from Zephyr's cryptographic random source.
A clean `no_std` Xtensa build and canonical host interoperability are hard
acceptance gates; failure permits no algorithm or protocol substitution.

## Persistent identity and configuration

Identity and device configuration occupy separate NVS namespaces. Each uses
two alternating fixed-binary slots containing schema, publication sequence,
generation, closed state, bounded payload length, and CRC. A publication is
canonical only after write and readback succeed. Startup selects the greatest
valid publication sequence; equal-ranked disagreement, CRC failure, unknown
schema or state, and malformed length fail closed.

`identity-init` alone generates an identity in a strict virgin store. Peer
states remain `unpaired`, `pending`, `committing`, `paired`, and `revoking`;
only `paired` permits a session. Exact unpair names the full 64-character peer
identity, publishes `revoking`, invalidates the old generation/session, joins
matching tasks, then publishes `unpaired` at that generation. Recovery never
resurrects an old session.

The single configuration contains a 1-32 byte SSID, 8-63 byte printable ASCII
WPA2 passphrase, exact RFC1918 host address, and fixed port. Storage is
plaintext NVS. Clear is logical deletion; only explicit
`recover --erase-storage` erases the partition and restores inert firmware.

USB control uses a bounded schema-1 binary frame with command identity, owner
generation, and payload length. It never echoes credentials. Commands are
limited to identity init/list/unpair, Wi-Fi provision/status/clear, and
run/status/shutdown. Run never flashes, generates identity, or changes
credentials implicitly.

## Experimental age profile

Pin age 1.3.1. The ignored default ciphertext is
`.deskkin/phase3-device/wifi.age`; the default identity is
`~/.config/chezmoi/age/identity.txt`. Both paths are overridable. Decrypted JSON
has exactly these fields and rejects unknown, missing, wrongly typed, or
out-of-range values:

```json
{
  "schema_version": 1,
  "ssid": "...",
  "password": "...",
  "host_ipv4": "192.168.x.x"
}
```

Profile creation reads hidden prompts, passes plaintext only through pipes,
writes ciphertext to a mode-0600 temporary, syncs, and atomically replaces it.
Provisioning decrypts an existing default or explicit profile through a pipe;
only an absent default falls back to hidden prompts. Explicit decrypt or schema
failure stops before mutation. Credentials never enter arguments, environment,
outputs, diagnostics, results, or fixtures; owned buffers are zeroized after
bounded serial transfer.

The task surface is `phase3:device:profile`, `phase3:device:build`,
`phase3:device:flash`, `phase3:device:identity`,
`phase3:device:provision`, `phase3:device:run`, and
`phase3:device:recover -- --erase-storage`.

## Diagnostics and licensing

The USB host runner records only allowlisted device events. Runs are
`physical.provision`, `protocol.pairing`, `protocol.session`,
`availability.read`, `identity.control`, and `device.ui`; operations distinguish
Wi-Fi, DHCP, TCP, Noise, bootstrap, NVS publication, view application, and
display transfer. The closed errors are `profile_decrypt_failed`,
`profile_schema_invalid`, `identity_store`, `wifi_association`,
`dhcp_timeout`, `tcp_connect`, `noise`, `pairing_rejected`,
`pairing_expired`, `pairing_busy`, `availability_timeout`, and
`recording_degraded`.

Only firmware/build identity, closed state, duration, opaque context IDs, view,
render size, and RGB565 digest are recordable. Keys, authentication strings,
SSID, password, address, wire bytes, payload, path, host/user/process identity
are forbidden. Host and device roots are each 16 MiB, retain 10 success and 20
non-success runs plus explicit retains, and have a 32 MiB aggregate maximum.
Recording degradation cannot alter semantics. The runner atomically publishes
results and prints one line with only `result`, `run_id`, and `result_path`.

Deskkin-authored source remains MIT. Slint-bearing CoreS3 firmware is a
GPL-3.0-only combined binary and is not distributed. Direct device dependencies
are limited to approved exact Slint 1.17.1, Snow 0.10.0, zeroize 1.9.0,
critical-section 1.2.0, embassy-executor 0.7.0, embassy-time 0.4.0, static_cell
2.1.1, zephyr 0.1.0, and repository path crates.

## Acceptance and live boundary

Reproducible acceptance covers shared-FSM simulator/device fakes, canonical
protocol/Noise interoperability, clean Xtensa builds, NVS fault injection,
generation/session rejection, queues/timeouts/reconnect, secret-free age round
trips, diagnostic equivalence, dependency boundaries, and `mise run test`.

Physical qualification requires immediate approval after displaying the exact
serial device, host address, age profile path, and retention choice. Only then
may the runner flash, provision, mutate storage, pair, interrupt/recover the
host, power-cycle the CoreS3, and exercise all availability mappings. The
selected final state retains demo firmware, Wi-Fi credential, and Noise
identity in plaintext flash. Cleanup is never automatic.
