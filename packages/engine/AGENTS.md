# Rust Workspace Guide

## Scope

Read the crate-specific `AGENTS.md` before working in
`packages/engine/crates/mornlea_engine/`, `packages/engine/crates/mornlea_client/`,
`packages/engine/crates/mornlea_domain/`, `packages/engine/crates/mornlea_protocol/`,
`packages/engine/crates/mornlea_client_core/`,
or `packages/engine/crates/mornlea_storage/`.

The root provider-aware orchestration policy applies without modification. This scoped guide does not impose an additional subagent count, sequence, or review topology.

## Toolchain and ownership

- `packages/engine/rust-toolchain.toml` pins Rust 1.97.1. Do not upgrade the compiler, Cargo lockfile, or wgpu dependency line independently.
- `mornlea_engine` is the windowless numerical core and exposes mesh/light, collision, raycast, physics, and world-generation operations through the engine ABI.
- `mornlea_client` owns the Darwin window, events, in-process WKWebView menu layer, and GPU backend and exposes a separate client ABI. The two ABIs evolve independently and do not share a version number.
- `mornlea_domain` owns shared identifiers, value rules, and semantic input/event records. `mornlea_protocol` owns versioned framing and packet codecs. `mornlea_storage` owns save records and migration codecs. Protocol and storage may depend on domain; none of the three may depend on `mornlea_engine`, `mornlea_client`, `mornlea_godot`, or an online authority in production.

## FFI

- `extern "C"` entry points must not allow a panic to unwind across FFI. Validate ABI version, pointers, lengths, alignment, overlap, and capacity before constructing slices, dereferencing pointers, or writing output.
- Validation failures must not leave partial output. Fixed capacity and overflow status are cross-language contracts and must not be silently truncated to pass tests.
- Rust exports use English `///` doc comments that explain safety preconditions, ownership, failure semantics, and ABI synchronization surfaces.

## Test organization

Organize Rust tests according to the topic-module and helper-center rules in `docs/test-organization.md`. Keep a single-topic test in its existing module tree rather than creating a parallel integration-test mirror.

## Validation and entry points

- Build cdylibs with `make rust`.
- The Makefile sets `CARGO_TARGET_DIR=packages/engine/target/cargo` for `make rust` by default and copies release dylibs to `packages/engine/target/release` through `scripts/engine/deploy-dylib.sh`. Direct Cargo invocations do not read the Makefile, so set `CARGO_TARGET_DIR` explicitly when that directory or another target is required. CI also sets it explicitly, and a value passed to `make rust` overrides the default.
- Run the workspace gate with `make rust-check`.
- Run focused crate tests with `cd packages/engine && cargo test -p mornlea_engine --locked` or `cd packages/engine && cargo test -p mornlea_client --locked`.
- Foundation contract crates: `rustup run 1.97.1 cargo test --manifest-path packages/engine/Cargo.toml -p mornlea_domain -p mornlea_protocol -p mornlea_storage --test runtime_contract --locked`.
- Current documentation entry points are `docs/notes/go-rust-division.md` and `docs/test-organization.md`.
