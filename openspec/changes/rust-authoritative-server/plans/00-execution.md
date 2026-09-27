# F2 execution contract and dispatch graph

This file is part of the active F2 plan. `tasks.md` is the sole status source. [Target rows D0/P0/S0/K0/S1–S4](../../../../docs/runtime-interface-architecture.md) and the F2 delta spec control behavior. The archived [Rust protocol completion packets](../../archive/2026-09-24-rust-protocol-completion/plans/03-server.md) and [storage closure packets](../../archive/2026-09-25-rust-storage-codec-closure/plans/05-dispatch-slices.md) supply the precedent for one source-bound case, a real negative, an independent provider gate, and a serial registry/adapter owner. They do not authorize using an archived planning status as current runtime evidence.

## Readiness and ownership

F1 is sealed at implemented source `d042982d33bb1694d768b75b01c297bd02534a08`, corpus `sha256:8b5813ecce2fe866ab1c4786a35925e8aadd9aab476196abb189d3c72d4f28f3` (112 points/1434 cases/zero gaps). F2 1.1a verifies that seal; no F2 provider is implemented or accepted. The new server crate does not exist yet. Node 1.2 lands the compiling S1 declaration/consumer-double surface before any rule or transport worker. Record its accepted commit SHA in `ledger.md`; every later brief must cite that SHA at dispatch. A plan file or test-double pass is not a real server. The controller exclusively edits `packages/engine/Cargo.toml`, `Cargo.lock`, `mornlea_server/src/lib.rs`, `src/core/mod.rs`, `src/rules/mod.rs`, the shared test entry files, fixture inventory and any wire/save version file. Workers own only the disjoint files named in their packet; a worker finding an API conflict reports the exact failing case and source instead of editing a shared declaration.

The first implementation keeps Go as the production authority. F2 test worlds are disposable copies and never opened by both runtimes. `mornlea_server` depends only on F1 Rust crates and permitted std/runtime libraries; Godot, client core and Python are excluded. `mornlea_domain::{CommandEnvelope,Event,RoutedEvent}` and `mornlea_protocol::{AdmittedLogin,PlayIntent}` are existing validated types. A packet conversion never assigns tick/session/arrival. Domain `Event` never contains hello/login/keepalive/disconnect.

## S1 contract to land in node 1.2

The owner creates `src/core/contracts.rs` with checked private-field value types and `src/core/state.rs` with an opaque single-threaded authority state. The planned public shape below and the [complete rule/store seams](02-core-seams.md) are accepted together in 1.2. A test double implements `ServerEndpoint`; the initial real provider lands in 1.3. Type and error names are owned here, not by rule workers.

```rust
pub struct SessionKey(NonZeroU64);
pub enum TransportKind { Memory, Tcp }
pub struct ServerLimits {
    max_players: u8,            // default 8, valid 1..=8
    queued_commands: usize,     // proposed maximum 4096 records
    session_outbox: usize,      // default 512 frames
    ready_chunk_results: usize, // proposed maximum 64 records
    snapshot_chunks: usize,     // default 64 per publication step
    snapshot_bytes: usize,      // default 1 MiB per publication step
}
pub struct TickBudget {
    commands: usize,            // 0..=4096 admitted commands
    fluid_updates_per_dimension: usize, // 0..=512 per dimension
    fluid_rescan_target_per_dimension: usize, // 0..=65536 target, section boundary
    farmland_checks: usize,     // 0..=65536
    farmland_block_reads: usize,// 0..=65536
}
pub struct Deadline(Instant); // process-local monotonic clock
pub enum SubmissionReceipt { QueuedForTick { tick: u64, arrival_index: u64 }, ControlAccepted }
pub trait ServerEndpoint {
    fn admit(&mut self, login: AdmittedLogin, transport: TransportKind) -> Result<SessionKey, ServerError>;
    fn submit(&mut self, session: SessionKey, intent: PlayIntent) -> Result<SubmissionReceipt, ServerError>;
    fn submit_companion(&mut self, candidate: CompanionActionEnvelope) -> Result<CompanionReceipt, ServerError>;
    fn advance_tick(&mut self, work: TickBudget) -> Result<TickPublication, ServerError>;
    fn close_session(&mut self, session: SessionKey, reason: CloseReason) -> Result<(), ServerError>;
    fn shutdown(&mut self, deadline: Deadline) -> Result<ShutdownReport, ShutdownFailure>;
}
```

The comments above document **planned contract fields**; production source comments added by implementation workers use English and never mention task IDs. The exact error variants, core state ports, rule effects/getters, receipt/order policy, S2 connection state machine, S3 ownership/acknowledgment and S4 lifecycle are defined in [02-core-seams.md](02-core-seams.md). That file is the single declaration authority; node1.2 lands it with consumer doubles. `SessionKey` is nonzero, monotonic and never reused. Constructor failure precedes allocation/mutation. Defaults8 players/512 outbox/64 snapshot chunks/1MiB snapshot publication come from Go;4096 command/64 ready-result and retained-save ceilings are Rust target proposals, subject to measured supported-run fit in1.1b/c. Go ingress has separate256 command/chat channels before its unbounded simulation inbox. Pending handshakes16 are S2-owned before login.

`submit` validates session/phase/payload/capacity, then assigns earliest-eligible tick and per-session arrival identity. It never advances applied sequence at arrival. Freeze eligible batch, sort, process budgeted prefix, preserve original identity on carry; stale/duplicate applied sequences are silent no-effect. Example arrivals9/select2,9/place,8/select1 execute8,first9 and discard second9 without CommandRejected. QueuedForTick is admission only. Chat/keepalive stay control plane. All internal-to-wire/close decisions are fixed in02; no new v45 rejection reason is introduced. Tick never waits for disk/socket/Agent/GPU/Python.

**Go parity order fixed for the serial reducer:** snapshot tunables/climate; assign environment tick; drain commands/chunk/companion inboxes; order and reject stale commands; player command phase; companion actions; apply acquired/generated chunks; player/companion physics; **subscription reconciliation and refreshed views immediately after actor advancement**; hostile combat/projectiles/deaths; passives; companion placements and ordered interactions; sleep/drops/furnaces; fluid boundary rescan and updates; farmland moisture; tramples; snow footprints; crop random ticks; container moves/mining/workbench; support sweeps for plants/saplings/torches/beds; commit mutations; publish entity/inventory state; advance tick/time/weather/season. The source of this sequence is `packages/server/sim/runtime/engine_step.go` plus `entity/tick.go` and `realm/environment.go`; `engine_step_phase_test.go` guards the important phase edges. The reducer in node 3.1 owns this one serial sequence. Rule modules supply phase operations and never edit `src/core/step.rs` concurrently.

## Common worker packet procedure

For every node in [01-server-slices.md](01-server-slices.md), the worker receives only its named packet, this execution contract, the accepted S1 SHA and the named read-only Go source. It must: (1) add the packet's exact success, negative and limit cases to its exclusive test module; (2) run the listed filter and capture a **behavioral** red after the contract compiles; (3) implement the stated provider in its exclusive source file; (4) run the nonempty focused Rust target and the cited Go oracle test; (5) report source SHA, fixture identity, discovered/executed count, overflow/error behavior and rollback; (6) let the controller review and make the scoped commit. A missing function import or a `-- --list` result alone is not a behavioral red/green. `cargo test` commands always use `rustup run 1.97.1 cargo test --manifest-path packages/engine/Cargo.toml -p mornlea_server --locked` with the packet's `--test` and filter suffix. `tests/server_replay.rs`, `tests/server_contract.rs`, `tests/persistence_failure.rs` and `tests/local_remote_parity.rs` are controller-owned entry files that import file-per-node modules; workers only edit their own modules.

## Cross-change graph

```text
complete F1 SHA → F2 1.1 inventory → F2 1.2 S1 contract
                                      ├─ 1.3–1.5 bounded authority lifecycle; serial 1.6 real mutation transaction
                                      ├─ 2.x disjoint rule providers (2.1b/2.9b after 1.6) → 3.1 serial tick reducer
                                      ├─ 3.2 shared S2 → 3.3a/3.3b parity → accepted S2 SHA → F3 1.1/1.2
                                      ├─ 3.4a/b S3 scheduling + 3.4c/d real disk backends → 3.5 lease/recovery
                                      └─ 3.6a/b/c S4 HTTP/lease/MCP/memory → 3.6d real Python integration
                                    → 3.7 real local/remote/save/Agent integration
                                    → 3.8 opt-in activation → F2 4.x full acceptance → F3 3.4 real integration
```

F2 3.1 consumes the rule modules and therefore follows their provider tests even where modules have no file overlap. The transport, persistence and Agent adapters may be built against S1 doubles while rules run, but final F2 integration is serial. S2 is accepted only after common validation and both real Memory/TCP adapter parity cases pass on one SHA; 3.2 alone is insufficient. The tool/test registry, corpus manifest, crate exports and activation/rollback owner are serial regardless of edit disjointness. F3 may consume the accepted S2 contract before full F2 completion, but F3 real integration cannot close without full F2 parity.

## Refinement precedence

Read [direct accepted predecessors](03-parallel-readiness.md) and [refined node decisions](04-refined-nodes.md) with this packet. Split parent IDs are retained here only as historical grouping; their child packets own execution. The dependency register controls readiness, and refined source mappings control absent fields. No worker infers a missing signature, capacity, source fact or shared-file edit.
