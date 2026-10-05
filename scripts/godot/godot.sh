#!/usr/bin/env bash
set -euo pipefail

script_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)"

# shellcheck source=version.env
source "${script_dir}/version.env"

cache_root="${MORNLEA_GODOT_CACHE_DIR:-/tmp/mornlea-godot-cache}"
case "$(uname -s):$(uname -m)" in
  Linux:x86_64) cached_binary="${cache_root}/${GODOT_VERSION}/linux-x86_64/Godot_v${GODOT_VERSION}_linux.x86_64" ;;
  *) cached_binary="${cache_root}/${GODOT_VERSION}/darwin-universal/Godot.app/Contents/MacOS/Godot" ;;
esac
application_binary="/Applications/Godot.app/Contents/MacOS/Godot"

if [[ -n "${MORNLEA_GODOT_BIN:-}" ]]; then
  godot_binary="${MORNLEA_GODOT_BIN}"
elif [[ -x "${cached_binary}" ]]; then
  godot_binary="${cached_binary}"
elif [[ -x "${application_binary}" ]]; then
  godot_binary="${application_binary}"
else
  printf 'Godot %s was not found. Run scripts/godot/fetch.sh to materialize the verified editor outside the repository.\n' "${GODOT_VERSION}" >&2
  exit 1
fi

actual_version="$("${godot_binary}" --version)"
expected_prefix="${GODOT_VERSION/-stable/.stable}"
case "${actual_version}" in
  "${expected_prefix}"*) ;;
  *)
    printf 'Godot version mismatch: got %s, want %s\n' "${actual_version}" "${GODOT_VERSION}" >&2
    exit 1
    ;;
esac

if [[ "${1:-}" == "--print-path" ]]; then
  (($# == 1)) || {
    printf 'Godot resolver --print-path takes no other arguments\n' >&2
    exit 2
  }
  printf '%s\n' "${godot_binary}"
  exit 0
fi

exec "${godot_binary}" "$@"
