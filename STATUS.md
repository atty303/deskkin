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
rendering, SPI2, and display transfer. GDMA remains an unresolved bring-up
limitation. The old product task aliases are
removed. APPCPU primary and secondary slots are 3 MiB. Two RGB565 framebuffers
are carved from the high 4 MiB PSRAM renderer region rather than PROCPU static
DRAM.

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

`mise run fix`, the complete host suite, the CoreS3 Python conformance suite,
and the final aggregate `mise run test` pass, including a clean
MCUboot/PROCPU/APPCPU/inert build. Repeated live AMP flashes to
`/dev/ttyACM2` completed every domain write, readback hash, and reset. The clean
image reaches PROCPU boot stage 9 and APPCPU renderer stage 4 on core 1, reports
a fresh generation-stable heartbeat and ready display, and continuously renders
the full-screen unpaired shell. One observed run advanced to 4,026 completed
frames with zero renderer, allocation, transfer, atlas-cache, or stale-snapshot
faults at a reported 40 MHz display SPI clock.

The APPCPU IllegalInstruction was caused by PROCPU NVS/flash work disabling the
shared instruction cache while APPCPU executed from IROM. The product now
serializes APPCPU startup and all NVS flash operations with one mutex and stalls
core 1 only for the cache-disabled interval. A regression test fixes the
stall/unstall and NVS coverage contract. APPCPU allocations now use its
caller-owned PSRAM `sys_heap`; the small Zephyr kernel heap remains available
only for drivers that require `k_malloc`.

The APPCPU entry trampoline is part of the repository's ordered, digest-checked
Zephyr patch series. Product builds no longer rewrite the shared pinned Zephyr
checkout around each invocation. Touch-ring loss is accumulated from the
generations actually skipped by the APPCPU consumer and remains visible in
later heartbeat publications.

The target has no usable identity, so it correctly remains in the Setup shell.
No identity, Wi-Fi profile, host profile, or pairing state was created. The
paired world path and fixed 60-second physical benchmark therefore remain
unmeasured. APPCPU display transfer is currently the stable synchronous SPI2
path; GDMA is still disabled in the APPCPU devicetree overlay and the live
`pixel_dma_batches` counter is zero. A bounded trial enabled the GDMA node and
SPI2 `dma-enabled` property, but APPCPU then stopped inside display
initialization at boot marker 49 before publishing any heartbeat or frame; the
trial was reverted and the stable image was reflashed.

The target CoreS3 is the Espressif USB JTAG/serial device at `/dev/ttyACM2`.

## Next work

Restore and verify SPI2 GDMA without regressing the stable APPCPU boot, shared
flash-cache guard, framebuffer ownership, or full-screen pairing shell. Remove
bring-up-only boot markers and replace the pinned APPCPU entry patch only when
an equivalent upstream-compatible startup path is verified.

Identity creation, Wi-Fi provisioning, host-profile selection, and pairing each
remain separate live authority boundaries. Once those prerequisites exist, run
the normal paired application and fixed 60-second world benchmark. Physical
acceptance still requires the user's visual confirmation of no cylinder
primitive, camera-facing boards, Availability plus Notice coexistence,
Character/object parallax, continuous 320 px turns without a seam jump, 180
degrees/s observed following, and intact pairing UI.

Physical servo power, neck actuation, and a neck pose sensor remain intentionally
out of scope; observed yaw is virtual.
