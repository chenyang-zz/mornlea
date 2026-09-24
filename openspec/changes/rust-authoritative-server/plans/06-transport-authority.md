# Memory/TCP parity and authority selection

Packets for tasks **3.1** and **3.2**.

## Node 3.1: Shared ingress for Memory and TCP

**Prerequisites:** node 2.5.

**Files:**

- Modify: `packages/engine/crates/mornlea_server/src/transport/{memory,tcp}.rs`, `src/session.rs`
- Test: `packages/engine/crates/mornlea_server/tests/local_remote_parity/mod.rs`
- Go reference: `transport_parity_integration_test.go`, `login_tcp_test.go`

**Red test:** `memory_and_tcp_reject_stale_action_equivalently` drives the same invalid play intent through both adapters and compares normalized rejection JSON.

**Validation:**

```bash
rustup run 1.97.1 cargo test --manifest-path packages/engine/Cargo.toml -p mornlea_server --test local_remote_parity --locked
```

**Commit:** `feat(server): unify memory and tcp validation path`

---

## Node 3.2: Exclusive activation and rollback

**Prerequisites:** node 3.1.

**Files:**

- Modify: `packages/engine/crates/mornlea_server/src/core.rs`
- Test: `packages/engine/crates/mornlea_server/tests/persistence_failure/selection.rs`, `tests/server_contract/authority.rs`
- Go test-only: `authority_selection_test.go` (new) exporting selection cases

**Red tests:**

1. `second_activate_refuses_competing_owner`
2. `rollback_with_incompatible_schema_fails_explicitly`
3. `successful_rollback_restores_named_backup`

**Validation:**

```bash
rustup run 1.97.1 cargo test --manifest-path packages/engine/Cargo.toml -p mornlea_server --test persistence_failure selection:: --locked
rustup run 1.97.1 cargo test --manifest-path packages/engine/Cargo.toml -p mornlea_server --test server_contract authority:: --locked
```

Default product startup remains Go; this node only proves opt-in paths.

**Commit:** `feat(server): add exclusive activation and rollback contract`
