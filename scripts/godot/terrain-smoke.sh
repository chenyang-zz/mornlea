#!/usr/bin/env bash
# Real-TCP dedicated-server terrain smoke for the Godot pilot.
#
# Builds and verifies the pilot's core and extension, builds the real
# `mornlea-server` binary into a per-run temporary directory, starts it on a
# deterministic loopback port with its world and config rooted in that same
# temporary directory (the binary has no in-memory flag; the no-repository-
# writes guarantee comes from pointing `--world` and `--config` at temporary
# paths, the same isolation the server's own subprocess tests use), runs the
# headless smoke scene against it, and reaps every child through the exit
# trap with a survivors check so no run, success or failure, leaks processes.
set -euo pipefail

script_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)"
repository_root="$(cd -- "${script_dir}/../.." && pwd -P)"
project_root="${repository_root}/apps/mornlea-godot"
case "$(uname -s):$(uname -m)" in
  Darwin:arm64) engine_name="libmornlea_engine.dylib"; loader_rpath="@loader_path" ;;
  Linux:x86_64) engine_name="libmornlea_engine.so"; loader_rpath='$ORIGIN' ;;
  *) printf 'unsupported desktop target: %s-%s\n' "$(uname -s)" "$(uname -m)" >&2; exit 1 ;;
esac
engine_dylib="${repository_root}/packages/engine/target/release/${engine_name}"

# shellcheck source=python-version.env
source "${script_dir}/python-version.env"
# shellcheck source=py4godot/build-inputs.env
source "${script_dir}/py4godot/build-inputs.env"
# shellcheck source=version.env
source "${script_dir}/version.env"

server_pid=""
godot_pid=""
smoke_root=""
child_pids=()

fail() {
  printf 'Godot terrain smoke: %s\n' "$*" >&2
  exit 1
}

record_child() {
  child_pids+=("$1")
}

# A direct child that exited but was not waited on still answers kill -0 as
# a zombie; only a live, non-zombie process counts as still running, so the
# bounded waits never spin on an already-exited child.
process_alive() {
  local pid="$1"
  kill -0 "${pid}" 2>/dev/null || return 1
  [[ "$(ps -o stat= -p "${pid}" 2>/dev/null || true)" != Z* ]]
}

# Terminate, wait, escalate, and reap every recorded child; every pid is
# explicitly waited so no zombie can outlive the gate.
reap_children() {
  local pid deadline
  for pid in ${child_pids[@]+"${child_pids[@]}"}; do
    if kill -0 "${pid}" 2>/dev/null; then
      kill "${pid}" 2>/dev/null || true
    fi
  done
  deadline=$((SECONDS + 10))
  for pid in ${child_pids[@]+"${child_pids[@]}"}; do
    while process_alive "${pid}"; do
      ((SECONDS < deadline)) || break
      sleep 0.2
    done
  done
  for pid in ${child_pids[@]+"${child_pids[@]}"}; do
    if kill -0 "${pid}" 2>/dev/null; then
      kill -9 "${pid}" 2>/dev/null || true
    fi
    wait "${pid}" 2>/dev/null || true
  done
}

cleanup() {
  local exit_status="$?"
  local pid survivors
  reap_children
  survivors=""
  for pid in ${child_pids[@]+"${child_pids[@]}"}; do
    if kill -0 "${pid}" 2>/dev/null; then
      survivors="${survivors} ${pid}"
    fi
  done
  # The per-run temporary root is unique to this invocation, so a pgrep over
  # it catches a server that forked or re-execed away from the recorded pid.
  if [[ -n "${smoke_root}" ]] && pgrep -f "${smoke_root}" >/dev/null 2>&1; then
    survivors="${survivors} pgrep(${smoke_root})"
  fi
  if [[ -n "${survivors}" ]]; then
    printf 'Godot terrain smoke: children survived the reap:%s\n' "${survivors}" >&2
    # Every other failure path dumps the captured logs; the rare survivor
    # path must not be the one place left without evidence.
    for log in "${server_log}" "${godot_log}"; do
      if [[ -f "${log}" ]]; then
        printf '--- %s ---\n' "${log}" >&2
        tail -n 40 -- "${log}" >&2 || true
      fi
    done
    exit_status=1
  fi
  if [[ -n "${smoke_root}" && -d "${smoke_root}" ]]; then
    rm -rf -- "${smoke_root}"
  fi
  return "${exit_status}"
}
trap cleanup EXIT

usage() {
  printf 'usage: %s [--port N]\n' "${0##*/}" >&2
}

port="${MORNLEA_GODOT_TERRAIN_SMOKE_PORT:-0}"
while (($# > 0)); do
  case "$1" in
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



# Build verifies: the core and extension the scene loads must match the
# committed sources before the run observes anything.
"${script_dir}/build-core.sh" --verify >/dev/null
"${script_dir}/build-extension.sh" --verify >/dev/null

if [[ "${port}" =~ ^[1-9][0-9]*$ ]] && ((port <= 65535)); then
  :
else
  # Derive a candidate port from the gate's own pid, the terrain-check
  # precedent; a taken port fails the server fast at bind time with a named
  # log line instead of hanging the scene.
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
[[ -n "${go_bin}" ]] || fail "a Go toolchain is required to build the dedicated server"

[[ -f "${engine_dylib}" ]] || \
  fail "the engine release library is missing: ${engine_dylib} (run make rust)"

# The per-run root holds the server binary, its colocated engine library
# (the binary resolves libmornlea_engine.dylib through @loader_path, the
# Makefile build-target convention), the disposable world directory, the
# absent config pointer, and every log. Nothing lands in the repository's
# saves or worlds directories.
smoke_root="$(mktemp -d "${TMPDIR:-/tmp}/mornlea-godot-terrain-smoke.$$.XXXXXX")"
mkdir -p "${smoke_root}/bin"
server_exe="${smoke_root}/bin/mornlea-server"
server_log="${smoke_root}/server.log"
world_dir="${smoke_root}/world"
config_pointer="${smoke_root}/absent-config.json"
godot_log="${smoke_root}/godot.log"
cp -- "${engine_dylib}" "${smoke_root}/bin/${engine_name}"

(
  cd "${repository_root}" &&
  "${go_bin}" build -ldflags="-extldflags=-Wl,-rpath,${loader_rpath}" \
    -o "${server_exe}" ./packages/server/cmd/mornlea-server
) || fail "the dedicated server could not be built"

# Memory-mode equivalent for a binary that has no in-memory flag: a fresh
# world created inside the per-run root (seed pinned to the server default
# for a deterministic spawn area) and an absent config file so the server
# falls back to its default configuration instead of reading a developer
# config; logging stays at the default info level in the captured log.
"${server_exe}" \
  --listen "127.0.0.1:${port}" \
  --world "${world_dir}" \
  --seed 42 \
  --config "${config_pointer}" \
  >"${server_log}" 2>&1 &
server_pid=$!
record_child "${server_pid}"

# Wait for the server's own startup line so the scene never races the bind.
listen_deadline=$((SECONDS + 30))
until grep -q -- "listen=127.0.0.1:${port}" "${server_log}" 2>/dev/null; do
  if ! process_alive "${server_pid}"; then
    wait "${server_pid}" 2>/dev/null || true
    server_pid=""
    cat -- "${server_log}" >&2 || true
    fail "the dedicated server exited before listening (port ${port} may be taken)"
  fi
  ((SECONDS < listen_deadline)) || fail "the dedicated server did not start listening in time"
  sleep 0.2
done

godot_binary="$("${script_dir}/godot.sh" --print-path)"
[[ -x "${godot_binary}" ]] || fail "Godot executable is unavailable: ${godot_binary}"
actual_godot_version="$("${godot_binary}" --version)"
[[ "${actual_godot_version}" == 4.7.2.stable* ]] || \
  fail "Godot version mismatch: got ${actual_godot_version}, want 4.7.2-stable"

# The scene runs against the loopback server only; the driver's own bounded
# frame budgets bound the run and --quit-after is the outer frame safety net.
/usr/bin/env \
  MORNLEA_TERRAIN_SMOKE_ADDRESS="127.0.0.1:${port}" \
  "${godot_binary}" --headless --path "${project_root}" --quit-after 20000 \
  res://tests/scenes/terrain_smoke_check.tscn >"${godot_log}" 2>&1 &
godot_pid=$!
record_child "${godot_pid}"

# Bounded wait for a clean pilot exit: at the engine's frame cadence the
# driver's budgets finish in roughly two minutes; the bound only catches a
# wedged scene that defeated --quit-after.
godot_deadline=$((SECONDS + 420))
while process_alive "${godot_pid}"; do
  ((SECONDS < godot_deadline)) || {
    cat -- "${godot_log}" >&2 || true
    cat -- "${server_log}" >&2 || true
    fail "the smoke scene did not exit within its bounded wait"
  }
  sleep 0.2
done
godot_status=0
wait "${godot_pid}" || godot_status=$?
godot_pid=""
check_output="$(cat -- "${godot_log}")"
if ((godot_status != 0)); then
  printf '%s\n' "${check_output}" >&2
  cat -- "${server_log}" >&2 || true
  fail "the smoke scene failed with exit status ${godot_status}"
fi
[[ "${check_output}" == *"Python terrain smoke passed:"* ]] || {
  printf '%s\n' "${check_output}" >&2
  cat -- "${server_log}" >&2 || true
  fail "the terrain smoke success marker is missing"
}
[[ "${check_output}" != *"ERROR:"* && "${check_output}" != *"SCRIPT ERROR:"* ]] || {
  printf '%s\n' "${check_output}" >&2
  cat -- "${server_log}" >&2 || true
  fail "the terrain smoke reported an engine or script error"
}
# The driver's own success line carries the run's evidence numbers.
printf '%s\n' "${check_output}" | grep -- "Python terrain smoke passed:"

# The dedicated server owns a SIGTERM shutdown path (its subprocess test
# proves a zero exit and world-lock release); use it and require a clean exit.
kill "${server_pid}" 2>/dev/null || true
shutdown_deadline=$((SECONDS + 15))
while process_alive "${server_pid}"; do
  ((SECONDS < shutdown_deadline)) || fail "the dedicated server did not shut down in time"
  sleep 0.2
done
server_status=0
wait "${server_pid}" || server_status=$?
server_pid=""
if ((server_status != 0)); then
  cat -- "${server_log}" >&2 || true
  fail "the dedicated server exited uncleanly after SIGTERM (${server_status})"
fi

printf 'Godot terrain smoke verified: real dedicated server on 127.0.0.1:%s through login, loaded terrain, the fixed frame run, and the clean close.\n' "${port}"
