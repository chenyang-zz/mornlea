# F2 worker execution contract

This document and the linked `plans/` packets are the controller's Superpowers implementation plan for **unstarted** F2 work. Baseline for planning reconciliation: `e951a4eb8d2b713fa83054b66f404ae104ea3dbe` on `dev`. The Go server remains the production authority until P14; this change authorizes planning and later opt-in Rust implementation only after complete F1 acceptance. Only `tasks.md` carries task-status checkboxes.

## Goal and architecture

Build one Rust authoritative server crate (`mornlea_server`) that owns session lifecycle, bounded tick scheduling, world/entity simulation orchestration, persistence coordination, independent Agent adaptation, and Memory/TCP ingress over **one** validation and simulation core. Preserve protocol v45, current save schemas, engine ABI v11, and externally observable outcomes recorded from the Go server. Numerical kernels stay in `mornlea_engine`; semantic values and wire/save codecs stay in F1 crates (`mornlea_domain`, `mornlea_protocol`, `mornlea_storage`). No Godot, client-core, or Python imports in production server code.

Use Rust **1.97.1** / edition 2024 from `packages/engine/rust-toolchain.toml`. Production dependency additions require an explicit controller ruling in the ledger; the default plan adds only `mornlea_server` workspace membership and test-only helpers under `packages/engine/tests/`. Go remains the compatibility oracle for behavior not yet covered by frozen server replay cases.

## F1 prerequisite gate (hard block)

F2 implementation nodes **1.2 onward** stay blocked until task **1.1** records passing evidence for **all** of the following, with zero uncovered supported families in the final F1 acceptance ledger:

| Prerequisite | Status source |
| --- | --- |
| Archived [domain-event successor](../archive/2026-09-22-rust-domain-event-completion/ledger.md) | Accepted archive + ledger SHA |
| Archived [protocol completion](../archive/2026-09-24-rust-protocol-completion/ledger.md) | Accepted archive + ledger SHA |
| Archived [region format completion](../archive/2026-09-23-rust-region-format-completion/ledger.md) | Accepted archive + ledger SHA |
| Archived [storage safety repairs](../archive/2026-09-24-rust-storage-safety-repairs/ledger.md) | Accepted archive + ledger SHA |
| Active [storage codec closure](../rust-storage-codec-closure/ledger.md) | Accepted ledger + seven-family corpus |
| Remaining F1 foundation nodes from archived [runtime foundation](../archive/2026-09-21-rust-runtime-foundation/tasks.md): numerical APIs **5.2–5.14**, pathfinding **5.12–5.13**, replay/acceptance **6.3–6.6** | Named acceptance command + SHA in a final F1 closeout ledger |

The archived baseline alone, any single successor, or this planning revision does **not** satisfy the gate. Task 1.1 must copy the exact final F1 acceptance command output into this change's `ledger.md` or record the specific missing family.

## Crate layout and dependency rules

Create `packages/engine/crates/mornlea_server/` with scoped `AGENTS.md` and register it in `packages/engine/Cargo.toml`. Allowed production dependencies: `mornlea_domain`, `mornlea_protocol`, `mornlea_storage`, `mornlea_engine`, and std/crates already used by those units. **Forbidden:** `mornlea_client`, `mornlea_godot`, Go FFI, WebGPU, direct `packages/agent` imports, shelling out to the Agent service, or embedding Python.

Internal module boundaries (single crate, multiple files):

| Module | Owns |
| --- | --- |
| `core` | `ServerCore`, tick driver, bounded queues, shutdown/cancellation, exclusive world ownership |
| `session` | Login state machine, play admission, sequencing, session tables |
| `sim` | Rust port of `runtime.Engine` orchestration calling engine kernels and F1 types |
| `persistence` | Async save workers, backpressure, recovery hooks (blocking I/O off hot path) |
| `agent` | Loopback HTTP/MCP adapter, candidate intake, tick-boundary revalidation |
| `transport` | Memory and TCP adapters sharing one `Ingress` trait into `session` |

Update `packages/engine/AGENTS.md` when the crate is created. Sync any new audit edges in the same implementation commit that introduces them.

## Public server-core surface (frozen for workers)

Workers implement against these names; local private helpers may differ only inside the owning file.

```rust
pub struct ServerConfig {
    pub world_seed: u64,
    pub max_sessions: u32,
    pub tick_queue_capacity: u32,
    pub persistence_queue_capacity: u32,
    pub agent_timeout: std::time::Duration,
}

pub enum ServerError {
    Saturated,
    ShuttingDown,
    InvalidSession,
    Persistence(mornlea_storage::StorageError),
    Protocol(mornlea_protocol::ProtocolError),
    AuthorityConflict,
    RollbackIncompatible,
}

pub struct TickBudget {
    pub max_commands: u32,
    pub max_chunk_jobs: u32,
}

pub struct ServerCore { /* private fields */ }

impl ServerCore {
    pub fn try_activate_exclusive(world_path: &std::path::Path, config: ServerConfig)
        -> Result<Self, ServerError>;
    pub fn enqueue_command(&mut self, session_id: u64, envelope: mornlea_domain::CommandEnvelope)
        -> Result<(), ServerError>;
    pub fn step_tick(&mut self, budget: TickBudget) -> Result<mornlea_domain::TickObservation, ServerError>;
    pub fn begin_shutdown(&mut self) -> Result<(), ServerError>;
    pub fn poll_shutdown(&mut self) -> Result<bool, ServerError>;
}
```

`TickObservation` is a test-facing aggregate defined in `mornlea_server` that serializes to the normalized JSON shape in [plans/00-capability-inventory.md](plans/00-capability-inventory.md). It is **not** a wire packet. Memory and TCP adapters call the same `enqueue_command` and `step_tick` paths after protocol decode and session admission.

Tick phase order inside `sim` MUST match the Go `StepWithTunables` order documented in `packages/server/sim/runtime/AGENTS.md`: command validation → companion actions → chunk ingress → physics/subscriptions → hostiles → combat/death → world interactions → sleep/drops/furnaces → fluids/farmland/crops → containers/mining → support check → single realm commit → time advance → publication. Reordering requires a controller contract revision before implementation.

## Capability inventory and replay corpus

Authoritative coverage is tracked under `testdata/runtime-migration/server/` (created in node 1.2). The manifest extends `contracts.json` schema version 2 with families `server.authority.*` documented in [plans/00-capability-inventory.md](plans/00-capability-inventory.md). Each inventory row maps one supported Go behavior to:

- a Go producer test (read-only oracle or export helper in `_test.go` only),
- one or more frozen replay cases with normalized checkpoint JSON,
- a Rust integration route in `packages/engine/crates/mornlea_server/tests/`.

Unsupported rows block F2 closeout. Existing F1 domain/protocol/storage corpus cases remain evidence for value and codec boundaries; they do not replace server tick replay.

## Integration test targets (prospective until node 1.2)

| Target | Exercises |
| --- | --- |
| `server_contract` | Queues, shutdown, admission rejection, Agent timeout/isolation |
| `server_replay` | Offline Go-recorded schedules vs Rust `ServerCore` checkpoints |
| `persistence_failure` | Injected I/O, interrupted save, restart recovery on disposable copies |
| `local_remote_parity` | Equivalent Memory and TCP sessions for login, play, invalid/stale actions |

Register each target with at least one failing case before claiming node completion. Discovery command:

`rustup run 1.97.1 cargo test --manifest-path packages/engine/Cargo.toml -p mornlea_server --tests --locked -- --list`

## Worker steps and closure

Every packet follows: (1) add the named regression to the listed test file; (2) run its filter and confirm the intended behavioral failure; (3) implement only the frozen transformation; (4) run focused suites plus listed integration checks; (5) submit evidence for controller review, then scoped commit. Workers never check off `tasks.md` or append acceptance ledger entries.

Controller acceptance per node requires: expected red reason, exclusive diff scope, exact command output with nonzero executed cases, independent review with no unresolved Important/Critical findings, scoped implementation commit, and `docs(openspec): record <node-slug> acceptance` for status files.

## Transport, Agent, and authority selection

- **Memory adapter:** in-process framed ingress using the same codec path as Go Memory integration tests (`packages/server/server` memory helpers).
- **TCP adapter:** loopback listener using `mornlea_protocol` framing; no second validation path.
- **Agent:** versioned loopback contract only; candidates validated before tick application; timeouts cancel without world mutation.
- **Opt-in activation:** `try_activate_exclusive` refuses competing owners; rollback restores named backup or fails explicitly (see node 3.2).

## Validation at implementation closeout

Record SHA, corpus digests, discovered/executed test counts, failure-path results, and rollback proof in `ledger.md`. Stage gates: `cargo fmt --check`, `make rust-check`, `make dev-check`, `make test-race`, `go test ./packages/audit -count=1`, change-specific replay suites, `make companion-agent-integration` where Agent nodes apply, and `openspec validate --all --strict --no-interactive`. Automated tests must not launch a foreground game window.
