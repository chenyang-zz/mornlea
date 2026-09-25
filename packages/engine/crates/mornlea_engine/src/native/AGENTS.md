# Native Numerical Implementation Guides

## Purpose
This directory contains the safe, typed Rust native traits and contracts (`contracts/`) for numerical engine operations, as well as the implementation of individual provider lanes (collision, physics, raycast, worldgen, etc.). These contracts provide the ABI boundary for the Go server and future embedded Python runtime.

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
