# ADR-0005: Extend the paired availability transport to CoreS3

- Status: Accepted
- Date: 2026-08-24

## Context

Phase 3 proves the authenticated availability path between a Linux host and a
hosted simulator over loopback. The next useful risk is the same path on the
M5Stack CoreS3, before adding an Unraid connector. That device cannot use the
loopback-only transport consequence of
[ADR-0004](0004-paired-host-protocol.md), but its established wire, Noise,
pairing, permission, timeout, and availability contracts remain suitable.

The complete approved physical-slice contract is recorded in
[`phase-3p-physical-slice-proposal.md`](../phase-3p-physical-slice-proposal.md).

## Decision

Add a separate Linux host mode that binds one exact, assigned RFC1918 IPv4
address on fixed TCP port `39042`. The CoreS3 initiates the connection over a
personally managed LAN. Wildcard, public, link-local, IPv6, and unassigned
binds are rejected. The existing host mode remains loopback-only; neither mode
changes firewall or network configuration.

Keep protocol major 1, bootstrap schema 1, the six-byte prelude, Noise pattern
`Noise_XX_25519_ChaChaPoly_BLAKE2s`, canonical wire bytes, capability and
permission negotiation, pairing publication, exact unpair, and availability
semantics unchanged from ADR-0004. Extract the hosted client's reconnect,
session, and request-correlation state into a reusable `no_std` crate that
depends only on the application core and protocol crate.

The CoreS3 has one Rust/Embassy UI owner and one dedicated Rust service worker.
The worker alone owns Wi-Fi, TCP, Noise send state, session writing, and
identity/configuration mutation. A bounded C adapter hides Zephyr APIs for
Wi-Fi, DHCP, sockets, NVS, cryptographic randomness, USB serial, display, and
touch. The device uses a custom Snow resolver backed by Zephyr's CSPRNG; failure
to prove `no_std` Xtensa interoperability is a hard stop.

Persist one X25519 identity and one WPA2-Personal 2.4 GHz profile in separate
Zephyr NVS namespaces. Fixed binary records use alternating slots, schema and
state validation, publication sequence, generation, bounded length, and CRC.
Only a successfully written and read-back record becomes canonical. Ambiguous,
corrupt, unknown, or equally ranked conflicting state fails closed. Identity
creation, provisioning, exact unpair, and recovery are explicit bounded USB
control commands; pairing and run never create or replace them implicitly.

The NVS storage is not encrypted. The Wi-Fi password and Noise private identity
therefore remain recoverable from flash. This slice does not enable flash
encryption, secure boot, eFuse mutation, or forensic erasure. Logical clear is
not claimed to erase prior NVS cells.

Support an experiment profile stored only as an age-encrypted ignored file.
Profile creation prompts without echo and encrypts without a plaintext file;
provisioning decrypts through pipes and never places credentials in arguments,
environment, result output, diagnostics, or committed artifacts. The normal
hidden-prompt path remains available.

Record bounded, local, non-interfering host-side diagnostics from allowlisted
USB events. Authentication strings, credentials, keys, addresses, protocol
bytes, payloads, paths, and machine or user identity are forbidden. A Slint
CoreS3 firmware binary is treated as GPL-3.0-only while Deskkin-authored source
remains MIT. No binary is distributed by this checkpoint.

## Consequences

- ADR-0004 remains normative except for its loopback-only transport
  consequence, which this ADR partially supersedes for the explicit physical
  mode.
- A LAN observer can attack availability and transport, but cannot silently
  replace a pinned peer without failing Noise authentication.
- The device contains plaintext Wi-Fi and private identity material until an
  independently approved storage-hardening slice.
- There is no discovery, hostname, IPv6, multiple profile, multiple device,
  WPA3, daemon, autostart, packaging, or provider behavior.
- Flashing, real provisioning, storage mutation, and power cycling require a
  separate live approval immediately before execution.

## Alternatives

### USB-only application transport

USB avoids LAN exposure but does not prove the deployment shape needed for an
independent companion.

### Discovery or wildcard binding

Discovery and wildcard listeners improve convenience but materially broaden
the network and identity contract before the exact-address slice is qualified.

### Flash encryption in this slice

It would improve at-rest protection but adds boot, key, eFuse, recovery, and
irreversible device-lifecycle decisions that require their own checkpoint.
