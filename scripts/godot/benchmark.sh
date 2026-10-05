#!/usr/bin/env bash
# Sample Godot pilot performance using benchmark scenario v23 windows.
set -euo pipefail

script_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)"
repository_root="$(cd -- "${script_dir}/../.." && pwd -P)"
project_root="${repository_root}/apps/mornlea-godot"
engine_dylib=""

# shellcheck source=python-version.env
source "${script_dir}/python-version.env"
# shellcheck source=py4godot/build-inputs.env
source "${script_dir}/py4godot/build-inputs.env"
# shellcheck source=version.env
source "${script_dir}/version.env"

smoke_root=""
server_log=""
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
  printf 'Godot benchmark: %s\n' "$*" >&2
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

process_alive() {
  local pid="$1"
  kill -0 "${pid}" 2>/dev/null || return 1
  [[ "$(ps -o stat= -p "${pid}" 2>/dev/null || true)" != Z* ]]
}

process_group_has_live_members() {
  local pgid="$1"
  ps -axo pgid=,stat= | awk -v wanted="${pgid}" '
    $1 == wanted && substr($2, 1, 1) != "Z" { found = 1 }
    END { exit found ? 0 : 1 }
  '
}

reap_children() {
  local pid pgid deadline
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
  for log in "${godot_log}" "${server_log}"; do
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
    printf 'Godot benchmark: children survived cleanup:%s\n' "${survivors}" >&2
    dump_logs
    exit_status=1
  fi
  if [[ -n "${smoke_root}" && -d "${smoke_root}" ]]; then
    case "$(basename -- "${smoke_root}")" in
      mornlea-godot-benchmark.*) rm -rf -- "${smoke_root}" ;;
      *)
        printf 'Godot benchmark refused unexpected temporary path: %s\n' "${smoke_root}" >&2
        exit_status=1
        ;;
    esac
  fi
  return "${exit_status}"
}
trap cleanup EXIT

usage() {
  printf 'usage: %s [--port N] [--output PATH]\n' "${0##*/}" >&2
}

port="${MORNLEA_GODOT_BENCHMARK_PORT:-0}"
output=""
while (($# > 0)); do
  case "$1" in
    --port)
      (($# >= 2)) || { usage; exit 2; }
      port="$2"
      shift 2
      ;;
    --output)
      (($# >= 2)) || { usage; exit 2; }
      output="$2"
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

case "${output}" in
  *testdata/visual-golden*)
    fail "benchmark output must stay outside tracked goldens"
    ;;
esac

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
engine_dylib="${repository_root}/packages/engine/target/release/${engine_name}"
"${script_dir}/build-core.sh" --verify >/dev/null
"${script_dir}/build-extension.sh" --verify >/dev/null
[[ -f "${engine_dylib}" ]] || fail "the engine release library is missing: ${engine_dylib} (run make rust)"

git_commit="$(git -C "${repository_root}" rev-parse HEAD)"
worktree_state="clean"
if [[ -n "$(git -C "${repository_root}" status --porcelain)" ]]; then
  worktree_state="dirty"
fi
run_id="$(date -u +%Y%m%dT%H%M%SZ)-${git_commit:0:12}"
if [[ -z "${output}" ]]; then
  output="${repository_root}/build/perf/godot-pilot/${run_id}.json"
fi
mkdir -p "$(dirname -- "${output}")"

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

smoke_root="$(mktemp -d "${TMPDIR:-/tmp}/mornlea-godot-benchmark.$$.XXXXXX")"
mkdir -p "${smoke_root}/bin"
server_exe="${smoke_root}/bin/mornlea-server"
server_log="${smoke_root}/server.log"
godot_log="${smoke_root}/godot.log"
world_dir="${smoke_root}/world"
config_pointer="${smoke_root}/absent-config.json"
cp -- "${engine_dylib}" "${smoke_root}/bin/${engine_name}"

(
  cd "${repository_root}" &&
  "${go_bin}" build -ldflags="-extldflags=-Wl,-rpath,${loader_rpath}" \
    -o "${server_exe}" ./packages/server/cmd/mornlea-server
) || fail "the dedicated server could not be built"

"${process_group_launcher[@]}" "${server_exe}" \
  --listen "127.0.0.1:${port}" \
  --world "${world_dir}" \
  --seed 20260726 \
  --config "${config_pointer}" \
  >"${server_log}" 2>&1 &
server_pid=$!
record_child "${server_pid}"

listen_deadline=$((SECONDS + 30))
until grep -q -- "listen=127.0.0.1:${port}" "${server_log}" 2>/dev/null; do
  if ! process_alive "${server_pid}"; then
    wait "${server_pid}" 2>/dev/null || true
    dump_logs
    fail "the dedicated server exited before listening"
  fi
  ((SECONDS < listen_deadline)) || fail "the dedicated server did not listen in time"
  sleep 0.2
done

godot_binary="$("${script_dir}/godot.sh" --print-path)"
[[ -x "${godot_binary}" ]] || fail "Godot executable is unavailable: ${godot_binary}"
actual_godot_version="$("${godot_binary}" --version)"
[[ "${actual_godot_version}" == 4.7.2.stable* ]] || \
  fail "Godot version mismatch: got ${actual_godot_version}, want 4.7.2-stable"

set +e
/usr/bin/env \
  MORNLEA_GODOT_BENCHMARK_ADDRESS="127.0.0.1:${port}" \
  MORNLEA_GODOT_BENCHMARK_OUTPUT="${output}" \
  MORNLEA_GODOT_BENCHMARK_WARMUP_SECONDS=10 \
  MORNLEA_GODOT_BENCHMARK_STILL_SECONDS=60 \
  MORNLEA_GODOT_BENCHMARK_FLYING_SECONDS=120 \
  MORNLEA_GODOT_BENCHMARK_COOLDOWN_SECONDS=30 \
  MORNLEA_GIT_COMMIT="${git_commit}" \
  MORNLEA_WORKTREE_STATE="${worktree_state}" \
  MORNLEA_GODOT_VERSION="${GODOT_VERSION}" \
  MORNLEA_PY4GODOT_VERSION="${PY4GODOT_VERSION}" \
  MORNLEA_PY4GODOT_SOURCE_REVISION="${PY4GODOT_SOURCE_REVISION}" \
  MORNLEA_CPYTHON_VERSION="${PY4GODOT_CPYTHON_VERSION}" \
  MORNLEA_PLATFORM_ARCH="$(uname -m)" \
  MORNLEA_PLATFORM_VERSION="${platform_version}" \
  "${godot_binary}" --audio-driver Dummy "${display_args[@]}" \
  --resolution 2560x1440 --path "${project_root}" --quit-after 50000 \
  res://tests/scenes/visual_benchmark.tscn >"${godot_log}" 2>&1
godot_status=$?
set -e
if ((godot_status != 0)); then
  dump_logs
  fail "the benchmark scene failed"
fi
grep -Fq "Python visual benchmark passed." "${godot_log}" || {
  dump_logs
  fail "the benchmark success marker is missing"
}
if grep -Fq "SCRIPT ERROR:" "${godot_log}"; then
  dump_logs
  fail "the benchmark reported a script error"
fi

(
  cd "${repository_root}/packages/tools/perfcheck" &&
  "${go_bin}" run . -godot-pilot-report "${output}"
) || fail "the benchmark report identity is incomplete"
printf 'Godot benchmark report written to %s\n' "${output}"
