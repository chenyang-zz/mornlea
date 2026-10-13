"""Tests for source guards, platform boundaries, tuning constraints, and import rules."""

import os
from pathlib import Path
import re
import subprocess

import pytest

from audit.common import run_cmd

MODULE_UNIT_PREFIX = "github.com/channing771/mornlea/packages/"


def _extract_go_imports(content: str) -> list[str]:
    """Extract raw import paths from Go source code."""
    text = re.sub(r"/\*.*?\*/", "", content, flags=re.DOTALL)
    imports: list[str] = []
    in_import_block = False

    for raw in text.splitlines():
        line = raw.split("//")[0].strip()
        if not line:
            continue

        if in_import_block:
            if line == ")" or line.startswith(")"):
                in_import_block = False
                continue
            match = re.search(r'\"([^\"]+)\"', line)
            if match:
                imports.append(match.group(1).strip())
        else:
            if line.startswith("import (") or line == "import(":
                in_import_block = True
                continue
            if line.startswith("import ") or line.startswith("import\t"):
                rest = line[len("import") :].strip()
                if rest == "(":
                    in_import_block = True
                    continue
                match = re.search(r'\"([^\"]+)\"', rest)
                if match:
                    imports.append(match.group(1).strip())

    return imports


def _extract_top_level_declarations(content: str) -> set[str]:
    """Extract top-level declared names (const, var, type, func) from Go source code."""
    text = re.sub(r"/\*.*?\*/", "", content, flags=re.DOTALL)
    names: set[str] = set()
    in_const_var_block = False
    in_type_block = False

    for raw in text.splitlines():
        line = raw.split("//")[0].strip()
        if not line:
            continue

        if in_const_var_block:
            if line == ")" or line.startswith(")"):
                in_const_var_block = False
                continue
            m = re.match(r"^([A-Za-z0-9_]+)", line)
            if m:
                names.add(m.group(1))
            continue

        if in_type_block:
            if line == ")" or line.startswith(")"):
                in_type_block = False
                continue
            m = re.match(r"^([A-Za-z0-9_]+)", line)
            if m:
                names.add(m.group(1))
            continue

        if line.startswith("const (") or line.startswith("var ("):
            in_const_var_block = True
            continue
        if line.startswith("type ("):
            in_type_block = True
            continue

        m = re.match(r"^(?:const|var)\s+([A-Za-z0-9_]+)", line)
        if m:
            names.add(m.group(1))
            continue

        m = re.match(r"^type\s+([A-Za-z0-9_]+)", line)
        if m:
            names.add(m.group(1))
            continue

        m = re.match(r"^func\s+(?:\([^)]+\)\s+)?([A-Za-z0-9_]+)", line)
        if m:
            names.add(m.group(1))
            continue

    return names


def test_no_package_imports_webgpu(repo_root: Path) -> None:
    """Verify that no Go package in the workspace imports WebGPU bindings."""
    packages_dir = repo_root / "packages"
    violations: list[str] = []

    for root, dirs, files in os.walk(packages_dir):
        dirs[:] = [d for d in dirs if d not in {".git", ".venv", ".pytest_cache", "node_modules", "target", "__pycache__"}]
        for f in files:
            if not f.endswith(".go"):
                continue
            file_path = Path(root) / f
            rel_path = file_path.relative_to(repo_root).as_posix()
            content = file_path.read_text(encoding="utf-8", errors="replace")
            imports = _extract_go_imports(content)
            for imp in imports:
                if "webgpu" in imp.lower():
                    violations.append(f"{rel_path} imports WebGPU binding {imp}")

    assert not violations, f"WebGPU import violations found:\n" + "\n".join(violations)


def test_tunable_constants_are_not_exported(repo_root: Path) -> None:
    """Verify that tunable constants in physics and tuning are not exported package-level constants."""
    tunable_packages: dict[str, list[str]] = {
        "packages/shared/physics": [
            "EyeHeight", "StepHeight", "WalkSpeed", "GroundAcceleration",
            "GroundDeceleration", "AirAcceleration", "JumpSpeed", "Gravity",
            "TerminalFallSpeed", "FluidGravity", "FluidSinkSpeed",
            "FluidAscendSpeed", "FluidHorizontalDrag",
        ],
        "packages/shared/tuning": [
            "RegenDelayTicks", "RegenIntervalTicks", "DropPickupDelayTicks",
            "PlayerDropPickupDelayTicks", "DropLifetimeTicks",
            "InteractionReach", "DropPickupRange", "SpawnRadius",
            "RandomTicksPerSection", "CropGrowthChancePercent",
            "StarvationDamageIntervalTicks", "ExhaustionThresholdMilli",
            "RegenHungerThreshold", "EatingTicks",
        ],
    }

    violations: list[str] = []

    for pkg_rel, banned_names in tunable_packages.items():
        pkg_dir = repo_root / pkg_rel
        assert pkg_dir.is_dir(), f"{pkg_rel} directory must exist"
        banned_set = set(banned_names)

        go_files = [p for p in pkg_dir.glob("*.go") if not p.name.endswith("_test.go")]
        assert go_files, f"{pkg_rel} must contain production Go files"

        for file_path in go_files:
            content = file_path.read_text(encoding="utf-8")
            # Extract package-level const and var declarations
            text = re.sub(r"/\*.*?\*/", "", content, flags=re.DOTALL)
            in_const_var_block = False
            for raw in text.splitlines():
                line = raw.split("//")[0].strip()
                if not line:
                    continue
                if in_const_var_block:
                    if line.startswith(")"):
                        in_const_var_block = False
                        continue
                    m = re.match(r"^([A-Za-z0-9_]+)", line)
                    if m and m.group(1) in banned_set:
                        violations.append(f"{file_path.name}: tunable {m.group(1)} exported in const/var block")
                    continue

                if line.startswith("const (") or line.startswith("var ("):
                    in_const_var_block = True
                    continue

                m = re.match(r"^(?:const|var)\s+([A-Za-z0-9_]+)", line)
                if m and m.group(1) in banned_set:
                    violations.append(f"{file_path.name}: tunable {m.group(1)} exported as const/var")

    assert not violations, f"Tunable constant export violations found:\n" + "\n".join(violations)


def test_tunable_defaults_are_only_read_in_tunables_file(repo_root: Path) -> None:
    """Verify default tunable identifiers are only referenced in tunables.go."""
    packages_to_check = [
        repo_root / "packages" / "shared" / "physics",
        repo_root / "packages" / "shared" / "tuning",
    ]
    violations: list[str] = []

    for pkg_dir in packages_to_check:
        assert pkg_dir.is_dir()
        go_files = [p for p in pkg_dir.glob("*.go") if not p.name.endswith("_test.go") and p.name != "tunables.go"]
        for file_path in go_files:
            content = file_path.read_text(encoding="utf-8")
            text = re.sub(r"/\*.*?\*/", "", content, flags=re.DOTALL)
            # Find declaration names in this file
            decl_names = _extract_top_level_declarations(content)

            for line_idx, raw_line in enumerate(text.splitlines(), start=1):
                code_line = raw_line.split("//")[0]
                matches = re.findall(r"\bdefault[A-Z]\w*\b", code_line)
                for m in matches:
                    if m not in decl_names:
                        violations.append(f"{file_path.name}:{line_idx} reads compiled default {m}")

    assert not violations, f"Tunable default read violations found:\n" + "\n".join(violations)


def test_only_tcp_implementation_imports_net(repo_root: Path) -> None:
    """Verify that in shared/network, only tcp.go imports the standard net package."""
    net_dir = repo_root / "packages" / "shared" / "network"
    assert net_dir.is_dir()

    violations: list[str] = []
    for file_path in net_dir.glob("*.go"):
        content = file_path.read_text(encoding="utf-8")
        imports = _extract_go_imports(content)
        if "net" in imports and file_path.name != "tcp.go":
            violations.append(f"{file_path.name} imports net; only tcp.go may import net")

    assert not violations, f"Network package net import violations:\n" + "\n".join(violations)


def test_only_commands_import_config(repo_root: Path) -> None:
    """Verify that shared/config is only imported by command entrypoints."""
    config_pkg = "github.com/channing771/mornlea/packages/shared/config"
    allowed_prefixes = (
        "packages/client/cmd/mornlea",
        "packages/server/cmd/mornlea-server",
        "packages/shared/config",
    )
    violations: list[str] = []

    for root, dirs, files in os.walk(repo_root / "packages"):
        dirs[:] = [d for d in dirs if d not in {".git", ".venv", ".pytest_cache", "node_modules", "target", "__pycache__"}]
        for f in files:
            if not f.endswith(".go") or f.endswith("_test.go"):
                continue
            file_path = Path(root) / f
            rel_path = file_path.relative_to(repo_root).as_posix()
            if any(rel_path.startswith(prefix) for prefix in allowed_prefixes):
                continue

            content = file_path.read_text(encoding="utf-8")
            imports = _extract_go_imports(content)
            if config_pkg in imports:
                violations.append(f"{rel_path} imports config; only command entrypoints may import config")

    assert not violations, f"Config import violations:\n" + "\n".join(violations)


def test_physics_step_uses_only_native(repo_root: Path) -> None:
    """Verify that physics Step delegates directly to nativeabi.PhysicsStep with no Go collision resolvers."""
    phys_dir = repo_root / "packages" / "shared" / "physics"
    assert phys_dir.is_dir()

    found_native_abi = False
    native_physics_step_calls = 0
    forbidden_resolvers = {"resolveMove", "clipAxis", "resolveStepMove"}
    violations: list[str] = []

    for file_path in phys_dir.glob("*.go"):
        if file_path.name.endswith("_test.go"):
            continue
        content = file_path.read_text(encoding="utf-8")
        imports = _extract_go_imports(content)
        if "github.com/channing771/mornlea/packages/shared/nativeabi" in imports:
            found_native_abi = True

        clean = re.sub(r"/\*.*?\*/", "", content, flags=re.DOTALL)
        for line in clean.splitlines():
            code = line.split("//")[0]
            if "nativeabi.PhysicsStep" in code:
                native_physics_step_calls += 1
            for res in forbidden_resolvers:
                if re.search(rf"\b{res}\b", code):
                    violations.append(f"{file_path.name} retains Go collision resolver {res}")

    assert found_native_abi, "packages/shared/physics must directly import nativeabi"
    assert native_physics_step_calls == 1, "packages/shared/physics must call nativeabi.PhysicsStep exactly once"
    assert not violations, f"Forbidden collision resolver violations:\n" + "\n".join(violations)


def test_core_uses_only_native_raycast(repo_root: Path) -> None:
    """Verify that core RaycastBlocks delegates to nativeabi.RaycastBatch with no Go DDA resolvers."""
    core_dir = repo_root / "packages" / "shared" / "core"
    raycast_file = core_dir / "raycast.go"
    assert raycast_file.is_file()

    raycast_content = raycast_file.read_text(encoding="utf-8")
    imports = _extract_go_imports(raycast_content)
    assert "github.com/channing771/mornlea/packages/shared/nativeabi" in imports, "core/raycast.go must import nativeabi"

    assert "nativeabi.RaycastBatch" in raycast_content, "core/raycast.go must call nativeabi.RaycastBatch"

    forbidden_dda = {"tDelta", "tMax", "entryFace"}
    violations: list[str] = []

    for file_path in core_dir.glob("*.go"):
        if file_path.name.endswith("_test.go"):
            continue
        content = file_path.read_text(encoding="utf-8")
        clean = re.sub(r"/\*.*?\*/", "", content, flags=re.DOTALL)
        for line in clean.splitlines():
            code = line.split("//")[0]
            for dda in forbidden_dda:
                if re.search(rf"\b{dda}\b", code):
                    violations.append(f"{file_path.name} retains Go DDA variable {dda}")

    assert not violations, f"Forbidden DDA identifiers found:\n" + "\n".join(violations)


def test_server_has_no_graphics_dependencies(repo_root: Path) -> None:
    """Verify that the authoritative server has no graphics or client rendering dependencies."""
    server_cmd_dir = repo_root / "packages" / "server" / "cmd" / "mornlea-server"
    assert server_cmd_dir.is_dir()

    forbidden_packages = [
        "github.com/channing771/mornlea/packages/client/client",
        "github.com/channing771/mornlea/packages/client/mesh",
        "github.com/channing771/mornlea/packages/client/render",
        "github.com/channing771/mornlea/packages/client/gfx",
        "github.com/go-gl/glfw",
        "github.com/oliverbestmann/webgpu",
        "golang.org/x/image",
        "golang.org/x/image/font",
    ]

    for file_path in server_cmd_dir.glob("*.go"):
        if file_path.name.endswith("_test.go"):
            continue
        content = file_path.read_text(encoding="utf-8")
        imports = _extract_go_imports(content)
        for imp in imports:
            for forbidden in forbidden_packages:
                assert not imp.startswith(forbidden), f"{file_path.name} directly imports forbidden graphics dependency {imp}"

    # Transitive dependencies via go list
    res = run_cmd(
        ["go", "list", "-f", "{{.ImportPath}}", "-deps", "./cmd/mornlea-server"],
        cwd=repo_root / "packages" / "server",
    )
    assert res.returncode == 0, f"go list failed: {res.stderr}"

    deps = set(res.stdout.split())
    assert "github.com/channing771/mornlea/packages/shared/nativeabi" in deps, "Server must transitively depend on nativeabi"

    for dep in deps:
        for forbidden in forbidden_packages:
            assert not (dep == forbidden or dep.startswith(forbidden + "/")), (
                f"Server transitively depends on forbidden graphics dependency {dep}"
            )


def test_runtime_corpus_write_guards(repo_root: Path) -> None:
    """Verify that test flag definitions in migration oracle packages cannot rewrite frozen corpus evidence."""
    oracle_dir = repo_root / "packages" / "tools" / "cmd" / "runtime-oracle"
    if not oracle_dir.is_dir():
        return

    write_tokens = {"update", "rewrite", "regen", "regenerate", "write"}
    violations: list[str] = []

    for file_path in oracle_dir.glob("*_test.go"):
        content = file_path.read_text(encoding="utf-8")
        clean = re.sub(r"/\*.*?\*/", "", content, flags=re.DOTALL)
        for line_idx, line in enumerate(clean.splitlines(), start=1):
            code = line.split("//")[0]
            # Match flag declarations: flag.String("name", ...), flags.Bool("name", ...)
            match = re.search(r'flag[s]?\.[A-Za-z0-9_]+\s*\([^"]*"([^"]+)"', code)
            if match:
                flag_name = match.group(1).lower()
                for token in write_tokens:
                    if token in flag_name:
                        violations.append(f"{file_path.name}:{line_idx} exposes corpus writer flag '{match.group(1)}'")
                        break

    assert not violations, f"Runtime corpus writer flag violations:\n" + "\n".join(violations)


def test_session_lifecycle_responsibilities(repo_root: Path) -> None:
    """Verify session lifecycle declarations reside strictly in session*.go and not server.go."""
    server_dir = repo_root / "packages" / "server" / "server"
    assert server_dir.is_dir()

    session_decls: set[str] = set()
    for p in server_dir.glob("session*.go"):
        if not p.name.endswith("_test.go"):
            session_decls.update(_extract_top_level_declarations(p.read_text(encoding="utf-8")))

    server_file = server_dir / "server.go"
    assert server_file.is_file()
    server_decls = _extract_top_level_declarations(server_file.read_text(encoding="utf-8"))

    required_session_symbols = [
        "inputCapacity", "trustedObserverSessionID", "SessionSpec", "SessionExit", "incomingCommand",
        "trustedObserverCenter", "appliedTrustedObserverCenter", "attachSessionLocked", "detachSessionLocked",
        "attachTrustedObserverLocked", "detachTrustedObserverLocked", "setTrustedObserverCenterLocked",
        "appliedTrustedObserverCenterLocked", "endpointReader", "translateClientMessage", "enqueueIncoming",
        "drainIncoming", "drainTrustedObserverCenter", "sortedSessionIDsLocked",
    ]

    for sym in required_session_symbols:
        assert sym in session_decls, f"session*.go must declare lifecycle symbol {sym}"
        assert sym not in server_decls, f"server.go must not declare session lifecycle symbol {sym}"


def test_legacy_player_authority_messages_are_gone(repo_root: Path) -> None:
    """Verify legacy player authority identifiers do not appear in first-party Go packages."""
    forbidden = {"SetViewCenter", "BreakRay", "PlaceRay", "CommandBreakRay", "CommandPlaceRay", "localSessionID"}
    violations: list[str] = []

    for unit in ["packages/shared", "packages/client", "packages/tools", "packages/audit"]:
        unit_dir = repo_root / unit
        if not unit_dir.exists():
            continue
        for root, dirs, files in os.walk(unit_dir):
            dirs[:] = [d for d in dirs if d not in {".git", ".venv", ".pytest_cache", "node_modules", "target", "__pycache__"}]
            for f in files:
                if not f.endswith(".go") or f == "source_guards_test.go":
                    continue
                file_path = Path(root) / f
                rel_path = file_path.relative_to(repo_root).as_posix()
                content = file_path.read_text(encoding="utf-8", errors="replace")
                clean = re.sub(r"/\*.*?\*/", "", content, flags=re.DOTALL)
                for line in clean.splitlines():
                    code = line.split("//")[0]
                    for ident in forbidden:
                        if re.search(rf"\b{ident}\b", code):
                            violations.append(f"{rel_path}: legacy player authority identifier {ident}")

    assert not violations, f"Legacy player authority identifier violations found:\n" + "\n".join(violations)


def test_companion_planner_production_uses_agent_service_only(repo_root: Path) -> None:
    """Verify companion planner production code uses loopback agent service rather than legacy direct models."""
    companion_dir = repo_root / "packages" / "shared" / "companion"
    server_dir = repo_root / "packages" / "server" / "server"

    forbidden_companion = [
        "type PlannerClient struct",
        "func NewPlannerClient(",
        "func (p *PlannerClient) Plan(",
        "type DialogueClient struct",
        "func NewDialogueClient(",
        "func (d *DialogueClient) Do(",
        "type DialogueRequest struct",
        "func NewDialogueRequest(",
        "func DecodeDialogueResponse(",
    ]

    for p in companion_dir.glob("*.go"):
        if p.name.endswith("_test.go"):
            continue
        content = p.read_text(encoding="utf-8")
        for sym in forbidden_companion:
            assert sym not in content, f"{p.name} retains direct planner symbol {sym}"

    forbidden_server = [
        "companion.NewPlannerClient(",
        "companion.NewDialogueClient(",
        "CompanionSummary",
        "slot.summary",
        "companionManagerSummaries(",
    ]

    for p in server_dir.glob("*.go"):
        if p.name.endswith("_test.go"):
            continue
        content = p.read_text(encoding="utf-8")
        for sym in forbidden_server:
            assert sym not in content, f"{p.name} retains direct model construction {sym}"


def test_immutable_message_safety_rules(repo_root: Path) -> None:
    """Verify immutability invariants for cross-goroutine handoffs and message delivery."""
    scheduler_test = repo_root / "packages" / "client" / "lod" / "scheduler_test.go"
    assert scheduler_test.is_file()
    content = scheduler_test.read_text(encoding="utf-8")
    assert "TestWorkerHandoffSlicesImmutableAcrossGoroutines" in content, (
        "LOD scheduler test must verify slice immutability across worker goroutines"
    )

    # Check root AGENTS.md invariant statement
    agents_doc = (repo_root / "AGENTS.md").read_text(encoding="utf-8")
    assert "A message and its slices are immutable after a successful cross-goroutine send." in agents_doc
