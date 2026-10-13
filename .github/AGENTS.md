# GitHub Automation Guide

This guide owns `.github` automation and pull-request metadata. Workflow policy is enforced by the structured tests in `packages/audit`; repository commands own validation semantics.

## Directory Map

```text
.github/
├── workflows/                 # Required and optional GitHub Actions orchestration
└── PULL_REQUEST_TEMPLATE.md    # Pull-request summary and validation format
```

## Dependency Direction

Workflows invoke repository-owned Make targets and scripts. Keep package selection, native artifact verification, and test selectors in those entry points. `TestRequiredCIWorkflow` enforces the required graph and thin orchestration boundary.

## Entry Modes and Lifecycle

`workflows/ci.yml` runs for pull requests and pushes to `main`. Its sole merge-authority result is `Required CI / merge-gate`; the aggregator must assert success for every required predecessor, including skipped or cancelled results. Candidate-bound platform artifacts must be downloaded to the repository root before verified consumer entry points run.

Actions use reviewed immutable commits with major-version comments. Runner images, timeouts, toolchains, cancellation, and least-privilege permissions are explicit. Duration summaries are informational; failures cannot be converted to success or retried automatically.

Godot migration validation belongs to a separate optional workflow and never participates in the required merge graph. Optional failures remain visible failures.

## Documentation Sync Policy

Update the executable workflow audits with graph, environment, or artifact contract changes. Keep shared architectural decisions in OpenSpec and canonical development guidance; do not copy test-selection logic into workflow YAML.

## Focused Verification

Run from the repository root:

```bash
uv run pytest packages/audit/tests/test_ci_workflows.py
```

The CI workflow tests parse YAML with PyYAML before checking policy and mutations. Also parse changed workflow YAML independently where a YAML parser is available. Full audit and the optional Godot rollback check cover cross-workflow changes; tiering follows `docs/notes/test-quickstart.md`.
