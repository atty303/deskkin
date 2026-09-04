# CoreS3 operation

## Supported product

The only CoreS3 product path is the MCUboot plus dual-Zephyr AMP sysbuild for
`m5stack_cores3/esp32s3/procpu` and `m5stack_cores3/esp32s3/appcpu`. The
repository pins Zephyr, west, the ESP Rust toolchain, Slint, and build tools.
Repository-local toolchains, builds, results, profiles, and encrypted Wi-Fi
material stay below ignored `.deskkin/` state.

PROCPU owns USB control, touch, Wi-Fi, DHCP/TCP, Noise, NVS, application
service, virtual pose, APPCPU boot, display power/reset, and status. APPCPU owns
the Slint instances, canonical billboard textures, custom world renderer,
SPI2/GDMA, and LCD transfer. Framebuffers remain in internal SRAM; the display
thread submits each completed framebuffer directly through GDMA. The
allocated PSRAM is reserved for heaps, cached textures and assets, networking,
and stacks that do not need cache-independent execution. There is no supported
single-image firmware or `core-s3:amp-*` task alias.

## Task and authority boundaries

| Task | Effect |
| --- | --- |
| `core-s3:bootstrap` | Install repository-local CoreS3 toolchains. |
| `core-s3:build` | Build MCUboot, PROCPU, APPCPU, and inert recovery. |
| `core-s3:status` | Read bounded USB product status. |
| `core-s3:identity-init` | Create the device Noise identity in NVS. |
| `core-s3:wifi-provision` | Write one WPA2-Personal profile from an encrypted host profile. |
| `core-s3:run` | Start the normal application service. |
| `core-s3:benchmark` | Run the fixed 60-second paired world benchmark. |
| `core-s3:raster-profile` | Sample per-frame raster timing without changing the scene. |
| `core-s3:flash` | Flash every AMP sysbuild domain. |
| `core-s3:recover` | Flash the inert recovery image. |

`mise run test`, `mise run test:host`, and `mise run test:core-s3` do not touch
a serial device. Build/test authority does not authorize flash, provisioning,
identity creation, application start, benchmark, recovery, or reset. A live
device mutation requires a separate explicit approval.

## Build and flash

Bootstrap once, then build:

```bash
mise run core-s3:bootstrap
```

```bash
mise run core-s3:build
```

The APPCPU primary and secondary slots are 3 MiB. `core-s3:build` produces and
validates MCUboot, PROCPU, APPCPU, and inert recovery artifacts. `core-s3:flash`
uses one multi-domain west flash operation; successful completion requires all
domain writes, readback hashes, and reset to succeed.

The APPCPU entry trampoline is carried in the repository's ordered Zephyr patch
series and is included in the bootstrap tree digest. Builds verify that pinned
tree instead of rewriting a shared Zephyr source file temporarily.

After separate live approval:

```bash
mise run core-s3:flash -- --device /dev/ttyACM0
```

## Pairing and normal application

Read status first. Missing identity, Wi-Fi profile, or host profile is reported
without creating it. Initialize or provision each missing prerequisite only
after its own approval. CoreS3 stores one generation-bound Noise identity and
one WPA2-Personal 2.4 GHz profile in separate two-slot NVS records with CRC and
readback verification. NVS is plaintext; flash encryption, secure boot, eFuse
mutation, and forensic erasure are outside the product.

The device and host derive the same six-digit Noise XX authentication string.
Opening the host pairing window authorizes one pairing attempt and prints the
host string without a second host-side confirmation prompt. Tap **Pair**,
compare the host string with the device, and confirm on the device. Setup,
Pair, Confirm, and Cancel remain full-screen Slint UI. Only the paired shell
enters world mode.

With an existing approved host profile:

```bash
mise run core-s3:run -- --device /dev/ttyACM0
```

The device initiates protocol major 1 to the profile's exact assigned RFC1918
IPv4 address on port 39042. Deskkin does not alter host firewall or interface
configuration.

## Continuous cylindrical billboard world

The cylinder is an invisible coordinate model. No cylinder surface, ring,
floor, track, tangent-facing board, mesh, lighting, or z-buffer is rendered.
The paired night-garden demo uses 23 camera-facing screen-axis-aligned
billboards and 224 fixed-pixel ground particles, with shared scene placement and motion in `demo_world.rs`:

- Character at radius 2.2, moving +12 degrees/s with the existing QOI loops.
- Availability at radius 1.8 and -45 degrees, above the character, or a
  labelled demo garden introduction when absent.
- CompositionCheck Notice at radius 1.8 and +35 degrees when present, or
  labelled demo field notes when absent.
- A third demo exploration-guide card at radius 2.3 and +170 degrees.
- A generated garden drone at +42 degrees, ping-ponging from radius 1.0 to
  2.5 at 0.25 unit/s while gently bobbing.
- Three botanical terrariums and three warm lanterns distributed around the
  circle at different heights and radii; only lanterns bob.
- Twelve small drifting lights. Their glow is baked alpha, not lighting.
- Forty-eight static ground particles: mushrooms, sedge, flowers, mossy cairns,
  crystals and trail markers. Six shared 12/6/3 px LOD sets are cached once,
  using 3,420 bytes including masks. Depth selects the LOD; native-size raster
  skips scaler setup (and its preparation count) and retains normal clipping,
  alpha and painter order.
- Another 176 grass clumps use three variants with 96x48, 48x24 and 6x3 px
  native LODs, adding 52,041 bytes including masks. Planting favors the
  outer ground; larger near and middle LODs overlap into a dense foreground.
  Source images contain 47 fine blades and dense low growth with irregular edges.

Particle foot anchors and the 1.2-unit character use ground height -1.0.

All autonomous motion uses bounded periodic integer phases. The render API
still accepts caller-owned slices; the 247-entity capacity belongs only to this
demo, not to a global entity registry. Projected entities and decoration
metadata use reusable PSRAM heap storage. A non-inlined raster helper keeps its
scene array off the recursive sort call stack. The renderer uses a 32 KiB
PSRAM stack for the expanded scene; no per-frame allocation is added.
The benchmark's expected entity count
includes decorations and three cards. Normal operation replaces absent semantic
boards with demo copy; absence still remains explicit in ApplicationViews.
The three additional canonical demo textures consume at most 190,400 bytes in
APPCPU PSRAM and are generated only on first use.

World coordinates are signed Q16.16 and azimuth is an unwrapped signed integer
with 65,536 units per turn. Camera radius is 4.0, near plane 0.25, viewport
320x240, horizontal FOV 90 degrees, and focal length 160 px. The shared
1,024-entry Q1.15 trigonometric table and integer projector are used on host and
device. Visible billboards are stably sorted far-to-near by actual depth, then
by billboard ID.

A continuous 3 degrees/s orbit adds one unwrapped target turn every 120 seconds.
It runs only in paired world mode; APPCPU retains fractional milliseconds and
publishes the combined orbit/touch target. A benchmark adds a persistent extra
turn offset on PROCPU so subsequent live targets do not cancel it.
A positive 320 px horizontal drag adds one target turn; vertical movement does
not affect yaw and a new gesture does not normalize accumulated turns. The
virtual observed yaw follows the unwrapped target at at most 0.5 turn/s without
overshoot. Only observed yaw enters projection. Physical servo power,
actuation, and a neck pose sensor are not implemented.

Availability states and CompositionCheck are 272x124 opaque Slint components
captured to RGB565 and cached in PSRAM. Camera motion does not redraw them. The
active Character QOI loop is converted once to RGB565+A8. Three canonical
96x96 RGBA8 generated sprites are converted once to shared RGB565+A8 textures
in PSRAM (82,944 bytes total), plus a 243-byte light texture. Source provenance
and prompts are in `assets/world/night-garden/README.md`. The custom rasterizer
writes directly into the clipped final framebuffer: nearest+A8 for Character
and decoration sprites, fixed-point bilinear for opaque information boards.
The exploration guide is a 136x204 portrait card with its own Slint layout and
captured dimensions; other information cards remain 272x124 landscape.
Solid clear is replaced by sky and dark-ground RGB565 gradients joined through
a soft fog band at the fixed center horizon. Character motion cannot shift it.
With 4x4 ordered dithering, a 3,840-byte flash-resident row table is copied directly
into the owned internal framebuffer in wire byte order; no extra framebuffer,
heap allocation, floor geometry, or per-pixel gradient calculation is needed.
Its time is included in world-raster and total render duration, not billboard
sampling counters. Native-size nearest sprites use direct texel addressing;
other sizes and information boards use the shared scaler.
The shared scaler uses exact incremental Q16 coordinates and specialized
nearest/bilinear, opaque/A8 and byte-order kernels. Its bounded 2,560-byte
horizontal coordinate/weight scratch lives on the existing PSRAM renderer stack;
no persistent allocation or internal-SRAM buffer is added. Per-pixel division
and alpha-endpoint blending are avoided without changing RGB565 rounding or
sample-counter semantics within each drawn span. Renderer/display priorities and DMA ownership are unchanged.

Opaque texture coverage has no alpha or mask allocation. A8 textures additionally
keep packed 8x8-source-block opaque masks in PSRAM: 360 bytes for the active
1152x156 Character atlas and 55 bytes for the three 96x96 decorations plus 9x9 light.
Each bit certifies all valid texels are 255, not merely that some are nonzero.
The renderer uses 8x8 screen-tile occlusion with conservative nearest-sample
source-footprint checks; opaque RGB565 cards need only full rectangle coverage.
Farther samples and background under a certified tile are omitted, while nearer
A8 layers retain their painter order. A reusable 2,400-byte u16 screen cutoff
table is allocated once in PSRAM. It is built over board bounds before drawing;
scaler coordinates are prepared once per non-hidden board and reused across
spans. Background visible spans are scanned once per tile band into 80 bytes of
stack scratch and reused across its rows, with four-pixel pattern stores.
Boards without occlusion retain continuous raster loops. Source footprint
coordinates are reused per tile column/row, and wholly non-opaque masks bypass
coverage testing. The portable API also supports 16x16 screen tiles with 600
bytes of storage, independently of the unchanged 8x8 source masks. There is no
depth buffer or per-entity screen mask. Scene/coordinate scratch uses the existing
PSRAM renderer stack; no per-frame heap allocation is added. The existing world-raster duration includes coverage
testing and span setup; nearest/bilinear counters count only visited samples
after occlusion (still including alpha-zero samples within visited spans).
Portable scene stats expose coverage-test and scaler-preparation counts. An
optional phase observer separates coverage, background, scaler setup and pixel
raster work without changing sampled pixels. APPCPU derives phase microseconds
from its 240 MHz cycle counter (including preemption and observer overhead),
not CPU-exclusive time or PSRAM bus traffic. Setup
includes validation, visibility selection and
coordinate preparation; pixel raster includes span traversal and blending.
Thirteen scalar values (phase/blit times and operation/sample counts) are
published together through a separate generation-checked shared slot. Only
successful world frames publish; zero generation means unavailable. The USB
status response is 224 bytes; the final 56 bytes carry this coherent record.
Shell observation explicitly changes to Paired when entering world mode, so
world sample/projection fields are not interpreted as setup-screen diagnostics.

`mise run core-s3:raster-profile -- --device /dev/ttyACM0` samples the unchanged
paired scene for 60 seconds (1-120 seconds via `--duration-seconds`), retaining
only min/mean/max scalar aggregates in the existing bounded local diagnostic
store. It reads status at no more than 5 Hz, does not initiate pairing or change
camera/animation state, rejects faults and stale/absent profiles, and supports
the existing `--recording off`. Samples are not all completed frames or a
matched-scene A/B benchmark. No pixels, text, touch traces or remote export are
added. Current firmware and host tooling must be updated together.

## AMP memory and channels

PROCPU initializes 8 MiB Quad PSRAM. Its allocator owns the low 4 MiB for the
portable service heap, Wi-Fi/network state, input/message queues, and other
non-cache-critical system stacks. The explicitly reserved high 4 MiB is a
caller-owned APPCPU allocator for canonical textures, decoded assets,
Slint/world allocations, and the display and renderer thread stacks. Two
320x240 RGB565 framebuffers remain in internal SRAM so SPI2 can submit each
completed frame directly through APPCPU-owned GDMA channel pair 0. They are the
only large render buffers that require internal SRAM.

The display and renderer threads have the same priority so rendering can run
while a five-chunk full-frame DMA transfer is active. The renderer retains a
one-tick per-thread slice at 1 kHz, and the display worker has the same one-tick
slice, without changing the global scheduler. During DMA, the SPI completion
loop yields to ready peers whenever the kernel permits yielding. This lets the
renderer use the CPU while hardware transfers the previous buffer. With no
ready peer, polling resumes immediately; no tick sleep or completion ISR is
added. Hardware completion and the cycle-based 100 ms timeout still bound each
batch. Non-DMA transfers and contexts that cannot yield continue polling.

SPI uses GDMA without a completion callback because the SPI HAL polls transfer
completion. The pinned GDMA patch therefore leaves RX/TX EOF interrupts disabled
for callback-free peripheral channels; callback-backed and memory-to-memory DMA
retain their completion interrupts. This removes the otherwise unused level-1
GDMA source implicated in the APPCPU interrupt-return failure beside the 1 kHz
level-3 timer. DMA descriptors, five-batch transfer, and direct internal-SRAM
framebuffers remain unchanged.

PROCPU and APPCPU static DRAM meet at `0x3fce4c00`; both linkers derive that
physical boundary from the AMP reservation and enforce it at link time. The
separate 32 KiB SRAM2 bank contains the 21 KiB service stack, 3 KiB control
stack, and 8 KiB Wi-Fi stack because those paths can execute while the shared
instruction cache is disabled. PROCPU starts the bounded USB control worker so
boot status remains observable, then prepares shared state and loads APPCPU
synchronously on its internal main stack before starting the application
service. The independent APPCPU entry normalizes the windowed ABI state and
initially uses the exact end of the first Zephyr interrupt-stack allocation;
normal kernel startup then moves to the internal main stack. APPCPU does not
enable Zephyr's whole-stack initialization because it would overwrite that
active pre-kernel boot stack. Its main stack is therefore reclaimed only after
thread termination and full zeroization; its boot high-water mark is reported
as unavailable rather than inferred from an unsafe fill pattern.

Boot completion is an explicit SRAM ownership boundary. After the terminated
PROCPU and APPCPU main threads are joined, their 4 KiB stacks and the unused
1 KiB shared-memory prefix are zeroized and combined into a 9 KiB internal
runtime heap used by the Espressif components that require internal memory.
During Wi-Fi initialization, its 1.5 KiB coordinator stack temporarily borrows
the beginning of the not-yet-active service stack. The coordinator is joined
and the borrowed bytes are zeroized before the service thread is created, so
the boot and runtime owners cannot overlap. APPCPU keeps only
cache-independent boot/device-initialization state and driver state in its
internal window. It creates the long-lived display and Slint/world renderer
stacks from its PSRAM allocator before starting them.

The ESP32-S3 L1 cache and MMU table are shared across the CPUs. APPCPU startup
adds its IROM and DROM mappings to unused entries without disabling the live
shared cache or changing the global IROM/DROM split. The two segment identities
are preserved with designated loader fields. After APPCPU starts, the NVS flash
guard stalls core 1 only around a cache-disabled NVS interval and always resumes
it before unlocking. Device NVS access must not bypass this guard.

The 1 KiB shared control area contains bounded generation-published records:

- PROCPU to APPCPU stable shell, optional SAS, `ApplicationViews`, and observed
  yaw snapshot.
- PROCPU to APPCPU 16-sample touch ring with generation and drop count.
- APPCPU to PROCPU Pair/Confirm/Cancel command slot.
- APPCPU to PROCPU latest target-yaw mailbox.
- APPCPU to PROCPU aggregate renderer heartbeat.

Writers publish zero while copying and release-publish the final generation.
Readers distinguish no update, unstable generation, unknown schema, invalid
semantics, and accepted stable data. Pixel buffers never cross as messages.

## Diagnostics and world benchmark

No per-frame log is emitted. The bounded status/heartbeat contains only closed
aggregate fields: view/pose/input generations, stale/drop counts, atlas
hit/miss/failure counts, visible/cull counts, nearest/bilinear sample counts,
last/max projection, sort, texture, world-raster and transfer times, and typed
allocation/decode/shared/render/transfer faults. Renderer and display each
publish one packed latest-progress value containing a bounded stage and
monotonic sequence independently of the frame heartbeat. A fresh-to-stale
heartbeat transition emits one diagnostic event with both stages; it does not
emit per-frame events.

It never records billboard text, SAS, pixels, image digest, asset path, touch
coordinate sequences, or screen snapshots. There is no remote exporter.

The fixed benchmark runs 60 seconds through the normal paired world path. It
requests a one-turn camera target, Character angular motion, generic-object
radial motion, and simultaneous Availability plus Notice while observing 1,200
updates at 20 Hz. World rendering runs as soon as a back buffer is available;
the input rate does not cap rendering. The 50 ms frame budget is a diagnostic
guideline. UI shells retain 20 Hz pacing, and character animation advances by
actual elapsed time:

```bash
mise run core-s3:benchmark -- --device /dev/ttyACM0
```

FPS, completed frames, deadline misses, and stage timings are measurements, not
performance thresholds. A typed renderer fault, stale shared snapshot, zero
completed frames, allocation/transfer failure, or incomplete observation makes
the run an integrity failure. Physical benchmark results cannot be inferred
from simulator timing.

## Verification and live acceptance

Repository verification is:

```bash
mise run fix
```

```bash
mise run test:host
```

```bash
mise run test:core-s3
```

```bash
mise run test
```

After those pass, inspect the target serial identity read-only and stop. Flash,
identity/profile/provision mutations, normal application start, and the
60-second benchmark each remain separate live checkpoints. Final physical
acceptance is visual: no cylinder primitive, every board camera-facing,
Availability and Notice coexist, Character/object parallax is visible, 320 px
drag makes one continuous turn without a seam jump, observed yaw follows at
180 degrees/s, and pairing UI remains intact.

### Background PIE kernel

CoreS3 background spans use 128-bit PIE stores through a device-only adapter.
The portable `Background` interface retains a scalar default and the existing
coverage and phase hooks. RGB565 wire order and the four-pixel dither phase
are resolved before vector stores; unaligned prefixes and suffixes stay scalar.
Each vector call writes at most 320 pixels. APPCPU PIE belongs exclusively to
the renderer thread, which enables CP3 once before its startup checks. Other
APPCPU threads and interrupt handlers must not use PIE. Zephyr preserves SAR
on interrupts and context switches; it leaves CP3 and CPENABLE untouched in
this configuration. Kernels therefore run with interrupts and scheduling enabled,
without per-call register preservation or critical sections. This ownership
contract must be revisited before introducing another PIE user on APPCPU.

APPCPU's entry trampoline calls C startup with `call4`. This leaves a valid
four-register outer caller for register-window spills; jumping directly into
C leaves that outer frame without a caller stack link when an eight-register
window overflows during deep startup calls. The pinned startup patch and its
bootstrap migration enforce this independently of the rendering kernels.

The pinned ESP32-S3 `xtensa/config/tie.h` classifies q0..q7 and SAR_BYTE as
caller-saved. No PIE value survives a kernel call. Each leaf reserves only the
32 bytes required for windowed-ABI register spills.
At startup the same kernel is checked against expected pixels and guard words
for all eight halfword alignments and lengths 0 through 320. Failure publishes
renderer fault 18 (`BackgroundCheck`) and stops before frame presentation.

### Texture blits

The portable `Blitter` interface accepts RGB565 source/destination slices,
optional A8 and destination wire order. `blit` accepts equal lengths;
`blit_from` accepts the complete readable backing slices and a source offset.
Only the destination slice is writable. Native raster passes the texture backing
storage after computing visible clipped/occlusion spans. Native rendering reuses
each span across the rows of its occlusion tile band, without copying pixels or
changing painter order. Scaled raster preserves
its sampler and uses reusable stack rows (640 bytes of colors and 320 bytes of
alpha, 16-byte aligned); their initialized spare capacity is readable padding.

CoreS3 prepares texture color/A8 planes at 16-byte-aligned offsets within
ordinary allocations, with row strides rounded up to 16 pixels. The pinned
Zephyr Rust allocator rejects layout alignment above 8 bytes, so each plane
reserves up to 15 spare bytes and generates texels directly at the aligned
offset; it does not copy an already-generated texture. Logical image dimensions and atlas frame
positions remain separate from storage dimensions; padding is zero-filled at
cache creation. No asset or grass composition changes are required.

Opaque blits use eight-pixel PIE vectors with RGB565 byte swapping. Independently
unaligned sources are assembled with `EE.SRC.Q.QUP` in registers, with no
per-span memory scratch copy. Both enclosing loads must fit inside the supplied
backing slice; otherwise the affected edge remains scalar. Destination edges
stay scalar to avoid writing outside the clipped span. One call handles the
entire aligned part of the span, with no 32-pixel splitting. Its 32-byte ABI
frame has no vector scratch. q0..q3 and SAR_BYTE are caller-clobbered; SAR and
other PIE state are untouched. The funnel performs one additional vector load
per span.

Generic alpha reads eight A8 values using one 64-bit load, or two enclosing
64-bit loads when unaligned, and expands them into 16-bit lanes in registers. It uses `weight = a8 + (a8 >> 7)` and
`dst + ((src - dst) * weight >> 8)` for each RGB component, with exact transparent
and opaque endpoints. This floor approximation can differ from the previous rounded
interpolation. No alpha expansion buffer or per-span color copy is used. Complete
zero-alpha vectors skip RGB source loads, blending and destination access; fully opaque vectors
copy source words without reading destination. Other vectors use the same
generic arithmetic. There is no grass-specific binary mask kernel.

Independent source/A8 alignment and enclosing loads are bounded by the supplied
backing slices. Destination prefixes/tails use the same approximation in scalar
code. Aligned, padded backing ends allow enclosing-vector validation once per
span; arbitrary backings retain explicit bounds checks. Consecutive visible
unaligned RGB565 vectors reuse the previous loaded vector; a transparent run
invalidates that cache. Three mask loads share PIE instructions with component
arithmetic, without changing the formula or adding memory traffic.
Each alpha call handles the entire aligned part of the span, with q0..q7, SAR
and SAR_BYTE caller-clobbered. It uses a 32-byte ABI frame and 64 bytes of
immutable masks kept in internal SRAM. Rust passes the mask address directly
to the assembly kernel; there is no intermediate C forwarding call.
There are no per-call q/SAR/SAR_BYTE
saves or restores, alpha expansion scratch, or extra source-copy buffers.

Startup checks 328,704 source/destination alignment, length 0–320, byte-order,
alpha/no-alpha and padded/unpadded backing combinations plus 12,800 RGB extreme
and alpha cases against the candidate arithmetic and guards. Another 4,096
cases vary A8 backing alignment independently and alternate transparent, opaque
and mixed runs to check source-cache transitions.
A failed blit check publishes renderer fault 19 (`BlitCheck`) before presentation.
The shared raster profile adds sampling/span overhead, opaque/alpha blit time,
and opaque/alpha pixel counts. All phase/blit times use the same 240 MHz cycle
counter, including elapsed preemption; sampling/span time is the pixel-phase
residual after blits. This differs from older approximate RTC-based phase times.
The USB status response is 224 bytes across supervisor, service and host decoder.
