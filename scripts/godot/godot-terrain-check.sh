#!/usr/bin/env bash
set -euo pipefail

script_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)"
repository_root="$(cd -- "${script_dir}/../.." && pwd -P)"
project_root="${repository_root}/apps/mornlea-godot"

# shellcheck source=python-version.env
source "${script_dir}/python-version.env"
# shellcheck source=py4godot/build-inputs.env
source "${script_dir}/py4godot/build-inputs.env"
# shellcheck source=version.env
source "${script_dir}/version.env"

scenario_path="${repository_root}/testdata/godot-pilot/transcripts/terrain/terrain-scenario.json"
helper_log=""
helper_exe=""
helper_pid=""

fail() {
  printf 'Godot terrain check: %s\n' "$*" >&2
  exit 1
}

cleanup() {
  local exit_status="$?"
  if [[ -n "${helper_pid}" ]] && kill -0 "${helper_pid}" 2>/dev/null; then
    kill "${helper_pid}" 2>/dev/null || true
    wait "${helper_pid}" 2>/dev/null || true
  fi
  if [[ -n "${helper_log}" && -f "${helper_log}" ]]; then
    cat -- "${helper_log}" >&2 || true
    rm -f -- "${helper_log}"
  fi
  if [[ -n "${helper_exe}" && -f "${helper_exe}" ]]; then
    rm -f -- "${helper_exe}"
  fi
  return "${exit_status}"
}
trap cleanup EXIT

usage() {
  printf 'usage: %s [--scenario PATH] [--port N]\n' "${0##*/}" >&2
}

port="${MORNLEA_GODOT_TERRAIN_CHECK_PORT:-0}"
while (($# > 0)); do
  case "$1" in
    --scenario)
      (($# >= 2)) || { usage; exit 2; }
      scenario_path="$2"
      shift 2
      ;;
    --port)
      (($# >= 2)) || { usage; exit 2; }
      port="$2"
      shift 2
      ;;
    --help|-h)
      usage
      exit 0
      ;;
    *)
      usage
      exit 2
      ;;
  esac
done

[[ -f "${scenario_path}" ]] || fail "terrain scenario is missing: ${scenario_path}"
case "$(uname -s):$(uname -m)" in
  Darwin:arm64|Linux:x86_64) ;;
  *) fail "unsupported desktop target: $(uname -s)-$(uname -m)" ;;
esac

# Build verifies: the core and extension the scene loads must match the
# committed sources before the run observes anything.
"${script_dir}/build-core.sh" --verify >/dev/null
"${script_dir}/build-extension.sh" --verify >/dev/null

if [[ "${port}" =~ ^[1-9][0-9]*$ ]] && ((port <= 65535)); then
  :
else
  # Derive a candidate port from the gate's own pid; a taken port fails the
  # helper fast with a named log line instead of hanging the scene.
  port=$$
  while ((port < 1024 || port > 65000)); do
    port=$(( (port * 7919 + 17) % 65000 ))
  done
fi

go_bin="$(command -v go || true)"
if [[ -z "${go_bin}" ]]; then
  for candidate in /usr/local/go/bin/go "${HOME}/.gvm/gos/"*/bin/go; do
    if [[ -x "${candidate}" ]]; then
      go_bin="${candidate}"
      break
    fi
  done
fi
[[ -n "${go_bin}" ]] || fail "a Go toolchain is required to build the transcript helper"

helper_exe="${TMPDIR:-/tmp}/mornlea-godot-transcripts.$$"
helper_log="$(mktemp "${TMPDIR:-/tmp}/mornlea-godot-transcripts-log.XXXXXX")"
(
  cd "${repository_root}/packages/client" &&
  "${go_bin}" build -o "${helper_exe}" ./cmd/mornlea-godot-transcripts
) || fail "the transcript helper could not be built"

"${helper_exe}" -scenario "${scenario_path}" -port "${port}" -deadline 4m >"${helper_log}" 2>&1 &
helper_pid=$!

# Wait for the helper's listening line so the scene never races the bind.
listen_deadline=$((SECONDS + 15))
until grep -q 'listening on 127.0.0.1' "${helper_log}" 2>/dev/null; do
  if ! kill -0 "${helper_pid}" 2>/dev/null; then
    fail "the transcript helper exited before listening (port ${port} may be taken)"
  fi
  ((SECONDS < listen_deadline)) || fail "the transcript helper did not start listening in time"
  sleep 0.2
done

godot_binary="$("${script_dir}/godot.sh" --print-path)"
[[ -x "${godot_binary}" ]] || fail "Godot executable is unavailable: ${godot_binary}"
actual_godot_version="$("${godot_binary}" --version)"
[[ "${actual_godot_version}" == 4.7.2.stable* ]] || \
  fail "Godot version mismatch: got ${actual_godot_version}, want 4.7.2-stable"

# The scene runs against the loopback helper only; no network sandbox denial
# is applied because the pilot session must reach the local transcript
# server, and the helper log (printed by the exit trap) attributes every
# scenario stage.
check_output="$(/usr/bin/env \
  MORNLEA_TERRAIN_CHECK_PORT="${port}" \
  "${godot_binary}" --headless --path "${project_root}" --quit-after 20000 \
  res://tests/scenes/terrain_check.tscn 2>&1)" || {
  printf '%s\n' "${check_output}" >&2
  fail "the terrain check scene failed"
}
[[ "${check_output}" == *"Python terrain check passed."* ]] || {
  printf '%s\n' "${check_output}" >&2
  fail "the terrain check success marker is missing"
}
[[ "${check_output}" != *"ERROR:"* && "${check_output}" != *"SCRIPT ERROR:"* ]] || {
  printf '%s\n' "${check_output}" >&2
  fail "the terrain check reported an engine or script error"
}

# The helper must finish both rounds and exit cleanly on its own; a bounded
# wait keeps a wedged helper from hanging the gate.
wait "${helper_pid}" || fail "the transcript helper did not complete its rounds cleanly"
helper_pid=""

printf 'Godot terrain check verified: transcript scenario through initial snapshot, chunk delta, forget, reset, and re-entry with no stale section.\n'
