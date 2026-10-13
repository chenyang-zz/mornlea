"""Tests for server simulation authority, mutation lifecycles, and core domain invariants."""

import os
from pathlib import Path
import re

import pytest


def _extract_struct_fields(content: str, struct_name: str) -> dict[str, str]:
    """Parse struct fields from Go source text using balanced brace matching."""
    clean = re.sub(r"/\*.*?\*/", "", content, flags=re.DOTALL)
    m = re.search(rf"(?m)^type\s+{struct_name}\s+struct\s*\{{", clean)
    if not m:
        raise ValueError(f"Struct {struct_name} not found")

    start = m.end()
    depth = 1
    i = start
    body = ""
    while i < len(clean) and depth > 0:
        if clean[i] == "{":
            depth += 1
        elif clean[i] == "}":
            depth -= 1
            if depth == 0:
                body = clean[start:i]
                break
        i += 1

    fields: dict[str, str] = {}
    for raw in body.splitlines():
        line = raw.split("//")[0].strip()
        if not line:
            continue
        parts = line.split(maxsplit=1)
        if len(parts) == 2:
            fields[parts[0]] = parts[1].strip()

    return fields


def test_sim_authority_engine_struct_fields(repo_root: Path) -> None:
    """Verify runtime.Engine struct fields match authoritative simulation shape exactly."""
    engine_file = repo_root / "packages" / "server" / "sim" / "runtime" / "engine.go"
    assert engine_file.is_file(), "packages/server/sim/runtime/engine.go must exist"

    fields = _extract_struct_fields(engine_file.read_text(encoding="utf-8"), "Engine")

    expected_runtime_engine_fields = {
        "viewRadius": "int",
        "seed": "int64",
        "subscriptions": "map[SessionID]*subscriptionState",
        "wanted": "map[core.ChunkKey]struct{}",
        "realm": "*realm.State",
        "entities": "*entity.State",
        "subscriptionsDirty": "bool",
        "entityViewScratch": "[]entity.TickSessionView",
        "activeChunkScratch": "[]core.ChunkKey",
        "inboxMu": "sync.Mutex",
        "commands": "[]Command",
        "companionActions": "[]CompanionAction",
        "hostileActions": "[]HostileAction",
        "acquired": "[]AcquiredChunk",
        "generated": "[]GeneratedChunk",
        "tick": "atomic.Uint64",
        "worldTime": "atomic.Uint64",
        "dayPhaseOffset": "atomic.Uint64",
        "weatherKind": "core.WeatherKind",
        "weatherRemaining": "uint32",
        "seasonOffset": "uint64",
        "stepPhaseObserver": "func(stepPhase)",
        "tunables": "tuning.Tunables",
        "physicsTunables": "physics.Tunables",
    }

    assert fields == expected_runtime_engine_fields, (
        f"runtime.Engine struct field mismatch:\n"
        f"Missing: {set(expected_runtime_engine_fields) - set(fields)}\n"
        f"Unexpected: {set(fields) - set(expected_runtime_engine_fields)}"
    )


def test_sim_authority_subscription_struct_fields(repo_root: Path) -> None:
    """Verify runtime.subscriptionState struct fields match authoritative shape exactly."""
    engine_file = repo_root / "packages" / "server" / "sim" / "runtime" / "engine.go"
    assert engine_file.is_file()

    fields = _extract_struct_fields(engine_file.read_text(encoding="utf-8"), "subscriptionState")

    expected_runtime_subscription_fields = {
        "lastSequence": "uint64",
        "lastTrustedObserverSequence": "uint64",
        "trustedObserver": "bool",
        "hasView": "bool",
        "dimension": "core.DimensionID",
        "center": "core.ChunkPos",
        "radius": "int",
        "wanted": "map[core.ChunkKey]struct{}",
    }

    assert fields == expected_runtime_subscription_fields, (
        f"runtime.subscriptionState struct field mismatch:\n"
        f"Missing: {set(expected_runtime_subscription_fields) - set(fields)}\n"
        f"Unexpected: {set(fields) - set(expected_runtime_subscription_fields)}"
    )


def test_sim_authority_entity_state_fields(repo_root: Path) -> None:
    """Verify entity.State and entity.sessionState struct fields contain required fields."""
    entity_file = repo_root / "packages" / "server" / "sim" / "entity" / "engine.go"
    assert entity_file.is_file(), "packages/server/sim/entity/engine.go must exist"

    content = entity_file.read_text(encoding="utf-8")
    state_fields = _extract_struct_fields(content, "State")
    session_fields = _extract_struct_fields(content, "sessionState")

    required_entity_state_fields = {
        "sessions": "map[SessionID]*sessionState",
        "companions": "map[companion.ID]*companionState",
        "hostiles": "hostileSet",
        "passives": "passiveSet",
        "projectiles": "projectileSet",
    }

    for name, expected_type in required_entity_state_fields.items():
        assert name in state_fields, f"entity.State missing required field {name}"
        assert state_fields[name] == expected_type, (
            f"entity.State field {name} has type {state_fields[name]}, expected {expected_type}"
        )

    required_entity_session_fields = {
        "player": "*playerState",
        "container": "core.ContainerRef",
    }

    for name, expected_type in required_entity_session_fields.items():
        assert name in session_fields, f"entity.sessionState missing required field {name}"
        assert session_fields[name] == expected_type, (
            f"entity.sessionState field {name} has type {session_fields[name]}, expected {expected_type}"
        )


def test_sim_authority_mutation_and_owners(repo_root: Path) -> None:
    """Verify runtime owner counts and deterministic NewMutation/Commit lifecycle."""
    runtime_dir = repo_root / "packages" / "server" / "sim" / "runtime"
    assert runtime_dir.is_dir()

    # Engine owner field count checks
    engine_file = runtime_dir / "engine.go"
    engine_fields = _extract_struct_fields(engine_file.read_text(encoding="utf-8"), "Engine")
    realm_owners = sum(1 for f_type in engine_fields.values() if f_type == "*realm.State")
    entity_owners = sum(1 for f_type in engine_fields.values() if f_type == "*entity.State")
    assert realm_owners == 1, f"runtime.Engine must have exactly 1 *realm.State owner, got {realm_owners}"
    assert entity_owners == 1, f"runtime.Engine must have exactly 1 *entity.State owner, got {entity_owners}"

    # Verify calls to NewMutation and Commit in production code
    new_mutation_calls = 0
    commit_calls = 0

    for p in runtime_dir.glob("*.go"):
        if p.name.endswith("_test.go"):
            continue
        content = p.read_text(encoding="utf-8")
        clean = re.sub(r"/\*.*?\*/", "", content, flags=re.DOTALL)
        for line in clean.splitlines():
            code = line.split("//")[0]
            if re.search(r"\bNewMutation\s*\(", code):
                new_mutation_calls += 1
            if re.search(r"\.Commit\s*\(", code):
                commit_calls += 1

    assert new_mutation_calls == 1, f"runtime production must call NewMutation exactly once, got {new_mutation_calls}"
    assert commit_calls == 1, f"runtime production must call Commit exactly once, got {commit_calls}"

    # Verify mutation block in engine_step.go has no async go or defer statements
    step_file = runtime_dir / "engine_step.go"
    step_content = step_file.read_text(encoding="utf-8")
    assert "NewMutation" in step_content and "Commit" in step_content

    lines = step_content.splitlines()
    start_idx = -1
    end_idx = -1
    for idx, l in enumerate(lines):
        if "NewMutation" in l and start_idx == -1:
            start_idx = idx
        if "Commit" in l and start_idx != -1:
            end_idx = idx
            break

    assert start_idx >= 0 and end_idx >= 0 and start_idx < end_idx
    mutation_block = "\n".join(lines[start_idx : end_idx + 1])
    mutation_clean = re.sub(r"/\*.*?\*/", "", mutation_block, flags=re.DOTALL)
    mutation_code = "\n".join(l.split("//")[0] for l in mutation_clean.splitlines())
    assert "go " not in mutation_code, "Mutation lifecycle must not spawn goroutines"
    assert "defer " not in mutation_code, "Mutation lifecycle must not contain defer statements"


def test_difficulty_domain_stays_single_core(repo_root: Path) -> None:
    """Verify world difficulty remains a single core domain with client and network blindness."""
    core_diff_path = "packages/shared/core/difficulty.go"
    blind_roots = ["packages/client", "packages/contracts", "packages/shared/network"]
    canonical_difficulty_texts = {'"normal"', '"peaceful"', '"hard"'}

    for root, dirs, files in os.walk(repo_root / "packages"):
        dirs[:] = [d for d in dirs if d not in {".git", ".venv", ".pytest_cache", "node_modules", "target", "__pycache__"}]
        for f in files:
            if not f.endswith(".go") or f.endswith("_test.go"):
                continue

            file_path = Path(root) / f
            rel_path = file_path.relative_to(repo_root).as_posix()
            content = file_path.read_text(encoding="utf-8")
            clean = re.sub(r"/\*.*?\*/", "", content, flags=re.DOTALL)

            # Check: Difficulty type declaration only allowed in packages/shared/core/difficulty.go
            if re.search(r"(?m)^type\s+Difficulty\b", clean) and rel_path != core_diff_path:
                pytest.fail(f"{rel_path} declares Difficulty type outside {core_diff_path}")

            # Check: Blind roots must have zero difficulty mentions in code
            is_blind = any(rel_path.startswith(b) for b in blind_roots)
            for line_idx, raw_line in enumerate(clean.splitlines(), start=1):
                code = raw_line.split("//")[0]
                if is_blind and "difficulty" in code.lower():
                    pytest.fail(
                        f"{rel_path}:{line_idx}: blind subtree mentions difficulty in code: '{code.strip()}'"
                    )

                # Canonical text check: "normal", "peaceful", "hard" as standalone string literals
                if rel_path != core_diff_path:
                    for text_lit in canonical_difficulty_texts:
                        if text_lit in code:
                            pytest.fail(
                                f"{rel_path}:{line_idx}: canonical difficulty text {text_lit} "
                                f"only allowed in {core_diff_path}"
                            )


def test_armor_domain_stays_single_core(repo_root: Path) -> None:
    """Verify armor domain symbols and durability tables remain confined to shared/core."""
    core_armor_dir = "packages/shared/core"
    armor_symbols = {"ArmorSlot", "ArmorSlotOf", "ArmorPoints", "ReducedDamage", "MaxArmorPoints"}
    table_fingerprints = {"165", "240", "225", "195"}

    for root, dirs, files in os.walk(repo_root / "packages"):
        dirs[:] = [d for d in dirs if d not in {".git", ".venv", ".pytest_cache", "node_modules", "target", "__pycache__"}]
        for f in files:
            if not f.endswith(".go") or f.endswith("_test.go"):
                continue

            file_path = Path(root) / f
            rel_path = file_path.relative_to(repo_root).as_posix()
            content = file_path.read_text(encoding="utf-8")
            clean = re.sub(r"/\*.*?\*/", "", content, flags=re.DOTALL)

            in_core = rel_path.startswith(core_armor_dir)

            # Check symbol declarations outside core
            if not in_core:
                for line_idx, raw_line in enumerate(clean.splitlines(), start=1):
                    code = raw_line.split("//")[0].strip()
                    for sym in armor_symbols:
                        # Declarations: type Sym, func Sym, const Sym, var Sym
                        if re.search(rf"^(?:type|func|const|var)\s+(?:\([^)]+\)\s+)?{sym}\b", code):
                            pytest.fail(
                                f"{rel_path}:{line_idx} declares armor domain symbol {sym} outside {core_armor_dir}"
                            )

            # Check table durability fingerprints: outside core, no file may contain 2+ fingerprints
            if not in_core:
                found_fingerprints: set[str] = set()
                for raw_line in clean.splitlines():
                    code = raw_line.split("//")[0]
                    numbers = re.findall(r"\b\d+\b", code)
                    for n in numbers:
                        if n in table_fingerprints:
                            found_fingerprints.add(n)

                assert len(found_fingerprints) < 2, (
                    f"{rel_path} contains durability fingerprints {found_fingerprints}: "
                    "armor tables must not be copied outside shared/core"
                )
