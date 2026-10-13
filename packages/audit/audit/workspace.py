"""Go workspace parsing and module discovery utilities."""

from pathlib import Path

from audit.common import get_repo_root


def parse_go_work_modules(content: str, repo_root: Path) -> list[str]:
    """Parse module paths from go.work content, returning normalized relative paths.

    Raises RuntimeError if the use list is empty.
    """
    modules: list[str] = []
    in_use_block = False

    for raw_line in content.splitlines():
        line = raw_line.split("//")[0].strip()
        if not line:
            continue

        if line.startswith("use (") and line.endswith(")"):
            inner = line[len("use (") : -1].strip()
            if inner:
                for token in inner.split():
                    clean_path = token.strip('"\'')
                    if clean_path:
                        modules.append(clean_path)
            continue

        if line == "use (":
            in_use_block = True
            continue
        if in_use_block and line == ")":
            in_use_block = False
            continue

        if in_use_block:
            for token in line.split():
                clean_path = token.strip('"\'')
                if clean_path:
                    modules.append(clean_path)
        elif line.startswith("use "):
            parts = line.split(maxsplit=1)
            if len(parts) == 2 and parts[1] != "(":
                token = parts[1].strip().strip('"\'')
                if token and token != "()":
                    modules.append(token)

    if not modules:
        raise RuntimeError("go.work use list is empty: repository must manage Go unit modules")

    normalized_modules: list[str] = []
    resolved_root = repo_root.resolve()
    for mod_path in modules:
        p = Path(mod_path)
        if not p.is_absolute():
            resolved = (repo_root / p).resolve()
        else:
            resolved = p.resolve()

        try:
            rel = resolved.relative_to(resolved_root).as_posix()
        except ValueError:
            rel = p.as_posix()
        normalized_modules.append(rel)

    return sorted(list(dict.fromkeys(normalized_modules)))


def get_workspace_modules(repo_root: Path | None = None) -> list[str]:
    """Return sorted, deduplicated relative paths of Go modules listed in go.work."""
    if repo_root is None:
        repo_root = get_repo_root()
    go_work_path = repo_root / "go.work"
    if not go_work_path.is_file():
        raise RuntimeError(f"go.work not found at {go_work_path}")
    content = go_work_path.read_text(encoding="utf-8")
    return parse_go_work_modules(content, repo_root)
