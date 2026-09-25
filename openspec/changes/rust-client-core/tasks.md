# F3 Rust client core and Godot bridge implementation

All nodes are pending. [C1/C2/G1 contract and graph](plans/00-client-contract.md) and [worker packets](plans/01-client-slices.md) define prerequisites, exact exclusive files, cases and commands. New crate/test targets are prospective until their owning nodes land. `ledger.md` records accepted contract SHA, fixtures, red/green and scoped commits. The current Go pilot remains explicitly selectable for rollback until the separate product cutover.

See the [cross-change dispatch map](../godot-default-client-switch/plans/00-cross-change-dispatch.md) for parallel lanes and serial gates.

## 1. C1 contract and session

- [ ] 1.1 Bind F1/F2 accepted prerequisites and enumerate all client semantic families.
- [ ] 1.2 Land compiling C1/C2 contract, validated frame and executing consumer double.
- [ ] 1.3 Implement login/session observation state machine.
- [ ] 1.4 Implement confirmed mirror and atomic observation order.
- [ ] 1.5 Implement bounded shared Memory/TCP I/O queues.

## 2. Parallel input, prediction, preparation and family providers

- [ ] 2.1 Implement typed input translation and local sequence validation.
- [ ] 2.2 Implement reversible prediction and authoritative correction replay.
- [ ] 2.3 Implement bounded preparation scheduling and stale-result rejection.
- [ ] 2.4 Integrate atomic immutable frame validation and publication.
- [ ] 2.5 Publish `terrain@1` semantics.
- [ ] 2.6a Publish `actors@1` semantics.
- [ ] 2.6b Publish `player-view@1` semantics.
- [ ] 2.7a Publish `inventory-ui@1` semantics.
- [ ] 2.7b Publish `world-ui@1` semantics.
- [ ] 2.8 Publish `audio-cues@1` semantics.
- [ ] 2.9 Publish `diagnostics@1` semantics.

## 3. Serial adapter and real integration

- [ ] 3.1 Assign G1 registry descriptors and land the safe Rust Godot adapter.
- [ ] 3.2 Implement symbolic family negotiation and one-session feature host activation.
- [ ] 3.3 Implement reset/reconnect/teardown and explicit 100-cycle Rust session smoke.
- [ ] 3.4 Integrate real F2 Memory/TCP server, C1/C2 and G1 against all accepted families.

## 4. Closeout

- [ ] 4.1 Reconcile zero-gap inventory, real provider/integration cases and guides.
- [ ] 4.2 Run complete Rust, Go, audit and OpenSpec stage gates on the recorded SHA.
