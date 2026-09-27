# Native Numerical Implementation Guides

## Purpose
This directory contains the safe, typed Rust native traits and contracts (`contracts/`) for numerical engine operations, as well as the implementation of individual provider lanes (collision, physics, raycast, worldgen, etc.). These contracts are the safe Rust numerical facade for future Rust core callers. Transitional Go callers use the separate C ABI adapters in `src/ffi.rs`. Godot's embedded Python owns presentation and accesses qualified presentation bridges; it does not call these numerical contracts directly or own authoritative state.

## Submodules
- `contracts/`: Shared request/result contracts, public operation traits, and error enums.
- Provider modules (e.g., `collision.rs`, `physics.rs`, `worldgen.rs`): Implementations of the respective engine operation traits.

## Contracts Boundary
The native contracts are the strict boundary for the engine operations. Provider implementations must satisfy the traits defined in `contracts/`. They must not bypass the typed request/result structures.

## Focused Test Commands
```bash
cargo test -p mornlea_engine --test native_contract --locked
cargo test -p mornlea_engine --lib --locked tree_blocks
cargo clippy -p mornlea_engine --all-targets --locked -- -D warnings
```
