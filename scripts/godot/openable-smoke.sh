#!/usr/bin/env bash
set -euo pipefail
# deny-network.sh enforces the platform equivalent of macOS deny network*.

script_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)"
repository_root="$(cd -- "${script_dir}/../.." && pwd -P)"
project_root="${repository_root}/apps/mornlea-godot"

case "${1:-}" in
  --without-native|--without-python) mode="$1" ;;
  *)
    printf 'usage: scripts/godot/openable-smoke.sh --without-native|--without-python\n' >&2
    exit 2
    ;;
esac
if (($# != 1)); then
  printf 'usage: scripts/godot/openable-smoke.sh --without-native|--without-python\n' >&2
  exit 2
fi

case "$(uname -s):$(uname -m)" in
  Darwin:arm64) expected_target="aarch64-apple-darwin"; runtime_family="darwin64"; platform_dir="macos-universal"; suffix="dylib" ;;
  Darwin:x86_64) expected_target="x86_64-apple-darwin"; runtime_family="darwin64"; platform_dir="macos-universal"; suffix="dylib" ;;
  Linux:x86_64) expected_target="x86_64-unknown-linux-gnu"; runtime_family="linux64"; platform_dir="linux-x86_64"; suffix="so" ;;
  *) printf 'unsupported desktop architecture: %s-%s\n' "$(uname -s)" "$(uname -m)" >&2; exit 1 ;;
esac

temporary_root="$(mktemp -d "${TMPDIR:-/tmp}/mornlea-godot-openable.XXXXXX")"
temporary_project="${temporary_root}/project"
cleanup() {
  rm -rf -- "${temporary_root}"
}
trap cleanup EXIT

mkdir -p -- "${temporary_project}"
tar -C "${project_root}" --exclude='./.godot' --exclude='./.venv' \
  --exclude='./addons/py4godot' --exclude='./addons/mornlea_bridge/bin' -cf - . | \
  tar -C "${temporary_project}" -xf -
mkdir -p -- "${temporary_project}/addons/mornlea_bridge"
touch "${temporary_project}/addons/mornlea_bridge/.gdignore"

python_addon="${temporary_project}/addons/py4godot"
python_runtime="${python_addon}/cpython-3.14.4-${runtime_family}/python"
bridge_library="${temporary_project}/addons/mornlea_bridge/bin/${platform_dir}/debug/libmornlea_godot.${suffix}"

materialize_python_placeholders() {
  mkdir -p -- "${python_runtime}/bin" "${python_runtime}/lib/python3.14"
  cp -- "${script_dir}/fixtures/python-extension-identity.gdextension" \
    "${python_addon}/python.gdextension"
  touch "${python_addon}/.gdignore" \
    "${python_runtime}/bin/pythonscript.${suffix}" \
    "${python_runtime}/bin/main.${suffix}" \
    "${python_runtime}/lib/libpython3.14.${suffix}" \
    "${python_runtime}/lib/python3.14/os.py"
}

materialize_bridge_placeholder() {
  mkdir -p -- "$(dirname -- "${bridge_library}")"
  touch "${bridge_library}" \
    "$(dirname -- "${bridge_library}")/libmornlea_client_core.${suffix}" \
    "$(dirname -- "${bridge_library}")/libmornlea_engine.${suffix}"
  [[ "${runtime_family}" != darwin64 ]] || touch "$(dirname -- "${bridge_library}")/libmornlea_client.dylib"
}

run_bootstrap() {
  if ! output="$("${script_dir}/deny-network.sh" \
    "${script_dir}/godot.sh" --headless --path "${temporary_project}" --quit-after 2 2>&1)"; then
    printf '%s\n' "${output}" >&2
    exit 1
  fi
}

assert_output() {
  local expected="$1"
  if [[ "${output}" != *"${expected}"* ]]; then
    printf 'Godot openable smoke output is missing %q:\n%s\n' "${expected}" "${output}" >&2
    exit 1
  fi
}

assert_common_output() {
  assert_output "[mornlea-bootstrap] state=setup-required"
  assert_output "target=${expected_target}"
  assert_output "prepare=scripts/godot/build-python-runtime.sh --verify --offline && scripts/godot/build-core.sh --verify && scripts/godot/build-extension.sh --target ${expected_target} --profile debug --verify"
  assert_output "python=not-imported"
  assert_output "network=not-started"
}

case "${mode}" in
  --without-native)
    materialize_python_placeholders
    run_bootstrap
    assert_common_output
    assert_output "missing=project-bridge-library@res://addons/mornlea_bridge/bin/${platform_dir}/debug/libmornlea_godot.${suffix}"
    assert_output "mismatched=none"

    materialize_bridge_placeholder
    sed 's/entry_symbol = "gdext_rust_init"/entry_symbol = "wrong_bridge_entry"/' \
      "${temporary_project}/addons/mornlea_bridge/mornlea_bridge.gdextension" > "${temporary_root}/descriptor"
    mv -- "${temporary_root}/descriptor" "${temporary_project}/addons/mornlea_bridge/mornlea_bridge.gdextension"
    run_bootstrap
    assert_common_output
    assert_output "missing=none"
    assert_output "mismatched=project-bridge-entry-symbol"
    ;;
  --without-python)
    materialize_bridge_placeholder
    run_bootstrap
    assert_common_output
    assert_output "missing=python-extension@res://addons/py4godot/python.gdextension,python-plugin@res://addons/py4godot/cpython-3.14.4-${runtime_family}/python/bin/pythonscript.${suffix},python-bridge@res://addons/py4godot/cpython-3.14.4-${runtime_family}/python/bin/main.${suffix},python-interpreter@res://addons/py4godot/cpython-3.14.4-${runtime_family}/python/lib/libpython3.14.${suffix},python-stdlib@res://addons/py4godot/cpython-3.14.4-${runtime_family}/python/lib/python3.14/os.py"
    assert_output "mismatched=none"

    materialize_python_placeholders
    sed 's/version = "4.7-alpha-21"/version = "wrong-version"/' \
      "${python_addon}/python.gdextension" > "${temporary_root}/descriptor"
    mv -- "${temporary_root}/descriptor" "${python_addon}/python.gdextension"
    run_bootstrap
    assert_common_output
    assert_output "missing=none"
    assert_output "mismatched=python-extension-version"
    ;;
esac

printf 'Godot Bootstrap reports %s dependency and identity diagnostics without Python feature imports or network access.\n' "${mode#--without-}"
