#!/bin/zsh
# Round-3 sealed validation: runs AFTER the code fixes are committed, so every
# record executes on a clean, exact source identity (HEAD == tree, no dirt).
# Recorded by run-record.py with source/tree/dirty-seal fields.
set -u
WT=/Users/chen/work/mornlea-f2-37-zcode
EV=$WT/zcode-37-r3-evidence
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

# Refuse to record "sealed" evidence on a dirty tracked tree.
if [ -n "$(git -C "$WT" status --porcelain | grep -v '^??')" ]; then
    echo "tracked worktree is dirty; commit fixes before sealed validation" >&2
    exit 2
fi

CARGO_TEST=(rustup run 1.97.1 cargo test --offline --manifest-path packages/engine/Cargo.toml --target-dir packages/engine/target/cargo -p mornlea_server --locked)

run 31-parity-r3            $CARGO_TEST --test local_remote_parity
run 32-persistence-full-r3  $CARGO_TEST --test persistence_failure
run 33-verifier-nul-r3      $CARGO_TEST --test persistence_failure manifest_batch_embedded_nul
run 34-go-audit-r3          go test ./packages/audit -count=1

shasum -a 256 "$PKG3/previous-server" "$PKG3/previous-verifier" "$PKG3/previous-runtime.json" \
  "$WT/packages/engine/target/cargo/release/mornlea-server" \
  "$WT/packages/engine/target/cargo/debug/mornlea-server" \
  "$VENV/python" > "$EV/selector-identities.txt"

python3 - "$EV" <<'PY'
import json, sys
from pathlib import Path
ev = Path(sys.argv[1])
rows = []
for j in sorted(ev.glob("[0-9][0-9]-*.json")):
    d = json.loads(j.read_text())
    rows.append({"name": d["name"], "exit": d["exit"], "elapsed_seconds": d["elapsed_seconds"],
                 "source": d.get("source"), "tree": d.get("tree"), "sealed": d.get("sealed")})
(ev / "summary.json").write_text(json.dumps(rows, indent=1) + "\n")
print(json.dumps(rows, indent=1))
PY
