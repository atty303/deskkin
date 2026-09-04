# Current status

Updated: 2026-09-04

## Current acceptance

The night garden uses 247 entities: 23 billboards and 224 static particles.
The added 176 grass clumps share three blade variants with native 96x48, 48x24
and 6x3 px LODs. Planting favors the outer ground; larger near/middle LODs
overlap into a denser foreground while the far LOD stays small. Each source
group contains 47 fine blades and dense low growth with irregular edges.
The existing six small-detail silhouettes retain
12/6/3 px LODs. All particle foot anchors use ground height -1.0. Grass textures
and masks add 52,041 bytes; all particle textures and masks total 55,461 bytes.
Native-size raster retains binary alpha, clipping, painter order and tile
occlusion without preparing scaler coordinates. Across a full camera turn,
particle bounding boxes cover at most 222,408 pixels before clipping/occlusion.

Projected entities and decoration descriptors use reusable renderer-owned PSRAM.
The device scene array remains separate from recursive sorting. Its expanded
renderer stack is 32 KiB (20 KiB additional PSRAM). ELF entry frames are
18,144 bytes for draw_world_scene, 2,960 for raster_scene_observed,
960 for render_world and 1,488 for rust_main. The background fill/wrapper/PIE
frames add 80/32/48 bytes. These are selected ELF entry frames, not a measured
whole-thread high-water mark. The desktop
simulator allocates its scene list on the heap. No dependencies or protocol,
persistence, pairing or framebuffer changes are included.

CoreS3 background spans now use 128-bit PIE stores with scalar alignment
edges, preserving wire order, dither phase, coverage clipping and phase metrics.
The portable background writer keeps its scalar default. Startup validates
2,568 alignment/length cases against pixels and guard words before rendering.
The leaf preserves q0 outside the windowed-ABI spill area; each at-most-320-pixel
call locks interrupts and saves/restores CPENABLE. No dependencies were added.

Headless front, side and rear compositions were inspected for the grass baseline.
Shared raster tests retain pixel equivalence in both byte orders, padded stride
and occlusion cases. Strict Clippy and the full `mise run test` passed, including
all AMP domains and inert recovery (build
`58094e49-c54e-4bf3-9553-e3958e229194`). Fresh review and ABI delta review have
no remaining actionable findings.

Flash `04585cb4-0146-477f-a39c-bfc7bfd006c9` verified every AMP domain hash.
The 60-second benchmark `e3fdaf8e-616b-497d-8b24-de45ebf1152e` observed 764
completed frames over 59.917 seconds: 12.750 FPS, 1,203 deadline misses and all
1,200 requested updates issued. Its final view contained all 247 entities.
Last render/transfer was 77.112/33.768 ms, maximum render 91.637 ms. Renderer,
allocation, transfer, stale-snapshot, touch-drop and cache faults remained zero.
The same grass scene with scalar background measured 12.591 FPS in benchmark
`4f5a5933-fd96-426d-906f-890942e2c8f1`. The soft 20 FPS target is still not met.

Normal-scene profiling `854ced7b-e772-4ec4-a5d8-ce7d6ddc5c43` collected 260
samples over 120 seconds. The scalar baseline profile
`1ccb143a-b6f5-4701-811a-b3b91b74e1ca` used the same scene and duration:

| Phase | Scalar mean | PIE mean |
| --- | --- | --- |
| Coverage | 3.238 ms | 3.300 ms |
| Background | 1.949 ms | 1.522 ms |
| Scaler setup | 4.964 ms | 4.621 ms |
| Pixel raster | 61.884 ms | 62.543 ms |
| Sum | 72.035 ms | 71.986 ms |

Background mean decreased about 22%, but these observational phase samples
include preemption, instrumentation and scene motion; their total barely
changed. A single FPS comparison does not establish a robust overall speedup.
Pixel raster remains dominant. Post-measurement status
`f33b8fb6-4e9f-452c-bec2-e560fc198da8` observed 2,887 completed frames, fresh
Paired heartbeat, advancing renderer/display sequences and zero renderer,
allocation, transfer, stale-snapshot or touch-drop faults.

The first PIE candidate stopped at 12 frames (failed benchmark
`594d3c5c-46bf-4456-957b-262758e193ca`). JTAG found APPCPU in
`_DoubleExceptionVector`, SP `0xffffff80`, EXCCAUSE 28, EXCVADDR `0x2c`, and
DEPC in `_handle_excint`. Its q0 scratch overlapped the windowed-ABI base save
area. Moving scratch to the lowest 16 bytes of a 48-byte frame was the only
executable correction; the runs above then passed. Startup pixel correctness
alone did not detect this register-spill corruption.

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

The user accepted the preceding particle garden composition. The denser grass
version still needs live visual acceptance for composition, parallax, continuous
320 px turns without a seam jump, 180 degrees/s observed following, and intact
pairing UI. Both normal and benchmark operation have 247 entities: absent
Availability/Notice slots use explicitly labelled demo cards, with a third
exploration guide always present. Camera target drifts 3 degrees/s in addition
to drag, through the existing observed-pose limiter.

Physical servo power, neck actuation, and a neck pose sensor remain intentionally
out of scope; observed yaw is virtual.
