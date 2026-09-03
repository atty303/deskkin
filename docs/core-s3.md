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
The paired scene uses four camera-facing screen-axis-aligned billboards:

- Character at radius 2.2, moving +12 degrees/s with the existing QOI loops.
- Availability at radius 1.2 and -15 degrees.
- CompositionCheck Notice at radius 1.8 and +18 degrees when present.
- A fixed 32x32 generic object at +30 degrees, ping-ponging from radius 1.0 to
  2.5 at 0.25 unit/s.

World coordinates are signed Q16.16 and azimuth is an unwrapped signed integer
with 65,536 units per turn. Camera radius is 4.0, near plane 0.25, viewport
320x240, horizontal FOV 90 degrees, and focal length 160 px. The shared
1,024-entry Q1.15 trigonometric table and integer projector are used on host and
device. Visible billboards are stably sorted far-to-near by actual depth, then
by billboard ID.

A positive 320 px horizontal drag adds one target turn; vertical movement does
not affect yaw and a new gesture does not normalize accumulated turns. The
virtual observed yaw follows the unwrapped target at at most 0.5 turn/s without
overshoot. Only observed yaw enters projection. Physical servo power,
actuation, and a neck pose sensor are not implemented.

Availability states and CompositionCheck are 272x124 opaque Slint components
captured to RGB565 and cached in PSRAM. Camera motion does not redraw them. The
active Character QOI loop is converted once to RGB565+A8. The custom rasterizer
writes directly into the clipped final framebuffer: nearest+A8 for Character
and object, fixed-point bilinear for opaque information boards. Background is
solid `#16191d`.

## AMP memory and channels

PROCPU initializes 8 MiB Quad PSRAM. Its allocator owns the low 4 MiB for the
portable service heap, Wi-Fi/network state, input/message queues, and other
non-cache-critical system stacks. The explicitly reserved high 4 MiB is a
caller-owned APPCPU allocator for canonical textures, decoded assets,
Slint/world allocations, and the display and renderer thread stacks. Two
320x240 RGB565 framebuffers remain in internal SRAM so SPI2 can submit each
completed frame directly through APPCPU-owned GDMA channel pair 0. They are the
only large render buffers that require internal SRAM.

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
allocation/decode/shared/render/transfer faults.

It never records billboard text, SAS, pixels, image digest, asset path, touch
coordinate sequences, or screen snapshots. There is no remote exporter.

The fixed benchmark runs 60 seconds through the normal paired world path. It
requests a one-turn camera target, Character angular motion, generic-object
radial motion, and simultaneous Availability plus Notice while observing 1,200
updates at 20 Hz:

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
