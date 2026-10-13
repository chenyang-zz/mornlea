# Mornlea Audit Subsystem Python Migration and Documentation Alignment Design

Date: 2026-10-13
Status: Approved in conversation, ready for implementation planning

## 1. Context and Problem Statement

`packages/audit` (historically Go package `archcheck`) has served as the cross-module architectural gatekeeper for Mornlea. It validates internal package dependency directions, unit module `require` boundaries, source guards (e.g. WebGPU import prohibitions, message immutability, tick budget), baseline versions, identity standards, multilingual comment language rules, CI workflow policies, Godot runtime pins, and documentation manifests.

However, implementing the audit gate as a Go module (`github.com/channing771/mornlea/packages/audit`) introduced significant structural awkwardness:
1. It forced `go.work` to coordinate a 6th Go module whose sole function is observing and auditing the other 5 units (`packages/contracts`, `packages/shared`, `packages/server`, `packages/client`, `packages/tools`).
2. The Go audit tests relied on executing `go list -deps` and subprocess scans, while also being included in all `go vet`, `go test -short`, and `test-race` loops across modules.
3. In line with the target architecture direction defined in `docs/architecture-target.md`, real-time game logic moves to Rust and Godot presentation to Python. Maintaining an independent Python audit test suite aligns with the project's scripting and verification strategy while removing the artificial 6th Go module from `go.work`.
4. Numerous documents and guides across the repository describe `go.work` as coordinating "six modules" and reference `go test ./packages/audit -count=1`. Migrating the audit code to Python requires comprehensively resolving all contradictory documentation repository-wide.

## 2. Goals and Success Criteria

1. **Python Audit Suite**: Migrate all architectural, dependency, identity, source guard, comment, CI workflow, Godot, and documentation tests from Go to a clean, idiomatic Python package under `packages/audit/` managed by `uv` and tested with `pytest`.
2. **Go Workspace Streamlining**: Remove `packages/audit` from `go.work`, deleting `packages/audit/go.mod`, `packages/audit/go.sum`, and all legacy Go source files in `packages/audit/`. `go.work` coordinates exactly 5 Go modules.
3. **Toolchain and CI Integration**: Update `Makefile` (`archcheck`, `comment-language-check`, `GO_TEST_MODULES`, `ci-preflight`, `dev-check`), `scripts/ci/doctor.sh`, `scripts/ci/package-inventory.sh`, `scripts/agents/gates.sh`, `scripts/agents/race-changed.sh`, `.github/workflows/ci.yml`, and `.github/workflows/godot.yml`.
4. **Documentation Synchronization**: Eliminate all global contradictions regarding 6 Go modules and Go audit invocations. Update root `AGENTS.md`, `packages/audit/AGENTS.md`, `openspec/config.yaml`, `openspec/specs/repository-code-organization/spec.md`, all subdirectory `AGENTS.md` guides, and related docs.
5. **Exact Behavioral Parity**: All architectural invariants and boundary assertions previously guarded by Go tests must pass under `uv run pytest packages/audit`.

## 3. Non-Goals

1. No modification to the runtime behavior, networking protocols, save formats, or game simulation of Mornlea.
2. No modification to the production Go packages (`shared`, `server`, `client`, `contracts`, `tools`) or Rust engines.
3. No introduction of heavy external Python dependencies (the audit package stays minimal with `pytest` and `pyyaml`).

## 4. Architectural Design

### 4.1 Subsystem Layout under `packages/audit/`

```text
packages/audit/
├── pyproject.toml         # Declares project metadata (mornlea-audit) & dependencies (pytest, pyyaml)
├── AGENTS.md              # Scoped guide for audit test development and maintenance
├── audit/                 # Core audit library modules
│   ├── __init__.py
│   ├── common.py          # Repository root resolution, subprocess runner, file utilities
│   ├── workspace.py       # go.work parser, module enumerator (5 Go modules)
│   ├── dependencies.py    # Go dependency graph analysis and allowed whitelist verification
│   ├── boundaries.py      # go.mod require graph verification & module separation
│   ├── comments.py        # Multilingual comment scanner (Go, Rust, C, GDScript, Python, etc.)
│   └── documentation.py   # Markdown manifest, doc pair sync, and relative link validator
└── tests/                 # pytest test suite
    ├── conftest.py        # Shared fixtures (repo_root, workspace_modules, etc.)
    ├── test_dependency.py # Package-level one-way dependency directions
    ├── test_unit_boundary.py # Module go.mod require boundaries
    ├── test_identity_and_baseline.py # Identity markers & baseline version matrix
    ├── test_source_guards.py # WebGPU import ban, immutability, loop/I/O limits
    ├── test_comments.py   # English comment policy and task ID exclusions
    ├── test_ci_workflows.py # GitHub workflows & entrypoint script compliance
    ├── test_documentation.py # Documentation pairs, manifest, and link validity
    ├── test_godot.py      # Godot closure, release unit, tooling, and isolation tests
    ├── test_companion_agent.py # Companion agent HTTP/MCP contracts
    └── test_sim_authority.py # Authoritative sim command stream rules
```

### 4.2 Core Audit Logic and Domain Mapping

1. **Dependency and Boundary Audits (`dependencies.py`, `boundaries.py`)**:
   - `audit/dependencies.py` encapsulates the `allowed` dependency graph mapping for all packages. It invokes `go list -json -deps` to inspect packages and enforces acyclic, one-way dependencies.
   - `sim` and `storage` subpackage layers are explicitly checked against their layered dependency rules.
   - `audit/boundaries.py` parses `go.mod` files of the 5 Go modules and enforces that modules only require permitted sibling modules.

2. **Source and Sim Authority Guards (`test_source_guards.py`, `test_sim_authority.py`)**:
   - Ensures no Go code imports WebGPU bindings.
   - Inspects cross-goroutine message immutability rules and non-blocking authoritative tick loops.
   - Asserts simulation state authority remains on the server.

3. **Identity and Baseline Matrix (`test_identity_and_baseline.py`)**:
   - Verifies project name `Mornlea` and absence of Mojang/Minecraft branding.
   - Enforces the baseline version matrix across protocol v45, player v9, chunk v9, world v6, companion v5, hostile v2, passive v1, engine ABI v11, client ABI v19, and benchmark scenario v23.

4. **Multilingual Comment Scanner (`comments.py`, `test_comments.py`)**:
   - Scans first-party code across `apps/`, `packages/`, `scripts/` in Go, Rust, C, Python, GDScript, and JavaScript/TypeScript.
   - Excludes third-party directories (`apps/mornlea-godot/addons/py4godot`, `.venv`, `node_modules`, `vendor`, `third-party`, `target`).
   - Asserts all new/modified comments are in English and contain no OpenSpec task identifiers.

5. **Documentation and CI Validation (`documentation.py`, `test_documentation.py`, `test_ci_workflows.py`)**:
   - Validates that markdown files match `docs/manifest.json`.
   - Validates synchronized English and Chinese doc pairs (`*.md` and `*.zh.md`).
   - Audits `.github/workflows/*.yml` for pinned actions, permissions, and required gate coverage.

6. **Godot and Companion Agent Rules (`test_godot.py`, `test_companion_agent.py`)**:
   - Re-implements all 18 Godot platform checks (project closure, export closure, python toolchain pin, desktop-only constraints, etc.).
   - Validates companion agent contract definitions.

### 4.3 Workspace and Build Integration

1. **`go.work`**:
   - Removes `./packages/audit`. The Go workspace now coordinates 5 units:
     `packages/contracts`, `packages/shared`, `packages/server`, `packages/client`, `packages/tools`.
2. **`Makefile`**:
   - `GO_TEST_MODULES` set to the 5 Go units.
   - `archcheck` targets `uv run pytest packages/audit` plus the server headless check.
   - `comment-language-check` targets `uv run pytest packages/audit/tests/test_comments.py`.
   - `ci-preflight` and `dev-check` updated to invoke the new Python audit targets.
3. **Scripts and Workflows**:
   - `scripts/ci/doctor.sh`: updates `audit)` profile to require `bash`, `python3`, `uv`, and `rg`.
   - `scripts/ci/package-inventory.sh`: updates expected modules list to 5 Go modules.
   - `scripts/agents/gates.sh` & `race-changed.sh`: updates module iterations to 5 modules, runs pytest for archcheck.
   - `.github/workflows/ci.yml` & `godot.yml`: cleans up `packages/audit/go.sum` references, updates trigger paths.

### 4.4 Documentation Synchronization

1. **Root `AGENTS.md`**:
   - Clarifies that `go.work` coordinates five Go modules, while `packages/audit` operates as an independent Python unit.
   - Updates focused validation command to `make archcheck` or `uv run pytest packages/audit`.
2. **`packages/audit/AGENTS.md`**:
   - Completely rewritten to document the Python audit architecture, runner requirements, and leaf boundary rules.
3. **`openspec/config.yaml`**:
   - Updates project context to reflect 5 Go modules and the Python audit system.
   - Updates validation gates and task requirements.
4. **Subdirectory `AGENTS.md` files**:
   - Replaces `go test ./packages/audit -count=1` with `make archcheck` across server, client, shared, tools, and scripts guides.
   - Updates references to `packages/audit/dependency_test.go` to point to `packages/audit/audit/dependencies.py`.
5. **Guidance and Quickstart Docs**:
   - Synchronizes `docs/notes/test-quickstart.md`, `docs/test-organization.md`, and `docs/architecture.md`.

## 5. Verification Plan

1. **Audit Test Execution**:
   - Run `uv run pytest packages/audit -v` to confirm all test suites pass.
2. **Makefile and Gate Scripts**:
   - Run `make archcheck`.
   - Run `make comment-language-check`.
   - Run `make dev-check`.
3. **Go Workspace Integrity**:
   - Run `go work check` and `for m in $(go list -m); do echo $m; done` to confirm 5 clean Go modules.
   - Verify `go test ./...` in each of the 5 modules.
4. **CI Preflight**:
   - Run `make ci-preflight`.
