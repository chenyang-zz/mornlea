"""Common repository discovery and subprocess execution utilities for Mornlea audit."""

import os
from pathlib import Path
import subprocess


def get_repo_root() -> Path:
    """Locate the repository root directory containing go.work.

    Fails closed if GOWORK=off or if go.work cannot be located in any parent directory.
    """
    if os.environ.get("GOWORK") == "off":
        raise RuntimeError("GOWORK is set to 'off': audit requires go.work workspace")

    # Traverse upward from current file's location
    for parent in [Path(__file__).resolve().parent, *Path(__file__).resolve().parents]:
        if (parent / "go.work").is_file():
            return parent

    # Traverse upward from current working directory as fallback
    cwd = Path.cwd().resolve()
    for parent in [cwd, *cwd.parents]:
        if (parent / "go.work").is_file():
            return parent

    raise RuntimeError("Could not find repository root containing go.work")


def run_cmd(
    cmd: list[str],
    cwd: Path | None = None,
    *,
    capture_output: bool = True,
    text: bool = True,
    check: bool = False,
    env: dict[str, str] | None = None,
    timeout: float | None = None,
) -> subprocess.CompletedProcess:
    """Execute a subprocess command and return CompletedProcess result."""
    return subprocess.run(
        cmd,
        cwd=cwd,
        capture_output=capture_output,
        text=text,
        check=check,
        env=env,
        timeout=timeout,
    )
