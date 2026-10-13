"""Module boundary and architectural isolation verification for Mornlea units."""

import os
from pathlib import Path
import re

from audit.common import get_repo_root
from audit.workspace import get_workspace_modules

# Allowed direct sibling require edges for each package unit's go.mod.
# Keys and values represent directory names under packages/.
# Legitimate dependency direction is strictly downward:
#   server  -> shared, contracts, client (client edge is for testing only, guarded at source level)
#   client  -> shared, server (server edge is for command entrypoint only, guarded at source level)
#   tools   -> shared, server, client, contracts (e.g. perfcheck benchmarks both server and client)
#   audit, contracts, shared are leaves with no sibling dependencies.
ALLOWED_SIBLING_REQUIRES: dict[str, set[str]] = {
    "server": {"shared", "contracts", "client"},
    "client": {"shared", "server"},
    "tools": {"shared", "server", "client", "contracts"},
    "audit": set(),
    "contracts": set(),
    "shared": set(),
}

MODULE_UNIT_PREFIX = "github.com/channing771/mornlea/packages/"
CLIENT_CMD_SUBTREE_PREFIX = "packages/client/cmd/mornlea"


def parse_go_mod_content(content: str) -> set[str]:
    """Parse go.mod content to extract declared direct sibling package dependencies.

    Indirect dependencies annotated with '// indirect' are excluded because they
    represent transitive bookkeeping rather than intentional unit dependency edges.
    Replace directives are also ignored as they define local path replacements rather
    than dependency graph edges.
    """
    siblings: set[str] = set()
    in_require_block = False

    for raw in content.splitlines():
        line = raw.strip()
        if not line or line.startswith("//"):
            continue

        if in_require_block:
            if line == ")":
                in_require_block = False
                continue
        else:
            if not line.startswith("require"):
                continue
            rest = line[len("require") :].strip()
            if rest == "(":
                in_require_block = True
                continue
            line = rest

        # Check inline comments and strip indirect annotations
        if "//" in line:
            comment_idx = line.index("//")
            comment = line[comment_idx:].strip()
            if comment.endswith("indirect") or "indirect" in comment.split():
                continue
            line = line[:comment_idx].strip()

        fields = line.split()
        if not fields:
            continue

        mod_path = fields[0]
        if mod_path.startswith(MODULE_UNIT_PREFIX):
            name = mod_path[len(MODULE_UNIT_PREFIX) :]
            if name and "/" not in name:
                siblings.add(name)

    return siblings


def parse_go_mod_requires(go_mod_path: Path) -> set[str]:
    """Parse a go.mod file from disk and return direct sibling module names."""
    content = go_mod_path.read_text(encoding="utf-8")
    return parse_go_mod_content(content)


def verify_module_require_violations(requires: dict[str, set[str]]) -> list[str]:
    """Verify given module require mapping against ALLOWED_SIBLING_REQUIRES.

    Accepts an in-memory dictionary mapping unit name to set of required sibling unit names.
    Returns a sorted list of violation descriptions.
    """
    violations: list[str] = []
    for unit, dependencies in sorted(requires.items()):
        if unit not in ALLOWED_SIBLING_REQUIRES:
            violations.append(
                f"unregistered unit packages/{unit}: go.mod found under packages/ but not registered in ALLOWED_SIBLING_REQUIRES"
            )
            continue

        allowed = ALLOWED_SIBLING_REQUIRES[unit]
        for dep in sorted(dependencies):
            if dep not in allowed:
                violations.append(
                    f"unit packages/{unit} is not allowed to require packages/{dep}: "
                    "allowed directions are server->{shared, contracts, client}, "
                    "client->{shared, server}, tools->{shared, server, client, contracts}, "
                    "and audit/contracts/shared must not depend on any sibling units"
                )

    violations.sort()
    return violations


def verify_module_require_boundaries(repo_root: Path | None = None) -> list[str]:
    """Discover all packages/*/go.mod files on disk and verify sibling require boundaries."""
    if repo_root is None:
        repo_root = get_repo_root()

    packages_dir = repo_root / "packages"
    if not packages_dir.is_dir():
        return []

    requires: dict[str, set[str]] = {}
    for entry in sorted(packages_dir.iterdir()):
        if not entry.is_dir():
            continue
        go_mod = entry / "go.mod"
        if not go_mod.is_file():
            continue
        requires[entry.name] = parse_go_mod_requires(go_mod)

    return verify_module_require_violations(requires)


def verify_workspace_modules_match_units(repo_root: Path | None = None) -> list[str]:
    """Verify that go.work use list exactly matches directories in packages/ with go.mod."""
    if repo_root is None:
        repo_root = get_repo_root()

    packages_dir = repo_root / "packages"
    if not packages_dir.is_dir():
        return []

    unit_modules = sorted([
        f"packages/{entry.name}"
        for entry in packages_dir.iterdir()
        if entry.is_dir() and (entry / "go.mod").is_file()
    ])
    workspace_modules = get_workspace_modules(repo_root)

    if workspace_modules != unit_modules:
        return [
            f"go.work use set does not match packages/ go.mod units: "
            f"use={workspace_modules}, unit_modules={unit_modules}"
        ]

    return []


def _extract_go_imports(content: str) -> list[str]:
    """Extract raw import paths from Go source code.

    Parses imports line by line after stripping block comments. Inline comments are
    removed before checking block closure or searching for string literals, preventing
    parentheses in comments from prematurely closing import blocks.
    """
    # Strip block comments
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


def server_client_import_violations(sources: dict[str, str]) -> list[str]:
    """Check repository-relative source files against server/client isolation boundaries.

    Rules enforced:
    1. Server production files (*.go, excluding *_test.go) MUST NOT import packages/client.
       Test files are exempted for mirror-driven integration tests.
    2. Client domain library files (packages/client/* outside cmd/mornlea) MUST NOT import
       packages/server in either production or test code. The only exception is cmd/mornlea.
    3. Shared files (*.go, including *_test.go) MUST NOT import packages/server
       or packages/client unconditionally.
    """
    violations: list[str] = []

    for path in sorted(sources.keys()):
        source = sources[path]
        is_test_file = path.endswith("_test.go")
        imports = _extract_go_imports(source)

        for imp in imports:
            target = ""
            if imp.startswith(MODULE_UNIT_PREFIX):
                target = imp[len(MODULE_UNIT_PREFIX) :]
            elif imp.startswith("packages/"):
                target = imp[len("packages/") :]
            else:
                continue

            is_server_import = target == "server" or target.startswith("server/")
            is_client_import = target == "client" or target.startswith("client/")

            # Rule 1: Server production code cannot import client
            if path.startswith("packages/server/"):
                if not is_test_file and is_client_import:
                    violations.append(
                        f"server production file {path} must not import packages/client package ({imp}): "
                        "client mirrors are only allowed in integration tests (_test.go)"
                    )

            # Rule 2: Client domain libraries cannot import server
            elif path.startswith("packages/client/"):
                in_command_subtree = (
                    path.startswith(CLIENT_CMD_SUBTREE_PREFIX + "/")
                    or path == CLIENT_CMD_SUBTREE_PREFIX
                )
                if not in_command_subtree and is_server_import:
                    violations.append(
                        f"client domain library file {path} must not import packages/server package ({imp}): "
                        f"server assembly is only allowed in {CLIENT_CMD_SUBTREE_PREFIX}"
                    )

            # Rule 3: Shared code cannot import server or client in either production or test files
            elif path.startswith("packages/shared/"):
                if is_server_import:
                    violations.append(
                        f"shared file {path} must not import packages/server package ({imp}): "
                        "shared domain must remain isolated from server in both production and test files"
                    )
                if is_client_import:
                    violations.append(
                        f"shared file {path} must not import packages/client package ({imp}): "
                        "shared domain must remain isolated from client in both production and test files"
                    )

    violations.sort()
    return violations


def verify_server_client_separation(
    repo_root: Path | None = None,
    sources: dict[str, str] | None = None,
) -> list[str]:
    """Scan server, client, and shared directories on disk and verify source isolation."""
    if sources is not None:
        return server_client_import_violations(sources)

    if repo_root is None:
        repo_root = get_repo_root()

    collected_sources: dict[str, str] = {}
    units_to_scan = [
        repo_root / "packages" / "server",
        repo_root / "packages" / "client",
        repo_root / "packages" / "shared",
    ]

    for unit_dir in units_to_scan:
        if not unit_dir.is_dir():
            continue
        for root_dir, dirs, files in os.walk(unit_dir):
            dirs[:] = [d for d in dirs if d != "testdata" and not d.startswith(".")]
            for filename in files:
                if filename.endswith(".go"):
                    full_path = Path(root_dir) / filename
                    rel_path = full_path.relative_to(repo_root).as_posix()
                    collected_sources[rel_path] = full_path.read_text(
                        encoding="utf-8", errors="replace"
                    )

    return server_client_import_violations(collected_sources)
