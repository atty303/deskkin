# Current status

Updated: 2026-09-05

## Completed work

CoreS3 now renders directly into two 320x32 RGB565 internal-SRAM bands, with
16 rows in the final band. Framebuffers shrink from **300 KiB to 40 KiB**,
releasing **260 KiB** of static internal SRAM. World and Slint shell rendering
share the completion-gated ownership path; only a successfully transferred
whole frame counts as presented. Rendering overlaps the preceding band's DMA.

Projection, sorting, coverage and horizontal sampling data are prepared once
per frozen frame. The reusable horizontal sampling cache adds 58,880 bytes of
PSRAM plus approximately 5,928 bytes of board metadata. There is no world
PSRAM full-frame intermediate, pixel-copy stage or per-frame allocation.
Grass density, size, LOD and assets are unchanged. No dependencies, portable
application-core changes or persistent user-state changes were introduced.

## Performance findings

| Implementation, both uncapped | Median FPS, 60 seconds x 3 | Mean pixel phase | Mean alpha blit | Internal framebuffer SRAM |
| --- | ---: | ---: | ---: | ---: |
| Fresh full-buffer baseline (`fd6a83c9`) | 21.178 | 37.860 ms | 13.835 ms | 300 KiB |
| Selected 32-row bands | **19.972** | 37.373 ms | 11.834 ms | **40 KiB** |

Baseline runs were 21.178 / 20.588 / 21.202 FPS; band runs were
19.913 / 19.972 / 19.991. This change trades **5.69%** lower median FPS for
260 KiB less internal framebuffer SRAM. It is near the 20 FPS guideline, not
a guaranteed minimum or a speed improvement over the full-buffer baseline.
Both implementations received a fresh flash, three 60-second benchmarks and
a 120-second normal profile under the same configuration.

The initial band candidate measured 15.511 FPS, with 65.992 ms mean transfer
time and 16.761 ms renderer buffer wait. A one-factor follow-up keeps short
SPI DMA transfers (at most 64 bytes, including 1–4 byte panel commands) polling
without yielding, while pixel payloads still yield for concurrent rendering.
This reduced mean transfer time to **38.206 ms** and renderer buffer wait to
**1.653 ms**. The first candidate also completed its 60-second benchmark and
120-second profile. The result identifies per-command task handoffs as a
major avoidable overhead when splitting the screen into eight transfers.

The selected 120-second profile has 259 samples. Mean display wait between
bands is 8.955 ms, coverage 1.683 ms, background 1.559 ms, scaler setup 1.180 ms,
opaque blit 0.329 ms and sampling/span residual 25.209 ms. Mean alpha/opaque
pixel counts are 164,001 / 13,708. Timings include elapsed preemption;
sampling/span residual is not isolated texture sampling. Band availability
still leaves transfer gaps, so overlap is preserved but not continuous.
PROCPU/APPCPU static DRAM are 111,928 / 34,096 bytes, compared with
378,168 / 34,104 bytes in the baseline.

## Verification

`mise run test` passed, including clean CoreS3 conformance build
`a0d3f259-db7c-4330-92b0-d45e01753c33`. The 27 portable presentation tests
include stitched bands against full-frame pixels, clipping, atlas offsets,
stride guards and scratch bounds. A host harness using the actual Slint line
renderer and band ownership adapter produced zero differing pixels across
three shell states and 24 bands with delayed fake DMA completion. A display
worker harness checked successful transfers and failures in every band across
17 two-frame scenarios, including recovery without false full-frame success.
Fresh independent full-diff review found no actionable issues at
`a42a00514ab10442e72f7c7bfa04fd5dc71658e0`; only this status summary changed
afterward.

Final flash `29139e5b-e1f0-44fa-94c4-0b9f4c893da5` completed startup hardware
blit/guard checks and continued drawing through all three benchmarks and
120-second profile `dff50c07-1514-44cb-b40c-2c8e15b8e147`.
Final status `a12907a9-1c8f-4b9a-8b45-fa0f2430fd36` confirmed 6,905 frames,
fresh heartbeat, renderer fault 0 and zero allocation, transfer and stale
snapshot failures. LCD motion has not been camera-observed: on this flashed
build, check that dragging the world keeps the grass and image contours intact
without horizontal seams or flicker. Pixel and ownership tests passed, but do
not observe physical scanout artifacts.

Source harnesses, logs, diagnostic run IDs, ELF digests and comparisons are
intentionally retained in `.deskkin/experiments/band-buffers/` as local evidence.
The existing renderer-exclusive PIE ownership and spillable `call4` startup
trampoline remain unchanged. Other APPCPU threads and ISRs do not use PIE;
q/SAR_BYTE state is not live across kernel calls. Zephyr preserves SAR across
preemption and leaves CP3 enabled.

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
prefix become a zeroized 9 KiB internal runtime heap. The Wi-Fi boot
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

The user accepted the preceding particle garden composition. The denser grass
version still needs live visual acceptance for composition, parallax, continuous
320 px turns without a seam jump, 180 degrees/s observed following, and intact
pairing UI. Both normal and benchmark operation have 247 entities: absent
Availability/Notice slots use explicitly labelled demo cards, with a third
exploration guide always present. Camera target drifts 3 degrees/s in addition
to drag, through the existing observed-pose limiter.

Physical servo power, neck actuation, and a neck pose sensor remain intentionally
out of scope; observed yaw is virtual.
