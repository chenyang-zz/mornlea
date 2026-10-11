#!/usr/bin/env python3
"""Round-3 binding generator: per-row specific evidence, no wide prefixes.

Consumes `bindings-v2.json` (one record per inventory row with class, exact
test names, scenario, preconditions, expected, assertions) and verifies every
cited test name against the recorded passing logs before a row may be BOUND.
A record whose test field is a module-only prefix (ends with '::' or contains
no '::' beyond the module path) is rejected, so wide prefixes cannot pass.
"""
import json
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
ROUND1 = ROOT / "zcode-37-evidence"
ROUND2 = ROOT / "zcode-37-review-evidence"
ROUND3 = Path(__file__).resolve().parent
INVENTORY = ROOT / "testdata/runtime-migration/server/capability-inventory.json"
SOURCE = ROUND3 / "bindings-v2.json"

VALID_CLASSES = {"admission", "provider", "order-guard", "real-integration"}

_CACHE: dict[str, list[str]] = {}


def log_lines(name: str) -> list[str]:
    for base in (ROUND3, ROUND2, ROUND1):
        path = base / name
        if path.exists():
            return path.read_text(errors="replace").splitlines()
    return []


def verified_in(log: str, test: str) -> bool:
    if log not in _CACHE:
        _CACHE[log] = log_lines(log)
    return any(
        line.startswith(f"test {test} ") and line.rstrip().endswith(" ok")
        for line in _CACHE[log]
    )


def is_specific(test: str) -> bool:
    # Exact test names carry at least module::function and a function segment
    # after the final '::' that is longer than empty.
    if "::" not in test or test.endswith("::"):
        return False
    tail = test.rsplit("::", 1)[1]
    return len(tail) > 0 and not tail.endswith("_")


def main() -> int:
    rows = json.loads(INVENTORY.read_text())["rows"]
    records = {record["id"]: record for record in json.loads(SOURCE.read_text())}
    table, problems = [], []
    for row in rows:
        rid = row["id"]
        record = records.get(rid)
        if record is None:
            problems.append(f"{rid}: no binding record")
            table.append({"id": rid, "status": "OPEN"})
            continue
        klass = record.get("class")
        if klass not in VALID_CLASSES:
            problems.append(f"{rid}: bad class {klass!r}")
        tests = [("test", record.get("test"))]
        if record.get("test2"):
            tests.append(("test2", record["test2"]))
        if record.get("transport"):
            tests.append(("transport", record["transport"]))
        evidence, ok = [], True
        for role, test in tests:
            if not test:
                if role == "test":
                    ok = False
                    problems.append(f"{rid}: missing primary test")
                continue
            if role != "transport" and not is_specific(test):
                ok = False
                problems.append(f"{rid}: non-specific test name {test!r}")
            log = record.get("log") if role == "test" else record.get(
                "log2" if role == "test2" else "transport_log"
            )
            hit = bool(log) and verified_in(log, test)
            evidence.append({"role": role, "test": test, "log": log, "verified": hit})
            if role in ("test", "test2") and not hit:
                ok = False
                problems.append(f"{rid}: {role} {test!r} not verified in {log!r}")
        for field in ("scenario", "preconditions", "expected", "assertions"):
            if not record.get(field):
                ok = False
                problems.append(f"{rid}: missing {field}")
        table.append({
            "id": rid,
            "status": "BOUND" if ok else "OPEN",
            "class": klass,
            "scenario": record.get("scenario"),
            "preconditions": record.get("preconditions"),
            "expected": record.get("expected"),
            "assertions": record.get("assertions"),
            "evidence": evidence,
            "gap": record.get("gap"),
        })

    (ROUND3 / "inventory-binding-v2.json").write_text(json.dumps(table, indent=1) + "\n")
    md = ["# 3.7 capability inventory bindings (round 3)", "",
          "Per-row specific evidence; wide prefixes are rejected by the generator.",
          "Classes: admission / provider / order-guard / real-integration.", ""]
    for entry in table:
        md.append(f"## {entry['id']} — {entry['status']} ({entry.get('class')})")
        if entry.get("scenario"):
            md.append(f"- scenario: {entry['scenario']}")
        for label, key in (("precondition", "preconditions"), ("expected", "expected"), ("assertion", "assertions")):
            for item in entry.get(key) or []:
                md.append(f"- {label}: {item}")
        for ev in entry.get("evidence") or []:
            mark = "✓" if ev["verified"] else "✗"
            md.append(f"- {mark} [{ev['role']}] `{ev['test']}` in `{ev['log']}`")
        if entry.get("gap"):
            md.append(f"- gap: {entry['gap']}")
    (ROUND3 / "inventory-binding-v2.md").write_text("\n".join(md) + "\n")

    bound = sum(1 for e in table if e["status"] == "BOUND")
    print(f"rows={len(table)} bound={bound} open={len(table) - bound}")
    for problem in problems:
        print("PROBLEM", problem)
    return 0 if bound == len(table) else 1


if __name__ == "__main__":
    sys.exit(main())
