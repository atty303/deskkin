# Current status

Updated: 2026-09-04

## Active work

Renderer-exclusive PIE qualification is complete. APPCPU enables CP3 once
before renderer startup checks. Its background, opaque and alpha kernels run
with interrupts and scheduling enabled; no other APPCPU thread or ISR uses PIE.
No q/SAR_BYTE value is live across kernel calls. Zephyr preserves SAR across
interrupts/context switches and leaves CP3/CPENABLE untouched in this build.

The kernels no longer save/restore q or shift state, toggle CPENABLE per call,
mask IRQs, or split blits into 32-pixel calls. One call handles each span's
aligned portion. Kernels use 32-byte ABI frames and 64 bytes of internal SRAM
alpha masks, releasing the previous 128-byte saved-q region. Texture alignment,
padding, clipping, approximation arithmetic, sampling and grass composition are
unchanged. Scaled sample rows remain; no source-copy buffer was added.

## Performance findings

| Implementation | Median FPS, 60 seconds x 3 | Mean alpha blit, 120-second profile | Mean pixel phase |
| --- | ---: | ---: | ---: |
| Padded-source scalar alpha | 14.184 | 22.789 ms | 53.746 ms |
| Endpoint-aware PIE with per-call protection (`568a9ce1`) | 14.530 | 19.955 ms | 52.067 ms |
| Renderer-exclusive PIE | **15.031** | **17.874 ms** | **50.224 ms** |

Final runs were 15.031 / 14.876 / 15.066 FPS, versus the preceding version's
14.549 / 14.530 / 14.389. The median improves 3.45%; alpha time improves 10.43%.
The 20 FPS guideline is not met. All final benchmarks completed 1,200 updates
with no allocation/transfer failures or stale snapshots. The normal profile
completed 120 seconds and 258 samples. Final status confirmed 3,870 frames,
fresh heartbeat and renderer fault 0.

The scene's alpha values are predominantly endpoints (front view: 48% zero,
49% opaque, 3% intermediate). Generic endpoint skips remain. Earlier per-call
register-state traffic was an avoidable implementation cost, not an inherent
cost of SIMD. All phase/blit timings include elapsed preemption. The sampling
field is residual pixel time, including span/occlusion work. Renderer and display
still share APPCPU at equal priority with 1 ms slices, and SPI busy-polls DMA
completion; their contention has not been separately quantified.

## Startup and verification

The interrupt-enabled candidate initially failed twice during APPCPU startup,
before renderer execution. JTAG found a window-overflow double exception while
running heap initialization: `epc1=0x43e79943`, `depc=0x403d4c8f`, and invalid
caller stack link `0x00060223`. The preceding firmware booted. The entry
trampoline now uses `call4` into C, providing a spillable outer caller instead
of jumping into a C entry without one. Both subsequent candidate flashes
passed boot, startup pixel checks and continuous rendering. Earlier long-run
or JTAG-associated stops have not been attributed to this startup defect.

The maintained startup patch and pinned digest include this fix. Recognized
older bootstrap states restore the startup source before applying the current
patch. An isolated historical `e7d258c56...` state migrated successfully to
`50936b1ab6...`; bootstrap verification passed.

`mise run test` passed on the final source (build record
`9a8a2731-74a5-4f07-af7b-bed3297ac5f3`). Independent full-diff review passed after
repairing one migration finding. Host span/arithmetic property tests and all
341,504 hardware blit startup cases passed. The final flash was
`f13592c9-21b9-4d3b-a04c-ee5ac5f271fe`, followed by the three final benchmarks.
Final status record: `8a8a9989-42ff-473c-b7a4-a387dda1bf66`.

Run IDs, profile summaries, disassembly, startup diagnostic registers and
migration evidence are retained in `.deskkin/experiments/pie-owner/`.
Earlier arithmetic/visual comparisons are in `.deskkin/experiments/alpha-direct/`.
The arithmetic and assets did not change in this slice; LCD motion has not
been observed through a camera. No remote publication is authorized.

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
