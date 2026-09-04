# Current status

Updated: 2026-09-04

## Active work

DMA-wait scheduling optimization is complete. The display thread yields to
ready peers while SPI DMA is in flight, when the kernel allows it. The
cycle-based 100 ms timeout, hardware completion polling, five batches,
framebuffers and thread priorities remain unchanged. No completion ISR, tick
sleep, pixel copy, buffer or allocation was added. Raster arithmetic, texture
alignment, assets and grass density, size and LOD remain unchanged.

## Performance findings

| Implementation | Median FPS, 60 seconds x 3 | Mean alpha blit | Mean pixel phase |
| --- | ---: | ---: | ---: |
| Preceding renderer-exclusive PIE (`f190d618`) | 15.031 | 17.874 ms | 50.224 ms |
| PIE with yielding DMA waits | **19.402** | **15.913 ms** | **41.536 ms** |

The final runs were 19.588 / 19.402 / 19.141 FPS; the preceding baseline's three
runs were 15.031 / 14.876 / 15.066 under the same 60-second benchmark procedure.
The median improves **29.08%**. The initial candidate run measured 19.478 FPS.
The 20 FPS guideline is not yet met. All benchmarks completed 1,200 requested
updates without allocation/transfer failures, stale snapshots or renderer fault.

The 120-second normal profile completed 258 samples. Mean opaque blit was
0.311 ms and residual sampling/span work was 25.312 ms. Nearest sample counts
were 163,862 versus 163,998 in the baseline profile; bilinear counts were 13,736
versus 13,697. These profiles observe moving scenes rather than matched frames.
All phase timings include elapsed preemption. Since raster code and pixel
traffic did not change, the benchmark isolates CPU contention during DMA
polling as a substantial throughput cost.

## Verification

`mise run test` passed on the final implementation, including clean CoreS3
builds (build record `64ecac5e-65c0-4003-9639-9cbc7fcf84f9`). Independent
full-diff review of `b8988cac` found no actionable issues; only this status
summary changed afterward. Both candidate and final flashes passed hardware
startup checks. The final flash was `46ed1d45-040b-49a6-be42-0529b1d7e290`,
followed by the three final benchmarks. Final status confirmed 5,612 frames,
fresh heartbeat, renderer fault 0 and zero allocation/transfer/stale failures
(record `d432d9d0-5929-4940-acf0-cea6b6fadf7d`).

Isolated migration from the preceding `50936b1ab6...` patch state and historical
`e7d258c56...` state reached `7220921d88...`; reapplication was idempotent and
bootstrap verification passed. Run IDs, profile summaries, disassembly and
migration evidence are retained in `.deskkin/experiments/dma-yield/`. The change
adds zero bytes of pixel buffers and no allocation sites. The existing typed
benchmark/profile and renderer/display status remain the observation surface;
no new telemetry, dependency or trust boundary was introduced.

The renderer-exclusive PIE ownership and spillable `call4` APPCPU startup
trampoline remain the baseline. Other APPCPU threads and ISRs do not use PIE;
q/SAR_BYTE state is not live across kernel calls. Zephyr preserves SAR across
preemption and leaves CP3 enabled in this build. Earlier startup diagnosis and
qualification evidence remain in `.deskkin/experiments/pie-owner/`; arithmetic
and deterministic visual comparisons remain in `.deskkin/experiments/alpha-direct/`.
LCD motion has not been observed through a camera; DMA-stall fault injection
was not performed. No remote publication is authorized.

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
