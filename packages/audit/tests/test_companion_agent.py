"""Tests for companion agent service boundaries, contracts, CI gates, and Go production isolation."""

from collections import deque
import os
from pathlib import Path
import re
from typing import Any
import pytest
import yaml

from audit.common import get_repo_root

COMPANION_AGENT_WORKFLOW_FIXTURE = """name: fixture
jobs:
  merge-gate:
    if: ${{ always() }}
    needs: [native-linux, integration-server]
    runs-on: ubuntu-24.04
    steps:
      - name: summary
        run: |
          test "${{ needs.integration-server.result }}" = success
  native-linux:
    runs-on: ubuntu-24.04
    steps: []
  unrelated-job:
    runs-on: ubuntu-24.04
    steps: []
  integration-server:
    needs: native-linux
    runs-on: ubuntu-24.04
    steps:
      - uses: actions/setup-python@ece7cb06caefa5fff74198d8649806c4678c61a1
        with:
          python-version: '3.12'
      - uses: astral-sh/setup-uv@37802adc94f370d6bfd71619e3f0bf239e1f3b78
        with:
          version: '0.12.5'
          enable-cache: true
          cache-dependency-glob: |
            packages/agent/pyproject.toml
            packages/agent/uv.lock
      - uses: actions/download-artifact@634f93cb2916e3fdff6788551b99b062d0335ce0
        with:
          name: native-linux-${{ github.sha }}
          path: .
      - name: integration
        run: make ci-integration-server CI_CANDIDATE_SHA="$GITHUB_SHA"
"""

COMPANION_GO_PRODUCTION_ROOTS = [
    "packages/client/cmd/mornlea",
    "packages/client/cmd/mornlea/app",
    "packages/server/cmd/mornlea-server",
    "packages/server/server",
    "packages/shared/companion",
]

COMPANION_GO_FORBIDDEN_IMPORT_ALLOWLIST = {
    "packages/client/cmd/mornlea/benchmark": {
        "os/exec": "benchmark mode runs standalone server probe, not used in companion agent assembly",
    },
    "packages/client/audio": {
        "C": "Darwin audio bridge, not used in companion agent assembly",
    },
    "packages/client/client": {
        "C": "Rust client ABI bridge, not used in companion agent assembly",
    },
    "packages/shared/nativeabi": {
        "C": "Rust engine ABI bridge, not used in companion agent assembly",
    },
}


def _workflow_shell_statements(script: str) -> list[str]:
    """Split shell script into non-empty, non-comment lines."""
    statements: list[str] = []
    for line in script.splitlines():
        line = line.strip()
        if line and not line.startswith("#"):
            statements.append(line)
    return statements


def companion_agent_workflow_violations(source: str | bytes) -> list[str]:
    """Audit companion agent CI contract in workflow YAML."""
    if isinstance(source, bytes):
        source_str = source.decode("utf-8")
    else:
        source_str = source

    try:
        data = yaml.safe_load(source_str)
    except Exception as e:
        return [f"CI YAML invalid: {e}"]

    if not isinstance(data, dict):
        return ["CI YAML must be a mapping"]

    violations: list[str] = []
    jobs = data.get("jobs", {})
    if not isinstance(jobs, dict):
        return ["CI YAML jobs must be a mapping"]

    integration = jobs.get("integration-server")
    if not integration or not isinstance(integration, dict):
        return ["CI missing integration job: integration-server"]

    # Check needs
    needs = integration.get("needs")
    if isinstance(needs, str):
        needs_list = [needs]
    elif isinstance(needs, list):
        needs_list = needs
    else:
        needs_list = []

    if needs_list != ["native-linux"]:
        violations.append(f"integration needs={needs_list}, must depend exactly on native-linux")

    # Check runner
    runs_on = integration.get("runs-on")
    if runs_on != "ubuntu-24.04":
        violations.append(f"integration runs-on={runs_on!r}, must be ubuntu-24.04")

    steps = integration.get("steps", [])
    if not isinstance(steps, list):
        steps = []

    python_indexes: list[int] = []
    uv_indexes: list[int] = []
    download_indexes: list[int] = []
    command_indexes: list[int] = []

    for idx, step in enumerate(steps):
        if not isinstance(step, dict):
            continue
        uses = step.get("uses", "")
        if uses == "actions/setup-python@ece7cb06caefa5fff74198d8649806c4678c61a1":
            python_indexes.append(idx)
            with_block = step.get("with", {})
            if not isinstance(with_block, dict):
                violations.append("setup-python with must be a mapping")
            else:
                py_ver = with_block.get("python-version")
                if not isinstance(py_ver, str):
                    violations.append("Python 3.12 must be specified as YAML string")
                elif py_ver != "3.12":
                    violations.append(f"integration Python 3.12 changed to {py_ver!r}")

        elif uses == "astral-sh/setup-uv@37802adc94f370d6bfd71619e3f0bf239e1f3b78":
            uv_indexes.append(idx)
            with_block = step.get("with", {})
            if not isinstance(with_block, dict):
                violations.append("setup-uv with must be a mapping")
            else:
                uv_ver = with_block.get("version")
                if not isinstance(uv_ver, str) or uv_ver != "0.12.5":
                    violations.append(f"integration uv 0.12.5 changed to {uv_ver!r}")

                enable_cache = with_block.get("enable-cache")
                if not isinstance(enable_cache, bool):
                    violations.append("uv enable-cache must be YAML bool")
                elif not enable_cache:
                    violations.append("uv enable-cache must be true")

                dep_glob = with_block.get("cache-dependency-glob")
                if not isinstance(dep_glob, str):
                    violations.append("uv cache-dependency-glob must be YAML string")
                else:
                    glob_lines = [line.strip() for line in dep_glob.splitlines() if line.strip()]
                    if "packages/agent/pyproject.toml" not in glob_lines:
                        violations.append("uv cache missing packages/agent/pyproject.toml")
                    if "packages/agent/uv.lock" not in glob_lines:
                        violations.append("uv cache missing packages/agent/uv.lock")

        elif uses == "actions/download-artifact@634f93cb2916e3fdff6788551b99b062d0335ce0":
            download_indexes.append(idx)
            with_block = step.get("with", {})
            name_val = with_block.get("name") if isinstance(with_block, dict) else None
            path_val = with_block.get("path") if isinstance(with_block, dict) else None
            if name_val != "native-linux-${{ github.sha }}":
                violations.append("native artifact download must use same-SHA name native-linux-${{ github.sha }}")
            if path_val != ".":
                violations.append("native artifact download path must be .")

        run_cmd = step.get("run")
        if isinstance(run_cmd, str):
            stmts = _workflow_shell_statements(run_cmd)
            if len(stmts) == 1 and stmts[0] == 'make ci-integration-server CI_CANDIDATE_SHA="$GITHUB_SHA"':
                command_indexes.append(idx)

    if len(python_indexes) != 1:
        violations.append(f"integration setup-python step count={len(python_indexes)}, want 1")
    if len(uv_indexes) != 1:
        violations.append(f"integration setup-uv step count={len(uv_indexes)}, want 1")
    if len(download_indexes) != 1:
        violations.append(f"integration same-SHA native artifact download step count={len(download_indexes)}, want 1")
    if len(command_indexes) != 1:
        violations.append(f"integration make ci-integration-server step count={len(command_indexes)}, want 1")

    cmd_idx = command_indexes[0] if len(command_indexes) == 1 else -1
    dl_idx = download_indexes[0] if len(download_indexes) == 1 else -1

    if dl_idx >= 0 and cmd_idx >= 0 and dl_idx >= cmd_idx:
        violations.append("same-SHA artifact download must execute before make ci-integration-server")

    for setup_idx in python_indexes + uv_indexes:
        if cmd_idx >= 0 and setup_idx >= cmd_idx:
            violations.append("Python/uv setup must execute before make ci-integration-server")

    # Check merge-gate
    summary = jobs.get("merge-gate")
    if not summary or not isinstance(summary, dict):
        violations.append("CI missing merge-gate job")
    else:
        if summary.get("if") != "${{ always() }}":
            violations.append("merge-gate job if condition must be ${{ always() }}")

        sum_needs = summary.get("needs")
        if isinstance(sum_needs, str):
            sum_needs_list = [sum_needs]
        elif isinstance(sum_needs, list):
            sum_needs_list = sum_needs
        else:
            sum_needs_list = []

        if "integration-server" not in sum_needs_list:
            violations.append("merge-gate job needs integration-server")

        found_assertion = False
        for step in summary.get("steps", []):
            if isinstance(step, dict) and isinstance(step.get("run"), str):
                if 'test "${{ needs.integration-server.result }}" = success' in _workflow_shell_statements(step["run"]):
                    found_assertion = True
                    break
        if not found_assertion:
            violations.append("merge-gate job missing exact integration result success assertion")

    violations.sort()
    return violations


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


def load_companion_production_import_graph(root: Path) -> dict[str, list[str]]:
    """Build Go production import graph covering all top-level repository packages."""
    graph: dict[str, list[str]] = {}
    module_prefix = "github.com/channing771/mornlea/"
    top_dirs = [
        "packages/contracts",
        "packages/shared",
        "packages/server",
        "packages/client",
        "packages/tools",
        "packages/audit",
    ]

    for top in top_dirs:
        top_path = root / top
        if not top_path.is_dir():
            continue
        for current, dirs, files in os.walk(top_path):
            current_path = Path(current)
            go_files = [f for f in files if f.endswith(".go") and not f.endswith("_test.go")]
            if not go_files:
                continue

            imports_set: set[str] = set()
            for gf in go_files:
                content = (current_path / gf).read_text(encoding="utf-8")
                for imp in _extract_go_imports(content):
                    if imp.startswith(module_prefix):
                        imp = imp[len(module_prefix) :]
                    imports_set.add(imp)

            rel_path = current_path.relative_to(root).as_posix()
            graph[rel_path] = sorted(imports_set)

    return graph


def companion_go_production_boundary_violations(
    graph: dict[str, list[str]], roots: list[str]
) -> list[str]:
    """Traverse production dependency graph and verify no forbidden imports are reachable."""
    module_prefix = "github.com/channing771/mornlea/"
    forbidden = {"C", "os/exec", "runtime/cgo"}
    visited: set[str] = set()
    queue = deque(roots)
    violations: list[str] = []

    while queue:
        pkg = queue.popleft()
        if pkg in visited:
            continue
        visited.add(pkg)

        imports = graph.get(pkg)
        if imports is None:
            violations.append(f"companion production dependency closure missing source package {pkg}")
            continue

        for imported in imports:
            if imported.startswith(module_prefix):
                imported = imported[len(module_prefix) :]

            if imported in forbidden:
                allowed_map = COMPANION_GO_FORBIDDEN_IMPORT_ALLOWLIST.get(pkg, {})
                if imported not in allowed_map:
                    violations.append(f"companion production closure package {pkg} imports forbidden {imported}")
                continue

            if imported in graph:
                queue.append(imported)

    violations.sort()
    return violations


def test_companion_agent_repository_boundary(repo_root: Path) -> None:
    """Verify companion agent distribution boundary files and lack of test fixtures in production."""
    agent = repo_root / "packages" / "agent"
    harness = agent / "harness" / "src" / "harness"
    gateway = agent / "app" / "src" / "app" / "gateway"

    required_files = [
        repo_root / "packages" / "contracts" / "companion-agent" / "http-v1" / "manifest.json",
        repo_root / "packages" / "contracts" / "companion-agent" / "http-v1" / "schema.json",
        repo_root / "packages" / "contracts" / "companion-agent" / "mcp-v1" / "manifest.json",
        repo_root / "packages" / "contracts" / "companion-agent" / "mcp-v1" / "schema.json",
        agent / "pyproject.toml",
        agent / "uv.lock",
        gateway / "app.py",
        harness / "tools" / "mcp_session.py",
        harness / "agents" / "companion" / "planner_factory.py",
        harness / "domain" / "http_v1.py",
        harness / "store" / "sqlite_memory.py",
        harness / "persistence" / "sqlite_schema.py",
        harness / "store" / "memory_ops.py",
        agent / "tests" / "integration" / "process.py",
    ]

    for req in required_files:
        assert req.is_file(), f"companion agent distribution boundary missing file: {req.relative_to(repo_root)}"

    harness_pyproject = (agent / "harness" / "pyproject.toml").read_text(encoding="utf-8")
    for marker in [
        '"../../../packages/contracts/companion-agent/mcp-v1/manifest.json"',
        '"../../../packages/contracts/companion-agent/mcp-v1/schema.json"',
    ]:
        assert marker in harness_pyproject, f"Python wheel missing shared MCP contract: {marker}"

    for pub_dir in [harness, gateway]:
        for root_dir, _, files in os.walk(pub_dir):
            for file in files:
                if file.endswith(".py"):
                    name_lower = file.lower()
                    assert "fake" not in name_lower and "fixture" not in name_lower, (
                        f"test fake/fixture must not enter Python distribution: {Path(root_dir) / file}"
                    )


def test_companion_agent_make_targets(repo_root: Path) -> None:
    """Verify companion-agent Makefile targets, recipes, and command markers."""
    makefile = (repo_root / "Makefile").read_text(encoding="utf-8")

    for target in ["companion-agent-check", "companion-agent-integration"]:
        assert f".PHONY: {target}" in makefile or f" {target} " in makefile, f"Makefile .PHONY missing {target}"
        assert f"make {target}" in makefile, f"make help missing {target}"

    check_recipe = ""
    integration_recipe = ""
    lines = makefile.splitlines()
    for idx, line in enumerate(lines):
        if line == "companion-agent-check:":
            check_recipe = "\n".join(lines[idx + 1 : idx + 10])
        elif line == "companion-agent-integration:":
            integration_recipe = "\n".join(lines[idx + 1 : idx + 10])

    for cmd in [
        "uv sync --locked",
        "uv run ruff format --check .",
        "uv run ruff check .",
        "uv run mypy",
        "uv run pytest -q",
    ]:
        assert cmd in check_recipe, f"companion-agent-check missing command: {cmd}"

    for cmd in ["uv sync --locked", "tests/integration", "-race", "CrossLanguage"]:
        assert cmd in integration_recipe, f"companion-agent-integration missing marker: {cmd}"


def test_companion_agent_ci_gates(repo_root: Path) -> None:
    """Verify required CI workflow conforms to companion agent CI contracts."""
    ci_path = repo_root / ".github" / "workflows" / "ci.yml"
    violations = companion_agent_workflow_violations(ci_path.read_text(encoding="utf-8"))
    assert not violations, f"companion agent CI contract violations:\n" + "\n".join(violations)


def test_companion_go_production_does_not_embed_python(repo_root: Path) -> None:
    """Verify Go production closure does not import forbidden native or exec packages."""
    graph = load_companion_production_import_graph(repo_root)
    violations = companion_go_production_boundary_violations(graph, COMPANION_GO_PRODUCTION_ROOTS)
    assert not violations, f"companion Go production boundary violations:\n" + "\n".join(violations)


def test_companion_agent_ci_gate_mutations() -> None:
    """Verify companion agent CI audit catches individual mutations and reorderings."""
    baseline_violations = companion_agent_workflow_violations(COMPANION_AGENT_WORKFLOW_FIXTURE)
    assert not baseline_violations, f"valid fixture rejected: {baseline_violations}"

    mutations = [
        ("integration dependency", "needs: native-linux\n    runs-on: ubuntu-24.04", "needs: quality\n    runs-on: ubuntu-24.04", "needs"),
        ("linux runner", "runs-on: ubuntu-24.04\n    steps:\n      - uses: actions/setup-python", "runs-on: macos-15\n    steps:\n      - uses: actions/setup-python", "ubuntu-24.04"),
        ("python version", "python-version: '3.12'", "python-version: '3.13'", "Python 3.12"),
        ("python version type", "python-version: '3.12'", "python-version: 3.12", "string"),
        ("uv version", "version: '0.12.5'", "version: 'latest'", "uv 0.12.5"),
        ("uv cache type", "enable-cache: true", "enable-cache: 'true'", "bool"),
        ("pyproject cache dependency", "            packages/agent/pyproject.toml\n", "", "pyproject.toml"),
        ("lock cache dependency", "            packages/agent/uv.lock\n", "", "uv.lock"),
        ("same sha artifact", "name: native-linux-${{ github.sha }}", "name: native-linux-latest", "same-SHA"),
        ("artifact root", "          path: .", "          path: wrong", "path"),
        ("integration command", '        run: make ci-integration-server CI_CANDIDATE_SHA="$GITHUB_SHA"', "        run: make ci-integration-server-disabled", "make ci-integration-server"),
        ("summary always condition missing", "    if: ${{ always() }}\n", "", "always"),
        ("summary always condition changed", "    if: ${{ always() }}", "    if: ${{ success() }}", "always"),
        ("summary dependency", "needs: [native-linux, integration-server]", "needs: [native-linux]", "merge-gate job needs integration"),
        ("summary assertion", 'test "${{ needs.integration-server.result }}" = success', 'test "${{ needs.quality.result }}" = success', "integration result"),
    ]

    for name, old, new, want in mutations:
        assert COMPANION_AGENT_WORKFLOW_FIXTURE.count(old) == 1, f"mutation sentinel {old!r} not unique"
        mutated = COMPANION_AGENT_WORKFLOW_FIXTURE.replace(old, new, 1)
        violations = companion_agent_workflow_violations(mutated)
        assert any(want in v for v in violations), f"mutation {name!r} failed to produce expected violation containing {want!r}: {violations}"

    download_block = """      - uses: actions/download-artifact@634f93cb2916e3fdff6788551b99b062d0335ce0
        with:
          name: native-linux-${{ github.sha }}
          path: .
"""
    command_block = """      - name: integration
        run: make ci-integration-server CI_CANDIDATE_SHA="$GITHUB_SHA"
"""
    assert (download_block + command_block) in COMPANION_AGENT_WORKFLOW_FIXTURE
    mutated_order = COMPANION_AGENT_WORKFLOW_FIXTURE.replace(download_block + command_block, command_block + download_block, 1)
    order_violations = companion_agent_workflow_violations(mutated_order)
    assert any("before" in v for v in order_violations), f"order mutation did not trigger 'before' check: {order_violations}"


def test_companion_agent_quickstart_regex_selects_real_tests(repo_root: Path) -> None:
    """Verify test-quickstart.md regex selects exactly the 3 intended integration tests."""
    quickstart = (repo_root / "docs" / "notes" / "test-quickstart.md").read_text(encoding="utf-8")
    match = re.search(r"`go test \./packages/server/server -run '([^']+)'`", quickstart)
    assert match, "test-quickstart.md missing go test pattern for companion agent"
    pattern = match.group(1)

    server_dir = repo_root / "packages" / "server" / "server"
    test_names: list[str] = []
    for test_file in server_dir.glob("*_test.go"):
        content = test_file.read_text(encoding="utf-8")
        for fn in re.findall(r"^func\s+(Test[A-Za-z0-9_]+)\s*\(", content, re.MULTILINE):
            test_names.append(fn)

    test_names = sorted(set(test_names))
    regex = re.compile(pattern)
    matched = [name for name in test_names if regex.search(name)]

    expected = [
        "TestCompanionAgentHTTPProcessIntegration",
        "TestMCPAgentCrossLanguageCancellationIntegration",
        "TestMCPAgentCrossLanguageIntegration",
    ]
    assert matched == expected, f"quickstart pattern {pattern!r} matched {matched}, want {expected}"


def test_companion_go_production_boundary_mutations() -> None:
    """Verify synthetic mutations to Go dependency graph trigger boundary violations."""
    roots = COMPANION_GO_PRODUCTION_ROOTS
    contract: dict[str, list[str]] = {
        "packages/client/cmd/mornlea": ["packages/client/cmd/mornlea/benchmark", "packages/client/audio", "packages/client/client"],
        "packages/client/cmd/mornlea/app": [],
        "packages/server/cmd/mornlea-server": [],
        "packages/server/server": [],
        "packages/shared/companion": [],
        "packages/client/cmd/mornlea/benchmark": ["os/exec"],
        "packages/tools/agent-board": ["os/exec"],
        "packages/client/audio": ["C"],
        "packages/client/client": ["C"],
        "packages/shared/nativeabi": ["C"],
    }

    base_violations = companion_go_production_boundary_violations(contract, roots)
    assert not base_violations, f"valid contract graph rejected: {base_violations}"

    for forbidden in ["os/exec", "C", "runtime/cgo"]:
        mutated = {k: list(v) for k, v in contract.items()}
        mutated["packages/shared/companion"] = mutated["packages/shared/companion"] + ["packages/shared/companion/pythonhelper"]
        mutated["packages/shared/companion/pythonhelper"] = [forbidden]

        violations = companion_go_production_boundary_violations(mutated, roots)
        assert any("pythonhelper" in v for v in violations) and any(forbidden in v for v in violations), (
            f"mutated graph for {forbidden} failed to trigger violations: {violations}"
        )
