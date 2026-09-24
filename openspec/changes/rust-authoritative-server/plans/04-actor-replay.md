# Actor, combat, drop, and companion replay

Packet for task **2.3**.

## Node 2.3: Hostile, passive, combat, and companion rules

**Prerequisites:** node 2.2.

**Files:**

- Modify: `packages/engine/crates/mornlea_server/src/sim.rs`
- Test: `packages/engine/crates/mornlea_server/tests/server_replay/{hostile,passive,companion}.rs`
- Go producers: `hostile_restore_test.go`, `passive_publication_test.go`, `companion_stage_acceptance_test.go`, combat parity tests

**Minimum case set:**

| Case ID | Proves |
| --- | --- |
| `server.authority.hostile/1/replay/melee-kill-drop` | Deterministic combat outcome |
| `server.authority.hostile/1/reject/overflow-spawn` | Bounded spawn queue |
| `server.authority.passive/1/replay/graze-restored` | Passive restore publication |
| `server.authority.companion/1/replay/stage-task-fifo` | Companion queue ordering |

**Red test:** `hostile_melee_kill_matches_go_checkpoint` on the melee case.

**Validation:**

```bash
rustup run 1.97.1 cargo test --manifest-path packages/engine/Cargo.toml -p mornlea_server --test server_replay hostile:: --locked
rustup run 1.97.1 cargo test --manifest-path packages/engine/Cargo.toml -p mornlea_server --test server_replay companion:: --locked
```

Incomplete families keep task **2.3** open even if one slice is green.

**Commit:** `feat(server): replay hostile and companion authority parity`
