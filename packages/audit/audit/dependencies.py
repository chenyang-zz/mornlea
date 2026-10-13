"""Package dependency graph and architectural layer direction verification for Mornlea."""

import json
import os
from pathlib import Path
import re

from audit.common import get_repo_root, run_cmd
from audit.workspace import get_workspace_modules

# Complete mapping of allowed direct internal dependencies for each internal Go package.
# Keys and values are repository-relative paths without the module domain prefix.
ALLOWED_DEPENDENCIES: dict[str, set[str]] = {
    "packages/audit": set(),
    "packages/client/audio": set(),
    "packages/contracts/companion-agent/mcp-v1": set(),
    "packages/server/cmd/mornlea-server": {
        "packages/server/server",
        "packages/server/storage",
        "packages/shared/companion",
        "packages/shared/config",
        "packages/shared/core",
        "packages/shared/logging",
        "packages/shared/network",
        "packages/shared/network/tcp",
        "packages/shared/world",
        "packages/shared/worldgen",
    },
    "packages/shared/companion": {"packages/shared/core", "packages/shared/pathfind"},
    "packages/shared/core": {"packages/shared/nativeabi"},
    "packages/shared/nativeabi": set(),
    "packages/shared/config": {
        "packages/shared/companion",
        "packages/shared/core",
        "packages/shared/physics",
        "packages/shared/tuning",
        "packages/shared/logging",
    },
    "packages/server/fluid": {"packages/server/updates", "packages/shared/core", "packages/shared/nativeabi"},
    "packages/server/updates": {"packages/shared/core"},
    "packages/shared/physics": {"packages/shared/core", "packages/shared/nativeabi"},
    "packages/shared/pathfind": {"packages/shared/core"},
    "packages/shared/logging": set(),
    "packages/shared/network": {"packages/shared/core", "packages/shared/network/codec", "packages/shared/network/protocol"},
    "packages/shared/network/codec": {"packages/shared/core", "packages/shared/network/protocol"},
    "packages/shared/network/protocol": {"packages/shared/companion", "packages/shared/core"},
    "packages/shared/network/tcp": {"packages/shared/network"},
    "packages/shared/profile": {"packages/shared/core"},
    "packages/server/sim/contract": {"packages/shared/companion", "packages/shared/core", "packages/shared/physics", "packages/shared/world"},
    "packages/server/sim/entity": {
        "packages/shared/companion",
        "packages/shared/core",
        "packages/shared/physics",
        "packages/shared/world",
        "packages/server/sim/contract",
        "packages/server/sim/realm",
        "packages/server/updates",
        "packages/shared/tuning",
    },
    "packages/server/sim/realm": {"packages/shared/core", "packages/server/fluid", "packages/server/updates", "packages/shared/world", "packages/shared/worldgen"},
    "packages/server/sim/runtime": {
        "packages/shared/companion",
        "packages/shared/core",
        "packages/shared/physics",
        "packages/shared/world",
        "packages/server/sim/contract",
        "packages/server/sim/entity",
        "packages/server/sim/realm",
        "packages/shared/tuning",
    },
    "packages/shared/tuning": {"packages/shared/core"},
    "packages/server/storage": {
        "packages/shared/core",
        "packages/server/storage/chunk",
        "packages/server/storage/companion",
        "packages/server/storage/hostile",
        "packages/server/storage/passive",
        "packages/server/storage/player",
        "packages/server/storage/region",
        "packages/server/storage/storagedef",
        "packages/shared/world",
    },
    "packages/server/storage/chunk": {"packages/shared/core", "packages/server/storage/region", "packages/server/storage/storagedef", "packages/shared/world"},
    "packages/server/storage/player": {"packages/shared/core", "packages/server/storage/storagedef"},
    "packages/server/storage/companion": {"packages/shared/companion", "packages/shared/core", "packages/server/storage/storagedef"},
    "packages/server/storage/hostile": {"packages/shared/core", "packages/server/storage/storagedef"},
    "packages/server/storage/passive": {"packages/shared/core", "packages/server/storage/storagedef"},
    "packages/server/storage/region": {"packages/shared/core", "packages/server/storage/storagedef"},
    "packages/server/storage/storagedef": set(),
    "packages/shared/world": {"packages/shared/core"},
    "packages/shared/worldgen": {"packages/shared/core", "packages/shared/world", "packages/shared/nativeabi"},
    "packages/client/mesh": {"packages/shared/core", "packages/shared/world", "packages/shared/nativeabi"},
    "packages/client/lod": {"packages/shared/core", "packages/shared/nativeabi"},
    "packages/client/assets": {"packages/shared/core", "packages/shared/world", "packages/client/mesh", "packages/shared/worldgen"},
    "packages/client/presentation": {"packages/client/mesh", "packages/shared/core"},
    "packages/client/runtime": {
        "packages/client/assets",
        "packages/client/client",
        "packages/client/mesh",
        "packages/client/presentation",
        "packages/shared/core",
        "packages/shared/network",
        "packages/shared/network/protocol",
        "packages/shared/network/tcp",
        "packages/shared/physics",
    },
    "packages/client/cmd/mornlea-godot-core": {
        "packages/client/assets",
        "packages/client/presentation",
        "packages/client/render",
        "packages/client/runtime",
        "packages/shared/core",
        "packages/shared/network",
        "packages/shared/network/protocol",
    },
    "packages/client/render": {"packages/shared/core", "packages/shared/world", "packages/client/mesh", "packages/client/assets"},
    "packages/client/render/hud": {"packages/shared/core", "packages/client/mesh", "packages/client/assets", "packages/client/render"},
    "packages/server/server": {
        "packages/contracts/companion-agent/mcp-v1",
        "packages/shared/companion",
        "packages/shared/core",
        "packages/shared/network",
        "packages/shared/pathfind",
        "packages/shared/physics",
        "packages/shared/world",
        "packages/shared/worldgen",
        "packages/server/sim/contract",
        "packages/server/sim/runtime",
        "packages/server/storage",
        "packages/server/server/persistence",
    },
    "packages/server/server/persistence": {
        "packages/shared/companion",
        "packages/shared/core",
        "packages/shared/physics",
        "packages/server/sim/contract",
        "packages/server/sim/runtime",
        "packages/server/storage",
    },
    "packages/client/client": {
        "packages/shared/companion",
        "packages/shared/core",
        "packages/shared/physics",
        "packages/shared/network",
        "packages/shared/world",
        "packages/client/mesh",
        "packages/client/assets",
        "packages/client/render",
    },
    "packages/tools/agent-board": set(),
    "packages/tools/composite_grass_side": set(),
    "packages/tools/gfxspike": {
        "packages/client/assets",
        "packages/client/client",
        "packages/client/mesh",
        "packages/client/render",
        "packages/shared/core",
        "packages/shared/world",
        "packages/shared/worldgen",
    },
    "packages/tools/perfcheck": {"packages/client/client"},
    "packages/tools/cmd/runtime-oracle": set(),
}

# Permitted dependency edges for authoritative simulation packages.
SIM_ALLOWED_EDGES: dict[str, set[str]] = {
    "packages/server/sim/contract": {"packages/shared/companion", "packages/shared/core", "packages/shared/physics", "packages/shared/world"},
    "packages/server/sim/realm": {"packages/shared/core", "packages/server/fluid", "packages/server/updates", "packages/shared/world", "packages/shared/worldgen"},
    "packages/server/sim/entity": {
        "packages/shared/companion",
        "packages/shared/core",
        "packages/shared/physics",
        "packages/shared/world",
        "packages/server/sim/contract",
        "packages/server/sim/realm",
        "packages/server/updates",
        "packages/shared/tuning",
    },
    "packages/server/sim/runtime": {
        "packages/shared/companion",
        "packages/shared/core",
        "packages/shared/physics",
        "packages/shared/world",
        "packages/server/sim/contract",
        "packages/server/sim/entity",
        "packages/server/sim/realm",
        "packages/shared/tuning",
    },
}

# Mandatory orchestration edges that must be present in simulation runtime assembly.
SIM_REQUIRED_EDGES: dict[str, set[str]] = {
    "packages/server/sim/runtime": {
        "packages/server/sim/contract",
        "packages/server/sim/entity",
        "packages/server/sim/realm",
        "packages/shared/tuning",
    },
}

# Permitted internal dependency edges within the client command subtree.
CLIENT_COMMAND_ALLOWED_EDGES: dict[str, set[str]] = {
    "packages/client/cmd/mornlea": {
        "packages/client/cmd/mornlea/app",
        "packages/client/cmd/mornlea/benchmark",
        "packages/client/cmd/mornlea/capture",
        "packages/client/cmd/mornlea/devcapture",
    },
    "packages/client/cmd/mornlea/app": set(),
    "packages/client/cmd/mornlea/benchmark": {"packages/client/cmd/mornlea/app"},
    "packages/client/cmd/mornlea/capture": {"packages/client/cmd/mornlea/app"},
    "packages/client/cmd/mornlea/devcapture": {"packages/client/cmd/mornlea/app"},
}

# Mandatory assembly edges required in client command wiring.
CLIENT_COMMAND_REQUIRED_EDGES: dict[str, set[str]] = {
    "packages/client/cmd/mornlea": {
        "packages/client/cmd/mornlea/app",
        "packages/client/cmd/mornlea/benchmark",
        "packages/client/cmd/mornlea/capture",
        "packages/client/cmd/mornlea/devcapture",
    },
    "packages/client/cmd/mornlea/benchmark": {"packages/client/cmd/mornlea/app"},
    "packages/client/cmd/mornlea/capture": {"packages/client/cmd/mornlea/app"},
    "packages/client/cmd/mornlea/devcapture": {"packages/client/cmd/mornlea/app"},
}

# Allowed internal package imports in runtime-oracle test files.
ORACLE_ALLOWED_TEST_IMPORTS: set[str] = {
    "packages/shared/network/codec",
    "packages/shared/network/protocol",
    "packages/shared/core",
    "packages/shared/world",
    "packages/shared/companion",
    "packages/shared/pathfind",
    "packages/shared/nativeabi",
    "packages/server/storage/chunk",
    "packages/server/storage/player",
    "packages/server/storage/companion",
    "packages/server/storage/hostile",
    "packages/server/storage/passive",
    "packages/server/storage/region",
    "packages/server/storage/storagedef",
}

# Module list pattern overrides to match architecture governance boundaries.
WORKSPACE_LIST_PATTERNS: dict[str, list[str]] = {
    "packages/client": [
        "./client/...",
        "./render/...",
        "./mesh/...",
        "./lod/...",
        "./audio/...",
        "./assets/...",
        "./runtime/...",
        "./presentation/...",
        "./cmd/mornlea-godot-core",
    ],
    "packages/tools": ["./...", "./gfxspike"],
}


def is_sim_package(local_path: str) -> bool:
    """Return True if local package path falls within simulation subtree."""
    return local_path == "packages/server/sim" or local_path.startswith("packages/server/sim/")


def is_client_command_package(local_path: str) -> bool:
    """Return True if local package path falls within client command subtree."""
    return local_path == "packages/client/cmd/mornlea" or local_path.startswith("packages/client/cmd/mornlea/")


def extract_raw_go_imports(content: str) -> list[str]:
    """Extract all import paths from Go source code without path prefix modification."""
    # Strip block comments
    text = re.sub(r"/\*.*?\*/", "", content, flags=re.DOTALL)
    imports: list[str] = []

    # Match single line import
    for m in re.finditer(r'^\s*import\s+(?:[A-Za-z0-9_.]+\s+)?\"([^\"]+)\"', text, flags=re.MULTILINE):
        imports.append(m.group(1).strip())

    # Match grouped imports
    for m in re.finditer(r"^\s*import\s*\((.*?)\)", text, flags=re.MULTILINE | re.DOTALL):
        block = m.group(1)
        for line in block.splitlines():
            line = line.split("//")[0].strip()
            if not line:
                continue
            str_m = re.search(r'\"([^\"]+)\"', line)
            if str_m:
                imports.append(str_m.group(1).strip())

    return imports


def parse_go_file_imports(content: str) -> list[str]:
    """Extract imported package paths from Go source code.
    
    Returns local package paths with any internal module prefix stripped.
    """
    raw_imports = extract_raw_go_imports(content)
    normalized: list[str] = []
    for imp in raw_imports:
        clean = imp.removeprefix("github.com/channing771/mornlea/")
        normalized.append(clean)
    return sorted(dict.fromkeys(normalized))


def scan_directory_production_imports(subtree: Path, repo_root: Path) -> dict[str, set[str]]:
    """Recursively scan a subtree for directories containing production Go files.
    
    Returns a mapping from repository-relative slash path to the set of internal packages imported.
    """
    edges: dict[str, set[str]] = {}
    if not subtree.is_dir():
        return edges

    for current_dir, _, files in os.walk(subtree):
        dir_path = Path(current_dir)
        prod_go_files = [
            f for f in files
            if f.endswith(".go") and not f.endswith("_test.go")
        ]
        if not prod_go_files:
            continue

        rel_path = dir_path.relative_to(repo_root).as_posix()
        imports: set[str] = set()
        for filename in prod_go_files:
            content = (dir_path / filename).read_text(encoding="utf-8", errors="replace")
            for imp in parse_go_file_imports(content):
                if imp.startswith("packages/"):
                    imports.add(imp)

        edges[rel_path] = imports

    return edges


def scan_sim_import_edges(repo_root: Path) -> dict[str, set[str]]:
    """Scan simulation subtree on disk dynamically and return production internal imports for each subpackage."""
    subtree = repo_root / "packages" / "server" / "sim"
    return scan_directory_production_imports(subtree, repo_root)


def scan_client_command_import_edges(repo_root: Path) -> dict[str, set[str]]:
    """Scan client command subtree on disk dynamically and return production internal imports for each subpackage."""
    subtree = repo_root / "packages" / "client" / "cmd" / "mornlea"
    return scan_directory_production_imports(subtree, repo_root)


def sim_dependency_violations(edges: dict[str, set[str] | list[str]]) -> list[str]:
    """Check simulation package import edges against allowed and required edge tables."""
    violations: list[str] = []
    for pkg, package_imports in edges.items():
        if pkg not in SIM_ALLOWED_EDGES:
            if is_sim_package(pkg):
                violations.append(f"unregistered simulation package: {pkg}")
            continue
        allowed_set = SIM_ALLOWED_EDGES[pkg]
        for dep in sorted(package_imports):
            if dep not in allowed_set:
                violations.append(
                    f"simulation subpackage {pkg} is not allowed to depend on {dep}: "
                    "contract must not depend on realm/entity/runtime, "
                    "realm must not depend on contract/entity/runtime, "
                    "entity must not depend on runtime, "
                    "runtime coordinates the other three and tuning snapshot"
                )

    for pkg, required in SIM_REQUIRED_EDGES.items():
        current_imports = set(edges.get(pkg, set()))
        for dep in sorted(required):
            if dep not in current_imports:
                violations.append(f"simulation subpackage {pkg} missing required edge -> {dep}")

    return sorted(violations)


def client_command_dependency_violations(edges: dict[str, set[str] | list[str]]) -> list[str]:
    """Check client command package import edges against allowed and required edge tables."""
    violations: list[str] = []
    for pkg, package_imports in edges.items():
        if pkg not in CLIENT_COMMAND_ALLOWED_EDGES:
            violations.append(f"unregistered client command package: {pkg}")
            continue
        allowed_set = CLIENT_COMMAND_ALLOWED_EDGES[pkg]
        for dep in sorted(package_imports):
            if not is_client_command_package(dep) or dep in allowed_set:
                continue
            violations.append(
                f"client command package {pkg} is not allowed to depend on {dep}: "
                "direction must be main -> app/benchmark/capture/devcapture, "
                "capture/benchmark/devcapture -> app, and capture and benchmark must not depend on each other"
            )

    for pkg, required in CLIENT_COMMAND_REQUIRED_EDGES.items():
        current_imports = set(edges.get(pkg, set()))
        for dep in sorted(required):
            if dep not in current_imports:
                violations.append(f"client command package {pkg} missing required edge -> {dep}")

    return sorted(violations)


def validate_oracle_imports(filename: str, source_text: str) -> list[str]:
    """Validate imports in runtime-oracle Go files.
    
    Production files cannot import any internal packages.
    Test files may only import packages listed in ORACLE_ALLOWED_TEST_IMPORTS.
    """
    errors: list[str] = []
    is_test = filename.endswith("_test.go")
    raw_imports = extract_raw_go_imports(source_text)

    for raw in raw_imports:
        rel = raw.removeprefix("github.com/channing771/mornlea/")
        is_internal = raw.startswith("github.com/channing771/mornlea/") or raw.startswith("packages/")
        if not is_internal:
            continue

        if not is_test:
            errors.append(f"production oracle file {filename} cannot import internal package {rel}")
        elif rel not in ORACLE_ALLOWED_TEST_IMPORTS:
            errors.append(f"oracle test file {filename} not permitted to import {rel}")

    return sorted(errors)


def find_dependency_cycles(graph: dict[str, set[str]]) -> list[list[str]]:
    """Detect cycles in a directed graph using depth-first search.
    
    Returns a list of detected cycle paths (e.g. [['A', 'B', 'A']]).
    """
    visited: dict[str, int] = {}  # 0: unvisited, 1: visiting, 2: visited
    cycles: list[list[str]] = []

    def dfs(node: str, path: list[str]) -> None:
        visited[node] = 1
        path.append(node)
        for neighbor in sorted(graph.get(node, set())):
            state = visited.get(neighbor, 0)
            if state == 1:
                idx = path.index(neighbor)
                cycles.append(path[idx:] + [neighbor])
            elif state == 0:
                dfs(neighbor, path)
        path.pop()
        visited[node] = 2

    for node in sorted(graph.keys()):
        if visited.get(node, 0) == 0:
            dfs(node, [])

    return cycles


def get_package_imports(repo_root: Path | None = None) -> dict[str, set[str]]:
    """Collect internal package imports across all Go modules in the workspace using go list -json.
    
    Returns a mapping from local package path (e.g. 'packages/server/server') to the set of
    internal packages directly imported by that package.
    """
    if repo_root is None:
        repo_root = get_repo_root()

    modules = get_workspace_modules(repo_root)
    result: dict[str, set[str]] = {}

    for module in modules:
        patterns = WORKSPACE_LIST_PATTERNS.get(module, ["./..."])
        cmd = ["go", "list", "-e", "-json", *patterns]
        res = run_cmd(cmd, cwd=repo_root / module)
        if res.returncode != 0 and not res.stdout.strip():
            raise RuntimeError(f"go list failed in {module}: {res.stderr}")

        decoder = json.JSONDecoder()
        content = res.stdout
        idx = 0
        while idx < len(content):
            stripped = content[idx:].lstrip()
            if not stripped:
                break
            idx = len(content) - len(stripped)
            try:
                pkg_info, end_idx = decoder.raw_decode(content, idx)
            except json.JSONDecodeError as err:
                raise RuntimeError(f"Failed to decode go list JSON in {module}: {err}") from err
            idx = end_idx

            import_path = pkg_info.get("ImportPath", "")
            if not import_path:
                continue

            local_pkg = import_path.removeprefix("github.com/channing771/mornlea/")
            raw_imports = pkg_info.get("Imports", [])
            internal_imports = {
                imp.removeprefix("github.com/channing771/mornlea/")
                for imp in raw_imports
                if imp.startswith("github.com/channing771/mornlea/") or imp.startswith("packages/")
            }
            result[local_pkg] = internal_imports

    return result


def verify_one_way_dependencies(
    repo_root: Path | None = None,
    package_imports: dict[str, set[str]] | None = None,
    check_all_registered: bool = True,
) -> list[str]:
    """Verify all internal package dependencies conform to ALLOWED_DEPENDENCIES and form a DAG.
    
    Returns a sorted list of violations. An empty list indicates full compliance.
    """
    if package_imports is None:
        if repo_root is None:
            repo_root = get_repo_root()
        package_imports = get_package_imports(repo_root)

    violations: list[str] = []

    # Check that each package in package_imports is registered in ALLOWED_DEPENDENCIES
    for pkg in sorted(package_imports.keys()):
        if pkg not in ALLOWED_DEPENDENCIES:
            violations.append(f"unregistered internal package: {pkg}")

    # Check that registered packages exist in workspace when checking completeness
    if check_all_registered:
        workspace_mods: list[str] = []
        if repo_root is not None:
            try:
                workspace_mods = get_workspace_modules(repo_root)
            except Exception:
                pass

        for pkg in sorted(ALLOWED_DEPENDENCIES.keys()):
            if pkg not in package_imports:
                if pkg == "packages/audit" and "packages/audit" not in workspace_mods:
                    continue
                violations.append(f"package {pkg} in allowed dependencies was not found in workspace")

    # Check each direct dependency against allowed map
    for pkg, imports in sorted(package_imports.items()):
        allowed = ALLOWED_DEPENDENCIES.get(pkg, set())
        for dep in sorted(imports):
            if dep not in allowed:
                violations.append(f"{pkg} is not allowed to import {dep}")

    # Check for cycles
    cycles = find_dependency_cycles(package_imports)
    for cycle in cycles:
        violations.append(f"dependency cycle detected: {' -> '.join(cycle)}")

    violations.sort()
    return violations
