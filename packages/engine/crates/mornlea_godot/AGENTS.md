# Godot Rust Adapter

## Scope and map

The Godot extension copies checked semantic values and owns host-facing resource
lifetimes. It consumes `mornlea_client_core`, protocol/domain values and numerical
engine contracts; it must not import the Rust server implementation.

- `src/client_core.rs` — explicit Go pilot FFI plus the separate safe Rust
  `CoreArena`; production Rust slots own the actual `ClientCore` endpoint.
- `src/abi.rs` — checked boundary shapes, bounded conversion and C2 rendering.
- `src/bridge.rs` — Godot methods and native Variant conversion/panic boundary.
- `src/feature_negotiation.rs` — closed producer descriptors and capability gates.
- `src/lifecycle.rs`, `pull_buffers.rs`, `status_decode.rs`, `frame_decode.rs` —
  lifecycle and owned pilot buffer handling.
- `src/terrain_*.rs`, `mesh_worker.rs`, `quad_decode.rs` — terrain preparation,
  decoding, bounded host resources and worker retirement.
- `tests/` — registered engine-free lifecycle, family and Rust producer targets.

## Ownership and entry points

The exported methods use the same checked routines exercised by engine-free
tests. Rust producer mode never loads the Go pilot library. A token addresses one
slot/generation; successful endpoint close precedes slot retirement, while a
refusal preserves the token for retry. Complete frames validate before replacing
the last owned boundary copy. Native input conversion enforces depth/record/byte
bounds before recursive allocation; panics cannot cross the Godot boundary.

The extension performs presentation only. Server authority, protocol decoding,
client mirror/prediction and CPU resource ownership remain in their established
owners. Feature descriptors never implicitly enable a producer or change defaults.

## Validation

```sh
rustup run 1.97.1 cargo test --manifest-path packages/engine/Cargo.toml -p mornlea_godot --locked
make godot-python-check
make godot-project-check
make godot-capability-check
```

These source/engine-free checks do not qualify native Godot marshalling, real
server integration or runtime session cycles. Automated work must not launch a
Godot process or game window without explicit runtime authorization.
