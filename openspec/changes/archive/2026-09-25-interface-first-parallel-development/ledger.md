# interface-first-parallel-development ledger

## 2026-09-25 — planning

- User scope: create a reusable project-wide parallel-development method; the active Rust numerical change is a concrete example, not the implementation target of this change.
- Baseline: `372da827` on `dev`; the worktree was clean before this change. The current Rust numerical interface landing is unimplemented: `mornlea_engine/src/lib.rs` has no public native module, and its active tasks 1.1/1.2 remain open.
- Design choice: require a contract landing only for a new or materially changed boundary shared by independently reviewable tasks. Alternatives rejected: a universal interface/trait framework would expand public surface without resolving real dependencies; direct per-task exports would make consumers guess signatures and impede independent starts.
- Parallelism rule: accepted contract SHA and nonoverlapping editable files permit independent provider/consumer nodes; shared adapters, registries, versioned migrations and final integration remain serial. This governs task readiness, not the separate agent-delegation decision.
- Evidence rule: contract double, real provider conformance and integrated producer-consumer behavior are distinct; no mock-only or compile-only closeout.
- Superpowers: `brainstorming` was used to classify architectural governance and compare approaches; `writing-plans` was used for three independently reviewable documentation/policy nodes. Existing authorization covers reversible planning and policy edits; no runtime implementation is in scope.
- Architecture skill: no change. This is a planning rule, not a new verified runtime ownership convention.

## 2026-09-25 — reusable guide

- Node 1.1: on baseline `de2dd7df`, the new English/Chinese guide and manifest entry passed `go test ./packages/audit -run 'TestDocumentationManifest|TestCompletedDocumentationPairsAreSynchronized|TestDocumentationPairValidation|TestDocumentationLinks' -count=1` and `git diff --check` (exit 0 each). The same documentation audit passed before the guide existed. No runtime red/green claim was made for this documentation addition.
- Controller review: both versions identify the same selection cases, contract packet fields, three evidence gates, central change path, and the Rust numerical example as unimplemented. The guide directory inherits `docs/AGENTS.md`; it creates no independent ownership boundary. The task's rollback unit is the guide pair plus its manifest registration.

## 2026-09-25 — task 2.1 contract identity clarification

- Ruling: a future accepted contract SHA cannot be known while the initial task plan is written. The plan must name the contract-landing predecessor; the dispatch packet gains the actual accepted SHA after that node passes. Both guide versions, their revision and task 2.1 ownership were reconciled before task 2.1 acceptance. This preserves the intended interface-first gate without a fabricated SHA placeholder.
- Node 2.1: baseline `5911d62f`. Both project skill `SKILL.md` files and both planning references are byte-identical (`cmp` exit 0). The focused provider-aware audit, focused documentation audit, strict change validation and `git diff --check` all passed (exit 0). No agent count or execution-shape setting changed.
- Readiness examples reviewed: disjoint provider files are dispatchable after one accepted contract SHA and passing predecessor gates; two tasks writing one adapter require a serial integration owner; a consumer double can prove only contract/consumer readiness, so a mock-only result cannot close provider or integration acceptance. The guide states all three cases and the machine guidance points to it.

## 2026-09-25 — workflow integration and closeout

- Node 3.1 tested source: `0d89e6d7` plus the English/Chinese development-process and documentation-map worktree diff and manifest revision. The focused documentation audit, full `go test ./packages/audit -count=1`, `git diff --check`, `make dev-check` (six-module vet, six-module short tests, Rust fmt/clippy/workspace tests), `make test-race` (Rust release build and six-module Go race), and `openspec validate --all --strict --no-interactive` all exited 0. Strict OpenSpec validation reported 128 passed, 0 failed. The long audit and race suites completed without a timeout or exemption.
- Review: navigation resolves to the new bilingual guide; revisions and manifest entries match. Contract landing, provider and integration remain separate gates. The current numerical contract is still planned and unimplemented; this governance closeout claims no numerical execution or runtime migration.
- Directory guidance: all edited documentation inherits `docs/AGENTS.md`, policy files inherit the root guide, and existing project skill directories retain their guides. No new independent ownership directory was created.
- Architecture skill: no change. The work codifies orchestration readiness, not a new code-backed cross-task architecture fact.

## 2026-09-25 — canonical sync and archive decision

- Accepted node commits: plan `de2dd7df`, guide `5911d62f`, machine readiness `0d89e6d7`, workflow navigation `258e2ed2`. All three task checkboxes are complete and the worktree was clean at `258e2ed2` before canonical sync.
- The four added governance requirements were copied into the canonical `openspec/specs/development-governance/spec.md` under its existing `## Requirements`, preserving all prior requirements. Exact delta-body comparison passed; `openspec validate --all --strict --no-interactive` passed 128/128 and `git diff --check` passed after sync. No version matrix, runtime source, save, protocol, or ABI changed.
- Archive choice: sync the completed behavioral governance delta before archiving so future plans use the canonical specification. No user confirmation is repeated because the current request already authorizes making the reusable rule effective, and the archive is a reversible repository move.
- Archive result: moved to `openspec/changes/archive/2026-09-25-interface-first-parallel-development/`. Post-move `openspec validate --all --strict --no-interactive` passed 127/127 current items, the focused documentation audit passed, and `git diff --check` passed. The completed change's six artifacts remain together as historical evidence.
