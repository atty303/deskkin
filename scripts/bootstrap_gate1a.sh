#!/usr/bin/env bash
set -euo pipefail

repo_root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd -P)
state_dir="$repo_root/.deskkin"
venv_dir="$state_dir/venv"
manifest_dir="$state_dir/manifest"
sdk_dir="$state_dir/sdk"
downloads_dir="$state_dir/downloads"

sdk_version=1.0.1
release_url="https://github.com/zephyrproject-rtos/sdk-ng/releases/download/v${sdk_version}"
minimal_name="zephyr-sdk-${sdk_version}_linux-x86_64_minimal.tar.xz"
arm_name="toolchain_gnu_linux-x86_64_arm-zephyr-eabi.tar.xz"
riscv_name="toolchain_gnu_linux-x86_64_riscv64-zephyr-elf.tar.xz"
require_qemu=${DESKKIN_GATE1A_REQUIRE_QEMU:-1}

if [[ "$require_qemu" != 0 && "$require_qemu" != 1 ]]; then
  printf 'DESKKIN_GATE1A_REQUIRE_QEMU must be 0 or 1\n' >&2
  exit 2
fi

mkdir -p "$state_dir" "$downloads_dir"
chmod 700 "$state_dir" "$downloads_dir"

if [[ ! -x "$venv_dir/bin/python" ]]; then
  python -m venv "$venv_dir"
fi
"$venv_dir/bin/python" -m pip install --disable-pip-version-check --require-hashes -r "$repo_root/requirements/gate1a.lock"

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

download_verified "$minimal_name" ca9bc0ff66fafca1dac9d592a36d953cf16d096a9d09b1c0357f021cf9f6a7eb
download_verified "$arm_name" 21b85981cb5a1818d9bc53d82af80f208946ec038b982ff1907287572ed3a634
download_verified "$riscv_name" 01750834c471fbdb335c1b8b8aee17010a1968938957db85640c366235771a38

if [[ ! -x "$sdk_dir/gnu/arm-zephyr-eabi/bin/arm-zephyr-eabi-gcc" || ! -x "$sdk_dir/gnu/riscv64-zephyr-elf/bin/riscv64-zephyr-elf-gcc" || ( "$require_qemu" == 1 && ( ! -x "$sdk_dir/sysroots/x86_64-pokysdk-linux/usr/bin/qemu-system-arm" || ! -x "$sdk_dir/sysroots/x86_64-pokysdk-linux/usr/bin/qemu-system-riscv32" ) ) ]]; then
  stage=$(mktemp -d "$state_dir/sdk-stage.XXXXXX")
  trap 'rm -rf -- "$stage"' EXIT
  tar -xJf "$downloads_dir/$minimal_name" -C "$stage"
  mv "$stage/zephyr-sdk-${sdk_version}" "$stage/sdk"
  mkdir -p "$stage/sdk/gnu"
  tar -xJf "$downloads_dir/$arm_name" -C "$stage/sdk/gnu"
  tar -xJf "$downloads_dir/$riscv_name" -C "$stage/sdk/gnu"
  "$stage/sdk/hosttools/zephyr-sdk-x86_64-hosttools-standalone-0.10.sh" -y -d "$stage/sdk"
  "$venv_dir/bin/python" - "$stage/sdk" "$repo_root/requirements/gate1a-sdk.json" "$require_qemu" <<'PY'
import hashlib
import json
import pathlib
import sys

sdk = pathlib.Path(sys.argv[1])
manifest = json.loads(pathlib.Path(sys.argv[2]).read_text(encoding="utf-8"))
require_qemu = sys.argv[3] == "1"
for relative, expected in manifest["files"].items():
    if not require_qemu and pathlib.PurePosixPath(relative).name.startswith("qemu-system-"):
        continue
    path = sdk / relative
    if not path.is_file() or hashlib.sha256(path.read_bytes()).hexdigest() != expected:
        raise SystemExit(f"staged Gate 1A SDK validation failed: {relative}")
PY
  if [[ -e "$sdk_dir" ]]; then
    mv "$sdk_dir" "$stage/previous-sdk"
  fi
  if ! mv "$stage/sdk" "$sdk_dir"; then
    if [[ -e "$stage/previous-sdk" ]]; then
      mv "$stage/previous-sdk" "$sdk_dir"
    fi
    exit 2
  fi
fi

"$venv_dir/bin/python" - "$sdk_dir" "$repo_root/requirements/gate1a-sdk.json" "$require_qemu" <<'PY'
import hashlib
import json
import pathlib
import sys

sdk = pathlib.Path(sys.argv[1])
manifest = json.loads(pathlib.Path(sys.argv[2]).read_text(encoding="utf-8"))
require_qemu = sys.argv[3] == "1"
for relative, expected in manifest["files"].items():
    if not require_qemu and pathlib.PurePosixPath(relative).name.startswith("qemu-system-"):
        continue
    path = sdk / relative
    if not path.is_file():
        raise SystemExit(f"Gate 1A SDK file missing: {relative}")
    actual = hashlib.sha256(path.read_bytes()).hexdigest()
    if actual != expected:
        raise SystemExit(f"Gate 1A SDK file digest mismatch: {relative}")
PY

for project in zephyr zephyr-lang-rust cmsis cmsis_6; do
  (cd "$state_dir" && "$venv_dir/bin/west" list "$project" -f '{name} {revision} {sha}')
done

printf 'Gate 1A bootstrap complete: %s\n' "$state_dir"
