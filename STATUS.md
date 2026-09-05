# Current status

Updated: 2026-09-05

## Active work

The scene-independent renderer foundation and distance-aware card filtering are
complete, verified and flashed. Application boards own focus; adapters request
bilinear for a focused near board and nearest for other near boards. Every lower
mip uses nearest. The adopted implementation measured a 26.475 FPS median against
the 23.976 FPS bilinear baseline. All USB inspection and live commands run outside
the agent sandbox. No implementation or device-validation step remains open.

## Completed work

Hot renderer/display stacks and reusable scene/coordinate tables use APPCPU's
existing internal SRAM pool. Horizontal coordinates use four-byte entries.
Scaled spans now sample and compose directly, with nearest gathers inserted
into PIE registers and no intermediate pixel/alpha row. Native textures prepare
as opaque, exact bit-packed cutout or A8; binary coverage uses a generic mask
selection kernel. Bilinear alpha filtering averages premultiplied color and
coverage. Cached card textures prepare portable mip levels at capture. Mip
selection is independent of the requested sampling filter.

SPI DMA completion and band reuse block on events. The display worker runs at
priority 0 and rendering at priority 1. APPCPU disables ISR-on-ISR preemption
with Zephyr's standard Xtensa setting after cross-level interrupt return
corruption was reproduced; rendering remains interruptible. Per-call PIE state
preservation and IRQ exclusion remain absent. Startup runs normal rendering.

Scene assets, density, size, native LODs and camera behavior are unchanged. The
accepted grass composition remains 176 wide clumps, with 144x48, 72x24 and 9x3
native LODs, radii 0.6 through 3.2, and a 12 degrees/s autonomous camera. The two
320x32 DMA bands remain 40 KiB; the final band contains 16 rows. Both CPUs keep
their existing SRAM pool reservations. `docs/core-s3.md` owns allocator and
rendering contracts.

## Verification

The baseline's three 60-second benchmarks measured 19.197, 19.181 and 19.169 FPS,
with a 19.181 FPS median. The final clean-build firmware measured 24.105, 23.976
and 23.922 FPS, with a 23.976 FPS median. Every retained performance candidate
completed a 60-second benchmark and 120-second normal profile:

| Candidate | FPS |
| --- | ---: |
| SRAM working storage and compact coordinates | 22.066 |
| Fused sampling/composition | 22.205 |
| Prepared coverage | 22.912 |
| Completion events with serialized ISR bodies | 22.970 |
| Higher-priority transfer worker | 23.180 |
| Cached mip levels | 23.945 |

The final 120-second normal profile collected 259 samples. Mean phase times
include preemption and timing overhead:

| Phase | Baseline | Adopted |
| --- | ---: | ---: |
| Coverage preparation | 2.354 ms | 2.153 ms |
| Background | 1.618 ms | 0.948 ms |
| Scaler setup | 1.253 ms | 0.674 ms |
| Pixel raster | 38.783 ms | 29.807 ms |
| Frame transfer | 37.845 ms | 32.473 ms |

Rendering and transfer overlap; these are elapsed phase times, not additive CPU
costs. Sampled spans now include composition in sampling time, so native blit
subtotals are not directly comparable to the old sampling/blit split. The final
status remained fresh at 7,741 frames, with zero faults, allocation failures and
transfer failures after three benchmarks and the normal profile. SRAM stack and
hot-table payload totals 74,632 bytes within the existing APPCPU pool; a cached
272x124 opaque card adds 25,152 bytes of PSRAM mip pixels before metadata.

The adopted focus policy measured 26.475, 26.509 and 26.448 FPS over three
60-second runs, with a 26.475 FPS median. This is 10.4% above the bilinear
baseline, 4.3% above the lower-mip-only nearest spike, and 1.6% below the
all-distance nearest spike. A 120-second profile collected 259 samples: mean
pixel raster was 20.743 ms, sampling 11.706 ms, nearest 192,855 samples/frame,
bilinear 2,038 samples/frame, and transfer 32.520 ms. Bilinear ranged from zero
to 11,304 samples/frame as the focused board crossed mip and visibility bounds.
One additional benchmark was excluded after the USB control observation stopped
halfway through; rendering remained at 26.658 FPS with zero renderer faults, and
a same-firmware hard reset restored control. Final status was fresh at 8,336
frames with zero renderer faults, stale snapshots, touch drops, allocation
failures and transfer failures. Evidence is retained in
`.deskkin/experiments/mip-nearest/focused-policy/`.

The combined-coverage experiment reduced sampled pixels but regressed to 18.923
FPS. Coverage preparation rose from 2.181 to 15.416 ms while pixel raster fell
only from 29.943 to 27.398 ms. Its additional 9,600-byte table and code were
removed. Painter order and the existing single-occluder tile scheme remain.

On-demand actual-PIE qualification passed 123,264 guarded native comparisons
and 128 sampled filter/clip cases. The corrected ISR setting completed 220
seconds of ordinary rendering, then repeated qualification successfully at
7,098 frames with fresh heartbeat and zero faults/allocation/transfer failures.
No qualification hook is included in the product source. Rejected candidate
code, temporary build/measurement helpers and firmware/source copies were
removed; diagnostic evidence remains local.

Deterministic baseline and mip previews cover three camera inputs at 1,000 ms.
Mips soften small card text while preserving the rest of the composition.
Component differences are diagnostics, not quality thresholds. Portable tests,
CoreS3 adapter guard tests, targeted Clippy and the full `mise run test` passed,
including the clean MCUboot/PROCPU/APPCPU/recovery build. Independent review is
complete; its test-reference mip selection and stale documentation findings
were corrected and rechecked without weakening pixel equality assertions.

Diagnostic logs, firmware digests and image comparisons are retained locally in
`.deskkin/experiments/renderer-foundation/`.

## Current baseline

Deskkin's portable application now publishes bounded `ApplicationViews` with
independent optional Availability and synthetic Notice boards. Both can remain
present simultaneously; a visible Notice owns focus and Availability regains it
when Notice clears. Renderer adapters translate focus to a concrete filter, so
presentation and raster data contain no application focus state. The old
surface-class arbiter and compatibility view layer are gone. Feature
registration, namespaced effect routing, exact
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
secondary slots are 3 MiB. Two 320x32 RGB565 band buffers occupy 40 KiB of internal SRAM;
the display thread transfers each completed band directly through
APPCPU-owned GDMA channel pair 0. The final band contains 16 rows. PROCPU owns the low 4 MiB PSRAM region for
service/network heaps, input/message queues, and non-cache-critical stacks;
APPCPU owns the explicitly reserved high 4 MiB for textures, decoded assets,
Slint captures and large image allocations. Renderer/display stacks and hot
working tables use the APPCPU internal heap. The
cache-independent 32 KiB SRAM2 bank contains the 21 KiB service, 3 KiB control,
and 8 KiB Wi-Fi stacks. PROCPU loads APPCPU synchronously on its internal main
stack before it starts control and service threads. After both cores leave
their main threads, the two 4 KiB main stacks plus the unused 1 KiB shared
prefix contribute 9 KiB to the internal runtime heap alongside the
linker-derived unused PROCPU range. The Wi-Fi boot
coordinator temporarily borrows 1.5 KiB from the inactive service stack and is
joined and zeroized before service ownership begins.

APPCPU rendering overlaps the previous band's DMA transfer. The display worker
has priority 0 and rendering priority 1; SPI completion and buffer reuse block
on events. Short panel commands use bounded polling. Callback-free GDMA EOF
interrupts stay disabled, and APPCPU ISR bodies cannot preempt each other.

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

The user accepted the widened, continuously planted grass composition and the
camera's autonomous and touch-driven rotation. Both normal and benchmark
operation have 247 entities: absent
Availability/Notice slots use explicitly labelled demo cards, with a third
exploration guide always present. Camera target drifts 12 degrees/s in addition
to drag, through the existing observed-pose limiter.

Physical servo power, neck actuation, and a neck pose sensor remain intentionally
out of scope; observed yaw is virtual.
