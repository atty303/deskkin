# CoreS3 Zephyr Rust patch series

Deskkin keeps an ordered local patch series against `zephyr-lang-rust` commit
`dd73abc242e995784da62352fe8c70d9a6c7ac2e`. It does not create a remote fork
or change another maintained module. The repository bootstrap verifies that
each patch applies, or is already applied, before the CoreS3 build runs.

No upstream issue or pull request has been created. Opening either is a remote
state change and requires separate approval. The patches were promoted from
the completed Gate 1C evidence into the current CoreS3 product build. The
upstream strategy remains to split the series by concern and add the smallest
maintainer-accepted ESP32-S3 build test before removing the local copies.

## Retained patches

1. `0001-map-esp32s3-xtensa-target.patch` maps Zephyr's ESP32-S3 Xtensa
   configuration to `xtensa-esp32s3-none-elf`. Remove it when the pinned or an
   approved later module revision supplies the mapping.
2. `0002-enable-esp32s3-xtensa-kconfig.patch` permits the Rust integration for
   that one SoC series. Remove it when upstream support includes ESP32-S3 in
   its supported-target predicate.
3. `0003-build-xtensa-core-from-source.patch` uses nightly Cargo's
   `-Zbuild-std=core,alloc` only for Xtensa because the Tier 3 toolchain does
   not ship those target libraries. Remove it when the selected toolchain
   supplies the libraries or upstream selects an equivalent build path.
4. `0004-recognize-esp32-flash-controller.patch` adds
   `espressif,esp32-flash-controller` to the existing flash-controller
   augmentation. Without it, generated fixed partitions call a raw-device
   accessor that is never generated. Remove it when the compatible value is
   upstream.
5. `0005-use-fixed-width-kconfig-integers.patch` emits numeric Kconfig values
   as `u64` and `i64`, preventing valid ESP32-S3 GPIO masks from overflowing a
   32-bit Rust `usize`. Remove it when upstream derives Kconfig types or uses a
   fixed-width representation that retains those values.

The first three patches establish the compiler target and build path. The last
two are narrowly demonstrated binding-generation fixes discovered by the
CoreS3 build. Gate 1C physical diagnostic run
`015a39fd-7191-45ef-8ca5-4ea5681d8514` supplied the original serial oracle.
The maintained proof is now the clean `apps/core-s3-device` build plus the
physical Phase 3P qualification recorded in
[`phase-3p-physical-qualification.md`](phase-3p-physical-qualification.md).

## Maintenance boundary

Remove a patch when the pinned or an approved later `zephyr-lang-rust` revision
provides the equivalent behavior and the clean CoreS3 build passes without it.
Any required scheduler, calling-convention, Rust compiler, driver-subsystem,
Slint, or second maintained-module change remains a new architecture decision.
