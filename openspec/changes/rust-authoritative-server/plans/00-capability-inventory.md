# Frozen server capability inventory contract

Normative for nodes **1.2** and **4.1**. Describes offline replay evidence for authoritative server behavior, not a wire or save format. Workers that discover incompatible Go or Rust APIs stop and return a failing example; they do not invent families, checkpoint shapes, or migration rules.

## Manifest families

Server inventory rows use `id=server.authority.<family>/<version>/<operation>/<label>` with `family`, `version`, `operation`, `input_format`, `expected`, `checkpoints`, and `rust_consumer:"mornlea_server"`. `version` is `1` until a future OpenSpec change explicitly bumps server replay schema. `operation` is one of:

| Operation | Meaning |
| --- | --- |
| `replay` | Apply ordered inputs for N ticks; compare normalized observations at listed checkpoints |
| `reject` | Single ingress action that must fail without world mutation |
| `failure` | Injected persistence, I/O, saturation, or shutdown fault with explicit recovery class |

`packet_key` is absent. `arguments` is a JSON object (≤1,024 bytes) with only the keys declared for that family below.

## Family map (Go oracle ownership)

Each family MUST list at least one executable case before F2 closeout. Go producers live in `_test.go` files only; production `packages/server` code is read-only characterization.

| Family | Go producer home (planned) | Rust route module | Covers |
| --- | --- | --- | --- |
| `server.authority.session` | `packages/server/server/login_*_test.go`, `session_*_test.go` | `tests/server_replay/session.rs` | Handshake/login/play admission, heartbeat, disconnect |
| `server.authority.tick` | `packages/server/sim/runtime/*_test.go` | `tests/server_contract/tick.rs` | Queue saturation, ordering, shutdown, pause gate |
| `server.authority.command` | `packages/server/sim/runtime/command_order_oracle_test.go` | `tests/server_replay/command.rs` | Sequenced command envelope outcomes |
| `server.authority.realm` | `packages/server/sim/realm/*_oracle_test.go` | `tests/server_replay/realm.rs` | Chunk ingress, environment, mutation commit |
| `server.authority.fluid` | `packages/server/fluid/oracle_test.go` | `tests/server_replay/fluid.rs` | Fluid scheduling and parity |
| `server.authority.player` | `packages/server/sim/entity/player*.go` tests + `server/*_parity_test.go` | `tests/server_replay/player.rs` | Movement, mining, inventory, crafting, containers |
| `server.authority.companion` | `packages/server/server/companion_*_test.go` | `tests/server_replay/companion.rs` | Companion actions, publication, Agent stages |
| `server.authority.hostile` | `packages/server/server/hostile_*_test.go` | `tests/server_replay/hostile.rs` | Hostile spawn, combat, restore |
| `server.authority.passive` | `packages/server/server/passive_*_test.go` | `tests/server_replay/passive.rs` | Passive mob lifecycle |
| `server.authority.persistence` | `packages/server/server/persistence_*_test.go` | `tests/persistence_failure/mod.rs` | Async save, backpressure, crash copy |
| `server.authority.transport` | `packages/server/server/transport_parity_integration_test.go`, `multiplayer_*_test.go` | `tests/local_remote_parity/mod.rs` | Memory vs TCP equivalence |
| `server.authority.agent` | `packages/server/server/companion_mcp_contract.go` tests + `make companion-agent-integration` | `tests/server_contract/agent.rs` | Timeout, cancellation, isolation |
| `server.authority.selection` | new `authority_selection_test.go` (test-only) | `tests/persistence_failure/selection.rs` | Exclusive owner, rollback, incompatible data |

Controller-owned files: `testdata/runtime-migration/server/inventory.json`, manifest fragments merged into `contracts.json`, and reviewed binary/json assets. Producers export to a fresh external directory using the same containment rules as F1 runtime-oracle exports; no producer writes tracked paths directly.

## Replay input envelope

`replay` cases use `input_format:"json"` with a top-level object:

```json
{
  "seed": 1,
  "world_fixture": "server/fixtures/<name>",
  "ticks": [
    {
      "tick": 0,
      "commands": [
        {
          "session": 1,
          "sequence": 1,
          "arrival_index": 0,
          "command": { "kind": "player_input", "payload": { } }
        }
      ]
    }
  ]
}
```

`command.kind` and `payload` use the frozen names from `mornlea_domain::Command` variants (not Go struct names). The Go producer translates real `contract.Command` values into this JSON when exporting; Rust replays through `CommandEnvelope` constructors. Chat intents use a parallel `chat` array per tick when needed; they never receive fabricated sequence numbers.

## Normalized observation JSON

Checkpoints compare `expected` JSON of the form:

```json
{
  "kind": "ok",
  "category": "server_tick",
  "checkpoint": "3",
  "value_sha256": "sha256:<64 lowercase hex>"
}
```

The hashed value tree includes tick number, per-session rejections, published block change digests, player/companion/hostile snapshots, persistence queue depth, and Agent disposition fields documented per family in the inventory row. Sort object keys; use decimal strings for u64/i64; use eight-digit lowercase hex for f32 bits. Go and Rust each build the tree from their own execution; neither reads the other's runtime object graph.

Error cases use `kind:"error"` with `category` in `saturated`, `shutting_down`, `invalid_session`, `persistence`, `protocol`, `authority_conflict`, `rollback_incompatible`, or family-specific `reject_*` categories named in the row.

## Zero-gap rule

Node **4.1** fails if any row in `inventory.json` lacks an executable case, a registered Rust route, or a recorded pass at the implementation SHA. F1 corpus completeness does not substitute for server inventory completeness.
