# Persistence workers and independent Agent adapter

Packets for tasks **2.4** and **2.5**.

## Node 2.4: Bounded persistence and recovery

**Prerequisites:** node 2.3.

**Files:**

- Modify: `packages/engine/crates/mornlea_server/src/persistence.rs`
- Test: `packages/engine/crates/mornlea_server/tests/persistence_failure/mod.rs`
- Go characterization: `persistence_backpressure_test.go`, `persistence_integration_test.go`, `metadata_restart_test.go`

**Red tests:**

1. `interrupted_save_surfaces_error_without_truncating_world` — inject write failure on disposable copy.
2. `restart_after_interrupt_matches_go_recovery_class` — compare normalized recovery category.

**Validation:**

```bash
rustup run 1.97.1 cargo test --manifest-path packages/engine/Cargo.toml -p mornlea_server --test persistence_failure --locked
```

**Commit:** `feat(server): add bounded persistence failure handling`

---

## Node 2.5: Loopback Agent adapter

**Prerequisites:** node 2.4.

**Files:**

- Modify: `packages/engine/crates/mornlea_server/src/agent.rs`
- Test: `packages/engine/crates/mornlea_server/tests/server_contract/agent.rs`
- Fake service: test-only HTTP server in `tests/support/fake_agent.rs`

**Red tests:**

1. `agent_timeout_cancels_without_world_mutation`
2. `invalid_candidate_revalidated_at_tick_boundary`

**Validation:**

```bash
rustup run 1.97.1 cargo test --manifest-path packages/engine/Cargo.toml -p mornlea_server --test server_contract agent:: --locked
make companion-agent-integration
```

**Commit:** `feat(server): add loopback agent adapter with timeout contract`
