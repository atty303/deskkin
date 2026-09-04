#!/usr/bin/env bash
set -euo pipefail

repo_root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd -P)
state_dir=${DESKKIN_STATE_DIR:-"$repo_root/.deskkin"}
venv_dir="$state_dir/venv"
manifest_dir="$state_dir/manifest"
sdk_dir="$state_dir/sdk"
downloads_dir="$state_dir/downloads"
rustup_dir="$state_dir/rustup"
export_file="$state_dir/esp-export.sh"

sdk_version=1.0.1
release_url="https://github.com/zephyrproject-rtos/sdk-ng/releases/download/v${sdk_version}"
minimal_name="zephyr-sdk-${sdk_version}_linux-x86_64_minimal.tar.xz"
minimal_digest=ca9bc0ff66fafca1dac9d592a36d953cf16d096a9d09b1c0357f021cf9f6a7eb
xtensa_name=toolchain_gnu_linux-x86_64_xtensa-espressif_esp32s3_zephyr-elf.tar.xz
xtensa_digest=904d42b75e4d819c58b0db640783911e508b9ee2a627eb5daffb907465a34be1
toolchain_name=deskkin-esp

case ${1:-install} in
  install)
    ;;
  --verify-only)
    verify_only=1
    ;;
  *)
    printf 'usage: %s [--verify-only]\n' "$0" >&2
    exit 2
    ;;
esac
verify_only=${verify_only:-0}

download_verified() {
  local name=$1
  local digest=$2
  local destination="$downloads_dir/$name"
  "$venv_dir/bin/python" - "$release_url/$name" "$destination" "$digest" <<'PY'
import hashlib
import pathlib
import sys
import tempfile
import urllib.request

url, destination_text, expected = sys.argv[1:]
destination = pathlib.Path(destination_text)
if destination.exists() and hashlib.sha256(destination.read_bytes()).hexdigest() == expected:
    raise SystemExit(0)
temporary = None
try:
    with tempfile.NamedTemporaryFile(dir=destination.parent, delete=False) as stream:
        temporary = pathlib.Path(stream.name)
        with urllib.request.urlopen(url, timeout=60) as response:
            while chunk := response.read(1024 * 1024):
                stream.write(chunk)
    actual = hashlib.sha256(temporary.read_bytes()).hexdigest()
    if actual != expected:
        raise SystemExit(f"SDK digest mismatch for {destination.name}: {actual}")
    temporary.chmod(0o600)
    temporary.replace(destination)
    temporary = None
finally:
    if temporary is not None:
        temporary.unlink(missing_ok=True)
PY
}

validate_installed_sdk() {
  "$venv_dir/bin/python" - "$sdk_dir" "$repo_root/requirements/core-s3-sdk.json" <<'PY' || return 1
import hashlib
import json
import pathlib
import subprocess
import sys

sdk = pathlib.Path(sys.argv[1])
manifest = json.loads(pathlib.Path(sys.argv[2]).read_text(encoding="utf-8"))
for relative, expected in manifest["files"].items():
    path = sdk / relative
    if not path.is_file():
        raise SystemExit(f"CoreS3 SDK file missing: {relative}")
    actual = hashlib.sha256(path.read_bytes()).hexdigest()
    if actual != expected:
        raise SystemExit(f"CoreS3 SDK file digest mismatch: {relative}")
for relative, expected in manifest["host_tools"].items():
    path = sdk / relative
    if not path.is_file():
        raise SystemExit(f"CoreS3 SDK host tool missing: {relative}")
    try:
        result = subprocess.run(
            [path, "--version"],
            check=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.STDOUT,
            text=True,
            timeout=10,
        )
    except (OSError, subprocess.SubprocessError) as error:
        raise SystemExit(f"CoreS3 SDK host tool failed: {relative}: {type(error).__name__}") from error
    if expected not in result.stdout:
        raise SystemExit(f"CoreS3 SDK host tool version mismatch: {relative}")
PY
}

verify_product_installation() {
  "$venv_dir/bin/python" - "$state_dir" <<'PY'
import hashlib
import pathlib
import subprocess
import sys

state = pathlib.Path(sys.argv[1])
revisions = {
    "west/zephyr": "1f6485eca25431b5ff27ce9a754218c9e559bbbb",
    "west/modules/lang/rust": "dd73abc242e995784da62352fe8c70d9a6c7ac2e",
    "west/modules/hal/cmsis": "512cc7e895e8491696b61f7ba8066b4a182569b8",
    "west/modules/hal/cmsis_6": "30a859f44ef8ab4dc8f84b03ed586fd16ccf9d74",
    "west/modules/hal/espressif": "19f979cfe66bcab09abe3b0b3aa419a664c1606c",
    "west/modules/hal/xtensa": "0495a1afd300b644d3ec8dd2c3bd11007e69a892",
    "west/modules/crypto/mbedtls": "a3e190fe44c78d1ba67f55979e1257328cc7d0d8",
    "west/modules/crypto/tf-psa-crypto": "dc575a2ddcc8cb16275d24c42a52eaf79ebe2231",
    "west/bootloader/mcuboot": "6d3b3d2c38ab20c242e5b9abb04d050086383eb2",
}
rust_module = "west/modules/lang/rust"
zephyr_module = "west/zephyr"
espressif_module = "west/modules/hal/espressif"
expected_rust_status = [
    " M CMakeLists.txt",
    " M Kconfig",
    " M dt-rust.yaml",
    " M zephyr-build/src/lib.rs",
]
expected_zephyr_status = [
    " M drivers/dma/dma_esp32_gdma.c",
    " M drivers/spi/spi_esp32_spim.c",
    " M drivers/wifi/esp32/src/esp_wifi_drv.c",
    " M soc/espressif/common/esp_psram.c",
    " M soc/espressif/common/loader.c",
    " M soc/espressif/esp32s3/default.ld",
    " M soc/espressif/esp32s3/default_appcpu.ld",
    " M soc/espressif/esp32s3/esp32s3-mp.c",
    " M soc/espressif/esp32s3/memory.h",
    " M soc/espressif/esp32s3/soc_appcpu.c",
]
expected_espressif_status = [
    " M components/esp_coex/esp32s3/esp_coex_adapter.c",
    " M components/esp_timer/src/esp_timer.c",
    " M components/esp_wifi/esp32s3/esp_adapter.c",
]
for relative, expected in revisions.items():
    project = state / relative
    actual = subprocess.run(
        ["git", "-C", project, "rev-parse", "HEAD"],
        check=True,
        capture_output=True,
        text=True,
    ).stdout.strip()
    if actual != expected:
        raise SystemExit(f"CoreS3 west revision mismatch: {relative}")
    status = subprocess.run(
        ["git", "-C", project, "status", "--porcelain"],
        check=True,
        capture_output=True,
        text=True,
    ).stdout.splitlines()
    if relative == rust_module:
        expected_status = expected_rust_status
    elif relative == zephyr_module:
        expected_status = expected_zephyr_status
    elif relative == espressif_module:
        expected_status = expected_espressif_status
    else:
        expected_status = []
    if status != expected_status:
        raise SystemExit(f"CoreS3 west tree mismatch: {relative}")

patch_diff = subprocess.run(
    ["git", "-C", state / rust_module, "diff", "--binary", "HEAD"],
    check=True,
    capture_output=True,
).stdout
if hashlib.sha256(patch_diff).hexdigest() != "3a16ecd15058a4ceb80245fcc0ba5ef89087183b428f718c8ce8890e5559f186":
    raise SystemExit("CoreS3 Rust patch series mismatch")

zephyr_patch_diff = subprocess.run(
    ["git", "-C", state / zephyr_module, "diff", "--binary", "HEAD"],
    check=True,
    capture_output=True,
).stdout
if hashlib.sha256(zephyr_patch_diff).hexdigest() != "7220921d88fd6445dad14b6c9b63351e9c4d17237bf353f19e80474e269cbedb":
    raise SystemExit("CoreS3 Zephyr patch series mismatch")

espressif_patch_diff = subprocess.run(
    ["git", "-C", state / espressif_module, "diff", "--binary", "HEAD"],
    check=True,
    capture_output=True,
).stdout
if hashlib.sha256(espressif_patch_diff).hexdigest() != "c89390b05dc714673494b8c6b3527dc3b70dc3376aff1e4b72cd12501ebdb0b3":
    raise SystemExit("CoreS3 Espressif HAL patch series mismatch")

toolchain = state / "rustup/toolchains/deskkin-esp"
tools = {
    toolchain / "bin/rustc": "fb6469add601520c44d68115ad4ff137985b1ff320e001a471ef35d786df51fd",
    toolchain / "bin/cargo": "f9f150db83d4b06a9da0a0dd1c1736048efba7edc5ada70c1b7972efe2539983",
    toolchain / "xtensa-esp32-elf-clang/esp-20.1.1_20250829/esp-clang/lib/libclang.so": "12e2f4e8e3fb62ce00e0cba30ddd9cef84935a899f040beb0c0a226940d73d8b",
}
for path, expected in tools.items():
    try:
        actual = hashlib.sha256(path.read_bytes()).hexdigest()
    except OSError as error:
        raise SystemExit(f"CoreS3 tool missing: {path.name}") from error
    if actual != expected:
        raise SystemExit(f"CoreS3 tool digest mismatch: {path.name}")

rust_version = subprocess.run(
    [toolchain / "bin/rustc", "-vV"],
    check=True,
    capture_output=True,
    text=True,
).stdout
if "commit-hash: 95e5bda868c960c607597bc03ed9e8f0ad26226d" not in rust_version:
    raise SystemExit("CoreS3 Rust toolchain mismatch")
PY
}

if [[ "$verify_only" == 1 ]]; then
  if [[ ! -x "$venv_dir/bin/python" ]]; then
    printf 'CoreS3 bootstrap required\n' >&2
    exit 2
  fi
  validate_installed_sdk
  verify_product_installation
  exit 0
fi

mkdir -p "$state_dir" "$downloads_dir"
chmod 700 "$state_dir" "$downloads_dir"

if [[ ! -x "$venv_dir/bin/python" ]]; then
  python -m venv "$venv_dir"
fi
"$venv_dir/bin/python" -m pip install --disable-pip-version-check --require-hashes -r "$repo_root/requirements/core-s3.lock"

mkdir -p "$manifest_dir"
install -m 600 "$repo_root/west.yml" "$manifest_dir/west.yml"
if [[ ! -d "$manifest_dir/.git" ]]; then
  git -C "$manifest_dir" init -q
fi
if [[ ! -f "$state_dir/.west/config" ]]; then
  "$venv_dir/bin/west" init -l "$manifest_dir"
fi
(
  cd "$state_dir"
  "$venv_dir/bin/west" update
)

download_verified "$minimal_name" "$minimal_digest"
download_verified "$xtensa_name" "$xtensa_digest"

if ! validate_installed_sdk; then
  stage=$(mktemp -d "$state_dir/core-s3-sdk-stage.XXXXXX")
  trap 'rm -rf -- "${stage:-}"' EXIT
  tar -xJf "$downloads_dir/$minimal_name" -C "$stage"
  mv "$stage/zephyr-sdk-${sdk_version}" "$stage/sdk"
  mkdir -p "$stage/sdk/gnu"
  tar -xJf "$downloads_dir/$xtensa_name" -C "$stage/sdk/gnu"
  extracted="$stage/sdk/gnu/xtensa-espressif_esp32s3_zephyr-elf"
  if [[ -x "$extracted/bin/xtensa-espressif_esp32s3-zephyr-elf-gcc" ]]; then
    mv "$extracted/bin/xtensa-espressif_esp32s3-zephyr-elf-gcc" "$extracted/bin/xtensa-espressif_esp32s3_zephyr-elf-gcc"
  fi
  if [[ -e "$sdk_dir" ]]; then
    mv "$sdk_dir" "$stage/previous-sdk"
  fi
  if ! mv "$stage/sdk" "$sdk_dir"; then
    if [[ -e "$stage/previous-sdk" ]]; then
      mv "$stage/previous-sdk" "$sdk_dir"
    fi
    exit 2
  fi
  if ! "$sdk_dir/hosttools/zephyr-sdk-x86_64-hosttools-standalone-0.10.sh" -y -d "$sdk_dir" || ! validate_installed_sdk; then
    mv "$sdk_dir" "$stage/failed-sdk"
    if [[ -e "$stage/previous-sdk" ]]; then
      mv "$stage/previous-sdk" "$sdk_dir"
    fi
    exit 2
  fi
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

module="$state_dir/west/zephyr"
zephyr_patch_digest=$(git -C "$module" diff --binary HEAD | sha256sum | cut -d ' ' -f 1)
case "$zephyr_patch_digest" in
  7220921d88fd6445dad14b6c9b63351e9c4d17237bf353f19e80474e269cbedb)
    zephyr_patches_current=1
    ;;
  50936b1ab6c554f7777a57f69bf92d5c922213736195f936dfdfd4a88a7440a3)
    relative=drivers/spi/spi_esp32_spim.c
    git -C "$module" show "HEAD:$relative" > "$module/$relative.migrating"
    mv "$module/$relative.migrating" "$module/$relative"
    ;;
  69df8e02eee5caf343859f0bfad651ac1f3d0605c5b8c735bb1985cbc543bd6d|ade32e58926d12ea981d512c6d4adb92812f55fda603d98a174fd146b6e4adf8|bf0b6c23ddf842be5bc5bc82ebfa2a0632150ef71915acba81288047da52fc32)
    for relative in drivers/spi/spi_esp32_spim.c soc/espressif/esp32s3/soc_appcpu.c; do
      git -C "$module" show "HEAD:$relative" > "$module/$relative.migrating"
      mv "$module/$relative.migrating" "$module/$relative"
    done
    ;;
  e7d258c56fd6fb412f5ffa0cfaba51ecc37c781d16385b0d2a71623463197871|2aa1a66261802c19f97df062bcff61b9781d4d42caa5599edb2f2ab7ebdf3dab|84b30f982fb801c945c84f9bd0aec4f91879268cf887924956dbb657d2fa3aec)
    for relative in drivers/spi/spi_esp32_spim.c drivers/display/display_ili9xxx.c drivers/mipi_dbi/mipi_dbi_esp32.c include/zephyr/drivers/mipi_dbi.h soc/espressif/esp32s3/soc_appcpu.c; do
      git -C "$module" show "HEAD:$relative" > "$module/$relative.migrating"
      mv "$module/$relative.migrating" "$module/$relative"
    done
    ;;
  8737d55925bcbcdd958bcb782ba771145ca8a46f560cd8bd4ea8223dfb009369)
    for relative in drivers/spi/spi_esp32_spim.c drivers/display/display_ili9xxx.c drivers/mipi_dbi/mipi_dbi_esp32.c include/zephyr/drivers/mipi_dbi.h soc/espressif/esp32s3/soc_appcpu.c; do
      git -C "$module" show "HEAD:$relative" > "$module/$relative.migrating"
      mv "$module/$relative.migrating" "$module/$relative"
    done
    ;;
  9a2d87fff34e6017b9e25e1e1a21b3c0d29898b185eae68a20e8e2f3e8e570a7)
    for relative in drivers/spi/spi_esp32_spim.c arch/xtensa/core/window_vectors.S soc/espressif/esp32s3/soc_appcpu.c; do
      git -C "$module" show "HEAD:$relative" > "$module/$relative.migrating"
      mv "$module/$relative.migrating" "$module/$relative"
    done
    ;;
esac
if [[ ${zephyr_patches_current:-0} != 1 ]]; then
  for patch_file in "$repo_root"/patches/zephyr-core-s3/*.patch; do
    if ! git -C "$module" apply --reverse --check "$patch_file" >/dev/null 2>&1; then
      git -C "$module" apply --check "$patch_file"
      git -C "$module" apply "$patch_file"
    fi
  done
fi

module="$state_dir/west/modules/hal/espressif"
espressif_patch_digest=$(git -C "$module" diff --binary HEAD | sha256sum | cut -d ' ' -f 1)
if [[ "$espressif_patch_digest" == d6fd76888b7bb78e21d10ad4cae24f5fe9e6437f113fbb6a0fc187be4eb62836 ||
      "$espressif_patch_digest" == 8f8ef95c8e3f66ea869ae409d939b3ebd6e649f18e9d108473666bf36b1a4926 ||
      "$espressif_patch_digest" == 014e8cf2d647998279d90dc20d20910be0f8f38435643d63c015dab214336f3e ||
      "$espressif_patch_digest" == e40fd7bef86deb194ba5a6783b4f60cbada49bea547b5a66e7f3724c40012d79 ]]; then
  for relative in components/esp_coex/esp32s3/esp_coex_adapter.c components/esp_timer/src/esp_timer.c components/esp_wifi/esp32s3/esp_adapter.c; do
    git -C "$module" show "HEAD:$relative" > "$module/$relative.migrating"
    mv "$module/$relative.migrating" "$module/$relative"
  done
  espressif_patch_digest=migrating
fi
if [[ "$espressif_patch_digest" != c89390b05dc714673494b8c6b3527dc3b70dc3376aff1e4b72cd12501ebdb0b3 ]]; then
  for patch_file in "$repo_root"/patches/hal-espressif-core-s3/*.patch; do
    if ! git -C "$module" apply --reverse --check "$patch_file" >/dev/null 2>&1; then
      git -C "$module" apply --check "$patch_file"
      git -C "$module" apply "$patch_file"
    fi
  done
fi

(
  cd -- "$state_dir"
  venv/bin/west blobs fetch hal_espressif
)

validate_installed_sdk
verify_product_installation

for project in zephyr zephyr-lang-rust cmsis cmsis_6 hal_espressif mcuboot; do
  (cd "$state_dir" && "$venv_dir/bin/west" list "$project" -f '{name} {revision} {sha}')
done
"$rustup_dir/toolchains/$toolchain_name/bin/rustc" --version
"$sdk_dir/gnu/xtensa-espressif_esp32s3_zephyr-elf/bin/xtensa-espressif_esp32s3_zephyr-elf-gcc" --version | head -1
printf 'CoreS3 bootstrap complete: %s\n' "$state_dir"
