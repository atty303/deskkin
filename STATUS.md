# Current status

Updated: 2026-09-05

## Active work

None. SIMD overhead reevaluation and final device verification are complete.

## Completed work

Native span dispatch and the CoreS3 blit wrapper are inlined. CoreS3 reads the
240 MHz CCOUNT directly, without a C call for every phase/span timestamp, while
retaining compiler memory effects around measured loads/stores. Small gains
were evaluated together rather than requiring a separate 2% improvement from
each change. PIE kernels, arithmetic, clipping, sampling, buffer ownership and
texture memory traffic are unchanged. No pixel buffer, source copy, heap
allocation or static SRAM reservation was added.

The accepted grass composition remains 176 wide clumps, with 144x48, 72x24 and
9x3 native LODs and distinct quadratic outer-weighted radii from 0.6 through
3.2. The autonomous camera remains 12 degrees/s. Far grass uses the 9x3 native
LOD beyond depth 4, or 1/256 of the near LOD pixel count per clump.

Per-call PIE state preservation and IRQ exclusion remain absent under the
exclusive renderer ownership contract. Product startup enters normal rendering
without SIMD qualification loops. The two 320x32 DMA bands remain 40 KiB, with
the final band containing 16 rows. Both CPUs retain their allocated SRAM pools;
`docs/core-s3.md` owns their layout and allocator contracts.

## Verification

Fresh baseline benchmarks measured 18.691, 18.692 and 18.685 FPS; the final
version measured 19.160, 19.292 and 19.218 FPS. Each run lasted 60 seconds.
The median improved from **18.691 to 19.218 FPS (+2.82%)**. The adopted version
uses ordinary `#[inline]` for portable dispatch and passes the existing Clippy
policy without suppression.

The baseline and final 120-second normal raster profiles each captured 259
samples. They sample autonomous motion, not matched camera frames or CPU-only
time; sampling/span time is the pixel-phase residual after blit timings.

| Mean elapsed time | Baseline | Final |
| --- | ---: | ---: |
| Pixel raster | 40.308 ms | 38.621 ms |
| Sampling and span overhead | 26.719 ms | 25.018 ms |
| Alpha blit | 13.279 ms | 13.285 ms |
| Opaque blit | 0.309 ms | 0.316 ms |
| Background | 1.628 ms | 1.592 ms |
| Frame transfer | 38.267 ms | 37.887 ms |

`mise run test` passed, including clean CoreS3 build
`5b0c11e9-40c8-4cfb-8910-cd85b84a7a86`. Full independent review and a delta review
found no remaining issues at `d651d8f747a3dbdb374bb560ece07f587040b3ab`; only this
status summary changed afterward. Final flash
`62ca3419-e0cb-45b3-89cd-9c97df63e8e3` uses the clean build. Its three benchmarks
and 120-second profile `294593e3-8d43-484d-94ff-57a450cf5a0e` completed. Final
status `832fca6a-05a5-4b63-b403-4329ace46ebe` reported 7,748 completed frames,
fresh heartbeat and zero renderer faults, allocation/transfer failures, stale
snapshots or touch drops. All three benchmarks also reported zero atlas failures.

Reevaluation also covered IRAM placement (18.672 FPS, no improvement) and
removal of background call splitting (19.144 FPS versus its 19.398 FPS control;
the caller already passes at most one row). Both were reverted. Generic
endpoint-mask selection alone measured 18.798 FPS, but actual-kernel pixel
comparison did not complete because JTAG halt/resume was unreliable. The
combined endpoint-mask build was withdrawn before timing, and the final flash
restored normal debugger-free operation. No new alpha kernel was adopted.

All performance candidates received a flash, 60-second benchmark and
120-second normal profile. Task-owned experimental source, debugger scripts,
measurement wrapper and downloaded manual were removed. Diagnostic results,
ELF digests and run IDs are intentionally retained in
`.deskkin/experiments/simd-revisit/`.

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
secondary slots are 3 MiB. Two 320x32 RGB565 band buffers occupy 40 KiB of internal SRAM;
the display thread transfers each completed band directly through
APPCPU-owned GDMA channel pair 0. The final band contains 16 rows. PROCPU owns the low 4 MiB PSRAM region for
service/network heaps, input/message queues, and non-cache-critical stacks;
APPCPU owns the explicitly reserved high 4 MiB for textures, decoded assets,
Slint/world allocations, and its long-lived renderer/display stacks. The
cache-independent 32 KiB SRAM2 bank contains the 21 KiB service, 3 KiB control,
and 8 KiB Wi-Fi stacks. PROCPU loads APPCPU synchronously on its internal main
stack before it starts control and service threads. After both cores leave
their main threads, the two 4 KiB main stacks plus the unused 1 KiB shared
prefix contribute 9 KiB to the internal runtime heap alongside the
linker-derived unused PROCPU range. The Wi-Fi boot
coordinator temporarily borrows 1.5 KiB from the inactive service stack and is
joined and zeroized before service ownership begins.

The APPCPU display and renderer threads remain at the same priority so world
rasterization can run while the preceding band transfer is active. Both
threads have a one-tick per-thread slice at 1 kHz. Callback-free SPI GDMA does
not enable its unused EOF interrupt. SPI yields to ready peers during pixel DMA waits; short transfers of at most
64 bytes use bounded polling to avoid a task handoff for each panel command.
The DMA configuration and timeout-aware completion checks are shared.

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
