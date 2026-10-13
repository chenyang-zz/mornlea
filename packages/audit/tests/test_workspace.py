"""Tests for workspace discovery and repository root helpers."""

from pathlib import Path
import pytest

from audit.common import get_repo_root, run_cmd
from audit.workspace import get_workspace_modules, parse_go_work_modules


def test_repo_root_resolves_and_contains_go_work() -> None:
    root = get_repo_root()
    assert isinstance(root, Path)
    assert root.is_dir()
    assert (root / "go.work").is_file()
    assert (root / "packages").is_dir()


def test_repo_root_fixture_matches(repo_root: Path) -> None:
    assert repo_root == get_repo_root()
    assert (repo_root / "go.work").is_file()


def test_get_workspace_modules_lists_go_modules() -> None:
    modules = get_workspace_modules()
    assert isinstance(modules, list)
    assert len(modules) >= 5

    # Check modules are sorted and unique
    assert modules == sorted(list(set(modules)))

    # Workspace must contain production Go units
    expected = {
        "packages/client",
        "packages/contracts",
        "packages/server",
        "packages/shared",
        "packages/tools",
    }
    assert expected.issubset(set(modules))

    # Each module path must exist and contain go.mod
    root = get_repo_root()
    for mod in modules:
        mod_dir = root / mod
        assert mod_dir.is_dir(), f"Module directory does not exist: {mod_dir}"
        assert (mod_dir / "go.mod").is_file(), f"Module missing go.mod: {mod_dir}"


def test_workspace_modules_fixture_matches(workspace_modules: list[str]) -> None:
    assert workspace_modules == get_workspace_modules()


def test_run_cmd_executes_successfully() -> None:
    root = get_repo_root()
    result = run_cmd(["echo", "mornlea-audit"], cwd=root)
    assert result.returncode == 0
    assert result.stdout.strip() == "mornlea-audit"


def test_get_repo_root_fails_closed_when_gowork_off(monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setenv("GOWORK", "off")
    with pytest.raises(RuntimeError, match="GOWORK"):
        get_repo_root()


def test_parse_go_work_modules_empty_fails_closed(tmp_path: Path) -> None:
    empty_work = tmp_path / "go.work"
    empty_work.write_text("go 1.26.0\n\nuse ()\n", encoding="utf-8")
    with pytest.raises(RuntimeError, match="use list is empty"):
        parse_go_work_modules(empty_work.read_text(encoding="utf-8"), tmp_path)


def test_parse_go_work_modules_parses_multiline_and_single(tmp_path: Path) -> None:
    work_content = """go 1.26.0

use (
    ./packages/shared
    ./packages/server
)

use ./packages/tools
"""
    modules = parse_go_work_modules(work_content, tmp_path)
    assert modules == ["packages/server", "packages/shared", "packages/tools"]
