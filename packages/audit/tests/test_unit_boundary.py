"""Tests for Go unit module require boundaries and server/client isolation."""

from pathlib import Path

from audit.boundaries import (
    ALLOWED_SIBLING_REQUIRES,
    parse_go_mod_content,
    parse_go_mod_requires,
    server_client_import_violations,
    verify_module_require_boundaries,
    verify_module_require_violations,
    verify_server_client_separation,
    verify_workspace_modules_match_units,
)
from audit.workspace import get_workspace_modules


def test_package_unit_require_boundaries(repo_root: Path) -> None:
    """Verify all packages/*/go.mod files adhere to allowed sibling require boundaries."""
    violations = verify_module_require_boundaries(repo_root)
    assert not violations, f"Module require boundary violations found:\n" + "\n".join(violations)

    # Server sibling requires must match exactly {client, contracts, shared}
    server_go_mod = repo_root / "packages" / "server" / "go.mod"
    assert server_go_mod.is_file(), "packages/server/go.mod must exist"
    server_requires = parse_go_mod_requires(server_go_mod)
    assert server_requires == {"client", "contracts", "shared"}, (
        f"Server sibling requires must be exactly {{client, contracts, shared}}, got {server_requires}"
    )

    # Client sibling requires must match {server, shared}
    client_go_mod = repo_root / "packages" / "client" / "go.mod"
    assert client_go_mod.is_file(), "packages/client/go.mod must exist"
    client_requires = parse_go_mod_requires(client_go_mod)
    assert client_requires == {"server", "shared"}, (
        f"Client sibling requires must be {{server, shared}}, got {client_requires}"
    )

    # Leaf units (audit, contracts, shared) must not depend on any sibling modules
    for leaf in ["audit", "contracts", "shared"]:
        mod_file = repo_root / "packages" / leaf / "go.mod"
        if mod_file.is_file():
            leaf_requires = parse_go_mod_requires(mod_file)
            assert leaf_requires == set(), f"Leaf unit packages/{leaf} must have no sibling requires, got {leaf_requires}"


def test_workspace_use_set_matches_unit_modules(repo_root: Path) -> None:
    """Verify go.work use entries exactly match the directories under packages/ containing go.mod."""
    violations = verify_workspace_modules_match_units(repo_root)
    assert not violations, "\n".join(violations)

    # Direct parity check
    packages_dir = repo_root / "packages"
    unit_modules = sorted([
        f"packages/{entry.name}"
        for entry in packages_dir.iterdir()
        if entry.is_dir() and (entry / "go.mod").is_file()
    ])
    assert get_workspace_modules(repo_root) == unit_modules


def test_unit_require_violations_detect_drift() -> None:
    """Verify synthetic illegal require edges and unregistered units are caught."""
    def contract_edges() -> dict[str, set[str]]:
        return {
            "server": {"shared", "contracts", "client"},
            "client": {"shared", "server"},
            "tools": {"shared", "server", "client", "contracts"},
            "audit": set(),
            "contracts": set(),
            "shared": set(),
        }

    # Contract edges must produce zero violations
    assert not verify_module_require_violations(contract_edges())

    # Injected forbidden require directions
    forbidden_probes = [
        ("client", "contracts"),
        ("client", "tools"),
        ("server", "tools"),
        ("shared", "server"),
        ("shared", "client"),
        ("contracts", "shared"),
        ("tools", "audit"),
        ("audit", "shared"),
    ]

    for src, dst in forbidden_probes:
        mutated = contract_edges()
        mutated[src].add(dst)
        violations = verify_module_require_violations(mutated)
        assert len(violations) == 1, f"Expected 1 violation for {src} -> {dst}, got {violations}"
        assert f"packages/{src}" in violations[0]
        assert f"packages/{dst}" in violations[0]
        assert "not allowed to require" in violations[0]

    # Unregistered unit
    mutated = contract_edges()
    mutated["widget"] = {"shared"}
    violations = verify_module_require_violations(mutated)
    assert len(violations) == 1
    assert "unregistered unit packages/widget" in violations[0]


def test_go_mod_sibling_requires_skips_indirect() -> None:
    """Verify parser extracts only direct sibling requires and skips indirect annotations or replace blocks."""
    sample_content = """module github.com/channing771/mornlea/packages/client

go 1.26.0

require (
\tgithub.com/channing771/mornlea/packages/contracts v0.0.0-00010101000000-000000000000 // indirect
\tgithub.com/channing771/mornlea/packages/server v0.0.0-00010101000000-000000000000
\tgithub.com/channing771/mornlea/packages/shared v0.0.0-00010101000000-000000000000
)

replace github.com/channing771/mornlea/packages/contracts => ../contracts
"""
    result = parse_go_mod_content(sample_content)
    assert result == {"server", "shared"}

    single_line = (
        "module github.com/channing771/mornlea/packages/server\n"
        "require github.com/channing771/mornlea/packages/client v0.0.0-00010101000000-000000000000\n"
        "replace github.com/channing771/mornlea/packages/client => ../client\n"
    )
    assert parse_go_mod_content(single_line) == {"client"}


def test_server_client_unit_production_boundaries(repo_root: Path) -> None:
    """Verify real server production code, client domain libraries, and shared code obey isolation."""
    violations = verify_server_client_separation(repo_root)
    assert not violations, f"Server/Client separation violations found:\n" + "\n".join(violations)


def test_server_client_boundary_guard_detects_drift() -> None:
    """Verify synthetic injection of forbidden server/client imports is caught by the guard."""
    def contract_sources() -> dict[str, str]:
        return {
            "packages/server/server/server.go": "package server\n",
            "packages/server/server/mirror_integration_test.go": (
                'package server_test\nimport _ "github.com/channing771/mornlea/packages/client/client"\n'
            ),
            "packages/client/client/mesher.go": "package client\n",
            "packages/client/render/render.go": "package render\n",
            "packages/client/cmd/mornlea/app/app_startup.go": (
                'package app\nimport _ "github.com/channing771/mornlea/packages/server/server"\n'
            ),
            "packages/client/cmd/mornlea/benchmark/benchmark.go": (
                'package benchmark\nimport _ "github.com/channing771/mornlea/packages/server/storage"\n'
            ),
            "packages/shared/core/block.go": "package core\n",
        }

    # Contract sources must have zero violations
    assert not server_client_import_violations(contract_sources())

    # Case 1: Server production file importing client is rejected
    sources = contract_sources()
    sources["packages/server/sim/runtime/engine.go"] = (
        'package runtime\nimport _ "github.com/channing771/mornlea/packages/client/client"\n'
    )
    violations = server_client_import_violations(sources)
    assert len(violations) == 1
    assert "server production file packages/server/sim/runtime/engine.go" in violations[0]
    assert "packages/client" in violations[0]

    # Case 2: Server test file importing client is allowed (integration test exception)
    sources = contract_sources()
    sources["packages/server/server/daylight_integration_test.go"] = (
        'package server_test\nimport _ "github.com/channing771/mornlea/packages/client/client"\n'
    )
    assert not server_client_import_violations(sources)

    # Case 3: Client domain library production file importing server is rejected
    sources = contract_sources()
    sources["packages/client/render/section_scheduler.go"] = (
        'package render\nimport _ "github.com/channing771/mornlea/packages/server/sim/runtime"\n'
    )
    violations = server_client_import_violations(sources)
    assert len(violations) == 1
    assert "client domain library file packages/client/render/section_scheduler.go" in violations[0]
    assert "packages/server" in violations[0]

    # Case 4: Client domain library test file importing server is also rejected (no test exemption for client)
    sources = contract_sources()
    sources["packages/client/client/mesher_parity_test.go"] = (
        'package client_test\nimport _ "github.com/channing771/mornlea/packages/server/sim/runtime"\n'
    )
    violations = server_client_import_violations(sources)
    assert len(violations) == 1
    assert "client domain library file packages/client/client/mesher_parity_test.go" in violations[0]
    assert "packages/server" in violations[0]

    # Case 5: Client command entrypoint importing server is allowed (local host assembly)
    sources = contract_sources()
    sources["packages/client/cmd/mornlea/app/app_dependencies.go"] = (
        'package app\nimport _ "github.com/channing771/mornlea/packages/server/server"\n'
    )
    assert not server_client_import_violations(sources)

    # Case 6: Shared production file importing server is rejected
    sources = contract_sources()
    sources["packages/shared/core/world.go"] = (
        'package core\nimport _ "github.com/channing771/mornlea/packages/server/server"\n'
    )
    violations = server_client_import_violations(sources)
    assert len(violations) == 1
    assert "shared file packages/shared/core/world.go" in violations[0]
    assert "packages/server" in violations[0]

    # Case 7: Shared test file importing server is also rejected (shared has no test exemption)
    sources = contract_sources()
    sources["packages/shared/core/world_test.go"] = (
        'package core_test\nimport _ "github.com/channing771/mornlea/packages/server/server"\n'
    )
    violations = server_client_import_violations(sources)
    assert len(violations) == 1
    assert "shared file packages/shared/core/world_test.go" in violations[0]
    assert "packages/server" in violations[0]

    # Case 8: Shared production file importing client is rejected
    sources = contract_sources()
    sources["packages/shared/physics/movement.go"] = (
        'package physics\nimport _ "github.com/channing771/mornlea/packages/client/client"\n'
    )
    violations = server_client_import_violations(sources)
    assert len(violations) == 1
    assert "shared file packages/shared/physics/movement.go" in violations[0]
    assert "packages/client" in violations[0]

    # Case 9: Shared test file importing client is also rejected
    sources = contract_sources()
    sources["packages/shared/physics/movement_test.go"] = (
        'package physics_test\nimport _ "github.com/channing771/mornlea/packages/client/client"\n'
    )
    violations = server_client_import_violations(sources)
    assert len(violations) == 1
    assert "shared file packages/shared/physics/movement_test.go" in violations[0]
    assert "packages/client" in violations[0]

    # Case 10: Import block with inline comment containing closing parenthesis is not truncated
    sources = contract_sources()
    sources["packages/server/sim/runtime/comment_parens.go"] = (
        'package runtime\n'
        'import (\n'
        '\t"math" // inline note (with parens)\n'
        '\t_ "github.com/channing771/mornlea/packages/client/client"\n'
        ')\n'
    )
    violations = server_client_import_violations(sources)
    assert len(violations) == 1
    assert "server production file packages/server/sim/runtime/comment_parens.go" in violations[0]
    assert "packages/client" in violations[0]
