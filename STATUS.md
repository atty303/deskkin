# Current status

Updated: 2026-09-03

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

The current AMP product was flashed to every domain through `/dev/ttyACM0`;
every write passed hash verification and reset (flash record
`5a90aa6f-783a-4646-8886-302185ed3c53`). USB status record
`e8ffd69e-2e76-4daf-8605-0436e3a3d15a` reports `heartbeat_freshness=1`,
`renderer_stage=4`, `renderer_fault=0`, `boot_stage=9`, 40 MHz display SPI,
and no allocation, transfer, stale-state, touch-drop, or boot fault. GDMA is
active; follow-up status record `ae201ac8-58ac-4e14-9190-dc53ef25f707`
observed completed frames increase from 147 to 4,696 with the same zero-fault
state. Push-stream record
`ac0a0682-78d2-4869-b309-3f5925d3d8cf` directly observed AP startup, the
9,216-byte runtime-SRAM handoff, Wi-Fi boot completion, service start, and the
debug auto-pair command without dropped events. The setup shell remains because
there is no configured host peer.

No new identity, Wi-Fi profile, host profile, or pairing state was created. The
paired world path and fixed 60-second physical benchmark remain unmeasured.
`mise run fix`, the host and portable suite, the clean CoreS3 conformance build,
the inert recovery build, and the repository-wide `mise run test` all pass.
Fresh review found no remaining blocking issue.

The stable target is the Espressif USB JTAG/serial device selected through its
by-id path and currently exposed as `/dev/ttyACM0`.

## Next work

Replace the pinned APPCPU entry patch only when an equivalent
upstream-compatible startup path is verified.

Identity creation, Wi-Fi provisioning, host-profile selection, and pairing each
remain separate live authority boundaries. Once those prerequisites exist, run
the normal paired application and fixed 60-second world benchmark. Physical
acceptance still requires the user's visual confirmation of no cylinder
primitive, camera-facing boards, Availability plus Notice coexistence,
Character/object parallax, continuous 320 px turns without a seam jump, 180
degrees/s observed following, and intact pairing UI.

Physical servo power, neck actuation, and a neck pose sensor remain intentionally
out of scope; observed yaw is virtual.
