# Current status

Updated: 2026-09-04

## Active work

Native span and PIE call-path optimization is complete. Native raster reuses
clipped occlusion spans across each tile band; the whole-hidden precheck also
visits one row per band. Rust calls the alpha assembly kernel directly with the
existing internal SRAM masks. Grass composition, pixels, alpha arithmetic,
sampling and PIE ownership remain fixed. No source copies, pixel buffers or
dependencies are added. The portable presentation layer keeps safe scalar
defaults; FFI remains inside the CoreS3 adapter. All inputs/code/resources remain
within one trusted personal-device control domain. No new product slice or
remote publication is authorized.

## Performance findings

| Implementation, both uncapped | Median FPS, 60 seconds x 3 | Mean alpha blit | Mean pixel phase | Mean sampling/span residual |
| --- | ---: | ---: | ---: | ---: |
| Preceding qualified baseline (`5b5acfc8`) | 20.308 | 13.943 ms | 39.635 ms | 25.387 ms |
| Selected span/PIE call path | **20.995** | **13.766 ms** | **37.780 ms** | **23.726 ms** |

Baseline runs were 20.308 / 20.309 / 19.993 FPS; selected runs were
21.205 / 20.995 / 20.811. The median improves **3.38%**, above the 2% selection
threshold. Mean pixel-phase time falls **4.68%** and sampling/span residual
falls **6.54%**. The median exceeds the 20 FPS guideline; it is not a per-frame
minimum. The baseline measurements are the preceding qualification in this
session, not a fresh interleaved control. Both use the same uncapped rendering
and benchmark configuration.

Each implementation completed a 120-second normal profile with 258 samples.
Mean opaque blit was 0.304 / 0.288 ms; alpha pixel counts were
163,794 / 163,821 and opaque counts were 13,700 / 13,745
(baseline / selected). These moving-scene profiles include elapsed preemption.
Sampling/span time is a residual, not isolated texture sampling.

Two isolated candidates also received flash, a 60-second benchmark and a
120-second normal profile. Sharing native tile-band spans measured 21.010 FPS,
38.257 ms pixel phase and 23.770 ms sampling/span residual. Forced inlining of
the native raster and adapter measured 20.687 FPS, below a 2% gain over the
baseline, and was discarded. The selected implementation combines span sharing
with removal of the C forwarding call; its incremental call-removal gain is not
isolated by repeated measurement. Only the selected implementation remains in
product code. Source variants, run IDs, profile summaries, disassembly and test
logs are retained as diagnostic evidence in `.deskkin/experiments/span-bands/`.

## Verification

`mise run test` passed, including clean CoreS3 conformance build
`e11d55e0-2053-40f1-a8b1-2da26bed5112`. The 26 portable presentation tests cover
reference pixels, statistics, clipping, atlas offsets and 8/16-pixel occlusion
tile boundaries. Fresh independent full-diff and follow-up review found no
actionable issues at `289eae7b`; only this status summary changed afterward.
Final flash `268cbac5-e1c1-4fd2-a96d-682b4cf0e455` passed 345,600 hardware
blit/reference and guard cases: lengths 0–320, independent source/destination/A8
alignment, padded and unpadded backings, byte order, alpha extremes and
transparent-to-visible source-cache transitions. Three benchmarks and final
120-second profile `3e409189-4d40-4d37-9748-9cf95aafa4a7` completed without
integrity failure. Final status `a2429e92-dd1f-49a0-97a8-b32e9a39c557` confirmed
11,924 frames, fresh heartbeat, renderer fault 0 and zero allocation, transfer
and stale-snapshot failures.

There are no new pixel buffers, source copies, mask storage, dependencies or
portable-core changes. Internal static SRAM remains 34,104 bytes; executable
text grows by 372 bytes relative to `5b5acfc8`. The existing 64-byte mask table
remains 16-byte aligned in internal SRAM. Existing typed benchmark/profile and
renderer/display status remain the observation surface. LCD motion has not
been observed through a camera in this optimization pass.

The yielding SPI DMA wait, renderer-exclusive PIE ownership and spillable
`call4` APPCPU startup trampoline remain the baseline. Other APPCPU threads
and ISRs do not use PIE; q/SAR_BYTE state is not live across kernel calls.
Zephyr preserves SAR across preemption and leaves CP3 enabled. Earlier
qualification evidence remains in `.deskkin/experiments/simd-kernel/`,
`.deskkin/experiments/dma-yield/`, `.deskkin/experiments/pie-owner/` and
`.deskkin/experiments/alpha-direct/`.

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

The simulator and CoreS3 share an invisible cylindrical night-garden scene with
23 camera-facing billboards and 224 native-size LOD particles over a dithered foggy night-sky/ground gradient: moving Character,
three information cards, a radially moving garden drone, three botanical
terrariums, three lanterns, and twelve drifting lights. Availability and Notice
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
cache-independent 32 KiB SRAM2 bank contains the 21 KiB service, 3 KiB control,
and 8 KiB Wi-Fi stacks. PROCPU loads APPCPU synchronously on its internal main
stack before it starts control and service threads. After both cores leave
their main threads, the two 4 KiB main stacks plus the unused 1 KiB shared
prefix become a zeroized 9 KiB internal runtime heap. The Wi-Fi boot
coordinator temporarily borrows 1.5 KiB from the inactive service stack and is
joined and zeroized before service ownership begins.

The APPCPU display and renderer threads remain at the same priority so world
rasterization can run while the five-batch full-frame transfer is active. Both
threads have a one-tick per-thread slice at 1 kHz. Callback-free SPI GDMA does
not enable its unused EOF interrupt. SPI yields to ready peers during DMA waits
and retains timeout-aware hardware completion polling.

AMP shared memory has schema-checked, generation-published world snapshots, a
bounded touch ring with per-slot publication and drop count, UI command slot,
target-yaw mailbox, and aggregate heartbeat. Readers use before/copy/after
validation; the renderer retains its last stable world snapshot while a writer
publishes and fails closed on invalid schema or semantics. Status exposes the
identity owner generation and shell together with view/pose/input generations,
stale/drop counters, cache hit/miss/failure, visible/cull counts, sample counts,
last/max stage timings, independent renderer/display progress, and typed faults
without recording semantic text, SAS, pixels, paths, digests, touch coordinate
sequences, or screenshots. A heartbeat fresh-to-stale transition produces one
bounded progress event rather than a per-frame log.

Pairing has one human confirmation boundary: the host opens a bounded pairing
window and displays its SAS, while the CoreS3 displays the peer SAS and commits
only after the device Confirm command. The host does not require a second
stdin confirmation. Its socket read timeout expands to the remaining pairing
deadline while it waits for the device decision and returns to the normal
request timeout afterward. A paired identity retains the world shell across
ordinary session disconnects and reconnects; reconnecting invalidates stale
application data but does not temporarily expose the setup shell.

The 60-second world benchmark uses the normal paired renderer path, requests a
one-turn unwrapped camera target, forces Availability and Notice present, and
keeps Character and generic-object motion active for 1,200 nominal 20 Hz
updates. The host requires a device terminal marker, exactly 1,200 requested
updates, and bounded observation gaps. FPS, completed frames, deadline misses,
and timings are measurements; typed faults, stale shared state, zero completed
frames, allocation/transfer failure, and incomplete observation are integrity
failures. The diagnostic writer applies a closed field allowlist before any
local record is persisted.

## Next work

Replace the pinned APPCPU entry patch only when an equivalent
upstream-compatible startup path is verified.

The user accepted the preceding particle garden composition. The denser grass
version still needs live visual acceptance for composition, parallax, continuous
320 px turns without a seam jump, 180 degrees/s observed following, and intact
pairing UI. Both normal and benchmark operation have 247 entities: absent
Availability/Notice slots use explicitly labelled demo cards, with a third
exploration guide always present. Camera target drifts 3 degrees/s in addition
to drag, through the existing observed-pose limiter.

Physical servo power, neck actuation, and a neck pose sensor remain intentionally
out of scope; observed yaw is virtual.
