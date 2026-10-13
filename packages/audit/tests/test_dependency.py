"""Tests for package dependency directions, allowed import graph, and layer boundaries."""

from pathlib import Path

from audit.common import get_repo_root
from audit.dependencies import (
    ALLOWED_DEPENDENCIES,
    CLIENT_COMMAND_ALLOWED_EDGES,
    CLIENT_COMMAND_REQUIRED_EDGES,
    ORACLE_ALLOWED_TEST_IMPORTS,
    SIM_ALLOWED_EDGES,
    SIM_REQUIRED_EDGES,
    client_command_dependency_violations,
    find_dependency_cycles,
    get_package_imports,
    parse_go_file_imports,
    scan_sim_import_edges,
    sim_dependency_violations,
    validate_oracle_imports,
    verify_one_way_dependencies,
)


def test_internal_dependencies_are_one_way(repo_root: Path) -> None:
    """Verify all workspace package dependencies are registered and form an acyclic graph."""
    actual_imports = get_package_imports(repo_root)

    # All actual packages must be registered in ALLOWED_DEPENDENCIES
    violations = verify_one_way_dependencies(repo_root, actual_imports)
    assert not violations, f"Dependency violations found:\n" + "\n".join(violations)

    # Dependency graph must be a DAG (no cycles)
    cycles = find_dependency_cycles(actual_imports)
    assert not cycles, f"Dependency cycles detected: {cycles}"


def test_allowed_dependencies_is_dag() -> None:
    """Verify the static ALLOWED_DEPENDENCIES map is strictly acyclic."""
    cycles = find_dependency_cycles(ALLOWED_DEPENDENCIES)
    assert not cycles, f"ALLOWED_DEPENDENCIES contains cycles: {cycles}"


def test_sim_subpackage_dependency_directions(repo_root: Path) -> None:
    """Verify simulation subpackages obey layer boundaries and orchestration edges."""
    edges = scan_sim_import_edges(repo_root)
    violations = sim_dependency_violations(edges)
    assert not violations, f"Simulation dependency violations found:\n" + "\n".join(violations)


def test_sim_allowed_edges_matches_global_allowed() -> None:
    """Verify SIM_ALLOWED_EDGES matches corresponding entries in ALLOWED_DEPENDENCIES."""
    for pkg, sim_allowed in SIM_ALLOWED_EDGES.items():
        assert pkg in ALLOWED_DEPENDENCIES, f"Package {pkg} missing from ALLOWED_DEPENDENCIES"
        assert sim_allowed == ALLOWED_DEPENDENCIES[pkg], (
            f"Package {pkg} mismatch: SIM_ALLOWED_EDGES has {sim_allowed}, "
            f"ALLOWED_DEPENDENCIES has {ALLOWED_DEPENDENCIES[pkg]}"
        )

    for pkg, deps in ALLOWED_DEPENDENCIES.items():
        if pkg == "packages/server/sim" or pkg.startswith("packages/server/sim/"):
            assert pkg in SIM_ALLOWED_EDGES, f"Sim package {pkg} in ALLOWED_DEPENDENCIES missing from SIM_ALLOWED_EDGES"


def test_sim_dependency_violations_detect_drift() -> None:
    """Verify sim_dependency_violations detects illegal reverse edges, missing required edges, and unregistered packages."""
    def make_contract_edges() -> dict[str, set[str]]:
        return {
            "packages/server/sim/contract": {
                "packages/shared/core",
                "packages/shared/world",
                "packages/shared/companion",
                "packages/shared/physics",
            },
            "packages/server/sim/realm": {
                "packages/shared/core",
                "packages/server/fluid",
                "packages/server/updates",
                "packages/shared/world",
                "packages/shared/worldgen",
            },
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

    # Clean contract edges should have zero violations
    assert not sim_dependency_violations(make_contract_edges())

    # Injected forbidden reverse edges must fail
    forbidden_edges = [
        ("packages/server/sim/contract", "packages/shared/tuning"),
        ("packages/server/sim/contract", "packages/server/sim/realm"),
        ("packages/server/sim/contract", "packages/server/sim/entity"),
        ("packages/server/sim/contract", "packages/server/sim/runtime"),
        ("packages/server/sim/realm", "packages/server/sim/contract"),
        ("packages/server/sim/realm", "packages/shared/tuning"),
        ("packages/server/sim/realm", "packages/server/sim/entity"),
        ("packages/server/sim/realm", "packages/server/sim/runtime"),
        ("packages/server/sim/entity", "packages/server/sim/runtime"),
    ]

    for src, dst in forbidden_edges:
        mutated = make_contract_edges()
        mutated[src].add(dst)
        violations = sim_dependency_violations(mutated)
        assert len(violations) == 1, f"Expected 1 violation for {src} -> {dst}, got {violations}"
        assert f"{src} is not allowed to depend on {dst}" in violations[0]

    # Missing required edges
    mutated = make_contract_edges()
    mutated["packages/server/sim/runtime"] = {
        "packages/server/sim/contract",
        "packages/server/sim/realm",
        "packages/shared/tuning",
    }
    violations = sim_dependency_violations(mutated)
    assert any("missing required edge" in v and "packages/server/sim/entity" in v for v in violations)

    # Unregistered package in sim subtree
    mutated = make_contract_edges()
    mutated["packages/server/sim/extra"] = {"packages/shared/core"}
    violations = sim_dependency_violations(mutated)
    assert any("unregistered simulation package" in v and "packages/server/sim/extra" in v for v in violations)


def test_client_command_dependency_directions(repo_root: Path) -> None:
    """Verify client command subtree packages follow thin assembly and consumer interface rules."""
    client_cmd_dir = repo_root / "packages" / "client" / "cmd" / "mornlea"
    assert client_cmd_dir.is_dir()

    edges: dict[str, set[str]] = {}
    for sub in ["", "app", "benchmark", "capture", "devcapture"]:
        sub_dir = client_cmd_dir / sub if sub else client_cmd_dir
        pkg_key = f"packages/client/cmd/mornlea/{sub}" if sub else "packages/client/cmd/mornlea"
        imports: set[str] = set()
        for go_file in sub_dir.glob("*.go"):
            if go_file.name.endswith("_test.go"):
                continue
            for imp in parse_go_file_imports(go_file.read_text(encoding="utf-8")):
                if imp.startswith("packages/client/cmd/mornlea"):
                    imports.add(imp)
        edges[pkg_key] = imports

    violations = client_command_dependency_violations(edges)
    assert not violations, f"Client command dependency violations:\n" + "\n".join(violations)


def test_client_command_dependency_violations_detect_drift() -> None:
    """Verify client_command_dependency_violations rejects invalid cross-imports and missing assembly edges."""
    def make_contract_edges() -> dict[str, set[str]]:
        return {
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

    assert not client_command_dependency_violations(make_contract_edges())

    # App reverse-depending on capture/benchmark
    for src, dst in [
        ("packages/client/cmd/mornlea/app", "packages/client/cmd/mornlea/capture"),
        ("packages/client/cmd/mornlea/app", "packages/client/cmd/mornlea/benchmark"),
        ("packages/client/cmd/mornlea/capture", "packages/client/cmd/mornlea/benchmark"),
        ("packages/client/cmd/mornlea/benchmark", "packages/client/cmd/mornlea/capture"),
    ]:
        mutated = make_contract_edges()
        mutated[src].add(dst)
        violations = client_command_dependency_violations(mutated)
        assert len(violations) == 1
        assert f"{src} is not allowed to depend on {dst}" in violations[0]

    # Missing required edge from main
    mutated = make_contract_edges()
    mutated["packages/client/cmd/mornlea"] = set()
    violations = client_command_dependency_violations(mutated)
    assert len(violations) == 4

    # Unregistered package
    mutated = make_contract_edges()
    mutated["packages/client/cmd/mornlea/widgets"] = {"packages/client/cmd/mornlea/app"}
    violations = client_command_dependency_violations(mutated)
    assert any("unregistered client command package" in v for v in violations)


def test_server_persistence_does_not_depend_on_server(repo_root: Path) -> None:
    """Verify persistence subpackage does not reverse-depend on parent server package."""
    assert "packages/server/server" not in ALLOWED_DEPENDENCIES["packages/server/server/persistence"]

    persistence_dir = repo_root / "packages" / "server" / "server" / "persistence"
    assert persistence_dir.is_dir()

    for go_file in persistence_dir.glob("*.go"):
        content = go_file.read_text(encoding="utf-8")
        if "persistence_contract" in content:
            continue
        imports = parse_go_file_imports(content)
        assert "packages/server/server" not in imports, (
            f"Production persistence file {go_file.name} must not import packages/server/server"
        )


def test_storage_subpackage_layer_boundaries() -> None:
    """Verify storage package and subpackages follow strictly layered dependencies."""
    storage_root = "packages/server/storage"
    storagedef = "packages/server/storage/storagedef"
    subpackages = [
        "packages/server/storage/chunk",
        "packages/server/storage/companion",
        "packages/server/storage/hostile",
        "packages/server/storage/passive",
        "packages/server/storage/player",
        "packages/server/storage/region",
    ]

    # storagedef is a leaf
    assert ALLOWED_DEPENDENCIES[storagedef] == set()

    # Subpackages must not depend back on storage root
    for sub in subpackages:
        assert storage_root not in ALLOWED_DEPENDENCIES[sub]
        assert storagedef in ALLOWED_DEPENDENCIES[sub]

    # Root storage aggregates all subpackages
    for sub in subpackages:
        assert sub in ALLOWED_DEPENDENCIES[storage_root]
    assert storagedef in ALLOWED_DEPENDENCIES[storage_root]


def test_runtime_oracle_internal_dependencies(repo_root: Path) -> None:
    """Verify runtime-oracle production files use only stdlib and test files only allowed imports."""
    oracle_dir = repo_root / "packages" / "tools" / "cmd" / "runtime-oracle"
    assert oracle_dir.is_dir()

    for go_file in oracle_dir.glob("*.go"):
        src = go_file.read_text(encoding="utf-8")
        errors = validate_oracle_imports(go_file.name, src)
        assert not errors, f"Oracle file {go_file.name} import violations: {errors}"


def test_oracle_dependency_red_matrix() -> None:
    """Verify validate_oracle_imports rejects production internal imports and unauthorized test imports."""
    prod_codec = 'package main\nimport _ "github.com/channing771/mornlea/packages/shared/network/codec"\n'
    errs = validate_oracle_imports("discover.go", prod_codec)
    assert errs, "Production oracle codec import must be rejected"

    test_codec = 'package main\nimport _ "github.com/channing771/mornlea/packages/shared/network/codec"\n'
    errs = validate_oracle_imports("runner_test.go", test_codec)
    assert not errs, f"Test oracle codec import must be accepted, got: {errs}"

    test_server = 'package main\nimport _ "github.com/channing771/mornlea/packages/server/server"\n'
    errs = validate_oracle_imports("runner_test.go", test_server)
    assert errs, "Test oracle server import must be rejected"

    prod_storagedef = 'package main\nimport _ "github.com/channing771/mornlea/packages/server/storage/storagedef"\n'
    errs = validate_oracle_imports("inventory.go", prod_storagedef)
    assert errs, "Production oracle storagedef import must be rejected"

    test_storagedef = 'package main\nimport _ "github.com/channing771/mornlea/packages/server/storage/storagedef"\n'
    errs = validate_oracle_imports("storage_manifest_test.go", test_storagedef)
    assert not errs, f"Test oracle storagedef import must be accepted, got: {errs}"


def test_verify_one_way_dependencies_detects_drift() -> None:
    """Verify synthetic edge violations and unregistered packages are flagged."""
    base = {
        "packages/shared/core": set(),
        "packages/shared/nativeabi": set(),
    }
    # Package missing from ALLOWED_DEPENDENCIES
    mutated = dict(base)
    mutated["packages/unknown/pkg"] = set()
    violations = verify_one_way_dependencies(package_imports=mutated)
    assert any("unregistered internal package" in v and "packages/unknown/pkg" in v for v in violations)

    # Forbidden dependency
    mutated = {
        "packages/shared/core": {"packages/server/server"},
        "packages/shared/nativeabi": set(),
    }
    violations = verify_one_way_dependencies(package_imports=mutated)
    assert any("packages/shared/core is not allowed to import packages/server/server" in v for v in violations)

    # Dependency cycle
    cycle_graph = {
        "packages/shared/core": {"packages/shared/nativeabi"},
        "packages/shared/nativeabi": {"packages/shared/core"},
    }
    cycles = find_dependency_cycles(cycle_graph)
    assert len(cycles) > 0
    violations = verify_one_way_dependencies(package_imports=cycle_graph)
    assert any("dependency cycle detected" in v for v in violations)
