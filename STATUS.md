# Current status

Updated: 2026-09-05

## Active work

None.

## Completed work

The accepted grass composition uses 176 wide clumps instead of 176 narrow
clumps. Its 144x48, 72x24 and 9x3 native LODs provide 50% more nominal
horizontal coverage, while all 176 clumps receive distinct, quadratic
outer-weighted radii from 0.6 through 3.2 instead
of occupying eight discrete rings. The larger outer radius fills the screen
edges with maximum full-turn detail bounding-box work bounded at 310,068
pixels. With the 12 degrees/s autonomous orbit, the 247-entity
60-second benchmark measured 18.778 FPS with zero renderer, allocation,
transfer, atlas, stale-snapshot or touch-drop faults. A 120-second raster
profile completed successfully (pixel raster mean 40.260 ms, alpha blit mean
13.280 ms, frame transfer mean 38.149 ms). Far grass remains on the 9x3 native
LOD beyond depth 4, or 1/256 of the near LOD pixel count per clump.

CoreS3 no longer runs the completed PIE background/blit qualification loops at
product startup. The renderer enables CP3 and enters normal initialization
directly; host arithmetic/bounds coverage remains, and a source contract test
prevents the startup loops from returning. On device, first presentation moved
from about 43 seconds to 2.683 seconds after boot. A 60-second benchmark measured
19.968 FPS with zero renderer, allocation, or transfer faults; a subsequent
120-second raster profile completed without a stop (alpha blit mean 11.781 ms,
opaque blit mean 0.316 ms, frame transfer mean 38.449 ms).

Internal SRAM released by the band buffers is now reusable on **both CPUs**.
SRAM layout comes from the Zephyr image linkers and `amp-memory.overlay`;
`amp-partitions.overlay` describes flash, not SRAM.

| Owner / use | Reserved or registered capacity |
| --- | ---: |
| DMA display bands, allocated in PROCPU and handed to APPCPU | 40,960 bytes (40 KiB) |
| PROCPU linker-derived unused range | 136,896 bytes (133.6875 KiB) |
| PROCPU runtime allocator, including reclaimed main stacks/shared prefix | 146,112 bytes (142.6875 KiB) |
| APPCPU independent system heap | 132,096 bytes (129 KiB), plus allocator metadata |

These are pool capacities before allocator metadata and live allocations, not
current free-byte measurements. PROCPU's aligned free range is
`[0x3fca3540, 0x3fcc4c00)`; its end matches APPCPU's start. APPCPU ends at
`0x3fced400`. IRAM aliases, ROM/shared memory and the separate SRAM2 worker
stacks remain disjoint. Display bands have a named `.noinit.deskkin_display_bands`
input section and cannot be included in the free pool.

PROCPU consumers use the existing `deskkin_runtime_internal_calloc/free` path;
APPCPU consumers use Zephyr `k_malloc/k_calloc/k_free`. Neither allocator owns
the other CPU's memory. PROCPU static growth automatically reduces the
linker-derived pool, with a link-time minimum-capacity guard. APPCPU's existing
1 KiB heap gained 128 KiB, reserved by moving both CPU boundary settings by the
same amount. The settings and their ROM/alignment offset are documented in
`docs/core-s3.md`. Large Slint/world allocations continue to use PSRAM.

The previously qualified 32-row double buffers, last band 16 rows, preserve
renderer/DMA overlap. Grass, assets, LOD, portable core, persistent state and
dependencies are unchanged. No pixel intermediate or copy stage was added.

## Verification

The final 176-clump source passed `mise run test`, including clean CoreS3 build
`e58d7811-f0cd-47c2-875f-d38ff0dab83a`. Physical run
`e4037eab-7f59-439b-a4a8-7f40ad147d4d` completed the 60-second benchmark at
18.778 FPS with zero renderer, allocation, transfer, atlas, stale-snapshot or
touch-drop faults. Profile run `8c4ee941-3dc0-41ec-8277-33a9f00281cf` then
completed 120 seconds without a stop.

`mise run test` passed, including clean CoreS3 build
`e34e027a-2d55-42a3-b284-359797dcc191`. Fresh independent full review and a
test-only delta review found no actionable issues at
`00514811ea4b20cf461c31d2737d0f692bcbdbfb`; only this status summary changed
afterward. Actual ELF symbols verify the common CPU boundary, IRAM/DRAM alias
bounds, band exclusion and heap extents.

Final flash `a76cdc12-1ba3-41be-bb4f-2debd970b206` booted successfully.
The existing memory-ready diagnostic reported **146,112 registered bytes**,
matching the linked PROCPU pool plus 9 KiB reclaimed after boot.
A 60-second benchmark measured **20.024 FPS**; the preceding band version's
three-run median was 19.972 FPS. This single run is a regression check, not a
claim of improved speed. A 120-second normal profile completed 259 samples:
mean pixel phase 37.394 ms, alpha blit 11.817 ms, transfer 38.233 ms,
renderer buffer wait 1.616 ms and display inter-band wait 8.869 ms.
Final status `383da205-2f97-4f38-85ad-04f1bdd33247` confirmed 5,318 frames,
fresh heartbeat and zero renderer faults, allocation/transfer failures or
stale snapshots. The user accepted the preceding band's physical appearance;
this layout change was checked through device diagnostics, without new camera
capture.

Logs, layout symbols, ELF digests and diagnostic IDs are intentionally retained
in `.deskkin/experiments/sram-layout/`. The earlier band pixel/guard/ownership
qualification remains in `.deskkin/experiments/band-buffers/`. No unresolved
implementation work remains in this slice.

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
