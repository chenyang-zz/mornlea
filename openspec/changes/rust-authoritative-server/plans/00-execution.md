# F2 execution contract and dispatch graph

This file is part of the active F2 plan. `tasks.md` is the sole status source. [Target rows D0/P0/S0/K0/S1–S4](../../../../docs/runtime-interface-architecture.md) and the F2 delta spec control behavior. The archived [Rust protocol completion packets](../../archive/2026-09-24-rust-protocol-completion/plans/03-server.md) and [storage closure packets](../../archive/2026-09-25-rust-storage-codec-closure/plans/05-dispatch-slices.md) supply the precedent for one source-bound case, a real negative, an independent provider gate, and a serial registry/adapter owner. They do not authorize using an archived planning status as current runtime evidence.

## Readiness and ownership

F2 begins only after complete F1 zero-gap acceptance names the implemented D0/P0/S0/K0 SHA and nonempty corpus. The new server crate does not exist yet. Node 1.2 lands the compiling S1 declaration/consumer-double surface before any rule or transport worker. Record its accepted commit SHA in `ledger.md`; every later brief must cite that SHA at dispatch. A plan file or test-double pass is not a real server. The controller exclusively edits `packages/engine/Cargo.toml`, `Cargo.lock`, `mornlea_server/src/lib.rs`, `src/core/mod.rs`, `src/rules/mod.rs`, the shared test entry files, fixture inventory and any wire/save version file. Workers own only the disjoint files named in their packet; a worker finding an API conflict reports the exact failing case and source instead of editing a shared declaration.

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
    fluid_updates: usize,       // 0..=512
    fluid_rescan_cells: usize,  // 0..=65536
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
    fn shutdown(&mut self, deadline: Deadline) -> Result<ShutdownReport, ServerError>;
}
```

The comments above document **planned contract fields**; production source comments added by implementation workers use English and never mention task IDs. `ServerError` has closed classifications `InvalidInput`, `IncompatibleVersion`, `InvalidState`, `StaleSession`, `Capacity`, `Cancelled`, `Disconnected`, `Io`, `Internal`, with structured details owned by the core. `TickPublication` owns `tick`, ordered `Vec<RoutedEvent>`, control replies, save submissions and counters; no borrow escapes the call. `SessionKey` is nonzero, monotonic and never reused during one server process; exhaustion fails admission. `ServerLimits::try_new` and `TickBudget::try_new` validate all fields before allocation or mutation, including checked multiplication for `max_players × session_outbox`; accessors are read-only and an internally forged over-limit value also fails `advance_tick`. The stated defaults copy current Go configuration where it has a bound: 8 active players, 512 outbox, 64 snapshot chunks and 1 MiB snapshot bytes. S2 owns 16 pending handshakes before `AdmittedLogin` exists; S1 `admit` does not reserve a prelogin slot. The Go command/chunk-result inboxes are unbounded; 4096/64 here are **proposed Rust target admission limits**, chosen to align with the existing domain batch cap and snapshot work unit. Node 1.1 measures command and ready-chunk high-water on every supported recorded run. Node 1.2 freezes these numbers only when each run fits and a +1 boundary is tested. If a supported replay needs more, the controller revises this proposal and all dependent packets before accepting the contract; workers do not raise limits locally.

The same contract landing defines the S3 `StoreHandle` and S4 `AgentHandle` traits before their adapter workers start. S3 operations are `submit(SaveRequest) -> Result<SaveTicket, ServerError>`, `poll(SaveTicket) -> SavePoll::{Pending,Durable,Failed}`, `poll_tick(tick, SaveBudget) -> Result<SaveScheduleReport, ServerError>`, and `flush(Deadline) -> Result<FlushReport,ServerError>`; a `Durable` result follows the F1 storage commit boundary, and cancel after durability cannot retract it. S4 operations are `submit(AgentRequest) -> Result<AgentRequestId,ServerError>` and `poll(AgentRequestId) -> AgentPoll::{Pending,Candidate,Failed}`; request/candidate include source tick and request ID and the candidate is never a direct world mutation. The controller supplies exact owned value definitions and deterministic success/error doubles in node 1.2 based on accepted F1 and the existing versioned Agent contract, records its SHA, then 3.4/3.6 implement only the providers. A missing type or double blocks dispatch rather than becoming a worker design choice.

`admit` checks version and phase before active capacity; S2 reserves pending handshakes before constructing `AdmittedLogin`. `submit` checks live session key, session phase, whole intent payload and (only for `Sequenced`) strictly increasing per-session sequence, then queue capacity, then commits sequence/arrival/receipt atomically. Thus an invalid sequence wins over a full queue, and a rejected request consumes neither sequence nor arrival index. Only `PlayIntent::Sequenced` becomes a `CommandEnvelope` at the next tick boundary. `Chat` is validated/routed as text without inventing a command sequence. `KeepAliveReply` updates control-plane liveness and returns `ControlAccepted` without entering domain commands. A queued receipt never means the command succeeded. One tick drains at most `work.commands` already admitted records; remaining records retain order, and no command is silently discarded. A full queue returns `Capacity` without accepting the input. The core uses the existing `order_commands` comparator, so same-session equal-sequence arrival ties resolve by earlier arrival. Tick work does not wait for disk, socket, Agent, GPU or Python.

**Go parity order fixed for the serial reducer:** snapshot tunables/climate; assign environment tick; drain commands/chunk/companion inboxes; order and reject stale commands; player command phase; companion actions; apply acquired/generated chunks; player/companion physics; **subscription reconciliation and refreshed views immediately after actor advancement**; hostile combat/projectiles/deaths; passives; companion placements and ordered interactions; sleep/drops/furnaces; fluid boundary rescan and updates; farmland moisture; tramples; snow footprints; crop random ticks; container moves/mining/workbench; support sweeps for plants/saplings/torches/beds; commit mutations; publish entity/inventory state; advance tick/time/weather/season. The source of this sequence is `packages/server/sim/runtime/engine_step.go` plus `entity/tick.go` and `realm/environment.go`; `engine_step_phase_test.go` guards the important phase edges. The reducer in node 3.1 owns this one serial sequence. Rule modules supply phase operations and never edit `src/core/step.rs` concurrently.

## Common worker packet procedure

For every node in [01-server-slices.md](01-server-slices.md), the worker receives only its named packet, this execution contract, the accepted S1 SHA and the named read-only Go source. It must: (1) add the packet's exact success, negative and limit cases to its exclusive test module; (2) run the listed filter and capture a **behavioral** red after the contract compiles; (3) implement the stated provider in its exclusive source file; (4) run the nonempty focused Rust target and the cited Go oracle test; (5) report source SHA, fixture identity, discovered/executed count, overflow/error behavior and rollback; (6) let the controller review and make the scoped commit. A missing function import or a `-- --list` result alone is not a behavioral red/green. `cargo test` commands always use `rustup run 1.97.1 cargo test --manifest-path packages/engine/Cargo.toml -p mornlea_server --locked` with the packet's `--test` and filter suffix. `tests/server_replay.rs`, `tests/server_contract.rs`, `tests/persistence_failure.rs` and `tests/local_remote_parity.rs` are controller-owned entry files that import file-per-node modules; workers only edit their own modules.

## Cross-change graph

```text
complete F1 SHA → F2 1.1 inventory → F2 1.2 S1 contract
                                      ├─ 1.3–1.5 bounded authority lifecycle; serial 1.6 real mutation transaction
                                      ├─ 2.x disjoint rule providers (2.1b/2.9b after 1.6) → 3.1 serial tick reducer
                                      ├─ 3.2 shared S2 → 3.3a/3.3b parity → accepted S2 SHA → F3 1.1/1.2
                                      ├─ 3.4a/3.4b S3 → 3.5 recovery
                                      └─ 3.6 S4 Agent adapter
                                    → 3.7 real local/remote/save/Agent integration
                                    → 3.8 opt-in activation → F2 4.x full acceptance → F3 3.4 real integration
```

F2 3.1 consumes the rule modules and therefore follows their provider tests even where modules have no file overlap. The transport, persistence and Agent adapters may be built against S1 doubles while rules run, but final F2 integration is serial. S2 is accepted only after common validation and both real Memory/TCP adapter parity cases pass on one SHA; 3.2 alone is insufficient. The tool/test registry, corpus manifest, crate exports and activation/rollback owner are serial regardless of edit disjointness. F3 may consume the accepted S2 contract before full F2 completion, but F3 real integration cannot close without full F2 parity.

## Refinement precedence

Read [direct accepted predecessors](03-parallel-readiness.md) and [refined node decisions](04-refined-nodes.md) with this packet. Split parent IDs are retained here only as historical grouping; their child packets own execution. The dependency register controls readiness, and refined source mappings control absent fields. No worker infers a missing signature, capacity, source fact or shared-file edit.
