#!/bin/zsh
# 3.7 evidence phase 3: corrected activation fixtures.
# MORNLEA_PREVIOUS_PACKAGE must name the previous-runtime.json manifest file
# (phase 2 wrongly passed the package directory), and python3 on PATH must be
# 3.12+ for prepare_previous's tarfile extractall(filter=...). Re-runs the two
# affected suites and records them.
set -u
WT=/Users/chen/work/mornlea-f2-37-zcode
EV=$WT/zcode-37-evidence
PKG3=/Users/chen/Documents/Codex/2026-10-03/task-3/mornlea-f2-mac-20261003/mac-evidence/previous-package3
RUST_BIN=$WT/packages/engine/target/cargo/release/mornlea-server
VENV=/Users/chen/work/mornlea/packages/agent/.venv/bin

export PATH="$VENV:$HOME/.cargo/bin:/opt/homebrew/bin:$HOME/.gvm/pkgsets/go1.26.0/global/bin:$HOME/.gvm/gos/go1.26.0/bin:$HOME/.gvm/pkgsets/go1.26.0/global/overlay/bin:$HOME/.gvm/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin"
export MORNLEA_AGENT_PYTHON=$VENV/python
export PYTHONPATH="$WT/packages/agent/app/src:$WT/packages/agent/harness/src:$WT/packages/agent/extension-api/src"
export TMPDIR=/private/tmp/f2z37
export CARGO_NET_OFFLINE=true
export PYTHONDONTWRITEBYTECODE=1
export GOWORK=auto
export GOFLAGS=-mod=readonly
export GOPROXY=off
export GOCACHE=$EV/go-cache
export GOMAXPROCS=4
export MORNLEA_PREVIOUS_PACKAGE=$PKG3/previous-runtime.json
export MORNLEA_PREVIOUS_SERVER_BIN=$PKG3/previous-server
export MORNLEA_RUST_SERVER_BIN=$RUST_BIN

run() { python3 "$EV/run-record.py" "$@"; }

python3 -c "import sys; print('python3 on PATH:', sys.version)"
echo "previous manifest: $MORNLEA_PREVIOUS_PACKAGE"

CARGO_TEST=(rustup run 1.97.1 cargo test --offline --manifest-path packages/engine/Cargo.toml --target-dir packages/engine/target/cargo -p mornlea_server --locked)

run 23-persistence-failure-fixtures-v2 $CARGO_TEST --test persistence_failure
run 24-server-contract-rerun $CARGO_TEST --test server_contract

python3 - "$EV" <<'PY'
import json, sys
from pathlib import Path
ev = Path(sys.argv[1])
rows = []
for name in ("23-persistence-failure-fixtures-v2", "24-server-contract-rerun"):
    p = ev / f"{name}.json"
    if p.exists():
        d = json.loads(p.read_text())
        rows.append({"name": d["name"], "exit": d["exit"], "elapsed_seconds": d["elapsed_seconds"]})
print(json.dumps(rows, indent=1))
PY
