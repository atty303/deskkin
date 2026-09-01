# CoreS3 operation

## Supported target

Deskkin builds product and inert-recovery firmware for StackChan on M5Stack
CoreS3 (`m5stack_cores3/esp32s3/procpu`). The repository pins Zephyr,
`zephyr-lang-rust`, the Zephyr SDK, the Xtensa Rust toolchain, Python
requirements, and all Cargo dependencies.

Repository-local CoreS3 state lives below `.deskkin/` and is ignored by version
control. Bootstrap verifies pinned west/module revisions, SDK manifests,
Rust/Cargo/libclang digests, and the local patch series before a build.

```sh
mise run core-s3:bootstrap
mise run core-s3:build
mise run core-s3:amp-build
```

`core-s3:build` and `core-s3:amp-build` are non-mutating with respect to a
physical device. The former performs clean product and inert builds; the latter
performs one clean sysbuild containing MCUboot, PROCPU, and APPCPU images.
`mise run test:core-s3` reproduces both build boundaries.

## Task authority

CoreS3 tasks deliberately separate build, observation, and mutation.

| Task | Effect |
| --- | --- |
| `core-s3:bootstrap` | Prepare and verify repository-local toolchains. |
| `core-s3:build` | Build product and inert firmware without device access. |
| `core-s3:amp-build` | Build the atlas-free AMP full-screen rendering pipeline. |
| `core-s3:amp-flash` | Flash all AMP domains in sysbuild order. |
| `core-s3:amp-gate` | Measure the APPCPU pipeline through the PROCPU USB status surface. |
| `core-s3:status` | Read boot and application status without mutation. |
| `core-s3:profile` | Create or replace the ignored age-encrypted Wi-Fi profile. |
| `core-s3:flash` | Flash the product firmware. |
| `core-s3:identity` | Execute an explicit identity lifecycle command. |
| `core-s3:provision` | Write the selected Wi-Fi profile to NVS. |
| `core-s3:run` | Run the configured application and record bounded diagnostics. |
| `core-s3:benchmark` | Stop the application worker and run the fixed Pet rendering benchmark. |
| `core-s3:recover` | Erase Deskkin storage and restore inert firmware. |

No mutation task runs from `mise run test`. Flash, identity mutation,
provisioning, application run, benchmark execution, recovery, USB reset, and
power cycling require an immediate explicit live approval that names the target
device and operation.
Read back the resulting status after an approved mutation. Recovery requires
the separate `--erase-storage` confirmation and must not be described as
forensic erasure.

Creating or replacing the encrypted profile is a separate repository-local
state mutation. Its default paths are:

- encrypted profile: `.deskkin/phase3-device/wifi.age`
- age identity: `~/.config/chezmoi/age/identity.txt`

The age identity must already exist and must decrypt the selected profile. Use
`--profile PATH` and `--age-identity PATH` after `--` to override either path.
Profile creation prompts for the SSID, WPA2 password, and exact host RFC1918
IPv4 address. Inspect and approve both paths before creating or replacing the
profile:

```sh
mise run core-s3:profile
mise run core-s3:profile -- --profile .deskkin/phase3-device/wifi.age --age-identity ~/.config/chezmoi/age/identity.txt
```

## Fresh bring-up

The following is the minimum order for a fresh device and a fresh physical-host
role. Replace `/dev/ttyACM0` and `192.168.1.10` with the inspected serial device
and the host's exact assigned RFC1918 address. `--device` may be omitted only
when automatic discovery identifies exactly one suitable serial device.

First prepare and build without touching the device:

```sh
mise run core-s3:bootstrap
mise run core-s3:build
```

After separate live approval for each mutation, flash the product firmware,
initialize the device identity, create the encrypted profile, and provision it:

```sh
mise run core-s3:flash -- --device /dev/ttyACM0
mise run core-s3:identity -- init --device /dev/ttyACM0
mise run core-s3:profile
mise run core-s3:provision -- --device /dev/ttyACM0
mise run core-s3:status -- --device /dev/ttyACM0
```

For an overridden encrypted profile, pass the same `--profile` and
`--age-identity` values to both `core-s3:profile` and `core-s3:provision`.

Initialize the matching host role, create its secret-free launch profile, and
start it in the foreground in a dedicated terminal:

```sh
mise run protocol:host -- identity-init .deskkin/phase3/physical-demo/host/identity
mise run deskkin:profile -- set core-s3 --role-root phase3/physical-demo/host --bind-mode private_lan --address 192.168.1.10:39042 --availability available --recording on
mise run deskkin:host -- --profile core-s3
```

In another terminal, open the single-use host pairing window. Tap **Pair** on
the CoreS3, compare the six-digit authentication string on both endpoints, type
`yes` on the host only if it matches, and tap **Confirm** on the device:

```sh
mise run protocol:host -- pairing-window-open .deskkin/phase3/physical-demo/host/identity
```

The configured device normally starts its application worker during boot.
`core-s3:run` is only for starting an already configured worker that is stopped;
it does not flash, initialize identity, provision Wi-Fi, or open the host pairing
window. Status remains read-only:

```sh
mise run core-s3:run -- --device /dev/ttyACM0
mise run core-s3:status -- --device /dev/ttyACM0
```

## Pet rendering benchmark

The product firmware contains a fixed 60-second Pet-only benchmark path. The
runner first stops the application worker, then requests 1,200 animation
updates at 20 FPS. The UI owner uses the same Slint software renderer, embedded
atlas, RGB565 framebuffer, dirty-line capture, and display transfer path as the
normal product UI. Network application work remains stopped for the measurement;
no servo adapter or servo-power path exists in this firmware slice.

The runner does not poll USB during the 60-second measurement. Afterward it
reads one bounded summary containing only timing and counters: requested and
completed frames, render and transfer totals and maxima, frames within 50 ms,
dirty lines and transferred bytes, deadline misses and maximum consecutive
misses, stalls over 250 ms, transfer and allocation failures, and frame-digest
update count. It records no image, pixel, digest value, asset path, or raw
packet.

The allocation-failure counter covers the fallible external-memory allocation
of the framebuffer and transfer staging buffer. Those buffers are allocated
before measurement and reused by every measured frame. A fatal Rust or Slint
allocator failure prevents terminal benchmark completion and therefore cannot
produce a passing summary.

The gate passes only when all 1,200 requests complete, at least 95% of completed
frames stay within the combined 50 ms render-and-transfer budget, no combined
render-and-transfer time exceeds 250 ms, no allocation or display-transfer
failure occurs, and the frame digest changes. Simulator timing is not an input
to this result.

Flashing the current product firmware and executing the benchmark are separate
live approvals. After both are approved for the inspected target device, run:

```sh
mise run core-s3:benchmark -- --device /dev/ttyACM0
```

The benchmark leaves the application worker stopped. Restarting it is another
explicit application run:

```sh
mise run core-s3:run -- --device /dev/ttyACM0
```

## AMP rendering pipeline

The separate AMP runtime contains no Pet atlas, network worker, NVS mutation,
or servo path. MCUboot occupies flash offset `0x0`, the signed PROCPU supervisor
image occupies `0x20000`, and the signed APPCPU image occupies `0x2c0000` in a
2 MiB renderer slot. Flashing is one multi-domain west operation so the
sysbuild dependency order is authoritative.

PROCPU exclusively owns the USB status surface and LCD power/reset/backlight.
It reserves two 320×240 RGB565 render buffers in PSRAM and one full-screen,
32-byte-aligned scanout buffer in internal DRAM, then publishes only their
addresses and readiness after PSRAM initialization and its memory test pass.
Failure inhibits APPCPU boot and is reported through the bounded boot error.
The PROCPU linker region ends at the APPCPU DRAM origin, so image growth cannot
silently overlap the renderer. APPCPU exclusively owns the Slint instance, software
renderer, SPI2, GDMA, and display driver. It renders alternating full frames in
PSRAM, copies a completed frame once into scanout, and transfers the scanout
while rendering the next frame. The SPI polling driver sleeps during DMA so the
single APP CPU can execute the renderer instead of busy-waiting.

The validated hardware configuration is Quad PSRAM at 80 MHz and LCD SPI at
40 MHz. Octal PSRAM does not boot this CoreS3. The ten-second pipeline gate does
not reset the device; it observes an already booted run and calculates FPS from
the device heartbeat times of its first and last valid samples. It requires at least
20 FPS, render and copy-plus-transfer maxima no greater than 50 ms, live display
and stable mailbox publications, a live final sample, at least 80% observation
coverage, bounded USB response latency, and zero allocation or transfer failures. The host
stores only bounded timing, counters, boot stages, and the effective SPI clock;
it records no frame, pixel, asset, raw shared-memory data, or raw USB packet.

```sh
mise run core-s3:amp-build
mise run core-s3:amp-flash -- --device /dev/ttyACM0
mise run core-s3:amp-gate -- --device /dev/ttyACM0
```

The current gate is an atlas-free pipeline gate, not yet the final 60-second Pet
benchmark. The next slice adds the complete bounded schedule and percentile
diagnostics before restoring the Pet asset.

Identity inspection and exact unpair are distinct commands. `list` reports the
64-character peer ID required by `unpair`; unpair is a device mutation and
requires its own live approval:

```sh
mise run core-s3:identity -- list --device /dev/ttyACM0
mise run core-s3:identity -- unpair --device /dev/ttyACM0 --peer-id PEER_ID
```

Recovery is the destructive cleanup route. It requires an explicit device,
live approval, and the additional storage-erasure confirmation:

```sh
mise run core-s3:recover -- --device /dev/ttyACM0 --erase-storage
```

## Device state

The device has one Rust/Embassy UI owner and one dedicated Rust service worker.
The worker owns Wi-Fi, TCP, Noise send state, session writing, and identity and
configuration mutation. A bounded C adapter hides Zephyr Wi-Fi, DHCP, socket,
NVS, randomness, USB serial, display, and touch APIs.

One X25519 identity and one WPA2-Personal 2.4 GHz profile are stored in separate
two-slot Zephyr NVS namespaces. Fixed records use schema and state validation,
publication sequence, generation, bounded lengths, CRC, and write/readback
verification. Identity creation, provisioning, exact unpair, and recovery are
explicit USB control operations. Pairing and run never create or replace these
prerequisites implicitly.

NVS is plaintext. The Wi-Fi password and Noise private identity remain
recoverable from flash. The firmware does not enable flash encryption, secure
boot, or eFuse mutation. Logical clear can leave prior cells physically
recoverable.

The host-side Wi-Fi profile is an ignored age-encrypted file. Profile creation
prompts without echo and encrypts without a plaintext intermediate file.
Provisioning decrypts through pipes and keeps credentials out of arguments,
environment, result output, diagnostics, and committed artifacts. Never commit
the encrypted profile or age identity.

## Network and host

CoreS3 initiates protocol major 1 over a personally managed LAN. The Linux host
must bind one exact assigned RFC1918 IPv4 address on TCP port `39042`. Deskkin
rejects wildcard, public, link-local, IPv6, unassigned, and other-port binds and
does not modify firewall or interface configuration.

The physical host profile is secret-free and references an already initialized
host role root. A paired root reconnects normally; an explicitly selected fresh
unpaired root may be launched to provide the pairing endpoint described above.
Creating a profile does not initialize, copy, pair, or repair identity state.
Launch remains foreground-only and stop is bound to the observed owner
generation.

## Diagnostics

The USB-connected host runner accepts only allowlisted device records and
publishes bounded local diagnostic runs. Authentication strings, credentials,
keys, addresses, protocol bytes, payloads, paths, NVS contents, and machine or
user identity are forbidden diagnostic fields. Device and host stores are each
capped at 16 MiB. Recording failure or runner absence does not change device
behavior or command results.

## Zephyr Rust patch series

Deskkin applies an ordered local patch series to `zephyr-lang-rust` commit
`dd73abc242e995784da62352fe8c70d9a6c7ac2e`. Bootstrap verifies that each patch
applies or is already applied.

1. `0001-map-esp32s3-xtensa-target.patch` maps the ESP32-S3 configuration to
   `xtensa-esp32s3-none-elf`.
2. `0002-enable-esp32s3-xtensa-kconfig.patch` permits Rust integration for the
   ESP32-S3 SoC series.
3. `0003-build-xtensa-core-from-source.patch` builds `core` and `alloc` for the
   Tier 3 Xtensa target.
4. `0004-recognize-esp32-flash-controller.patch` augments the ESP32 flash
   controller so fixed-partition bindings use a generated raw-device accessor.
5. `0005-use-fixed-width-kconfig-integers.patch` emits numeric Kconfig values as
   `u64` and `i64`, preserving values such as ESP32-S3 GPIO masks on a 32-bit
   target.

Remove a patch only when the pinned or deliberately upgraded upstream module
provides equivalent behavior and the clean product and inert builds pass
without it. Toolchain, module, scheduler, calling-convention, driver, or Slint
changes require an explicit scoped dependency update and full CoreS3
conformance.

No upstream issue or pull request is implied by this repository. Filing one is
a separate remote state change.
