# Deskkin architecture

## Product boundary

Deskkin is a platform for embodied desktop companions. The first device is
StackChan on M5Stack CoreS3, but device-specific code remains below portable
application and protocol boundaries.

The desktop host is the external-authority boundary. It owns provider
credentials, connector state, authorization policy, desktop access, and broad
network access. A companion device holds only its paired identity, the network
configuration needed to reach its host, and capabilities granted for the
current session. Provider APIs and credentials never enter device firmware.

```text
external services
       |
       v
desktop host  <---- authenticated semantic protocol ---->  companion device
connectors / policy                                      application / Slint
credentials / state                                     Zephyr / Embassy
```

The current implementation has no external provider connector. Availability
is served by a deterministic host connector.

## Dependency direction

```text
application-core
       ^
       |
application-features
       ^
       |
deskkin-application <---- platform presenters and effect executors

deskkin-presentation <---- simulator and device UI owners

deskkin-protocol <---- deskkin-protocol-client <---- host/device adapters

deskkin-host-capabilities <---- desktop-host adapter <---- future connectors
```

Portable crates do not depend on Slint, Zephyr, Embassy, desktop APIs,
filesystems, sockets, provider types, or runtime executors.

## Portable application

`application-core` is allocation-free `no_std` Rust. It owns only
feature-neutral lifecycle values, local effect identities, and surface-class
vocabulary.

`application-features` depends only on `application-core`. It contains the
availability state machine and a bounded synthetic-notice feature used for
composition conformance. Features own typed state, inputs, semantic surfaces,
and effect requests. They do not call one another or access UI, transport,
connectors, or platform APIs.

`deskkin-application` is the sole concrete composition root. It owns the closed
compile-time feature registry, registry-order lifecycle broadcast, namespaced
effect identities, exact completion routing, and deterministic surface
selection. `Information` surfaces precede `Ambient`; equal classes use fixed
registry order. Rejected input, capacity exhaustion, and identity exhaustion
are transactional and publish no partial state or effect.

The same composition and presenter model run on the simulator and CoreS3.
Small features remain modules in `application-features`; a feature receives a
separate crate only when it has a materially independent dependency or reuse
boundary. Dynamic device plugins are not supported.

`deskkin-presentation` is allocation-free `no_std` Rust. It owns only the
presentation-time Pet animation state and normalized loop/frame coordinates. It is
not a character domain model and does not own application semantics, assets,
Slint properties, clocks, or hardware motion. UI owners supply elapsed time and
adapt its closed frame result to the shared Slint Pet surface.

## User interface and runtime

Slint is the shared declarative UI. One owner controls each Slint instance.
Runtime tasks and callbacks exchange typed application inputs and views with
that owner rather than mutating UI state directly.

The embedded Koyori skin is four normalized horizontal QOI loop atlases. Its
Codex Pet JSON and WebP source are not runtime inputs. The simulator and CoreS3
use the same native 144×156 Slint crop geometry; only a physical CoreS3 benchmark can make
claims about sustained rendering rate or display-transfer latency.

The CoreS3 Pet benchmark is a fixed 60-second device operation. It stops the
application worker, schedules 1,200 Pet updates at 20 FPS, and publishes one
bounded timing-and-counter summary after measurement. Simulator timing, image
content, pixels, asset paths, raw device packets, and digest values are outside
the benchmark diagnostic contract.

The simulator uses a hosted runtime and deterministic virtual-time scenario
driver. The current CoreS3 product firmware uses one Rust/Embassy UI owner and
one Rust service worker hosted by Zephyr threads. Embassy is a runtime adapter,
not part of the portable core.

The replacement CoreS3 runtime consists of two Zephyr AMP images. PROCPU owns
USB control, boot/status supervision, LCD power/reset/backlight, and two static
full-screen internal-SRAM RGB565 framebuffers. PROCPU initializes and maps a
bounded Quad-PSRAM allocation region once before starting APPCPU, then enables
the second core's cache bus and publishes that region. APPCPU owns the heap
metadata and uses the dedicated high-end 4 MiB region only for Slint dynamic
allocations; it is disjoint from PROCPU's registered external heap. APPCPU
exclusively owns Slint software rendering, SPI2, GDMA, and LCD transfer. The
kernels exchange bounded publication metadata through shared memory;
pixel contents are never message payloads and SPI2 has one CPU owner.
Heartbeat snapshots use an explicit unstable marker and matching generations
so the supervisor never accepts a payload while APPCPU is rewriting it.

The physical pipeline keeps compressed QOI bytes in flash and only the active
decoded loop in PSRAM. A loop transition clears the Slint image owner, decodes
the next QOI directly into its final RGBA buffer, installs it, and only then
requests a draw. Idle and Attend advance every 100 ms; MoveRight and MoveLeft
advance every 50 ms. Rendering uses two complete 320×240 internal-SRAM
framebuffers. Slint `SwappedBuffers` reports the region
changed across the two-buffer repaint history; the adapter transfers its
bounding rectangle directly from the full-frame stride. The ESP32 MIPI-DBI
driver gathers the selected row spans into GDMA descriptors without a bounce
copy. A complete-frame dirty region uses the same path with bounded 30-line
transactions. Completion ownership prevents either buffer from being reused
early. LCD SPI runs at 40 MHz and QIO flash at 80 MHz. Separate same-priority
APPCPU render and display threads overlap safely while the separate PROCPU
remains responsive. The
fixed 60-second benchmark derives throughput from device heartbeat times at its first
and last valid observations. It treats frame rate and latency as measurements,
while stale or incomplete observation, unresponsive PROCPU status, allocation
failure, and transfer failure remain integrity errors. Simulator timing is not
used.

Zephyr owns CoreS3 device discovery, hardware topology, drivers, networking,
storage, scheduling, and system services. Unsafe Rust, C FFI, and raw Zephyr
types end at narrow platform adapters. Board quirks belong in devicetree,
drivers, or board support rather than portable application branches.

## Desktop host and connectors

`deskkin-host-capabilities` is the host-only capability and connector
composition root. It owns a closed compile-time registry, connector lifecycle,
semantic requests and results, namespaced effect identities, and exact
completion validation. It is independent of protocol, provider, runtime,
filesystem, credential, and diagnostic implementations.

The desktop host adapts between host semantics and protocol messages. Connector
failures remain distinct inside the host even where protocol major 1 collapses
them to `ReadFailed`. Stale or mismatched completions cannot update capability
state.

The current registry contains only availability read and a deterministic
availability connector. A provider connector must define semantic mapping,
credential ownership, authority, failure classification, and observation at
the host boundary. Provider payloads are never forwarded directly to a device.

The host accepts one authenticated device session. Its ordinary mode binds an
explicit loopback address. Physical mode binds one exact assigned RFC1918 IPv4
address on port `39042`; wildcard, public, link-local, IPv6, unassigned, and
other-port binds are rejected. Deskkin does not modify firewall or interface
configuration.

A single session writer owns encrypted frames and TCP writes. Bounded
application and reserved-control queues make overload and shutdown explicit.
Identity filesystem work is isolated in a blocking actor. While a runtime is
alive, identity mutation passes through its private generation-bound Unix owner
socket instead of racing another store writer.

Named physical-host profiles are ignored schema-1 JSON below
`.deskkin/profiles/`. A profile contains only a canonical role root, bind mode
and address, deterministic availability selection, and recording choice. It
contains no identity, pairing, Wi-Fi, or provider credential. Launch remains
foreground-only; status compares exact launch metadata and stop is
generation-bound.

## Protocol

Protocol major 1 carries semantic messages rather than UI properties, Zephyr
events, provider payloads, or hardware operations.

The initiator sends the six-byte prelude `44 53 4b 4e 00 01`. Both peers bind
it as the prologue for `Noise_XX_25519_ChaChaPoly_BLAKE2s`. Encrypted bootstrap
schema 1 independently negotiates protocol majors, required and optional
feature bits, requested permission bits, selected features, and granted
permissions.

`deskkin-protocol` is an allocation-free `no_std` codec with closed canonical
messages, caller-provided buffers, one-byte message tags, and two-byte
big-endian bounded frame lengths. It owns no socket, authentication,
persistence, runtime, or application type.

The implemented feature is `availability.read.v1`; its permission is
`availability.read`. Hardware capability, application capability, protocol
capability, and granted permission are distinct. Opaque transaction, session,
and operation identities correlate requests and diagnostics without making a
late result from a closed session valid.

`deskkin-protocol-client` owns the portable reconnect and request-correlation
state. Disconnect invalidates availability to `Unknown`; it never fabricates
`Unavailable`. Terminal incompatibility, missing required features, and
authorization denial require an explicit new connect. Transient busy and
transport failures use bounded reconnect.

## Identity and persistent state

Each peer has an explicitly initialized X25519 static identity. Noise XX
pairing derives the same six-digit local authentication string on both peers;
the string is never accepted from the remote peer or persisted. Both peers
must explicitly confirm it. Only durable `paired` state permits an application
session.

Pairing and exact unpair are generation-bound, crash-recoverable state
machines. Hosted stores use private modes, atomic replacement, bounded files,
and fail-closed validation of symlinks, unknown entries, corrupt state, and
ambiguous recovery.

CoreS3 stores one pairing identity and one WPA2-Personal 2.4 GHz Wi-Fi profile
in separate two-slot Zephyr NVS namespaces. Records contain schema and state,
publication sequence, generation, bounded payloads, and CRC. A write becomes
canonical only after readback; corrupt, unknown, ambiguous, or equally ranked
conflicting records fail closed.

CoreS3 NVS is plaintext. Wi-Fi material and the Noise private identity are
recoverable from flash. Deskkin does not enable flash encryption, secure boot,
or eFuse mutation, and logical clear does not claim forensic erasure of prior
cells.

## Observation

External, asynchronous, state-changing, and multi-stage paths expose separate
result, owner-control, and local diagnostic surfaces. The reusable recorder
publishes bounded atomic runs and never changes semantic results when disabled,
full, or unhealthy.

Hosted diagnostics contain closed operation, outcome, error, duration,
protocol/feature/permission, queue, correlation, completeness, and recording
health fields. They exclude authentication strings, raw keys, credentials,
addresses, wire bytes, payloads, arbitrary errors, paths, environment, and
machine/user/process identity. Host and simulator stores are separately locked
and capped at 16 MiB with no remote exporter.

CoreS3 diagnostics are recorded by the USB-connected host runner from
allowlisted device records. Device and host diagnostic roots are independently
capped at 16 MiB. Runner absence or recording failure cannot alter firmware
behavior.

## Current limitations

- Linux is the only hosted platform.
- Availability is the only production application feature.
- There is no external provider connector or provider credential store.
- Device features and host connectors are compile-time registered.
- Physical mode supports one exact IPv4 listener and one device session; there
  is no discovery, hostname, wildcard bind, IPv6, daemon, or autostart.
- CoreS3 supports one Wi-Fi profile and plaintext NVS only.
- There is no OTA, rollback, firmware compatibility, packaging, release, or
  migration policy.
- Mutation capabilities, confirmation, authorization replay protection, UI
  navigation, and conversation semantics are not implemented.
- Pet animation is presentation-only. Fixed-world projection, parallax,
  information cues, pose observation, and physical motion are not implemented.

Resolve a limitation only when a concrete vertical slice needs it. Update this
document to describe the resulting current architecture; use version-control
history for superseded designs and decision rationale.
