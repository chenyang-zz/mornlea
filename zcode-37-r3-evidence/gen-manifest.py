#!/usr/bin/env python3
"""Round-3 evidence manifest generator.

Differs from the earlier manifest in two review-mandated ways: the manifest
never lists itself (the previous one hashed the already-truncated output file,
recording the empty-input digest), and every recorded hash is re-verified
after writing. The manifest is bound to repository history by the evidence
commit that contains it, recorded in the round report.
"""
import hashlib
import sys
from pathlib import Path

EVIDENCE = Path(__file__).resolve().parent
MANIFEST = EVIDENCE / "manifest-sha256.txt"
SKIP = {MANIFEST.name}


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1 << 16), b""):
            digest.update(chunk)
    return digest.hexdigest()


def main() -> int:
    entries = []
    for path in sorted(EVIDENCE.iterdir()):
        if not path.is_file() or path.name in SKIP:
            continue
        entries.append((sha256(path), path.name))
    lines = [f"{digest}  {name}" for digest, name in entries]
    MANIFEST.write_text("\n".join(lines) + "\n")
    # Re-verify what was written before reporting success.
    for digest, name in entries:
        if sha256(EVIDENCE / name) != digest:
            print(f"verification failed for {name}", file=sys.stderr)
            return 1
    print(f"entries={len(entries)} self_excluded={MANIFEST.name} verified=all")
    return 0


if __name__ == "__main__":
    sys.exit(main())
