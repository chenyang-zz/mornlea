# F3 Rust client core and Godot bridge implementation

All 52 nodes are complete at runtime source `2cea2020091168ea5a6845517bc49ccbdbae0268`. The [completion handoff](completion-handoff.md) binds independent core, actual-provider/inventory and native/host reviews to complete source-bound Rust, Go, audit, OpenSpec and Darwin runtime gates. Historical code-only and failed qualification rounds remain in `ledger.md` and the retained evidence. The [C1/C2/G1 contract and graph](plans/00-client-contract.md), [typed family schema](plans/02-family-schemas.md), [worker packets](plans/01-client-slices.md), [runtime packet](plans/17-runtime-qualification.md) and [final repair packet](plans/18-completion-review-repairs.md) preserve scope and acceptance. The Go pilot remains explicitly selectable until the separate product cutover.

See the [node-by-node dependency and ownership gate](plans/03-parallel-readiness.md) before dispatch. It refines the linked packets without duplicating status.

See the [cross-change dispatch map](../godot-default-client-switch/plans/00-cross-change-dispatch.md) for parallel lanes and serial gates.

## 1. C1 contract and session

- [x] 1.1 Bind F1/F2 accepted prerequisites and enumerate all client semantic families.
- [x] 1.2 Land compiling C1/C2 contract, typed family schema, frame validator and executing consumer double.
- [x] 1.3 Implement login/session observation state machine.
- [x] 1.4 Implement confirmed mirror and atomic observation order.
- [x] 1.5 Implement bounded shared Memory/TCP I/O queues.

## 2. Parallel input, prediction, preparation and family providers

- [x] 2.1 Implement semantic typed input, UI token and local sequence validation.
- [x] 2.2 Implement reversible prediction and authoritative correction replay.
- [x] 2.3 Implement bounded preparation scheduling and stale-result rejection.
- [x] 2.3b Implement bounded far-tile LOD preparation and stale completion cancellation.
- [x] 2.4 Assemble and atomically publish validated immutable frames.
- [x] 2.5 Publish `terrain@1` semantics.
- [x] 2.6a Project remote players into `actors@1`.
- [x] 2.6c Project hostiles into `actors@1`.
- [x] 2.6d Project passives into `actors@1`.
- [x] 2.6e Project projectiles into `actors@1`.
- [x] 2.6f1 Project companions into `actors@1`.
- [x] 2.6f2 Project item drops into `actors@1`.
- [x] 2.6g Assemble the complete `actors@1` family.
- [x] 2.6b Publish `player-view@1` semantics.
- [x] 2.7a1 Project inventory and hotbar state.
- [x] 2.7a2 Project containers and chest revision.
- [x] 2.7a3 Project crafting state.
- [x] 2.7a4 Project furnace state.
- [x] 2.7a5 Assemble `inventory-ui@1`.
- [x] 2.7b1 Project environment state.
- [x] 2.7b2 Project survival state.
- [x] 2.7b3 Project chat state.
- [x] 2.7b4 Project task state.
- [x] 2.7b5 Project prompts.
- [x] 2.7b6 Assemble `world-ui@1`.
- [x] 2.8 Publish provenance-aware `audio-cues@1` semantics.
- [x] 2.9 Publish `diagnostics@1` semantics.

## 3. Serial adapter and real integration

- [x] 3.1a Assign and validate G1 logical-to-numeric descriptors without enabling features.
- [x] 3.1b Connect the safe Rust core adapter and migrate Godot-callable methods serially.
- [x] 3.2 Implement symbolic family negotiation and one-session feature host activation.
- [x] 3.3a Implement core reset, reconnect and close with queued-work invalidation.
- [x] 3.3b Implement native Godot/Python release ordering and boundary panic containment.
- [x] 3.3c Qualify rebuilt Rust producer artifacts through 100 real headless session cycles.
- [x] 3.4 Integrate real F2 Memory/TCP server, C1/C2 and G1 against all accepted families.

## 4. Closeout

- [x] 4.1 Reconcile zero-gap inventory, real provider/integration cases and guides.
- [x] 4.2 Run complete Rust, Go, audit and OpenSpec stage gates on the recorded SHA.


## Completed execution and qualification boundary

The user authorized the outstanding headless runtime gates on 2026-10-11 and completion, global review and local dev merge on 2026-10-13. Packet17 qualification and packet18 repairs now have actual passing prescribed evidence. Original native, real-provider and full-stage criteria were not replaced by code-only checks. The earlier [code-only packet](plans/07-code-only-implementation-handoff.md) and [repair handoff](review-repair-handoff.md) remain historical. F2's separate gameplay executable and P8–P14 product work remain independently owned; production defaults are unchanged.

## 5. Comprehensive-review repairs

These nodes closed the earlier code-repair scope. The original qualification nodes above now close with the separate actual runtime evidence in the completion handoff.

- [x] 5.1 Restore exact G1 value schemas, bounded native input conversion and fail-closed host frame admission.
- [x] 5.2 Repair actual session lifecycle, observation capacity handling, retained bytes, work limits and crafting attribution.
- [x] 5.3 Repair cross-frame actor retirement and failed prediction replay atomicity.
- [x] 5.4 Repair terrain admission, resource retention, request identity and bounded preparation metadata.
- [x] 5.5 Stage terrain references until complete-frame commit and reject stale or incomplete publications.
- [x] 5.6 Build real bounded near/far CPU geometry and expose the owning core's resource arena.
- [x] 5.7 Bound audio provenance history and derive snow cues from actual prediction source facts.
- [x] 5.8 Compose the real endpoint, confirmed block ownership, input/keepalive output, prediction and all ten families.
- [x] 5.9 Run repaired provider/integration regressions, independent review and aggregate code gates; reconcile remaining qualification status.

## 6. Completion-review repairs

[18-completion-review-repairs.md](plans/18-completion-review-repairs.md) resolves the original-contract defects found during the user-authorized final global review.

- [x] 6.1 Bound retained live mirrors and suppress stale authoritative source facts before projection.
- [x] 6.2 Report stable Rust publication failures through bounded host status.
