# Shared world mutation review

Repository `/Users/chen/work/mornlea-f2-server`; branch `cursor/rust-authoritative-server-98e6`; requested source baseline `cff1d6ec`. HEAD advanced through concurrent disjoint repairs to `3e41bb15615eb88bedf83ae959efcdecf1f4f9ab`. Diff from `cff1d6ec` to that HEAD is empty for every production file reviewed below. Reviewer made no repository edits, staging or commits.

Every line reviewed in `packages/engine/crates/mornlea_server/src/core/{mutation,interaction,drop_store,container_store,world}.rs` (1219/51/269/298/166 lines), plus relevant `state.rs` observation, staging, validation, application and MutationTxn code and declarations/validators in `contracts.rs`. Root/engine/server-crate and Go source ancestor guidance, OpenSpec config, proposal, delta spec, design/tasks and prior world review were read. Existing Ready revision/snapshot, sneak/input/sleep/burn and executable-assembly findings are excluded.

## Executed evidence

Disposable crate `/tmp/mornlea-f2-mutation-review-probe` links actual checkout `mornlea_server`, domain, protocol and storage crates. It copies five original contract test modules and their original helpers, with nine appended review probes. It copies no production algorithm and uses no substitute mutation provider.

- `rustup run 1.97.1 cargo test --manifest-path /tmp/mornlea-f2-mutation-review-probe/Cargo.toml --offline -- --skip review_`: **69 original tests PASS**, final log `/tmp/mornlea-f2-mutation-existing-green.log`.
- `rustup run 1.97.1 cargo test --manifest-path /tmp/mornlea-f2-mutation-review-probe/Cargo.toml --offline review_`: **9 review probes FAIL after successful compilation**, log `/tmp/mornlea-f2-mutation-red.log`.
- Initial probe layout had integration-module `super` import compilation errors; corrected by retaining modules below one test root. Those compile errors are not behavior evidence and are overwritten in the final RED log.

Expected Go outcomes below are source-derived, not executed Go differential results. These direct actual-provider tests use fixture setup, not a live authority process/TCP/disk worker. They do not accept production integration. The three seam cases explicitly insert a mutation between resolution and commit or submit an adversarial compound; current ordinary placement resolves and commits immediately.

## Findings reached by the current placement provider

### 1. [P1] Crop, sapling and torch support checks are absent

`core/mutation.rs:909–910` sends every non-door/non-bed form directly to a single write. No other branch checks crop farmland, sapling dirt/grass or torch collision-bearing support. `rules/world_mutation.rs:14–16` says these branches belong to other providers, but its actual settlement at219–225 calls this resolver for every PlaceBlock. The other rule modules have no PlaceBlock dispatch. `core/step.rs:537,559` calls this same provider.

Current Go oracle `packages/server/sim/entity/placement.go:144–161` requires crop target air and farmland below,163–180 requires sapling target air and dirt/grass below,191–208 requires torch dry target/full-cell player exclusion and an observed collision-bearing support. Concrete source tests: entity `farming_test.go:478` TestPlantSeedsRejectsNonFarmlandGround; `sapling_placement_test.go:114` TestPlantSaplingRejectsUnsupportedGround; `torch_test.go:212` TestTorchPlacementRejectsWithoutConsumption.

Compiled RED probes `review_crops_require_farmland`, `review_sapling_requires_dirt_or_grass`, `review_torch_requires_solid_support`: actual active actor [0.5,64,0.5], source look PI/0, air target(0,65,1). Crop seed and sapling have stone support; torch ray hits sapling89 at(0,65,2), deriving wall torch75 against zero-collision support. Each resolves Ok with a debit instead of source `Wire(InvalidBlock)`. Later support sweeps can delete an illegal plant/torch, but cannot replace the source zero-effect refusal or undo the already consumed item. Controller must freeze ownership/geometry corrections; this report chooses no shared API.

### 2. [P1] Ordinary placement allows writing into the player's body

`mutation.rs:843–849` checks target block only, and the subsequent resolver has no player collision test. Go `placement.go:104–110` combines target occupancy with `placementOverlapsPlayer`; helper at313 onward tests PlayerBounds against the form-specific collision boxes. Zero-collision torches separately use full-cell overlap at195–196. Current runtime oracle `hotbar_test.go:157` TestHotbarFailedPlaceKeepsItem covers body exclusion without consumption.

RED `review_block_cannot_overlap_player`: active player [0.5,64,0.9], eye ray points +Z, stone hit(0,65,2), air target(0,65,1). The player's body reaches z1.2 and intersects that target. Rust resolves a dirt write/debit; expected source Occupied. This is a current mutation-path defect, not the previous movement-provider issue. Exact native collision geometry is the controller's repair responsibility; a whole-cell gate for every form would change source behavior for partial collision shapes.

### 3. [P2] Air-only target check refuses legal water replacement and changes special-item reject classes

`mutation.rs:843–849` calls `require_air` unconditionally, before knowing the form. Go `placement.go:104` allows ordinary non-fluid solid forms to replace any water source/flowing form; crops/saplings/torches then refuse water as InvalidBlock and door/bed refuse it as Occupied through their strict footprint checks.

RED `review_ordinary_block_replaces_water`: dirt item, water27 at(0,65,1), stone hit(0,65,2). Rust returns Wire(Occupied); source permits replacement and one debit. Existing Rust `tests/server_contract/mutation.rs:2130–2164` incorrectly freezes water27 and34 as Occupied alongside an open door, explicitly claiming water cannot host a block. That test must be reconciled with source, not retained as an oracle override. Source `packages/shared/core/raycast.go:202–205` also explicitly documents fluid placement as coverable.

Special item water reason differences are source inspection of this same unconditional gate; no separate special-item water probe was run.

### 4. [P2] Invalid held item loses source rejection precedence

Rust `mutation.rs:826–849` traces the ray and validates the destination before looking up/consuming the item at860–864/912–913. Current Go `placement.go:48–61` validates ItemPlacement/torch eligibility and rehearses consumption before raycasting. This changes source-reported failure even though neither branch mutates state.

RED `review_empty_item_precedes_no_target`: canonical empty held slot and an observed all-air ray within reach. Rust returns Wire(NoTarget); source returns InvalidBlock before ray traversal. Related occupied/unready cases are source-inferred, not separately probed. This is raw resolver policy evidence; the current outer provider collapses reject shapes, so an exact final wire outcome is not claimed by this probe.

## Shared seam contract conflicts requiring a controller ruling

### 5. [P2] A resolved transaction does not retain or revalidate actor/geometry read dependencies

`contracts.rs:1324–1331` BlockTxn stores writes/debit/container/output but no actor preimage, ray cells/hit or support basis. `state.rs:2336–2377` validates only write observations, inventory preimage and captured container/output arms. `MutationTxn::commit:2806–2824` ignores stored producer and tick. The resolver reads actor/current pose/reach and the ray hit but drops those observations when building the transaction (`mutation.rs:915 onward`).

RED `review_resolved_placement_checks_hit_basis`: resolve dirt placement, then use real system transaction to remove the supporting ray-hit stone at(0,65,2), leaving the air target/debit unchanged. try_place still succeeds. RED `review_resolved_placement_rechecks_actor_lifecycle`: resolve then stage same actor as Dead; try_place still succeeds. Each expected StaleObservation follows the planned complete permission/current-pose/reach → observed-bases → collision/support preflight (`plans/02-core-seams.md:62`) and stated authority revalidation, but the current placement call has no intervening stage between resolution/commit. These are **shared seam failures**, not proof of an exploited live production command. Controller must decide whether the seam retains all dependencies or enforces a narrower lifetime by API; no new policy is selected here. Cross-tick replay acceptance is a source concern because tick is ignored; no cross-tick probe was run.

### 6. [P2] Compound inventory CAS validates every patch against the initial state

`state.rs:2316–2319` checks InventoryPatch against `self.inventories`, without a pending inventory rehearsal; BlockTxn's inventory arm at2342–2345 does the same. `apply_effect:2570–2572` overwrites with each after record. In contrast drops, Ready containers and projectiles rehearse in compound order. A stale second inventory debit can therefore coexist with two output effects while only one debit survives. Chained inventory patches whose second preimage is the first after record conversely refuse.

RED `review_compound_inventory_rejects_repeated_preimage`: actual admitted actor inventory dirt2; submit Compound with two identical checked patches dirt2→dirt1. stage returns Ok and final inventory1, rather than refusing second stale preimage and preserving inventory2. The test establishes the actual seam behavior. No current real-provider compound containing two inventory patches for one actor was identified, and no production duplication exploit is claimed. This conflicts with before/after CAS and ordered settlement expected of the shared compound API; controller should explicitly rule ordering/duplicate policy before correction. No alternative owner or algorithm is introduced by this review.

## Source-only limits and reviewed invariants

- No new local drop/container atomicity defect was found. Reviewed fixed32 drop slots,36 input bound, exact-cell merges, stack splitting, generation exhaustion/rebirth, full-preimage patches, counter-only dirty behavior, exact integer block-producer anchors, dimension-keyed fixed furnace/chest storage, captured-content clear, reusable slot allocation, checked payloads and ordered compound drop/container rehearsal. Original tests exercise these actual providers including capacity/refusal, save encoding and negative/far coordinate round-trip.
- Interaction classification matches Go's air/fluid/open-door transparency and malformed/missing lower fallback. Look cast order and overflow-resistant normalization have source-bit KAT tests.
- Mining tables, harvest outputs, tool exemptions, wrong-tool clear, captured container products, companion credit-before-wear, human paired door/bed clear and companion door single-cell behavior were examined against current Go mining/bed/drop code. No new local policy mismatch was found there. Current Go malformed partner clearing is preserved; the reviewer does not invent a stricter matching-half policy.
- Mining partner additions are checked. Placement `adjacent` atmutation.rs473 and bed head arithmetic at887 use unchecked i32 additions. No certified reachable default-reach extremal placement panic was demonstrated, so this is **a source concern**, not an actionable confirmed coordinate failure. Do not copy the prior sleep coordinate failure claim into this module without an actual ray fixture.
- RuleTunables accepts every finite reach (`contracts.rs:1546–1551`); the ray loops until kernel completion (`mutation.rs:440–463`) with no whole-ray charge/cell ceiling. Kernel batches cap64 but do not cap the complete traversal. Loaded-view bounds/accepted supported tuning need controller verification before this can be called a proven hot-path work violation. No timing result is claimed.
- Compound validation caps part count and each transaction's write count separately rather than aggregate total writes (`state.rs:2284,2337`). Thus the textual maximum4096 components/blocks is ambiguous for compound transactions. No current-provider over-ceiling compound was found, and external test code cannot construct arbitrary private BlockTxn. Controller should rule aggregate accounting with existing consumers; this review reports a contract ambiguity rather than inventing an expected budget test.
- Ready base height/storage snapshot logic was read completely. Existing known revision/snapshot omission findings are excluded. No new codec error, container/drop partial refusal, or height mismatch was found in this scope.

No independent Go replay, full-runtime path, platform gate or performance measurement was executed here. The69 passing tests do not close the six new finding groups, and these results do not close production assembly or integrated parity acceptance.
