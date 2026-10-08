#!/usr/bin/env bash
# Regenerate task 3.8 activation/rollback evidence against the current checkout.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/../../../../.." && pwd)"
EV="$(cd "$(dirname "$0")" && pwd)"
: "${MORNLEA_RUST_SERVER_BIN:?set to the rebuilt release mornlea-server}"
: "${MORNLEA_PREVIOUS_SERVER_BIN:?set to the sealed previous Go server}"
: "${MORNLEA_PREVIOUS_PACKAGE:?set to the sealed previous-runtime.json}"
export PATH="${HOME}/.cargo/bin:${PATH}"
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-/tmp/mornlea-3.8-target}"
export TMPDIR="${TMPDIR:-/tmp/e38}"
mkdir -p "$TMPDIR"
SRC="$(git -C "$ROOT" rev-parse HEAD)"

header() {
  local log="$1"; shift
  {
    echo "# command: $*"
    echo "# source_commit: $SRC"
    echo "# started_utc: $(date -u +%Y-%m-%dT%H:%M:%SZ)"
    echo "# env: MORNLEA_RUST_SERVER_BIN=$MORNLEA_RUST_SERVER_BIN"
    echo "# env: MORNLEA_PREVIOUS_SERVER_BIN=$MORNLEA_PREVIOUS_SERVER_BIN"
    echo "# env: MORNLEA_PREVIOUS_PACKAGE=$MORNLEA_PREVIOUS_PACKAGE"
    echo "# env: TMPDIR=$TMPDIR"
  } >"$log"
}

run_cargo() {
  local name="$1"; shift
  local log="$EV/${name}.log"
  header "$log" "$@"
  ( cd "$ROOT/packages/engine" && "$@" ) >>"$log" 2>&1
  local code=$?
  echo "# exit: $code" >>"$log"
  echo "# finished_utc: $(date -u +%Y-%m-%dT%H:%M:%SZ)" >>"$log"
  return "$code"
}

run_cargo activation-tests \
  cargo test -p mornlea_server --locked --test persistence_failure \
  -- activation::actual_activate_stop_compatible_rollback \
     activation::actual_backup_restore \
     activation::interrupted_each_phase \
     activation::live_writer_and_bad_previous_identity \
  --exact --nocapture --test-threads=1

run_cargo default-startup \
  cargo test -p mornlea_server --locked --test persistence_failure \
  -- activation::default_startup_paths_untouched --exact --nocapture

header "$EV/self-test.log" bash scripts/rust-server-opt-in.sh --self-test
( cd "$ROOT" && bash scripts/rust-server-opt-in.sh --self-test ) >>"$EV/self-test.log" 2>&1
echo "# exit: $?" >>"$EV/self-test.log"
echo "# finished_utc: $(date -u +%Y-%m-%dT%H:%M:%SZ)" >>"$EV/self-test.log"

echo "regenerated under $EV; rewrite MANIFEST.sha256 after reviewing the logs"
