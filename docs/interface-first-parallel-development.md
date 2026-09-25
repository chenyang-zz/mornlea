---
doc_id: interface-first-parallel-development
doc_revision: 2026-09-25.1
language: en
counterpart: interface-first-parallel-development.zh.md
---
# Interface-first parallel development

This guide makes a multi-task plan executable when tasks share a changing boundary. It governs task readiness and integration; [the development process](development-process.md) still governs agent delegation, review, commits, and required gates. [中文版](interface-first-parallel-development.zh.md).

## Decide whether a separate contract landing is needed

Draw producer → consumer edges before assigning workers. Classify each boundary, then record the choice in the active OpenSpec `design.md` and task DAG.

| Boundary | Plan shape |
| --- | --- |
| Existing stable API with tested behavior | Tasks use its current identity and focused tests; no new landing. |
| New or materially changed API consumed by at least two independently reviewable tasks | One controller-owned, compile-ready contract landing precedes those tasks. |
| API private to one implementation task | Keep the API and implementation in that task. |
| Shared mutable file, registry, schema version, adapter, or migration | Assign an exclusive serial integration owner even if other tasks use the same contract. |

An interface can be a function signature, semantic value, Rust trait, Go interface, schema, message, or fixture format. Use a trait or interface when consumers need that form of substitution; do not add one merely because work is parallel. Put the contract in the lowest stable owner reachable by all consumers without a dependency cycle. Expose the smallest semantic surface; keep storage layout, parser state, algorithms, and provider helpers private. A plan with no eligible shared boundary records why no contract landing is needed.

## Contract packet to freeze before dispatch

The controller writes one packet per shared boundary in the active change's `design.md` or linked brief. `tasks.md` remains the only task-status source. A packet is ready only when every row below has a concrete value or an explicit non-applicable ruling.

| Field | Required decision |
| --- | --- |
| Identity and ownership | Contract name, owning module/file, baseline source and accepted contract commit SHA; exact provider and consumer tasks. |
| API | Exact operation signatures and field types, existing versus new type owner, allowed dependency direction, version or compatibility identity. |
| Data meaning | Units, coordinates, ordering, default/unknown values, valid and invalid ranges, normalization, mutability, ownership and lifetime. |
| Call lifecycle | State transitions, concurrency and cancellation, scratch ownership and reuse, publication/atomicity, error precedence and capacity units. |
| Bounds | Maximum count, bytes, work and allocations; overflow behavior and hot-path restrictions. |
| Examples | Deterministic success, invalid-input, boundary and failure examples with exact expected observations and an independent oracle where compatibility matters. |
| Files and gates | Contract, provider, consumer and integration-owned paths; test-discovery and run commands; rollback unit and downstream gates. |

Do not hide a design choice behind “implementation-defined,” “handle edge cases,” or a worker's discretion. A worker may choose private helper names and equivalent local code organization, but not public behavior or ownership.

## Land the contract once

1. Create the smallest compile-ready declarations and validated constructors or parsers needed for real fixtures. If module registration requires future provider files, create empty compiling files and reserve them for their owners. Do not add a production operation that returns synthetic success, panics as a placeholder, or silently falls back to another owner.
2. Compile a consumer against the public surface using a deterministic double, and execute at least one success and one typed failure path. Check real constructor validation, ordering and capacity examples where applicable. A compile-only test proves shape, not behavior.
3. Review and validate the landing, then record its commit SHA and exact commands in `ledger.md`. Every independent provider or consumer task names that SHA and its direct predecessors. Work starts only from the accepted contract identity.

The landing proves that peers can develop without reading one another's internals. It does not prove any concrete provider or integrated runtime.

## Schedule by dependencies and ownership

```text
accepted contract SHA
  ├─ provider A ─┐
  ├─ provider B ─┼─ serial shared adapter/registry ─ integrated gate
  └─ consumer C ─┘
```

Two nodes are ready together only if all predecessors have accepted evidence, editable files do not overlap, neither mutates shared state owned by the other, and neither needs an unresolved version or compatibility ruling. Give each node exact editable and read-only files and an independently reviewable test cycle. Separate worktrees or exclusive file sets may start from the same accepted SHA. Shared adapters, generated artifacts, registries, migrations and version bumps have one editor or an explicit serial order. Merge reviewed results one at a time and rerun affected downstream checks on the merged SHA. Task parallelism does not change the project's agent concurrency and delegation policy.

## Keep the three evidence gates separate

| Gate | What must execute | What it does not prove |
| --- | --- | --- |
| Contract | Public calls, validated fixtures, and consumer doubles for success and failure. | Concrete provider correctness. |
| Provider | Real implementation against exact success, error, limit and ordering cases; focused regression/oracle checks. | Real consumer integration. |
| Integration | Real producer and consumer on one recorded SHA, plus applicable ABI, protocol, save, corpus, or independent oracle checks. | Nothing beyond the executed case set. |

An inventory name, an unexecuted test, type compilation, or a passing double cannot close a provider or integration task. Record nonzero discovered and executed cases, command, result, SHA and applicable fixture identity in the change ledger. `tasks.md` closes only after its named gate passes.

## Change a frozen contract deliberately

When implementation finds a discrepancy, the worker reports the verified source, a failing case, the exact conflicting field or operation, and affected tasks. The controller rules on the contract, updates the active design, behavioral delta spec if needed, all affected briefs and contract tests, and lands a new accepted SHA. Affected tasks rebase or otherwise adopt that identity and rerun their gates. Unaffected tasks may continue if they do not consume the changed surface. Wire, save and ABI changes retain their existing compatibility and exclusivity rules. No worker silently edits a shared declaration to satisfy a local test.

## Planned example: Rust numerical closure

The active [numerical closure design](../openspec/changes/rust-native-numerical-closure/design.md) plans node 1.2 as a typed `mornlea_engine::native::contracts` landing with shared validated inputs and operation-trait doubles. After its **accepted implementation SHA**, disjoint collision, raycast, world, fluid, mesh and path provider nodes can start against that contract. The `ffi.rs` adapter and corpus registration have serial owners, followed by real ABI and source-bound corpus execution. At the time of this guide, the contract landing and providers are **planned, not implemented**; their OpenSpec status alone is not acceptance evidence. This example shows the task graph, not a claim that all lanes already pass.
