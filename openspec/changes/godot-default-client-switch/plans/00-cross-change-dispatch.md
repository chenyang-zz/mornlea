# Cross-change parallel dispatch map

This is an index over the active OpenSpec `tasks.md` files, not a second status source. It applies the [target interface contract](../../../../docs/runtime-interface-architecture.md) and the archived protocol/storage worker-packet precedent. Detailed editable files, tests and rollback for a node live in that node's change-local `plans/` packet; the controller verifies its prerequisites at dispatch. All implementation checkboxes remain open. A packet is only dispatchable after its compile-ready upstream declaration, deterministic consumer double and accepted SHA actually exist; a plan file cannot supply that SHA.

| Stage and status source | Open nodes | Detailed packet entry |
| --- | ---: | --- |
| [F1 numerical closure](../../rust-native-numerical-closure/tasks.md) | 29 | [Foundation packets](../../rust-native-numerical-closure/plans/00-foundation.md) and its linked provider/adapter/closure packets |
| [F2 authoritative server](../../rust-authoritative-server/tasks.md) | 25 | [S1 contract](../../rust-authoritative-server/plans/00-execution.md), [server slices](../../rust-authoritative-server/plans/01-server-slices.md) |
| [F3 client core and bridge](../../rust-client-core/tasks.md) | 22 | [C1/C2/G1 contract](../../rust-client-core/plans/00-client-contract.md), [client slices](../../rust-client-core/plans/01-client-slices.md) |
| [P8 terrain](../../godot-production-terrain/tasks.md) | 12 | [Terrain packets](../../godot-production-terrain/plans/worker-packets.md) |
| [P9 actors](../../godot-complete-actors/tasks.md) | 15 | [Actor packets](../../godot-complete-actors/plans/worker-packets.md) |
| [P10 UI](../../godot-ui-migration/tasks.md) | 12 | [UI packets](../../godot-ui-migration/plans/worker-packets.md) |
| [P11 desktop audio/input](../../godot-desktop-audio/tasks.md) | 11 | [Desktop packets](../../godot-desktop-audio/plans/worker-packets.md) |
| [P12 tooling/evidence](../../godot-production-tooling/tasks.md) | 14 | [Tooling packets](../../godot-production-tooling/plans/worker-packets.md) |
| [P13 packaging](../../godot-desktop-packaging/tasks.md) | 13 | [Release packets](../../godot-desktop-packaging/plans/worker-packets.md) |
| [P14 cutover](../tasks.md) | 13 | [Cutover packets](worker-packets.md) |

The nine revised changes contain 137 nodes; the existing F1 change has 29, for 166 open implementation nodes in this program. These counts are planning inventory, not completion evidence.

| Wave | Serial landing / accepted gate | Concurrent disjoint lanes after the gate | Serial join / acceptance |
| --- | --- | --- | --- |
| F1 numerical closure | Existing F1 1.1/1.2, then complete D0/P0/S0/K0 zero-gap acceptance | Existing F1 native numerical providers and adapters under its own DAG | F1 3.12 corpus and 4.x; no F2 worker before complete F1 ledger |
| F2 S1 | F2 1.1 measured inventory, 1.2 compiling S1 types/double | F2 1.3–1.5 session/mailbox/publication; 2.1–2.9 disjoint rule modules; 3.2 transport common, 3.4 store, 3.6 Agent with doubles | F2 3.1 one tick reducer; 3.3a/3.3b transport parity; 3.5 recovery; 3.7 real integration; 3.8 opt-in activation; 4.x |
| F3 C1/C2 | Accepted F2 S2 and F3 1.1 measured inventory/1.2 compiling C1/C2 types/double | F3 1.3–1.5 session/mirror/I/O; 2.1–2.3 input/prediction/preparation as dependencies permit; 2.5–2.9 independent family providers | F3 2.4 one atomic frame owner, 3.1 one G1 registry/adapter owner, 3.2 host, 3.3 lifecycle, 3.4 real F2 integration (after full F2), 4.x |
| Godot features | Accepted F3 family schema/G1 descriptors; P8–P11 each land inventory and test harness | P8 terrain modules; P9 actor-kind scenes; P10 UI Controls and typed intent; P11 audio/input/device lifecycle; disjoint feature directories and test modules | Each change's 3.1 real integration/catalog owner, then candidate evidence and 4.x |
| T1 evidence | P12 1.2 schema/double after F3, then 2.4 accepted capture/report/dispatcher | P12 2.1–2.3 validator/capture/report; P8–P11/P13 candidate captures after phase 2; P12 3.3/3.4 independent tools | P12 3.1 handoff implementation and 3.2 reviewed per-case transfer; no tracked write without explicit approval |
| Desktop release | F2/F3 real integration and P13 1.1/1.2 manifest/test seam | P13 2.1 supervisor, 2.2 launch UI, 2.3 resolver; after 2.4 macOS/Windows/Linux qualification on separate hosts | P13 2.4 release harness, 3.4 rollback, 4.x; P14 1.x prerequisites |
| Product cutover | P14 1.1/1.2 gate, 1.3 audit tests; P13 target reports and P12 approved ownership | P14 2.1 native diagnostic and 2.2 closure checker | P14 cycle 1, distinct cycle 2, explicit approval, default switch, separate Bootstrap/pilot/renderer/Go edge retirements, 4.x |

The F2 S2 protocol/session contract can unblock F3 C1 planning/provider work before F2 full implementation. F3 real integration and P13 cannot close until complete F2 parity. P12 phase 2 unblocks P8–P11 candidate evidence before P12 phase 3 handoffs, avoiding a cycle. P8–P11 feature work may run in parallel because the F3 family schema/registry is frozen first and feature edits stay within separate directories. `mornlea_godot` registry, Godot host, common test roots, catalogs, release manifests, visual producer registry, tracked baselines, Makefile/CI and default launch each have one serial editor. At most three independent workers run at once, per root guidance.

## Dispatch record for a lower-capability worker

The controller sends one packet with: (1) its exact node ID and accepted prerequisite SHAs; (2) the files in that packet's exclusive ownership and named read-only oracle; (3) exact existing type declarations from the accepted contract; (4) first behavioral red input/expected result and focused command; (5) bounded algorithm/order/error policy and +1 limit case; (6) real-provider green and cross-boundary integration dependency; (7) exclusion, guide update, scoped commit and rollback unit. A worker does not infer a missing type, numeric descriptor, producer owner, test helper or capacity from this table. If the accepted contract differs from the planned signature, the controller revises all consuming packets before dispatch. The controller retains one ledger/status owner per change and records whether architecture-skill promotion has verified current code/test support.

## Initial runnable frontier

As of this planning revision the Rust server and client-core crates do not exist, and F1 has not completed its zero-gap gate. The first implementation frontier is the already detailed F1 numerical-closure 1.1 inventory, followed by its 1.2 compile-ready native contract. The F2/F3 and Godot packets describe subsequent work; they become runnable only when their named prerequisite SHA and real test harness exist. This prevents a lower-capability worker from treating a future API sketch as present code.
