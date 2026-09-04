# Current status

Updated: 2026-09-04

## Current acceptance

The source uses 71 entities: 23 billboards and 48 static particles. Six silhouettes
(mushrooms, sedge, flowers, cairns, crystals and trail markers) share native-size
12/6/3 px LODs. Native-size nearest raster bypasses scaler coordinates and its
preparation counter while retaining clipping, alpha, tile occlusion, wire order
and diagnostic phase hooks. Particle textures and masks consume 3,420 bytes.
The fog horizon is fixed; the 1.2-unit character and particles use ground height
-1.0. Information cards are raised above the planting layer.

Projected entities and decoration descriptors use reusable renderer-owned PSRAM
heap storage. The scene array lives in a non-inlined raster helper, separating
its stack frame from recursive depth sorting. The existing 12 KiB renderer stack
and framebuffer ownership are unchanged. Whole-thread stack high-water is not
measured. No dependency, persistence, protocol or pairing change is included.

Headless front, side and rear composition is inspected. `mise run test` passed
(record `337bd732-1853-456e-998b-cb5f7fac5082`); following review corrections,
strict Clippy, 26 presentation and 15 simulator tests passed, and all AMP domains
plus inert recovery rebuilt cleanly (`aabf1830-1726-4c63-a70e-f14f09ec751b`).
Fresh review and delta review have no remaining required changes.

Final flash `d13b9187-47ef-4283-a78b-d3e3b1c5875b` verified every AMP domain hash.
The 60-second physical benchmark `79e3b14e-10b8-4fe6-a482-4abf7e41a51d` completed
all 1,200 requested updates with 1,208 frames over 59.958 measured seconds:
20.147 FPS, two deadline misses, and zero renderer, allocation, transfer,
stale-snapshot, touch-drop or cache faults. It observed 69 visible and two culled
entities. Last render/transfer was 30.814/32.053 ms. The 20 FPS target is met;
it remains a measurement rather than a performance gate.

Matched-duration normal-scene profiles each sampled 260 frames over 120 seconds:

| Phase | Before mean | Final mean |
| --- | --- | --- |
| Coverage | 1.167 ms | 0.447 ms |
| Background | 2.118 ms | 2.193 ms |
| Scaler setup | 1.593 ms | 2.832 ms |
| Pixel raster | 28.328 ms | 28.875 ms |
| Sum | 33.206 ms | 34.347 ms |

Records are `a3be5d8a-6279-4b01-b65d-8659a21b9fde` and
`6d35deda-2af8-44eb-907f-a20c23eaf1bb`. The phase sum is 1.141 ms (3.4%) higher
with the added particles and revised composition. Setup timing includes
validation and visibility work even when native sprites skip scaler coordinates;
preparation counts exclude those sprites. These full-turn observational samples
include preemption and instrumentation, are not frame-locked CPU timing, and
cannot isolate particle cost from the composition changes. Both the benchmark
and final normal-scene profile completed without a renderer stop or typed fault.
Post-measurement status `7cd6a105-5a06-4045-afdc-bc3fa2ef8c78` observed 6,022
completed frames, a fresh Paired heartbeat, advancing renderer/display sequences,
and zero renderer, allocation, transfer, stale-snapshot and touch-drop faults.

## Previous qualified baseline

CoreS3 builds inherit the caller's Cargo network policy instead of forcing an
offline cache prerequisite. Both AMP manifests pass locked dependency fetching
from an empty temporary Cargo cache; regression tests cover absent, enabled, and
disabled caller offline settings. Fresh review and the final repository-wide
`mise run test` pass (build record
`383d0226-56eb-4a06-9d3f-45e55030e906`).

The user has completed visual acceptance of the world scene. All 16 dependency
patches pass the host unified-diff parser regression, and isolated replay from
their pinned pristine source files matches the installed patched trees byte for
byte. SPI patch hunk metadata now matches the source after sleep removal.

The intermittent APPCPU display stop is fixed and qualified by a physical
60-second benchmark. JTAG captured both frozen runs in Xtensa's permanent
`_DoubleExceptionVector`: the exception handler tried to restore a context
through a null base pointer in a state consistent with an interrupt-return
collision.
APPCPU had only the 1 kHz level-3 timer and one level-1 shared interrupt enabled;
the latter contained only GDMA RX0/TX0 EOF sources. Zephyr enabled those EOF
interrupts on every DMA start even though SPI supplied no completion callback
and polled the SPI HAL instead. Removing that implicated but unused level-1
interrupt eliminated the observed failure in two consecutive physical
qualification runs; no interrupt-history trace was available to establish the
exact collision sequence.

The pinned GDMA patch now enables peripheral completion interrupts only when a
callback exists. Memory-to-memory DMA and callback-backed DMA retain their
interrupt behavior. The SPI completion loop uses timeout-aware busy polling
without an extra tick sleep. Display and renderer stay equal priority and each
has a one-tick, 1 ms per-thread slice. Direct GDMA from the two internal-SRAM
framebuffers and all PSRAM ownership remain unchanged.

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
23 camera-facing billboards and 48 native-size LOD particles over a dithered foggy night-sky/ground gradient: moving Character,
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
not enable its unused EOF interrupt, while SPI retains timeout-aware completion
polling.

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

## Verification state

The current clean MCUboot/PROCPU/APPCPU build succeeds with strict
PROCPU/APPCPU separation. The PROCPU static image ends at `0x3fce4650`, its
libc heap ends at the APPCPU origin `0x3fce4c00`, and the APPCPU image begins
at that same address. The device completes AP startup, publishes the main-stack
handoff, creates the 9,216-byte phase heap, and completes `esp_wifi_init`, its
mode setup, and the boot coordinator join. The pinned Zephyr patch series also
applies from a clean upstream HEAD and passes the normal bootstrap verifier.

Three independent failures were separated during live bring-up. PROCPU
NVS/flash work initially disabled the shared instruction cache while APPCPU
executed from IROM. AP-image loading then exposed a PROCPU heap panic when it
ran on a PSRAM stack during cache-disabled flash reads. Finally, the
independently loaded APPCPU inherited window state and the first entry
trampoline placed its stack pointer 16 bytes beyond the Zephyr interrupt-stack
allocation. Its first call8 spill therefore repeated indefinitely. AP loading
now runs on the PROCPU internal main stack. The APPCPU entry clears inherited
window state, establishes the reset window, uses the exact end of the first
2 KiB Zephyr interrupt stack, and tail-jumps into normal C startup. Runtime NVS
remains serialized and stalls core 1 only for an actual cache-disabled NVS
interval. APPCPU whole-stack initialization is disabled because it overwrote
that active pre-kernel stack; its terminated main stack is still zeroized and
reclaimed, but its boot high-water value remains unavailable.

The APPCPU entry trampoline is part of the repository's ordered, digest-checked
Zephyr patch series. Product builds no longer rewrite the shared pinned Zephyr
checkout around each invocation. Touch-ring loss is accumulated from the
generations actually skipped by the APPCPU consumer and remains visible in
later heartbeat publications.

The encrypted Wi-Fi profile was provisioned through the authorized local
profile path, and device-only confirmation completed pairing with the configured
desktop host. The previously qualified AMP product was flashed to every domain through
`/dev/ttyACM0`; every write passed hash verification and reset (flash record
`b65187a8-c44a-4d89-a945-0e5a077f613d`). USB status record
`bee223e9-79dc-4cc7-aab3-636a1d0ec91e` reports paired shell 4,
`heartbeat_freshness=1`, `renderer_stage=4`, `renderer_fault=0`,
`boot_stage=9`, 40 MHz display SPI, progressing frames, and no allocation,
transfer, stale-state, touch-drop, or boot fault.

A 60-second USB push-stream observation crossed two real service disconnect and
reconnect sequences. Shell publication remained 4 throughout; the former
paired `4 -> 2 -> 4` transition that briefly rendered Setup required was not
observed, and no diagnostic event was dropped. Physical world benchmark record
`01ed62d0-5736-4210-a125-3943b71c0a96` completed all 1,200 requested updates
with 1,205 frames at 20.132 measured FPS, zero deadline misses, four simultaneous
billboards, and zero allocation, transfer, stale-snapshot, and atlas-cache
failures. The five-batch 40 MHz full-frame transfer measured 31.780 ms last and
33.789 ms maximum, restored from the prior 46--61 ms range while rendering
continued concurrently. The benchmark runner retains the benchmark-scene
sample separately from the terminal post-benchmark sample, so removal of the
synthetic Notice after completion cannot incorrectly fail the four-billboard
integrity check.

An APPCPU-only 2 kHz trial did not complete AMP startup, so the product retains
the verified 1 kHz system tick. Display and renderer each use a one-tick slice,
meeting the 1 ms worker-interleaving contract without changing the global tick.

For the current source, `mise run fix`, the device-tool suite, patch provenance
verification, and a clean MCUboot plus PROCPU plus APPCPU sysbuild pass (build
record `57ac7a0a-90d9-4518-84f6-23bcf342f896`). Flash record
`08b847cb-a10a-4a43-863c-aac64cc35e6d` verified all three domains and reset.
Physical benchmark `165bbc51-900e-4a4b-8972-f34ff8d45409` completed all 1,200
updates over 59.94 measured seconds with 1,165 frames at 19.436 FPS. It reported
456 measured deadline misses, four visible billboards, a 32.102 ms last and
38.934 ms maximum five-batch transfer, and zero renderer, stale-snapshot,
allocation, transfer, touch-drop, or atlas-cache fault. A later status record
`8f6b50e3-e0cd-4348-bf66-8de43c9c8891` observed 1,686 completed frames with a
fresh heartbeat and both renderer/display progress sequences still advancing.
A consecutive second benchmark
`dd9eaf51-9558-43a0-a265-ad264e5057d6` again completed all 1,200 updates and
1,165 frames over 59.913 seconds at 19.444 FPS with all integrity-fault counts
zero. Its last transfer was 31.689 ms; the boot-lifetime maximum remained
38.934 ms. Post-benchmark status
`ea74da0a-f154-4955-a3d4-fa348d10210f` observed 4,230 completed frames, a fresh
heartbeat, and advancing renderer/display progress. The repository-wide final
test passed, including host/portable suites, patch provenance, and clean
MCUboot, PROCPU, APPCPU, and inert-recovery builds (build record
`97a0a03c-9e76-4d1e-869c-45fefb98e7ea`). Fresh review found no code issue; its
one documentation finding was resolved by separating direct JTAG observations
from the inferred interrupt sequence.

## Next work

Replace the pinned APPCPU entry patch only when an equivalent
upstream-compatible startup path is verified.

The current particle garden still needs live visual acceptance for composition, parallax, continuous
320 px turns without a seam jump, 180 degrees/s observed following, and intact
pairing UI. Both normal and benchmark operation have 71 entities: absent
Availability/Notice slots use explicitly labelled demo cards, with a third
exploration guide always present. Camera target drifts 3 degrees/s in addition
to drag, through the existing observed-pose limiter.

Physical servo power, neck actuation, and a neck pose sensor remain intentionally
out of scope; observed yaw is virtual.
