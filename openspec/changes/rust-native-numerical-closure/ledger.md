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

## 2026-09-26 — Node 2.10 dispatch preflight

- Frontier: 2.1–2.9 accepted; 2.10 is the next unchecked node. Worktree clean at BASE `60b3bcb6de9fa2109d4ee4d37f80bc1af44c3b1d`; four-point orphan check consistent (checkboxes, ledger rows, tracked files; `tests/native_contract/mesh_view.rs` stub empty and registered).
- Preflight scan (packet `plans/01-providers.md#node-2-10`, brief `worker-briefs.md` 5.11 parser/light half, `tasks.md` node 2.10):
  - 2.10 ↔ 2.11 share `src/native/mesh.rs`: 2.10 lands constructors/view/scratch without geometry; 2.11 adds the geometry method to the accepted surface. Serial order already frozen; no conflict.
  - 2.10 ↔ 3.1/3.11 share `ffi.rs`: read-only here; ABI byte behavior must stay identical, and the all-air structural-only exemption stays in place for 3.11's ABI test. No conflict.
  - Derived-artifact inventory: the mesh family pins `packages/engine/include/mornlea_engine.h`, `packages/shared/nativeabi/native_test.go`, `src/greedy/mod.rs`; none of 2.10's editable files is pinned, so no manifest refresh belongs to this node. No conflict.
- Ruling: 2.10 may edit `src/native/mod.rs` solely to widen `pub(crate) mod mesh` to `pub mod mesh` so the external test can name the native surface — same accepted pattern as nodes 2.4/2.5/2.6. Cost if wrong: a one-line visibility diff also required by 2.11.
- Ruling: the typed light entry in `src/native/mesh.rs` is worker-local naming inside the frozen contract and must not add `MeshOp::mesh`; internal level/queue reset assertions live in `src/light.rs` unit tests because `MeshScratch` fields are crate-private contract state, while `tests/native_contract/mesh_view.rs` asserts observable public behavior.
- Dispatch: implementer subagent for node 2.10 from BASE `60b3bcb6`; report file `task-2.10-report.md` in the session scratch directory.
- Implementer report: DONE_WITH_CONCERNS; commit `cfd0bf1c` (6 files, single commit, editable set respected; `src/native/mod.rs` diff is the ruled one-line visibility widening). TDD evidence: two compile reds plus four genuinely observed semantic reds, then green gates `mesh_view` 5/5, `--lib light` 31/31, clippy/fmt/gofmt clean, `TestKernelMeshView` pass, full `--lib` 270 / `native_contract` 69 / `numerical_migration` 31. Raw ABI export produced 3 cases under the session scratch `mesh-view-export/` for later closure review.
- Ruling on the count-1 concern: the byte lane at BASE rejects `air_id == barrier_id` and requires both sentinels present (tracked `has_air`/`has_barrier`), so the typed pass mirroring distinct-plus-present is raw-lane semantic parity, not an over-constraint; the packet's count matrix is covered as 96 accept / 97 reject / 1 reject-by-sentinel. Cost if wrong: one test expectation change once the reviewer or final review disagrees.
- Ruling on `model 7`: the typed `MeshModel` enum is closed (`0..=6`, frozen contract), so tag-7 rejection is only expressible in the raw byte lane and Go producer; no contract edit is allowed in this node. Cost if wrong: none beyond the typed lane being strictly stronger than raw for that field.
- Task 2.10 review: Spec compliant; one Important finding — the raw producer's rejected cases checked status only, so a regression writing output on registry rejection would have passed. Fix round 1/5 committed `49810017`, limited to `kernel_mesh_view_test.go` (shared `assertUntouched` canary helper on every rejected call, plus de-backticking the `MGM1` magic to keep the repo audit gate green). Fix report names `TestKernelMeshView`, `gofmt`, `go vet`, and audit-gate runs with actual output; scoped re-review over `cfd0bf1c..49810017` is in flight.
- Artifact reconciliation (review minor, controller-owned): `worker-briefs.md` and `plans/01-providers.md` no longer imply a one-entry registry is accepted; they now state the byte-lane sentinel-presence rejection outcome.

## 2026-09-26 — Node 2.10 mesh registry, view and light

- Predecessor SHA: `60b3bcb6`
- Result SHA: `49810017` (provider `cfd0bf1c`, review fix `49810017`)
- Implementation summary:
  - `src/input.rs` centralizes registry shape and per-entry semantic validation (`check_registry_shape`, `check_registry_entry`) for both lanes; `validate_typed_registry` closes the frozen constructor's gaps (air/barrier distinct and both present); `TypedRegistryView`, `typed_cell_index`, `typed_height_slot` and `MeshViewAccess` share the byte lane's 27-section layout and fallback semantics.
  - `src/light.rs` introduces `LightAccess` with a raw and a typed implementation over one `build_light_core`; `build_light` keeps its exact FFI signature and behavior; `build_light_view(view, &mut MeshScratch)` consumes the typed view with full level/queue reset and `MeshError` → `KernelError` mapping.
  - `src/native/mesh.rs` exposes `try_new_registry` (frozen `try_new` plus typed validation) and `build_light` (always validates registry semantics, including all-air; no geometry method); `src/native/mod.rs` widens `mesh` to `pub` (ruling R1).
  - Tests: `tests/native_contract/mesh_view.rs` (5 tests) and six new `src/light.rs` unit tests; Go producer `kernel_mesh_view_test.go` (`TestKernelMeshView`) pins the raw all-air structural-only exception, registry/light cases, and exported three raw ABI cases to the session scratch `mesh-view-export/`.
- Verification:
  - `native_contract --test ... mesh_view`: 5 passed; `--lib light`: 31 passed; full `--lib` 270, `native_contract` 69, `numerical_migration` 31 passed.
  - clippy `-D warnings` clean; `cargo fmt --all --check` clean; `gofmt -l ./packages` empty.
  - `go test ./packages/tools/cmd/runtime-oracle -run '^TestKernelMeshView' -count=1` passed; `go test ./packages/audit -run 'TestCommentBacktickIdentifiersExist'` passed after de-backticking the `MGM1` wire magic in the new producer comment.
- Review: Task Reviewer subagent: Spec compliant; one Important finding (rejected raw cases lacked destination-untouched canary assertions) fixed in `49810017`; the scoped re-review marked it ADDRESSED with no new breakage. Deferred minor for the final whole-branch review: the typed rejection test does not pin that validation leaves the scratch untouched. Requirement artifacts were reconciled to the landed count-1 sentinel semantics.
- Rollback: Revert `49810017`, `cfd0bf1c` in that order.

## 2026-09-26 — Node 2.11 dispatch preflight

- Frontier: 2.10 accepted and recorded by `4eace34c`; 2.11 is next. Worktree clean at BASE `4eace34cb48658cc8e8c3a7638b411778acce820`; `tests/native_contract/mesh.rs` and `tests/numerical_migration/mesh.rs` are empty registered stubs; `kernel_mesh_test.go` absent.
- Preflight scan (packet `plans/01-providers.md#node-2-11`, dispatch packet `worker-briefs.md` node 2.11, brief 5.11 geometry half, `tasks.md` node 2.11):
  - 2.11 ↔ 2.10: consumes the accepted `native::mesh` surface (`try_new_registry`, `build_light`) and adds only the geometry entry; 2.10 is closed. No conflict.
  - 2.11 ↔ 3.1/3.11: `ffi.rs` read-only here; the typed geometry and stage behavior is what 3.11 adapts (24,576-slot preflight and alias/metadata cases remain 3.1/3.11). No conflict.
  - Derived-artifact inventory: the mesh family pins `src/greedy/mod.rs`; 2.11 edits it, so the controller lands the matching hash refresh and the package-wide runtime-oracle run after the provider commit, following the accepted node 2.6–2.9 pattern. `src/quad.rs` and the greedy emission/test files are not pinned.
- Ruling: the 2.11 commit subject is `feat(engine): stage safe native mesh geometry` (the node's dispatch packet wording); the file packet's "stage safe native mesh and light output" variant is a wording slip because this node adds no light behavior. Cost if wrong: commit-subject text only.
- Dispatch: implementer subagent for node 2.11 from BASE `4eace34c`; report `task-2.11-report.md` in the session scratch directory.

## 2026-09-26 — Node 2.11 mesh geometry and typed publication

- Predecessor SHA: `4eace34c`
- Result SHA: `6b3b7c74` (provider `373478c6`, review fix `6b3b7c74`, controller hash refresh `74620e9b`)
- Implementation summary:
  - `src/quad.rs` adds the checked quad construction/validation used before `packed()`.
  - `src/greedy/mod.rs`, `src/greedy/torch.rs` and `src/greedy/bed.rs` route emission through one geometry core that keeps the existing face/slice/row/plant/model order and merge rules while staging into the bounded scratch; the untouched `plant_tests.rs`/`torch_tests.rs` remain the retention evidence.
  - `src/native/mesh.rs` adds the typed geometry entry implementing the frozen `MeshOp` for the mesh provider: validate, stage into `MeshScratch.stage` (40960), exact needed count, single copy; late packing violations return `OutputInvariant` with the entire destination untouched; short destinations return exact `OutputTooSmall { needed, available }`.
  - Tests: `tests/native_contract/mesh.rs` (12 tests), `tests/numerical_migration/mesh.rs` (3 tests, packed-bit/ordered-record parity against the raw ABI), Go producer `kernel_mesh_test.go` (`TestKernelMesh`, 4 exported cases under `task-2.11-exports/`).
- Verification:
  - `native_contract --test ... mesh` 12 passed; `numerical_migration --test ... mesh` 3 passed; `--lib greedy` 35 passed; full crate suite 380 passed.
  - clippy `-D warnings`, `cargo fmt --all --check`, `gofmt -l ./packages` all clean.
  - `go test ./packages/tools/cmd/runtime-oracle -run '^TestKernelMesh' -count=1` passed; after the controller hash refresh (`src/greedy/mod.rs` `b2409165…` -> `5cfd1aa5…`), the package-wide `go test ./packages/tools/cmd/runtime-oracle -count=1` passed in 70.8s; `go test ./packages/audit -run '^TestCommentBacktickIdentifiersExist$'` passed after the fix round.
- Review: Task Reviewer subagent: every node-2.11 functional item verified (packed-bit and ordered-record parity, not type presence); one Important finding (backticked `MGM1` in the new Go producer turned the repo-wide audit gate red) fixed in `6b3b7c74`, which also rewrote the three de-staled Chinese comment spots in English; the scoped re-review marked both items ADDRESSED with no new breakage.
- Rollback: Revert `74620e9b`, `6b3b7c74`, `373478c6` in that order.

## 2026-09-26 — Node 2.12 dispatch preflight

- Frontier: 2.1–2.11 accepted; 2.12 is next. Worktree clean at BASE `6a39ada5`; `src/pathfind.rs` absent, `src/native/pathfind.rs` and `tests/numerical_migration/path_grid.rs` empty registered stubs; oracle `pathfind_test.go` absent.
- Preflight scan (packet `plans/01-providers.md#node-2-12`, dispatch packet `worker-briefs.md` node 2.12, brief 5.12, `tasks.md` node 2.12):
  - 2.12 ↔ 2.13 share `src/pathfind.rs` and `src/native/pathfind.rs`: 2.12 lands grid/snapshot/indexing with no search; 2.13 adds search to the accepted surface. Serial order already frozen; no conflict.
  - 2.12 ↔ 3.12 share nothing yet: corpus registration is controller-owned at 3.12; this node only produces temporary Go grid observations. No conflict.
  - Go oracle boundary: `packages/shared/pathfind` stays read-only; unexported block/standing/expansion reads are observable only through exported `NewPathBlockTable`/`NewPathGrid`/`FindPath` (revision normalization via a start==goal result). No conflict.
  - Derived-artifact inventory: `kernel.pathfind` pins only the two Go files; neither is edited here and new Rust files are unpinned, so the package-wide oracle run must be fully green with no expected mismatch. No conflict.
- Ruling: 2.12 may add the single `mod pathfind;` registration line in `src/lib.rs` and may widen the `pathfind` line in `src/native/mod.rs` to `pub mod pathfind;` iff the migration test must name the native grid surface. Cost if wrong: two one-line diffs also required by 2.13.
- Ruling: frozen-contract gaps (e.g., unchecked size-product arithmetic in `PathGrid::try_new`) are worker-reported concerns, never worker edits; the contract changes only by controller ruling with artifact reconciliation. Cost if wrong: an extreme-size debug panic survives to a later node.
- Dispatch: implementer subagent for node 2.12 from BASE `6a39ada5`; brief `.superpowers/sdd/tasks-rust-native-numerical-closure/task-2.12-brief.md`, report `task-2.12-report.md` in the same workspace.

## 2026-09-26 — Node 2.12 immutable path grid

- Predecessor SHA: `6a39ada5`
- Result SHA: `180e331a` (single commit; no controller hash refresh — `kernel.pathfind` pins only the two untouched Go files, and the package-wide oracle run stayed green)
- Implementation summary:
  - New `src/pathfind.rs` exposes free read functions over the frozen contract grid: `flat_index` (Y-fast `((x * size_z) + z) * size_y + y`, bounds-first with checked arithmetic), `block_at` (widened `i64` subtraction, `.get()` lookup, never panics), `is_passable` (in-grid AND table), `is_standing` (feet/head passable, support in-grid nonpassable, `checked_add`/`checked_sub` at Y extremes).
  - `src/native/pathfind.rs` re-exports the four reads for external consumers; no search, no `PathfindOp`, no ABI bytes.
  - Registration diffs only: one `mod pathfind;` line in `src/lib.rs`, `pub mod pathfind;` widening in `src/native/mod.rs`.
  - Tests: `tests/numerical_migration/path_grid.rs` (`path_grid_shape_revision_and_snapshot`: unit/zero/over-cap/exact-cap/wrong-length/overflow, nine/ten/dedup/conflict revisions, Y-fast markers with X-fast-disagree spotlights, unknown 65535 blocked-yet-support-solid, move-clone ownership, determinism); Go producer `pathfind_test.go` (`TestPathfindOracleGrid`: revision passthrough, failure matrix with zero-fetch-reads, cap, verbatim overflow log, Y-fast waypoints, unknown column unreachable both ways, snapshot mutation replay, create-exclusive export).
- Verification:
  - `native_contract` untouched; `numerical_migration --test ... path_grid` 1 passed with `-- --list` discovery 1/1.
  - `go test ./packages/shared/pathfind -count=1` passed; `go test ./packages/tools/cmd/runtime-oracle -run '^TestPathfindOracleGrid$' -count=1` passed.
  - clippy `-D warnings` clean; `cargo fmt --all --check` clean after fix round; `gofmt -l ./packages` empty; `go vet` on runtime-oracle clean; audit `TestCommentBacktickIdentifiersExist` passed.
  - `go test ./packages/tools/cmd/runtime-oracle -count=1` fully green — no manifest drift, as ruled.
- Review: Task Reviewer subagent: spec ✅ with one Important (plan-mandated `cargo fmt` RED, 8 line-width reformats, no logic change); controller applied the mechanical fmt fix; scoped re-review marked it ADDRESSED with no new breakage. Divergence adjudicated: Go accepts max/min-int32 origins with size 2 (logged verbatim) while Rust rejects far-corner-past-maximum as `InvalidGrid` per the frozen contract — the Rust rejection stands. No implementer report was written (subagent terminated after one usage line); the diff plus controller-collected gates are the evidence. Three minors deferred to the final whole-branch review (Go exact-131073 case, `sha=unpinned` stamp, three-layer wording).
- Rollback: Revert `180e331a`.

## 2026-09-26 — Node 2.13 deterministic path search

- Predecessor SHA: `03e65f88`
- Result SHA: `ba918521` (provider `0b3bbd53`, review fix `ba918521`; no controller hash refresh — `kernel.pathfind` pins only the two untouched Go files, and the package-wide oracle run stayed green)
- Implementation summary:
  - `src/pathfind.rs` adds the bounded A* over the frozen grid: directions `[-X, +X, -Z, +Z]`, four independent transition checks per direction in flat (1) / jump-up (2) / fall-one (1) / gap-two (2) order with checked `g` accumulation, widened horizontal Manhattan heuristic ignoring Y with `f` as `u64`, `(f, insertion-ordinal)` binary heap with decrease-key retaining the ordinal, strict `candidate < g` update only, no reopening of closed cells, standing-endpoint validation before start-equals-goal, scratch-capacity check between them, budget check before the 4097th pop, parent-chain reconstruction into an owned boxed path only on success with chain length bounded by cell count. The 2.12 grid readers are behavior-identical apart from the import line.
  - `src/native/pathfind.rs` adds the public `find_path` entry plus the zero-sized `NativePathfind` provider implementing the frozen `PathfindOp`; grid-read re-exports kept; no ABI bytes.
  - Tests: `tests/numerical_migration/path_search.rs` (`path_search_transition_matrix` plus focused helpers: corridor, jump, jump-head-blocked, fall, two-cell fall, gap, gap-head-blocked, two-cell gap, flat-plus-gap both eligible, diamond plus mirror, decrease-key detour, terrace equal-cost, start-equals-goal with revisions, non-standing endpoints, 4096/4097 corridor pair, success→failure→success reuse, provider parity); ordered comparison reports the first differing waypoint index. Go producer `pathfind_test.go` extended in place with `TestPathfindOracleSearch` (same matrix via `NewPathGridFromLayers`, exact budget pair against `MaxPathNodes`, repeat-identical determinism, `errors.Is` category mapping); `TestPathfindOracleGrid` behavior untouched.
- Verification:
  - `numerical_migration --test ... path_search` 1 passed with `-- --list` discovery 1/1; `numerical_migration --test ... path_grid` green (2.12 regression).
  - `go test ./packages/shared/pathfind -count=1` passed; `go test ./packages/tools/cmd/runtime-oracle -run '^TestPathfindOracleSearch$' -count=1` passed.
  - clippy `-D warnings` clean; `cargo fmt --all --check` clean; `gofmt -l ./packages` empty; `git diff --check` clean.
  - `go test ./packages/tools/cmd/runtime-oracle -count=1` fully green (~65s) — no manifest drift, as ruled.
  - Sensitivity: diamond expectation flipped to the Z-first route fails exactly at the first differing waypoint; every Rust fixture waypoint sequence was cross-checked against Go `FindPath` on identical terrain before pinning (decrease-key detour: 7 waypoints via `(1,65,0)` and `(1,64,1)`, identical both sides).
- Review: Task Reviewer subagent: spec compliant, no read-only or contract-file touch, no scope-creeping extras; two Important findings (stale `scratch_cells` paragraph describing a nonexistent debug-rendering mechanism; `SearchSpace` doc claiming cross-call reuse while `find_path` allocates fresh per call) fixed docs-only in `ba918521` with no logic change; scoped re-review confirmed comments-only (8 insertions, 11 deletions) and marked both ADDRESSED with no new breakage. Four minors deferred to the final whole-branch review: generation/reset/touch machinery is dead weight given fresh per-call allocation; the retained-result reuse check clones waypoints instead of re-reading the retained result; `ScratchTooSmall` has no migration-test fixture; flat/gap use unchecked horizontal arithmetic while jump/fall use checked vertical arithmetic.
- Frozen-gap observations (report only, no contract edits): `PathResult::revisions()` carries only `[[i32; 2]]` chunk coordinates so revision numbers do not round-trip; `PathScratch` carries only `cells: usize` so working state lives in per-call function-local buffers sized by the grid cell count with trivially holding generation-reset semantics and no warm heap reuse; `PathGrid::try_new` size multiply stays unchecked per the 2.12 ruling with the search using checked arithmetic for its own counts. Search fixtures avoid the ruled Go/Rust int32-origin divergence edge.
- Rollback: Revert `ba918521`, `0b3bbd53` in that order.

## 2026-09-26 — Node 3.1 dispatch preflight

- Frontier: 2.1–2.13 accepted; 3.1 is the first serial `ffi.rs` node. Worktree clean at BASE `57e8cc71`; `tests/native_contract/publication.rs` is a one-line registered stub; no `ffi.rs` work in flight.
- Preflight scan (packet `plans/02-adapters.md#node-3-1`, dispatch packet `worker-briefs.md` node 3.1, brief 5.1 publication correction, `tasks.md` node 3.1):
  - 3.1 ↔ 3.2–3.11 share `src/ffi.rs`: 3.1 lands first and owns only the metadata-clear order in the four exports plus its unit tests; adapters consume the accepted order. Serial order already frozen; no conflict.
  - 3.1 ↔ 2.x providers: providers read-only here; no numerical core moves. No conflict.
  - Derived-artifact inventory: no kernel family pins `src/ffi.rs` (only the header, `native_test.go`, and per-family algorithm files are pinned), so NO manifest refresh belongs to this node; the package-wide oracle run must stay fully green with no expected mismatch.
- Ruling: the ABI-version handshake moves after the null-pointer checks but before the clear (previously the clear came first). A wrong version means the call never belonged to this engine, so no caller word may be touched; pre-existing tests asserting `output_len == 0` on pre-clear rejections are corrected to `usize::MAX` unchanged. Cost if wrong: the Go bridge reads the count only on success/overflow, so no caller-visible change; the final whole-branch review re-checks.
- Dispatch: three implementer subagent attempts returned partial/uncommitted work (two empty, one orphan implementation with tests but no fix); the controller adopted the orphan diff as the starting point per the handover rule and finished the node directly. Report `task-3.1-report.md` in the session workspace.

## 2026-09-26 — Node 3.1 metadata alias preflight and atomic publication

- Predecessor SHA: `57e8cc71`
- Result SHA: `58710c66` (single commit; no manifest refresh — `src/ffi.rs` unpinned, package-wide oracle green)
- Implementation summary:
  - `src/ffi.rs`: each `output_len.write(0)` in `mornlea_mesh_section`, `lod_shell_with`, `fluid_eval_batch_with`, `fluid_rescan_with` moved to after the complete pointer/range/overlap preflight (order: pointer self-check, null checks, version handshake, clear, all range+overlap checks, core). No status, seam, unwind, or core change; LOD/rescan exact-needed overflow writes kept.
  - New `ffi.rs` unit groups `metadata_alias_mesh` (input/output/scratch alias + non-aliased clear), `metadata_alias_lod`/`_fluid_eval`/`_fluid_rescan` (input/output alias + non-aliased clear + exact-needed probes): full-word canary arenas asserted equal to pre-call clones; non-aliased metadata clears to zero.
  - `tests/native_contract/publication.rs`: four typed `OutputTooSmall` atomicity tests (mesh/LOD pre-filled with the known-good probe prefix; eval 2-write/1-slot with surviving-slot check; rescan short-destination check).
  - Corrected 17 pre-existing stale assertions encoding the old early-clear behavior (`output_len == 0` → `usize::MAX` on pre-clear rejections; aliased words → full-arena before-image equality); post-clear rejections keep `== 0`.
- Verification:
  - `native_contract --test ... publication` 5 passed; `--lib` 274 passed; full `native_contract` 80 passed; `numerical_migration` 36 passed.
  - clippy `-D warnings` clean; `cargo fmt --all --check` clean; `gofmt -l ./packages` empty; `git diff --check` clean.
  - `make rust` ok; `go test ./packages/shared/nativeabi -run 'Test.*(FailureAtomicity|InvalidArguments|InvalidBuffers)' -count=1` passed; `go test ./packages/shared/nativeabi -count=1` passed; `go test ./packages/tools/cmd/runtime-oracle -count=1` fully green (~60s, no drift).
  - TDD RED: alias tests failed with the first metadata byte clobbered (`left: [0, 165, ...]` vs `right: [165, ...]`) before the fix; GREEN after.
- Review: Task Reviewer subagent: spec compliant; no Critical/Important findings; four Minor canary-strength notes. Controller-applied polish (full-word fills, probe-prefix short destinations, 2-write eval case) verified by scoped re-review: all addressed, no new breakage. Deferred minor for the final whole-branch review: none from this node.
- Rollback: Revert `58710c66`.

## 2026-09-26 — Node 3.2 dispatch preflight

- Frontier: 3.1 accepted and recorded by `fee8c06d`; 3.2 is next. Worktree clean at BASE `fee8c06d`; `tests/numerical_migration/collision.rs` holds only the 2.1 `parity_test`; oracle `kernel_collision_test.go` accepted at 2.1.
- Preflight scan (packet `plans/02-adapters.md#node-3-2`, dispatch packet `worker-briefs.md` node 3.2, brief 5.2/adapter table, `tasks.md` node 3.2):
  - 3.2 ↔ 3.3 share nothing yet (physics routes after 3.2 through its own hunk; both consume the accepted collision core). Serial `ffi.rs` order already frozen; no conflict.
  - 3.2 ↔ 3.1: consumes the accepted metadata-clear order; this hunk has no metadata pointer. No conflict.
  - Derived-artifact inventory: the collision family pins the header, `native_test.go`, `src/collision.rs`; none is edited here and new/edited files are unpinned, so the package-wide oracle run must stay fully green with no expected mismatch.
- Ruling: the adapter routes to the shared `resolve_collision_parts` core, NOT the stricter typed `native::collision::resolve_collision` wrapper, preserving the legacy swept-prism admission exactly (same reason node 3.3 will cite). The `resolver` injection seam stays live so existing status-9 tests pass unmodified. Cost if wrong: duplicated numerical work per call until a later node re-owns the seam; recorded as plan-mandated debt.
- Dispatch: implementer subagent for node 3.2 from BASE `fee8c06d`; brief `task-3.2-brief.md` in the session workspace, report `task-3.2-report.md` in the same workspace.

## 2026-09-26 — Node 3.2 collision ABI adapter

- Predecessor SHA: `fee8c06d`
- Result SHA: `2832658c` (provider `a2ae1d70`, review fix `2832658c`; no manifest refresh — collision family pins only the untouched header/test/core files, and the package-wide oracle run stayed green)
- Implementation summary:
  - `src/ffi.rs` `collision_resolve_with` hunk: every admission line byte-identical (version, nulls, 16-byte preflights, ranges, input/output overlap, `collision_input_is_valid` + swept-prism coverage, `catch_collision`, single-copy publish). Only the `Ok(resolver(bytes))` call now decodes validated header fields and invokes shared `resolve_collision_parts`, packing the local `[u8; 16]` through the existing publish path. The `resolver` seam parameter stays invoked inside the panic boundary so injected-panic tests still prove status 9 with output untouched.
  - `tests/numerical_migration/collision.rs`: `collision_abi_native_bits` calls the real exported symbol for floor/wall/unknown with full 16-byte vector parity (added in the fix round), 15-byte status-7 canary, and a corrupt-magic status-3 canary.
- Verification:
  - Baseline GREEN before the route edit (same core): migration collision 2/2 with identical bytes; post-edit GREEN with the same vectors.
  - `make rust`, `go test ./packages/shared/nativeabi -run 'TestCollision'`, `go test ./packages/tools/cmd/runtime-oracle -run '^TestKernelCollision'`, `--lib collision` 8/8 (seam tests unmodified), clippy `-D warnings`, `cargo fmt --check`, full runtime-oracle package green (~61s, no drift).
- Review: Task Reviewer subagent: routing/admission/core-choice/seam correct; one Important finding (migration test asserted a semantic subset, not the brief's full 16-byte equality) fixed in `2832658c` by pinning all three vectors; scoped re-review confirmed ADDRESSED with no new breakage. One Minor deferred to the final whole-branch review: every ABI call runs both the legacy resolver path (discarded) and the parts core — deliberate plan-mandated seam-preserving debt until a later node re-owns the seam tests.
- Rollback: Revert `2832658c`, `a2ae1d70` in that order.

## 2026-09-26 — Node 3.3 dispatch preflight

- Frontier: 3.2 accepted and recorded by `90133e10`; 3.3 is next. Worktree clean at BASE `90133e10`; `tests/numerical_migration/physics.rs` holds only the 2.2 typed parity test; oracle `kernel_physics_test.go` accepted at 2.2.
- Preflight scan (packet `plans/02-adapters.md#node-3-3`, dispatch packet `worker-briefs.md` node 3.3, brief 5.3/adapter table, `tasks.md` node 3.3):
  - 3.3 ↔ 3.2: disjoint hunks (`physics_step_with` vs `collision_resolve_with`); 3.2 landed and closed. Serial `ffi.rs` order frozen; no conflict.
  - 3.3 ↔ 2.2: consumes the accepted physics provider; `src/step.rs` read-only here. No conflict.
  - Derived-artifact inventory: the physics family pins the header, `native_test.go`, `src/step.rs`; none is edited here and new/edited files are unpinned, so the package-wide oracle run must stay fully green with no expected mismatch.
- Ruling: route to shared `physics_step` (which runs `integrate` plus the collision parts core), never the stricter typed wrapper or standalone swept-grid validation, preserving the legacy physics prism admission. The hunk already calls the shared core, so the node pins the route with a comment rather than inventing churn. Cost if wrong: none — admission and numerics provably unchanged.
- Dispatch: implementer subagent for node 3.3 from BASE `90133e10`; brief `task-3.3-brief.md` in the session workspace, report `task-3.3-report.md` in the same workspace.

## 2026-09-26 — Node 3.3 physics ABI adapter

- Predecessor SHA: `90133e10`
- Result SHA: `4910504b` (provider `cb46e319`, review polish `4910504b`; no manifest refresh — physics family pins only the untouched header/test/core files, and the package-wide oracle run stayed green)
- Implementation summary:
  - `src/ffi.rs` `physics_step_with` hunk: admission byte-identical; only a 3-line English comment naming the shared route (`physics_step` → `integrate` + collision parts core). The hunk already invoked the shared core on the validated slice, so no call change belonged here.
  - `tests/numerical_migration/physics.rs`: `physics_abi_native_bits` calls the real exported symbol for landing/jump/fluid/sneak+sprint with FULL 32-byte vectors incl. reserved zeros, one-ULP accepted vs two-ULP status-3 rejection, malformed-axis status-3 canary, and 31-byte status-7 known-good pre-fill canary. Full-word canaries throughout.
- Verification:
  - Baseline GREEN before the route change (same core): migration physics 2/2 with identical vectors; post-change GREEN with the same vectors.
  - `make rust`, `go test ./packages/shared/nativeabi -run 'TestPhysicsStep'`, `go test ./packages/tools/cmd/runtime-oracle -run '^TestKernelPhysics'`, `--lib physics_step` 5/5 plus full `--lib` 274/274, clippy `-D warnings`, `cargo fmt --check`, full runtime-oracle package green (~59s, no drift).
- Review: Task Reviewer subagent: spec compliant; the comment-only hunk satisfies the brief's explicit no-churn clause; no Important findings; two Minor notes (duplicated landing literal, committed `println!` noise) both fixed in `4910504b` without re-review scope creep. No deferred minor from this node.
- Rollback: Revert `4910504b`, `cb46e319` in that order.

## 2026-09-26 — Node 3.4 dispatch preflight

- Frontier: 3.3 accepted and recorded by `84bea73d`; 3.4 is next. Worktree clean at BASE `84bea73d`; `tests/numerical_migration/raycast.rs` holds only the 2.3 typed continuation test; oracle `kernel_raycast_test.go` accepted at 2.3.
- Preflight scan (packet `plans/02-adapters.md#node-3-4`, dispatch packet `worker-briefs.md` node 3.4, brief 5.4/adapter table, `tasks.md` node 3.4):
  - 3.4 ↔ 3.2/3.3: disjoint hunks (`raycast_batch_with` vs collision/physics); both landed and closed. Serial `ffi.rs` order frozen; no conflict.
  - 3.4 ↔ 2.3: consumes the accepted raycast provider; `src/raycast.rs` read-only here. No conflict.
  - Derived-artifact inventory: the raycast family pins the header, `native_test.go`, `src/raycast.rs`; none is edited here and new/edited files are unpinned, so the package-wide oracle run must stay fully green with no expected mismatch.
- Ruling (seam load-bearing, found during preflight): unlike collision, the raycast `resolver` seam is load-bearing — `raycast_success_publishes_local_cursor_and_output_once` asserts CUSTOM resolver output, not just panics, so the 3.2 preserve-dual-execution pattern would leave dead custom coverage. The node REMOVES the `resolver` parameter (3 call sites) and re-owns both seam tests (panic proof via `catch_raycast` + real-core publish proof). Cost if wrong: the old custom-output test is replaced by real-core assertions; the migration test carries the exact publish proof.
- Dispatch: implementer subagent for node 3.4 from BASE `84bea73d`; brief `task-3.4-brief.md` (revised for seam removal) in the session workspace, report `task-3.4-report.md` in the same workspace.

## 2026-09-26 — Node 3.4 raycast ABI adapter

- Predecessor SHA: `84bea73d`
- Result SHA: `cc317e8e` (provider `06f92f81`, comment fix `cc317e8e`; no manifest refresh — raycast family pins only the untouched header/test/core files, and the package-wide oracle run stayed green)
- Implementation summary:
  - `src/ffi.rs` `raycast_batch_with` hunk: admission byte-identical (metadata checks, weak-history `raycast_cursor_is_valid` admission, capacity preflights, overlap checks, `catch_raycast`, staged cursor+output+count+done publish). Only the call changed: `Ok(resolver(input_bytes, cursor_bytes))` → `Ok(raycast_batch(input_bytes, cursor_bytes))` on the validated slices; the `resolver` parameter removed from the helper and its 3 call sites. No public raw cursor parser; no typed opaque-cursor routing.
  - Seam tests re-owned by name: `raycast_panic_through_publish_path_is_atomic` proves the panic boundary directly via `catch_raycast` plus a real-core no-panic publish with guard bytes intact; `raycast_success_publishes_local_cursor_and_output_once` asserts the real core's staged publish (guards, count, advanced cursor). No custom-resolver coverage remains.
  - `tests/numerical_migration/raycast.rs`: `raycast_abi_cursor_sequence` calls the real exported symbol for the 65-record ray (64 then 7 with full ordered record bytes, full cursor bytes, count/done after each call), repeat-done preservation, tampered-cursor status 3 with full preservation, and input/output overlap status 2 with full preservation.
- Verification:
  - Baseline GREEN before the route edit (same core): migration raycast 2/2; post-edit GREEN with identical bytes.
  - `make rust`, `go test ./packages/shared/nativeabi -run 'TestRaycast'`, `go test ./packages/tools/cmd/runtime-oracle -run '^TestKernelRaycast'`, full `--lib` 274/274, full `native_contract` + `numerical_migration`, clippy `-D warnings`, `cargo fmt --check`, full runtime-oracle package green (~60s, no drift).
- Review: Task Reviewer subagent: spec compliant against the revised brief; no Important findings; three Minor notes (one Chinese fragment in an edited SAFETY line — fixed in `cc317e8e`; weaker-than-old through-path panic proof explicitly allowed by the brief's fallback clause; publish-test exactness relying on the migration test). No deferred minor from this node beyond the recorded fallback acceptance.
- Rollback: Revert `cc317e8e`, `06f92f81` in that order.
