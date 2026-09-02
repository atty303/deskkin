# Current status

Updated: 2026-09-03

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
Wi-Fi stacks. PROCPU loads APPCPU synchronously on its internal main stack
before it starts control and service threads.

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

A clean MCUboot/PROCPU/APPCPU/inert build succeeds with the restored baseline
memory topology (final aggregate build record
`aa9e5061-3f6f-4d1c-9c53-d11f6939296c`). Link
maps place the 307,200-byte double framebuffer at `0x3fc97f60` in internal
SRAM, keep the cache-independent stacks in internal SRAM/SRAM2, and preserve
the caller-owned high 4 MiB PSRAM allocator for APPCPU textures, decoded
assets, Slint/world allocations, and long-lived renderer/display stacks.
`mise run fix`, `mise run test:host`, `mise run test:core-s3`, and the final
`mise run test` aggregate pass. A fresh review found no remaining blocking
issue after its bootstrap-migration and raw boot-trace findings were corrected.
One unchanged desktop-host concurrency test failed once during host verification
and passed on the immediate full-suite rerun and final aggregate run.

Three independent failures were separated during live bring-up. PROCPU
NVS/flash work initially disabled the shared instruction cache while APPCPU
executed from IROM. AP-image loading then exposed a PROCPU heap panic when it
ran on a PSRAM stack during cache-disabled flash reads. Finally, the
independently loaded APPCPU inherited an uncontrolled stack pointer at
`0x3fced6f0`, inside its Zephyr system heap, so device initialization corrupted
the heap and made GDMA startup layout-dependent. AP loading now runs on the
PROCPU internal main stack. The APPCPU entry clears inherited window state,
switches to the first 2 KiB Zephyr interrupt stack, and tail-jumps into normal
C startup; it no longer creates an artificial bottom call frame or uses the
loader-provided stack. Runtime NVS remains serialized and stalls core 1 only
for an actual cache-disabled NVS interval.

The APPCPU entry trampoline is part of the repository's ordered, digest-checked
Zephyr patch series. Product builds no longer rewrite the shared pinned Zephyr
checkout around each invocation. Touch-ring loss is accumulated from the
generations actually skipped by the APPCPU consumer and remains visible in
later heartbeat publications.

The normal AMP product was flashed to every domain through `/dev/ttyACM0`;
every write passed hash verification and reset (flash record
`a533146d-6242-43f2-b13b-af023688cb55`). USB status now reports
`heartbeat_freshness=1`, `renderer_stage=4`, `renderer_fault=0`,
`boot_stage=9`, and no boot error (status record
`8eec5148-e253-49c7-9e57-621a72e6fac9`). A further hardware reset without
reflashing produced the same healthy status in record
`64cc87ba-5b2d-4388-bbf4-530209a3a3f4`. GDMA is active; the setup shell is
running because the device remains unpaired.

No identity, Wi-Fi profile, host profile, or pairing state was created. The
paired world path and fixed 60-second physical benchmark remain unmeasured.

The stable target is the Espressif USB JTAG/serial device selected through its
by-id path and currently exposed as `/dev/ttyACM0`.

## Next work

Replace the pinned APPCPU entry patch only when an equivalent
upstream-compatible startup path is verified.

Identity creation, Wi-Fi provisioning, host-profile selection, and pairing each
remain separate live authority boundaries. Once those prerequisites exist, run
the normal paired application and fixed 60-second world benchmark. Physical
acceptance still requires the user's visual confirmation of no cylinder
primitive, camera-facing boards, Availability plus Notice coexistence,
Character/object parallax, continuous 320 px turns without a seam jump, 180
degrees/s observed following, and intact pairing UI.

Physical servo power, neck actuation, and a neck pose sensor remain intentionally
out of scope; observed yaw is virtual.
