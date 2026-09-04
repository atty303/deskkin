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
feature-neutral lifecycle values and local effect identities.

`application-features` depends only on `application-core`. It contains the
availability state machine and a bounded synthetic-notice feature used for
composition conformance. Features own typed state, inputs, semantic surfaces,
and effect requests. They do not call one another or access UI, transport,
connectors, or platform APIs.

`deskkin-application` is the sole concrete composition root. It owns the closed
compile-time feature registry, registry-order lifecycle broadcast, namespaced
effect identities, exact completion routing, and the bounded `ApplicationViews`
snapshot. Availability and synthetic notice are independent optional members,
so both can remain present without a surface arbiter. Rejected input, capacity
exhaustion, and identity exhaustion are transactional and publish no partial
state or effect.

The same composition and presenter model run on the simulator and CoreS3.
Small features remain modules in `application-features`; a feature receives a
separate crate only when it has a materially independent dependency or reuse
boundary. Dynamic device plugins are not supported.

`deskkin-presentation` is allocation-free `no_std` Rust. It owns Pet animation
state and the shared continuous-world implementation: signed Q16.16 world units,
unwrapped turn angles, cylindrical entity/camera poses, camera-facing billboard
projection, stable far-to-near sorting, touch-to-target yaw, rate-limited
observed yaw, and direct RGB565 rasterization. A generated 1,024-entry Q1.15
trigonometric table with linear residual interpolation keeps simulator and
CoreS3 integer behavior identical. Callers provide bounded slices; there is no
dynamic entity registry.

## User interface and runtime

Slint remains the shared declarative UI, with exactly one owner per instance.
The unpaired Setup/Pair/Confirm/Cancel shell is a full-screen Slint component.
In paired mode, Availability and CompositionCheck are rendered from a reusable
272×124 Slint billboard template into opaque canonical RGB565 textures. Camera
movement never redraws those templates. A custom renderer replaces solid clear
with a shared RGB565 night-sky/ground gradient,
projects and sorts billboards, and writes clipped scaled pixels directly into
the final 320×240 RGB565 framebuffer. Information textures use fixed-point
bilinear sampling; Character and decoration sprites use nearest sampling
and A8 blending. The cylinder is only a coordinate model: no cylinder, ring,
floor geometry, track, tangent-facing geometry, mesh, lighting, or z-buffer is drawn.
The rasterizer preserves exact Q16 sampling with quotient/remainder stepping,
reuses horizontal sample coordinates across rows, and selects filter/format/byte-order
kernels before the pixel loop. Transparent samples skip color/background access;
fully opaque samples skip blending and background reads. Bilinear interpolation
retains its intermediate RGB565 rounding. Textures carry either implicit opaque
coverage or A8 plus a packed one-bit-per-8x8-source-block mask, generated once
from the alpha plane. Only blocks whose valid texels are all 255 have set bits;
zero means unknown, not transparent. The mask is bound to the unchanged alpha
plane and texture dimensions, including atlas regions.

The screen is divided into 8x8 tiles. Each tile finds the nearest single billboard
that guarantees full opaque coverage, then draws only it and nearer billboards
in the existing far-to-near/ID order. Unknown and partially covered tiles remain
conservative; several partial occluders are not combined. Coverage is prepared
front-to-back over each board's complete-tile bounds. Source footprint endpoints
are reused by row and column; masks with no opaque block are excluded. Drawing
then visits boards in painter order, preparing scaler coordinates once per
non-hidden board and reusing them across tile spans. Unoccluded boards use one
continuous raster call. Background is omitted only below guaranteed opaque
tiles; visible background spans are scanned once per tile band and reused across
its rows with four-pixel pattern stores. Frames without any coverage use continuous
background writes. A reusable caller-owned u16 cutoff table stores painter
index + 1 (zero means no occluder),
with an explicit limit of 65,535 boards. There is no depth or per-entity visibility
buffer. Screen tiles can be 8x8 or 16x16 independently of the source mask;
the product uses 8x8. Sampling counters count visited destination samples after
occlusion, including transparent samples; skipped samples are not counted.
The background joins a muted sky to darker green ground through a soft fog band
at the level camera's fixed horizon. Character movement does not move the ground.
Compile-time ordered-dither row patterns avoid a full background texture and
per-pixel gradient arithmetic. The same painter supports native and wire byte order.

Particles have a world-space bottom-center anchor and three discrete native-size
LODs, sharing projection, depth sorting and occlusion with billboards. Native-size
nearest sprites bypass scaler tables and coordinate stepping, including clipped
and occluded spans. They retain the same sampling counters and raster phase hooks.
There is no particle simulation, physics or per-frame texture generation.
The device keeps projected entities and decoration descriptors in reusable PSRAM
heap storage. A non-inlined raster helper owns the scene array so its stack frame
is not live during recursive depth sorting; the renderer stack remains 12 KiB.

The paired night-garden demo contains a moving Character, three information cards,
a radially moving garden drone, three terrariums, three
lanterns, twelve drifting lights, and 48 static ground particles. Six hand-authored
pixel silhouettes (mushrooms, sedge, flowers, cairns, crystals and trail markers)
share cached 12/6/3 px LODs at depths up to 2/4/8 units, using 3,402 bytes of
RGB565+A8 plus 18 bytes of opaque masks on the device. Generated artwork is normalized to three
96x96 straight-alpha sprites, converted once to RGB565+A8 in the renderer's
owned heap, and reused by repeated entities. The shared `demo_world` module
owns deterministic bounded periodic motion and placement; its capacity does
not limit the generic renderer. The character is 1.2 units tall with its foot
anchor at ground height -1.0; the ground particles share that height. Information
cards are raised above the planting layer. All are screen-axis
aligned camera-facing billboards. Camera radius is 4.0, entity radius is bounded
to 0..=3.0, the near plane is 0.25, horizontal FOV is 90 degrees, and focal
length is 160 px. Touch maps each positive 320 px horizontal drag to one
unwrapped target turn. Only observed yaw affects projection; it follows the
unwrapped target without overshoot at at most 0.5 turn/s. Vertical drag has no
yaw effect. A shared fractional-time camera adapter adds a slow 3 degrees/s
unwrapped orbit to touch, without regenerating textures. APPCPU publishes this
target and PROCPU owns observed following. The first two card slots display
Availability and Notice when present, otherwise explicitly labelled demo prose;
the third is a 136x204 portrait exploration guide; the first two stay 272x124
landscape cards. Semantic view absence is not fabricated
as a real status or notice. Each closed demo texture is cached on first use.

The simulator owns one hosted Slint window. It synchronously switches the same
owner into capture mode to make missing canonical textures, restores world mode
before returning to the event loop, and uses the portable projector/rasterizer.
Its deterministic scenario driver covers camera drag, lag, multi-turn motion,
autonomous entity motion, view coexistence, and recording degradation.

The CoreS3 product is one MCUboot plus dual-Zephyr AMP sysbuild. PROCPU owns
touch, Wi-Fi, Noise, NVS, the application service, virtual observed pose, USB
control, power/reset, and status supervision. APPCPU exclusively owns Slint
texture generation, the custom world renderer, SPI2, and display transfer.
SPI2 pixel payloads use APPCPU-owned GDMA channel pair 0. The display thread
submits a completed full-screen RGB565 framebuffer directly from internal SRAM.
Because SPI polls completion without a DMA callback, callback-free peripheral
GDMA channels keep completion interrupts disabled; callback-backed and
memory-to-memory DMA behavior is unchanged. Display and renderer workers each
retain a one-tick slice at the 1 kHz system tick so either worker can run while
the other owns its current buffer.
PROCPU owns the low 4 MiB Quad-PSRAM region for service allocation,
Wi-Fi/network state, input/message queues, and non-cache-critical stacks. It
publishes the explicitly reserved high 4 MiB as the APPCPU caller-owned heap
for Character decode, canonical information and object textures, Slint/world
allocation, and the long-lived renderer/display stacks. The two full-screen
framebuffers remain in internal SRAM because GDMA requires them there; other
large storage defaults to PSRAM.

The cache-independent SRAM2 bank contains the PROCPU service and Wi-Fi stacks
that can run across flash cache-disable intervals. The PROCPU stack that loads
the AP image is internal for the same cache-disable requirement. APPCPU internal
SRAM contains only boot/device initialization, drivers, kernel state, and the
framebuffers; its long-lived rendering work runs on stacks allocated from the
high PSRAM region.

ESP32-S3 flash operations can temporarily disable the instruction cache shared
by both CPUs. PROCPU therefore serializes APPCPU startup and every device NVS
operation with one guard, stalling APPCPU only while flash may be cache-disabled
and resuming it before releasing the guard. APPCPU never initiates flash work.

AMP exchanges generation-published bounded values: stable shell/SAS/view/pose
snapshots, a touch ring with drop count, a UI command slot, and a latest target
yaw mailbox. Zero publication marks a payload unstable; readers distinguish
unstable generations, unknown schemas, and invalid semantics. Pixel content is
never a message payload and SPI2 has one CPU owner. The fixed 60-second world
benchmark starts the same paired path, requests one unwrapped camera turn, keeps
Character/radial motion and both information views active, and observes 1,200
20 Hz updates. FPS and timing are measurements only. Typed renderer faults,
stale shared state, zero completed frames, allocation/transfer failure, or an
incomplete record are integrity failures.

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
- The continuous world uses virtual observed yaw only. Physical servo power,
  neck actuation, and a neck pose sensor are not implemented.
- The world is billboard-only and has no tangent-facing boards, mesh geometry,
  z-buffer, lighting, semantic LOD, or size clamp.

Resolve a limitation only when a concrete vertical slice needs it. Update this
document to describe the resulting current architecture; use version-control
history for superseded designs and decision rationale.
