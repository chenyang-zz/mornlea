# rust-native-numerical-closure ledger

## 2026-09-25 — interface-first planning

- Baseline: `3aba0b94` on `dev`, clean before this planning change. This is planning only; no Rust provider, ABI adapter, corpus route or F1/F2 runtime acceptance is claimed.
- User intent: continue the next Superpowers task decomposition against the final Rust-authoritative server/client-core and Godot/Python presentation direction; freeze each feature interface before independent implementations start so work can proceed concurrently.
- Scope ruling: the next executable F1 frontier is ten safe native numerical operations plus Rust pathfinding. Domain-event, protocol, region and seven save-codec changes have archived acceptance; complete F1 zero-gap acceptance and F2 Rust authority remain separate later gates. The engine ABI stays v11 and current Go authority stays selected.
- Superpowers: used the installed `brainstorming` skill to classify this as architectural planning and compare a shared-contract-first approach against direct per-family exports and a separate kernel-contract crate. Used `writing-plans` to create the linked worker briefs and independently reviewable task nodes. The user already requested planning, so no additional approval was needed to write reversible OpenSpec artifacts; runtime implementation remains unstarted.
- Design ruling: keep the existing windowless `mornlea_engine` owner. One controller-owned contract landing exports types and object-safe operation traits; independent family modules implement them. This avoids a new crate and keeps the target dependency direction. A new crate would add a second type-ownership seam before any numerical behavior exists. Direct per-family exports without a frozen shared contract would make physics, probe/LOD and future F2/F3 consumers wait for provider completion or guess signatures.
- Parallel topology: after 1.2, collision/physics, raycast, world sampling, fluid evaluation/rescan, mesh/light and pathfinding lanes can develop against the contract. Shared `lib.rs`, contract declarations, test registration, `ffi.rs` and corpus manifest have a single controller owner. Within the world and mesh lanes, nodes sharing implementation files are serial. ABI adaptation and final corpus acceptance are explicit serial integration points; dependency waits are distinct from unfinished design.
- Ownership: planning files inherit root guidance; no runtime directory is created in this round. Node 1.2 updates the engine guide when it creates `src/native/`; no new guide is planned for leaf test topic directories because they inherit the crate guide. Each provider exports temporary test cases; the controller owns source-bound case import, manifest hashes and every derived consumer gate.
- Decision coverage: field maps, units, ordering, validation, capacity, error precedence, immutable result ownership, scratch lifecycle, compatibility and rollback are specified in `design.md` and `worker-briefs.md`. The source snapshot in the archived kernel extraction is read-only evidence; current code/tests override it at implementation start. A contract conflict returns to the controller for artifact reconciliation, not worker invention.
- Planning validation: `openspec validate rust-native-numerical-closure --type change --strict --no-interactive` passed; `openspec status --change rust-native-numerical-closure` reports 4/4 artifacts. `git diff --check` passed for tracked files; new files are untracked at this point. No runtime test was run or represented as completed.
- Plan self-review: 29 unique checkbox nodes with no numbering gap. Requirement coverage maps typed calls to 1.2/2.1–2.13, bounds and atomicity to family tests/3.1, path choice to 2.12–2.13, ABI compatibility to 3.1–3.11, and executed evidence to 3.12. Ten adapter nodes match ten existing ABI families; the eleventh route is pathfinding. Dependency edges are acyclic: 1.2 precedes all providers, same-file world/mesh/path nodes are serial, controller-owned `ffi.rs` nodes are serial, and 3.12 waits for every family. The five highest-risk cases—late mesh overflow, metadata aliasing, outer rescan halo, 4096/4097 path budget, and worldgen signed extremes—each have a named red case and owner. The active documents have no broken relative links, trailing whitespace, or unfinished task placeholders. Strict all-project OpenSpec validation passed 127/127 items.
- Architecture skill: no change. The Rust numerical ownership and typed-boundary direction are already recorded; this interface is planned, not verified implementation evidence.

## 2026-09-25 — detailed dispatch revision

- User correction: every task must be detailed enough for a weaker implementer and explicitly preserve interface-first parallel progress. This revision remains planning only; every task checkbox stays open and no numerical implementation or corpus acceptance is claimed.
- Superpowers `brainstorming` and `writing-plans` were applied to review the previous decomposition against the final Rust-authoritative server/client core and Godot/Python presentation boundary. The controller inspected the current engine ABI functions, Go nativeabi tests, runtime-oracle registry/producer rules, Rust corpus loader and source ownership before authoring these packets.
- Dispatch readiness: `worker-briefs.md#dispatch-packets` now has one packet for each of the 29 task IDs, with baseline/predecessor, exclusive paths via `tasks.md`, exact interfaces via sections 5.1–5.14, concrete positive/error/boundary fixtures, expected status/capacity, test-first sequence, validation command reference, exclusions, review/commit and rollback. The ten ABI adapters distinguish baseline-green parity from genuine behavioral reds; no fictitious numerical mismatch is required to prove an internal route change.
- Interface ruling: node 1.2 owns the validated `CollisionCell`, `CollisionGrid` and `WorldgenParams` constructors. Physics and probe/LOD consumers can therefore build real fixtures and execute trait doubles before collision/chunk providers land. It also owns the crate-private `WorldgenParams::as_legacy()` conversion and allocation-free `worldgen::visit_tree_blocks` seam. Chunk, probe, tree and LOD provider files are disjoint after this landing; the signed-extreme shared-core correction is reconciled by later integrated parity, not used to block independent provider development.
- Corpus ruling: ten legacy numerical families use binary raw-request cases against exported Rust ABI symbols and Go-produced ABI observations; typed providers have separate native-contract/parity tests. `kernel.pathfind` uses one JSON/RLE fixture with independent Go and Rust search. Node 3.12 alone owns `BaselineConsumerRegistry`, the Rust `CorpusConsumer::Engine` loader, engine test dependencies, guides, manifest hashes and case assets. Public Go wrappers that panic on rejected status cannot produce exact raw error observations, so a package-local test-only `nativeabi/kernel_oracle_test.go` producer uses existing unexported bridge helpers; no production cgo/API is added. Closure requires nonempty per-family executed success/error/boundary counts and a comparison-failing scalar/path mutation. The corpus schema, status categories, capacity units and eleven minimum case rows are explicit in the dispatch packet.
- Ownership/risk correction: a new `src/native/AGENTS.md` accompanies the interface directory; `ffi.rs` remains serial controller-owned. The prior plan omitted the corpus registry and loader files and incorrectly required observable red output for behavior-preserving adapters. Both were corrected. Producer fragments remain temporary until reviewed import; current Go authority, ABI v11 and default runtime are unchanged.
- Static self-review: 29 unique task checkboxes and 29 unique matching packets, no missing/extra IDs; every local Markdown link resolves. Focused OpenSpec change validation and scoped `git diff --check` passed. `openspec validate --all --strict --no-interactive` passed 127/127 items. Architecture skill: no change; these are unimplemented decisions, not verified reusable rules.

## 2026-09-25 — archived-style file-level plan revision

- User correction: use the recently archived OpenSpec storage closure's file-specific packet style for **every** current task. This is a planning-only revision; all 29 checkboxes remain open, no provider or corpus case is accepted, and no default runtime changes.
- The controller applied Superpowers `brainstorming` to preserve the accepted interface-first dependency graph and `writing-plans` to assign exact files, APIs, test fixtures, expected red/green results, focused gates, exclusions, commit and rollback to every node. The archived `rust-storage-codec-closure/plans/` was a structural example only; current code, tests, active spec and target architecture remain authoritative.
- Added `plans/00-foundation.md` for baseline/interface and full module/test-root creation, `plans/01-providers.md` for all 13 bounded providers, `plans/02-adapters.md` for the metadata repair and ten serialized ABI adapters, and `plans/03-closure.md` for eleven source-bound corpus routes plus review and integrated gates. Every `tasks.md` checkbox links to exactly one node anchor. The shared field/algorithm rulings remain in `worker-briefs.md` and are referenced from each packet rather than copied into conflicting variants.
- Parallelism ruling: node 1.2 is the only shared-contract landing; disjoint provider files can begin on its accepted SHA. Mesh and path continuations and all `ffi.rs` edits are serial by actual shared-file ownership. Metadata alias repair can begin after the interface and precedes every adapter. The controller owns corpus registration and SHA refresh only after reviewed producer fragments; this does not block independent provider tests.
- File-ownership correction: 1.2 also creates and registers empty `tests/native_contract/publication.rs`, so 3.1 can add its public atomicity cases without editing a controller-only test root. The explicit file packets name each provider's test-only Go producer, exact Rust source/test files, and read-only peers.
- Static check: 29 unique open checkbox IDs, 29 links and 29 matching plan anchors; no broken local Markdown target or unfinished plan placeholder. Focused strict OpenSpec validation, scoped `git diff --check`, and `openspec validate --all --strict --no-interactive` passed (127/127). Pre-existing `.cursor/` deletions and the user-owned root `AGENTS.md` edit were left untouched. Architecture skill: no change; this revision adds implementation instructions, not verified cross-task code facts.

## 2026-09-25 — Node 1.1 baseline pinning

- Execution baseline: `372da827b3f7c15690b7a75855943d16b04090c8` on branch `feat/rust-native-numerical-closure`, initially clean.
- Four accepted predecessor change SHAs:
  - Domain event completion: archive `44f15ac5a4202f635a589c5b9ce4313c9eead1ca`, code result `b390e5314eeef76c1231f28b49e6dcf3b12ce0ff`.
  - Protocol completion: archive `81a56bb897e9e623eb64fbcad093bafe430f81d5`, code result `cbbd1de9b531bc16a1b24bfba6c8d35661601004`.
  - Region format completion: archive `380428873722e0300dc976077557e937d2f93427`, code result `699d4cd42b26b48220feceae1df7f7fa9e2a6d71`.
  - Storage codec closure: archive `e9d272f9011e0e891c784346eb4a2d1d053257df`, code result `d4b08a0b1eae7889ef43c96e54626559d69be3e3`.
- Current engine source identity and ABI:
  - ABI version: v11 (`mornlea_engine_abi_version() -> 11`).
  - `packages/engine/crates/mornlea_engine/src/ffi.rs`: `sha256:703d20c76b2d7f16d2e0933badf6e60fc2e8dd32be60f05607de63a529cedaa2`.
  - `packages/engine/crates/mornlea_engine/src/lib.rs`: `sha256:11dc7fb9641630a6704ac7052108dd60f2ddfc29fdcbf128133e1dd319153804`.
  - Ten exported numerical C symbols verified in `src/ffi.rs`: `mornlea_collision_resolve`, `mornlea_fluid_eval_batch`, `mornlea_fluid_rescan`, `mornlea_lod_shell`, `mornlea_mesh_section`, `mornlea_physics_step`, `mornlea_raycast_batch`, `mornlea_tree_blocks`, `mornlea_worldgen_chunk`, `mornlea_worldgen_probe`.
- Manifest inspection (`testdata/runtime-migration/contracts.json`):
  - Eleven kernel families enumerated; all currently have zero accepted cases (null):
    1. `kernel.mornlea_collision_resolve` (version 11, cases: 0)
    2. `kernel.mornlea_fluid_eval_batch` (version 11, cases: 0)
    3. `kernel.mornlea_fluid_rescan` (version 11, cases: 0)
    4. `kernel.mornlea_lod_shell` (version 11, cases: 0)
    5. `kernel.mornlea_mesh_section` (version 11, cases: 0)
    6. `kernel.mornlea_physics_step` (version 11, cases: 0)
    7. `kernel.mornlea_raycast_batch` (version 11, cases: 0)
    8. `kernel.mornlea_tree_blocks` (version 11, cases: 0)
    9. `kernel.mornlea_worldgen_chunk` (version 11, cases: 0)
    10. `kernel.mornlea_worldgen_probe` (version 11, cases: 0)
    11. `kernel.pathfind` (version 1, cases: 0)
  - Source SHA-256 validation (all match checked-out files):
    - `packages/engine/include/mornlea_engine.h`: `sha256:26d19f6fc265ebee750ff7ce9f15212321237c2f94f137ff2ff7af652dc58c9f`
    - `packages/shared/nativeabi/native_test.go`: `sha256:43e3da7c40a54cd568e1497c4e92d73b8ee497818732e78f8706073a18fef20c`
    - `packages/engine/crates/mornlea_engine/src/collision.rs`: `sha256:ed8adb64ca6a14c0b205eba6d262b632089ba2bce4fc3e4fc029b1702aa99507`
    - `packages/engine/crates/mornlea_engine/src/fluid_eval.rs`: `sha256:47716d86f213cd003cae72bc4773ff39ea08546bea83f234b2bcc503d3bbc2a0`
    - `packages/engine/crates/mornlea_engine/src/fluid_rescan.rs`: `sha256:d9fab8dc772fc66496ddee739c6e30bdc8e648473141680b340f2d41be658fd9`
    - `packages/engine/crates/mornlea_engine/src/lod.rs`: `sha256:dfdc994ed2cf305e32679b2f0500f7634e060f65dae6de458836071aba20f62a`
    - `packages/engine/crates/mornlea_engine/src/greedy/mod.rs`: `sha256:b24091657579b1d3c1aa996a445fc6c9d2c2edadc89f054ae762553b15e98b50`
    - `packages/engine/crates/mornlea_engine/src/step.rs`: `sha256:188cb73517c29e471afc2733a0c49bced834087ed04871ca408dabf5b7446aac`
    - `packages/engine/crates/mornlea_engine/src/raycast.rs`: `sha256:61acf71d6bf411a4d0fc3e2698ab6881f0baa7e26ba4d184abdb3cad53dd5652`
    - `packages/engine/crates/mornlea_engine/src/worldgen.rs`: `sha256:1bd2d720ee17e975bc62a8ca0673a504b81634cc506adcd1e262beb6bffb66bd`
    - `packages/shared/pathfind/pathfind.go`: `sha256:228bbfa1c2682a242f89084f59c2843a415829fae534ad3b0c048494b7263878`
    - `packages/shared/pathfind/pathfind_test.go`: `sha256:540bbaf3d8d3d12b2717e0c2daa9e8e38d22a6e469d895c7ea22d0970571e031`
- Validation commands:
  - `go test ./packages/tools/cmd/runtime-oracle -run '^TestContractInventory' -count=1`: passed (ok github.com/channing771/mornlea/packages/tools/cmd/runtime-oracle 3.968s).
  - `rustup run 1.97.1 cargo test --manifest-path packages/engine/Cargo.toml -p mornlea_engine --locked -- --list`: passed (262 unit tests discovered, 0 benchmarks, 0 doc-tests).
- Outcome: Baseline verified; no engine symbol or version drift. All 11 numerical routes confirmed uncovered without corpus mutation. Ready for Node 1.2 interface landing.
- Rollback: Revert this ledger entry only.

## 2026-09-25 — Node 1.2 interface contract landing

- Predecessor SHA: `0aeb35e6244a59adc2639ef187576dea1a5119b2`
- Result SHA: `dabde3465b8a0babdcde8e59698457fc605b8598`
- Implementation summary:
  - All eleven object-safe operation traits, shared request/result structs, capacity unit checkers and `KernelError`/`PathError` enums defined in `packages/engine/crates/mornlea_engine/src/native/contracts/`.
  - Shared validated constructors implemented and tested: `CollisionCell::try_new`, `CollisionGrid::try_new`, `WorldgenParams::try_new`, `RayBatch::from_parts`, `TreeBlocks::from_parts`, `PathResult::new`.
  - Crate-private sampler bridge `WorldgenParams::as_legacy()` tested with field-for-field parity.
  - Allocation-free `visit_tree_blocks` seam in `worldgen.rs` preserves dy/dz/dx ordering, <= 128 record bound, and early refusal abortion.
  - Test roots `tests/native_contract.rs` and `tests/numerical_migration.rs` registered.
  - Test doubles for all 11 traits compiled and exercised in `tests/native_contract/contracts.rs`, with `FluidEvalOp` double tested for both success and typed error.
  - `src/native/AGENTS.md` created; `crates/mornlea_engine/AGENTS.md` updated.
- Verification:
  - `rustup run 1.97.1 cargo test --manifest-path packages/engine/Cargo.toml -p mornlea_engine --test native_contract --locked -- --list`: passed (6 tests discovered).
  - `rustup run 1.97.1 cargo test --manifest-path packages/engine/Cargo.toml -p mornlea_engine --test native_contract --locked`: passed (6 tests passed, 0 failures).
  - `rustup run 1.97.1 cargo test --manifest-path packages/engine/Cargo.toml -p mornlea_engine --lib --locked tree_blocks`: passed (2 tests passed).
  - `rustup run 1.97.1 cargo clippy --manifest-path packages/engine/Cargo.toml -p mornlea_engine --all-targets --locked -- -D warnings`: passed cleanly.
- Review: Task Reviewer subagent approved both Spec Compliance and Code Quality.
- Rollback: Revert `dabde346` prior to dependent provider tasks.

## 2026-09-25 — Node 2.1 collision provider

- Predecessor SHA: `b61f6393e32def407418d70c50b590c0d9603365`
- Result SHA: `cc93cc9e19265f0b93167344144c2f59c324c861`
- Implementation summary:
  - `src/collision.rs` refactored to expose the internal resolution engine through a crate-private `CollisionCells` read view, enabling zero-copy native grid consumption without ABI re-encoding.
  - `src/native/collision.rs` implements `CollisionOp` for `NativeCollision` and `resolve_collision`, with finite float validation and exact swept prism coverage validation (`KernelError::DisplacementOutOfBounds`).
  - `tests/native_contract/collision.rs` exercises loaded floor/wall, unloaded neighbors, negative-zero displacement, swept boundary coverage, 4096/4097 cells, and NaN handling.
  - `tests/numerical_migration/collision.rs` verifies bitwise parity against ABI observations.
  - `packages/tools/cmd/runtime-oracle/kernel_collision_test.go` adds `TestKernelCollision` covering valid execution and panic handling on short input/output and invalid headers.
- Verification:
  - `rustup run 1.97.1 cargo test --manifest-path packages/engine/Cargo.toml -p mornlea_engine --test native_contract --locked collision`: passed (8 tests passed).
  - `rustup run 1.97.1 cargo test --manifest-path packages/engine/Cargo.toml -p mornlea_engine --test numerical_migration --locked collision`: passed (1 test passed).
  - `rustup run 1.97.1 cargo test --manifest-path packages/engine/Cargo.toml -p mornlea_engine --lib --locked collision`: passed (14 tests passed).
  - `go test ./packages/tools/cmd/runtime-oracle -run '^TestKernelCollision' -count=1`: passed (ok github.com/channing771/mornlea/packages/tools/cmd/runtime-oracle 0.509s).
  - `rustup run 1.97.1 cargo clippy --manifest-path packages/engine/Cargo.toml -p mornlea_engine --all-targets --locked -- -D warnings`: passed cleanly.
- Review: Task Reviewer subagent approved after addressing test bounds and comment hygiene in two fix rounds.
- Rollback: Revert `cc93cc9e`.

## 2026-09-25 — Node 2.2 physics provider

- Predecessor SHA: `20026bb4409e33687258351c24aba4329356775e`
- Result SHA: `24dccf0163791a3a0dfc70e8b51b784ae3dbbbcd`
- Implementation summary:
  - `src/step.rs` refactored to extract shared `integrate` logic without altering formula order or float math.
  - `src/native/physics.rs` implements `PhysicsOp` for `NativePhysics` and `step_physics`. Validates finite floats, move axes -1..=1, sweep bounds, checks displacement within one ULP tolerance (`DisplacementOutOfBounds`), executes collision parts via local `PhysicsGridWrapper`, and zeroes clipped velocity axes.
  - `tests/native_contract/physics.rs` tests loaded floor landing, jump, fluid drag and gravity, sneak priority over sprint, one ULP allowance, two ULP rejection, and nonfinite/invalid input rejections.
  - `tests/numerical_migration/physics.rs` tests bitwise exact parity with ABI observations.
  - `packages/tools/cmd/runtime-oracle/kernel_physics_test.go` exercises `nativeabi.PhysicsStep` on valid and error conditions with panic recovery.
- Verification:
  - `rustup run 1.97.1 cargo test --manifest-path packages/engine/Cargo.toml -p mornlea_engine --test native_contract --locked physics`: passed (8 tests passed).
  - `rustup run 1.97.1 cargo test --manifest-path packages/engine/Cargo.toml -p mornlea_engine --test numerical_migration --locked physics`: passed (1 test passed).
  - `rustup run 1.97.1 cargo test --manifest-path packages/engine/Cargo.toml -p mornlea_engine --lib --locked physics_step`: passed (11 tests passed).
  - `go test ./packages/tools/cmd/runtime-oracle -run '^TestKernelPhysics' -count=1`: passed (ok github.com/channing771/mornlea/packages/tools/cmd/runtime-oracle 0.505s).
  - `rustup run 1.97.1 cargo clippy --manifest-path packages/engine/Cargo.toml -p mornlea_engine --all-targets --locked -- -D warnings`: passed cleanly.
- Review: Task Reviewer subagent approved after one fix round addressing an unused variable in the test file.
- Rollback: Revert `24dccf01`.

## 2026-09-25 — Node 2.3 ray traversal provider

- Predecessor SHA: `e40a73a530e2e8a45af0ca27c6945dbf92833298`
- Result SHA: `75a047c9ddbe41baf61998076004ec68a6fa0bcb`
- Implementation summary:
  - `src/raycast.rs` exposed raycast traversal step and internal cursor math.
  - `src/native/raycast.rs` implements `NativeRaycast` for `RaycastOp`, with commit-on-success semantics (mutates cursor only after `RayBatch::from_parts` succeeds), handling `done` repeat calls, negative floors, tie priorities, endpoint inclusion, and wrapping math.
  - `tests/native_contract/raycast.rs` tests input validation on `RayCursor::try_new`, multi-batch 65-record traversal, repeated done, negative floor, tie priority, endpoint inclusion, and verifies cursor remains unchanged on error.
  - `tests/numerical_migration/raycast.rs` verifies multi-batch bitwise parity with ABI observations.
  - `packages/tools/cmd/runtime-oracle/kernel_raycast_test.go` exercises `nativeabi.RaycastBatch` on valid multi-batch cases and panic error conditions.
- Verification:
  - `rustup run 1.97.1 cargo test --manifest-path packages/engine/Cargo.toml -p mornlea_engine --test native_contract --locked raycast`: passed (7 tests passed).
  - `rustup run 1.97.1 cargo test --manifest-path packages/engine/Cargo.toml -p mornlea_engine --test numerical_migration --locked raycast`: passed (1 test passed).
  - `rustup run 1.97.1 cargo test --manifest-path packages/engine/Cargo.toml -p mornlea_engine --lib --locked raycast`: passed (11 tests passed).
  - `go test ./packages/tools/cmd/runtime-oracle -run '^TestKernelRaycast' -count=1`: passed (ok github.com/channing771/mornlea/packages/tools/cmd/runtime-oracle 0.498s).
  - `rustup run 1.97.1 cargo clippy --manifest-path packages/engine/Cargo.toml -p mornlea_engine --all-targets --locked -- -D warnings`: passed cleanly.
- Review: Task Reviewer subagent approved after two fix rounds ensuring robust `RayCursor::try_new` tests, true commit-on-success in `next_batch`, and clean comments.
- Rollback: Revert `75a047c9`.

## 2026-09-25 — Node 2.4 world chunk provider

- Predecessor SHA: `06869a1cb62234fb56ffbcd3694f432f72df9fb3`
- Result SHA: `b81d0dad`
- Implementation summary:
  - `src/worldgen.rs` replaces debug-overflow-prone coordinate and fringe arithmetic with explicit two's-complement wrapping (`wrapping_shl` for chunk base, `wrapping_add`/`wrapping_sub` for world coordinates and tree/short-grass fringe windows, `wrapping_sub` for crown tier matching) while preserving signed comparisons and ascending inclusive ranges; an inverted wrapped window iterates zero times instead of exploding. Y arithmetic stays bounded and ordinary.
  - `src/native/worldgen.rs` implements `NativeWorldgen` for `WorldgenOp`, preflighting destination capacity before any write, generating into the caller-owned `WorldgenScratch` stage, and copying exactly 98,304 cells so a short destination is untouched and surplus destination cells keep their canary.
  - `src/native/mod.rs` widens the `worldgen` module to `pub` so integration tests can name `NativeWorldgen`, matching the ray lane.
  - `tests/native_contract/worldgen_chunk.rs` covers exact/short/extra capacity, destination canaries, full destination overwrite on success, scratch reuse, and the shared duplicate-material constructor rule (`water == air` accepted, other duplicates rejected).
  - `tests/numerical_migration/worldgen_chunk.rs` pins bitwise cell parity for seed 0 at chunk (0,0), extreme-coordinate FNV-1a 64 digests at both signed extremes, seeds 0/1/-1 at chunk (0,0) and (-1,2), and a modified permutation, cross-checked against an independent bedrock-layer rule and a digest mutation check.
  - `packages/tools/cmd/runtime-oracle/kernel_worldgen_chunk_test.go` freezes release ABI observations from `nativeabi.WorldgenChunk` (FNV-1a 64 over all 98,304 output cells) for the seed/chunk/permutation matrix and exercises eight panic conditions.
- Verification:
  - `rustup run 1.97.1 cargo test --manifest-path packages/engine/Cargo.toml -p mornlea_engine --test native_contract --locked worldgen_chunk`: passed (6 tests passed).
  - `rustup run 1.97.1 cargo test --manifest-path packages/engine/Cargo.toml -p mornlea_engine --test numerical_migration --locked worldgen_chunk`: passed (4 tests passed).
  - `rustup run 1.97.1 cargo test --manifest-path packages/engine/Cargo.toml -p mornlea_engine --lib --locked worldgen`: passed (43 tests passed).
  - `go test ./packages/tools/cmd/runtime-oracle -run '^TestKernelWorldgenChunk' -count=1`: passed (ok github.com/channing771/mornlea/packages/tools/cmd/runtime-oracle 0.732s).
  - `rustup run 1.97.1 cargo clippy --manifest-path packages/engine/Cargo.toml -p mornlea_engine --all-targets --locked -- -D warnings`: passed cleanly.
  - `go test ./packages/shared/nativeabi -run 'TestWorldgen' -count=1`: passed.
- Review: Task Reviewer subagent approved with three minor findings deferred to the final whole-branch review (controller ratification of the `src/native/mod.rs` visibility boundary, Go oracle rejection paths missing an output-untouched assertion behind an accurate comment, and a missing `water == stone` duplicate rejection case).
- Rollback: Revert `b81d0dad`.

## 2026-09-25 — Node 2.5 world probe provider

- Predecessor SHA: `722e8b3a`
- Result SHA: `8b75dad4`
- Implementation summary:
  - `src/native/world_probe.rs` implements `NativeWorldProbe` for `ProbeOp`, validating the query count bound (1..=64) before evaluation, rejecting a short destination with `OutputTooSmall { needed, available }` without touching it, converting parameters once through `as_legacy()`, evaluating into a fixed 64-entry local stage, and publishing exactly the used prefix after every query succeeds.
  - `src/native/mod.rs` widens the `world_probe` module to `pub` so integration tests can name the provider.
  - `tests/native_contract/worldgen_probe.rs` covers mode mapping, out-of-world Y air, seeded decoration reachable through the Base layer, 0/1/64/65 count bounds, short-output canary, surplus-destination suffix preservation, query-order preservation, and reuse safety.
  - `tests/numerical_migration/worldgen_probe.rs` pins bitwise probe values for both seeds at ordinary positive/negative coordinates across all three modes, Height Y-independence, extreme-coordinate determinism with tree fringe wrapping across both signed extremes, and a mutated-permutation digest difference.
  - `packages/tools/cmd/runtime-oracle/kernel_worldgen_probe_test.go` freezes release ABI observations for the seed/mode/coordinate matrix and exercises the failure inventory with post-rejection output byte-identity checks.
- Verification:
  - `rustup run 1.97.1 cargo test --manifest-path packages/engine/Cargo.toml -p mornlea_engine --test native_contract --locked worldgen_probe`: passed (8 tests passed).
  - `rustup run 1.97.1 cargo test --manifest-path packages/engine/Cargo.toml -p mornlea_engine --test numerical_migration --locked worldgen_probe`: passed (6 tests passed).
  - `rustup run 1.97.1 cargo test --manifest-path packages/engine/Cargo.toml -p mornlea_engine --lib --locked probe`: passed (3 tests passed).
  - `go test ./packages/tools/cmd/runtime-oracle -run '^TestKernelWorldgenProbe' -count=1`: passed.
  - `rustup run 1.97.1 cargo clippy --manifest-path packages/engine/Cargo.toml -p mornlea_engine --all-targets --locked -- -D warnings`: passed cleanly.
  - `go test ./packages/shared/nativeabi -run 'TestWorldgenProbe' -count=1`: passed.
- Review: Task Reviewer subagent approved with three minor findings deferred to the final whole-branch review (oracle export style wording, duplicated release-ABI fixture across the two test roots, and an overclaiming fixture test name). The reviewer verified two named risks against source: the probe ABI input frame has no reserved field, so the brief's reserved-bytes rejection was inapplicable and the worker's mode-word/reserved-zero/long-output mapping is the correct reading; and the extreme-coordinate tree fringe genuinely wraps across both signed extremes.
- Rollback: Revert `8b75dad4`.

## 2026-09-25 — Derived-artifact ruling: tracked source hashes

- `testdata/runtime-migration/contracts.json` pins a `sha256` per family source file and `TestStorageCorpus` in `packages/tools/cmd/runtime-oracle` reconciles those hashes against the working tree. The inventory was green at the change baseline `372da827` and drifted at nodes 1.2 (`src/worldgen.rs`), 2.1 (`src/collision.rs`), 2.2 (`src/step.rs`) and 2.3 (`src/raycast.rs`); every node used filtered `-run '^TestKernel*'` commands, so the drift went unnoticed until a package-wide `go test ./packages/tools/cmd/runtime-oracle` was run.
- Ruling: the controller is the refresh authority for this derived artifact. Whenever a tracked hashed source changes, the affected `sources[].sha256` entries are refreshed in the same commit, following the archived storage/protocol pattern ("per-family commits keep the existing equal source-revision fields and update exact `Family.Sources` hashes"). `source_revision` stays `f75dfcf4db03eebdbf07deb5e3ff512e6c417a28`; cases and corpus assets remain exclusive to the closure node.
- `9f7e3801` applies rustfmt and gofmt (required by the closure gates, previously red on 38 files and 2 Go producers) and refreshes the four drifted source hashes. `go test ./packages/tools/cmd/runtime-oracle -count=1` is green again.
- Consequence for remaining nodes: any node whose editable set includes `src/collision.rs`, `src/step.rs`, `src/raycast.rs`, `src/worldgen.rs`, `src/fluid_eval.rs`, `src/fluid_rescan.rs`, `src/lod.rs`, `src/greedy/mod.rs` or `packages/engine/include/mornlea_engine.h` must land the matching hash refresh, and the package-wide runtime-oracle run is part of that node's evidence rather than only the filtered kernel test.

## 2026-09-25 — Node 2.6 runtime tree provider

- Predecessor SHA: `0ca11ac5`
- Result SHA: `f6d7ee0e`
- Implementation summary:
  - `src/native/tree.rs` implements `NativeTree` for `TreeOp`, admitting roots exactly as the legacy ABI does (X/Z ±2 must be representable, root Y in `-64..=311`, all seeds accepted) before any geometry read, evaluating through the read-only `worldgen::visit_tree_blocks` seam into a fixed 128-record local stage, and returning `TreeBlocks::from_parts` on success or `KernelError::OutputInvariant` when the visitor reports an overflow — never truncating or returning a partial tree.
  - `src/native/mod.rs` widens the `tree` module to `pub` so integration tests can name the provider.
  - `tests/native_contract/tree_blocks.rs` covers root Y and X/Z edges on both sides, strict `(dy, dz, dx)` ordering with offset and block-id domains, seed-selected trunk heights 5/6/7 and the fluffy crown, record-count bounds across 32 seeds and 5 roots, and the shared constructor's oversize-length rejection.
  - `tests/numerical_migration/tree_blocks.rs` pins ordered `(dx, dy, dz, block)` literal sequences for the height 5/6/7 and fluffy-crown families, matching the Go observations record for record, plus determinism and seed divergence.
  - `packages/tools/cmd/runtime-oracle/kernel_tree_blocks_test.go` freezes ordered geometry observations for seeds 0/1 at both root-Y bounds and both representable horizontal extremes, and exercises recovered failures with canary-protected untouched output.
- Verification:
  - `rustup run 1.97.1 cargo test --manifest-path packages/engine/Cargo.toml -p mornlea_engine --test native_contract --locked tree_blocks`: passed (6 tests passed).
  - `rustup run 1.97.1 cargo test --manifest-path packages/engine/Cargo.toml -p mornlea_engine --test numerical_migration --locked tree_blocks`: passed (6 tests passed).
  - `rustup run 1.97.1 cargo test --manifest-path packages/engine/Cargo.toml -p mornlea_engine --lib --locked tree_blocks`: passed (8 tests passed).
  - `go test ./packages/tools/cmd/runtime-oracle -run '^TestKernelTreeBlocks' -count=1`: passed.
  - `go test ./packages/tools/cmd/runtime-oracle -count=1`: passed (source-hash reconciliation intact).
  - `rustup run 1.97.1 cargo clippy --manifest-path packages/engine/Cargo.toml -p mornlea_engine --all-targets --locked -- -D warnings`, `cargo fmt --all --check`, `gofmt -l ./packages`: all clean.
- Review: Task Reviewer subagent approved after the controller ruled on one plan-mandated deviation: the dispatch brief listed "long output" as a recovered failure, but the frozen tree ABI is at-least-capacity (`src/ffi.rs` rejects only `output_len < needed`), so a larger buffer correctly succeeds with an untouched suffix. The brief was pattern-matched from the chunk/probe exact-length semantics and is amended here; the implementation stands. Two minor findings deferred to the final whole-branch review (no negative or extreme seed pins the no-seed-validation rule; ~250 lines of record literals duplicated across the Go and Rust pins).
- Rollback: Revert `f6d7ee0e`.

## 2026-09-25 — Node 2.7 LOD shell provider

- Predecessor SHA: `11401286`
- Result SHA: `ad781b71` (provider `5cd4b11e`, controller hash refresh `0126c19a`, capacity fix `ad781b71`)
- Implementation summary:
  - `src/lod.rs` gains two allocation-free seams without changing behavior: `WindowField<'a>` borrows its window cells as `[[i32; 2]]`, `sample_field_into` fills the caller's 1156-record sample buffer in the existing `(gj+1)*(n+2)+(gi+1)` order, and `visit_lod_shell` carries the former `build_shell` body behind an emitter. `build_shell` and `lod_shell` delegate through the seams and keep their output; the golden-shell byte test is untouched.
  - `src/native/lod.rs` implements `NativeLod` for `LodOp`, admitting tiles exactly as the legacy ABI does (per axis `checked_mul(64)`, then `checked_add(64)`, then `checked_sub(8)` as independent chains), sampling into `LodScratch::samples`, emitting into `LodScratch::stage`, and publishing only after the exact required count is known.
  - `tests/native_contract/lod.rs` covers steps 2/4/8 with ordered quad fields, the taller-side-owned skirt, the sea clamp with and without `water == air`, tile overflow preflight on both axes, exact/short/surplus destinations with canaries, and warm scratch reuse across steps and across a failure.
  - `tests/numerical_migration/lod.rs` pins ordered quad parity against the Go observations for one tile per step size, determinism, and exact required-count parity.
  - `packages/tools/cmd/runtime-oracle/kernel_lod_test.go` freezes ordered LOD observations and exercises recovered failures.
- Verification:
  - `rustup run 1.97.1 cargo test --manifest-path packages/engine/Cargo.toml -p mornlea_engine --test native_contract --locked lod`: passed (5 tests passed).
  - `rustup run 1.97.1 cargo test --manifest-path packages/engine/Cargo.toml -p mornlea_engine --test numerical_migration --locked lod`: passed (6 tests passed).
  - `rustup run 1.97.1 cargo test --manifest-path packages/engine/Cargo.toml -p mornlea_engine --lib --locked lod`: passed (22 tests passed, golden shell bytes stable).
  - `go test ./packages/tools/cmd/runtime-oracle -run '^TestLodShell' -count=1`: passed.
  - `rustup run 1.97.1 cargo clippy --manifest-path packages/engine/Cargo.toml -p mornlea_engine --all-targets --locked -- -D warnings`, `cargo fmt --all --check`, `gofmt -l ./packages`: all clean.
  - `go test ./packages/tools/cmd/runtime-oracle -count=1`: passed after the controller's source-hash refresh (`src/lod.rs` `dfdc994e…` -> `9c6c7a17…`).
- Review: Task Reviewer subagent required one fix round for the exact-capacity boundary, which the implementer fixed in `ad781b71` by building into `&mut exact[..needed]` and asserting `Ok(needed)` with every slot written; the scoped re-review marked it ADDRESSED with no new breakage. The reviewer's second finding was ruled waived: a zero-quad tile is unreachable through the frozen `WorldgenParams::try_new` contract (terrain is never air), so the empty-tile evidence is the seam test `empty_tile_produces_no_quads` and no contract entry was added. One minor deferred to the final whole-branch review: the Go ordered walk omits the skirt `w == step` check the Rust walk asserts.
- Rollback: Revert `ad781b71`, `0126c19a`, `5cd4b11e` in that order.

## 2026-09-25 — Node 2.8 fluid evaluation provider

- Predecessor SHA: `21103c78`
- Result SHA: `cf0d8ae5` (provider `cf0d8ae5`, controller hash refresh `c20261c8`)
- Implementation summary:
  - `src/native/fluid_eval.rs` implements `NativeFluidEval` for `FluidEvalOp`, preflighting the batch count (0..=4096) and then destination capacity, evaluating each item through the existing `eval_one` rules into a stack-local 12-byte record, and decoding it into a typed four-entry `FluidWrites` with the no-write sentinel skipped so padding is never published.
  - `src/fluid_eval.rs` widens `SLOT_NO_WRITE` to `pub(crate)` and changes nothing else; no fluid rule moved.
  - `tests/native_contract/fluid_eval.rs` pins vertical-before-horizontal priority, four contiguous write slots, non-publication of unused entries, unknown id 65535 as nonfluid, the 0/1/4096/4097 count bounds with a test that fails if the preflight order is swapped, exact/short/surplus destinations with canaries, and the established decay, survival, infinite-source and vertical-over-horizontal rules.
  - `tests/numerical_migration/fluid_eval.rs` pins typed write parity and a digest over the Go-observed wire records, determinism, and the native 4096/4097 boundary.
  - `packages/tools/cmd/runtime-oracle/kernel_fluid_eval_test.go` freezes the rule matrix at batch sizes 0, 1 and 4096, records that the legacy ABI still accepts 4097 as an observation only, and asserts output-untouched canaries on every recovered failure.
- Verification:
  - `rustup run 1.97.1 cargo test --manifest-path packages/engine/Cargo.toml -p mornlea_engine --test native_contract --locked fluid_eval`: passed (7 tests passed).
  - `rustup run 1.97.1 cargo test --manifest-path packages/engine/Cargo.toml -p mornlea_engine --test numerical_migration --locked fluid_eval`: passed (3 tests passed).
  - `rustup run 1.97.1 cargo test --manifest-path packages/engine/Cargo.toml -p mornlea_engine --lib --locked fluid_eval`: passed (31 tests passed).
  - `go test ./packages/tools/cmd/runtime-oracle -run '^TestFluidEval' -count=1`: passed.
  - `rustup run 1.97.1 cargo clippy --manifest-path packages/engine/Cargo.toml -p mornlea_engine --all-targets --locked -- -D warnings`, `cargo fmt --all --check`, `gofmt -l ./packages`: all clean.
  - `go test ./packages/tools/cmd/runtime-oracle -count=1`: passed after the controller's source-hash refresh (`src/fluid_eval.rs` `47716d86…` -> `edee1299…`).
- Review: Task Reviewer subagent approved with no Critical or Important findings and two minor findings deferred to the final whole-branch review (a vacuous zero-length check in the Go oracle, and an anti-padding predicate that would false-positive on a legitimate self-cell air write).
- Rollback: Revert `c20261c8`, `cf0d8ae5` in that order.

## 2026-09-25 — Round-end governance retrospective

- Architecture skill: promoted one rule to the synchronized `mornlea-architecture` skill. `testdata/runtime-migration/contracts.json` is a live provenance registry whose per-family `sources[].sha256` pins are reconciled against the working tree by `TestStorageCorpus`; any hashed-source change must land the matching refresh before the node closes, `source_revision` stays pinned, corpus assets stay closure-owned, and only a package-wide runtime-oracle run proves the reconciliation. Verified against `packages/tools/cmd/runtime-oracle/inventory.go`, `storage_coverage_test.go`, the archived storage-closure ledger's per-family hash-refresh pattern, and the drift reproduced across nodes 1.2-2.5 of this change. No other finding met the promotion bar; everything else recorded here is task history or already specified in the change's own briefs.

## 2026-09-25 — Node 2.9 fluid rescan provider

- Predecessor SHA: `dc0684b4`
- Result SHA: `1866f315` (provider `1866f315`, controller hash refresh `ff2c119c`)
- Implementation summary:
  - `src/fluid_rescan.rs` extracts the canonical rescan accounting loop into one shared `rescan_scan` read through a small access capability, with `section_cell_index` and `box_to_world` replacing the previously inline arithmetic. `fluid_rescan` becomes a thin byte-encoding adapter and its golden output is unchanged; the legacy `skirt_column` panic path stays as it was.
  - `src/native/fluid_rescan.rs` implements `NativeFluidRescan` for `FluidRescanOp` plus the typed read accessor: closed box coordinates `1..=16`, `start_section < 24`, scratch capacity `area * 16 * (24 - start_section)`, `MissingHalo` only on an actually-read out-of-owned coordinate, out-of-world Y still answering the barrier for sealed-source parity, and a single publication pass after the scan.
  - `tests/native_contract/fluid_rescan.rs` covers budget accounting with entered-section completion, exact world positions for unsealed edge sources, Y/Z/X emission order, `start_section` 23 completion and break, range rejection, `ScratchTooSmall` worst-case math, exact/short/surplus destinations with canaries, center-over-metadata precedence, and scratch reuse after a rejected call.
  - `tests/numerical_migration/fluid_rescan.rs` pins position and summary parity against the Go observations, determinism, and typed-layer outer-column rejection.
  - `packages/tools/cmd/runtime-oracle/kernel_fluid_rescan_test.go` freezes the observation matrix including the interior halo case and asserts output-untouched canaries on every recovered failure.
- Verification:
  - `rustup run 1.97.1 cargo test --manifest-path packages/engine/Cargo.toml -p mornlea_engine --test native_contract --locked fluid_rescan`: passed (8 tests passed).
  - `rustup run 1.97.1 cargo test --manifest-path packages/engine/Cargo.toml -p mornlea_engine --test numerical_migration --locked fluid_rescan`: passed (3 tests passed).
  - `rustup run 1.97.1 cargo test --manifest-path packages/engine/Cargo.toml -p mornlea_engine --lib --locked fluid_rescan`: passed (18 tests passed).
  - `go test ./packages/tools/cmd/runtime-oracle -run '^TestFluidRescan' -count=1`: passed.
  - `rustup run 1.97.1 cargo clippy --manifest-path packages/engine/Cargo.toml -p mornlea_engine --all-targets --locked -- -D warnings`, `cargo fmt --all --check`, `gofmt -l ./packages`: all clean.
  - `go test ./packages/tools/cmd/runtime-oracle -count=1`: passed after the controller's source-hash refresh (`src/fluid_rescan.rs` `d9fab8dc…` -> `11732749…`).
- Review: Task Reviewer subagent approved with no Critical or Important findings; the named duplication risk is absent because the accounting loop lives once in `rescan_scan`. Two minor findings deferred to the final whole-branch review (an overclaiming test name for the typed-range rejection, and `meta_uniform` admitting the center `(0,0)` pair although the seal loop never uses it).
- Rollback: Revert `ff2c119c`, `1866f315` in that order.
