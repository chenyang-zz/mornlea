# P12 parallel implementation plan

> For agentic workers: use the project-selected Superpowers execution method. Read this change's proposal, delta specifications, design and linked packets first. Only `tasks.md` carries checkboxes or completion state.

**Goal:** Make each existing deliverable independently reviewable and dispatch only after all listed predecessors have accepted evidence.

**Architecture:** Frozen upstream contracts feed disjoint providers. One controller owns shared declarations, registrations, derived artifacts and real integration; a double never closes a provider or integrated node.

**Tech Stack:** Rust 1.97.1, Go 1.26 and, where applicable, qualified Godot/embedded Python desktop presentation.

**Spec:** [design](../design.md), [exact file/API/test packets](worker-packets.md). [Refined split-node packets](04-refined-nodes.md) override their replaced parent packets; unsplit packets retain their exact ownership and commands.

## Global constraints

- Baseline for this planning revision: `974458f0`; numerical implementation accepted at `9b843bbc`; complete F1 remains unaccepted until the [foundation successor](../../archive/2026-10-08-rust-runtime-foundation-acceptance/tasks.md) seals it.
- Preserve protocol v45, player/chunk v9, metadata v6, companions v5, hostiles v2, passives v1, engine ABI v11, renderer client ABI v19 and scenario v23. The pilot core ABI 1.1 is a distinct rollback surface.
- Upstream contract SHA is an execution-time ledger value, never the planning SHA. Every provider owns exactly its packet files; all other sources and parent/shared entrypoints are read-only.
- New target files/tests/CLI flags remain prospective until their owning predecessor lands. Discovery must contain the selected case; a filter with zero executions fails acceptance.
- Three workers maximum, isolated worktrees for independent implementation. Controller integrations and ledgers are serialized across changes; separate feature directories do not authorize concurrent catalog edits.
- Worker source comments use English and contain no applicable task identifier. Each focused passing node receives a scoped commit; no automatic runtime, baseline or default cutover follows planning.

## Review focus

1. Partial enqueue or publication on cap+1: owning packet asserts unchanged sequence, queue and prior visible state.
2. Late reset/despawn/cancel completion: owning packet rejects the old epoch/generation and releases exactly once.
3. Unsupported source information: projections preserve absence; no invented task ID, actor hit association, wire container revision or successful local receipt.
4. Empty test filter or stale binary: controller records nonzero execution and source/fixture/producer identity before closing a node.
5. Shared or derived file drift: one controller refreshes source hashes, module/test registrations, manifests/catalogs and affected downstream gates on the merged SHA.

## Direct predecessor register

Every local ID below is in this change's `tasks.md`. External `F1-final` means `rust-runtime-foundation-acceptance`; stage aliases resolve through the [dispatch index](../../godot-default-client-switch/plans/00-cross-change-dispatch.md). `F2:S2-accepted` means accepted common S2 plus BOTH actual Memory/TCP adapters on one SHA, not the common declarations alone. Transitive prerequisites are not repeated. For a split node, its refined packet replaces the parent ID; no retired parent is an independent task.

| Node | Direct accepted predecessors | Execution owner / boundary | Independently accepted deliverable |
| --- | --- | --- | --- |
| `1.1` | `F3:3.1b` | controller; serial shared files | Inventory canonical producers and release exclusions. |
| `1.2` | `1.1` | controller; serial shared files | Land the tooling-only strict run/report schema and behavioral double. |
| `2.1` | `1.2` | isolated provider; exact packet files only | Implement producer registry and required-case validation. |
| `2.2a` | `1.2,F2:4.2` | isolated provider; exact packet files only | Implement source-bound semantic replay adapter. |
| `2.2b1` | `1.2` | isolated provider; exact packet files only | Implement qualified no-focus UI/world still-frame capture. |
| `2.2b2` | `1.2` | isolated provider; exact packet files only | Implement bounded no-focus motion timing and capture. |
| `2.2c` | `2.1,2.2a,2.2b1,2.2b2,2.3` | isolated provider; exact packet files only | Integrate atomic candidate run orchestration. |
| `2.3` | `1.2` | isolated provider; exact packet files only | Implement strict artifact/report comparison. |
| `2.4` | `2.2c` | controller; serial shared files | Integrate ownership-aware visual-regression dispatcher; accept phase-2 contract. |
| `3.1` | `2.4` | controller; serial shared files | Implement transactional per-case reviewed handoff. |
| `3.2` | `3.1,P8:3.2,P9:3.2,P10:3.2,P11:3.3` | controller; serial shared files | Transfer only explicitly approved cases and prove rollback. |
| `3.3` | `1.2` | isolated provider; exact packet files only | Implement informational benchmark report collector. |
| `3.4a` | `2.4` | isolated provider; exact packet files only | Implement import resource checks. |
| `3.4b` | `2.4` | isolated provider; exact packet files only | Implement developer capture checks. |
| `3.4c` | `3.4a,3.4b` | controller; serial shared files | Integrate tooling CI without premature required-entry promotion. |
| `4.1` | `3.2,3.3,3.4c` | controller; serial shared files | Reconcile producer/adapter coverage, guides and rollback. |
| `4.2` | `4.1` | controller; serial shared files | Run complete implementation stage gates. |

## Dispatch and acceptance procedure

For the selected row, send only its exact packet, consumed upstream declarations, accepted prerequisite SHAs, read-only oracle paths and first failing case. Execute its test cycle in this order: add the specified assertion; run and capture behavioral failure after contract compilation; implement the named provider/algorithm; rerun the selected nonempty target and named compatibility oracle; independently review the deliverable; integrate and rerun affected consumer gates; commit only owned passing files. Fill `{node, baseline_sha, contract_sha, editable_files, read_only_files, fixture_sha256, red_command/result, green_command/result, discovered/executed_cases, derived_refresh, integration_sha, rollback}` in the controller-owned ledger.

The contract gate executes declarations/constructors and a success/failure double. Provider acceptance executes real code against the packet's success, failure, ordering and boundary cases. Integration acceptance executes real producer and consumer together. Each has a separate ledger result. A contract discrepancy pauses only affected consumers while the controller revises declarations, design, packets and tests and records a new accepted SHA.

The shared edit lease includes workspace/lockfiles, crate exports, test roots, fixture/corpus indexes, Godot bridge/host/catalogs, shared runners, release manifests, Makefile and CI. Acquire it serially, reread merged files and refresh their derived consumers before release. Provider rollback reverts only its files; a contract rollback includes all consumers of that identity. Stage closure runs the full gates specified in the original packet, including formatting, six-module vet via `make dev-check`, `make test-race`, audit and strict OpenSpec validation. Stop at focused gates inside editing loops unless a new failure warrants more.
