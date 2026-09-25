# interface-first-parallel-development ledger

## 2026-09-25 — planning

- User scope: create a reusable project-wide parallel-development method; the active Rust numerical change is a concrete example, not the implementation target of this change.
- Baseline: `372da827` on `dev`; the worktree was clean before this change. The current Rust numerical interface landing is unimplemented: `mornlea_engine/src/lib.rs` has no public native module, and its active tasks 1.1/1.2 remain open.
- Design choice: require a contract landing only for a new or materially changed boundary shared by independently reviewable tasks. Alternatives rejected: a universal interface/trait framework would expand public surface without resolving real dependencies; direct per-task exports would make consumers guess signatures and impede independent starts.
- Parallelism rule: accepted contract SHA and nonoverlapping editable files permit independent provider/consumer nodes; shared adapters, registries, versioned migrations and final integration remain serial. This governs task readiness, not the separate agent-delegation decision.
- Evidence rule: contract double, real provider conformance and integrated producer-consumer behavior are distinct; no mock-only or compile-only closeout.
- Superpowers: `brainstorming` was used to classify architectural governance and compare approaches; `writing-plans` was used for three independently reviewable documentation/policy nodes. Existing authorization covers reversible planning and policy edits; no runtime implementation is in scope.
- Architecture skill: no change. This is a planning rule, not a new verified runtime ownership convention.
