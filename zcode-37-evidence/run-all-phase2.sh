#!/bin/zsh
# 3.7 evidence phase 2: fixture-env completion and gate-form repetition.
# - Re-runs the one server_contract failure three times to classify flake vs defect.
# - Re-runs persistence_failure with the sealed previous-package fixtures
#   (reused read-only from the established ROOT mac-evidence custody; identities
#   are rehashed here and recorded).
# - Repeats the 3.6d gate form required by refined node 3.7: agent_process
#   --nocapture, companion-agent-check, companion-agent-integration, and the
#   pytest shutdown case.
set -u
WT=/Users/chen/work/mornlea-f2-37-zcode
EV=$WT/zcode-37-evidence
PKG3=/Users/chen/Documents/Codex/2026-10-03/task-3/mornlea-f2-mac-20261003/mac-evidence/previous-package3
RUST_BIN=$WT/packages/engine/target/cargo/release/mornlea-server

export PATH="$HOME/.cargo/bin:/opt/homebrew/bin:$HOME/.gvm/pkgsets/go1.26.0/global/bin:$HOME/.gvm/gos/go1.26.0/bin:$HOME/.gvm/pkgsets/go1.26.0/global/overlay/bin:$HOME/.gvm/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin"
# 19 runs before 20 creates this worktree's venv, so reuse the already
# proven interpreter from the primary worktree (read-only fixture use).
export MORNLEA_AGENT_PYTHON=/Users/chen/work/mornlea/packages/agent/.venv/bin/python
export PYTHONPATH="$WT/packages/agent/app/src:$WT/packages/agent/harness/src:$WT/packages/agent/extension-api/src"
export TMPDIR=/private/tmp/f2z37
export CARGO_NET_OFFLINE=true
export PYTHONDONTWRITEBYTECODE=1
export GOWORK=auto
export GOFLAGS=-mod=readonly
export GOPROXY=off
export GOCACHE=$EV/go-cache
export GOMAXPROCS=4
export MORNLEA_PREVIOUS_PACKAGE=$PKG3
export MORNLEA_PREVIOUS_SERVER_BIN=$PKG3/previous-server
export MORNLEA_RUST_SERVER_BIN=$RUST_BIN

run() { python3 "$EV/run-record.py" "$@"; }

echo "== fixture identities =="
shasum -a 256 "$PKG3/previous-server" "$PKG3/previous-verifier" "$RUST_BIN" > "$EV/fixture-identities.txt" 2>&1 || true
cat "$EV/fixture-identities.txt"

CARGO_TEST=(rustup run 1.97.1 cargo test --offline --manifest-path packages/engine/Cargo.toml --target-dir packages/engine/target/cargo -p mornlea_server --locked)

run 15-mcp-seventeenth-retry1 $CARGO_TEST --test server_contract agent_mcp::lifecycle::seventeenth_connection_is_closed_without_tool_dispatch
run 16-mcp-seventeenth-retry2 $CARGO_TEST --test server_contract agent_mcp::lifecycle::seventeenth_connection_is_closed_without_tool_dispatch
run 17-mcp-seventeenth-retry3 $CARGO_TEST --test server_contract agent_mcp::lifecycle::seventeenth_connection_is_closed_without_tool_dispatch

run 18-persistence-failure-with-fixtures $CARGO_TEST --test persistence_failure

run 19-agent-process-nocapture $CARGO_TEST --test agent_process -- --nocapture

run 20-companion-agent-check make -C "$WT" companion-agent-check
run 21-companion-agent-integration make -C "$WT" companion-agent-integration
run 22-pytest-shutdown-case zsh -c "cd '$WT/packages/agent' && uv run pytest -q tests/test_http_v1.py::test_shutdown_stops_accepting_cancels_runs_then_closes_model_and_sqlite"

python3 - "$EV" <<'PY'
import json, sys
from pathlib import Path
ev = Path(sys.argv[1])
rows = []
for j in sorted(ev.glob("1[5-9]-*.json")) + sorted(ev.glob("2[0-2]-*.json")):
    d = json.loads(j.read_text())
    rows.append({"name": d["name"], "exit": d["exit"], "elapsed_seconds": d["elapsed_seconds"]})
print(json.dumps(rows, indent=1))
PY
