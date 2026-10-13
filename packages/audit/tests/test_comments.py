"""Tests for multilingual comment extraction, language validation, and task ID scanning."""

from pathlib import Path
import pytest

from audit.comments import (
    CommentToken,
    Finding,
    Source,
    collect_first_party_sources,
    comment_language_for_path,
    extract_comments,
    python_string_end,
    scan_comment_language,
    scan_comment_task_ids,
)


def test_comment_language_for_path() -> None:
    assert comment_language_for_path("main.go") == "go"
    assert comment_language_for_path("lib.rs") == "rust"
    assert comment_language_for_path("native.c") == "c"
    assert comment_language_for_path("header.h") == "c"
    assert comment_language_for_path("script.py") == "python"
    assert comment_language_for_path("scene.gd") == "gdscript"
    assert comment_language_for_path("app.js") == "javascript"
    assert comment_language_for_path("component.jsx") == "javascript"
    assert comment_language_for_path("tool.mjs") == "javascript"
    assert comment_language_for_path("types.ts") == "typescript"
    assert comment_language_for_path("view.tsx") == "typescript"
    assert comment_language_for_path("readme.md") is None


def test_extract_comments_go() -> None:
    source = b"""package main
const msg = `raw // not a comment`
const str = "quoted /* not a comment */"
// First Go comment
/* Multiline Go
   block comment */
"""
    tokens = extract_comments(source, "go")
    assert len(tokens) == 2
    assert tokens[0] == CommentToken(text="// First Go comment", line=4, offset=89)
    assert tokens[1].line == 5
    assert "Multiline Go" in tokens[1].text


def test_extract_comments_rust() -> None:
    source = b"""fn main() {
    let raw = r#"raw /* not a comment */"#;
    let ch = 'a';
    // Line comment
    /* outer /* nested */ comment */
}
"""
    tokens = extract_comments(source, "rust")
    assert len(tokens) == 2
    assert tokens[0].text == "// Line comment"
    assert tokens[0].line == 4
    assert tokens[1].text == "/* outer /* nested */ comment */"
    assert tokens[1].line == 5


def test_extract_comments_c() -> None:
    source = b"""int main() {
    const char *s = "text // not comment";
    // spliced line \\\ncontinuation
    /* block comment */
    return 0;
}
"""
    tokens = extract_comments(source, "c")
    assert len(tokens) == 2
    assert "spliced line \\\ncontinuation" in tokens[0].text
    assert tokens[0].line == 3
    assert tokens[1].text == "/* block comment */"
    assert tokens[1].line == 5


def test_extract_comments_python() -> None:
    source = b'''# Header comment
s = "string with # not a comment"
t = """triple
# not a comment
"""
r = r"""raw triple with \\""" not a comment"""
# Line comment
x = 1 # Inline comment
'''
    tokens = extract_comments(source, "python")
    assert len(tokens) == 3
    assert tokens[0].text == "# Header comment"
    assert tokens[0].line == 1
    assert tokens[1].text == "# Line comment"
    assert tokens[1].line == 7
    assert tokens[2].text == "# Inline comment"
    assert tokens[2].line == 8


def test_python_string_end_raw_triple_quotes() -> None:
    source = b'r"""hello\\"""world"""'
    start = source.find(b'"""')
    assert python_string_end(source, start, raw=True) == len(source)


def test_extract_comments_gdscript() -> None:
    source = b'''extends Node
var lit = "str # not comment"
var multi = """multi
# still string"""
# GDScript comment
'''
    tokens = extract_comments(source, "gdscript")
    assert len(tokens) == 1
    assert tokens[0].text == "# GDScript comment"
    assert tokens[0].line == 5


def test_extract_comments_javascript_and_typescript() -> None:
    source = b"""const re = /\\/\\/not comment/;
const s = `template ${ /* inside template */ 123 } literal`;
// JS line comment
/* JS block comment */
"""
    tokens = extract_comments(source, "javascript")
    assert len(tokens) == 3
    assert tokens[0].text == "/* inside template */"
    assert tokens[0].line == 2
    assert tokens[1].text == "// JS line comment"
    assert tokens[1].line == 3
    assert tokens[2].text == "/* JS block comment */"
    assert tokens[2].line == 4


def test_scan_comment_language_mutations() -> None:
    fixtures = [
        {
            "name": "go",
            "path": "sample.go",
            "clean": "package sample\nconst localized = `中文 // 不是注释`\n// English comment.\n",
            "mutations": [
                "package sample\nconst localized = `中文 // 不是注释`\n// 中文行注释\n",
                "package sample\nconst localized = \"中文 /* 不是注释 */\"\n/* 中文块注释 */\n",
            ],
            "want_lines": [3, 3],
            "want_tokens": ["// 中文行注释", "/* 中文块注释 */"],
        },
        {
            "name": "rust",
            "path": "sample.rs",
            "clean": "const LOCALIZED: &str = r#\"中文 // 不是注释\"#;\n// English comment.\n",
            "mutations": [
                "const LOCALIZED: &str = r#\"中文 // 不是注释\"#;\n// 中文行注释\n",
                "const LOCALIZED: &str = \"中文 /* 不是注释 */\";\n/* outer /* 中文嵌套注释 */ block */\n",
            ],
            "want_lines": [2, 2],
            "want_tokens": ["// 中文行注释", "/* outer /* 中文嵌套注释 */ block */"],
        },
        {
            "name": "c",
            "path": "sample.c",
            "clean": "const char *localized = \"中文 // 不是注释\";\n// English comment.\n",
            "mutations": [
                "const char *localized = \"中文 // 不是注释\";\n// 中文行注释\n",
                "const char *localized = \"中文 /* 不是注释 */\";\n/* 中文块注释 */\n",
            ],
            "want_lines": [2, 2],
            "want_tokens": ["// 中文行注释", "/* 中文块注释 */"],
        },
        {
            "name": "gdscript",
            "path": "sample.gd",
            "clean": "var localized = \"中文 # 不是注释\"\nvar multiline = \"\"\"中文\n# 仍然不是注释\"\"\"\n# English comment.\n",
            "mutations": [
                "var localized = \"中文 # 不是注释\"\n# 中文行注释\n",
            ],
            "want_lines": [2],
            "want_tokens": ["# 中文行注释"],
        },
        {
            "name": "javascript",
            "path": "sample.js",
            "clean": "const localized = \"中文 // 不是注释\";\nconst template = `中文 /* 不是注释 */`;\nconst matcher = /\\/\\/中文/;\n",
            "mutations": [
                "const localized = \"中文 // 不是注释\";\n// 中文行注释\n",
                "const localized = `中文 /* 不是注释 */`;\n/* 中文块注释 */\n",
            ],
            "want_lines": [2, 2],
            "want_tokens": ["// 中文行注释", "/* 中文块注释 */"],
        },
        {
            "name": "typescript",
            "path": "sample.ts",
            "clean": "const localized: string = '中文 // 不是注释';\n// English comment.\n",
            "mutations": [
                "const localized: string = '中文 // 不是注释';\n// 中文行注释\n",
                "const localized: string = \"中文 /* 不是注释 */\";\n/* 中文块注释 */\n",
            ],
            "want_lines": [2, 2],
            "want_tokens": ["// 中文行注释", "/* 中文块注释 */"],
        },
        {
            "name": "python",
            "path": "sample.py",
            "clean": "localized = '中文 # 不是注释'\nmultiline = '''中文\n# 仍然不是注释'''\n# English comment.\n",
            "mutations": [
                "localized = '中文 # 不是注释'\n# 中文行注释\n",
            ],
            "want_lines": [2],
            "want_tokens": ["# 中文行注释"],
        },
    ]

    for fix in fixtures:
        clean_source = [Source(path=fix["path"], data=fix["clean"].encode("utf-8"))]
        clean_findings = scan_comment_language(clean_source)
        assert len(clean_findings) == 0, f"False positive in {fix['name']}: {clean_findings}"

        for idx, mutation in enumerate(fix["mutations"]):
            mut_source = [Source(path=fix["path"], data=mutation.encode("utf-8"))]
            findings = scan_comment_language(mut_source)
            assert len(findings) == 1, f"{fix['name']} mutation {idx} findings count = {len(findings)}"
            assert findings[0].line == fix["want_lines"][idx]
            assert findings[0].text == fix["want_tokens"][idx]


def test_scan_comment_task_ids() -> None:
    bad_prefix = chr(65) + "-01"
    bad_source = [
        Source(path="bad.go", data=f"package sample\n// Wait for {bad_prefix} done\n".encode("utf-8")),
        Source(path="bad.rs", data=f"// Wait for {chr(66)}-02 done\nfn main() {{}}\n".encode("utf-8")),
        Source(path="bad.py", data=f"# Wait for {chr(67)}-03 done\n".encode("utf-8")),
    ]
    findings = scan_comment_task_ids(bad_source)
    assert len(findings) == 3
    by_path = {f.path: f for f in findings}
    assert by_path["bad.go"].ids == [bad_prefix]
    assert by_path["bad.go"].line == 2
    assert by_path["bad.rs"].ids == [chr(66) + "-02"]
    assert by_path["bad.rs"].line == 1
    assert by_path["bad.py"].ids == [chr(67) + "-03"]
    assert by_path["bad.py"].line == 1

    string_literals = [
        Source(path="strings.go", data=f"package sample\nconst task = `{'A'}-01 // {'B'}-02`\n".encode("utf-8")),
        Source(
            path="strings.rs",
            data=f'const A: &str = "{chr(67)}-03 // {chr(68)}-04";\nconst B: &str = r#"{chr(69)}-05 /* {chr(70)}-06 */"#;\n'.encode("utf-8"),
        ),
        Source(path="strings.py", data=f'task = "{chr(65)}-01 # {chr(66)}-02"\n'.encode("utf-8")),
    ]
    assert len(scan_comment_task_ids(string_literals)) == 0

    ordinary_comments = [
        Source(path="ordinary.go", data=b"package sample\n// Use SHA-256 for digest.\n"),
        Source(path="ordinary.rs", data=b"/* Use CRC-32C for checksum. */\nfn main() {}\n"),
        Source(path="ordinary.py", data=b"# Verification using SHA-256 and CRC-32C.\n"),
    ]
    assert len(scan_comment_task_ids(ordinary_comments)) == 0


def test_collect_first_party_sources_skips_declared_trees(tmp_path: Path) -> None:
    files = {
        "packages/core/main.go": "package core\n",
        "packages/core/testdata/localized.ts": "const text = 'text';\n",
        "scripts/check.mjs": "export const ok = true;\n",
        "apps/game/main.gd": "extends Node\n",
        "apps/mornlea-godot/addons/py4godot/copied.gd": "# Upstream runtime.\n",
        "apps/mornlea-godot/addons/py4godot/xterm.js": "`unclosed third-party terminfo",
        "apps/mornlea-godot/addons/mornlea_bridge/owned.gd": "# First-party bridge.\n",
        "apps/mornlea-godot/addons/mornlea_tools/controller.gd": "# Project-owned plugin.\nextends Node\n",
        "apps/game/addons/py4godot/local.gd": "# Another app owns this source.\nextends Node\n",
        "packages/core/vendor/copied.go": "package copied\n",
        "packages/core/generated/schema.go": "package generated\n",
        "packages/engine/target/debug/build.rs": "fn main() {}\n",
        "packages/web/node_modules/library/index.ts": "export {};\n",
        "packages/web/dist/index.js": "void 0;\n",
        "packages/core/licenses/copied_license.c": "int copied;\n",
        "apps/game/.godot/imported/cache.gd": "extends Node\n",
        "outside/not_first_party.go": "package outside\n",
    }
    for rel_path, content in files.items():
        dest = tmp_path / rel_path
        dest.parent.mkdir(parents=True, exist_ok=True)
        dest.write_text(content, encoding="utf-8")

    sources = collect_first_party_sources(tmp_path)
    collected = [s.path for s in sources]
    expected = [
        "apps/game/addons/py4godot/local.gd",
        "apps/game/main.gd",
        "apps/mornlea-godot/addons/mornlea_bridge/owned.gd",
        "apps/mornlea-godot/addons/mornlea_tools/controller.gd",
        "packages/core/main.go",
        "packages/core/testdata/localized.ts",
        "scripts/check.mjs",
    ]
    assert collected == expected


def test_repo_scan_first_party_sources(repo_root: Path) -> None:
    sources = collect_first_party_sources(repo_root)
    assert len(sources) > 50

    covered_languages: set[str] = set()
    for s in sources:
        lang = comment_language_for_path(s.path)
        assert lang is not None, f"Unsupported source returned: {s.path}"
        covered_languages.add(lang)
        # Verify tokenization succeeds without error on all sources
        extract_comments(s.data, lang)

    for req_lang in ["go", "rust", "c", "javascript", "typescript", "gdscript", "python"]:
        assert req_lang in covered_languages, f"Missing language coverage in repo: {req_lang}"

    # Repository should pass task ID scan
    findings = scan_comment_task_ids(repo_root)
    assert len(findings) == 0, f"Task IDs found in repository comments: {findings}"
