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
- `build.rs` — source/contract diagnostics identity from Git and the system
  SHA256 utility at build time; live sessions perform no identity I/O.
- `src/prediction.rs` — atomic replay, actual-dimension target and snow owners;
  `src/prediction/world.rs` borrows bounded numerical views at each position.
- `src/preparation.rs` — terrain keys, LOD configuration, owned preparation
  jobs/results/tickets and their byte charge; `src/preparation/lod.rs` is the
  far-LOD provider's file; `src/preparation/near.rs` schedules bounded immutable
  confirmed sections and neighbor replacements. `src/preparation/native.rs` uses the existing mesh
  and LOD kernels with reusable scratch charged to the selected byte limit.
- `src/session/` — the confirmed mirror contract (`mod.rs`) and the login,
  mirror, I/O and lifecycle provider files. `world.rs` holds compact immutable
  block columns inside that same confirmed owner for prediction, preparation
  and prompt readers; it is not an independently mutable world cache.
  `login.rs` serially composes all live owners, and `publication.rs` assembles
  the ten families before source consumption or resource retirement.
- `src/presentation/` — supporting values and projection context (`mod.rs`),
  the records and frame validator (`frame.rs`), the checked drop-geometry
  helper (`geometry.rs`), and the per-family provider files.
- `src/runtime_data.rs` — private native input constructors; generated mesh
  and collision tables in `src/runtime_data_generated.rs` preserve the
  compatibility mesh, collision and interaction registry. Callers cache registry/worldgen Arcs per session
  outside hot numerical work.
- `tests/` — the four registered contract targets plus `support/mod.rs`, the
  deterministic replay harness and contract doubles.

## Ownership and boundaries

- The confirmed mirror, the visible `PresentationFrame` and the preparation
  arena each have exactly one owning provider; projections are read-only.
- Confirmed block content uses the selected inbound record/byte bounds and
  retires with chunk forget or epoch replacement. Column replacements stage
  exact source changes before publishing their content revision.
- Whole-batch input admission, whole-observation mirror updates and
  whole-frame publication are atomic; validation precedes allocation and
  sequence advancement, and a rejection never publishes a partial frame.
- Family providers edit only their own source and test files; exports, roots,
  registries and the frame validator are the contract landing's files.
- Wire compatibility is frozen at protocol v45; inbound bytes always pass
  through `mornlea_protocol` decoding.

## Entry points

- Library consumers start at `ClientCore::new` and the `ClientEndpoint`
  trait; the Godot arena stores this same core and preserves its token when
  close refuses. Numeric TCP and injected Memory use the same login/codec path.
- The registered test targets are `session_replay`, `prediction_replay`,
  `presentation_contract` and `lifecycle_contract`.

## Focused validation

```sh
rustup run 1.97.1 cargo test --manifest-path packages/engine/Cargo.toml -p mornlea_client_core --test session_replay --locked
rustup run 1.97.1 cargo test --manifest-path packages/engine/Cargo.toml -p mornlea_client_core --test prediction_replay --locked
rustup run 1.97.1 cargo test --manifest-path packages/engine/Cargo.toml -p mornlea_client_core --test presentation_contract --locked
rustup run 1.97.1 cargo test --manifest-path packages/engine/Cargo.toml -p mornlea_client_core --test lifecycle_contract --locked
```
