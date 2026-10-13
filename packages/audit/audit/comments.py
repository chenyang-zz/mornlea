"""Multilingual comment extraction, language auditing, and task identifier scanner."""

from dataclasses import dataclass, field
import os
from pathlib import Path
import re

from audit.common import get_repo_root

FIRST_PARTY_SOURCE_ROOTS = ("apps", "packages", "scripts")

EXCLUDED_DIRECTORIES = {
    ".godot",
    ".venv",
    "dist",
    "generated",
    "licenses",
    "node_modules",
    "target",
    "third-party",
    "third_party",
    "vendor",
}

TASK_ID_PATTERN = re.compile(r"(?<![A-Za-z0-9])[A-F]-[0-9]{2}(?![A-Za-z0-9])")

JS_REGEX_KEYWORDS = {
    b"await",
    b"case",
    b"delete",
    b"in",
    b"instanceof",
    b"new",
    b"of",
    b"return",
    b"throw",
    b"typeof",
    b"void",
    b"yield",
}


@dataclass(frozen=True)
class CommentToken:
    """Represents an extracted comment token from a source file."""

    text: str
    line: int
    offset: int


@dataclass(frozen=True)
class Source:
    """Represents a source file path and its raw byte payload."""

    path: str
    data: bytes


@dataclass(frozen=True)
class Finding:
    """Represents a scan finding with path, line, text, and optional matched identifiers."""

    path: str
    line: int
    text: str = ""
    ids: list[str] = field(default_factory=list)


def is_han(char: str) -> bool:
    """Check if a Unicode character belongs to the Han script."""
    cp = ord(char)
    return (
        0x4E00 <= cp <= 0x9FFF
        or 0x3400 <= cp <= 0x4DBF
        or 0x20000 <= cp <= 0x2A6DF
        or 0x2A700 <= cp <= 0x2B73F
        or 0x2B740 <= cp <= 0x2B81F
        or 0x2B820 <= cp <= 0x2CEAF
        or 0x2CEB0 <= cp <= 0x2EBEF
        or 0x30000 <= cp <= 0x3134F
        or 0x31350 <= cp <= 0x323AF
        or 0xF900 <= cp <= 0xFAFF
        or 0x2F800 <= cp <= 0x2FA1F
        or 0x2E80 <= cp <= 0x2EFF
        or 0x2F00 <= cp <= 0x2FDF
        or cp == 0x3005
        or 0x3190 <= cp <= 0x319F
    )


def contains_han(text: str | bytes) -> bool:
    """Check if string or byte sequence contains any Han characters."""
    if isinstance(text, bytes):
        text = text.decode("utf-8", errors="replace")
    return any(is_han(ch) for ch in text)


def is_copied_license_file(name: str) -> bool:
    """Check if file name corresponds to copied third-party license text."""
    lower = name.lower()
    return lower in ("license", "copying") or lower.startswith(("license.", "copying."))


def comment_language_for_path(path: str | Path) -> str | None:
    """Return the normalized canonical comment language name for a file path."""
    name = str(path).lower()
    suffix = Path(name).suffix
    if suffix == ".go":
        return "go"
    elif suffix == ".rs":
        return "rust"
    elif suffix in (".c", ".h"):
        return "c"
    elif suffix == ".py":
        return "python"
    elif suffix == ".gd":
        return "gdscript"
    elif suffix in (".js", ".jsx", ".mjs", ".cjs"):
        return "javascript"
    elif suffix in (".ts", ".tsx", ".mts", ".cts"):
        return "typescript"
    return None


def collect_first_party_sources(root: Path) -> list[Source]:
    """Walk first-party source roots, filtering excluded trees and license files."""
    sources: list[Source] = []
    for source_root in FIRST_PARTY_SOURCE_ROOTS:
        abs_root = root / source_root
        if not abs_root.exists():
            continue
        for current_dir, dirnames, filenames in os.walk(abs_root):
            rel_dir = os.path.relpath(current_dir, root).replace("\\", "/")
            filtered_dirs = []
            for d in dirnames:
                sub_rel = f"{rel_dir}/{d}" if rel_dir != "." else d
                if sub_rel == "apps/mornlea-godot/addons/py4godot":
                    continue
                if d in EXCLUDED_DIRECTORIES:
                    continue
                filtered_dirs.append(d)
            dirnames[:] = filtered_dirs

            for f in filenames:
                if is_copied_license_file(f):
                    continue
                file_rel = f"{rel_dir}/{f}" if rel_dir != "." else f
                if comment_language_for_path(file_rel) is None:
                    continue
                file_path = Path(current_dir) / f
                try:
                    data = file_path.read_bytes()
                except OSError:
                    continue
                sources.append(Source(path=file_rel, data=data))

    sources.sort(key=lambda s: s.path)
    return sources


def quoted_literal_end(source: bytes, start: int, quote: int) -> int:
    """Find the exclusive end offset of a quoted literal with backslash escapes."""
    idx = start + 1
    n = len(source)
    while idx < n:
        b = source[idx]
        if b == 0x5C:
            idx += 2
        elif b == quote:
            return idx + 1
        else:
            idx += 1
    raise ValueError(f"unterminated quoted literal at offset {start}")


def has_odd_backslash_run(source: bytes, end: int) -> bool:
    """Determine whether end index is preceded by an odd number of backslashes."""
    count = 0
    idx = end - 1
    while idx >= 0 and source[idx] == 0x5C:
        count += 1
        idx -= 1
    return count % 2 == 1


def line_comment_end(source: bytes, start: int, splice_line: bool) -> int:
    """Find the exclusive end offset of a slash-slash line comment."""
    idx = start + 2
    n = len(source)
    while idx < n:
        b = source[idx]
        if b not in (0x0A, 0x0D):
            idx += 1
            continue
        if splice_line and has_odd_backslash_run(source, idx):
            if b == 0x0D and idx + 1 < n and source[idx + 1] == 0x0A:
                idx += 2
            else:
                idx += 1
            continue
        return idx
    return n


def hash_comment_end(source: bytes, start: int) -> int:
    """Find the exclusive end offset of a hash line comment."""
    idx = start + 1
    n = len(source)
    while idx < n:
        if source[idx] in (0x0A, 0x0D):
            return idx
        idx += 1
    return n


def block_comment_end(source: bytes, start: int, nested: bool) -> int:
    """Find the exclusive end offset of a block comment."""
    depth = 1
    idx = start + 2
    n = len(source)
    while idx + 1 < n:
        pair = source[idx : idx + 2]
        if nested and pair == b"/*":
            depth += 1
            idx += 2
            continue
        if pair == b"*/":
            depth -= 1
            idx += 2
            if depth == 0:
                return idx
            continue
        idx += 1
    raise ValueError(f"unterminated block comment at offset {start}")


def slash_comment_at(
    source: bytes, start: int, nested_block: bool, splice_line: bool
) -> tuple[CommentToken | None, int]:
    """Inspect offset for line or block comment."""
    if start + 1 >= len(source) or source[start] != 0x2F:
        return None, start
    next_b = source[start + 1]
    if next_b == 0x2F:
        end = line_comment_end(source, start, splice_line)
        line = source[:start].count(b"\n") + 1
        text = source[start:end].decode("utf-8", errors="replace")
        return CommentToken(text=text, line=line, offset=start), end
    elif next_b == 0x2A:
        end = block_comment_end(source, start, nested_block)
        line = source[:start].count(b"\n") + 1
        text = source[start:end].decode("utf-8", errors="replace")
        return CommentToken(text=text, line=line, offset=start), end
    return None, start


def go_comment_tokens(source: bytes) -> list[CommentToken]:
    """Extract comments from Go source code."""
    comments: list[CommentToken] = []
    idx = 0
    n = len(source)
    while idx < n:
        b = source[idx]
        if b == 0x22:
            idx = quoted_literal_end(source, idx, 0x22)
        elif b == 0x60:
            end = source.find(b"`", idx + 1)
            if end == -1:
                raise ValueError(f"unterminated raw string literal at offset {idx}")
            idx = end + 1
        elif b == 0x27:
            idx = quoted_literal_end(source, idx, 0x27)
        elif b == 0x2F:
            tok, end = slash_comment_at(source, idx, nested_block=False, splice_line=False)
            if tok is not None:
                comments.append(tok)
                idx = end
            else:
                idx += 1
        else:
            idx += 1
    return comments


def rust_raw_literal_end(source: bytes, start: int) -> tuple[int, bool]:
    """Scan raw string literal in Rust source."""
    idx = start
    n = len(source)
    if idx < n and source[idx] in (0x62, 0x63):
        idx += 1
    if idx >= n or source[idx] != 0x72:
        return 0, False
    idx += 1
    hashes = 0
    while idx < n and source[idx] == 0x23:
        hashes += 1
        idx += 1
    if idx >= n or source[idx] != 0x22:
        return 0, False
    idx += 1
    target = b'"' + (b"#" * hashes)
    end = source.find(target, idx)
    if end == -1:
        raise ValueError(f"unterminated Rust raw string at offset {start}")
    return end + len(target), True


def rust_character_literal_end(source: bytes, start: int) -> tuple[int, bool]:
    """Scan character literal in Rust source, skipping lifetime references."""
    idx = start + 1
    n = len(source)
    if idx >= n:
        return 0, False
    if source[idx] == 0x5C:
        idx += 1
        if idx >= n:
            return 0, False
        esc = source[idx]
        if esc == 0x78:
            idx += 3
        elif esc == 0x75:
            idx += 2
            while idx < n and source[idx] != 0x7D:
                idx += 1
            idx += 1
        else:
            idx += 1
    else:
        first = source[idx]
        if first < 0x80:
            idx += 1
        elif (first & 0xE0) == 0xC0:
            idx += 2
        elif (first & 0xF0) == 0xE0:
            idx += 3
        elif (first & 0xF8) == 0xF0:
            idx += 4
        else:
            idx += 1
    if idx < n and source[idx] == 0x27:
        return idx + 1, True
    return 0, False


def rust_comment_tokens(source: bytes) -> list[CommentToken]:
    """Extract comments from Rust source code."""
    comments: list[CommentToken] = []
    idx = 0
    n = len(source)
    while idx < n:
        end, ok = rust_raw_literal_end(source, idx)
        if ok:
            idx = end
            continue
        if source[idx] in (0x62, 0x63):
            if idx + 1 < n and source[idx + 1] == 0x22:
                idx = quoted_literal_end(source, idx + 1, 0x22)
                continue
            if source[idx] == 0x62 and idx + 1 < n and source[idx + 1] == 0x27:
                end, ok = rust_character_literal_end(source, idx + 1)
                if ok:
                    idx = end
                    continue
        b = source[idx]
        if b == 0x22:
            idx = quoted_literal_end(source, idx, 0x22)
        elif b == 0x27:
            end, ok = rust_character_literal_end(source, idx)
            if ok:
                idx = end
            else:
                idx += 1
        elif b == 0x2F:
            tok, end = slash_comment_at(source, idx, nested_block=True, splice_line=False)
            if tok is not None:
                comments.append(tok)
                idx = end
            else:
                idx += 1
        else:
            idx += 1
    return comments


def c_comment_tokens(source: bytes) -> list[CommentToken]:
    """Extract comments from C and header source code."""
    comments: list[CommentToken] = []
    idx = 0
    n = len(source)
    while idx < n:
        b = source[idx]
        if b in (0x22, 0x27):
            idx = quoted_literal_end(source, idx, b)
        elif b == 0x2F:
            tok, end = slash_comment_at(source, idx, nested_block=False, splice_line=True)
            if tok is not None:
                comments.append(tok)
                idx = end
            else:
                idx += 1
        else:
            idx += 1
    return comments


def gdscript_string_end(source: bytes, start: int, raw: bool) -> int:
    """Find the exclusive end offset of a GDScript string literal."""
    quote = source[start]
    triple = (
        start + 2 < len(source)
        and source[start + 1] == quote
        and source[start + 2] == quote
    )
    if not triple:
        if raw:
            idx = start + 1
            while idx < len(source):
                if source[idx] == quote:
                    return idx + 1
                idx += 1
            raise ValueError(f"unterminated raw GDScript string at offset {start}")
        return quoted_literal_end(source, start, quote)

    idx = start + 3
    while idx + 2 < len(source):
        if not raw and source[idx] == 0x5C:
            idx += 2
            continue
        if source[idx] == quote and source[idx + 1] == quote and source[idx + 2] == quote:
            return idx + 3
        idx += 1
    raise ValueError(f"unterminated triple-quoted GDScript string at offset {start}")


def gdscript_comment_tokens(source: bytes) -> list[CommentToken]:
    """Extract comments from GDScript source code."""
    comments: list[CommentToken] = []
    idx = 0
    n = len(source)
    while idx < n:
        if source[idx] == 0x72 and idx + 1 < n and source[idx + 1] in (0x22, 0x27):
            idx = gdscript_string_end(source, idx + 1, raw=True)
            continue
        b = source[idx]
        if b in (0x22, 0x27):
            idx = gdscript_string_end(source, idx, raw=False)
        elif b == 0x23:
            end = hash_comment_end(source, idx)
            line = source[:idx].count(b"\n") + 1
            text = source[idx:end].decode("utf-8", errors="replace")
            comments.append(CommentToken(text=text, line=line, offset=idx))
            idx = end
        else:
            idx += 1
    return comments


def python_string_end(source: bytes, start: int, raw: bool) -> int:
    """Find the exclusive end offset of a Python string literal."""
    quote = source[start]
    triple = (
        start + 2 < len(source)
        and source[start + 1] == quote
        and source[start + 2] == quote
    )
    if not triple:
        if raw:
            idx = start + 1
            while idx < len(source):
                if source[idx] == 0x5C:
                    idx += 2
                    continue
                if source[idx] == quote:
                    return idx + 1
                idx += 1
            raise ValueError(f"unterminated raw string literal at offset {start}")
        return quoted_literal_end(source, start, quote)

    idx = start + 3
    while idx + 2 < len(source):
        if source[idx] == 0x5C:
            idx += 2
            continue
        if source[idx] == quote and source[idx + 1] == quote and source[idx + 2] == quote:
            return idx + 3
        idx += 1
    raise ValueError(f"unterminated triple-quoted string literal at offset {start}")


def python_comment_tokens(source: bytes) -> list[CommentToken]:
    """Extract comments from Python source code."""
    comments: list[CommentToken] = []
    idx = 0
    n = len(source)
    while idx < n:
        if (
            source[idx : idx + 2].lower() in (b"rf", b"fr", b"rb", b"br")
            and idx + 2 < n
            and source[idx + 2] in (0x22, 0x27)
        ):
            raw = b"r" in source[idx : idx + 2].lower()
            idx = python_string_end(source, idx + 2, raw=raw)
            continue
        if (
            source[idx : idx + 1].lower() in (b"r", b"u", b"f", b"b")
            and idx + 1 < n
            and source[idx + 1] in (0x22, 0x27)
        ):
            raw = source[idx : idx + 1].lower() == b"r"
            idx = python_string_end(source, idx + 1, raw=raw)
            continue
        b = source[idx]
        if b in (0x22, 0x27):
            idx = python_string_end(source, idx, raw=False)
        elif b == 0x23:
            end = hash_comment_end(source, idx)
            line = source[:idx].count(b"\n") + 1
            text = source[idx:end].decode("utf-8", errors="replace")
            comments.append(CommentToken(text=text, line=line, offset=idx))
            idx = end
        else:
            idx += 1
    return comments


def is_ascii_whitespace(b: int) -> bool:
    """Check if byte is ASCII whitespace."""
    return b in (0x20, 0x09, 0x0A, 0x0D, 0x0C)


def is_js_identifier_part(b: int) -> bool:
    """Check if byte is part of a JavaScript identifier."""
    return (
        0x61 <= b <= 0x7A
        or 0x41 <= b <= 0x5A
        or 0x30 <= b <= 0x39
        or b in (0x5F, 0x24)
    )


def javascript_regex_can_start(source: bytes, slash: int) -> bool:
    """Inspect previous token to determine if slash begins a regex literal."""
    idx = slash - 1
    while idx >= 0 and is_ascii_whitespace(source[idx]):
        idx -= 1
    if idx < 0:
        return True
    if source[idx] in b"([{=,:;!?&|~%^>*+-":
        return True
    if not is_js_identifier_part(source[idx]):
        return False
    end = idx + 1
    while idx >= 0 and is_js_identifier_part(source[idx]):
        idx -= 1
    word = source[idx + 1 : end]
    return word in JS_REGEX_KEYWORDS


def javascript_regex_end(source: bytes, start: int) -> int:
    """Find the exclusive end offset of a JavaScript regex literal."""
    in_class = False
    idx = start + 1
    n = len(source)
    while idx < n:
        b = source[idx]
        if b == 0x5C:
            idx += 2
        elif b == 0x5B:
            in_class = True
            idx += 1
        elif b == 0x5D:
            in_class = False
            idx += 1
        elif b == 0x2F:
            if in_class:
                idx += 1
                continue
            idx += 1
            while idx < n and is_js_identifier_part(source[idx]):
                idx += 1
            return idx
        elif b in (0x0A, 0x0D):
            raise ValueError(f"unterminated JavaScript regular expression at offset {start}")
        else:
            idx += 1
    raise ValueError(f"unterminated JavaScript regular expression at offset {start}")


def javascript_template_end(source: bytes, start: int) -> tuple[list[CommentToken], int]:
    """Scan template literal, extracting nested expression comments."""
    comments: list[CommentToken] = []
    idx = start + 1
    n = len(source)
    while idx < n:
        b = source[idx]
        if b == 0x5C:
            idx += 2
        elif b == 0x60:
            return comments, idx + 1
        elif b == 0x24:
            if idx + 1 < n and source[idx + 1] == 0x7B:
                nested, end = javascript_comments_in_range(source, idx + 2, stop_at_right_brace=True)
                comments.extend(nested)
                idx = end
                continue
            idx += 1
        else:
            idx += 1
    raise ValueError(f"unterminated template literal at offset {start}")


def javascript_comments_in_range(
    source: bytes, start: int, stop_at_right_brace: bool
) -> tuple[list[CommentToken], int]:
    """Scan JavaScript range, optionally stopping at balanced right brace."""
    comments: list[CommentToken] = []
    brace_depth = 0
    idx = start
    n = len(source)
    while idx < n:
        b = source[idx]
        if b in (0x22, 0x27):
            idx = quoted_literal_end(source, idx, b)
        elif b == 0x60:
            nested, end = javascript_template_end(source, idx)
            comments.extend(nested)
            idx = end
        elif b == 0x2F:
            tok, end = slash_comment_at(source, idx, nested_block=False, splice_line=False)
            if tok is not None:
                comments.append(tok)
                idx = end
                continue
            if javascript_regex_can_start(source, idx):
                idx = javascript_regex_end(source, idx)
                continue
            idx += 1
        elif b == 0x7B:
            if stop_at_right_brace:
                brace_depth += 1
            idx += 1
        elif b == 0x7D:
            if not stop_at_right_brace:
                idx += 1
                continue
            if brace_depth == 0:
                return comments, idx + 1
            brace_depth -= 1
            idx += 1
        else:
            idx += 1
    if stop_at_right_brace:
        raise ValueError(f"unterminated template expression at offset {start}")
    return comments, n


def javascript_comment_tokens(source: bytes) -> list[CommentToken]:
    """Extract comments from JavaScript and TypeScript source code."""
    comments, end = javascript_comments_in_range(source, 0, stop_at_right_brace=False)
    if end != len(source):
        raise ValueError(f"scanner stopped at offset {end}")
    return comments


def extract_comments(source_bytes: bytes, lang: str) -> list[CommentToken]:
    """Extract all comment tokens from source bytes for the given language."""
    norm = lang.lower()
    if norm in ("go", ".go"):
        return go_comment_tokens(source_bytes)
    elif norm in ("rust", "rs", ".rs"):
        return rust_comment_tokens(source_bytes)
    elif norm in ("c", "h", ".c", ".h"):
        return c_comment_tokens(source_bytes)
    elif norm in ("python", "py", ".py"):
        return python_comment_tokens(source_bytes)
    elif norm in ("gdscript", "gd", ".gd"):
        return gdscript_comment_tokens(source_bytes)
    elif norm in ("javascript", "js", ".js", ".jsx", ".mjs", ".cjs"):
        return javascript_comment_tokens(source_bytes)
    elif norm in ("typescript", "ts", ".ts", ".tsx", ".mts", ".cts"):
        return javascript_comment_tokens(source_bytes)
    raise ValueError(f"unsupported source language: {lang}")


def scan_comment_language(
    target: Path | list[Source] | None = None,
    *,
    sources: list[Source] | None = None,
    repo_root: Path | None = None,
) -> list[Finding]:
    """Scan comment tokens for non-English Han characters."""
    if sources is not None:
        srcs = sources
    elif isinstance(target, list):
        srcs = target
    else:
        root = target if isinstance(target, Path) else (repo_root or get_repo_root())
        srcs = collect_first_party_sources(root)

    sorted_sources = sorted(srcs, key=lambda s: s.path)
    findings: list[Finding] = []
    for source in sorted_sources:
        lang = comment_language_for_path(source.path)
        if lang is None:
            continue
        comments = extract_comments(source.data, lang)
        for comment in comments:
            start_line = source.data[:comment.offset].count(b"\n") + 1
            for line_offset, line in enumerate(comment.text.split("\n")):
                if not contains_han(line):
                    continue
                findings.append(
                    Finding(
                        path=source.path,
                        line=start_line + line_offset,
                        text=line.strip(),
                    )
                )
    return findings


def scan_comment_task_ids(
    target: Path | list[Source] | None = None,
    *,
    sources: list[Source] | None = None,
    repo_root: Path | None = None,
) -> list[Finding]:
    """Scan comment tokens for task identifiers."""
    if sources is not None:
        srcs = sources
    elif isinstance(target, list):
        srcs = target
    else:
        root = target if isinstance(target, Path) else (repo_root or get_repo_root())
        srcs = collect_first_party_sources(root)

    sorted_sources = sorted(srcs, key=lambda s: s.path)
    findings: list[Finding] = []
    for source in sorted_sources:
        lang = comment_language_for_path(source.path)
        if lang is None:
            continue
        comments = extract_comments(source.data, lang)
        for comment in comments:
            matches = TASK_ID_PATTERN.findall(comment.text)
            if not matches:
                continue
            line = source.data[:comment.offset].count(b"\n") + 1
            findings.append(
                Finding(
                    path=source.path,
                    line=line,
                    text=comment.text,
                    ids=matches,
                )
            )
    return findings
