# F2 Rust authoritative server implementation

The checkboxes below are the sole current node-status source. [Execution contract and dependency graph](plans/00-execution.md), [compile-ready S1 seams](plans/02-core-seams.md) and [exact worker packets](plans/01-server-slices.md) are part of this plan. These new crate/tests are prospective until their owning nodes land; `-- --list` alone is never provider acceptance. Record the accepted contract SHA, fixture identity, behavioral red/green and focused commit for each node in `ledger.md`. A worker edits only packet-owned files. F1 complete acceptance is a hard prerequisite; the current Go server remains the production authority until the separate product cutover.

[Controller algorithms and concrete acceptance tables](plans/04-refined-nodes.md) cover every command and rule branch. The node graph includes separate real filesystem and Agent lifecycle/integration owners.

See the [node-by-node dependency and ownership gate](plans/03-parallel-readiness.md) before dispatch. It refines the linked packets without duplicating status.

See the [cross-change dispatch map](../godot-default-client-switch/plans/00-cross-change-dispatch.md) for parallel lanes and serial gates.

## 1. Contract and lifecycle

- [x] 1.1a Verify the sealed complete F1 prerequisite and enumerate server capabilities.
- [x] 1.1b Record command, chunk-result and persistence high-water from every supported server replay.
- [x] 1.1c Freeze S1 bounds and source-bound capability-to-test mappings before the contract landing.
- [x] 1.2 Land compiling S1 contract, bounded types and executing consumer double; freeze its SHA.
- [x] 1.3 Implement session admission, sequenced intake and control-plane separation.
- [x] 1.4 Implement bounded tick/chunk mailboxes and cancellation.
- [x] 1.5a Implement owned tick publication and bounded slow-receiver outboxes.
- [x] 1.5b Implement one final unpublished tick and retryable shutdown phases.
- [x] 1.6 Implement authority-resolved atomic placement/mining transaction and failure invariants.

## 2. Independent authoritative rule providers

- [x] 2.1a Implement chunk acquisition and stale-generation rejection.
- [x] 2.1b Implement world placement/internal interaction geometry through the atomic transaction.
- [x] 2.1c Implement continuous mining progress, tool reset and atomic completion.
- [x] 2.2 Implement time, season, weather and environment transition replay.
- [x] 2.3 Implement bounded fluid rescan and update scheduling.
- [x] 2.4a Implement bounded farmland moisture.
- [x] 2.4b Implement actor trample and snow footprints.
- [x] 2.4d0 Complete indexed Ready world reads, height cache and compact replay snapshots.
- [x] 2.4d Implement deterministic crop, dry farmland, tree, grass and snow random rules.
- [x] 2.4c Implement ordered block-support sweeps ([exact packet](plans/11-support-sweeps.md)).
- [x] 2.5a Implement authoritative player control and movement.
- [x] 2.5b Implement survival transitions and correction observations.
- [x] 2.5c Implement atomic eating inventory and hunger settlement.
- [x] 2.6a Implement inventory authority and item conservation.
- [x] 2.6b Implement containers and generation/view validation.
- [x] 2.6c Implement atomic workbench crafting.
- [ ] 2.6c1 Complete workbench anchor lifecycle and container-view mutual exclusion.
- [x] 2.6d Implement tick-driven furnaces.
- [x] 2.6e0 Add bounded tick-local mining suppression and action receipt preflight.
- [x] 2.6e Implement farming tools, bone meal and atomic buckets.
- [x] 2.7a Implement hostile lifecycle and motion provider.
- [ ] 2.7a1 Complete frozen pre-physics hostile targeting, melee and ranged action production.
- [x] 2.7b0 Complete projectile read, compare-and-replace staging and replay initialization contracts.
- [x] 2.7b Implement projectiles and hit validation.
- [ ] 2.7c0 Land frozen hostile melee input contract ([packet](plans/17-combat-input-contract.md)).
- [ ] 2.7c1 Implement bounded melee snapshot and settlement.
- [ ] 2.7c2 Complete player, hostile and passive death outputs and reset.
- [ ] 2.7c Implement one-time hostile death and combat outcomes.
- [x] 2.8a Implement passive lifecycle.
- [x] 2.8b0 Complete bounded drop staging, identity and snapshot contracts.
- [x] 2.8b1 Implement bounded drop lifetime and atomic pickup.
- [x] 2.8b2 Complete fixed container ownership and atomic world outputs.
- [x] 2.8b3a Land shared deterministic harvest samplers ([contract packet](plans/12-harvest-contract.md)).
- [x] 2.8b3b Complete flooded-plant and trample drop outputs ([packet](plans/13-environment-outputs.md)).
- [x] 2.8b3c Complete human mining harvest and no-drop branches ([packet](plans/14-mining-outputs.md)).
- [x] 2.8b4a Implement selected-item and inventory/crafting panel drops ([packet](plans/15-player-drop-commands.md)).
- [ ] 2.8b4b Complete container view binding and panel drop settlement ([packet](plans/16-container-drop-views.md)).
- [ ] 2.8b4 Implement selected-item and panel drop commands.
- [ ] 2.8b Complete drop commands and all producer integration.
- [x] 2.8c Implement sleeping and time transition.
- [x] 2.9a Implement sessionless companion candidate provenance and admission.
- [ ] 2.9b Revalidate and execute companion actions through the shared mutation pipeline.

## 3. Adapters and serial integration

- [ ] 3.1 Integrate the one authoritative tick reducer after all rule providers, including the refined packet's retired-session filtering, explicit damage-victim routing and indexed observation gates.
- [x] 3.2 Land the common protocol/login/validation transport path.
- [x] 3.3a Implement the Memory adapter over common admission.
- [x] 3.3b Implement the TCP adapter over common admission.
- [x] 3.4a Implement bounded durable store mailbox.
- [x] 3.4b Implement autosave, retry, backpressure and flush scheduling.
- [x] 3.4c0 Complete decoded-load, partial-commit and cancellable persistence I/O contracts.
- [x] 3.4c Implement real region commit, crash boundaries and compaction.
- [x] 3.4d Implement standalone atomic file persistence.
- [x] 3.5 Implement exclusive world lease, recovery and named-backup rollback.
- [x] 3.6a Implement Agent HTTP wire and lease provider.
- [ ] 3.6b Implement frozen snapshot registry and MCP provider.
- [ ] 3.6c Implement Agent task, dialogue and memory ownership.
- [ ] 3.6d Execute Rust integration with the actual Python Agent and MCP.
- [ ] 3.7 Prove real local/remote, save/restart and Agent integration against the full inventory.
- [ ] 3.8 Qualify explicit opt-in activation and rollback without changing default startup.

## 4. Closeout

- [ ] 4.1 Reconcile zero-gap inventory, real-provider/integration evidence and guides.
- [ ] 4.2 Run complete Rust, Go, audit and OpenSpec stage gates on the recorded SHA.
