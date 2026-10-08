## 1. Planning rule

- [x] 1.1 Update `AGENTS.md`, `openspec/config.yaml`, synchronized `.codex/skills/mornlea-implementation-orchestration/` and `.claude/skills/mornlea-implementation-orchestration/` with mandatory Superpowers planning and the controller readiness checklist. Validate with `go test ./packages/audit -run 'TestProviderAwareOrchestration|TestDelegationBudget|TestProjectOrchestration|TestProjectOpenSpecApply' -count=1` and byte comparison of both skill trees.
- [x] 1.2 Synchronize `docs/development-process.md`, `docs/development-process.zh.md`, `docs/openspec.md`, and `docs/openspec.zh.md`, including matching revision metadata and the two entries in `docs/documentation-manifest.json`. Validate with `go test ./packages/audit -run 'TestDocumentationManifest|TestCompletedDocumentationPairsAreSynchronized|TestDocumentationPairValidation|TestLegacyDocumentation|TestDocumentationLinks' -count=1`; classify any unchanged baseline version failure separately without claiming it passes.

## 2. Qualification

- [x] 2.1 Apply the checklist to the unfinished `rust-runtime-foundation` plan; record concrete interfaces, family-level tasks, dependency graph, failing examples and acceptance rather than controller-held design placeholders. Validate all task links, unique IDs and acyclic dependencies, and record the self-review in that change's ledger.
- [x] 2.2 Validate both project skills with the available skill validator, run `git diff --check` and `openspec validate --all --strict --no-interactive`, and record the exact result in `ledger.md`. No production source changed, so formatting/build, six-module vet via `make dev-check`, and full `make test-race` are not executed here; they remain mandatory stage gates in the foundation implementation plan.
