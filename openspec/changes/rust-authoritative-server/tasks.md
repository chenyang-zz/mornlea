# F2 Rust authoritative server implementation

All nodes are pending. [Execution contract and dependency graph](plans/00-execution.md) plus [exact worker packets](plans/01-server-slices.md) are part of this plan. These new crate/tests are prospective until their owning nodes land; `-- --list` alone is never provider acceptance. Record the accepted contract SHA, fixture identity, behavioral red/green and focused commit for each node in `ledger.md`. A worker edits only packet-owned files. F1 complete acceptance is a hard prerequisite; the current Go server remains the production authority until the separate product cutover.

See the [cross-change dispatch map](../godot-default-client-switch/plans/00-cross-change-dispatch.md) for parallel lanes and serial gates.

## 1. Contract and lifecycle

- [ ] 1.1 Verify complete F1 zero-gap acceptance and enumerate supported server capabilities.
- [ ] 1.2 Land compiling S1 contract, bounded types and executing consumer double; freeze its SHA.
- [ ] 1.3 Implement session admission, sequenced intake and control-plane separation.
- [ ] 1.4 Implement bounded tick/chunk mailboxes and cancellation.
- [ ] 1.5 Implement owned publication, slow-receiver policy and shutdown report.

## 2. Independent authoritative rule providers

- [ ] 2.1 Implement chunk acquisition and world edits with stale-generation rejection.
- [ ] 2.2 Implement time, season, weather and environment transition replay.
- [ ] 2.3 Implement bounded fluid rescan and update scheduling.
- [ ] 2.4 Implement farming, random ticks and block-support sweep ordering.
- [ ] 2.5 Implement player movement, survival validation and correction observations.
- [ ] 2.6 Implement inventory, containers, crafting, furnaces and item conservation.
- [ ] 2.7 Implement hostile combat, projectiles and deterministic deaths.
- [ ] 2.8 Implement passives, drops and sleeping.
- [ ] 2.9 Implement revalidated companion actions through the normal command path.

## 3. Adapters and serial integration

- [ ] 3.1 Integrate the one authoritative tick reducer after all rule providers.
- [ ] 3.2 Land the common protocol/login/validation transport path.
- [ ] 3.3a Implement the Memory adapter over common admission.
- [ ] 3.3b Implement the TCP adapter over common admission.
- [ ] 3.4 Implement bounded store mailbox and durable/failure reporting.
- [ ] 3.5 Implement exclusive world lease, recovery and named-backup rollback.
- [ ] 3.6 Implement bounded loopback Agent adapter and candidate revalidation.
- [ ] 3.7 Prove real local/remote, save/restart and Agent integration against the full inventory.
- [ ] 3.8 Qualify explicit opt-in activation and rollback without changing default startup.

## 4. Closeout

- [ ] 4.1 Reconcile zero-gap inventory, real-provider/integration evidence and guides.
- [ ] 4.2 Run complete Rust, Go, audit and OpenSpec stage gates on the recorded SHA.
