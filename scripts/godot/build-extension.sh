#!/usr/bin/env bash
set -euo pipefail

script_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)"
repository_root="${MORNLEA_REPOSITORY_ROOT:-$(cd -- "${script_dir}/../.." && pwd -P)}"
repository_root="$(cd -- "${repository_root}" && pwd -P)"
engine_root="${repository_root}/packages/engine"
project_root="${MORNLEA_GODOT_PROJECT_ROOT:-${repository_root}/apps/mornlea-godot}"
profile="debug"
verify="false"

case "$(uname -s):$(uname -m)" in
  Linux:x86_64) target="x86_64-unknown-linux-gnu" ;;
  Darwin:arm64) target="aarch64-apple-darwin" ;;
  Darwin:x86_64) target="x86_64-apple-darwin" ;;
  *) target="unsupported" ;;
esac

while (($# > 0)); do
  case "$1" in
    --target)
      (($# >= 2)) || { printf '%s\n' '--target requires a value' >&2; exit 2; }
      target="$2"
      shift 2
      ;;
    --profile)
      (($# >= 2)) || { printf '%s\n' '--profile requires a value' >&2; exit 2; }
      profile="$2"
      shift 2
      ;;
    --verify)
      verify="true"
      shift
      ;;
    *)
      printf 'unsupported build-extension argument: %s\n' "$1" >&2
      exit 2
      ;;
  esac
done

[[ "${profile}" == "debug" || "${profile}" == "release" ]] || {
  printf 'unsupported GDExtension profile: %s\n' "${profile}" >&2
  exit 1
}

case "${target}" in
  aarch64-apple-darwin|x86_64-apple-darwin)
    platform_dir="macos-universal"
    library_name="libmornlea_godot.dylib"
    ;;
  x86_64-unknown-linux-gnu)
    platform_dir="linux-x86_64"
    library_name="libmornlea_godot.so"
    ;;
  x86_64-pc-windows-msvc)
    platform_dir="windows-x86_64"
    library_name="mornlea_godot.dll"
    ;;
  *)
    printf 'unsupported Godot desktop target: %s\n' "${target}" >&2
    exit 1
    ;;
esac

host_target="$(rustup run 1.97.1 rustc -vV | awk '/^host:/ {print $2}')"
[[ "${target}" == "${host_target}" ]] || {
  printf 'cross-target GDExtension builds are reserved but not enabled in this pilot: target=%s host=%s\n' "${target}" "${host_target}" >&2
  exit 1
}

# Cargo and the packaging consumer share this exported root so an inherited
# override cannot make the produced library invisible to the distribution step.
if [[ -z "${CARGO_TARGET_DIR:-}" ]]; then
  cargo_target_dir="${engine_root}/target"
elif [[ "${CARGO_TARGET_DIR}" == /* ]]; then
  cargo_target_parent="$(dirname -- "${CARGO_TARGET_DIR}")"
  mkdir -p -- "${cargo_target_parent}"
  cargo_target_dir="$(cd -- "${cargo_target_parent}" && pwd -P)/$(basename -- "${CARGO_TARGET_DIR}")"
else
  # Relative overrides remain under the repository; traversal is rejected
  # before Cargo can create an output outside that ownership boundary.
  case "/${CARGO_TARGET_DIR}/" in
    */../*)
      printf "%s\n" "relative CARGO_TARGET_DIR must not contain '..'" >&2
      exit 2
      ;;
  esac
  cargo_target_dir="${repository_root}/${CARGO_TARGET_DIR}"
fi
export CARGO_TARGET_DIR="${cargo_target_dir}"

source_profile_dir="debug"
if [[ "${profile}" == "release" ]]; then
  source_profile_dir="release"
  rustup run 1.97.1 cargo build \
    --manifest-path "${engine_root}/Cargo.toml" \
    --package mornlea_godot \
    --locked \
    --target "${target}" \
    --release
else
  rustup run 1.97.1 cargo build \
    --manifest-path "${engine_root}/Cargo.toml" \
    --package mornlea_godot \
    --locked \
    --target "${target}"
fi

source_library="${CARGO_TARGET_DIR}/${target}/${source_profile_dir}/${library_name}"
destination_dir="${project_root}/addons/mornlea_bridge/bin/${platform_dir}/${profile}"
destination_library="${destination_dir}/${library_name}"
[[ -f "${source_library}" ]] || { printf 'built GDExtension is missing: %s\n' "${source_library}" >&2; exit 1; }
mkdir -p -- "${destination_dir}"
cp -- "${source_library}" "${destination_library}"

if [[ "${verify}" == "true" ]]; then
  # The editor always loads the debug variant while discovering extensions,
  # including when this invocation qualifies the release distribution.
  debug_library="${project_root}/addons/mornlea_bridge/bin/${platform_dir}/debug/${library_name}"
  [[ -f "${debug_library}" ]] || {
    printf 'debug GDExtension is required before verification: %s\n' "${debug_library}" >&2
    exit 1
  }
  nm_flags=(-gU)
  [[ "${target}" != x86_64-unknown-linux-gnu ]] || nm_flags=(-D --defined-only)
  entry_symbols="$(nm "${nm_flags[@]}" "${destination_library}")"
  grep -q 'gdext_rust_init' <<<"${entry_symbols}" || {
    printf 'GDExtension entry symbol is missing from %s\n' "${destination_library}" >&2
    exit 1
  }
  editor_exit_args=(--quit)
  if [[ "${target}" == x86_64-unknown-linux-gnu ]]; then
    # Godot 4.7.2 can execute deferred EditorHelp extension documentation after
    # cleanup_doc has freed it during immediate cold editor exit. Let the editor
    # finish initialization before requesting its bounded, normal shutdown.
    editor_exit_args=(--quit-after 120)
  fi
  if ! output="$("${script_dir}/godot.sh" \
    --headless \
    --path "${project_root}" \
    --editor \
    "${editor_exit_args[@]}" 2>&1)"; then
    printf '%s\n' "${output}" >&2
    exit 1
  fi
  printf '%s\n' "${output}"
  if [[ "${output}" == *"SCRIPT ERROR:"* || "${output}" == *"ERROR:"* ]]; then
    printf 'Godot editor extension import failed.\n' >&2
    exit 1
  fi
  if ! output="$("${script_dir}/godot.sh" \
    --headless \
    --path "${project_root}" \
    --script res://tests/scripts/bridge_identity_check.gd 2>&1)"; then
    printf '%s\n' "${output}" >&2
    exit 1
  fi
  printf '%s\n' "${output}"
  if [[ "${output}" == *"SCRIPT ERROR:"* || "${output}" == *"ERROR:"* || "${output}" != *"Godot bridge identity check passed."* ]]; then
    printf 'Godot bridge identity verification failed.\n' >&2
    exit 1
  fi

  if ! output="$("${script_dir}/godot.sh" \
    --headless \
    --path "${project_root}" \
    --quit-after 300 \
    res://tests/scenes/bridge_host_check.tscn 2>&1)"; then
    printf '%s\n' "${output}" >&2
    exit 1
  fi
  printf '%s\n' "${output}"
  if [[ "${output}" == *"SCRIPT ERROR:"* || "${output}" == *"ERROR:"* || "${output}" != *"Python bridge host check passed."* ]]; then
    printf 'Python bridge host verification failed.\n' >&2
    exit 1
  fi
fi

printf 'GDExtension built for %s/%s: %s\n' "${target}" "${profile}" "${destination_library}"
