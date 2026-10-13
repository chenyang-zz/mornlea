"""Documentation manifest, bilingual pair synchronization, markdown link, and governance policy audits."""

from __future__ import annotations

import hashlib
import json
import os
from pathlib import Path
import re
import subprocess
from typing import Any

import yaml

from audit.common import get_repo_root, run_cmd

DOCUMENTATION_MANIFEST_PATH = "docs/documentation-manifest.json"
OPENSPEC_LANGUAGE_MIGRATION_PATH = "testdata/audit/english-openspec-migration.json"
UPDATE_OPENSPEC_LANGUAGE_DEBT_ENV = "MORNLEA_UPDATE_OPENSPEC_LANGUAGE_BASELINE"

DOCUMENTATION_CLASSIFICATIONS = {
    "bilingual",
    "generated",
    "historical",
    "legacy",
    "license",
    "machine",
    "plan",
}

MARKDOWN_LINK_PATTERN = re.compile(r"\[[^\]]*\]\(([^)]+)\)")
INLINE_CODE_PATTERN = re.compile(r"`([^`\n]+)`")

BASELINE_VERSION_MAPPINGS = [
    {
        "name": "协议版本",
        "doc_pattern": r"protocol v(\d+)",
        "source_path": "packages/shared/network/protocol/packet.go",
        "code_pattern": r"const\s+ProtocolVersion\s+uint32\s*=\s*(\w+)",
    },
    {
        "name": "区块 schema",
        "doc_pattern": r"chunk schema v(\d+)",
        "source_path": "packages/server/storage/chunk/chunk_codec.go",
        "code_pattern": r"currentChunkSchema\s+uint32\s*=\s*(\w+)",
    },
    {
        "name": "玩家 schema",
        "doc_pattern": r"player schema v(\d+)",
        "source_path": "packages/server/storage/player/player_codec.go",
        "code_pattern": r"CurrentSchema\s+uint32\s*=\s*(\w+)",
    },
    {
        "name": "companions.ai schema",
        "doc_pattern": r"`companions\.ai` schema v(\d+)",
        "source_path": "packages/server/storage/companion/companion_codec.go",
        "code_pattern": r"CurrentSchema\s+uint32\s*=\s*(\w+)",
    },
    {
        "name": "世界 metadata 版本",
        "doc_pattern": r"world metadata v(\d+)",
        "source_path": "packages/server/storage/metadata.go",
        "code_pattern": r"currentMetadataVersion\s+uint32\s*=\s*(\w+)",
    },
    {
        "name": "engine ABI",
        "doc_pattern": r"engine ABI v(\d+)",
        "source_path": "packages/engine/include/mornlea_engine.h",
        "code_pattern": r"#define\s+MORNLEA_ENGINE_ABI_VERSION\s+(\d+)u",
    },
    {
        "name": "client ABI",
        "doc_pattern": r"client ABI v(\d+)",
        "source_path": "packages/engine/include/mornlea_client.h",
        "code_pattern": r"#define\s+MORNLEA_CLIENT_ABI_VERSION\s+(\d+)u",
    },
    {
        "name": "hostile_mobs schema",
        "doc_pattern": r"`hostile_mobs` schema v(\d+)",
        "source_path": "packages/server/storage/hostile/hostile_codec.go",
        "code_pattern": r"CurrentSchema\s+uint32\s*=\s*(\w+)",
    },
    {
        "name": "passive_mobs schema",
        "doc_pattern": r"`passive_mobs` schema v(\d+)",
        "source_path": "packages/server/storage/passive/passive_codec.go",
        "code_pattern": r"CurrentSchema\s+uint32\s*=\s*(\w+)",
    },
    {
        "name": "benchmark scenario",
        "doc_pattern": r"benchmark scenario v(\d+)",
        "source_path": "packages/client/cmd/mornlea/benchmark/benchmark.go",
        "code_pattern": r"scenarioVersion\s*=\s*(\w+)",
    },
]


def is_english_canonical_markdown(path: str) -> bool:
    """Check if path is a canonical English markdown path (ends with .md and no language suffix)."""
    return path.endswith(".md") and not path.endswith(".zh.md") and not path.endswith(".en.md")


def read_documentation_manifest(repo_root: Path) -> dict[str, Any]:
    """Read and decode documentation manifest from repository root."""
    path = repo_root / DOCUMENTATION_MANIFEST_PATH
    if not path.is_file():
        fallback = repo_root / "docs" / "manifest.json"
        if fallback.is_file():
            path = fallback
    data = path.read_text(encoding="utf-8")
    return json.loads(data)


def current_governed_markdown(repo_root: Path) -> list[str]:
    """Enumerate all markdown files governed by the documentation manifest."""
    current: list[str] = []
    # Root level markdown files
    for entry in repo_root.iterdir():
        if entry.is_file() and entry.name.endswith(".md"):
            current.append(entry.name)

    # docs/ subtree excluding docs/superpowers
    docs_root = repo_root / "docs"
    if docs_root.is_dir():
        for dirpath, dirnames, filenames in os.walk(docs_root):
            if "superpowers" in dirnames:
                dirnames.remove("superpowers")
            for filename in filenames:
                if filename.endswith(".md"):
                    rel = Path(dirpath, filename).relative_to(repo_root).as_posix()
                    current.append(rel)

    # testdata/visual-golden directory markdown files
    visual_root = repo_root / "testdata" / "visual-golden"
    if visual_root.is_dir():
        for entry in visual_root.iterdir():
            if entry.is_file() and entry.name.endswith(".md"):
                current.append(f"testdata/visual-golden/{entry.name}")

    current.sort()
    return current


def validate_documentation_manifest_shape(manifest: dict[str, Any], current: list[str]) -> list[str]:
    """Validate documentation manifest schema and coverage against governed markdown files."""
    problems: list[str] = []
    if manifest.get("schema_version") != 1:
        problems.append(f"documentation manifest schema_version = {manifest.get('schema_version')}, want 1")

    classified: dict[str, str] = {}
    ids: set[str] = set()

    for index, entry in enumerate(manifest.get("documents", [])):
        doc_id = entry.get("id", "")
        if not doc_id:
            problems.append(f"documents[{index}] has an empty id")
        elif doc_id in ids:
            problems.append(f'duplicate document id "{doc_id}"')
        ids.add(doc_id)

        classification = entry.get("classification", "")
        if classification not in DOCUMENTATION_CLASSIFICATIONS:
            problems.append(f'document "{doc_id}" has unknown classification "{classification}"')

        paths = entry.get("paths", [])
        if not paths:
            problems.append(f'document "{doc_id}" has no current paths')

        english_path = entry.get("english_path", "")
        chinese_path = entry.get("chinese_path", "")
        revision = entry.get("revision", "")

        if classification == "bilingual":
            if not english_path or not chinese_path or not revision:
                problems.append(f'bilingual document "{doc_id}" must declare english_path, chinese_path, and revision')
            if english_path and not is_english_canonical_markdown(english_path):
                problems.append(f'bilingual document "{doc_id}" English path "{english_path}" must be an unsuffixed *.md path')
            if chinese_path and not chinese_path.endswith(".zh.md"):
                problems.append(f'bilingual document "{doc_id}" Chinese path "{chinese_path}" must use the *.zh.md suffix')

        if classification == "legacy" and (english_path or chinese_path or revision):
            problems.append(f'legacy document "{doc_id}" must not declare bilingual path or revision metadata')

        for raw_path in paths:
            path = Path(raw_path).as_posix()
            if path == "." or path.startswith("../") or Path(raw_path).is_absolute():
                problems.append(f'document "{doc_id}" has invalid repository-relative path "{path}"')
                continue
            if path.endswith(".en.md"):
                problems.append(f'English-suffixed documentation path "{path}" is forbidden')
            if path in classified:
                previous = classified[path]
                problems.append(f'document path "{path}" is classified by both "{previous}" and "{doc_id}"')
                continue
            classified[path] = doc_id

    for path in current:
        if path not in classified:
            problems.append(f'current Markdown document "{path}" is not classified in {DOCUMENTATION_MANIFEST_PATH}')

    for path, doc_id in classified.items():
        if path not in current:
            problems.append(f'document "{doc_id}" classifies "{path}", which is outside the current governed Markdown set')

    return problems


def verify_doc_manifest(repo_root: Path) -> list[str]:
    """Read documentation manifest, validate its structure, and verify that all classified files exist on disk."""
    manifest = read_documentation_manifest(repo_root)
    current = current_governed_markdown(repo_root)
    problems = validate_documentation_manifest_shape(manifest, current)

    for entry in manifest.get("documents", []):
        for path in entry.get("paths", []):
            norm_path = Path(path).as_posix()
            full_path = repo_root / norm_path
            if not full_path.is_file():
                problems.append(f'classified document path "{norm_path}" must exist as a file')

    return problems


def changed_repository_paths(repo_root: Path) -> set[str]:
    """Enumerate modified and untracked repository paths relative to root."""
    changed: set[str] = set()
    for args in [["diff", "--name-only", "HEAD", "--"], ["ls-files", "--others", "--exclude-standard"]]:
        proc = run_cmd(["git", *args], cwd=repo_root)
        if proc.returncode == 0:
            for line in proc.stdout.strip().splitlines():
                line = line.strip()
                if line:
                    changed.add(Path(line).as_posix())
    return changed


def legacy_documentation_change_problems(manifest: dict[str, Any], changed: set[str] | dict[str, bool]) -> list[str]:
    """Check that legacy documents in manifest have not changed without migrating to bilingual pairs."""
    problems: list[str] = []
    for entry in manifest.get("documents", []):
        if entry.get("classification") != "legacy":
            continue
        doc_id = entry.get("id", "")
        for path in entry.get("paths", []):
            is_changed = changed.get(path, False) if isinstance(changed, dict) else (path in changed)
            if is_changed:
                problems.append(f'legacy document "{doc_id}" at {path} changed and must migrate to an English *.md and Chinese *.zh.md pair')
    problems.sort()
    return problems


def read_documentation_front_matter(repo_root: Path, relative: str) -> tuple[dict[str, Any], list[str]]:
    """Read and parse YAML front matter from a markdown file."""
    path = repo_root / relative
    if not path.is_file():
        return {}, [f"read {relative}: file does not exist"]
    text = path.read_text(encoding="utf-8")
    if not text.startswith("---\n"):
        return {}, [f"{relative} has no YAML front matter"]
    rest = text[4:]
    end = rest.find("\n---\n")
    if end < 0:
        return {}, [f"{relative} has unterminated YAML front matter"]
    try:
        data = yaml.safe_load(rest[:end])
        if not isinstance(data, dict):
            return {}, [f"decode {relative} front matter: invalid format"]
        return data, []
    except Exception as exc:
        return {}, [f"decode {relative} front matter: {exc}"]


def resolved_counterpart(owner_path: str, counterpart: str) -> str:
    """Resolve counterpart path relative to owner documentation path."""
    if not counterpart:
        return ""
    if "/" in counterpart:
        return Path(counterpart).as_posix()
    parent = Path(owner_path).parent
    if parent == Path("."):
        return counterpart
    return (parent / counterpart).as_posix()


def validate_documentation_pair(repo_root: Path, entry: dict[str, Any]) -> list[str]:
    """Validate bilingual documentation pair consistency and front matter attributes."""
    problems: list[str] = []
    english_path = entry.get("english_path", "")
    chinese_path = entry.get("chinese_path", "")

    if not is_english_canonical_markdown(english_path):
        problems.append(f"{english_path} must be an unsuffixed *.md English canonical path")
    if not chinese_path.endswith(".zh.md"):
        problems.append(f"{chinese_path} must use the *.zh.md Chinese counterpart suffix")

    # Public repository README pair omits internal YAML front matter
    if entry.get("id") == "root-readme":
        for rel in [english_path, chinese_path]:
            if not (repo_root / rel).is_file():
                problems.append(f"read {rel}: file does not exist")
        return problems

    english_meta, english_problems = read_documentation_front_matter(repo_root, english_path)
    chinese_meta, chinese_problems = read_documentation_front_matter(repo_root, chinese_path)
    metadata_problems = [*english_problems, *chinese_problems]
    problems.extend(metadata_problems)
    if metadata_problems:
        return problems

    for path, meta in [(english_path, english_meta), (chinese_path, chinese_meta)]:
        if meta.get("doc_id") != entry.get("id"):
            problems.append(f'{path} doc_id = "{meta.get("doc_id")}", want "{entry.get("id")}"')
        if meta.get("doc_revision") != entry.get("revision"):
            problems.append(f'{path} doc_revision = "{meta.get("doc_revision")}", want "{entry.get("revision")}"')

    if english_meta.get("language") != "en":
        problems.append(f'{english_path} language = "{english_meta.get("language")}", want en')
    if chinese_meta.get("language") != "zh-CN":
        problems.append(f'{chinese_path} language = "{chinese_meta.get("language")}", want zh-CN')

    if resolved_counterpart(english_path, english_meta.get("counterpart", "")) != chinese_path:
        problems.append(f'{english_path} counterpart = "{english_meta.get("counterpart")}", want "{chinese_path}"')
    if resolved_counterpart(chinese_path, chinese_meta.get("counterpart", "")) != english_path:
        problems.append(f'{chinese_path} counterpart = "{chinese_meta.get("counterpart")}", want "{english_path}"')

    return problems


def verify_doc_pairs(repo_root: Path) -> list[str]:
    """Verify that all completed bilingual documentation pairs in manifest are synchronized."""
    manifest = read_documentation_manifest(repo_root)
    problems: list[str] = []
    for entry in manifest.get("documents", []):
        if entry.get("classification") != "bilingual":
            continue
        english_path = entry.get("english_path", "")
        chinese_path = entry.get("chinese_path", "")
        paths = entry.get("paths", [])
        if english_path in paths and chinese_path in paths:
            problems.extend(validate_documentation_pair(repo_root, entry))
    return problems


def active_openspec_markdown(repo_root: Path) -> list[str]:
    """Enumerate active OpenSpec markdown files, skipping only the frozen top-level archive directory."""
    changes_root = repo_root / "openspec" / "changes"
    archive_root = changes_root / "archive"
    paths: list[str] = []
    if changes_root.is_dir():
        for dirpath, dirnames, filenames in os.walk(changes_root):
            curr_dir = Path(dirpath)
            if curr_dir == archive_root or archive_root in curr_dir.parents:
                continue
            for filename in filenames:
                if filename.endswith(".md"):
                    rel = (curr_dir / filename).relative_to(repo_root).as_posix()
                    paths.append(rel)
    paths.sort()
    return paths


def current_completed_documentation_pairs(repo_root: Path) -> list[str]:
    """Return paths belonging to completed bilingual documentation pairs."""
    manifest = read_documentation_manifest(repo_root)
    paths: list[str] = []
    for entry in manifest.get("documents", []):
        if entry.get("classification") != "bilingual":
            continue
        en = entry.get("english_path", "")
        zh = entry.get("chinese_path", "")
        entry_paths = entry.get("paths", [])
        if en in entry_paths and zh in entry_paths:
            paths.extend([en, zh])
    paths.sort()
    return paths


def documentation_link_scope(repo_root: Path) -> list[str]:
    """Enumerate Markdown files whose local links must resolve."""
    paths = [*current_completed_documentation_pairs(repo_root), *active_openspec_markdown(repo_root)]
    return sorted(set(paths))


def local_documentation_link_target(raw: str) -> str:
    """Extract local documentation link target path, excluding anchors and external schemes."""
    target = raw.split("#", 1)[0].strip()
    if not target or "://" in target or target.startswith("mailto:"):
        return ""
    if target.startswith("<") and target.endswith(">"):
        target = target[1:-1]
    return Path(target).as_posix()


def documentation_link_problems(repo_root: Path, relative: str, text: str) -> list[str]:
    """Inspect Markdown text for broken relative local links."""
    problems: list[str] = []
    for match in MARKDOWN_LINK_PATTERN.finditer(text):
        raw_target = match.group(1)
        target = local_documentation_link_target(raw_target)
        if not target:
            continue
        absolute = (repo_root / relative).parent / target
        if not absolute.exists():
            problems.append(f'{relative} has broken local link "{raw_target}": {target} does not exist')
    return problems


def verify_markdown_links(repo_root: Path) -> list[str]:
    """Verify that all relative markdown links in scope resolve to valid repository paths."""
    scope = documentation_link_scope(repo_root)
    problems: list[str] = []
    for rel in scope:
        text = (repo_root / rel).read_text(encoding="utf-8")
        problems.extend(documentation_link_problems(repo_root, rel, text))
    return problems


def strip_historical_markdown_sections(text: str) -> str:
    """Strip historical markdown sections beneath headings indicating historical context."""
    lines = text.split("\n")
    output: list[str] = []
    skip_level = 0
    for line in lines:
        trimmed = line.lstrip(" ")
        count = 0
        while count < len(trimmed) and trimmed[count] == "#":
            count += 1
        if count > 0 and count < len(trimmed) and trimmed[count] == " ":
            level = count
            heading = trimmed[count + 1:].strip()
            if skip_level > 0 and level <= skip_level:
                skip_level = 0
            lower = heading.lower()
            if "historical" in lower or "历史" in heading:
                skip_level = level
                continue
        if skip_level == 0:
            output.append(line)
    return "\n".join(output)


def normalize_documentation_command(text: str) -> str:
    """Normalize command text by removing comments and placeholder arguments."""
    if " #" in text:
        text = text.split(" #", 1)[0]
    fields = text.split()
    for index in range(len(fields) - 1):
        if fields[index] == "--name":
            fields[index + 1] = "<localized-name>"
    return " ".join(fields)


def is_documentation_command(text: str) -> bool:
    """Determine whether text represents an executable documentation command."""
    prefixes = [
        "$openspec-",
        "./scripts/",
        "GATES_",
        "VISUAL_OUT=",
        "corepack ",
        "gh ",
        "git ",
        "go ",
        "make ",
        "node ",
        "npm ",
        "npx ",
        "openspec ",
        "scripts/",
    ]
    return any(text.startswith(p) for p in prefixes)


def doc_command_claims(text: str) -> list[str]:
    """Extract normalized documentation commands from code fences and inline blocks."""
    text = strip_historical_markdown_sections(text)
    commands: list[str] = []
    in_fence = False
    for line in text.split("\n"):
        trimmed = line.strip()
        if trimmed.startswith("```"):
            in_fence = not in_fence
            continue
        if in_fence and is_documentation_command(trimmed):
            commands.append(normalize_documentation_command(trimmed))
        for match in INLINE_CODE_PATTERN.finditer(line):
            candidate = match.group(1).strip()
            if is_documentation_command(candidate):
                commands.append(normalize_documentation_command(candidate))
    return sorted(set(commands))


def normalize_localized_documentation_path(path: str) -> str:
    """Normalize Chinese localized documentation paths to their canonical form."""
    if path.endswith(".zh.md"):
        return path[:-6] + ".md"
    return path


def is_repository_path_claim(candidate: str) -> bool:
    """Check if candidate string represents a known repository path."""
    if not candidate or "://" in candidate:
        return False
    prefixes = [
        "apps/",
        "docs/",
        "openspec/",
        "packages/",
        "scripts/",
        "testdata/",
        ".codex/",
        ".claude/",
        ".github/",
    ]
    if any(candidate.startswith(p) for p in prefixes):
        return True
    exact = ["AGENTS.md", "CLAUDE.md", "Makefile", "README.md", "README.zh.md", "go.work"]
    return candidate in exact


def doc_repo_path_claims(text: str) -> list[str]:
    """Extract referenced repository paths from inline code spans."""
    text = strip_historical_markdown_sections(text)
    claims: list[str] = []
    for match in INLINE_CODE_PATTERN.finditer(text):
        for field in match.group(1).split():
            candidate = field.strip("'\"(),:;[]")
            if candidate.startswith("./"):
                candidate = candidate[2:]
            if is_repository_path_claim(candidate):
                claims.append(normalize_localized_documentation_path(candidate))
    return sorted(set(claims))


def doc_local_link_claims(text: str) -> list[str]:
    """Extract local markdown link target claims."""
    text = strip_historical_markdown_sections(text)
    claims: list[str] = []
    for match in MARKDOWN_LINK_PATTERN.finditer(text):
        target = local_documentation_link_target(match.group(1))
        if target:
            claims.append(normalize_localized_documentation_path(target))
    return sorted(set(claims))


def current_documentation_version_patterns(mapping: dict[str, Any]) -> list[str]:
    """Return version matching regex patterns for a given baseline mapping."""
    patterns = [mapping["doc_pattern"]]
    name = mapping["name"]
    if name == "协议版本":
        patterns.append(r"协议 v(\d+)")
    elif name == "区块 schema":
        patterns.append(r"区块 schema v(\d+)")
    elif name == "玩家 schema":
        patterns.append(r"玩家 schema v(\d+)")
    elif name == "世界 metadata 版本":
        patterns.append(r"世界 metadata v(\d+)")
    elif name == "engine ABI":
        patterns.append(r"engine C ABI (?:is currently |currently |当前为 )?v(\d+)")
    elif name == "client ABI":
        patterns.append(r"client C ABI (?:is currently |currently |当前为 )?v(\d+)")
    elif name == "benchmark scenario":
        patterns.append(r"benchmark scenario 为 v(\d+)")
    return patterns


def doc_version_claims(text: str) -> list[str]:
    """Extract contract version claims matching baseline patterns."""
    text = strip_historical_markdown_sections(text)
    claims: list[str] = []
    for mapping in BASELINE_VERSION_MAPPINGS:
        for pattern in current_documentation_version_patterns(mapping):
            for match in re.finditer(pattern, text):
                claims.append(f"{mapping['name']}=v{match.group(1)}")
    return sorted(set(claims))


def validate_documentation_semantic_pair(
    doc_id: str,
    english_path: str,
    chinese_path: str,
    english: str,
    chinese: str,
) -> list[str]:
    """Validate that English and Chinese documentation share identical semantic claims."""
    checks = [
        ("command", doc_command_claims),
        ("repository path", doc_repo_path_claims),
        ("local link", doc_local_link_claims),
        ("version", doc_version_claims),
    ]
    problems: list[str] = []
    for name, extract in checks:
        en_claims = extract(english)
        zh_claims = extract(chinese)
        if en_claims != zh_claims:
            problems.append(
                f"{doc_id} {name} claims differ between {english_path} and {chinese_path}:\n"
                f"English: {en_claims}\n"
                f"Chinese: {zh_claims}"
            )
    return problems


def current_normative_documentation(repo_root: Path) -> list[str]:
    """Enumerate machine-readable and completed bilingual normative documents."""
    manifest = read_documentation_manifest(repo_root)
    paths: list[str] = []
    for entry in manifest.get("documents", []):
        classification = entry.get("classification")
        if classification == "machine":
            paths.extend(entry.get("paths", []))
        elif classification == "bilingual":
            en = entry.get("english_path", "")
            zh = entry.get("chinese_path", "")
            entry_paths = entry.get("paths", [])
            if en in entry_paths and zh in entry_paths:
                paths.extend([en, zh])
    return sorted(set(paths))


def resolve_baseline_constant(source_text: str, code_pattern: str) -> str:
    """Extract constant value, resolving indirect constant definitions up to 4 hops."""
    match = re.search(code_pattern, source_text)
    if not match:
        raise ValueError(f"Pattern {code_pattern!r} did not match source text")

    val = match.group(1).strip()
    max_hops = 4
    for _ in range(max_hops):
        if val.isdigit():
            return val
        indirect_pattern = rf"(?m)^\s*(?:const\s+)?{re.escape(val)}\s+(?:uint32\s*=\s*|\s*=\s*)(\w+)"
        next_match = re.search(indirect_pattern, source_text)
        if not next_match:
            next_match = re.search(rf"\b{re.escape(val)}\s+uint32\s*=\s*(\w+)", source_text)
            if not next_match:
                raise ValueError(f"Indirect constant {val} not found in source text")
        val = next_match.group(1).strip()

    if not val.isdigit():
        raise ValueError(f"Constant {val} could not be resolved to digits within {max_hops} hops")
    return val


def contains_han_char(text: str) -> bool:
    """Check if character sequence contains any CJK Han character."""
    for char in text:
        cp = ord(char)
        if (
            0x4E00 <= cp <= 0x9FFF
            or 0x3400 <= cp <= 0x4DBF
            or 0x20000 <= cp <= 0x2A6DF
            or 0x2A700 <= cp <= 0x2B73F
            or 0x2B740 <= cp <= 0x2B81F
            or 0x2B820 <= cp <= 0x2CEAF
            or 0xF900 <= cp <= 0xFAFF
            or 0x2F800 <= cp <= 0x2FA1F
        ):
            return True
    return False


def remove_explicit_localized_literals(line: str) -> str:
    """Strip localized(...) string wrapper literals from code prose."""
    marker = 'localized("'
    while True:
        start = line.find(marker)
        if start < 0:
            return line
        rest = line[start + len(marker):]
        end = rest.find('")')
        if end < 0:
            return line
        line = line[:start] + rest[end + 2:]


def non_english_markdown_prose_lines(text: str) -> list[str]:
    """Scan markdown for lines containing non-English Han prose outside code fences and literals."""
    findings: list[str] = []
    in_fence = False
    for line_number, line in enumerate(text.split("\n"), 1):
        trimmed = line.strip()
        if trimmed.startswith("```") or trimmed.startswith("~~~"):
            in_fence = not in_fence
            continue
        if in_fence:
            continue
        prose = INLINE_CODE_PATTERN.sub("", line)
        prose = remove_explicit_localized_literals(prose)
        if contains_han_char(prose):
            findings.append(f"line {line_number}: {trimmed}")
    return findings


def current_openspec_language_debt(repo_root: Path) -> dict[str, Any]:
    """Calculate current non-English line counts and content digests across OpenSpec specifications."""
    spec_root = repo_root / "openspec" / "specs"
    paths: list[str] = []
    if spec_root.is_dir():
        for dirpath, _, filenames in os.walk(spec_root):
            for filename in filenames:
                if filename.endswith(".md"):
                    rel = Path(dirpath, filename).relative_to(repo_root).as_posix()
                    paths.append(rel)
    paths.sort()

    migration: dict[str, Any] = {
        "schema_version": 1,
        "total_files": 0,
        "total_lines": 0,
        "files": [],
    }
    for path in paths:
        text = (repo_root / path).read_text(encoding="utf-8")
        lines = non_english_markdown_prose_lines(text)
        if not lines:
            continue
        digest = hashlib.sha256("\n".join(lines).encode("utf-8")).hexdigest()
        migration["files"].append({
            "path": path,
            "count": len(lines),
            "digest": digest,
        })
        migration["total_lines"] += len(lines)
    migration["total_files"] = len(migration["files"])
    return migration


def compare_planning_language_debt(
    baseline: dict[str, Any],
    current: dict[str, Any],
    allow_decrease: bool = False,
) -> list[str]:
    """Compare baseline language debt inventory against current scan results."""
    problems: list[str] = []
    if baseline.get("schema_version") != 1:
        return [f"baseline schema_version = {baseline.get('schema_version')}, want 1"]
    if current.get("schema_version") != 1:
        return [f"current schema_version = {current.get('schema_version')}, want 1"]

    baseline_files = {f["path"]: f for f in baseline.get("files", [])}
    current_files = {f["path"]: f for f in current.get("files", [])}

    for path, curr_file in current_files.items():
        if path not in baseline_files:
            problems.append(f"{path} has new non-English planning debt ({curr_file['count']} lines)")
            continue
        base_file = baseline_files[path]
        if curr_file["count"] > base_file["count"]:
            problems.append(f"{path} increased from {base_file['count']} to {curr_file['count']} lines")
        elif curr_file["count"] == base_file["count"] and curr_file["digest"] != base_file["digest"]:
            problems.append(f"{path} planning debt changed without decreasing")
        elif curr_file["count"] < base_file["count"] and not allow_decrease:
            problems.append(
                f"{path} decreased from {base_file['count']} to {curr_file['count']} lines but the checked-in baseline was not ratcheted"
            )

    for path, base_file in baseline_files.items():
        if path not in current_files and not allow_decrease:
            problems.append(
                f"{path} decreased from {base_file['count']} to 0 lines but the checked-in baseline was not ratcheted"
            )

    if not allow_decrease:
        if baseline.get("total_files") != current.get("total_files"):
            problems.append(f"total_files changed from {baseline.get('total_files')} to {current.get('total_files')}")
        if baseline.get("total_lines") != current.get("total_lines"):
            problems.append(f"total_lines changed from {baseline.get('total_lines')} to {current.get('total_lines')}")

    problems.sort()
    return problems


def verify_openspec_language(repo_root: Path) -> list[str]:
    """Verify that active OpenSpec changes contain only English prose and debt inventory is not exceeded."""
    problems: list[str] = []
    for rel in active_openspec_markdown(repo_root):
        text = (repo_root / rel).read_text(encoding="utf-8")
        for line in non_english_markdown_prose_lines(text):
            problems.append(f"{rel}: {line}")

    debt_path = repo_root / OPENSPEC_LANGUAGE_MIGRATION_PATH
    if debt_path.is_file():
        baseline = json.loads(debt_path.read_text(encoding="utf-8"))
        current = current_openspec_language_debt(repo_root)
        problems.extend(compare_planning_language_debt(baseline, current, allow_decrease=False))

    return problems


def validate_visual_evidence_route(
    path: str,
    pilot: bool = False,
    update: bool = False,
    has_handoff: bool = False,
) -> str | None:
    """Validate visual artifact destination path against routing and handoff policies."""
    norm = Path(path).as_posix()
    tracked = "testdata/visual-golden/"
    if norm.startswith(tracked + "godot/"):
        return "renderer-specific tracked visual classes are forbidden"
    if pilot and norm.startswith(tracked):
        return "pilot evidence must stay outside tracked visual baselines"
    if pilot and not norm.startswith("build/visual/godot-pilot/"):
        return "Godot pilot evidence must use the dedicated build output"
    if update and norm.startswith(tracked) and not has_handoff:
        return "tracked producer update requires an approved handoff"
    return None


def delegation_policy_fragments() -> list[str]:
    """Required text fragments enforcing subagent budget and context isolation."""
    return [
        "isolation-first",
        "At most three subagents may run concurrently",
        "main-context retention",
        "Do not delegate merely for parallel speed",
        "concise task brief",
    ]


def delegation_policy_violations(text: str) -> list[str]:
    """Find missing delegation policy fragments in governance text."""
    violations: list[str] = []
    for fragment in delegation_policy_fragments():
        if fragment not in text:
            violations.append(f"missing delegation policy fragment: {fragment}")
    return violations


def canonical_delegation_budget_violations(text: str) -> list[str]:
    """Verify canonical governance specification preserves the 3-subagent ceiling."""
    violations: list[str] = []
    for required in [
        "MUST run no more than three subagents concurrently",
        "MUST NOT start a fourth concurrent subagent",
    ]:
        if required not in text:
            violations.append(f"missing canonical ceiling fragment: {required}")
    for forbidden in [
        "no more than two subagents",
        "start a third concurrent subagent",
    ]:
        if forbidden in text:
            violations.append(f"stale canonical ceiling fragment: {forbidden}")
    return violations


def project_orchestration_retrospective_skill_fragments() -> list[str]:
    """Skill text fragments governing retrospective task status tracking."""
    return [
        "`tasks.md` is the sole OpenSpec plan identity and status source",
        "Do not create or maintain a flat or packet-keyed `.superpowers/sdd` progress store",
        "A failed required closeout gate leaves its node open",
        "Revise the acceptance contract explicitly before archive",
        "repository-wide producer enumeration",
        "enumerate every hashed, generated, embedded, or source-scanned consumer of each editable file",
        "Assign the refresh authority or algorithm, artifact ownership, and downstream consumer gate",
    ]


def project_orchestration_retrospective_reference_fragments() -> list[str]:
    """Reference document text fragments governing retrospective task status tracking."""
    return [
        "`tasks.md` remains the sole OpenSpec plan identity and status source",
        "Append per-node implementation and status evidence to the change ledger before starting the next node",
        "Batched retroactive acceptance is a recorded deviation",
        "A failed required closeout gate keeps the node open",
        "explicit acceptance-contract revision before archive",
        "repository-wide producer enumeration",
        "Comment-only changes are not exempt from derived-consumer inventory",
        "Repeat the derived-consumer enumeration during review",
    ]


def missing_orchestration_retrospective_fragments(source: str, required: list[str]) -> list[str]:
    """Return violations for missing retrospective policy fragments."""
    violations: list[str] = []
    for fragment in required:
        if fragment not in source:
            violations.append(f"missing retrospective-policy fragment: {fragment}")
    return violations


def openspec_apply_routing_violations(source: str) -> list[str]:
    """Verify OpenSpec apply skill routes orchestration and preserves status states."""
    required = [
        "../mornlea-implementation-orchestration/SKILL.md",
        "openspec status --change",
        "openspec instructions apply --change",
        'state: "blocked"',
        'state: "all_done"',
        "contextFiles",
        "Preserve CLI-controlled `blocked`, `ready`, and `all_done` states",
    ]
    violations: list[str] = []
    for fragment in required:
        if fragment not in source:
            violations.append(f"missing required apply-workflow fragment: {fragment}")
    return violations


PILOT_GO_CLIENT_CORE_PACKAGE = "packages/client/cmd/mornlea-godot-core"
GODOT_PROJECT_ROOT = "apps/mornlea-godot"
GODOT_RUST_CRATE_ROOT = "packages/engine/crates/mornlea_godot"

GRANDFATHERED_GO_REALTIME_PACKAGES = [
    "packages/client/client",
    "packages/client/cmd/mornlea-godot-core",
    "packages/client/lod",
    "packages/client/mesh",
    "packages/client/presentation",
    "packages/client/runtime",
    "packages/server/cmd/mornlea-server",
    "packages/server/fluid",
    "packages/server/server",
    "packages/server/server/persistence",
    "packages/server/sim/contract",
    "packages/server/sim/entity",
    "packages/server/sim/realm",
    "packages/server/sim/runtime",
    "packages/shared/core",
    "packages/shared/nativeabi",
    "packages/shared/network",
    "packages/shared/network/codec",
    "packages/shared/network/protocol",
    "packages/shared/network/tcp",
    "packages/shared/physics",
    "packages/shared/worldgen",
]

PILOT_BOOTSTRAP_GDSCRIPT = [
    "app/bootstrap/bootstrap.gd",
    "app/bootstrap/setup_required.gd",
]


def is_go_realtime_import(imported: str) -> bool:
    """Check if imported package is part of the real-time boundary."""
    return (
        imported.startswith("packages/shared/network")
        or imported.startswith("packages/server/sim")
        or imported == "packages/client/runtime"
        or imported == "packages/shared/nativeabi"
    )


def package_owns_go_realtime(pkg: str, imports: list[str] | set[str]) -> bool:
    """Determine whether Go package owns real-time responsibilities."""
    if pkg.startswith("packages/tools") or pkg == "packages/audit" or pkg.startswith("packages/contracts"):
        return False
    if pkg == PILOT_GO_CLIENT_CORE_PACKAGE:
        return True
    for imported in imports:
        if is_go_realtime_import(imported):
            return True
    return (
        pkg in ("packages/client/client", "packages/client/runtime", "packages/client/presentation")
        or pkg.startswith("packages/server/sim/")
        or pkg in ("packages/server/server", "packages/server/server/persistence", "packages/shared/nativeabi")
        or pkg.startswith("packages/shared/network")
    )


def architecture_realtime_growth_violations(
    packages: dict[str, list[str] | set[str]],
    profile: str,
) -> list[str]:
    """Check that no new Go package assumes real-time ownership beyond grandfathered inventory."""
    inventory = set(GRANDFATHERED_GO_REALTIME_PACKAGES)
    violations: list[str] = []
    for pkg in sorted(packages.keys()):
        if not package_owns_go_realtime(pkg, packages[pkg]):
            continue
        if pkg in inventory:
            if profile == "target" and pkg == PILOT_GO_CLIENT_CORE_PACKAGE:
                continue
            continue
        violations.append(f"new Go real-time ownership {pkg} is not in the grandfathered inventory")
    if profile == "pilot" and PILOT_GO_CLIENT_CORE_PACKAGE not in inventory:
        violations.append("pilot profile lost the Go client-core transition exception")
    return violations


def architecture_gdscript_violations(repo_root: Path, profile: str) -> list[str]:
    """Scan Godot project scripts for GDScript files outside approved Bootstrap allowlist."""
    allow = set(PILOT_BOOTSTRAP_GDSCRIPT)
    violations: list[str] = []
    project_root = repo_root / GODOT_PROJECT_ROOT
    if project_root.is_dir():
        for dirpath, dirnames, filenames in os.walk(project_root):
            for skip in [".godot", ".venv", "tests", "py4godot"]:
                if skip in dirnames:
                    dirnames.remove(skip)
            for f in filenames:
                if f.endswith(".gd"):
                    rel = Path(dirpath, f).relative_to(project_root).as_posix()
                    if rel not in allow:
                        violations.append(f"production GDScript outside Bootstrap: {rel}")
    if profile == "pilot":
        for rel in PILOT_BOOTSTRAP_GDSCRIPT:
            if not (project_root / rel).is_file():
                violations.append(f"pilot Bootstrap script is missing: {rel}")
    return violations


def architecture_engine_access_from_texts(cargo: str, sources: list[str]) -> list[str]:
    """Detect direct mornlea_engine references in Godot Rust extension crate."""
    violations: list[str] = []
    for marker in ["mornlea_engine", "mornlea_engine.h", "nativeabi"]:
        if marker in cargo:
            violations.append(f"mornlea_godot Cargo.toml references {marker}")
    for source in sources:
        for marker in ["mornlea_engine", "mornlea_engine.h", "FluidEvalBatch", "nativeabi"]:
            if marker in source:
                violations.append(f"mornlea_godot source references {marker}")
    return violations


def architecture_godot_engine_access_violations(repo_root: Path) -> list[str]:
    """Verify mornlea_godot crate does not call numerical engine directly."""
    cargo_path = repo_root / GODOT_RUST_CRATE_ROOT / "Cargo.toml"
    cargo = cargo_path.read_text(encoding="utf-8") if cargo_path.is_file() else ""
    sources: list[str] = []
    src_dir = repo_root / GODOT_RUST_CRATE_ROOT / "src"
    if src_dir.is_dir():
        for p in src_dir.rglob("*.rs"):
            if p.is_file():
                sources.append(p.read_text(encoding="utf-8"))
    return architecture_engine_access_from_texts(cargo, sources)


def architecture_go_host_boundary_from_imports(imports: dict[str, list[str] | set[str]]) -> list[str]:
    """Check that Go packages do not import Godot, Python, or GPU bindings."""
    violations: list[str] = []
    for pkg in sorted(imports.keys()):
        if pkg == "packages/audit":
            continue
        for imported in imports[pkg]:
            lower = imported.lower()
            if "godot" in lower:
                violations.append(f"{pkg} imports Godot binding {imported}")
            elif "py4godot" in lower or "cpython" in lower:
                violations.append(f"{pkg} imports Python runtime binding {imported}")
            elif "webgpu" in lower or "vulkan" in lower:
                violations.append(f"{pkg} imports GPU binding {imported}")
    return violations

