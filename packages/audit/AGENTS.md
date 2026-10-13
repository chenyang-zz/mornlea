# Mornlea Audit Package Guide (`packages/audit`)

## Purpose

This document supplements the root `AGENTS.md` for `packages/audit`. `packages/audit` is a standalone Python package (`mornlea-audit`) that provides repository-wide cross-module architectural, governance, and documentation gates executed via `pytest`.

## Directory Map

- `audit/`: Core verification logic and static scanners.
  - `audit/dependencies.py`: Direct and transitive package import allowlists and one-way dependency rules.
  - `audit/boundaries.py`: Module require boundary enforcement across `packages/*/go.mod` and server/client source isolation guards.
  - `audit/documentation.py`: Documentation manifest classification, bilingual pair synchronization, markdown links, and version baseline validation.
  - `audit/comments.py`: English prose and comment language policy enforcement.
  - `audit/workspace.py`: Root `go.work` module membership and alignment checks.
  - `audit/common.py`: Shared repository path resolution and execution helpers.
- `tests/`: Test suites corresponding to each architectural gate.
- `pyproject.toml`: Package definition and tool configurations for `pytest`.

## Ownership and Boundary

- The audit package is an external, read-only observer across repository modules. It inspects units via `go list`, file system traversal, regex parsing, and AST scanners.
- It MUST NOT import Go packages, runtime internals, or depend on game simulation state.
- Allowed package dependency directions are authoritatively defined in `audit/dependencies.py`.
- Module require directions are authoritatively defined in `audit/boundaries.py`.
- The comment collector excludes declared third-party installations such as `apps/mornlea-godot/addons/py4godot`. Project-owned addons and directories elsewhere remain audited.
- Production sources in `packages/tools/cmd/runtime-oracle` stay stdlib-only; test files may import `packages/server/storage/storagedef` for save error-class checks as whitelisted in `audit/dependencies.py` (`ORACLE_ALLOWED_TEST_IMPORTS`).

## Focused Validation

- Complete architectural and governance test suite: `make archcheck` or `uv run pytest packages/audit`.
- Focused comment language check: `make comment-language-check` or `uv run pytest packages/audit/tests/test_comments.py`.
