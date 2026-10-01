# F3 Rust client core and Godot bridge implementation

All nodes are pending. [C1/C2/G1 contract and graph](plans/00-client-contract.md), [typed family schema](plans/02-family-schemas.md) and [worker packets](plans/01-client-slices.md) define prerequisites, exact exclusive files, cases and commands. New crate/test targets are prospective until their owning nodes land. `ledger.md` records accepted contract SHA, fixtures, red/green and scoped commits. The current Go pilot remains explicitly selectable for rollback until the separate product cutover.

See the [node-by-node dependency and ownership gate](plans/03-parallel-readiness.md) before dispatch. It refines the linked packets without duplicating status.

See the [cross-change dispatch map](../godot-default-client-switch/plans/00-cross-change-dispatch.md) for parallel lanes and serial gates.

## 1. C1 contract and session

- [x] 1.1 Bind F1/F2 accepted prerequisites and enumerate all client semantic families.
- [ ] 1.2 Land compiling C1/C2 contract, typed family schema, frame validator and executing consumer double.
- [ ] 1.3 Implement login/session observation state machine.
- [ ] 1.4 Implement confirmed mirror and atomic observation order.
- [ ] 1.5 Implement bounded shared Memory/TCP I/O queues.

## 2. Parallel input, prediction, preparation and family providers

- [ ] 2.1 Implement semantic typed input, UI token and local sequence validation.
- [ ] 2.2 Implement reversible prediction and authoritative correction replay.
- [ ] 2.3 Implement bounded preparation scheduling and stale-result rejection.
- [ ] 2.3b Implement bounded far-tile LOD preparation and stale completion cancellation.
- [ ] 2.4 Assemble and atomically publish validated immutable frames.
- [ ] 2.5 Publish `terrain@1` semantics.
- [ ] 2.6a Project remote players into `actors@1`.
- [ ] 2.6c Project hostiles into `actors@1`.
- [ ] 2.6d Project passives into `actors@1`.
- [ ] 2.6e Project projectiles into `actors@1`.
- [ ] 2.6f1 Project companions into `actors@1`.
- [ ] 2.6f2 Project item drops into `actors@1`.
- [ ] 2.6g Assemble the complete `actors@1` family.
- [ ] 2.6b Publish `player-view@1` semantics.
- [ ] 2.7a1 Project inventory and hotbar state.
- [ ] 2.7a2 Project containers and chest revision.
- [ ] 2.7a3 Project crafting state.
- [ ] 2.7a4 Project furnace state.
- [ ] 2.7a5 Assemble `inventory-ui@1`.
- [ ] 2.7b1 Project environment state.
- [ ] 2.7b2 Project survival state.
- [ ] 2.7b3 Project chat state.
- [ ] 2.7b4 Project task state.
- [ ] 2.7b5 Project prompts.
- [ ] 2.7b6 Assemble `world-ui@1`.
- [ ] 2.8 Publish provenance-aware `audio-cues@1` semantics.
- [ ] 2.9 Publish `diagnostics@1` semantics.

## 3. Serial adapter and real integration

- [ ] 3.1a Assign and validate G1 logical-to-numeric descriptors without enabling features.
- [ ] 3.1b Connect the safe Rust core adapter and migrate Godot-callable methods serially.
- [ ] 3.2 Implement symbolic family negotiation and one-session feature host activation.
- [ ] 3.3a Implement core reset, reconnect and close with queued-work invalidation.
- [ ] 3.3b Implement native Godot/Python release ordering and boundary panic containment.
- [ ] 3.3c Qualify rebuilt Rust producer artifacts through 100 real headless session cycles.
- [ ] 3.4 Integrate real F2 Memory/TCP server, C1/C2 and G1 against all accepted families.

## 4. Closeout

- [ ] 4.1 Reconcile zero-gap inventory, real provider/integration cases and guides.
- [ ] 4.2 Run complete Rust, Go, audit and OpenSpec stage gates on the recorded SHA.


## Current execution packet and qualification boundary

The approved complete code-only scope is detailed in [07-code-only-implementation-handoff.md](plans/07-code-only-implementation-handoff.md). It refines allowed code-level checks without removing original qualification gates. This file remains the sole checkbox/status source. No task is completed by this planning update. Native Godot marshalling/real bridge checks, 3.3c, 3.4, and full closeout remain pending; any node whose complete prescribed acceptance has not run remains unchecked. The running F2 implementation and P8–P14 product work are separately owned.
