#!/usr/bin/env bash
# Run the complete minimum Godot pilot against a disposable real server.
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

smoke_root=""
server_log=""
helper_log=""
godot_log=""
child_pids=()
child_pgids=()
process_group_launcher=(
  /usr/bin/perl
  -MPOSIX
  -e
  'POSIX::setpgid(0, 0); exec @ARGV or die "exec: $!\n"'
)

fail() {
  printf 'Godot playable smoke: %s\n' "$*" >&2
  exit 1
}

record_child() {
  local pid="$1"
  local pgid deadline
  child_pids+=("${pid}")
  deadline=$((SECONDS + 2))
  while :; do
    pgid="$(ps -o pgid= -p "${pid}" 2>/dev/null | tr -d '[:space:]')"
    [[ "${pgid}" == "${pid}" ]] && break
    process_alive "${pid}" || fail "background process ${pid} exited before process-group setup"
    ((SECONDS < deadline)) || fail "background process ${pid} did not enter its exact process group"
    sleep 0.02
  done
  [[ "${pgid}" =~ ^[1-9][0-9]*$ && "${pgid}" == "${pid}" ]] || \
    fail "background process ${pid} did not enter its exact process group"
  child_pgids+=("${pgid}")
}

retire_child() {
  local retired="$1"
  local index pid pgid
  local remaining=()
  local remaining_groups=()
  for ((index = 0; index < ${#child_pids[@]}; index++)); do
    pid="${child_pids[index]}"
    pgid="${child_pgids[index]}"
    if [[ "${pid}" != "${retired}" ]]; then
      remaining+=("${pid}")
      remaining_groups+=("${pgid}")
    elif process_group_has_live_members "${pgid}"; then
      fail "process group ${pgid} retained descendants after leader ${pid} exited"
    fi
  done
  if (( ${#remaining[@]} )); then
    child_pids=("${remaining[@]}")
    child_pgids=("${remaining_groups[@]}")
  else
    child_pids=()
    child_pgids=()
  fi
}

# A direct zombie has exited and only needs `wait`; treating it as alive would
# make the bounded shutdown loop spin until its deadline.
process_alive() {
  local pid="$1"
  kill -0 "${pid}" 2>/dev/null || return 1
  [[ "$(ps -o stat= -p "${pid}" 2>/dev/null || true)" != Z* ]]
}

# Match the recorded numeric process group, never a command-line substring.
process_group_has_live_members() {
  local pgid="$1"
  ps -axo pgid=,stat= | awk -v wanted="${pgid}" '
    $1 == wanted && substr($2, 1, 1) != "Z" { found = 1 }
    END { exit found ? 0 : 1 }
  '
}

reap_children() {
  local index pid pgid deadline
  for pgid in ${child_pgids[@]+"${child_pgids[@]}"}; do
    kill -TERM -- "-${pgid}" 2>/dev/null || true
  done
  deadline=$((SECONDS + 10))
  for pgid in ${child_pgids[@]+"${child_pgids[@]}"}; do
    while process_group_has_live_members "${pgid}"; do
      ((SECONDS < deadline)) || break
      sleep 0.2
    done
  done
  for pgid in ${child_pgids[@]+"${child_pgids[@]}"}; do
    if process_group_has_live_members "${pgid}"; then
      kill -KILL -- "-${pgid}" 2>/dev/null || true
    fi
  done
  for pid in ${child_pids[@]+"${child_pids[@]}"}; do
    wait "${pid}" 2>/dev/null || true
  done
}

dump_logs() {
  local log
  for log in "${godot_log}" "${helper_log}" "${server_log}"; do
    if [[ -n "${log}" && -f "${log}" ]]; then
      printf -- '--- %s ---\n' "${log}" >&2
      tail -n 80 -- "${log}" >&2 || true
    fi
  done
}

cleanup() {
  local exit_status="$?"
  local survivors=""
  local index pgid
  reap_children
  for ((index = 0; index < ${#child_pgids[@]}; index++)); do
    pgid="${child_pgids[index]}"
    if process_group_has_live_members "${pgid}"; then
      survivors="${survivors} pgid(${pgid})"
    fi
  done
  child_pids=()
  child_pgids=()
  if [[ -n "${survivors}" ]]; then
    printf 'Godot playable smoke: children survived cleanup:%s\n' "${survivors}" >&2
    dump_logs
    exit_status=1
  fi
  if [[ -n "${smoke_root}" && -d "${smoke_root}" ]]; then
    case "$(basename -- "${smoke_root}")" in
      mornlea-godot-playable-smoke.*) rm -rf -- "${smoke_root}" ;;
      *)
        printf 'Godot playable smoke refused unexpected temporary path: %s\n' \
          "${smoke_root}" >&2
        exit_status=1
        ;;
    esac
  fi
  return "${exit_status}"
}
trap cleanup EXIT

usage() {
  printf 'usage: %s [--duration Ns|Nm] [--port N]\n' "${0##*/}" >&2
}

duration="300s"
port="${MORNLEA_GODOT_PLAYABLE_SMOKE_PORT:-0}"
while (($# > 0)); do
  case "$1" in
    --duration)
      (($# >= 2)) || { usage; exit 2; }
      duration="$2"
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

if [[ "${duration}" =~ ^([1-9][0-9]*)s$ ]]; then
  duration_seconds="${BASH_REMATCH[1]}"
elif [[ "${duration}" =~ ^([1-9][0-9]*)m$ ]]; then
  duration_seconds=$((BASH_REMATCH[1] * 60))
else
  fail "duration must be a positive whole number of seconds or minutes"
fi



"${script_dir}/build-core.sh" --verify >/dev/null
"${script_dir}/build-extension.sh" --verify >/dev/null

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
[[ -n "${go_bin}" ]] || fail "a Go toolchain is required"
[[ -f "${engine_dylib}" ]] || \
  fail "the engine release library is missing: ${engine_dylib} (run make rust)"

# Every binary, world file, config pointer, and log lives below this unique
# root, so the integration gate cannot touch repository saves or user config.
smoke_root="$(mktemp -d "${TMPDIR:-/tmp}/mornlea-godot-playable-smoke.$$.XXXXXX")"
mkdir -p "${smoke_root}/bin"
server_exe="${smoke_root}/bin/mornlea-server"
helper_exe="${smoke_root}/bin/mornlea-godot-smoke"
server_log="${smoke_root}/server.log"
helper_log="${smoke_root}/helper.log"
godot_log="${smoke_root}/godot.log"
world_dir="${smoke_root}/world"
config_pointer="${smoke_root}/absent-config.json"
cp -- "${engine_dylib}" "${smoke_root}/bin/${engine_name}"

(
  cd "${repository_root}" &&
  "${go_bin}" build -ldflags="-extldflags=-Wl,-rpath,${loader_rpath}" \
    -o "${server_exe}" ./packages/server/cmd/mornlea-server &&
  "${go_bin}" build -ldflags="-extldflags=-Wl,-rpath,${loader_rpath}" \
    -o "${helper_exe}" ./packages/client/cmd/mornlea-godot-smoke
) || fail "the dedicated server or remote helper could not be built"

"${process_group_launcher[@]}" "${server_exe}" \
  --listen "127.0.0.1:${port}" \
  --world "${world_dir}" \
  --seed 42 \
  --config "${config_pointer}" \
  >"${server_log}" 2>&1 &
server_pid=$!
record_child "${server_pid}"

listen_deadline=$((SECONDS + 30))
until grep -q -- "listen=127.0.0.1:${port}" "${server_log}" 2>/dev/null; do
  if ! process_alive "${server_pid}"; then
    wait "${server_pid}" 2>/dev/null || true
    retire_child "${server_pid}"
    dump_logs
    fail "the dedicated server exited before listening"
  fi
  ((SECONDS < listen_deadline)) || fail "the dedicated server did not listen in time"
  sleep 0.2
done

# The helper lifetime exceeds the scene's requested run so only the gate owns
# normal teardown; its distinct UUID makes the actor path observe a real peer.
helper_duration_seconds=$((duration_seconds + 240))
helper_player_id="72e706d8-578c-4b8c-9b71-3d5ac52f73c6"
godot_player_id="6a3f2c1e-9b4d-4e8a-a1c2-5d7e8f9a0b1c"
"${process_group_launcher[@]}" "${helper_exe}" \
  --address "127.0.0.1:${port}" \
  --duration "${helper_duration_seconds}s" \
  --player-id "${helper_player_id}" \
  --expected-peer-id "${godot_player_id}" \
  >"${helper_log}" 2>&1 &
helper_pid=$!
record_child "${helper_pid}"

helper_deadline=$((SECONDS + 30))
helper_connected_marker="Mornlea Godot smoke helper connected: player=${helper_player_id} expected_peer=${godot_player_id} "
until grep -Fq -- "${helper_connected_marker}" "${helper_log}" 2>/dev/null; do
  if ! process_alive "${helper_pid}"; then
    wait "${helper_pid}" 2>/dev/null || true
    retire_child "${helper_pid}"
    dump_logs
    fail "the remote helper exited before login completed"
  fi
  ((SECONDS < helper_deadline)) || fail "the remote helper did not log in time"
  sleep 0.2
done

godot_binary="$("${script_dir}/godot.sh" --print-path)"
[[ -x "${godot_binary}" ]] || fail "Godot executable is unavailable: ${godot_binary}"
actual_godot_version="$("${godot_binary}" --version)"
[[ "${actual_godot_version}" == 4.7.2.stable* ]] || \
  fail "Godot version mismatch: got ${actual_godot_version}, want 4.7.2-stable"

# `--quit-after` is only a generous frame-count safety net. The Python driver
# uses accumulated engine time for the requested duration and performs the
# feature-owned clean close before quitting.
quit_after=$(( (duration_seconds + 240) * 1000 ))
"${process_group_launcher[@]}" /usr/bin/env \
  MORNLEA_PLAYABLE_SMOKE_ADDRESS="127.0.0.1:${port}" \
  MORNLEA_PLAYABLE_SMOKE_DURATION_SECONDS="${duration_seconds}" \
  MORNLEA_PLAYABLE_SMOKE_REMOTE_PLAYER_ID="${helper_player_id}" \
  "${godot_binary}" --headless --disable-render-loop --path "${project_root}" \
  --quit-after "${quit_after}" \
  res://tests/scenes/playable_smoke_check.tscn >"${godot_log}" 2>&1 &
godot_pid=$!
record_child "${godot_pid}"

godot_deadline=$((SECONDS + duration_seconds + 420))
while process_alive "${godot_pid}"; do
  ((SECONDS < godot_deadline)) || {
    dump_logs
    fail "the playable scene did not exit within its bounded wait"
  }
  if ! process_alive "${helper_pid}"; then
    dump_logs
    fail "the remote helper exited while the playable scene was running"
  fi
  sleep 0.2
done
godot_status=0
wait "${godot_pid}" || godot_status=$?
retire_child "${godot_pid}"
if ((godot_status != 0)); then
  dump_logs
  fail "the playable scene failed with exit status ${godot_status}"
fi

grep -Fq -- "Python playable smoke passed:" "${godot_log}" || {
  dump_logs
  fail "the playable success marker is missing"
}
"${script_dir}/playable-log-check.sh" "${godot_log}" || {
  dump_logs
  fail "the playable scene reported an engine or script error"
}
grep -q -- "Mornlea Godot smoke helper reached Play." "${helper_log}" || {
  dump_logs
  fail "the remote helper never reached Play"
}
grep -Fq -- "Mornlea Godot smoke helper observed target:" "${helper_log}" || {
  dump_logs
  fail "the remote helper never observed a target block"
}
grep -Fq -- "Mornlea Godot smoke helper observed Godot peer: player=${godot_player_id}." \
  "${helper_log}" || {
  dump_logs
  fail "the remote helper never observed the fixed Godot peer"
}
grep -Fm1 -- "Python playable smoke passed:" "${godot_log}"

kill -TERM -- "-${helper_pid}" 2>/dev/null || true
helper_shutdown_deadline=$((SECONDS + 15))
while process_alive "${helper_pid}"; do
  ((SECONDS < helper_shutdown_deadline)) || fail "the remote helper did not shut down in time"
  sleep 0.2
done
helper_status=0
wait "${helper_pid}" || helper_status=$?
retire_child "${helper_pid}"
if ((helper_status != 0)); then
  dump_logs
  fail "the remote helper exited uncleanly (${helper_status})"
fi

kill -TERM -- "-${server_pid}" 2>/dev/null || true
server_shutdown_deadline=$((SECONDS + 15))
while process_alive "${server_pid}"; do
  ((SECONDS < server_shutdown_deadline)) || fail "the dedicated server did not shut down in time"
  sleep 0.2
done
server_status=0
wait "${server_pid}" || server_status=$?
retire_child "${server_pid}"
if ((server_status != 0)); then
  dump_logs
  fail "the dedicated server exited uncleanly (${server_status})"
fi

printf 'Godot playable smoke verified: %ss real-server minimum loop and clean teardown.\n' \
  "${duration_seconds}"
