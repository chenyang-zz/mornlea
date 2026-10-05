#!/usr/bin/env bash
# Capture one headless Godot world frame into untracked pilot evidence.
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

scenario_path="${repository_root}/testdata/godot-pilot/transcripts/terrain/visual-capture-scenario.json"
helper_log=""
helper_exe=""
helper_pid=""
run_dir=""

fail() {
  printf 'Godot visual capture: %s\n' "$*" >&2
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
  printf 'usage: %s [--scenario PATH] [--port N] [--run-dir PATH]\n' "${0##*/}" >&2
}

port="${MORNLEA_GODOT_CAPTURE_PORT:-0}"
run_dir_override=""
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
    --run-dir)
      (($# >= 2)) || { usage; exit 2; }
      run_dir_override="$2"
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

[[ -f "${scenario_path}" ]] || fail "capture scenario is missing: ${scenario_path}"
case "$(uname -s):$(uname -m)" in
  Darwin:arm64)
    display_args=(--display-driver macos --rendering-driver metal)
    runtime_family="darwin64"
    platform_version="$(sw_vers -productName) $(sw_vers -productVersion)"
    engine_name="libmornlea_engine.dylib"
    loader_rpath="@loader_path"
    ;;
  Linux:x86_64)
    [[ -n "${DISPLAY:-}" ]] || fail "Linux capture requires DISPLAY; use a desktop or Xvfb"
    display_args=(--display-driver x11 --rendering-driver opengl3)
    runtime_family="linux64"
    platform_version="$(uname -s) $(uname -r)"
    engine_name="libmornlea_engine.so"
    loader_rpath='$ORIGIN'
    ;;
  *) fail "unsupported desktop target: $(uname -s)-$(uname -m)" ;;
esac
case "${run_dir_override}" in
  *testdata/visual-golden*)
    fail "pilot evidence must stay outside tracked visual baselines"
    ;;
esac

"${script_dir}/build-core.sh" --verify >/dev/null
"${script_dir}/build-extension.sh" --verify >/dev/null

git_commit="$(git -C "${repository_root}" rev-parse HEAD)"
worktree_state="clean"
if [[ -n "$(git -C "${repository_root}" status --porcelain)" ]]; then
  worktree_state="dirty"
fi
run_id="$(date -u +%Y%m%dT%H%M%SZ)-${git_commit:0:12}"
if [[ -n "${run_dir_override}" ]]; then
  run_dir="${run_dir_override}"
else
  run_dir="${repository_root}/build/visual/godot-pilot/${run_id}"
fi
mkdir -p "${run_dir}/world"
run_dir="$(cd -- "${run_dir}" && pwd -P)"

if [[ "${port}" =~ ^[1-9][0-9]*$ ]] && ((port <= 65535)); then
  :
else
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

helper_exe="${TMPDIR:-/tmp}/mornlea-godot-transcripts-capture.$$"
helper_log="$(mktemp "${TMPDIR:-/tmp}/mornlea-godot-capture-helper.XXXXXX")"
(
  cd "${repository_root}/packages/client" &&
  "${go_bin}" build -o "${helper_exe}" ./cmd/mornlea-godot-transcripts
) || fail "the transcript helper could not be built"

"${helper_exe}" -scenario "${scenario_path}" -port "${port}" -deadline 4m >"${helper_log}" 2>&1 &
helper_pid=$!

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

if /usr/bin/env \
  MORNLEA_GODOT_CAPTURE_PORT="${port}" \
  MORNLEA_GODOT_CAPTURE_DIR="${run_dir}" \
  MORNLEA_GODOT_CAPTURE_RUN_ID="${run_id}" \
  MORNLEA_GIT_COMMIT="${git_commit}" \
  MORNLEA_WORKTREE_STATE="${worktree_state}" \
  MORNLEA_GODOT_VERSION="${GODOT_VERSION}" \
  MORNLEA_PY4GODOT_VERSION="${PY4GODOT_VERSION}" \
  MORNLEA_PY4GODOT_SOURCE_REVISION="${PY4GODOT_SOURCE_REVISION}" \
  MORNLEA_CPYTHON_VERSION="${PY4GODOT_CPYTHON_VERSION}" \
  MORNLEA_PLATFORM_ARCH="$(uname -m)" \
  MORNLEA_PLATFORM_VERSION="${platform_version}" \
  "${godot_binary}" --audio-driver Dummy "${display_args[@]}" \
  --resolution 640x360 --path "${project_root}" --quit-after 20000 \
  res://tests/scenes/visual_capture.tscn >"${run_dir}/godot.log" 2>&1; then
  check_output="$(cat -- "${run_dir}/godot.log")"
else
  scene_status="$?"
  cat -- "${run_dir}/godot.log" >&2
  fail "the visual capture scene failed (exit ${scene_status})"
fi
[[ "${check_output}" == *"Python visual capture passed."* ]] || {
  printf '%s\n' "${check_output}" >&2
  fail "the visual capture success marker is missing"
}
[[ "${check_output}" != *"ERROR:"* && "${check_output}" != *"SCRIPT ERROR:"* ]] || {
  printf '%s\n' "${check_output}" >&2
  fail "the visual capture reported an engine or script error"
}

runtime_python="${project_root}/addons/py4godot/cpython-${PY4GODOT_CPYTHON_VERSION}-${runtime_family}/python/bin/python3.14"
[[ -x "${runtime_python}" ]] || fail "embedded Python is missing"
"${runtime_python}" "${script_dir}/visual_evidence_contract.py" \
  --identity "${run_dir}/identity.json" \
  --run-dir "${run_dir}" || fail "the captured identity is incomplete"

printf 'Godot visual evidence written to %s\n' "${run_dir}"
