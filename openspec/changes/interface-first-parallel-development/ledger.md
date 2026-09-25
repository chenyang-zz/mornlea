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
