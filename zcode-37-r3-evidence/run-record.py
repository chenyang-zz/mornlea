#!/usr/bin/env python3
"""Round-3 evidence recorder: seals execution identity with the source tree.

Extends the earlier recorder with the identity sealing the review requires:
every record captures HEAD, its tree hash, and the tracked-dirty state at
execution time. A run is marked "sealed" only when the tracked worktree is
clean; otherwise the record carries the SHA256 of the dirty diff so history
is never reconstructed from later file equality.
"""
import hashlib
import json
import os
import subprocess
import sys
import time
from datetime import datetime, timezone
from pathlib import Path

EVIDENCE = Path(__file__).resolve().parent
WORKTREE = Path(__file__).resolve().parent.parent


def git(args: list[str]) -> str:
    return subprocess.run(
        ["git", *args], cwd=WORKTREE, capture_output=True, text=True, check=True
    ).stdout.strip()


def record(name: str, argv: list[str]) -> int:
    env = dict(os.environ)
    head = git(["rev-parse", "HEAD"])
    tree = git(["rev-parse", "HEAD^{tree}"])
    status = git(["status", "--porcelain"])
    tracked_dirty = "\n".join(
        line for line in status.splitlines() if not line.startswith("??")
    )
    dirty_diff_sha = None
    if tracked_dirty:
        diff = subprocess.run(
            ["git", "diff", "HEAD"], cwd=WORKTREE, capture_output=True, check=True
        ).stdout
        dirty_diff_sha = hashlib.sha256(diff).hexdigest()
    recorded_env = {
        k: env.get(k)
        for k in (
            "MORNLEA_AGENT_PYTHON",
            "MORNLEA_PREVIOUS_PACKAGE",
            "MORNLEA_PREVIOUS_SERVER_BIN",
            "MORNLEA_RUST_SERVER_BIN",
            "PYTHONPATH",
            "TMPDIR",
            "CARGO_NET_OFFLINE",
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
        argv, cwd=WORKTREE, env=env,
        stdout=subprocess.PIPE, stderr=subprocess.STDOUT, timeout=7200,
    )
    elapsed = time.monotonic() - t0
    end = datetime.now(timezone.utc)
    (EVIDENCE / f"{name}.log").write_bytes(proc.stdout)
    payload = {
        "name": name,
        "argv": argv,
        "cwd": str(WORKTREE),
        "start": start.isoformat(),
        "end": end.isoformat(),
        "elapsed_seconds": round(elapsed, 3),
        "exit": proc.returncode,
        "source": head,
        "tree": tree,
        "tracked_dirty_at_start": tracked_dirty or None,
        "dirty_diff_sha256": dirty_diff_sha,
        "sealed": tracked_dirty == "",
        "env": recorded_env,
    }
    (EVIDENCE / f"{name}.json").write_text(json.dumps(payload, indent=1) + "\n")
    tail = proc.stdout[-600:].decode("utf-8", "replace")
    print(f"[{name}] exit={proc.returncode} elapsed={elapsed:.1f}s "
          f"sealed={payload['sealed']} source={head[:12]}")
    print(tail)
    return proc.returncode


if __name__ == "__main__":
    if len(sys.argv) < 3:
        print("usage: run-record.py <name> <argv...>", file=sys.stderr)
        raise SystemExit(2)
    raise SystemExit(record(sys.argv[1], sys.argv[2:]))
