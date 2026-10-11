#!/usr/bin/env python3
"""Record one evidence command: argv, cwd, timing, exit, env subset, full log.

Each invocation writes <name>.log and <name>.json into the evidence directory
next to this script, mirroring the repository's established mac-evidence
record format. Nothing here edits tracked files.
"""
import json
import os
import subprocess
import sys
import time
from datetime import datetime, timezone
from pathlib import Path

EVIDENCE = Path(__file__).resolve().parent
WORKTREE = Path(__file__).resolve().parent.parent


def record(name: str, argv: list[str]) -> int:
    env = dict(os.environ)
    recorded_env = {
        k: env.get(k)
        for k in (
            "MORNLEA_AGENT_PYTHON",
            "PYTHONPATH",
            "TMPDIR",
            "CARGO_NET_OFFLINE",
            "CARGO_BUILD_JOBS",
            "GOWORK",
            "GOFLAGS",
            "GOPROXY",
            "GOCACHE",
            "GOMAXPROCS",
            "PATH",
        )
    }
    start = datetime.now(timezone.utc)
    t0 = time.monotonic()
    proc = subprocess.run(
        argv,
        cwd=WORKTREE,
        env=env,
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
        timeout=7200,
    )
    elapsed = time.monotonic() - t0
    end = datetime.now(timezone.utc)
    log_path = EVIDENCE / f"{name}.log"
    log_path.write_bytes(proc.stdout)
    head_sha = subprocess.run(
        ["git", "rev-parse", "HEAD"],
        cwd=WORKTREE,
        capture_output=True,
        text=True,
        check=True,
    ).stdout.strip()
    status = subprocess.run(
        ["git", "status", "--porcelain"],
        cwd=WORKTREE,
        capture_output=True,
        text=True,
        check=True,
    ).stdout.strip()
    payload = {
        "name": name,
        "argv": argv,
        "cwd": str(WORKTREE),
        "start": start.isoformat(),
        "end": end.isoformat(),
        "elapsed_seconds": round(elapsed, 3),
        "exit": proc.returncode,
        "source": head_sha,
        "worktree_status_porcelain": status,
        "env": recorded_env,
    }
    (EVIDENCE / f"{name}.json").write_text(json.dumps(payload, indent=1) + "\n")
    tail = proc.stdout[-800:].decode("utf-8", "replace")
    print(f"[{name}] exit={proc.returncode} elapsed={elapsed:.1f}s")
    print(tail)
    return proc.returncode


if __name__ == "__main__":
    if len(sys.argv) < 3:
        print("usage: run-record.py <name> <argv...>", file=sys.stderr)
        raise SystemExit(2)
    raise SystemExit(record(sys.argv[1], sys.argv[2:]))
