#!/bin/zsh
# 3.7 real-integration evidence runner for the isolated ZCode worktree.
# Runs every named suite and focused Go oracle sequentially, recording each
# through run-record.py. A failing command is recorded and does not stop the
# remaining commands. No tracked file is modified.
set -u
WT=/Users/chen/work/mornlea-f2-37-zcode
EV=$WT/zcode-37-evidence
mkdir -p /private/tmp/f2z37

export PATH="$HOME/.cargo/bin:$HOME/.gvm/pkgsets/go1.26.0/global/bin:$HOME/.gvm/gos/go1.26.0/bin:$HOME/.gvm/pkgsets/go1.26.0/global/overlay/bin:$HOME/.gvm/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin"
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

run() { python3 "$EV/run-record.py" "$@"; }

CARGO_TEST=(rustup run 1.97.1 cargo test --offline --manifest-path packages/engine/Cargo.toml --target-dir packages/engine/target/cargo -p mornlea_server --locked)

run 01-local-remote-parity $CARGO_TEST --test local_remote_parity
run 02-server-replay      $CARGO_TEST --test server_replay
run 03-server-contract    $CARGO_TEST --test server_contract
run 04-persistence-failure $CARGO_TEST --test persistence_failure
run 05-agent-process      $CARGO_TEST --test agent_process

run 06-make-rust make -C "$WT" rust

run 07-go-oracle-step go test ./packages/server/sim/runtime -run 'StepWithTunablesPinsBlockUpdatesSubOrder|StepSequenceGuardRejectsSwappedOrder|PlaceBlockThroughFluid|FarmlandMoisture' -count=1
run 08-go-oracle-network go test ./packages/shared/network -count=1
run 09-go-oracle-login go test ./packages/server/server -run 'Login' -count=1
run 10-go-oracle-local go test ./packages/server/server -run 'Local|Login' -count=1
run 11-go-oracle-tcp go test ./packages/server/server -run 'TCP|Login' -count=1
run 12-go-oracle-persistence go test ./packages/server/server/persistence -run 'Autosave|UrgentSave|SaveJobs|SaveCompletion|SaveError|FullSaveQueue|SaveSelection|SaveFailure|SaveNilError|Retry|Flush|Backpressure|MutationDuring' -count=1
run 13-go-oracle-region go test ./packages/server/storage/chunk -run '^TestRegion' -count=1
run 14-go-oracle-storage go test ./packages/server/storage -run 'WorldLock|DiskStore(Sync|Close)|WorldBackup' -count=1

python3 - "$EV" <<'PY'
import json, sys
from pathlib import Path
ev = Path(sys.argv[1])
rows = []
for j in sorted(ev.glob("[0-9][0-9]-*.json")):
    d = json.loads(j.read_text())
    rows.append({"name": d["name"], "exit": d["exit"], "elapsed_seconds": d["elapsed_seconds"]})
(ev / "summary.json").write_text(json.dumps(rows, indent=1) + "\n")
print(json.dumps(rows, indent=1))
PY
