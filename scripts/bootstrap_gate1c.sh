#!/usr/bin/env bash
set -euo pipefail

repo_root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd -P)
state_dir="$repo_root/.deskkin"
sdk_dir="$state_dir/sdk"
downloads_dir="$state_dir/downloads"
rustup_dir="$state_dir/rustup"
export_file="$state_dir/esp-export.sh"
sdk_version=1.0.1
toolchain_name=deskkin-esp
xtensa_archive=toolchain_gnu_linux-x86_64_xtensa-espressif_esp32s3_zephyr-elf.tar.xz
xtensa_digest=904d42b75e4d819c58b0db640783911e508b9ee2a627eb5daffb907465a34be1

DESKKIN_GATE1A_REQUIRE_QEMU=0 "$repo_root/scripts/bootstrap_gate1a.sh"
"$state_dir/venv/bin/python" -m pip install --disable-pip-version-check --require-hashes -r "$repo_root/requirements/core-s3.lock"

download="$downloads_dir/$xtensa_archive"
if [[ ! -f "$download" ]] || ! printf '%s  %s\n' "$xtensa_digest" "$download" | sha256sum --check --status; then
  temporary=$(mktemp "$downloads_dir/gate1c-download.XXXXXX")
  trap 'rm -f -- "${temporary:-}"' EXIT
  python - "$download" "$temporary" "$xtensa_digest" <<'PY'
import hashlib
import pathlib
import sys
import urllib.request

destination = pathlib.Path(sys.argv[1])
temporary = pathlib.Path(sys.argv[2])
expected = sys.argv[3]
with urllib.request.urlopen(
    "https://github.com/zephyrproject-rtos/sdk-ng/releases/download/v1.0.1/" + destination.name,
    timeout=60,
) as response, temporary.open("wb") as stream:
    while chunk := response.read(1024 * 1024):
        stream.write(chunk)
if hashlib.sha256(temporary.read_bytes()).hexdigest() != expected:
    raise SystemExit("Gate 1C SDK archive digest mismatch")
temporary.chmod(0o600)
temporary.replace(destination)
PY
fi

xtensa_root="$sdk_dir/gnu/xtensa-espressif_esp32s3_zephyr-elf"
xtensa_gcc="$xtensa_root/bin/xtensa-espressif_esp32s3_zephyr-elf-gcc"
if [[ -d "$sdk_dir/xtensa-espressif_esp32s3_zephyr-elf" && ! -e "$xtensa_root" ]]; then
  mkdir -p "$sdk_dir/gnu"
  mv "$sdk_dir/xtensa-espressif_esp32s3_zephyr-elf" "$xtensa_root"
fi
if [[ ! -x "$xtensa_gcc" ]]; then
  stage=$(mktemp -d "$state_dir/gate1c-sdk-stage.XXXXXX")
  trap 'rm -rf -- "${stage:-}"; rm -f -- "${temporary:-}"' EXIT
  tar -xJf "$download" -C "$stage"
  extracted="$stage/xtensa-espressif_esp32s3_zephyr-elf"
  [[ -x "$extracted/bin/xtensa-espressif_esp32s3-zephyr-elf-gcc" ]] && mv "$extracted/bin/xtensa-espressif_esp32s3-zephyr-elf-gcc" "$extracted/bin/xtensa-espressif_esp32s3_zephyr-elf-gcc"
  mkdir -p "$sdk_dir/gnu"
  mv "$extracted" "$xtensa_root"
fi

mkdir -p "$rustup_dir"
chmod 700 "$rustup_dir"
if [[ ! -x "$rustup_dir/toolchains/$toolchain_name/bin/rustc" ]]; then
  RUSTUP_HOME="$rustup_dir" espup install --name "$toolchain_name" --toolchain-version 1.95.0.0 --targets esp32s3 --export-file "$export_file"
fi

module="$state_dir/west/modules/lang/rust"
for patch_file in "$repo_root"/patches/zephyr-lang-rust-core-s3/*.patch; do
  if ! git -C "$module" apply --reverse --check "$patch_file" >/dev/null 2>&1; then
    git -C "$module" apply --check "$patch_file"
    git -C "$module" apply "$patch_file"
  fi
done

(
  cd -- "$state_dir"
  venv/bin/west blobs fetch hal_espressif
)

"$rustup_dir/toolchains/$toolchain_name/bin/rustc" --version
"$xtensa_gcc" --version | head -1
printf 'Gate 1C bootstrap complete: %s\n' "$state_dir"
