# F2 Rust authoritative server implementation

All nodes are pending. [Execution contract and dependency graph](plans/00-execution.md), [compile-ready S1 seams](plans/02-core-seams.md) and [exact worker packets](plans/01-server-slices.md) are part of this plan. These new crate/tests are prospective until their owning nodes land; `-- --list` alone is never provider acceptance. Record the accepted contract SHA, fixture identity, behavioral red/green and focused commit for each node in `ledger.md`. A worker edits only packet-owned files. F1 complete acceptance is a hard prerequisite; the current Go server remains the production authority until the separate product cutover.

See the [node-by-node dependency and ownership gate](plans/03-parallel-readiness.md) before dispatch. It refines the linked packets without duplicating status.

See the [cross-change dispatch map](../godot-default-client-switch/plans/00-cross-change-dispatch.md) for parallel lanes and serial gates.

## 1. Contract and lifecycle

- [ ] 1.1a Verify the sealed complete F1 prerequisite and enumerate server capabilities.
- [ ] 1.1b Record command, chunk-result and persistence high-water from every supported server replay.
- [ ] 1.1c Freeze S1 bounds and source-bound capability-to-test mappings before the contract landing.
- [ ] 1.2 Land compiling S1 contract, bounded types and executing consumer double; freeze its SHA.
- [ ] 1.3 Implement session admission, sequenced intake and control-plane separation.
- [ ] 1.4 Implement bounded tick/chunk mailboxes and cancellation.
- [ ] 1.5a Implement owned tick publication and bounded slow-receiver outboxes.
- [ ] 1.5b Implement one final unpublished tick and retryable shutdown phases.
- [ ] 1.6 Implement authority-resolved atomic placement/mining transaction and failure invariants.

## 2. Independent authoritative rule providers

- [ ] 2.1a Implement chunk acquisition and stale-generation rejection.
- [ ] 2.1b Implement world placement/mining geometry through the atomic transaction.
- [ ] 2.2 Implement time, season, weather and environment transition replay.
- [ ] 2.3 Implement bounded fluid rescan and update scheduling.
- [ ] 2.4a Implement bounded farmland moisture.
- [ ] 2.4b Implement trample, snow and crop random ticks.
- [ ] 2.4c Implement ordered block-support sweeps.
- [ ] 2.5a Implement authoritative player control and movement.
- [ ] 2.5b Implement survival transitions and correction observations.
- [ ] 2.6a Implement inventory authority and item conservation.
- [ ] 2.6b Implement containers and chest revision validation.
- [ ] 2.6c Implement atomic workbench crafting.
- [ ] 2.6d Implement tick-driven furnaces.
- [ ] 2.7a Implement hostile lifecycle and targeting.
- [ ] 2.7b Implement projectiles and hit validation.
- [ ] 2.7c Implement one-time hostile death and combat outcomes.
- [ ] 2.8a Implement passive lifecycle.
- [ ] 2.8b Implement item drops and pickup.
- [ ] 2.8c Implement sleeping and time transition.
- [ ] 2.9a Implement sessionless companion candidate provenance and admission.
- [ ] 2.9b Revalidate and execute companion actions through the shared mutation pipeline.

## 3. Adapters and serial integration

- [ ] 3.1 Integrate the one authoritative tick reducer after all rule providers.
- [ ] 3.2 Land the common protocol/login/validation transport path.
- [ ] 3.3a Implement the Memory adapter over common admission.
- [ ] 3.3b Implement the TCP adapter over common admission.
- [ ] 3.4a Implement bounded durable store mailbox.
- [ ] 3.4b Implement autosave, retry, backpressure and flush scheduling.
- [ ] 3.5 Implement exclusive world lease, recovery and named-backup rollback.
- [ ] 3.6 Implement bounded loopback Agent adapter and candidate revalidation.
- [ ] 3.7 Prove real local/remote, save/restart and Agent integration against the full inventory.
- [ ] 3.8 Qualify explicit opt-in activation and rollback without changing default startup.

## 4. Closeout

- [ ] 4.1 Reconcile zero-gap inventory, real-provider/integration evidence and guides.
- [ ] 4.2 Run complete Rust, Go, audit and OpenSpec stage gates on the recorded SHA.
