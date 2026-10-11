#!/bin/zsh
set -u
WT=/Users/chen/work/mornlea-f2-37-zcode
EV=$WT/zcode-37-review-evidence
PKG3=/Users/chen/Documents/Codex/2026-10-03/task-3/mornlea-f2-mac-20261003/mac-evidence/previous-package3
VENV=/Users/chen/work/mornlea/packages/agent/.venv/bin

export PATH="$VENV:$HOME/.cargo/bin:/opt/homebrew/bin:$HOME/.gvm/pkgsets/go1.26.0/global/bin:$HOME/.gvm/gos/go1.26.0/bin:$HOME/.gvm/pkgsets/go1.26.0/global/overlay/bin:$HOME/.gvm/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin"
export MORNLEA_AGENT_PYTHON=$VENV/python
export PYTHONPATH="$WT/packages/agent/app/src:$WT/packages/agent/harness/src:$WT/packages/agent/extension-api/src"
export TMPDIR=/private/tmp/f2z37
export CARGO_NET_OFFLINE=true PYTHONDONTWRITEBYTECODE=1
export GOWORK=auto GOFLAGS=-mod=readonly GOPROXY=off GOCACHE=$EV/go-cache GOMAXPROCS=4
export MORNLEA_PREVIOUS_PACKAGE=$PKG3/previous-runtime.json
export MORNLEA_PREVIOUS_SERVER_BIN=$PKG3/previous-server
export MORNLEA_RUST_SERVER_BIN=$WT/packages/engine/target/cargo/release/mornlea-server

run() { python3 "$EV/run-record.py" "$@"; }
CARGO_TEST=(rustup run 1.97.1 cargo test --offline --manifest-path packages/engine/Cargo.toml --target-dir packages/engine/target/cargo -p mornlea_server --locked)

run 25-parity-review $CARGO_TEST --test local_remote_parity
run 26-persistence-full-postfix $CARGO_TEST --test persistence_failure
run 27-go-audit go test ./packages/audit -count=1

echo '--- selector identities ---'
shasum -a 256 "$PKG3/previous-server" "$PKG3/previous-verifier" "$PKG3/previous-runtime.json" \
  "$WT/packages/engine/target/cargo/release/mornlea-server" \
  "$WT/packages/engine/target/cargo/debug/mornlea-server" \
  "$VENV/python" > "$EV/selector-identities.txt"
cat "$EV/selector-identities.txt"
