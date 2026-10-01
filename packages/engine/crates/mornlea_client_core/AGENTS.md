# mornlea_client_core

## Scope

The standalone semantic client core: the checked C1 session/input/preparation
contracts, the C2 typed family records, the frame validator and the
substitution ports. It depends only on `mornlea_domain`, `mornlea_protocol`
and `mornlea_engine`; it must not import the server implementation, a Godot
host, Python, GPU code, or any online authority, and it never adds a second
wire decoder.

## Directory map

- `src/contracts.rs` — frozen C1 external boundary: epochs, revisions, limits,
  errors, `ClientEndpoint`, `ClientCore`, identity/config, clock/connector
  substitution ports, the shared family header and the `PreparationPort`
  trait.
- `src/input.rs` — the twenty semantic actions, whole-batch token rules, the
  F1 mapping table, `InputTranslator` validate/commit and the local
  view-validity overlay.
- `src/prediction.rs` — the declared player prediction state owner.
- `src/preparation.rs` — terrain keys, LOD configuration, owned preparation
  jobs/results/tickets and their byte charge; `src/preparation/lod.rs` is the
  far-LOD provider's file.
- `src/session/` — the confirmed mirror contract (`mod.rs`) and the login,
  mirror, I/O and lifecycle provider files.
- `src/presentation/` — supporting values and projection context (`mod.rs`),
  the records and frame validator (`frame.rs`), the checked drop-geometry
  helper (`geometry.rs`), and the per-family provider files.
- `tests/` — the four registered contract targets plus `support/mod.rs`, the
  deterministic replay harness and contract doubles.

## Ownership and boundaries

- The confirmed mirror, the visible `PresentationFrame` and the preparation
  arena each have exactly one owning provider; projections are read-only.
- Whole-batch input admission, whole-observation mirror updates and
  whole-frame publication are atomic; validation precedes allocation and
  sequence advancement, and a rejection never publishes a partial frame.
- Family providers edit only their own source and test files; exports, roots,
  registries and the frame validator are the contract landing's files.
- Wire compatibility is frozen at protocol v45; inbound bytes always pass
  through `mornlea_protocol` decoding.

## Entry points

- Library consumers start at `ClientCore::new` and the `ClientEndpoint`
  trait; the Godot adapter consumes the same safe Rust surface later.
- The registered test targets are `session_replay`, `prediction_replay`,
  `presentation_contract` and `lifecycle_contract`.

## Focused validation

```sh
rustup run 1.97.1 cargo test --manifest-path packages/engine/Cargo.toml -p mornlea_client_core --test session_replay --locked
rustup run 1.97.1 cargo test --manifest-path packages/engine/Cargo.toml -p mornlea_client_core --test prediction_replay --locked
rustup run 1.97.1 cargo test --manifest-path packages/engine/Cargo.toml -p mornlea_client_core --test presentation_contract --locked
rustup run 1.97.1 cargo test --manifest-path packages/engine/Cargo.toml -p mornlea_client_core --test lifecycle_contract --locked
```
