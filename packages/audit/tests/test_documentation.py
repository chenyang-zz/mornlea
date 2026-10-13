"""Tests for documentation manifest, documentation pairs, links, semantic claims, and orchestration policies."""

from __future__ import annotations

import json
from pathlib import Path
import re
import subprocess

import pytest

from audit.documentation import (
    DOCUMENTATION_MANIFEST_PATH,
    active_openspec_markdown,
    canonical_delegation_budget_violations,
    changed_repository_paths,
    compare_planning_language_debt,
    current_governed_markdown,
    current_normative_documentation,
    current_openspec_language_debt,
    delegation_policy_violations,
    doc_command_claims,
    doc_local_link_claims,
    doc_repo_path_claims,
    doc_version_claims,
    documentation_link_problems,
    documentation_link_scope,
    legacy_documentation_change_problems,
    missing_orchestration_retrospective_fragments,
    non_english_markdown_prose_lines,
    openspec_apply_routing_violations,
    project_orchestration_retrospective_reference_fragments,
    project_orchestration_retrospective_skill_fragments,
    read_documentation_front_matter,
    read_documentation_manifest,
    resolved_counterpart,
    strip_historical_markdown_sections,
    validate_documentation_manifest_shape,
    validate_documentation_pair,
    validate_documentation_semantic_pair,
    validate_visual_evidence_route,
    verify_doc_manifest,
    verify_doc_pairs,
    verify_markdown_links,
    verify_openspec_language,
    current_documentation_version_patterns,
    BASELINE_VERSION_MAPPINGS,
    resolve_baseline_constant,
    PILOT_GO_CLIENT_CORE_PACKAGE,
    architecture_realtime_growth_violations,
    architecture_gdscript_violations,
    architecture_godot_engine_access_violations,
    architecture_engine_access_from_texts,
    architecture_go_host_boundary_from_imports,
)
from audit.dependencies import get_package_imports
from tests.test_godot import scan_godot_script_discipline


def test_documentation_manifest_classifies_current_markdown(repo_root: Path) -> None:
    """Validate documentation manifest shape against governed markdown files in the repository."""
    problems = verify_doc_manifest(repo_root)
    assert not problems, f"Documentation manifest verification failed:\n" + "\n".join(problems)


def test_documentation_manifest_validation_rejects_duplicate_and_unclassified_entries() -> None:
    """Verify manifest shape validation rejects duplicate IDs and unclassified paths."""
    manifest = {
        "schema_version": 1,
        "documents": [
            {"id": "duplicate", "classification": "machine", "paths": ["AGENTS.md"]},
            {"id": "duplicate", "classification": "machine", "paths": ["AGENTS.md", "README.en.md"]},
        ],
    }
    problems = "\n".join(validate_documentation_manifest_shape(manifest, ["AGENTS.md", "README.md"]))
    for want in ["duplicate document id", "classified by both", "README.en.md", "README.md"]:
        assert want in problems, f"Manifest problems must mention {want!r}:\n{problems}"


def test_documentation_manifest_legacy_entries_cannot_declare_partial_pairs() -> None:
    """Verify legacy entries cannot declare bilingual paths or revision metadata."""
    manifest = {
        "schema_version": 1,
        "documents": [
            {
                "id": "legacy-guide",
                "classification": "legacy",
                "paths": ["docs/legacy.md"],
                "english_path": "docs/legacy.md",
            }
        ],
    }
    problems = "\n".join(validate_documentation_manifest_shape(manifest, ["docs/legacy.md"]))
    assert "must not declare bilingual path or revision metadata" in problems, (
        f"Legacy partial pair was accepted:\n{problems}"
    )


def test_legacy_documentation_remains_unchanged_until_pair_migration(repo_root: Path) -> None:
    """Verify that governed legacy documentation in the manifest has not been modified."""
    manifest = read_documentation_manifest(repo_root)
    changed = changed_repository_paths(repo_root)
    problems = legacy_documentation_change_problems(manifest, changed)
    assert not problems, f"Legacy documentation was modified without pair migration:\n" + "\n".join(problems)


def test_legacy_documentation_change_requires_pair_migration() -> None:
    """Verify that modifying a legacy document requires migrating to a bilingual pair."""
    manifest = {
        "schema_version": 1,
        "documents": [
            {"id": "legacy-guide", "classification": "legacy", "paths": ["docs/legacy.md"]},
        ],
    }
    problems = "\n".join(legacy_documentation_change_problems(manifest, {"docs/legacy.md": True}))
    assert "must migrate to an English *.md and Chinese *.zh.md pair" in problems, (
        f"Changed legacy document was accepted:\n{problems}"
    )


def test_completed_documentation_pairs_are_synchronized(repo_root: Path) -> None:
    """Verify that all completed bilingual documentation pairs have valid synchronized front matter."""
    problems = verify_doc_pairs(repo_root)
    assert not problems, f"Bilingual documentation pair verification failed:\n" + "\n".join(problems)


def test_documentation_pair_validation_rejects_missing_and_mismatched_metadata(tmp_path: Path) -> None:
    """Verify bilingual pair validation detects missing counterparts and mismatched metadata."""
    entry = {
        "id": "sample",
        "classification": "bilingual",
        "paths": ["docs/sample.md", "docs/sample.zh.md"],
        "english_path": "docs/sample.md",
        "chinese_path": "docs/sample.zh.md",
        "revision": "2026-09-15.1",
    }
    en_file = tmp_path / "docs" / "sample.md"
    en_file.parent.mkdir(parents=True, exist_ok=True)
    en_file.write_text(
        "---\n"
        "doc_id: sample\n"
        "doc_revision: 2026-09-15.1\n"
        "language: en\n"
        "counterpart: sample.zh.md\n"
        "---\n"
        "# Sample\n",
        encoding="utf-8",
    )

    problems = validate_documentation_pair(tmp_path, entry)
    assert len(problems) > 0, "Missing Chinese counterpart must fail"

    zh_file = tmp_path / "docs" / "sample.zh.md"
    zh_file.write_text(
        "---\n"
        "doc_id: other\n"
        "doc_revision: 2026-09-15.2\n"
        "language: en\n"
        "counterpart: missing.md\n"
        "---\n"
        "# Sample ZH\n",
        encoding="utf-8",
    )
    problems_str = "\n".join(validate_documentation_pair(tmp_path, entry))
    for want in ["doc_id", "doc_revision", "language", "counterpart"]:
        assert want in problems_str, f"Mismatched pair problems must mention {want}:\n{problems_str}"


def test_public_root_readme_omits_front_matter(tmp_path: Path) -> None:
    """Verify public root README pair is accepted without YAML front matter."""
    entry = {
        "id": "root-readme",
        "classification": "bilingual",
        "paths": ["README.md", "README.zh.md"],
        "english_path": "README.md",
        "chinese_path": "README.zh.md",
        "revision": "2026-09-30.1",
    }
    (tmp_path / "README.md").write_text("# Mornlea\n", encoding="utf-8")
    (tmp_path / "README.zh.md").write_text("# Mornlea\n", encoding="utf-8")
    problems = validate_documentation_pair(tmp_path, entry)
    assert not problems, f"Public README pair without front matter was rejected:\n" + "\n".join(problems)


def test_documentation_pair_validation_accepts_relative_counterparts(tmp_path: Path) -> None:
    """Verify valid relative counterparts in front matter are accepted."""
    entry = {
        "id": "sample",
        "classification": "bilingual",
        "paths": ["docs/sample.md", "docs/sample.zh.md"],
        "english_path": "docs/sample.md",
        "chinese_path": "docs/sample.zh.md",
        "revision": "2026-09-15.1",
    }
    en_file = tmp_path / "docs" / "sample.md"
    en_file.parent.mkdir(parents=True, exist_ok=True)
    en_file.write_text(
        "---\n"
        "doc_id: sample\n"
        "doc_revision: 2026-09-15.1\n"
        "language: en\n"
        "counterpart: sample.zh.md\n"
        "---\n"
        "# Sample\n",
        encoding="utf-8",
    )
    zh_file = tmp_path / "docs" / "sample.zh.md"
    zh_file.write_text(
        "---\n"
        "doc_id: sample\n"
        "doc_revision: 2026-09-15.1\n"
        "language: zh-CN\n"
        "counterpart: sample.md\n"
        "---\n"
        "# Sample ZH\n",
        encoding="utf-8",
    )
    problems = validate_documentation_pair(tmp_path, entry)
    assert not problems, f"Valid pair rejected:\n" + "\n".join(problems)


def test_documentation_pair_validation_rejects_wrong_language_suffixes(tmp_path: Path) -> None:
    """Verify pair validation rejects non-standard language path suffixes."""
    entry = {
        "id": "sample",
        "classification": "bilingual",
        "paths": ["docs/sample.en.md", "docs/sample.md"],
        "english_path": "docs/sample.en.md",
        "chinese_path": "docs/sample.md",
        "revision": "2026-09-15.1",
    }
    en_file = tmp_path / "docs" / "sample.en.md"
    en_file.parent.mkdir(parents=True, exist_ok=True)
    en_file.write_text(
        "---\n"
        "doc_id: sample\n"
        "doc_revision: 2026-09-15.1\n"
        "language: en\n"
        "counterpart: sample.md\n"
        "---\n"
        "# Sample\n",
        encoding="utf-8",
    )
    zh_file = tmp_path / "docs" / "sample.md"
    zh_file.write_text(
        "---\n"
        "doc_id: sample\n"
        "doc_revision: 2026-09-15.1\n"
        "language: zh-CN\n"
        "counterpart: sample.en.md\n"
        "---\n"
        "# Sample Chinese\n",
        encoding="utf-8",
    )
    problems_str = "\n".join(validate_documentation_pair(tmp_path, entry))
    for want in ["unsuffixed *.md", "*.zh.md"]:
        assert want in problems_str, f"Pair problems must mention {want!r}:\n{problems_str}"


def test_documentation_manifest_forbids_english_suffix(repo_root: Path) -> None:
    """Verify that no path in documentation manifest uses the forbidden .en.md suffix."""
    manifest = read_documentation_manifest(repo_root)
    for entry in manifest.get("documents", []):
        for path in entry.get("paths", []):
            assert not path.endswith(".en.md"), f"English-suffixed documentation path {path!r} is forbidden"


def test_documentation_links(repo_root: Path) -> None:
    """Verify all relative links in governed documentation and active changes resolve to existing files."""
    problems = verify_markdown_links(repo_root)
    assert not problems, f"Documentation link verification failed:\n" + "\n".join(problems)


def test_documentation_link_scope_covers_active_changes_but_not_archive(tmp_path: Path) -> None:
    """Verify documentation link scope covers active changes while skipping the frozen top-level archive."""
    manifest_file = tmp_path / DOCUMENTATION_MANIFEST_PATH
    manifest_file.parent.mkdir(parents=True, exist_ok=True)
    manifest_file.write_text(json.dumps({"schema_version": 1, "documents": []}), encoding="utf-8")

    files = {
        "openspec/changes/sample-change/proposal.md": "[gone](missing.md)\n",
        "openspec/changes/sample-change/plans/01-plan.md": "[ok](../proposal.md)\n",
        "openspec/changes/sample-change/archive/notes.md": "[gone](missing.md)\n\n嵌套目录中的规范文字。\n",
        "openspec/changes/archive-foo/proposal.md": "[gone](missing.md)\n\n名称以 archive 开头的变更。\n",
        "openspec/changes/archive/2026-01-01-old/proposal.md": "[gone](missing.md)\n\n已归档的历史文字。\n",
        "openspec/changes/archive/2026-01-01-old/plans/01-old.md": "[gone](missing.md)\n",
    }
    for rel, content in files.items():
        p = tmp_path / rel
        p.parent.mkdir(parents=True, exist_ok=True)
        p.write_text(content, encoding="utf-8")

    scope = documentation_link_scope(tmp_path)
    want = [
        "openspec/changes/archive-foo/proposal.md",
        "openspec/changes/sample-change/archive/notes.md",
        "openspec/changes/sample-change/plans/01-plan.md",
        "openspec/changes/sample-change/proposal.md",
    ]
    assert scope == want, f"Documentation link scope = {scope}, want {want}"
    assert active_openspec_markdown(tmp_path) == want

    problems = []
    for rel in scope:
        problems.extend(documentation_link_problems(tmp_path, rel, files[rel]))

    want_broken = [
        "openspec/changes/archive-foo/proposal.md",
        "openspec/changes/sample-change/archive/notes.md",
        "openspec/changes/sample-change/proposal.md",
    ]
    assert len(problems) == len(want_broken)
    for i, rel in enumerate(want_broken):
        assert problems[i].startswith(rel + " ")

    non_english = [
        rel for rel in active_openspec_markdown(tmp_path)
        if len(non_english_markdown_prose_lines(files[rel])) != 0
    ]
    want_non_english = [
        "openspec/changes/archive-foo/proposal.md",
        "openspec/changes/sample-change/archive/notes.md",
    ]
    assert non_english == want_non_english


def test_documentation_link_validation_rejects_missing_target(tmp_path: Path) -> None:
    """Verify local link validator rejects missing targets and ignores external links."""
    problems = documentation_link_problems(tmp_path, "docs/sample.md", "[missing](missing.md)\n")
    assert len(problems) == 1 and "missing.md" in problems[0], f"Missing local link problems = {problems}"

    ext_problems = documentation_link_problems(tmp_path, "docs/sample.md", "[external](https://example.com)\n")
    assert len(ext_problems) == 0, f"External link must not be treated as a repository path: {ext_problems}"


def test_documentation_semantic_claims(repo_root: Path) -> None:
    """Verify commands, repo paths, local links, and version claims match across bilingual pairs."""
    manifest = read_documentation_manifest(repo_root)
    all_problems = []
    for entry in manifest.get("documents", []):
        if entry.get("classification") != "bilingual":
            continue
        en_path = entry.get("english_path", "")
        zh_path = entry.get("chinese_path", "")
        paths = entry.get("paths", [])
        if en_path in paths and zh_path in paths:
            en_text = (repo_root / en_path).read_text(encoding="utf-8")
            zh_text = (repo_root / zh_path).read_text(encoding="utf-8")
            all_problems.extend(validate_documentation_semantic_pair(
                entry["id"], en_path, zh_path, en_text, zh_text
            ))
    assert not all_problems, "Semantic pair claim differences detected:\n" + "\n".join(all_problems)


def test_documentation_semantic_pair_rejects_command_path_and_version_drift() -> None:
    """Verify semantic claims checker rejects differences in commands, paths, links, and versions."""
    english = "Run `make dev-check`, then inspect `packages/client/runtime` and [architecture](architecture.md). Current protocol v44.\n"
    chinese = "Run `make test-race`, then inspect `packages/server/runtime` and [architecture](other.zh.md). Current protocol v43.\n"
    problems_str = "\n".join(validate_documentation_semantic_pair("sample", "docs/sample.md", "docs/sample.zh.md", english, chinese))
    for want in ["command claims differ", "repository path claims differ", "local link claims differ", "version claims differ"]:
        assert want in problems_str, f"Semantic pair problems must mention {want!r}:\n{problems_str}"


def test_current_documentation_versions(repo_root: Path) -> None:
    """Verify current normative documentation references authoritative contract constants."""
    for rel in current_normative_documentation(repo_root):
        text = (repo_root / rel).read_text(encoding="utf-8")
        current_text = strip_historical_markdown_sections(text)
        for mapping in BASELINE_VERSION_MAPPINGS:
            source = (repo_root / mapping["source_path"]).read_text(encoding="utf-8")
            want = resolve_baseline_constant(source, mapping["code_pattern"])
            for pat in current_documentation_version_patterns(mapping):
                for match in re.finditer(pat, current_text):
                    got = match.group(1)
                    assert got == want, (
                        f"{rel} has stale {mapping['name']} v{got}, want v{want} from {mapping['source_path']}"
                    )


def test_current_documentation_version_guard_ignores_explicit_history() -> None:
    """Verify that version checks ignore headings designated as historical."""
    text = "## Current\nengine ABI v11\n\n## Historical note\nengine ABI v10\n\n## Next\nclient ABI v19\n"
    stripped = strip_historical_markdown_sections(text)
    assert "engine ABI v10" not in stripped, f"Historical section was not stripped:\n{stripped}"
    for want in ["engine ABI v11", "client ABI v19"]:
        assert want in stripped, f"Current claim {want!r} was incorrectly stripped"


def test_current_documentation_version_pattern_captures_one_value() -> None:
    """Verify every baseline version pattern compiles and captures exactly one group."""
    for mapping in BASELINE_VERSION_MAPPINGS:
        for pat in current_documentation_version_patterns(mapping):
            compiled = re.compile(pat)
            assert compiled.groups == 1, f"{mapping['name']} pattern {pat!r} has {compiled.groups} capture groups, want 1"


def test_architecture_target_documents_foundation_and_rollback(repo_root: Path) -> None:
    """Verify target architecture documents foundation milestones, rollback principles, and cross-links."""
    en_text = (repo_root / "docs" / "architecture-target.md").read_text(encoding="utf-8")
    zh_text = (repo_root / "docs" / "architecture-target.zh.md").read_text(encoding="utf-8")

    en_fragments = [
        "status: target-not-current",
        "F1", "F2", "F3",
        "P8", "P9", "P10", "P11", "P12", "P13", "P14",
        "`apps/mornlea-godot/`",
        "`scripts/godot`",
        "No-Go rollback",
        "offline replay",
        "independently reversible",
        "never runs two online authorities",
    ]
    for fragment in en_fragments:
        assert fragment in en_text, f"architecture-target.md is missing {fragment!r}"

    zh_fragments = [
        "status: target-not-current",
        "F1", "F2", "F3",
        "P8", "P14",
        "`apps/mornlea-godot/`",
        "`scripts/godot`",
        "No-Go rollback",
        "offline replay",
        "绝不运行两个在线权威",
    ]
    for fragment in zh_fragments:
        assert fragment in zh_text, f"architecture-target.zh.md is missing {fragment!r}"

    assert "current default production" not in en_text, (
        "architecture-target.md must not present the pilot as current default production behavior"
    )

    cross_links = {
        "AGENTS.md": "docs/architecture-target.md",
        "docs/architecture.md": "architecture-target.md",
        "docs/README.md": "architecture-target.md",
        "docs/notes/go-rust-division.md": "../architecture-target.md",
        ".codex/skills/mornlea-architecture/SKILL.md": "../../../docs/architecture-target.md",
        ".claude/skills/mornlea-architecture/SKILL.md": "../../../docs/architecture-target.md",
    }
    for rel, needle in cross_links.items():
        text = (repo_root / rel).read_text(encoding="utf-8")
        assert needle in text, f"{rel} does not cross-link the target architecture document via {needle!r}"


def test_project_architecture_skills_match(repo_root: Path) -> None:
    """Verify Codex and Claude architecture skills are identical and carry required conventions."""
    codex_skill = repo_root / ".codex" / "skills" / "mornlea-architecture" / "SKILL.md"
    claude_skill = repo_root / ".claude" / "skills" / "mornlea-architecture" / "SKILL.md"
    assert codex_skill.read_bytes() == claude_skill.read_bytes(), (
        "project-owned Codex and Claude architecture skills must be byte-identical"
    )

    text = codex_skill.read_text(encoding="utf-8")
    required = [
        "code and tests, canonical `openspec/specs/`",
        "`apps/mornlea-godot/` is the stable Godot project root",
        "`platform/desktop/` is the only platform-adapter family",
        "Route visual evidence by observable semantics",
        "English `*.md` plus synchronized Chinese `*.zh.md`",
        "current code, tests, or canonical specifications verify it",
        "applies across future tasks",
        "not already stated more authoritatively elsewhere",
        "contains no volatile count or task history",
        "Architecture skill: no change",
    ]
    for fragment in required:
        assert fragment in text, f"architecture skill is missing required convention or promotion criterion {fragment!r}"


def test_directory_scoped_guidance_policy(repo_root: Path) -> None:
    """Verify AGENTS.md, style guide, and architecture skills declare directory-scoped guidance rules."""
    paths = [
        "AGENTS.md",
        "docs/agents-md-style.md",
        ".codex/skills/mornlea-architecture/SKILL.md",
        ".claude/skills/mornlea-architecture/SKILL.md",
    ]
    required = [
        "important directory",
        "`AGENTS.md`",
        "independent ownership, dependency, lifecycle, or validation boundary",
        "purpose",
        "directory map",
        "entry points",
        "focused validation",
        "create or update",
        "inherit the parent",
    ]
    for rel in paths:
        text = (repo_root / rel).read_text(encoding="utf-8").lower()
        for fragment in required:
            assert fragment.lower() in text, f"{rel} does not state directory-scoped guidance rule {fragment!r}"


def test_orchestration_carries_directory_guidance_policy(repo_root: Path) -> None:
    """Verify orchestration skills and config carry directory guidance rules."""
    paths = [
        "openspec/config.yaml",
        ".codex/skills/mornlea-implementation-orchestration/SKILL.md",
        ".claude/skills/mornlea-implementation-orchestration/SKILL.md",
    ]
    required = [
        "ancestor `AGENTS.md` chain",
        "important directory",
        "same change",
        "inheritance rationale",
    ]
    for rel in paths:
        text = (repo_root / rel).read_text(encoding="utf-8")
        for fragment in required:
            assert fragment in text, f"{rel} does not carry directory-guidance orchestration rule {fragment!r}"


def test_project_orchestration_skills_match(repo_root: Path) -> None:
    """Verify Codex and Claude implementation orchestration skills match and preserve isolation policy."""
    codex_skill = repo_root / ".codex" / "skills" / "mornlea-implementation-orchestration" / "SKILL.md"
    claude_skill = repo_root / ".claude" / "skills" / "mornlea-implementation-orchestration" / "SKILL.md"
    assert codex_skill.read_bytes() == claude_skill.read_bytes(), (
        "project-owned Codex and Claude orchestration skills must be byte-identical"
    )

    for rel in [
        "AGENTS.md",
        "openspec/config.yaml",
        ".codex/skills/mornlea-implementation-orchestration/SKILL.md",
        ".claude/skills/mornlea-implementation-orchestration/SKILL.md",
    ]:
        source = (repo_root / rel).read_text(encoding="utf-8")
        for fragment in ["isolation-first", "main-context retention", "concise task brief"]:
            assert fragment in source, f"{rel} does not state context-isolation rule {fragment!r}"


def test_project_orchestration_retrospective_policy(repo_root: Path) -> None:
    """Verify skills and references preserve retrospective task status and consumer tracking policies."""
    targets = [
        (".codex/skills/mornlea-implementation-orchestration/SKILL.md", project_orchestration_retrospective_skill_fragments()),
        (".claude/skills/mornlea-implementation-orchestration/SKILL.md", project_orchestration_retrospective_skill_fragments()),
        (".codex/skills/mornlea-implementation-orchestration/references/worker-planning.md", project_orchestration_retrospective_reference_fragments()),
        (".claude/skills/mornlea-implementation-orchestration/references/worker-planning.md", project_orchestration_retrospective_reference_fragments()),
    ]
    for rel, fragments in targets:
        source = (repo_root / rel).read_text(encoding="utf-8")
        violations = missing_orchestration_retrospective_fragments(source, fragments)
        assert not violations, f"{rel} does not preserve retrospective policy:\n" + "\n".join(violations)


def test_project_orchestration_retrospective_policy_guard_detects_drift() -> None:
    """Verify that missing retrospective fragments are caught by the checker."""
    for name, fragments in [
        ("skill", project_orchestration_retrospective_skill_fragments()),
        ("reference", project_orchestration_retrospective_reference_fragments()),
    ]:
        valid = "\n".join(fragments)
        assert not missing_orchestration_retrospective_fragments(valid, fragments)
        for fragment in fragments:
            mutated = valid.replace(fragment, "", 1)
            violations = missing_orchestration_retrospective_fragments(mutated, fragments)
            assert len(violations) == 1, f"Expected 1 violation when removing {fragment!r}, got {violations}"


def test_project_openspec_apply_skills_route_orchestration(repo_root: Path) -> None:
    """Verify OpenSpec apply skills route orchestration commands and preserve CLI state contract."""
    for rel in [
        ".codex/skills/openspec-apply-change/SKILL.md",
        ".claude/skills/openspec-apply-change/SKILL.md",
    ]:
        source = (repo_root / rel).read_text(encoding="utf-8")
        violations = openspec_apply_routing_violations(source)
        assert not violations, f"{rel} does not preserve the apply workflow route:\n" + "\n".join(violations)


def test_openspec_apply_routing_guard_detects_drift() -> None:
    """Verify drift in OpenSpec apply skill fragments is caught."""
    valid_fragments = [
        "../mornlea-implementation-orchestration/SKILL.md",
        "openspec status --change",
        "openspec instructions apply --change",
        'state: "blocked"',
        'state: "all_done"',
        "contextFiles",
        "Preserve CLI-controlled `blocked`, `ready`, and `all_done` states",
    ]
    valid = "\n".join(valid_fragments)
    assert not openspec_apply_routing_violations(valid)
    for fragment in valid_fragments:
        mutated = valid.replace(fragment, "", 1)
        violations = openspec_apply_routing_violations(mutated)
        assert len(violations) == 1, f"Expected 1 violation when removing {fragment!r}, got {violations}"


def test_provider_aware_orchestration(repo_root: Path) -> None:
    """Verify provider-aware orchestration policies in AGENTS.md and openspec/config.yaml."""
    for rel in ["AGENTS.md", "openspec/config.yaml"]:
        text = (repo_root / rel).read_text(encoding="utf-8")
        for required in [
            "verified ChatGPT or Codex controller using an OpenAI model",
            "standing project authorization",
            "direct, delegated, or mixed execution",
            "without a separate per-task user request",
            "non-OpenAI or unknown-provider controller",
            "strict `subagent-driven-development`",
            "explicit user prohibition",
            "higher-priority runtime restriction",
            "three subagents",
        ]:
            assert required in text, f"{rel} does not state required provider-aware policy {required!r}"


def test_agent_guidance(repo_root: Path) -> None:
    """Verify subtree AGENTS.md files defer to root provider-aware orchestration policy."""
    for rel in ["docs/AGENTS.md", "packages/engine/AGENTS.md"]:
        text = (repo_root / rel).read_text(encoding="utf-8")
        assert "The root provider-aware orchestration policy applies" in text, (
            f"{rel} must defer to root provider-aware orchestration policy"
        )


def test_code_comment_language_policy(repo_root: Path) -> None:
    """Verify policy requiring English comments across documentation."""
    for rel in ["AGENTS.md", "docs/AGENTS.md", "packages/engine/AGENTS.md", "docs/test-organization.md"]:
        text = (repo_root / rel).read_text(encoding="utf-8")
        assert "comments" in text and "English" in text, f"{rel} must require English comments"
        for forbidden in ["comments use Chinese", "Chinese comments", "注释使用中文", "中文注释", "中文 `///`", "中文 doc 注释"]:
            assert forbidden not in text, f"{rel} retains obsolete Chinese comment requirement {forbidden!r}"


def test_delegation_budget_and_context_isolation(repo_root: Path) -> None:
    """Verify delegation budget and context isolation constraints in governance files."""
    for rel in [
        "AGENTS.md",
        ".codex/skills/mornlea-implementation-orchestration/SKILL.md",
        ".claude/skills/mornlea-implementation-orchestration/SKILL.md",
    ]:
        text = (repo_root / rel).read_text(encoding="utf-8")
        violations = delegation_policy_violations(text)
        assert not violations, f"{rel} does not enforce delegation policy:\n" + "\n".join(violations)


def test_delegation_budget_guard_detects_drift() -> None:
    """Verify delegation budget scanner detects missing fragments."""
    valid = (
        "isolation-first\n"
        "At most three subagents may run concurrently\n"
        "main-context retention\n"
        "Do not delegate merely for parallel speed\n"
        "concise task brief\n"
    )
    assert not delegation_policy_violations(valid)
    mutated = valid.replace("At most three subagents may run concurrently\n", "")
    violations = delegation_policy_violations(mutated)
    assert len(violations) == 1


def test_canonical_governance_spec_delegation_budget(repo_root: Path) -> None:
    """Verify canonical governance specification enforces the three-subagent ceiling."""
    spec_path = repo_root / "openspec" / "specs" / "development-governance" / "spec.md"
    text = spec_path.read_text(encoding="utf-8")
    violations = canonical_delegation_budget_violations(text)
    assert not violations, f"{spec_path} drifted from subagent ceiling:\n" + "\n".join(violations)


def test_canonical_governance_spec_delegation_budget_guard_detects_drift() -> None:
    """Verify canonical delegation budget checker detects stale fragments."""
    valid = "MUST run no more than three subagents concurrently\n- **AND** MUST NOT start a fourth concurrent subagent\n"
    assert not canonical_delegation_budget_violations(valid)
    stale = "MUST run no more than two subagents concurrently\n- **AND** MUST NOT start a third concurrent subagent\n"
    violations = canonical_delegation_budget_violations(stale)
    assert len(violations) == 4


def test_openspec_language(repo_root: Path) -> None:
    """Verify all active OpenSpec change artifacts and debt inventory contain only English prose."""
    problems = verify_openspec_language(repo_root)
    assert not problems, f"OpenSpec language verification failed:\n" + "\n".join(problems)


def test_openspec_language_classifier_excludes_only_code_and_localized_literals() -> None:
    """Verify language classifier excludes code blocks, inline code, and localized literals."""
    text = "English prose with `中文标识符`.\n\n```text\n中文 fixture\n```\n\nlocalized(\"中文界面文字\")\n"
    assert len(non_english_markdown_prose_lines(text)) == 0
    text += "Normative 中文 prose must fail.\n"
    lines = non_english_markdown_prose_lines(text)
    assert len(lines) == 1 and "Normative" in lines[0]


def test_openspec_language_debt_rejects_growth_and_replacement() -> None:
    """Verify language debt comparison rejects new debt files or modified lines without net decrease."""
    baseline = {
        "schema_version": 1,
        "total_files": 1,
        "total_lines": 2,
        "files": [{"path": "openspec/specs/example/spec.md", "count": 2, "digest": "old"}],
    }
    current_new = {
        "schema_version": 1,
        "total_files": 2,
        "total_lines": 3,
        "files": [
            {"path": "openspec/specs/example/spec.md", "count": 2, "digest": "old"},
            {"path": "openspec/specs/new/spec.md", "count": 1, "digest": "new"},
        ],
    }
    problems_new = "\n".join(compare_planning_language_debt(baseline, current_new, allow_decrease=True))
    assert "new non-English planning debt" in problems_new

    current_replace = {
        "schema_version": 1,
        "total_files": 1,
        "total_lines": 2,
        "files": [{"path": "openspec/specs/example/spec.md", "count": 2, "digest": "different"}],
    }
    problems_replace = "\n".join(compare_planning_language_debt(baseline, current_replace, allow_decrease=True))
    assert "changed without decreasing" in problems_replace

    current_decrease = {
        "schema_version": 1,
        "total_files": 1,
        "total_lines": 1,
        "files": [{"path": "openspec/specs/example/spec.md", "count": 1, "digest": "smaller"}],
    }
    problems_no_allow = "\n".join(compare_planning_language_debt(baseline, current_decrease, allow_decrease=False))
    assert "decreased from 2 to 1" in problems_no_allow

    assert not compare_planning_language_debt(baseline, current_decrease, allow_decrease=True)


def test_openspec_language_grandfather_allows_unchanged_debt() -> None:
    """Verify unchanged debt baseline is accepted."""
    baseline = {
        "schema_version": 1,
        "total_files": 1,
        "total_lines": 2,
        "files": [{"path": "openspec/specs/example/spec.md", "count": 2, "digest": "unchanged-prose"}],
    }
    assert not compare_planning_language_debt(baseline, baseline, allow_decrease=False)
    assert not compare_planning_language_debt(baseline, baseline, allow_decrease=True)


def test_historical_planning_exemption(tmp_path: Path) -> None:
    """Verify archived change folders are exempt from the active English language scan."""
    active = tmp_path / "openspec" / "changes" / "active" / "proposal.md"
    active.parent.mkdir(parents=True, exist_ok=True)
    active.write_text("English active prose.\n", encoding="utf-8")

    archive = tmp_path / "openspec" / "changes" / "archive" / "old" / "proposal.md"
    archive.parent.mkdir(parents=True, exist_ok=True)
    archive.write_text("历史中文证据。\n", encoding="utf-8")

    paths = active_openspec_markdown(tmp_path)
    assert paths == ["openspec/changes/active/proposal.md"]


def test_visual_baseline_routing(repo_root: Path) -> None:
    """Verify renderer-specific visual classes are absent and visual routing rules are enforced in skills."""
    forbidden = repo_root / "testdata" / "visual-golden" / "godot"
    assert not forbidden.exists(), f"Renderer-specific visual class must not exist: {forbidden}"

    for rel in [
        ".codex/skills/visual-baseline/SKILL.md",
        ".claude/skills/visual-baseline/SKILL.md",
    ]:
        text = (repo_root / rel).read_text(encoding="utf-8")
        for required in [
            "Route by the observable subject, never by renderer identity",
            "build/visual/godot-pilot/<run-id>/",
            "must not write tracked goldens or relax thresholds",
            "requires an approved feature change",
        ]:
            assert required in text, f"{rel} is missing visual routing rule {required!r}"


def test_visual_producer_ownership() -> None:
    """Verify visual evidence routing rules across test cases."""
    cases = [
        {"path": "build/visual/godot-pilot/run-1/frame.png", "pilot": True, "want_error": False},
        {"path": "testdata/visual-golden/world/frame.png", "pilot": True, "want_error": True},
        {"path": "testdata/visual-golden/godot/frame.png", "want_error": True},
        {"path": "testdata/visual-golden/ui/panel.png", "update": True, "want_error": True},
        {"path": "testdata/visual-golden/ui/panel.png", "update": True, "has_handoff": True, "want_error": False},
    ]
    for case in cases:
        err = validate_visual_evidence_route(
            case["path"],
            pilot=case.get("pilot", False),
            update=case.get("update", False),
            has_handoff=case.get("has_handoff", False),
        )
        assert (err is not None) == case["want_error"], f"Route test failed for {case}: {err}"


def test_architecture_pilot_profile(repo_root: Path) -> None:
    """Verify pilot architecture profile registers required transition exceptions and allowlists."""
    packages = get_package_imports(repo_root)
    assert PILOT_GO_CLIENT_CORE_PACKAGE in packages, (
        f"pilot profile must register the Go client-core transition package {PILOT_GO_CLIENT_CORE_PACKAGE}"
    )
    assert "packages/client/runtime" in packages, "pilot profile must register packages/client/runtime"
    assert "packages/client/presentation" in packages, "pilot profile must register packages/client/presentation"

    violations = architecture_realtime_growth_violations(packages, "pilot")
    assert not violations, f"pilot real-time inventory drifted:\n" + "\n".join(violations)

    gd_violations = architecture_gdscript_violations(repo_root, "pilot")
    assert not gd_violations, f"pilot Bootstrap allowlist drifted:\n" + "\n".join(gd_violations)


def test_architecture_target_profile(repo_root: Path) -> None:
    """Verify target architecture profile rejects new Go real-time ownership and foreign host bindings."""
    packages = get_package_imports(repo_root)
    violations = architecture_realtime_growth_violations(packages, "target")
    assert not violations, f"target profile found new Go real-time ownership:\n" + "\n".join(violations)

    gd_violations = architecture_gdscript_violations(repo_root, "target")
    assert not gd_violations, f"target profile found production GDScript outside Bootstrap:\n" + "\n".join(gd_violations)

    engine_violations = architecture_godot_engine_access_violations(repo_root)
    assert not engine_violations, f"mornlea_godot must not call the engine ABI directly:\n" + "\n".join(engine_violations)

    host_violations = architecture_go_host_boundary_from_imports(packages)
    assert not host_violations, f"Go packages must not import Godot, Python, or GPU bindings:\n" + "\n".join(host_violations)

    findings = scan_godot_script_discipline(repo_root / "apps" / "mornlea-godot")
    assert not findings, f"Python/Godot feature code crossed a target ownership boundary:\n{findings}"


def test_architecture_profiles_detect_drift() -> None:
    """Verify architecture profile scanners detect real-time growth, direct engine access, and host leaks."""
    contract = {
        "packages/client/runtime": ["packages/shared/network/protocol"],
        "packages/server/server": ["packages/server/sim/contract"],
    }
    assert not architecture_realtime_growth_violations(contract, "target")

    # New client core is rejected in target profile
    drift_client = dict(contract)
    drift_client["packages/client/cmd/mornlea-godot-core-v2"] = ["packages/client/runtime", "packages/shared/network/protocol"]
    v_client = architecture_realtime_growth_violations(drift_client, "target")
    assert len(v_client) == 1 and "packages/client/cmd/mornlea-godot-core-v2" in v_client[0]

    # New protocol owning package is rejected
    drift_proto = dict(contract)
    drift_proto["packages/client/session2"] = ["packages/shared/network/protocol"]
    v_proto = architecture_realtime_growth_violations(drift_proto, "target")
    assert len(v_proto) == 1 and "packages/client/session2" in v_proto[0]

    # Pilot still admits grandfathered client core
    drift_pilot = dict(contract)
    drift_pilot[PILOT_GO_CLIENT_CORE_PACKAGE] = ["packages/client/runtime"]
    assert not architecture_realtime_growth_violations(drift_pilot, "pilot")

    # Direct engine access is rejected
    engine_violations = architecture_engine_access_from_texts(
        'mornlea_engine = { path = "../mornlea_engine" }',
        ["use mornlea_engine::mesh;"],
    )
    assert len(engine_violations) == 2

    # Go Godot import is rejected
    host_violations = architecture_go_host_boundary_from_imports({
        "packages/client/runtime": ["github.com/godot-rust/gdext"],
    })
    assert len(host_violations) == 1 and "Godot" in host_violations[0]

