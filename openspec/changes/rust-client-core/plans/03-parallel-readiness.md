# F3 parallel implementation plan

> For agentic workers: use the project-selected Superpowers execution method. Read this change's proposal, delta specifications, design and linked packets first. Only `tasks.md` carries checkboxes or completion state.

**Goal:** Make each existing deliverable independently reviewable and dispatch only after all listed predecessors have accepted evidence.

**Architecture:** Frozen upstream contracts feed disjoint providers. One controller owns shared declarations, registrations, derived artifacts and real integration; a double never closes a provider or integrated node.

**Tech Stack:** Rust 1.97.1, Go 1.26 and, where applicable, qualified Godot/embedded Python desktop presentation.

**Spec:** [design](../design.md), [exact file/API/test packets](01-client-slices.md). [Refined split-node packets](04-refined-nodes.md) override their replaced parent packets; unsplit packets retain their exact ownership and commands.

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
| `1.1` | `F1-final:2.2,F2:S2-accepted` | controller; serial shared files | Bind F1/F2 accepted prerequisites and enumerate all client semantic families. |
| `1.2` | `1.1` | controller; serial shared files | Land compiling C1/C2 contract, typed family schema, frame validator and executing consumer double. |
| `1.3` | `1.2` | controller; serial shared files | Implement login/session observation state machine. |
| `1.4` | `1.2` | controller; serial shared files | Implement confirmed mirror and atomic observation order. |
| `1.5` | `1.2` | controller; serial shared files | Implement bounded shared Memory/TCP I/O queues. |
| `2.1` | `1.2` | isolated provider; exact packet files only | Implement semantic typed input, UI token and local sequence validation. |
| `2.2` | `1.4,2.1` | isolated provider; exact packet files only | Implement reversible prediction and authoritative correction replay. |
| `2.3` | `1.4` | isolated provider; exact packet files only | Implement bounded preparation scheduling and stale-result rejection. |
| `2.3b` | `1.2,1.4,1.5` | isolated provider; exact packet files only | Prepare bounded far-tile LOD resources through accepted numerical facade. |
| `2.4` | `1.3,1.5,2.5,2.6g,2.6b,2.7a5,2.7b6,2.8,2.9` | controller; serial shared files | Assemble and atomically publish validated immutable frames. |
| `2.5` | `2.3,2.3b` | controller; serial shared files | Publish `terrain@1` semantics. |
| `2.6a` | `1.4` | isolated provider; exact packet files only | Project remote players into `actors@1`. |
| `2.6c` | `1.4` | isolated provider; exact packet files only | Project hostiles into `actors@1`. |
| `2.6d` | `1.4` | isolated provider; exact packet files only | Project passives into `actors@1`. |
| `2.6e` | `1.4` | isolated provider; exact packet files only | Project projectiles into `actors@1`. |
| `2.6f1` | `1.4` | isolated provider; exact packet files only | Project companions into `actors@1`. |
| `2.6f2` | `1.4` | isolated provider; exact packet files only | Project item drops into `actors@1`. |
| `2.6g` | `2.6a,2.6c,2.6d,2.6e,2.6f1,2.6f2` | controller; serial shared files | Assemble the complete `actors@1` family. |
| `2.6b` | `2.2` | isolated provider; exact packet files only | Publish `player-view@1` semantics. |
| `2.7a1` | `1.4` | isolated provider; exact packet files only | Project inventory and hotbar state. |
| `2.7a2` | `1.4` | isolated provider; exact packet files only | Project containers and chest revision. |
| `2.7a3` | `1.4` | isolated provider; exact packet files only | Project crafting state. |
| `2.7a4` | `1.4` | isolated provider; exact packet files only | Project furnace state. |
| `2.7a5` | `2.7a1,2.7a2,2.7a3,2.7a4` | controller; serial shared files | Assemble `inventory-ui@1`. |
| `2.7b1` | `1.4` | isolated provider; exact packet files only | Project environment state. |
| `2.7b2` | `1.4` | isolated provider; exact packet files only | Project survival state. |
| `2.7b3` | `1.4` | isolated provider; exact packet files only | Project chat state. |
| `2.7b4` | `1.4` | isolated provider; exact packet files only | Project task state. |
| `2.7b5` | `1.4,2.2` | isolated provider; exact packet files only | Project prompts. |
| `2.7b6` | `2.7b1,2.7b2,2.7b3,2.7b4,2.7b5` | controller; serial shared files | Assemble `world-ui@1`. |
| `2.8` | `1.4,2.1,2.2` | isolated provider; exact packet files only | Publish provenance-aware `audio-cues@1` semantics. |
| `2.9` | `1.2` | isolated provider; exact packet files only | Publish `diagnostics@1` semantics. |
| `3.1a` | `2.4` | controller; serial shared files | Assign and validate G1 logical-to-numeric descriptors without enabling features. |
| `3.1b` | `3.1a,3.3a` | controller; serial shared files | Connect the safe Rust core adapter and migrate Godot-callable methods serially. |
| `3.2` | `3.1b` | controller; serial shared files | Implement symbolic family negotiation and one-session feature host activation. |
| `3.3a` | `2.4` | controller; serial shared files | Implement core reset, reconnect and close with queued-work invalidation. |
| `3.3b` | `3.3a,3.2` | controller; serial shared files | Implement native Godot/Python release ordering and boundary panic containment. |
| `3.3c` | `3.3b` | controller; serial shared files | Qualify rebuilt Rust producer artifacts through 100 real headless session cycles. |
| `3.4` | `3.3c,F2:4.2` | controller; serial shared files | Integrate real F2 Memory/TCP server, C1/C2 and G1 against all accepted families. |
| `4.1` | `3.4` | controller; serial shared files | Reconcile zero-gap inventory, real provider/integration cases and guides. |
| `4.2` | `4.1` | controller; serial shared files | Run complete Rust, Go, audit and OpenSpec stage gates on the recorded SHA. |

## Dispatch and acceptance procedure

For the selected row, send only its exact packet, consumed upstream declarations, accepted prerequisite SHAs, read-only oracle paths and first failing case. Execute its test cycle in this order: add the specified assertion; run and capture behavioral failure after contract compilation; implement the named provider/algorithm; rerun the selected nonempty target and named compatibility oracle; independently review the deliverable; integrate and rerun affected consumer gates; commit only owned passing files. Fill `{node, baseline_sha, contract_sha, editable_files, read_only_files, fixture_sha256, red_command/result, green_command/result, discovered/executed_cases, derived_refresh, integration_sha, rollback}` in the controller-owned ledger.

The contract gate executes declarations/constructors and a success/failure double. Provider acceptance executes real code against the packet's success, failure, ordering and boundary cases. Integration acceptance executes real producer and consumer together. Each has a separate ledger result. A contract discrepancy pauses only affected consumers while the controller revises declarations, design, packets and tests and records a new accepted SHA.

The shared edit lease includes workspace/lockfiles, crate exports, test roots, fixture/corpus indexes, Godot bridge/host/catalogs, shared runners, release manifests, Makefile and CI. Acquire it serially, reread merged files and refresh their derived consumers before release. Provider rollback reverts only its files; a contract rollback includes all consumers of that identity. Stage closure runs the full gates specified in the original packet, including formatting, six-module vet via `make dev-check`, `make test-race`, audit and strict OpenSpec validation. Stop at focused gates inside editing loops unless a new failure warrants more.
