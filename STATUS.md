# Current status

Updated: 2026-09-04

## Active work

SIMD blit optimization is complete and the selected firmware is flashed.
The generic alpha kernel loads eight A8 bytes per vector, skips RGB source
loads for transparent vectors and fuses three mask loads with PIE arithmetic.
Aligned padded backings use simpler span validation; empty scalar edges are
skipped. Grass assets, density, size, LOD and blend arithmetic are unchanged.
There is no grass-specific mixed binary-alpha kernel.

World rendering runs when a back buffer is available, with no 50 ms pacing
wait. The 20 FPS value remains a diagnostic budget. UI shells retain 20 Hz
pacing and character animation uses elapsed time. Both sides of the final
comparison use this same pacing, isolating the SIMD change from cap removal.

## Performance findings

| Implementation, both uncapped | Median FPS, 60 seconds x 3 | Mean alpha blit | Mean pixel phase |
| --- | ---: | ---: | ---: |
| Previous PIE kernels (`2fd20405`) | 19.523 | 15.983 ms | 41.520 ms |
| Selected PIE kernels | **20.308** | **13.943 ms** | **39.635 ms** |

The control runs were 19.645 / 19.523 / 19.406 FPS; selected runs were
20.308 / 20.309 / 19.993. The median improves **4.02%**, above the 2% selection
threshold. Mean alpha blit falls **12.76%** and pixel-phase time falls **4.54%**.
The median exceeds the 20 FPS guideline; this is not a per-frame minimum.

Each implementation also completed a 120-second normal profile with 258
samples. Mean opaque blit was 0.319 / 0.304 ms and residual sampling/span work
was 25.217 / 25.387 ms (control / selected). Mean alpha pixel counts were
163,872 / 163,794; opaque counts were 13,707 / 13,700. These moving-scene
profiles include elapsed preemption. Sampling/span time is a residual,
not isolated texture sampling.

The input-load, internal-instruction-RAM and combined capped candidates each
completed flash, a 60-second benchmark and a 120-second normal profile. Moving
kernels to internal instruction RAM did not improve alpha time and was dropped.
Only the selected implementation remains in product code. Experiment variants,
run IDs, profile summaries and test logs are retained in
`.deskkin/experiments/simd-kernel/`.

## Verification

`mise run test` passed, including clean CoreS3 conformance build
`69d2494e-6f97-4c20-aa6d-42711d69aecb`. Fresh independent full-diff review of
`2c675eb9` found no actionable issues; only this status summary changed afterward.
Final flash `3ee02585-5a43-4a62-a485-7853f44cd5a0` passed 345,600 hardware
blit/reference and guard cases: lengths 0–320, independent source/destination/A8
alignment, padded and unpadded backings, byte order, alpha extremes and
transparent-to-visible source-cache transitions. Three benchmarks and the final
120-second profile completed without integrity failure. Final status
`9ac5d4da-b21f-472c-b3b1-22972ce3532c` confirmed 9,302 frames, fresh heartbeat,
renderer fault 0 and zero allocation/transfer/stale failures.

There are no new pixel buffers, source copies, mask storage, dependencies or
portable-core changes. Internal static SRAM remains 34,104 bytes; executable
text grows by 92 bytes relative to the equally uncapped control. Existing typed
benchmark/profile and renderer/display status remain the observation surface.
LCD motion has not been observed through a camera in this optimization pass.

The yielding SPI DMA wait, renderer-exclusive PIE ownership and spillable
`call4` APPCPU startup trampoline remain the baseline. Other APPCPU threads
and ISRs do not use PIE; q/SAR_BYTE state is not live across kernel calls.
Zephyr preserves SAR across preemption and leaves CP3 enabled. Earlier
qualification evidence remains in `.deskkin/experiments/dma-yield/`,
`.deskkin/experiments/pie-owner/` and `.deskkin/experiments/alpha-direct/`.
No remote publication is authorized.

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
