# F2 parallel implementation plan

> For agentic workers: use the project-selected Superpowers execution method. Read this change's proposal, delta specifications, design and linked packets first. Only `tasks.md` carries checkboxes or completion state.

**Goal:** Make each existing deliverable independently reviewable and dispatch only after all listed predecessors have accepted evidence.

**Architecture:** Frozen upstream contracts feed disjoint providers. One controller owns shared declarations, registrations, derived artifacts and real integration; a double never closes a provider or integrated node.

**Tech Stack:** Rust 1.97.1, Go 1.26 and, where applicable, qualified Godot/embedded Python desktop presentation.

**Spec:** [design](../design.md), [exact file/API/test packets](01-server-slices.md). [Refined split-node packets](04-refined-nodes.md) override their replaced parent packets; unsplit packets retain their exact ownership and commands.

## Global constraints

- Planning baseline: `974458f0`; numerical implementation accepted at `9b843bbc`. Complete F1 is sealed at source `d042982d33bb1694d768b75b01c297bd02534a08` in the [foundation acceptance identity](../../rust-runtime-foundation-acceptance/acceptance.json): 112 supported points, 1434 cases, zero gaps. Verify that exact source/corpus identity before dispatch; current node acceptance is recorded only in `tasks.md` and the change ledger.
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
| `1.1a` | `F1-final:2.2` | controller; serial shared files | Verify the sealed complete F1 prerequisite and enumerate server capabilities. |
| `1.1b` | `1.1a` | controller; serial shared files | Record command, chunk-result and persistence high-water from every supported server replay. |
| `1.1c` | `1.1b` | controller; serial shared files | Freeze S1 bounds and source-bound capability-to-test mappings before the contract landing. |
| `1.2` | `1.1c` | controller; serial shared files | Land compiling S1 contract, bounded types and executing consumer double; freeze its SHA. |
| `1.3` | `1.2` | controller; serial shared files | Implement session admission, sequenced intake and control-plane separation. |
| `1.4` | `1.2` | controller; serial shared files | Implement bounded tick/chunk mailboxes and cancellation. |
| `1.5a` | `1.2` | controller; serial shared files | Implement owned tick publication and bounded slow-receiver outboxes. |
| `1.5b` | `1.5a` | controller; serial shared files | Implement one final unpublished tick and retryable shutdown phases. |
| `1.6` | `1.2` | controller; serial shared files | Implement authority-resolved atomic placement/mining transaction and failure invariants. |
| `2.1a` | `1.2,1.4` | isolated provider; exact packet files only | Implement chunk acquisition and stale-generation rejection. |
| `2.1b` | `1.6,2.1a,2.6a` | isolated provider; exact packet files only | Implement world placement/internal interaction geometry through the atomic transaction. |
| `2.1c` | `1.6,2.1b,2.5a,2.6a` | isolated provider; exact packet files only | Implement continuous mining progress, tool reset and atomic completion. |
| `2.2` | `1.2` | isolated provider; exact packet files only | Implement time, season, weather and environment transition replay. |
| `2.3` | `1.2,1.6,2.1a` | isolated provider; exact packet files only | Implement bounded fluid rescan and update scheduling. |
| `2.4a` | `2.3` | isolated provider; exact packet files only | Implement bounded farmland moisture. |
| `2.4b` | `2.4a,2.5a,2.8a` | isolated provider; exact packet files only | Implement actor trample and snow footprints. |
| `2.4d` | `2.4b,2.1b` | isolated provider; exact packet files only | Implement deterministic crop, dry farmland, tree, grass and snow random rules. |
| `2.4c` | `2.4d,2.1b` | isolated provider; exact packet files only | Implement ordered block-support sweeps. |
| `2.5a` | `1.3,2.1a` | isolated provider; exact packet files only | Implement authoritative player control and movement. |
| `2.5b` | `2.5a` | isolated provider; exact packet files only | Implement survival transitions and correction observations. |
| `2.5c` | `2.5b,2.6a` | isolated provider; exact packet files only | Implement atomic eating inventory and hunger settlement. |
| `2.6a` | `1.2` | isolated provider; exact packet files only | Implement inventory authority and item conservation. |
| `2.6b` | `2.6a` | isolated provider; exact packet files only | Implement containers and generation/view validation. |
| `2.6c` | `2.6b` | isolated provider; exact packet files only | Implement atomic workbench crafting. |
| `2.6d` | `2.6b` | isolated provider; exact packet files only | Implement tick-driven furnaces. |
| `2.6e0` | `2.1c` | controller-owned shared state/mining contract | Add bounded tick-local suppression and receipt preflight. |
| `2.6e` | `1.6,2.1b,2.6a,2.4a,2.6e0` | isolated provider; exact packet files only | Implement farming tools, bone meal and atomic buckets. |
| `2.7a` | `1.2,2.1a` | isolated provider; exact packet files only | Implement hostile lifecycle and targeting. |
| `2.7b0` | `1.2` | controller-owned shared contract; see refined packet | Complete projectile read, compare-and-replace staging and replay initialization. |
| `2.7b` | `2.7a,2.6a,2.7b0` | isolated provider; exact packet files only | Implement projectiles and hit validation. |
| `2.7c` | `2.7b,2.5a,2.6a,2.8a` | isolated provider; exact packet files only | Implement one-time hostile death and combat outcomes. |
| `2.8a` | `1.2,1.6,2.1a` | isolated provider; exact packet files only | Implement passive lifecycle. |
| `2.8b` | `2.6a,2.7c,2.8a` | isolated provider; exact packet files only | Implement item drops and pickup. |
| `2.8c` | `2.5b,2.2,2.8a` | isolated provider; exact packet files only | Implement sleeping and time transition. |
| `2.9a` | `1.2` | isolated provider; exact packet files only | Implement sessionless companion candidate provenance and admission. |
| `2.9b` | `2.9a,1.6,2.1a,2.6a,2.1b,2.1c` | isolated provider; exact packet files only | Revalidate and execute companion actions through the shared mutation pipeline. |
| `3.1` | `1.3,1.4,1.5b,1.6,2.1c,2.2,2.3,2.4c,2.5c,2.6c,2.6d,2.6e,2.7c,2.8b,2.8c,2.9b` | controller; serial shared files | Integrate the one authoritative tick reducer after all rule providers. |
| `3.2` | `1.2,1.3` | isolated provider; exact packet files only | Land the common protocol/login/validation transport path. |
| `3.3a` | `3.2` | isolated provider; exact packet files only | Implement the Memory adapter over common admission. |
| `3.3b` | `3.2` | isolated provider; exact packet files only | Implement the TCP adapter over common admission. |
| `3.4a` | `1.2` | isolated provider; exact packet files only | Implement bounded durable store mailbox. |
| `3.4b` | `3.4a` | isolated provider; exact packet files only | Implement autosave, retry, backpressure and flush scheduling. |
| `3.4c0` | `3.4a,3.4b` | controller; shared I/O contract | Complete decoded loads, partial outcomes and fault/cancellation seam. |
| `3.4c` | `1.2,3.4c0` | isolated provider; exact packet files only | Implement real region commit, crash boundaries and compaction. |
| `3.4d` | `1.2,3.4c0` | isolated provider; exact packet files only | Implement standalone atomic file persistence. |
| `3.5` | `3.4b,3.4c,3.4d` | isolated provider; exact packet files only | Implement exclusive world lease, recovery and named-backup rollback. |
| `3.6a` | `1.2` | isolated provider; exact packet files only | Implement Agent HTTP wire and lease provider. |
| `3.6b` | `1.2` | isolated provider; exact packet files only | Implement frozen snapshot registry and MCP provider. |
| `3.6c` | `3.6a,3.6b,2.9a` | isolated provider; exact packet files only | Implement Agent task, dialogue and memory ownership. |
| `3.6d` | `3.6c,2.9b,3.1,3.5` | controller; serial shared files | Execute Rust integration with the actual Python Agent and MCP. |
| `3.7` | `3.1,3.3a,3.3b,3.5,3.6d` | controller; serial shared files | Prove real local/remote, save/restart and Agent integration against the full inventory. |
| `3.8` | `3.7` | controller; serial shared files | Qualify explicit opt-in activation and rollback without changing default startup. |
| `4.1` | `3.8` | controller; serial shared files | Reconcile zero-gap inventory, real-provider/integration evidence and guides. |
| `4.2` | `4.1` | controller; serial shared files | Run complete Rust, Go, audit and OpenSpec stage gates on the recorded SHA. |

## Dispatch and acceptance procedure

For the selected row, send only its exact packet, consumed upstream declarations, accepted prerequisite SHAs, read-only oracle paths and first failing case. Execute its test cycle in this order: add the specified assertion; run and capture behavioral failure after contract compilation; implement the named provider/algorithm; rerun the selected nonempty target and named compatibility oracle; independently review the deliverable; integrate and rerun affected consumer gates; commit only owned passing files. Fill `{node, baseline_sha, contract_sha, editable_files, read_only_files, fixture_sha256, red_command/result, green_command/result, discovered/executed_cases, derived_refresh, integration_sha, rollback}` in the controller-owned ledger.

The contract gate executes declarations/constructors and a success/failure double. Provider acceptance executes real code against the packet's success, failure, ordering and boundary cases. Integration acceptance executes real producer and consumer together. Each has a separate ledger result. A contract discrepancy pauses only affected consumers while the controller revises declarations, design, packets and tests and records a new accepted SHA.

The shared edit lease includes workspace/lockfiles, crate exports, test roots, fixture/corpus indexes, Godot bridge/host/catalogs, shared runners, release manifests, Makefile and CI. Acquire it serially, reread merged files and refresh their derived consumers before release. Provider rollback reverts only its files; a contract rollback includes all consumers of that identity. Stage closure runs the full gates specified in the original packet, including formatting, six-module vet via `make dev-check`, `make test-race`, audit and strict OpenSpec validation. Stop at focused gates inside editing loops unless a new failure warrants more.
