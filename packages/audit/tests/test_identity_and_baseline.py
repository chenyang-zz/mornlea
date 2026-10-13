"""Tests for Mornlea project identity, baseline version matrix, compatibility matrix, and native artifacts."""

import json
import os
from pathlib import Path
import re
import subprocess

import pytest

from audit.common import run_cmd

MODULE_PATH = "github.com/channing771/mornlea"

def _tok(*parts: str) -> str:
    return "".join(parts)


FORBIDDEN_CURRENT_IDENTITY = [
    _tok("module minecraft", "-go"),
    _tok('"minecraft', "-go/internal/"),
    _tok("github.com/channing771/minecraft", "-go"),
    _tok("cmd/mc", "go"),
    _tok("cmd/mc", "god"),
    _tok("bin/mc", "go"),
    _tok("mc", "go_mesh"),
    _tok("libmc", "go_mesh"),
    _tok("mc", "go_engine"),
    _tok("MC", "GO_ENGINE_"),
    _tok("MC", "GO_STATUS_"),
    _tok("MINECRAFT", "_GO_"),
    _tok("MC", "GOD_"),
]

LEGACY_IDENTITY_PATTERN = re.compile(_tok(r"(?i)(?:minecraft[-_]", r"go|mc", r"go)"))

# File path to expected match count in legacy identity allowlist
EXPECTED_LEGACY_IDENTITY_ALLOWANCES: dict[str, int] = {
    "packages/engine/crates/mornlea_server/src/store/recovery.rs": 1,
    "packages/engine/crates/mornlea_server/src/runtime/config/resolve.rs": 1,
    "packages/shared/config/config.go": 1,
    "packages/shared/config/migration_test.go": 18,
    "packages/shared/profile/profile.go": 1,
    "packages/shared/profile/profile_test.go": 18,
    "packages/client/cmd/mornlea/run_test.go": 1,
    "packages/server/cmd/mornlea-server/main_test.go": 1,
    "packages/server/storage/backup.go": 1,
    "packages/server/storage/backup_test.go": 4,
}

CURRENT_IDENTITY_ROOTS = [
    "go.work",
    "packages/contracts",
    "packages/server",
    "packages/shared",
    "packages/client",
    "packages/tools",
    "packages/audit",
    "packages/engine/Cargo.toml",
    "packages/engine/Cargo.lock",
    "packages/engine/crates",
    "packages/engine/include",
    "Makefile",
    ".github/workflows/ci.yml",
    "scripts/agent-hooks",
    ".gitignore",
]

CLAUDE_AGENT_IMPORT = """# CLAUDE.md

本作用域的 Agent 指南位于 [AGENTS.md](AGENTS.md)，供 Claude Code、Codex、OpenCode 及其他编码代理共享。Claude Code 在下方导入该指南。

@AGENTS.md
"""

BASELINE_VERSION_MAPPINGS = [
    {
        "name": "protocol",
        "doc_pattern": r"protocol v(\d+)",
        "source_path": "packages/shared/network/protocol/packet.go",
        "code_pattern": r"const\s+ProtocolVersion\s+uint32\s*=\s*(\w+)",
    },
    {
        "name": "chunk_schema",
        "doc_pattern": r"chunk schema v(\d+)",
        "source_path": "packages/server/storage/chunk/chunk_codec.go",
        "code_pattern": r"currentChunkSchema\s+uint32\s*=\s*(\w+)",
    },
    {
        "name": "player_schema",
        "doc_pattern": r"player schema v(\d+)",
        "source_path": "packages/server/storage/player/player_codec.go",
        "code_pattern": r"CurrentSchema\s+uint32\s*=\s*(\w+)",
    },
    {
        "name": "companion_schema",
        "doc_pattern": r"`companions\.ai` schema v(\d+)",
        "source_path": "packages/server/storage/companion/companion_codec.go",
        "code_pattern": r"CurrentSchema\s+uint32\s*=\s*(\w+)",
    },
    {
        "name": "world_metadata",
        "doc_pattern": r"world metadata v(\d+)",
        "source_path": "packages/server/storage/metadata.go",
        "code_pattern": r"currentMetadataVersion\s+uint32\s*=\s*(\w+)",
    },
    {
        "name": "engine_abi",
        "doc_pattern": r"engine ABI v(\d+)",
        "source_path": "packages/engine/include/mornlea_engine.h",
        "code_pattern": r"#define\s+MORNLEA_ENGINE_ABI_VERSION\s+(\d+)u",
    },
    {
        "name": "client_abi",
        "doc_pattern": r"client ABI v(\d+)",
        "source_path": "packages/engine/include/mornlea_client.h",
        "code_pattern": r"#define\s+MORNLEA_CLIENT_ABI_VERSION\s+(\d+)u",
    },
    {
        "name": "hostile_schema",
        "doc_pattern": r"`hostile_mobs` schema v(\d+)",
        "source_path": "packages/server/storage/hostile/hostile_codec.go",
        "code_pattern": r"CurrentSchema\s+uint32\s*=\s*(\w+)",
    },
    {
        "name": "passive_schema",
        "doc_pattern": r"`passive_mobs` schema v(\d+)",
        "source_path": "packages/server/storage/passive/passive_codec.go",
        "code_pattern": r"CurrentSchema\s+uint32\s*=\s*(\w+)",
    },
    {
        "name": "benchmark_scenario",
        "doc_pattern": r"benchmark scenario v(\d+)",
        "source_path": "packages/client/cmd/mornlea/benchmark/benchmark.go",
        "code_pattern": r"scenarioVersion\s*=\s*(\w+)",
    },
]

COMPATIBILITY_RUST_MIRRORS = {
    "player_schema": ("packages/engine/crates/mornlea_storage/src/player.rs", r"pub const CURRENT_SCHEMA: u32 = (\d+);"),
    "chunk_schema": ("packages/engine/crates/mornlea_storage/src/chunk.rs", r"pub const CURRENT_SCHEMA: u32 = (\d+);"),
    "companion_schema": ("packages/engine/crates/mornlea_storage/src/companion.rs", r"pub const CURRENT_SCHEMA: u32 = (\d+);"),
    "hostile_schema": ("packages/engine/crates/mornlea_storage/src/hostile.rs", r"pub const CURRENT_SCHEMA: u32 = (\d+);"),
    "passive_schema": ("packages/engine/crates/mornlea_storage/src/passive.rs", r"pub const CURRENT_SCHEMA: u32 = (\d+);"),
    "engine_abi": ("packages/engine/crates/mornlea_engine/src/ffi.rs", r"pub\(crate\) const ABI_VERSION: u32 = (\d+);"),
    "client_abi": ("packages/engine/crates/mornlea_client/src/ffi.rs", r"pub const CLIENT_ABI_VERSION: u32 = (\d+);"),
}

HARDCODED_VERSION_CLAIM_PATTERN = re.compile(
    r"(?i)(protocol|协议|ABI|metadata|scenario)\s*(为|is)?\s*v\d+"
    r"|(player|chunk|玩家|区块|companions\.ai`?|hostile_mobs`?|passive_mobs`?)[^\n，。；;]{0,12}schema\s*(为|is)?\s*v\d+"
)


def resolve_baseline_constant(source_text: str, code_pattern: str) -> str:
    """Extract constant value, resolving indirect constant definitions up to 4 hops."""
    match = re.search(code_pattern, source_text)
    if not match:
        raise ValueError(f"Pattern {code_pattern} did not match source text")

    val = match.group(1).strip()
    max_hops = 4
    for _ in range(max_hops):
        if val.isdigit():
            return val
        indirect_pattern = rf"(?m)^\s*(?:const\s+)?{re.escape(val)}\s+(?:uint32\s*=\s*|\s*=\s*)(\w+)"
        next_match = re.search(indirect_pattern, source_text)
        if not next_match:
            # Fallback pattern for indirect definitions
            next_match = re.search(rf"\b{re.escape(val)}\s+uint32\s*=\s*(\w+)", source_text)
            if not next_match:
                raise ValueError(f"Indirect constant {val} not found in source text")
        val = next_match.group(1).strip()

    if not val.isdigit():
        raise ValueError(f"Constant {val} could not be resolved to digits within {max_hops} hops")
    return val


def strip_historical_markdown_sections(text: str) -> str:
    """Filter out sections under headings that discuss historical migrations or context."""
    lines = text.splitlines()
    output: list[str] = []
    skip_level = 0

    for line in lines:
        stripped = line.lstrip()
        count = 0
        while count < len(stripped) and stripped[count] == "#":
            count += 1

        if count > 0 and len(stripped) > count and stripped[count] == " ":
            heading = stripped[count + 1 :].strip()
            if skip_level > 0 and count <= skip_level:
                skip_level = 0
            lower = heading.lower()
            if "historical" in lower or "历史" in heading:
                skip_level = count
                continue

        if skip_level == 0:
            output.append(line)

    return "\n".join(output)


def test_native_engine_library_identity(repo_root: Path) -> None:
    """Verify native engine library naming and absence of legacy identifiers."""
    engine_crate = repo_root / "packages" / "engine" / "crates" / "mornlea_engine"
    assert (engine_crate / "Cargo.toml").is_file(), "mornlea_engine Cargo.toml must exist"

    old_crate = repo_root / "packages" / "engine" / "crates" / "mornlea_mesh"
    assert not old_crate.exists(), "Legacy mornlea_mesh crate must not exist"

    cargo_root = (repo_root / "packages" / "engine" / "Cargo.toml").read_text(encoding="utf-8")
    assert "crates/mornlea_engine" in cargo_root
    assert "crates/mornlea_mesh" not in cargo_root

    engine_cargo = (engine_crate / "Cargo.toml").read_text(encoding="utf-8")
    assert 'name = "mornlea_engine"' in engine_cargo
    assert 'name = "mornlea_mesh"' not in engine_cargo

    build_rs = (engine_crate / "build.rs").read_text(encoding="utf-8")
    assert "@rpath/libmornlea_engine.dylib" in build_rs
    assert "libmornlea_mesh.dylib" not in build_rs

    makefile = (repo_root / "Makefile").read_text(encoding="utf-8")
    assert "libmornlea_engine.dylib" in makefile
    assert "libmornlea_mesh.dylib" not in makefile

    native_go = (repo_root / "packages" / "shared" / "nativeabi" / "native.go").read_text(encoding="utf-8")
    assert "-lmornlea_engine" in native_go
    assert "-lmornlea_mesh" not in native_go

    for rel in ["AGENTS.md", "README.md", "README.zh.md", "openspec/config.yaml", "docs/notes/progress.md"]:
        doc_text = (repo_root / rel).read_text(encoding="utf-8")
        assert "mornlea_engine" in doc_text, f"{rel} must mention mornlea_engine"
        assert "libmornlea_mesh" not in doc_text, f"{rel} must not contain libmornlea_mesh"


def test_mornlea_current_identity(repo_root: Path) -> None:
    """Verify repository root identity, forbidden identity tokens, and legacy identity allowlist."""
    # Root go.mod must not exist: go.work coordinates all modules
    assert not (repo_root / "go.mod").exists(), "Repository root must not retain go.mod"

    # Require linux server bundle identity in Makefile help and CI workflow
    help_res = run_cmd(["make", "help"], cwd=repo_root)
    assert help_res.returncode == 0
    assert "make build-linux-server 构建 Linux amd64 专服与同目录 Rust .so" in help_res.stdout

    ci_workflow = (repo_root / ".github" / "workflows" / "ci.yml").read_text(encoding="utf-8")
    for req in ["native-linux:", "runs-on: ubuntu-24.04", 'make ci-native-linux CI_CANDIDATE_SHA="$GITHUB_SHA"']:
        assert req in ci_workflow, f"CI workflow missing expected Linux native bundle marker: {req}"
    assert "rg " not in ci_workflow, "CI workflow must not depend on runner-installed rg"

    # Total allowance count assertions
    total_expected_matches = sum(EXPECTED_LEGACY_IDENTITY_ALLOWANCES.values())
    assert len(EXPECTED_LEGACY_IDENTITY_ALLOWANCES) == 10  # 10 distinct files
    assert total_expected_matches == 47

    actual_allowance_matches: dict[str, int] = {k: 0 for k in EXPECTED_LEGACY_IDENTITY_ALLOWANCES}

    for root_rel in CURRENT_IDENTITY_ROOTS:
        target = repo_root / root_rel
        if not target.exists():
            continue

        files_to_check: list[Path] = []
        if target.is_file():
            files_to_check.append(target)
        else:
            for root, dirs, files in os.walk(target):
                dirs[:] = [
                    d
                    for d in dirs
                    if d not in {".git", ".venv", ".pytest_cache", "node_modules", "target", "__pycache__"}
                ]
                for file_name in files:
                    ext = Path(file_name).suffix.lower()
                    if ext in {".png", ".dylib", ".so", ".tar", ".gz", ".zip"}:
                        continue
                    files_to_check.append(Path(root) / file_name)

        for file_path in files_to_check:
            rel_str = file_path.relative_to(repo_root).as_posix()

            for forbidden in FORBIDDEN_CURRENT_IDENTITY:
                assert forbidden not in rel_str, f"Path {rel_str} contains forbidden identity token: {forbidden}"

            legacy_path_match = LEGACY_IDENTITY_PATTERN.search(rel_str)
            assert (
                legacy_path_match is None
            ), f"Path {rel_str} contains unapproved legacy identity match: {legacy_path_match.group(0)}"

            try:
                content = file_path.read_text(encoding="utf-8", errors="replace")
            except Exception:
                continue

            for forbidden in FORBIDDEN_CURRENT_IDENTITY:
                assert forbidden not in content, f"File {rel_str} contains forbidden identity token: {forbidden}"

            matches = LEGACY_IDENTITY_PATTERN.findall(content)
            if matches:
                if rel_str not in EXPECTED_LEGACY_IDENTITY_ALLOWANCES:
                    pytest.fail(f"File {rel_str} contains unapproved legacy identity matches: {matches}")
                actual_allowance_matches[rel_str] += len(matches)

    for path, expected_count in EXPECTED_LEGACY_IDENTITY_ALLOWANCES.items():
        actual_count = actual_allowance_matches[path]
        assert (
            actual_count == expected_count
        ), f"Legacy identity allowlist match mismatch for {path}: got {actual_count}, expected {expected_count}"


def test_baseline_versions_match_code(repo_root: Path) -> None:
    """Verify baseline contract versions in AGENTS.md and openspec/config.yaml match code constants."""
    agents_doc = (repo_root / "AGENTS.md").read_text(encoding="utf-8")
    openspec_config = (repo_root / "openspec" / "config.yaml").read_text(encoding="utf-8")

    assert len(BASELINE_VERSION_MAPPINGS) == 10, "Baseline mappings must have exactly 10 entries"

    for mapping in BASELINE_VERSION_MAPPINGS:
        source_path = repo_root / mapping["source_path"]
        assert source_path.is_file(), f"Authoritative source file {source_path} must exist"
        source_text = source_path.read_text(encoding="utf-8")

        code_value = resolve_baseline_constant(source_text, mapping["code_pattern"])

        # Check AGENTS.md
        doc_matches = re.findall(mapping["doc_pattern"], agents_doc)
        assert doc_matches, f"Mapping {mapping['name']} pattern {mapping['doc_pattern']} not found in AGENTS.md"
        for doc_val in doc_matches:
            assert doc_val == code_value, (
                f"AGENTS.md baseline version stale for {mapping['name']}: "
                f"doc has v{doc_val}, code has {code_value} in {mapping['source_path']}"
            )

        # Check openspec/config.yaml
        config_matches = re.findall(mapping["doc_pattern"], openspec_config)
        assert config_matches, f"Mapping {mapping['name']} pattern {mapping['doc_pattern']} not found in openspec/config.yaml"
        for cfg_val in config_matches:
            assert cfg_val == code_value, (
                f"openspec/config.yaml baseline version stale for {mapping['name']}: "
                f"config has v{cfg_val}, code has {code_value} in {mapping['source_path']}"
            )


def test_claude_imports_agent_guidance(repo_root: Path) -> None:
    """Verify CLAUDE.md files are thin imports of AGENTS.md."""
    claude_paths = [
        repo_root / "CLAUDE.md",
        repo_root / "packages" / "engine" / "CLAUDE.md",
        repo_root / "packages" / "client" / "cmd" / "mornlea" / "CLAUDE.md",
    ]
    for p in claude_paths:
        assert p.is_file(), f"{p} must exist"
        content = p.read_text(encoding="utf-8")
        assert content == CLAUDE_AGENT_IMPORT, f"{p} must match standard thin AGENTS.md import exactly"


def test_compatibility_matrix_matches_source(repo_root: Path) -> None:
    """Verify compatibility matrix tables match baseline mappings and Rust mirrors."""
    matrix_docs = [
        ("docs/notes/compatibility.md", "## Current version matrix"),
        ("docs/notes/compatibility.zh.md", "## 当前版本矩阵"),
    ]

    canonical_rows: list[tuple[str, str, str]] = []

    for index, (doc_rel, heading) in enumerate(matrix_docs):
        doc_path = repo_root / doc_rel
        assert doc_path.is_file(), f"{doc_rel} must exist"
        content = doc_path.read_text(encoding="utf-8")
        lines = content.splitlines()

        start = -1
        for i, l in enumerate(lines):
            if l.strip() == heading:
                start = i
                break
        assert start >= 0, f"Section {heading} not found in {doc_rel}"

        rows: list[tuple[str, str, str]] = []
        for line in lines[start + 1 :]:
            trimmed = line.strip()
            if trimmed.startswith("## "):
                break
            if not trimmed.startswith("|"):
                if rows:
                    break
                continue

            cells = [c.strip() for c in trimmed.strip("|").split("|")]
            if len(cells) != 3:
                continue
            if not cells[1].startswith("v") or cells[1] == "v":
                continue  # header or divider
            rows.append((cells[0], cells[1], cells[2]))

        assert len(rows) == 10, f"{doc_rel} must contain exactly 10 matrix rows, found {len(rows)}"

        # Validate each row against baseline source
        for label, version, source_cite in rows:
            clean_label = label.replace("`", "")
            claim = f"{clean_label} {version}"

            matched_mapping = None
            for m in BASELINE_VERSION_MAPPINGS:
                clean_pat = m["doc_pattern"].replace("`", "")
                if re.match(f"^{clean_pat}$", claim):
                    matched_mapping = m
                    break
            assert matched_mapping is not None, f"Matrix row {label} {version} in {doc_rel} matches no baseline contract"

            source_file = repo_root / matched_mapping["source_path"]
            source_text = source_file.read_text(encoding="utf-8")
            code_val = resolve_baseline_constant(source_text, matched_mapping["code_pattern"])

            assert version == f"v{code_val}", (
                f"Matrix row {label} in {doc_rel} has version {version}, "
                f"expected v{code_val} from {matched_mapping['source_path']}"
            )
            assert matched_mapping["source_path"] in source_cite, (
                f"Matrix row {label} in {doc_rel} does not cite authority {matched_mapping['source_path']}"
            )

            # Check Rust mirror if registered
            mapping_name = matched_mapping["name"]
            if mapping_name in COMPATIBILITY_RUST_MIRRORS:
                mirror_rel, mirror_pat = COMPATIBILITY_RUST_MIRRORS[mapping_name]
                mirror_text = (repo_root / mirror_rel).read_text(encoding="utf-8")
                mirror_val = resolve_baseline_constant(mirror_text, mirror_pat)
                assert mirror_val == code_val, (
                    f"Rust mirror {mirror_rel} has {mapping_name} v{mirror_val}, "
                    f"want v{code_val} from {matched_mapping['source_path']}"
                )

        if index == 0:
            canonical_rows = rows
        else:
            assert [r[1] for r in rows] == [r[1] for r in canonical_rows], f"{doc_rel} versions differ from canonical"

    # Bilingual notes outside compatibility matrix must not hardcode current versions
    manifest_file = repo_root / "docs" / "documentation-manifest.json"
    if manifest_file.is_file():
        manifest = json.loads(manifest_file.read_text(encoding="utf-8"))
        for entry in manifest.get("documents", []):
            if entry.get("classification") != "bilingual" or entry.get("id") == "compatibility-guide":
                continue
            for rel in entry.get("paths", []):
                if not rel.startswith("docs/notes/"):
                    continue
                p = repo_root / rel
                if not p.is_file():
                    continue
                text = strip_historical_markdown_sections(p.read_text(encoding="utf-8"))
                for line_idx, line in enumerate(text.splitlines(), start=1):
                    match = HARDCODED_VERSION_CLAIM_PATTERN.search(line)
                    assert match is None, (
                        f"{rel}:{line_idx} hardcodes current version claim '{match.group(0)}': "
                        "link to docs/notes/compatibility.md instead"
                    )


def test_native_artifact_platform_identity(repo_root: Path) -> None:
    """Verify CI platform identification script behavior across target platforms."""
    script = repo_root / "scripts" / "ci" / "platform-id.sh"
    assert script.is_file(), "scripts/ci/platform-id.sh must exist"

    test_cases = [
        ("Linux", "x86_64", "linux-amd64", False),
        ("Darwin", "arm64", "macos-arm64", False),
        ("Darwin", "x86_64", "macos-x86_64", False),
        ("FreeBSD", "amd64", "unsupported CI platform: FreeBSD/amd64", True),
    ]

    for sys_name, machine, expected_out, should_fail in test_cases:
        env = os.environ.copy()
        env["MORNLEA_CI_UNAME_S"] = sys_name
        env["MORNLEA_CI_UNAME_M"] = machine

        proc = subprocess.run(
            ["bash", str(script)],
            capture_output=True,
            text=True,
            env=env,
        )

        if should_fail:
            assert proc.returncode != 0, f"Expected failure for {sys_name}/{machine}"
            assert expected_out in proc.stdout or expected_out in proc.stderr
        else:
            assert proc.returncode == 0, f"Failed for {sys_name}/{machine}: {proc.stderr}"
            assert proc.stdout.strip() == expected_out
