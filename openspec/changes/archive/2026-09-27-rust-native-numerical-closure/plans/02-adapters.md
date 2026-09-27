# ABI v11 adapter file packets

**Goal:** route each of the ten existing exported ABI v11 numerical operations to its reviewed native/shared core without changing successful bytes, symbols, layouts or error meanings. **Architecture:** only the controller edits `packages/engine/crates/mornlea_engine/src/ffi.rs`, one scoped accepted commit at a time. **Tech stack:** Rust ABI tests, Go `nativeabi`, source-bound runtime oracle. **Spec:** [delta](../specs/rust-runtime-foundation/spec.md), [design](../design.md), [field and algorithm contract](../worker-briefs.md#node-5-1). `tasks.md` alone records status.

All `src/` and `tests/` paths are relative to `packages/engine/crates/mornlea_engine`. Every node's exact editable paths are `src/ffi.rs` and its named `tests/numerical_migration/<topic>.rs`, except 3.1, which also owns `tests/native_contract/publication.rs` and `src/ffi.rs` unit tests. Other `ffi.rs` hunks, contracts, providers, Go production bridge and corpus manifest are read-only. A node begins from the latest accepted `ffi.rs` SHA, even if its provider landed earlier. Write or confirm the named parity/canary test before changing the route. A behavior-preserving route may be baseline green; cite the provider's genuine red/green evidence and inspect the changed call path rather than inventing a numerical failure. Keep `catch_unwind`, status precedence and unsafe pointer validation. Pack into bounded local/reusable storage, then publish only after success. Run `make rust` and the exact focused Rust/Go commands, review, ledger, scoped commit. A rollback reverts this adapter hunk/test, and any dependent corpus fragment; it does not erase accepted unrelated providers.

<a id="node-3-1"></a>
## Node 3.1 — metadata alias preflight and atomic publication

**Prerequisite:** accepted 1.2; execute before any other `ffi.rs` change. **Files:** `src/ffi.rs` owns unit canary tests and the preflight order in `mornlea_mesh_section`, `mornlea_lod_shell`, `mornlea_fluid_eval_batch`, `mornlea_fluid_rescan`; `tests/native_contract/publication.rs` checks public typed destination atomicity without calling private symbols. **Read-only:** providers, contracts and Go production bridge.

For each of four exports, test valid metadata pointer aliased to input, output and scratch arenas (where a scratch parameter exists), with all overlapping bytes initialized to `0xa5`; each rejection must preserve every byte including metadata. Also test nonaliased valid metadata on invalid input: metadata clears only after all pointer/range/overlap checks, while payload is unchanged. Write unit tests first: current early clear is the expected substantive red. Move metadata clear after complete alias/length/alignment preflight; retain family status precedence and LOD/rescan exact-needed metadata on overflow. Validate `rustup run 1.97.1 cargo test --manifest-path packages/engine/Cargo.toml -p mornlea_engine --test native_contract --locked publication`, `rustup run 1.97.1 cargo test --manifest-path packages/engine/Cargo.toml -p mornlea_engine --lib --locked metadata`, `make rust`, and `go test ./packages/shared/nativeabi -run 'Test.*(FailureAtomicity|InvalidArguments|InvalidBuffers)' -count=1`. **Commit:** `fix(engine): validate metadata aliases before publication`.

<a id="node-3-2"></a>
## Node 3.2 — collision ABI

**Prerequisite:** 2.1 and 3.1. **Files:** `src/ffi.rs` hunk `mornlea_collision_resolve`/`collision_resolve_with` validates raw request and uses typed/read-view shared collision core; `tests/numerical_migration/collision.rs` owns `collision_abi_native_bits`, short output and panic-injection canaries. **Read-only:** collision provider, contracts and Go bridge.

Freeze loaded floor/wall/unknown results as all 16 output bytes, exact `f32` bits and flags before routing; 15-byte destination returns status 7 with full canary. Retain raw swept-prism and pointer admission, resolver injection and `catch_unwind` status 9 test. Convert validated input to read-view cells without copying an ABI grid into typed bytes, compute one result, pack 16 local bytes and publish once. Validate `make rust`, `rustup run 1.97.1 cargo test --manifest-path packages/engine/Cargo.toml -p mornlea_engine --test numerical_migration --locked collision`, `go test ./packages/shared/nativeabi -run 'TestCollision' -count=1`, and `go test ./packages/tools/cmd/runtime-oracle -run '^TestKernelCollision' -count=1`. **Commit:** `refactor(engine): route collision abi through native core`.

<a id="node-3-3"></a>
## Node 3.3 — physics ABI

**Prerequisite:** 2.2 and 3.2. **Files:** `src/ffi.rs` hunk `mornlea_physics_step`/`physics_step_with` converts validated raw fields to `PhysicsRequest` and calls shared physics/collision parts; `tests/numerical_migration/physics.rs` owns `physics_abi_native_bits`. **Read-only:** step/collision providers, contracts and Go bridge.

Pin landing, jump, fluid drag, sprint+sneak and one/two-ULP displacement as **32 complete bytes**, including reserved zeros. Invalid axis/NaN stays status 3; short output stays 7; both retain payload canary. Do not invoke standalone collision swept-grid validation from the physics route: it is stricter than the legacy physics prism. After shared core returns, locally pack position/velocity/flags and publish once. Validate `make rust`, `rustup run 1.97.1 cargo test --manifest-path packages/engine/Cargo.toml -p mornlea_engine --test numerical_migration --locked physics`, `go test ./packages/shared/nativeabi -run 'TestPhysicsStep' -count=1`, and `go test ./packages/tools/cmd/runtime-oracle -run '^TestKernelPhysics' -count=1`. **Commit:** `refactor(engine): route physics abi through native core`.

<a id="node-3-4"></a>
## Node 3.4 — raycast ABI

**Prerequisite:** 2.3 and latest accepted `ffi.rs` SHA. **Files:** `src/ffi.rs` hunk `mornlea_raycast_batch`/`raycast_batch_with` retains raw-cursor admission and calls native continuation core; `tests/numerical_migration/raycast.rs` owns `raycast_abi_cursor_sequence`. **Read-only:** provider, contracts and Go bridge.

Test a ray yielding 64 then a 65th record: compare all ordered records, count, cursor bytes and done after each call; repeat done for empty/done. Tampered cursor, output/cursor overlap and insufficient capacity preserve both output and cursor arenas with current status. Decode the admitted byte cursor through a private adapter, compute a local batch/next cursor, pack both into locals and publish together only after success. No public raw cursor parser. Validate `make rust`, `rustup run 1.97.1 cargo test --manifest-path packages/engine/Cargo.toml -p mornlea_engine --test numerical_migration --locked raycast`, `go test ./packages/shared/nativeabi -run 'TestRaycast' -count=1`, and `go test ./packages/tools/cmd/runtime-oracle -run '^TestKernelRaycast' -count=1`. **Commit:** `refactor(engine): route raycast abi through native core`.

<a id="node-3-5"></a>
## Node 3.5 — world chunk ABI

**Prerequisite:** 2.4 and latest `ffi.rs` SHA. **Files:** `src/ffi.rs` hunk `mornlea_worldgen_chunk`/`worldgen_chunk_with` parses `MGW1` and routes typed parameters to reviewed sampler; `tests/numerical_migration/worldgen_chunk.rs` owns `worldgen_chunk_abi_bytes`. **Read-only:** native world provider and Go bridge.

Before edit, freeze **release** ABI digest and selected cells for seeds 0/1, negative chunk and signed i32 extremes. The valid output is exactly 196,608 LE bytes (98,304 `u16`); 196,607 bytes returns status 7 with complete canary. Parse once, call typed/shared core, encode into a complete local output stage and copy once; do not construct a second full input grid or change exact-length raw admission. Validate `make rust`, `rustup run 1.97.1 cargo test --manifest-path packages/engine/Cargo.toml -p mornlea_engine --test numerical_migration --locked worldgen_chunk`, `go test ./packages/shared/nativeabi -run 'TestWorldgen' -count=1`, and `go test ./packages/tools/cmd/runtime-oracle -run '^TestKernelWorldgenChunk' -count=1`. **Commit:** `refactor(engine): route chunk abi through native worldgen`.

<a id="node-3-6"></a>
## Node 3.6 — world probe ABI

**Prerequisite:** 2.5, 3.5 and latest `ffi.rs` SHA. **Files:** `src/ffi.rs` hunk `mornlea_worldgen_probe`/`worldgen_probe_with` maps raw mode 0/1/2 to `ProbeQuery`; `tests/numerical_migration/worldgen_probe.rs` owns `worldgen_probe_abi_modes` and signed-extreme replay. **Read-only:** probe/chunk provider, worldgen source and Go bridge.

Height with two different raw Y fields must return identical 8-byte records; Terrain/Base must match sampler; 64 queries succeed, 65 reject as status 3, a short output leaves canary. Replay 2.5 release observations at extreme X/Z against merged debug/release provider after 2.4's wrapping fix. Stage every 8-byte record with zero reserved bytes, then publish once; invalid mode 3 cannot truncate or map to another mode. Validate `make rust`, `rustup run 1.97.1 cargo test --manifest-path packages/engine/Cargo.toml -p mornlea_engine --test numerical_migration --locked worldgen_probe`, `go test ./packages/shared/nativeabi -run 'TestWorldgenProbe' -count=1`, and `go test ./packages/tools/cmd/runtime-oracle -run '^TestKernelWorldgenProbe' -count=1`. **Commit:** `refactor(engine): route probe abi through native worldgen`.

<a id="node-3-7"></a>
## Node 3.7 — tree ABI

**Prerequisite:** 2.6 and latest `ffi.rs` SHA. **Files:** `src/ffi.rs` hunk `mornlea_tree_blocks`/`tree_blocks_with` retains `MTB1` admission and invokes `NativeTree`; `tests/numerical_migration/tree_blocks.rs` owns `tree_blocks_abi_order`. **Read-only:** tree provider, shared visitor and Go bridge.

Pin roots/seeds from 2.6 as 4-byte count plus every ordered 8-byte record, not only a digest. `needed=4+8*N` with checked arithmetic; `needed−1` returns status 7 and unchanged payload, while a larger destination's suffix stays unchanged. Stage exact bytes and copy only after success. Validate `make rust`, `rustup run 1.97.1 cargo test --manifest-path packages/engine/Cargo.toml -p mornlea_engine --test numerical_migration --locked tree_blocks`, `go test ./packages/shared/nativeabi -run 'TestTreeBlocks' -count=1`, and `go test ./packages/tools/cmd/runtime-oracle -run '^TestKernelTreeBlocks' -count=1`. **Commit:** `refactor(engine): route tree abi through native geometry`.

<a id="node-3-8"></a>
## Node 3.8 — LOD ABI

**Prerequisite:** 2.7, 3.5 and latest `ffi.rs` SHA. **Files:** `src/ffi.rs` hunk `mornlea_lod_shell`/`lod_shell_with` retains `MLD1`/tile admission, invokes `NativeLod` and translates count to byte need; `tests/numerical_migration/lod.rs` owns `lod_abi_exact_needed`. **Read-only:** LOD provider, 3.1 alias rules and Go bridge.

For steps 2/4/8 compare every ordered 20-byte quad. With destination `needed−1`, expect status 7, unchanged payload and metadata exactly `needed` bytes; exact retry succeeds. Compute `quad_count.checked_mul(20)`, pack into stage, publish once. Do not reset metadata before 3.1 preflight or turn stage ceiling into actual required bytes. Validate `make rust`, `rustup run 1.97.1 cargo test --manifest-path packages/engine/Cargo.toml -p mornlea_engine --test numerical_migration --locked lod`, `go test ./packages/shared/nativeabi -run 'TestLodShell' -count=1`, and `go test ./packages/tools/cmd/runtime-oracle -run '^TestKernelLod' -count=1`. **Commit:** `refactor(engine): route lod abi through native shell`.

<a id="node-3-9"></a>
## Node 3.9 — fluid evaluation ABI

**Prerequisite:** 2.8 and latest `ffi.rs` SHA. **Files:** `src/ffi.rs` hunk `mornlea_fluid_eval_batch`/`fluid_eval_batch_with` retains raw count behavior and invokes bounded native chunks; `tests/numerical_migration/fluid_eval.rs` owns `fluid_eval_abi_4097`. **Read-only:** fluid provider, contracts and Go bridge.

ABI v11 accepts a valid 4097-record request although one native call rejects 4097. Preflight exact `8+14*N` input, checked `12*N` output and existing nonnull rules; reserve complete output stage, call native in chunks `<=4096`, pack four slots per record, and publish only after all chunks succeed. For one byte short expect existing status **2**, not status 7, with unchanged output; count 0 remains accepted. Compare full 4097×12 bytes and canaries. Validate `make rust`, `rustup run 1.97.1 cargo test --manifest-path packages/engine/Cargo.toml -p mornlea_engine --test numerical_migration --locked fluid_eval`, `go test ./packages/shared/nativeabi -run 'TestFluidEval' -count=1`, and `go test ./packages/tools/cmd/runtime-oracle -run '^TestKernelFluidEval' -count=1`. **Commit:** `refactor(engine): route fluid eval abi through bounded batches`.

<a id="node-3-10"></a>
## Node 3.10 — fluid rescan ABI

**Prerequisite:** 2.9 and latest `ffi.rs` SHA. **Files:** `src/ffi.rs` hunk `mornlea_fluid_rescan`/`fluid_rescan_with` retains raw 0..17 range while invoking shared halo-safe scan; `tests/numerical_migration/fluid_rescan.rs` owns `fluid_rescan_outer_halo` and needed-byte tests. **Read-only:** rescan provider, contracts and Go bridge.

Raw outer air succeeds; an outer fluid source that actually reads missing halo maps `MissingHalo` to existing caught-panic status **9**, leaves payload intact and clears valid nonaliased metadata. Interior short output returns status 7, metadata `8+12*N`, unchanged payload; exact retry returns identical Y/Z/X positions. Do not reject all outer ranges eagerly or invent barrier blocks; preserve section-before-budget and center metadata rules. Pack full header/positions in stage before output. Validate `make rust`, `rustup run 1.97.1 cargo test --manifest-path packages/engine/Cargo.toml -p mornlea_engine --test numerical_migration --locked fluid_rescan`, `go test ./packages/shared/nativeabi -run 'TestFluidRescan' -count=1`, and `go test ./packages/tools/cmd/runtime-oracle -run '^TestKernelFluidRescan' -count=1`. **Commit:** `refactor(engine): route rescan abi through halo-safe core`.

<a id="node-3-11"></a>
## Node 3.11 — mesh ABI and late-overflow repair

**Prerequisite:** 2.10, 2.11 and latest `ffi.rs` SHA. **Files:** `src/ffi.rs` hunk `mornlea_mesh_section`/`mesh_section_with` retains structural parse and all-air exception, routes nonair to typed view/light/geometry and stages packed quads; `tests/numerical_migration/mesh.rs` owns `mesh_abi_late_overflow`. **Read-only:** mesh providers, contracts and Go bridge.

Use an accepted dense model+plant registry that emits more than the prior six-quads-per-cell assumption. With exactly 24,576 `u64` output slots, expect status 7, metadata zero and every slot unchanged; an old partial-prefix write is a genuine red. With full required capacity, compare all packed `u64` values/order. Raw all-air with semantically invalid but structurally valid unused registry still succeeds; nonair registry/emission/queue errors retain statuses 5/6/8, packing invariant maps to 9. Preserve 552,960-byte/8-aligned legacy scratch and 3.1 alias preflight. Stage all packed quads separately, then copy only after count and capacity pass; measure bounded allocation cost. Validate `make rust`, `rustup run 1.97.1 cargo test --manifest-path packages/engine/Cargo.toml -p mornlea_engine --test numerical_migration --locked mesh`, `go test ./packages/shared/nativeabi -run 'TestMeshSection' -count=1`, and `go test ./packages/tools/cmd/runtime-oracle -run '^TestKernelMesh' -count=1`. **Commit:** `fix(engine): publish mesh abi only after complete geometry`.
