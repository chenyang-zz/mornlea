# Player movement, actions, and inventory replay

Packet for task **2.2**.

## Node 2.2: Player command and inventory parity

**Prerequisites:** node 2.1c (realm/fluid baseline).

**Files:**

- Modify: `packages/engine/crates/mornlea_server/src/sim.rs`, `src/session.rs`
- Test: `packages/engine/crates/mornlea_server/tests/server_replay/player.rs`, `tests/server_contract/command_reject.rs`
- Go producers: `command_order_oracle_test.go`, selected `server/*_parity_test.go` exports for mining, stack split, crafting

**Minimum case set (must exist before green):**

| Case ID | Proves |
| --- | --- |
| `server.authority.command/1/replay/order-two-sessions` | Envelope ordering matches Go oracle |
| `server.authority.player/1/replay/mining-transcript` | Block break + item conservation |
| `server.authority.player/1/reject/stale-sequence` | No mutation on stale sequence |
| `server.authority.player/1/replay/quick-move-chest` | Container quick-move deterministic target |

**Red test example:**

```rust
#[test]
fn stale_sequence_rejects_without_mutation() {
    let case = load_server_case("server.authority.player/1/reject/stale-sequence");
    let (before, after, err) = replay_single_action(&case);
    assert!(err);
    assert_normalized_state_unchanged(before, after);
}
```

**Validation:**

```bash
rustup run 1.97.1 cargo test --manifest-path packages/engine/Cargo.toml -p mornlea_server --test server_replay player:: --locked
rustup run 1.97.1 cargo test --manifest-path packages/engine/Cargo.toml -p mornlea_server --test server_contract command_reject:: --locked
```

**Commit:** `feat(server): replay player command and inventory parity`
