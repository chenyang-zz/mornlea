# World, chunk, environment, and fluid replay

Packet for task **2.1**. Split dispatch slices **2.1a–2.1c** if a single session cannot close all families.

## Node 2.1a: Realm chunk ingress and mutation commit

**Prerequisites:** node 1.3.

**Files:**

- Modify: `packages/engine/crates/mornlea_server/src/sim.rs`
- Test: `packages/engine/crates/mornlea_server/tests/server_replay/realm.rs`
- Go producer (controller-integrated): extend runtime/realm oracle export with `server.authority.realm/1/replay/chunk-ingress-minimal`

**Red test:** `realm_replay_matches_go_checkpoint_0` loads frozen case `server.authority.realm/1/replay/chunk-ingress-minimal` and asserts `value_sha256` at checkpoint `"0"`.

**Validation:**

```bash
rustup run 1.97.1 cargo test --manifest-path packages/engine/Cargo.toml -p mornlea_server --test server_replay realm:: --locked
```

**Commit:** `feat(server): replay realm chunk ingress parity`

## Node 2.1b: Environment and farmland moisture

**Prerequisites:** node 2.1a.

**Files:** `sim.rs`, `tests/server_replay/realm.rs`, Go `environment_oracle_test.go` / `farmland_moisture_oracle_test.go` export routes.

**Red test:** `environment_tick_advances_moisture_checkpoint` for case `server.authority.realm/1/replay/farmland-moisture-one-tick`.

**Validation:** same `server_replay` filter prefix `realm::`.

**Commit:** `feat(server): replay environment moisture parity`

## Node 2.1c: Fluid scheduling

**Prerequisites:** node 2.1b.

**Files:** `sim.rs`, `tests/server_replay/fluid.rs`, Go `packages/server/fluid/oracle_test.go` export.

**Red test:** `fluid_parity_single_source` case `server.authority.fluid/1/replay/single-source-spread`.

**Validation:**

```bash
rustup run 1.97.1 cargo test --manifest-path packages/engine/Cargo.toml -p mornlea_server --test server_replay fluid:: --locked
```

**Commit:** `feat(server): replay fluid scheduling parity`

**Task 2.1 completion:** all three slices accepted; ledger lists per-family case IDs and SHAs.
