# Evidence foundation and server core lifecycle

Packets for tasks **1.1–1.3**. Node 1.1 is controller-only verification. Nodes 1.2–1.3 may delegate implementation after F1 gate clears.

## Node 1.1: Verify complete F1 acceptance

**Deliverable:** ledger entry that either binds the final F1 acceptance SHA and command output or names the exact missing prerequisite family.

**Read-only:** archived F1 successor ledgers, active `rust-storage-codec-closure/ledger.md`, `openspec/changes/archive/2026-09-21-rust-runtime-foundation/tasks.md` nodes 5.2–5.14 and 6.3–6.6.

**Procedure:**

1. Confirm archived ledgers for domain-event, protocol-completion, region-format, and storage-safety-repairs contain accepted closeout SHAs.
2. Confirm `rust-storage-codec-closure` task 5.2 is accepted with seven `save.*` families nonzero in `contracts.json`.
3. Run the final F1 acceptance command exactly as recorded in the F1 closeout ledger (if absent, record **blocked: final F1 acceptance change not published** and stop).
4. Append results to `ledger.md`; do not mark task 1.1 complete without nonzero executed Rust/Go suites from that command.

**Validation:** controller inspection only; no `mornlea_server` crate yet.

---

## Node 1.2: Register `mornlea_server` crate and inventory skeleton

**Prerequisites:** accepted node 1.1.

**Files:**

- Create: `packages/engine/crates/mornlea_server/Cargo.toml`, `src/lib.rs`, `src/{core,session,sim,persistence,agent,transport}.rs` (module stubs), `AGENTS.md`
- Create: `packages/engine/crates/mornlea_server/tests/{server_contract,server_replay,persistence_failure,local_remote_parity}/mod.rs` with one `#[test] fn inventory_registration_pending()` each
- Create: `testdata/runtime-migration/server/inventory.json` (empty `families` array allowed at red stage)
- Modify: `packages/engine/Cargo.toml` workspace members, `packages/engine/AGENTS.md`

**Red test (server_contract):**

```rust
#[test]
fn crate_registers_nonempty_integration_targets() {
    // Implemented as a compile-time include_str check on AGENTS.md listing four test targets
    // and cargo test --list discovery ≥1 test per target after registration.
    panic!("mornlea_server integration targets not registered");
}
```

**Implementation:** register crate with dependencies on F1 crates and `mornlea_engine`; export stub `ServerCore` returning `ServerError::ShuttingDown` from `step_tick`; wire four integration test binaries.

**Validation:**

```bash
rustup run 1.97.1 cargo metadata --manifest-path packages/engine/Cargo.toml --no-deps --format-version 1
rustup run 1.97.1 cargo test --manifest-path packages/engine/Cargo.toml -p mornlea_server --tests --locked -- --list
```

Expected after green: ≥4 tests listed across the four targets.

**Exclusions:** no simulation logic, no manifest merge into `contracts.json` yet.

**Commit:** `feat(engine): register mornlea_server scaffold and test targets`

---

## Node 1.3: Bounded tick queue, shutdown, and cancellation

**Prerequisites:** node 1.2.

**Files:**

- Modify: `packages/engine/crates/mornlea_server/src/core.rs`, `src/sim.rs`
- Test: `packages/engine/crates/mornlea_server/tests/server_contract/tick.rs`

**Interfaces:**

- Consumes: `ServerCore`, `ServerConfig`, `TickBudget` from [execution-contract.md](../execution-contract.md)
- Produces: `step_tick` honoring `tick_queue_capacity`; `begin_shutdown`/`poll_shutdown` releasing workers without dropping acknowledged durable work

**Red test:**

```rust
#[test]
fn saturation_rejects_enqueue_while_shutdown_drains() {
    let mut core = ServerCore::test_with_capacity(1);
    core.fill_queue();
    assert!(matches!(core.enqueue_command(1, sample_envelope()), Err(ServerError::Saturated)));
    core.begin_shutdown().unwrap();
    assert!(core.poll_shutdown().unwrap());
}
```

**Validation:**

```bash
rustup run 1.97.1 cargo test --manifest-path packages/engine/Cargo.toml -p mornlea_server --test server_contract tick:: --locked
```

**Commit:** `feat(server): add bounded tick queue and shutdown contract`
