# Current status

Updated: 2026-09-02

## Current baseline

Deskkin's portable application now publishes bounded `ApplicationViews` with
independent optional Availability and synthetic Notice members. Both can remain
present simultaneously; the old surface-class arbiter and compatibility view
layer are gone. Feature registration, namespaced effect routing, exact
completion validation, and transactional failure behavior remain intact.

`deskkin-presentation` contains the allocation-free `no_std` continuous-world
implementation shared by simulator and CoreS3: Q16.16 cylindrical poses,
unwrapped angles, generated Q1.15 trigonometry, fixed 320x240 projection,
stable far-to-near painter sorting, direct RGB565 nearest/bilinear/A8 raster,
horizontal touch mapping, and 0.5 turn/s observed-yaw limiting.

The simulator renders an invisible cylindrical coordinate world with four
camera-facing billboards over solid `#16191d`: moving Character, Availability,
optional Notice, and a radially moving generic object. Availability and Notice
use canonical 272x124 Slint captures; the custom renderer handles projection,
scaling, clipping, sorting, and composition. Deterministic tests cover view
coexistence/expiry, seam continuity, multi-turn drag, observed lag, autonomous
motion, culling, sorting, sampling, alpha, and RGB565 output.

The sole CoreS3 product path is MCUboot plus AMP sysbuild. PROCPU owns touch,
Wi-Fi, Noise, NVS, the migrated application service, USB control, power/reset,
and virtual observed pose. APPCPU owns Slint shell/texture generation, world
rendering, SPI2/GDMA, and display transfer. GDMA is enabled again as the active
product baseline. The old product task aliases are removed. APPCPU primary and
secondary slots are 3 MiB. Two RGB565 framebuffers remain in internal SRAM and
the display thread transfers each completed frame directly through
APPCPU-owned GDMA channel pair 0. PROCPU owns the low 4 MiB PSRAM region for
service/network heaps, input/message queues, and non-cache-critical stacks;
APPCPU owns the explicitly reserved high 4 MiB for textures, decoded assets,
Slint/world allocations, and its long-lived renderer/display stacks. The
cache-independent 32 KiB SRAM2 bank contains the 24 KiB service and 8 KiB
Wi-Fi stacks. The PROCPU AP-image loader stack is internal because it remains
active while `esp_appcpu_init()` disables the shared cache.

AMP shared memory has schema-checked, generation-published world snapshots, a
bounded touch ring with per-slot publication and drop count, UI command slot,
target-yaw mailbox, and aggregate heartbeat. Readers use before/copy/after
validation; the renderer retains its last stable world snapshot while a writer
publishes and fails closed on invalid schema or semantics. Status exposes the
identity owner generation and shell together with view/pose/input generations,
stale/drop counters, cache hit/miss/failure, visible/cull counts, sample counts,
last/max stage timings, and typed faults without recording semantic text, SAS,
pixels, paths, digests, touch coordinate sequences, or screenshots.

The 60-second world benchmark uses the normal paired renderer path, requests a
one-turn unwrapped camera target, forces Availability and Notice present, and
keeps Character and generic-object motion active for 1,200 nominal 20 Hz
updates. The host requires a device terminal marker, exactly 1,200 requested
updates, and bounded observation gaps. FPS, completed frames, deadline misses,
and timings are measurements; typed faults, stale shared state, zero completed
frames, allocation/transfer failure, and incomplete observation are integrity
failures. The diagnostic writer applies a closed field allowlist before any
local record is persisted.

## Verification state

A clean MCUboot/PROCPU/APPCPU/inert build passes with the restored baseline
memory topology. Link maps place the 307,200-byte double framebuffer and 4 KiB
AP-image loader stack in internal SRAM, service and Wi-Fi stacks in SRAM2,
PROCPU heap/network/input/message state and non-critical stacks in low PSRAM,
and reserve the high 4 MiB allocator for APPCPU render resources and long-lived
renderer/display stacks. `mise run fix`, `mise run test:host`,
`mise run test:core-s3`, and the final `mise run test` pass; the final aggregate
CoreS3 build record is `3b36ec8d-c12c-4e28-b4ef-f5a3f1b8a175`. The bootstrap
migration also converges both retained legacy Zephyr patch digests after the
obsolete strided-display patch is removed.

An earlier APPCPU IllegalInstruction was caused by PROCPU NVS/flash work
disabling the shared instruction cache while APPCPU executed from IROM. During
restoration of the direct-DMA topology, a separate PROCPU heap panic exposed
that its AP-image loader stack had incorrectly been placed in PSRAM while
`esp_appcpu_init()` disabled the cache. The product now keeps that loader stack
internal, serializes APPCPU startup and all NVS flash operations with one mutex,
and stalls core 1 only for an NVS cache-disabled interval. A regression test
fixes the stack placement, stall/unstall, and NVS coverage contracts. APPCPU
allocations use its caller-owned high-PSRAM `sys_heap`; the small Zephyr kernel
heap remains available only for drivers that require `k_malloc`.

The APPCPU entry trampoline is part of the repository's ordered, digest-checked
Zephyr patch series. Product builds no longer rewrite the shared pinned Zephyr
checkout around each invocation. Touch-ring loss is accumulated from the
generations actually skipped by the APPCPU consumer and remains visible in
later heartbeat publications.

The normal AMP product was flashed to all domains on `/dev/ttyACM2`; every
write passed hash verification and the target was reset (flash record
`3926c1f1-fcd4-4f92-85c9-36bd03b605df`). The subsequent USB control status
request still ends in the typed `control_timeout` (status record
`c3bbb27e-a6f5-4d1d-a4a8-86296a70fd94`). The same timeout occurred with an
AP-disabled diagnostic image, so the remaining failure is not isolated to
world rendering, AP startup, or GDMA. The current image is the normal dual-core
product, but boot stage, renderer heartbeat, screen contents, and the
previously observed Setup shell cannot be confirmed through the control
transport.

No identity, Wi-Fi profile, host profile, or pairing state was created. The
paired world path and fixed 60-second physical benchmark remain unmeasured.

The target CoreS3 is the Espressif USB JTAG/serial device at `/dev/ttyACM2`.

## Next work

Restore USB control status readback without weakening the fixed AMP ownership,
memory bounds, or direct-DMA path. JTAG inspection is currently unavailable to
the development user because the host USB bus node is not writable; no host
permission or udev policy was changed. Once status is observable, confirm the
boot/renderer heartbeat and measure full-frame DMA transfer cost through the
paired world benchmark. Replace the pinned APPCPU entry patch only when an
equivalent upstream-compatible startup path is verified.

Identity creation, Wi-Fi provisioning, host-profile selection, and pairing each
remain separate live authority boundaries. Once those prerequisites exist, run
the normal paired application and fixed 60-second world benchmark. Physical
acceptance still requires the user's visual confirmation of no cylinder
primitive, camera-facing boards, Availability plus Notice coexistence,
Character/object parallax, continuous 320 px turns without a seam jump, 180
degrees/s observed following, and intact pairing UI.

Physical servo power, neck actuation, and a neck pose sensor remain intentionally
out of scope; observed yaw is virtual.
