# Gate 1C local patch series

Gate 1C keeps an ordered local patch series against `zephyr-lang-rust` commit
`dd73abc242e995784da62352fe8c70d9a6c7ac2e`. It does not create a remote fork
or change another maintained module. The repository bootstrap verifies that
each patch applies, or is already applied, before the gate runs.

No upstream issue or pull request has been created. Opening either is a remote
state change and requires separate approval. Gate 1C passed on hardware; the
upstream strategy remains to split the series by concern, add the smallest
maintainer-accepted ESP32-S3 build test, and obtain issue or pull-request URLs
before these patches are considered for use beyond this local feasibility
spike.

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
CoreS3 build. The Gate 1C sample and runner are the local proof: they compile a
Rust `no_std` image, link bidirectional C ABI calls, exercise the selected
critical-section implementation, and build normal and deliberate-panic
firmware. Physical diagnostic run
`015a39fd-7191-45ef-8ca5-4ea5681d8514` supplied the final serial oracle for
boot, bidirectional ABI values, nested critical-section and interrupt-state
restoration, allocation/free, deliberate panic, and normal inert-idle cleanup.

## Stop boundary

Discard this series, the Gate 1C sample, `espup`, and the repository-local ESP
toolchain if the physical gate fails and cannot be repaired within the approved
target mapping, compiler/link arguments, cfg/atomic/binding, or
compiler-builtins boundary. Any required scheduler, calling-convention, Rust
compiler, driver-subsystem, Slint, or second maintained-module change stops the
gate for a new architecture decision.
