# Current status

Updated: 2026-09-04

## Active work

Generic alpha SIMD qualification is complete; the selected implementation keeps
source-aligned padded texture rows and opaque register funnels from `bd4d5bea`.
A8 expands directly into 16-bit lanes; difference-form /256 interpolation keeps
exact alpha endpoints. Entire transparent vectors skip destination access and
entire opaque vectors copy directly. Remaining vectors use the generic blend;
there is no grass-specific binary mixed-mask kernel. Grass assets, placement,
density, size and LOD, application core, protocol and dependencies are unchanged.

The alpha adapter uses 192 bytes of internal SRAM for q-state and masks and a
64-byte assembly frame, preserving q0..q7, SAR, SAR_BYTE and CPENABLE with
IRQ-locked calls of at most 32 pixels. There is no alpha expansion scratch or
per-span source copy. Scaled sample rows and register-state traffic remain.

## Performance findings

All completed candidate screenings passed 60 seconds with 1,200 updates and
zero reported integrity faults, followed by 120-second normal profiles:

| Candidate | FPS | Mean alpha blit | Mean pixel phase |
| --- | ---: | ---: | ---: |
| Padded-source scalar baseline (`bd4d5bea`) | 14.184 median | 22.789 ms | 53.746 ms |
| Direct A8 SIMD, SRAM q-state | 14.297 | 21.396 ms | 53.481 ms |
| Same arithmetic, PSRAM q-state | 14.336 | 21.562 ms | 53.754 ms |
| SRAM with source vector reuse | 14.319 | 21.689 ms | 53.555 ms |
| Plus all-transparent vector skip | 14.376 | 21.170 ms | 53.141 ms |
| Plus all-opaque vector copy | 14.486 | 19.955 ms | 52.067 ms |

Candidate ordering above is one-factor screening, not repeated-run medians.
Final selected-version runs were 14.549 / 14.530 / 14.389 FPS (median 14.530),
versus baseline 14.184 / 14.222 / 14.115 FPS (median 14.184): +2.44%. This exceeds
the agreed 2% selection threshold, so the generic endpoint-aware SIMD path is
retained. The 20 FPS guideline is not met. Source padding adds 6,369 bytes
of scene color/A8 contents, plus at most 15 alignment bytes per plane.

A deterministic host preview at front/side/rear camera positions measured
47–48% transparent, 46–49% opaque and only 3.1–6.3% intermediate-alpha pixels.
For the front view, 166,458 alpha pixels form 3,571 spans; the padded alignment
model predicts 6,046 bounded PIE calls. Thus scalar usually skips multiplication,
source reads for zero alpha, and destination reads for opaque alpha. Assembly
inspection confirms those skips. Unconditionally vectorizing the blend does
more work than that baseline. Changing q-state memory alone had little effect.

A temporary on-device counter probe captured one completed frame: 5,612 calls,
141,720 vector pixels, 10.062 ms inside the assembly call, 11.460 ms including
C-side IRQ/CPENABLE handling, and 18.519 ms total alpha blit. Remaining alpha
time includes scalar edges, Rust span handling and call/preemption overhead;
these are not separately attributed. The probe was removed from product code.
JTAG acquisition was followed by a stop in `arch_system_halt` with reason 0;
three reads returned the same frame. Do not treat them as independent samples
or include the probe in performance qualification. Earlier normal profiles
completed without JTAG. The older baseline's long-run stop remains unexplained.

The reported sampling field is pixel-phase time minus opaque/alpha blit time;
it includes native span/occlusion and other overhead, not just sampling.
CCOUNT durations include preemption. Renderer and display share APPCPU at equal
priority with 1 ms time slices; the active SPI driver busy-polls DMA completion.
The contribution of that CPU contention is not isolated by these measurements.

Evidence and diagnostic source/images are retained locally under
`.deskkin/experiments/alpha-direct/` (runs.json, alpha-distribution.json,
kernel-probe.json, comparison PNGs and candidate source snapshots). Component
RGB8 error maxima across the three rendered views were 17 / 9 / 17, with
channel means below 0.84. Visual comparison and enlarged translucent regions
showed no clear artifact; differences are diagnostic, not acceptance thresholds.
LCD motion has not been observed through a camera.

## Verification state

Host guards cover 5,259,264 opaque and 5,259,264 independent color/A8 span
combinations, plus alpha endpoint/component arithmetic. Hardware startup covers
328,704 alignment/length/order/backing combinations and 12,800 endpoint/color
extremes. The full-endpoint candidate passed startup and its initial live runs.
The final `mise run test` passed (build record
`a2e454f1-2130-4c0d-b456-3c04fee1d156`). Fresh independent review of all six
changed files found no actionable issues. The selected source was flashed as
`6ae4e869-ca20-4057-b42d-23f52c603216`; all three final 60-second benchmarks
completed 1,200 updates, with no allocation/transfer failures or stale
snapshots. They also verified continued rendering across more than 180 seconds.
Their run IDs and results are retained in `alpha-direct/runs.json`. A final
status read confirmed 4,341 completed frames, fresh heartbeat and renderer
fault 0 (`38c92438-ae26-4ba2-9ffb-42047409cd82`).
The first benchmark after flash overlapped startup self-tests and measured only
34.637 seconds, so it is excluded rather than counted as a performance run.
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
