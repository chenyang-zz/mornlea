"""Shared test fixtures for Mornlea architectural audit tests."""

from pathlib import Path
import pytest

from audit.common import get_repo_root
from audit.workspace import get_workspace_modules


@pytest.fixture(scope="session")
def repo_root() -> Path:
    """Fixture providing repository root path."""
    return get_repo_root()


@pytest.fixture(scope="session")
def workspace_modules() -> list[str]:
    """Fixture providing workspace Go module relative paths."""
    return get_workspace_modules()
