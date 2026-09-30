#!/usr/bin/env bash
set -euo pipefail

script_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)"
repository_root="$(cd -- "${script_dir}/../.." && pwd -P)"
project_root="${repository_root}/apps/mornlea-godot"
iterations=100
isolated_python="false"
smoke_root=""

usage() {
  printf 'usage: %s [--iterations POSITIVE_INTEGER] [--isolated-python]\n' "${0##*/}" >&2
}

cleanup() {
  local exit_status="$?"
  if [[ -n "${smoke_root}" && -d "${smoke_root}" ]]; then
    case "$(basename -- "${smoke_root}")" in
      mornlea-godot-smoke.*) rm -rf -- "${smoke_root}" ;;
      *) printf 'Godot smoke refused to remove unexpected temporary path: %s\n' "${smoke_root}" >&2 ;;
    esac
  fi
  return "${exit_status}"
}
trap cleanup EXIT

while (($# > 0)); do
  case "$1" in
    --iterations)
      (($# >= 2)) || { usage; exit 2; }
      iterations="$2"
      shift 2
      ;;
    --isolated-python)
      isolated_python="true"
      shift
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

[[ "${iterations}" =~ ^[1-9][0-9]*$ ]] || {
  printf 'Godot smoke iterations must be a positive integer: %s\n' "${iterations}" >&2
  exit 2
}

require_marker_once() {
  local output="$1"
  local marker="$2"
  local count
  count="$(awk -v needle="${marker}" 'index($0, needle) { count++ } END { print count + 0 }' <<<"${output}")"
  if [[ "${count}" -ne 1 ]]; then
    printf 'Godot lifecycle marker count mismatch: marker=%s count=%s\n' "${marker}" "${count}" >&2
    return 1
  fi
}

require_marker_order() {
  local output="$1"
  shift
  local previous=0
  local marker line
  for marker in "$@"; do
    line="$(awk -v needle="${marker}" 'index($0, needle) { print NR; exit }' <<<"${output}")"
    if [[ -z "${line}" || "${line}" -le "${previous}" ]]; then
      printf 'Godot lifecycle marker is missing or out of order: %s\n' "${marker}" >&2
      return 1
    fi
    previous="${line}"
  done
}

check_lifecycle() {
  local output="$1"
  local rust_editor_init="[mornlea-lifecycle] rust-init=editor"
  local rust_editor_deinit="[mornlea-lifecycle] rust-deinit=editor"
  local -a markers=(
    "[mornlea-lifecycle] rust-init=scene"
    "[mornlea-lifecycle] rust-init=main-loop"
    "[mornlea-lifecycle] python-init=host"
    "[mornlea-lifecycle] python-deinit=features"
    "[mornlea-lifecycle] python-deinit=bridge"
    "[mornlea-lifecycle] rust-deinit=main-loop"
    "[mornlea-lifecycle] rust-deinit=scene"
  )
  local marker
  for marker in "${markers[@]}"; do
    require_marker_once "${output}" "${marker}" || return 1
  done

  # Editor lifecycle is optional in exported builds, but when present it must be paired
  # and nested between Scene and MainLoop in exact reverse shutdown order.
  if grep -Fq -- "${rust_editor_init}" <<<"${output}" || grep -Fq -- "${rust_editor_deinit}" <<<"${output}"; then
    require_marker_once "${output}" "${rust_editor_init}" || return 1
    require_marker_once "${output}" "${rust_editor_deinit}" || return 1
    require_marker_order "${output}" \
      "[mornlea-lifecycle] rust-init=scene" \
      "${rust_editor_init}" \
      "[mornlea-lifecycle] rust-init=main-loop" \
      "[mornlea-lifecycle] python-init=host" \
      "[mornlea-lifecycle] python-deinit=features" \
      "[mornlea-lifecycle] python-deinit=bridge" \
      "[mornlea-lifecycle] rust-deinit=main-loop" \
      "${rust_editor_deinit}" \
      "[mornlea-lifecycle] rust-deinit=scene"
    return
  fi
  require_marker_order "${output}" "${markers[@]}"
}

resolve_godot_binary() {
  "${script_dir}/godot.sh" --print-path
}

find_crash_reports() {
  local marker_path="$1"
  shift
  local crash_root
  for crash_root in "$@"; do
    [[ -d "${crash_root}" ]] || continue
    find "${crash_root}" -type f -newer "${marker_path}" \
      \( -iname 'Godot*.crash' -o -iname 'Godot*.ips' \
      -o -iname '*Mornlea*.crash' -o -iname '*Mornlea*.ips' \
      -o -name 'core' -o -name 'core.*' \) -print
  done
}

# Materialize both exact native units once; each iteration exercises only project startup
# and teardown so compilation time cannot hide lifecycle leaks or ordering failures.
"${script_dir}/build-python-runtime.sh" --verify --offline >/dev/null
"${script_dir}/build-extension.sh" --verify >/dev/null

smoke_root="$(mktemp -d "${TMPDIR:-/tmp}/mornlea-godot-smoke.XXXXXX")"
isolated_home="${smoke_root}/home"
isolated_bin="${smoke_root}/bin"
forbidden_pythonpath="${smoke_root}/forbidden-pythonpath"
mkdir -p -- \
  "${isolated_home}/Library/Logs/DiagnosticReports" \
  "${isolated_bin}" \
  "${forbidden_pythonpath}"

# A non-isolated interpreter imports this ambient hook and fails immediately. The
# embedded runtime must ignore it together with Python home, startup, and user-site data.
printf '%s\n' 'raise RuntimeError("ambient Python environment reached Mornlea smoke")' \
  > "${forbidden_pythonpath}/sitecustomize.py"

godot_binary="$(resolve_godot_binary)"
[[ -x "${godot_binary}" ]] || {
  printf 'Godot smoke executable is unavailable: %s\n' "${godot_binary}" >&2
  exit 1
}

crash_roots=("${smoke_root}" "${isolated_home}/Library/Logs/DiagnosticReports")
if [[ "$(uname -s)" == "Darwin" ]] && command -v dscl >/dev/null 2>&1; then
  system_user_home="$(dscl . -read "/Users/$(id -un)" NFSHomeDirectory 2>/dev/null | awk '{print $2}')"
  if [[ -n "${system_user_home}" ]]; then
    crash_roots+=("${system_user_home}/Library/Logs/DiagnosticReports")
  fi
fi

for ((iteration = 1; iteration <= iterations; iteration++)); do
  smoke_token="mornlea-smoke-$$-${iteration}-${RANDOM}"
  iteration_marker="${smoke_root}/iteration-${iteration}.start"
  touch -- "${iteration_marker}"

  command_args=(
    --headless
    --path "${project_root}"
    --quit-after 30
    -- "--mornlea-smoke-token=${smoke_token}"
  )
  if [[ "${isolated_python}" == "true" ]]; then
    # deny-network.sh preserves the sandbox-exec deny network* policy on both platforms.
    # The sandbox denies network access and supplies no executable search path, so the
    # project can succeed only with its checked-in, embedded Python distribution.
    run_command=(
      "${script_dir}/deny-network.sh"
      /usr/bin/env
      HOME="${isolated_home}"
      TMPDIR="${smoke_root}"
      PATH="${isolated_bin}"
      PYTHONHOME="${forbidden_pythonpath}"
      PYTHONPATH="${forbidden_pythonpath}"
      PYTHONSTARTUP="${forbidden_pythonpath}/sitecustomize.py"
      PYTHONNOUSERSITE=1
      PYTHONUSERBASE="${smoke_root}/forbidden-user-base"
      PIP_CONFIG_FILE=/dev/null
      "${godot_binary}"
      "${command_args[@]}"
    )
  else
    run_command=("${script_dir}/godot.sh" "${command_args[@]}")
  fi

  if ! output="$("${run_command[@]}" 2>&1)"; then
    printf '%s\n' "${output}" >&2
    printf 'Godot lifecycle smoke failed on iteration %s.\n' "${iteration}" >&2
    exit 1
  fi
  if grep -Eiq -- 'ERROR:|SCRIPT ERROR:|leaked at exit|instances leaked|resources still in use at exit|orphan (StringName|NodePath)|interpreter.+not finalized|features?.+still active|fatal python error|segmentation fault|abort trap' <<<"${output}"; then
    printf '%s\n' "${output}" >&2
    printf 'Godot lifecycle smoke reported an error or leak on iteration %s.\n' "${iteration}" >&2
    exit 1
  fi
  check_lifecycle "${output}" || {
      printf '%s\n' "${output}" >&2
      printf 'Godot lifecycle smoke order failed on iteration %s.\n' "${iteration}" >&2
      exit 1
    }

  if crash_reports="$(find_crash_reports "${iteration_marker}" "${crash_roots[@]}")" && [[ -n "${crash_reports}" ]]; then
    printf '%s\n' "${crash_reports}" >&2
    printf 'Godot lifecycle smoke found a crash report on iteration %s.\n' "${iteration}" >&2
    exit 1
  fi
  if leftover_pids="$(pgrep -f -- "${smoke_token}")"; then
    printf 'Godot lifecycle smoke left pilot processes on iteration %s: %s\n' \
      "${iteration}" "${leftover_pids//$'\n'/,}" >&2
    exit 1
  fi
done

printf 'Godot lifecycle smoke passed: iterations=%s isolated_python=%s.\n' \
  "${iterations}" "${isolated_python}"
