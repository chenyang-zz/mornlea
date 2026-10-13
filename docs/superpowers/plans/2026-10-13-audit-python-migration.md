# Mornlea Audit Subsystem Python Migration Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Migrate the Mornlea audit subsystem (`packages/audit`) from Go (54 files, 18.9k lines) to a standalone Python package (`mornlea-audit`) using `pytest` and `uv`, remove `packages/audit` from `go.work` to leave 5 clean Go modules, integrate the new runner into `Makefile` and CI scripts, and resolve all contradictory documentation repository-wide.

**Architecture:** Python `packages/audit` structured into domain modules (`audit/` containing `common`, `workspace`, `dependencies`, `boundaries`, `comments`, `documentation`) and `pytest` suites (`tests/`). Go's `go.work` retains the 5 domain/production modules (`contracts`, `shared`, `server`, `client`, `tools`). Makefile and CI scripts invoke `uv run pytest packages/audit`. Global documentation is updated to eliminate outdated references to 6 Go modules and Go-based audit gates.

**Tech Stack:** Python 3.12, `uv`, `pytest`, `pyyaml`, Go 1.26 (for inspected modules and `go list`), Bash.

**Spec:** [`docs/superpowers/specs/2026-10-13-audit-python-migration-design.md`](docs/superpowers/specs/2026-10-13-audit-python-migration-design.md)

## Global Constraints

- Python audit dependencies remain minimal: `pytest>=8.0.0` and `pyyaml>=6.0.0`.
- All new first-party source comments and docstrings must be in English.
- No OpenSpec task IDs (such as `2.1` or `[A-F]-[0-9]{2}`) may appear in comments.
- Git commits must be one-line in `<type>(<scope>): <subject>` format without body or footer.
- The 5 Go modules (`contracts`, `shared`, `server`, `client`, `tools`) must continue to compile and pass tests cleanly.

## Review Focus

- `go list -deps` failure or timeout when querying packages: must fail-closed with clear diagnostic output.
- Missing `go.work` or `GOWORK=off`: must fail-closed rather than silently passing with empty results.
- Handling of third-party directories in comment scans: ensure vendored folders (e.g. `py4godot`) are skipped without skipping first-party code.
- Boundary verification across 5 Go modules: ensure `server` production imports of `client` remain strictly rejected while test imports in `server` have explicit exemptions.
- Discrepancy between documentation and code: ensure every `AGENTS.md` referencing `go test ./packages/audit` is updated to `make archcheck` or `uv run pytest packages/audit`.

---

### Task 1: Python Audit Package Scaffolding & Common Helpers

**Files:**
- Create: `packages/audit/pyproject.toml`
- Create: `packages/audit/audit/__init__.py`
- Create: `packages/audit/audit/common.py`
- Create: `packages/audit/audit/workspace.py`
- Create: `packages/audit/tests/__init__.py`
- Create: `packages/audit/tests/conftest.py`
- Create: `packages/audit/tests/test_workspace.py`

**Interfaces:**
- Produces:
  - `audit.common.get_repo_root() -> Path`
  - `audit.common.run_cmd(cmd: list[str], cwd: Path | None) -> subprocess.CompletedProcess`
  - `audit.workspace.get_workspace_modules() -> list[str]`
  - `conftest.py` fixtures: `repo_root`, `workspace_modules`

- [ ] **Step 1: Write the failing test for workspace discovery**
Create `packages/audit/tests/test_workspace.py` verifying `get_repo_root()` and `get_workspace_modules()` return valid paths and list the Go modules.

- [ ] **Step 2: Run test to verify it fails**
Run: `uv run pytest packages/audit/tests/test_workspace.py -v`
Expected: FAIL with missing module `audit`.

- [ ] **Step 3: Implement `pyproject.toml`, `common.py`, and `workspace.py`**
Implement `pyproject.toml` with `pytest` and `pyyaml`. Implement `get_repo_root()` locating the directory containing `go.work`. Implement `get_workspace_modules()` parsing `go.work` use statements.

- [ ] **Step 4: Run test to verify it passes**
Run: `uv run pytest packages/audit/tests/test_workspace.py -v`
Expected: PASS.

- [ ] **Step 5: Commit**
```bash
git add packages/audit/pyproject.toml packages/audit/audit packages/audit/tests
git commit -m "feat(audit): scaffold python audit package and workspace helpers"
```

---

### Task 2: Multilingual Comment & Task ID Scanner

**Files:**
- Create: `packages/audit/audit/comments.py`
- Create: `packages/audit/tests/test_comments.py`

**Interfaces:**
- Consumes: `audit.common.get_repo_root()`
- Produces:
  - `audit.comments.extract_comments(source_bytes: bytes, lang: str) -> list[CommentToken]`
  - `audit.comments.scan_comment_language(repo_root: Path) -> list[Finding]`
  - `audit.comments.scan_comment_task_ids(repo_root: Path) -> list[Finding]`

- [ ] **Step 1: Write failing tests for comment language and task ID scanner**
Create `packages/audit/tests/test_comments.py` with test cases testing comment token extraction across Go, Rust, C, Python, GDScript, JS/TS, non-English comment detection in first-party code, and OpenSpec task ID pattern detection.

- [ ] **Step 2: Run test to verify it fails**
Run: `uv run pytest packages/audit/tests/test_comments.py -v`
Expected: FAIL with `comments` module not defined.

- [ ] **Step 3: Implement `audit/comments.py`**
Port comment scanning logic from `comment_scanner_helpers_test.go`, `code_comment_language_test.go`, and `code_comment_task_id_test.go`. Handle exclusions for `.venv`, `node_modules`, `py4godot`, `target`, and `vendor`.

- [ ] **Step 4: Run test to verify it passes**
Run: `uv run pytest packages/audit/tests/test_comments.py -v`
Expected: PASS.

- [ ] **Step 5: Commit**
```bash
git add packages/audit/audit/comments.py packages/audit/tests/test_comments.py
git commit -m "feat(audit): implement multilingual comment scanner and tests"
```

---

### Task 3: Package Dependencies & Allowed Direction Audit

**Files:**
- Create: `packages/audit/audit/dependencies.py`
- Create: `packages/audit/tests/test_dependency.py`

**Interfaces:**
- Consumes: `audit.common.get_repo_root()`, `audit.workspace.get_workspace_modules()`
- Produces:
  - `audit.dependencies.ALLOWED_DEPENDENCIES: dict[str, set[str]]`
  - `audit.dependencies.SIM_ALLOWED_EDGES: dict[str, set[str]]`
  - `audit.dependencies.get_package_imports(repo_root: Path) -> dict[str, set[str]]`
  - `audit.dependencies.verify_one_way_dependencies() -> list[str]`

- [ ] **Step 1: Write failing test for package dependencies**
Create `packages/audit/tests/test_dependency.py` testing internal dependencies against the `allowed` map, layered sim subpackage dependencies (`contract`, `tuning`, `realm`, `entity`, `runtime`), and storage subpackage dependencies.

- [ ] **Step 2: Run test to verify it fails**
Run: `uv run pytest packages/audit/tests/test_dependency.py -v`
Expected: FAIL.

- [ ] **Step 3: Implement `audit/dependencies.py`**
Port the complete `allowed` map from `dependency_test.go`. Implement package dependency collection using `go list -json -deps` across workspace modules. Enforce one-way DAG dependencies.

- [ ] **Step 4: Run test to verify it passes**
Run: `uv run pytest packages/audit/tests/test_dependency.py -v`
Expected: PASS.

- [ ] **Step 5: Commit**
```bash
git add packages/audit/audit/dependencies.py packages/audit/tests/test_dependency.py
git commit -m "feat(audit): implement dependency graph and direction verification"
```

---

### Task 4: Unit Module Boundaries & Server/Client Isolation

**Files:**
- Create: `packages/audit/audit/boundaries.py`
- Create: `packages/audit/tests/test_unit_boundary.py`

**Interfaces:**
- Consumes: `audit.common.get_repo_root()`, `audit.workspace.get_workspace_modules()`
- Produces:
  - `audit.boundaries.ALLOWED_SIBLING_REQUIRES: dict[str, set[str]]`
  - `audit.boundaries.verify_module_require_boundaries() -> list[str]`
  - `audit.boundaries.verify_server_client_separation() -> list[str]`

- [ ] **Step 1: Write failing test for unit boundaries**
Create `packages/audit/tests/test_unit_boundary.py` asserting that `go.mod` files of Go modules only require permitted sibling modules (`contracts`, `shared`, etc.), and server production code does not import `client`.

- [ ] **Step 2: Run test to verify it fails**
Run: `uv run pytest packages/audit/tests/test_unit_boundary.py -v`
Expected: FAIL.

- [ ] **Step 3: Implement `audit/boundaries.py`**
Parse `go.mod` files using regex or line parsing to extract module `require` statements. Enforce sibling unit require constraints and server/client boundary guards.

- [ ] **Step 4: Run test to verify it passes**
Run: `uv run pytest packages/audit/tests/test_unit_boundary.py -v`
Expected: PASS.

- [ ] **Step 5: Commit**
```bash
git add packages/audit/audit/boundaries.py packages/audit/tests/test_unit_boundary.py
git commit -m "feat(audit): implement unit module boundaries and isolation tests"
```

---

### Task 5: Identity, Baseline Version Matrix & Source Guards

**Files:**
- Create: `packages/audit/tests/test_identity_and_baseline.py`
- Create: `packages/audit/tests/test_source_guards.py`
- Create: `packages/audit/tests/test_sim_authority.py`

**Interfaces:**
- Consumes: `audit.common.get_repo_root()`

- [ ] **Step 1: Write tests for identity, baseline matrix, and source guards**
Port checks from `identity_test.go`, `baseline_test.go`, `source_guards_test.go`, and `sim_authority_test.go`:
- Project identity `Mornlea`, no Minecraft/Mojang branding.
- Baseline versions: protocol v45, player schema v9, chunk schema v9, world metadata v6, companions.ai v5, hostile_mobs v2, passive_mobs v1, engine ABI v11, client ABI v19, benchmark scenario v23.
- Source guards: no Go package imports `webgpu`, cross-goroutine immutable message safety.
- Sim authority: server authoritative simulation command stream rules.

- [ ] **Step 2: Run test to verify failures where appropriate**
Run: `uv run pytest packages/audit/tests/test_identity_and_baseline.py packages/audit/tests/test_source_guards.py packages/audit/tests/test_sim_authority.py -v`
Expected: Tests run and fail if assertions are incomplete.

- [ ] **Step 3: Complete test implementations**
Implement scanning and verification logic matching the Go test behavior.

- [ ] **Step 4: Run tests to verify they pass**
Run: `uv run pytest packages/audit/tests/test_identity_and_baseline.py packages/audit/tests/test_source_guards.py packages/audit/tests/test_sim_authority.py -v`
Expected: PASS.

- [ ] **Step 5: Commit**
```bash
git add packages/audit/tests/test_identity_and_baseline.py packages/audit/tests/test_source_guards.py packages/audit/tests/test_sim_authority.py
git commit -m "feat(audit): implement identity baseline and source guard tests"
```

---

### Task 6: CI Workflows, Entrypoints & Godot Platform Audits

**Files:**
- Create: `packages/audit/tests/test_ci_workflows.py`
- Create: `packages/audit/tests/test_godot.py`
- Create: `packages/audit/tests/test_companion_agent.py`

**Interfaces:**
- Consumes: `audit.common.get_repo_root()`, `pyyaml`

- [ ] **Step 1: Write tests for CI workflows, Godot platform, and companion agent**
Port checks from `ci_workflow_standard_test.go`, `ci_entrypoints_test.go`, `ci_local_contracts_test.go`, the 18 `godot_*_test.go` files, and `companion_agent_service_test.go`.

- [ ] **Step 2: Run tests to verify execution**
Run: `uv run pytest packages/audit/tests/test_ci_workflows.py packages/audit/tests/test_godot.py packages/audit/tests/test_companion_agent.py -v`

- [ ] **Step 3: Complete test implementations**
Ensure YAML parsing of `.github/workflows/*.yml` checks required gates, permissions, and runner configurations. Check Godot project closure, license compliance, python isolation, and desktop-only constraints. Check companion agent HTTP/MCP schemas.

- [ ] **Step 4: Run tests to verify they pass**
Run: `uv run pytest packages/audit/tests/test_ci_workflows.py packages/audit/tests/test_godot.py packages/audit/tests/test_companion_agent.py -v`
Expected: PASS.

- [ ] **Step 5: Commit**
```bash
git add packages/audit/tests/test_ci_workflows.py packages/audit/tests/test_godot.py packages/audit/tests/test_companion_agent.py
git commit -m "feat(audit): implement ci workflows godot and companion agent tests"
```

---

### Task 7: Documentation Manifest, Doc Pairs & Links

**Files:**
- Create: `packages/audit/audit/documentation.py`
- Create: `packages/audit/tests/test_documentation.py`

**Interfaces:**
- Consumes: `audit.common.get_repo_root()`
- Produces:
  - `audit.documentation.verify_doc_manifest() -> list[str]`
  - `audit.documentation.verify_doc_pairs() -> list[str]`
  - `audit.documentation.verify_markdown_links() -> list[str]`

- [ ] **Step 1: Write failing tests for documentation verification**
Port checks from `documentation_manifest_test.go`, `documentation_pair_test.go`, `current_documentation_test.go`, and `architecture_target_documentation_test.go`.

- [ ] **Step 2: Run test to verify it fails**
Run: `uv run pytest packages/audit/tests/test_documentation.py -v`
Expected: FAIL.

- [ ] **Step 3: Implement `audit/documentation.py`**
Validate `docs/manifest.json`, verify En/Zh documentation pair synchronization, verify relative markdown links resolve to valid targets.

- [ ] **Step 4: Run test to verify it passes**
Run: `uv run pytest packages/audit/tests/test_documentation.py -v`
Expected: PASS.

- [ ] **Step 5: Commit**
```bash
git add packages/audit/audit/documentation.py packages/audit/tests/test_documentation.py
git commit -m "feat(audit): implement documentation manifest and link tests"
```

---

### Task 8: Go Workspace Streamlining & Legacy Go Audit Cleanup

**Files:**
- Modify: `go.work`
- Delete: `packages/audit/go.mod`
- Delete: `packages/audit/go.sum`
- Delete: `packages/audit/*.go` (all 54 Go test and helper files)

- [ ] **Step 1: Remove `./packages/audit` from `go.work`**
Update `go.work` use list to contain only the 5 Go modules (`contracts`, `shared`, `server`, `client`, `tools`).

- [ ] **Step 2: Remove legacy Go audit files**
Remove `packages/audit/go.mod`, `packages/audit/go.sum`, and all 54 `packages/audit/*.go` files.

- [ ] **Step 3: Verify Go workspace integrity**
Run: `go work edit -json`
Run: `go vet ./packages/contracts/... ./packages/shared/... ./packages/server/... ./packages/client/... ./packages/tools/...`
Expected: All 5 Go modules vet cleanly without errors.

- [ ] **Step 4: Run the full Python audit suite**
Run: `uv run pytest packages/audit -v`
Expected: All Python audit tests pass cleanly.

- [ ] **Step 5: Commit**
```bash
git rm packages/audit/*.go packages/audit/go.mod packages/audit/go.sum
git add go.work
git commit -m "refactor(audit): remove go audit module and streamline go.work"
```

---

### Task 9: Makefile & CI Scripts Integration

**Files:**
- Modify: `Makefile`
- Modify: `scripts/ci/doctor.sh`
- Modify: `scripts/ci/package-inventory.sh`
- Modify: `scripts/agents/gates.sh`
- Modify: `scripts/agents/race-changed.sh`
- Modify: `.github/workflows/ci.yml`
- Modify: `.github/workflows/godot.yml`

- [ ] **Step 1: Update `Makefile`**
Update `GO_TEST_MODULES` to 5 modules. Update `archcheck` to run `uv run pytest packages/audit` and the server headless check. Update `comment-language-check` to run `uv run pytest packages/audit/tests/test_comments.py`. Update `ci-preflight` and `dev-check`.

- [ ] **Step 2: Update CI helper scripts**
- `scripts/ci/doctor.sh`: update `audit)` profile to `required=(bash python3 uv rg)`.
- `scripts/ci/package-inventory.sh`: update `expected_modules` to 5 Go modules.
- `scripts/agents/gates.sh`: update Go module loops and `archcheck` command.
- `scripts/agents/race-changed.sh`: remove `packages/audit` from Go race closures.

- [ ] **Step 3: Update GitHub Actions workflows**
Update `.github/workflows/ci.yml` and `.github/workflows/godot.yml` to remove `packages/audit/go.sum` and point audit checks to Python.

- [ ] **Step 4: Verify `make archcheck` and `make comment-language-check`**
Run: `make archcheck`
Run: `make comment-language-check`
Expected: PASS.

- [ ] **Step 5: Commit**
```bash
git add Makefile scripts/ .github/
git commit -m "build(audit): integrate python audit suite into makefile and ci"
```

---

### Task 10: Global Documentation Synchronization

**Files:**
- Modify: `AGENTS.md` (root)
- Create: `packages/audit/AGENTS.md`
- Modify: `openspec/config.yaml`
- Modify: `openspec/specs/repository-code-organization/spec.md`
- Modify: `packages/server/AGENTS.md`, `packages/client/AGENTS.md`, `packages/shared/network/AGENTS.md`, etc. (all child `AGENTS.md` referencing `go test ./packages/audit` or `dependency_test.go`)
- Modify: `docs/notes/test-quickstart.md`, `docs/test-organization.md`, `docs/architecture.md`

- [ ] **Step 1: Update root `AGENTS.md` and `packages/audit/AGENTS.md`**
Update root `AGENTS.md` to state that `go.work` coordinates five Go modules, and `packages/audit` is a standalone Python unit. Update validation command to `make archcheck` / `uv run pytest packages/audit`. Rewrite `packages/audit/AGENTS.md` for the Python suite.

- [ ] **Step 2: Update `openspec/config.yaml` and `openspec/specs/repository-code-organization/spec.md`**
Update 6 Go modules references to 5 Go modules plus Python audit unit.

- [ ] **Step 3: Synchronize child `AGENTS.md` and documentation files**
Replace all stale `go test ./packages/audit -count=1` references with `make archcheck` (or `uv run pytest packages/audit`), and update `dependency_test.go` mentions to `packages/audit/audit/dependencies.py`.

- [ ] **Step 4: Run full validation suite**
Run: `make archcheck`
Run: `make dev-check`
Run: `npx --yes @fission-ai/openspec@1.7.0 validate --all --strict --no-interactive`
Expected: ALL gates pass with zero failures or doc discrepancies.

- [ ] **Step 5: Commit**
```bash
git add AGENTS.md packages/audit/AGENTS.md openspec/ packages/ docs/
git commit -m "docs(audit): synchronize documentation for python audit migration"
```
