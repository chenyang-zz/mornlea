# P13 parallel implementation plan

> For agentic workers: use the project-selected Superpowers execution method. Read this change's proposal, delta specifications, design and linked packets first. Only `tasks.md` carries checkboxes or completion state.

**Goal:** Make each existing deliverable independently reviewable and dispatch only after all listed predecessors have accepted evidence.

**Architecture:** Frozen upstream contracts feed disjoint providers. One controller owns shared declarations, registrations, derived artifacts and real integration; a double never closes a provider or integrated node.

**Tech Stack:** Rust 1.97.1, Go 1.26 and, where applicable, qualified Godot/embedded Python desktop presentation.

**Spec:** [design](../design.md), [exact file/API/test packets](worker-packets.md). [Refined split-node packets](04-refined-nodes.md) override their replaced parent packets; unsplit packets retain their exact ownership and commands.

## Global constraints

- Baseline for this planning revision: `974458f0`; numerical implementation accepted at `9b843bbc`; complete F1 remains unaccepted until the [foundation successor](../../rust-runtime-foundation-acceptance/tasks.md) seals it.
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
| `1.1` | `F2:4.2,F3:4.2` | controller; serial shared files | Inventory exact release assets, dependencies, target and backup identities. |
| `1.2` | `1.1` | controller; serial shared files | Register nonempty launcher/release harness with behavioral red. |
| `2.1a` | `1.2,F2:3.5` | isolated provider; exact packet files only | Implement generic supervised loopback Rust-server launch and lease coordination. |
| `2.1b` | `1.2` | isolated provider; exact packet files only | Implement Unix process-group teardown. |
| `2.1c` | `1.2` | isolated provider; exact packet files only | Implement Windows Job Object teardown. |
| `2.2` | `1.2` | isolated provider; exact packet files only | Implement typed Godot launch progress and cancellation. |
| `2.3` | `1.2` | isolated provider; exact packet files only | Implement relocatable package asset/export resolver. |
| `2.3b1` | `1.2` | isolated provider; exact packet files only | Verify and prepare pinned Windows Godot editor. |
| `2.3b2` | `2.3b1` | isolated provider; exact packet files only | Build and verify relocatable Windows embedded Python payload. |
| `2.3b3` | `2.3b1` | isolated provider; exact packet files only | Build and verify Windows native Rust/GDExtension payload. |
| `2.3c1` | `1.2` | isolated provider; exact packet files only | Verify and prepare pinned Linux Godot editor. |
| `2.3c2` | `2.3c1` | isolated provider; exact packet files only | Build and verify relocatable Linux embedded Python payload. |
| `2.3c3` | `2.3c1` | isolated provider; exact packet files only | Build and verify Linux native Rust/GDExtension payload. |
| `2.4a` | `2.1b,2.1c,2.3b2,2.3b3,2.3c2,2.3c3` | controller; serial shared files | Route shared build wrappers to accepted target-specific providers. |
| `2.4b` | `2.4a,2.1a,2.2` | controller; serial shared files | Integrate the Rust desktop launcher with actual supervisor and readiness providers. |
| `2.4c` | `2.4b,2.3` | controller; serial shared files | Integrate strict package closure and identity-complete release reports. |
| `3.1` | `2.4c` | isolated provider; exact packet files only | Qualify macOS package on macOS. |
| `3.2` | `2.4c` | isolated provider; exact packet files only | Qualify Windows package on Windows. |
| `3.3` | `2.4c` | isolated provider; exact packet files only | Qualify Linux package on Linux. |
| `3.4` | `3.1,3.2,3.3` | controller; serial shared files | Prove save failure/recovery and complete previous-release restore. |
| `3.5` | `3.4,P12:2.4,P8:4.2,P9:4.2,P10:4.2,P11:4.2` | controller; serial shared files | Capture untracked candidate release evidence after P12 phase 2. |
| `4.1` | `3.5` | controller; serial shared files | Reconcile supported target/package coverage, guides and rollback. |
| `4.2` | `4.1` | controller; serial shared files | Run complete implementation stage gates. |

## Dispatch and acceptance procedure

For the selected row, send only its exact packet, consumed upstream declarations, accepted prerequisite SHAs, read-only oracle paths and first failing case. Execute its test cycle in this order: add the specified assertion; run and capture behavioral failure after contract compilation; implement the named provider/algorithm; rerun the selected nonempty target and named compatibility oracle; independently review the deliverable; integrate and rerun affected consumer gates; commit only owned passing files. Fill `{node, baseline_sha, contract_sha, editable_files, read_only_files, fixture_sha256, red_command/result, green_command/result, discovered/executed_cases, derived_refresh, integration_sha, rollback}` in the controller-owned ledger.

The contract gate executes declarations/constructors and a success/failure double. Provider acceptance executes real code against the packet's success, failure, ordering and boundary cases. Integration acceptance executes real producer and consumer together. Each has a separate ledger result. A contract discrepancy pauses only affected consumers while the controller revises declarations, design, packets and tests and records a new accepted SHA.

The shared edit lease includes workspace/lockfiles, crate exports, test roots, fixture/corpus indexes, Godot bridge/host/catalogs, shared runners, release manifests, Makefile and CI. Acquire it serially, reread merged files and refresh their derived consumers before release. Provider rollback reverts only its files; a contract rollback includes all consumers of that identity. Stage closure runs the full gates specified in the original packet, including formatting, six-module vet via `make dev-check`, `make test-race`, audit and strict OpenSpec validation. Stop at focused gates inside editing loops unless a new failure warrants more.
