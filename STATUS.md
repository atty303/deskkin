# Current status

Updated: 2026-09-02

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

The simulator renders an invisible cylindrical coordinate world with four
camera-facing billboards over solid `#16191d`: moving Character, Availability,
optional Notice, and a radially moving generic object. Availability and Notice
use canonical 272x124 Slint captures; the custom renderer handles projection,
scaling, clipping, sorting, and composition. Deterministic tests cover view
coexistence/expiry, seam continuity, multi-turn drag, observed lag, autonomous
motion, culling, sorting, sampling, alpha, and RGB565 output.

The sole CoreS3 product path is MCUboot plus AMP sysbuild. PROCPU owns touch,
Wi-Fi, Noise, NVS, the migrated application service, USB control, power/reset,
and virtual observed pose. APPCPU owns Slint shell/texture generation, world
rendering, SPI2/GDMA, and display transfer. The old product task aliases are
removed. APPCPU primary and secondary slots are 3 MiB. Two RGB565 framebuffers
are carved from the high 4 MiB PSRAM renderer region rather than PROCPU static
DRAM.

AMP shared memory has schema-checked, generation-published world snapshots, a
bounded touch ring with per-slot publication and drop count, UI command slot,
target-yaw mailbox, and aggregate heartbeat. Readers use before/copy/after
validation; the renderer retains its last stable world snapshot while a writer
publishes and fails closed on invalid schema or semantics. Status exposes the
identity owner generation and shell together with view/pose/input generations,
stale/drop counters, cache hit/miss/failure, visible/cull counts, sample counts,
last/max stage timings, and typed faults without recording semantic text, SAS,
pixels, paths, digests, touch coordinate sequences, or screenshots.

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

The final `mise run test` passes through the application, protocol, host,
simulator, presentation, scenario, and diagnostic suites and a pristine CoreS3
build of MCUboot, PROCPU, APPCPU, and inert recovery. A fresh independent review
and its follow-up review are clean after resolving publication ordering,
semantic-result correlation, deadline/drop accounting, and schema-fault
handling. No firmware from this slice has been flashed, so live frame timings
and the 60-second benchmark remain unmeasured.

## Next work

Inspect the target serial device read-only and stop for explicit flash approval.

After approved flash and existing identity/profile checks, run the normal
application and 60-second world benchmark. Physical acceptance still requires
the user's visual confirmation of no cylinder primitive, camera-facing boards,
Availability plus Notice coexistence, Character/object parallax, continuous
320 px turns without a seam jump, 180 degrees/s observed following, and intact
pairing UI.

Physical servo power, neck actuation, and a neck pose sensor remain intentionally
out of scope; observed yaw is virtual.
