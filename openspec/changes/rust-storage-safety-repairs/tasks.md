# Rust storage safety repairs — Superpowers implementation plan

> For worker agents: implement one linked node at a time. This is the sole checkbox and status source. The linked packets contain frozen interfaces, cases, steps, file ownership, commands and rollback. Do not infer readiness from an archived F1 checkbox.

**Goal:** make constructed Rust save values bounded and decoder-compatible while preserving legacy queue ownership and one domain rule source.

**Architecture:** keep storage DTOs as format records; validate current values through checked domain constructors. Use the same complete chunk aggregate validator for every encoder.

**Tech stack:** Rust 1.97.1 workspace, existing Go 1.26 read-only storage oracle, OpenSpec. **Spec:** [delta](specs/rust-runtime-foundation/spec.md); [design](design.md).

**Global constraints:** protocol v45; player/chunk v9; metadata v6; companions v5; hostile v2; passive v1; region v1; engine ABI v11; client ABI v19; no live world, new dependency or version bump. Test-first for behavior, one scoped commit after each accepted node. Discover each named Rust target with -- --list and reject zero tests. Controller alone updates checkboxes, shared exports and ledger.

**Review focus:** (1) zero-ID legacy queues silently surviving count-only tests; (2) 65-body clone before rejection; (3) a logical encoder bypassing active-container checks; (4) a raw armor triple being treated as an ordinary stack; (5) a compact palette being reordered during domain conversion. The owning packets pin all five.

## 1. Companion admission

- [x] 1.1 [Preserve v2–v4 queue owner and ordered content](plans/01-admission.md#node-1-1). Modify `src/companion.rs`; run `rustup run 1.97.1 cargo test --manifest-path packages/engine/Cargo.toml -p mornlea_storage --test safety_legacy_queue --locked`.
- [x] 1.2 [Reject oversized and mismatched v5 aggregates before clone](plans/01-admission.md#node-1-2). Modify `src/companion.rs`; run `rustup run 1.97.1 cargo test --manifest-path packages/engine/Cargo.toml -p mornlea_storage --test safety_companion_bounds --locked`.

## 2. Chunk publication

- [x] 2.1 [Apply aggregate validity to current chunk encode](plans/01-admission.md#node-2-1). Modify `src/chunk.rs`; run `rustup run 1.97.1 cargo test --manifest-path packages/engine/Cargo.toml -p mornlea_storage --test safety_chunk_aggregate --locked current_`.
- [x] 2.2 [Close logical and historical-schema encoder bypasses](plans/01-admission.md#node-2-2). Modify `src/chunk.rs`; run `rustup run 1.97.1 cargo test --manifest-path packages/engine/Cargo.toml -p mornlea_storage --test safety_chunk_aggregate --locked logical_`.

## 3. Shared current-value rules

- [ ] 3.1 [Delegate UUID validation and expose checked IDs](plans/02-values.md#node-3-1). Modify `src/{identity,lib}.rs`; run `rustup run 1.97.1 cargo test --manifest-path packages/engine/Cargo.toml -p mornlea_storage --test safety_domain_values --locked identity_`.
- [ ] 3.2 [Delegate ordinary item rules while preserving raw armor](plans/02-values.md#node-3-2). Modify `src/{items,lib}.rs`; run `rustup run 1.97.1 cargo test --manifest-path packages/engine/Cargo.toml -p mornlea_storage --test safety_domain_values --locked stack_`.
- [ ] 3.3 [Validate compact sections through domain constructors](plans/02-values.md#node-3-3). Modify `src/{chunk,lib}.rs`; run `rustup run 1.97.1 cargo test --manifest-path packages/engine/Cargo.toml -p mornlea_storage --test safety_domain_values --locked section_`.

## 4. Integrated acceptance

- [ ] 4.1 [Close safety evidence and downstream consumers](plans/03-closure.md#node-4-1). Review crate guide and run full storage target, `make rust-check`, `make dev-check`, `make test-race`, `go test ./packages/audit -count=1`, and `openspec validate --all --strict --no-interactive`; record actual counts and SHA in `ledger.md`.
