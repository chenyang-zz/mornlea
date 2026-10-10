# Inventory, container, crafting and tool provider review

Reviewed checkout: `/Users/chen/work/mornlea-f2-server`, HEAD observed during review `fcd9c26e622bfe0b45f7dd6e3337b7634c4723b4`. Concurrent controller/worker changes were present; none of the four reviewed production providers was edited by this review. No repository files or commits were created or changed. All executable review cases live outside the repository under `/tmp/mornlea-f2-provider-repros`.

## Verdict and evidence boundaries

Found six new actionable behavior mismatches. Seven temporary regression tests against the actual public Rust providers fail, including a transcript using the real production order of the container and workbench drains. Existing focused suites are green (47 tests), so their passing status does not cover these gaps. Expected Go behavior below comes from directly read current source, not a newly executed Go-vs-Rust differential harness. These tests accept neither the absent gameplay runtime nor full F2 provider/integration completeness.

Commands for existing suites, all exit 0:

```
rustup run 1.97.1 cargo test --manifest-path packages/engine/Cargo.toml -p mornlea_server --test server_replay --locked inventory:: -- --nocapture
# 4 passed
rustup run 1.97.1 cargo test --manifest-path packages/engine/Cargo.toml -p mornlea_server --test server_replay --locked containers:: -- --nocapture
# 19 passed
rustup run 1.97.1 cargo test --manifest-path packages/engine/Cargo.toml -p mornlea_server --test server_replay --locked crafting:: -- --nocapture
# 15 passed
rustup run 1.97.1 cargo test --manifest-path packages/engine/Cargo.toml -p mornlea_server --test server_replay --locked tools:: -- --nocapture
# 9 passed
```

Executable new failures:

```
CARGO_TARGET_DIR=/Users/chen/work/mornlea-f2-server/packages/engine/target rustup run 1.97.1 cargo test --manifest-path /tmp/mornlea-f2-provider-repros/Cargo.toml --offline --test review review_ -- --nocapture
# 7 failed; exit 101
```

Full diagnostics: `/tmp/mornlea-f2-provider-repros/results.log`. Source: `/tmp/mornlea-f2-provider-repros/tests/crafting.rs`, tests named `review_*`. This temporary crate copies the existing replay helpers and calls the real path dependency `mornlea_server`; it does not replace providers with doubles.

## Verified findings

### P1: crafting half/single and quick moves have no provider implementation

Location: `packages/engine/crates/mornlea_server/src/rules/crafting.rs:1025-1098`, especially fallback at 1098. `inventory.rs:396-408` owns only Inventory-view partial/quick commands; container intake owns only Container-view variants. Crafting intake recognizes only MoveCrafting, TakeCraftingOutput, and OpenContainer, so valid Crafting-view partial and quick commands all return `InvalidInput { field: "command" }`. This is a missing semantic family in the provider, not a failure to project an already-computed result.

Minimum transcript: Active actor, Personal grid empty, inventory slot 0 holds oak planks `(item21,count5,durability0)`. Submit `MovePartial(Crafting, from9, to0, single=false)`: Go moves3 to grid0, keeps2 in inventory0; Rust refuses unchanged. Independently submit `QuickMove(Crafting, slot9)` on the same preimage: Go moves5 into first fitting grid cell0; Rust refuses unchanged. Both commands are valid accepted domain values. Temporary `review_crafting_partial_and_quick_must_settle` executes both and records both refusals.

Go evidence: `packages/server/sim/entity/tick.go:305-405` (Crafting partial arm358-394), quick Crafting arm445 onward, and `quick_move.go:122-172`. Required acceptance: both directions for partial half/single and quick Crafting moves, personal extended-cell bounds, mixed/non-fitting targets, inventory-internal endpoint refusal where applicable, durable singleton migration, full pack/repack refusal and exact conservation; drive the real intake/reducer as well as provider tests.

### P1: hotbar selection executes earlier than the source, changing same-tick actions

Location: `packages/engine/crates/mornlea_server/src/rules/inventory.rs:386-388`. It writes selected immediately during PlayerCommand. Go `tick.go:139-156` appends SelectHotbar to the deferred interaction list; `tick.go:774-779` writes selection when that list executes. EquipArmor remains inline at command handling in Go.

Minimum transcript: selected0, slot0 stone pickaxe `(10,1,131)`, slot1 iron helmet `(58,1,165)`, empty armor. Intake `[SelectHotbar(1) seq1, EquipArmor seq2]`. Go EquipArmor still sees slot0 pickaxe and refuses NotArmor, then interaction selection becomes1. Rust selects1 immediately and equips the helmet. Temporary `review_hotbar_selection_must_wait_for_interaction_order` observes helmet in armor0 and an applied equip result instead of refusal. Another source consequence: `[TillSoil seq1, SelectHotbar(other hand) seq2]` changes the hand before the older deferred till in Rust, unlike Go.

Required acceptance: a real ordered tick containing both commands, preserving phase-sensitive selected-slot reads by equip, eating/bow, and interactions; verify both sequence directions with exact inventory/armor/result events. The controller must reconcile sequencing before dispatching an implementation; this report does not choose a new policy.

### P1: independently drained opens reverse latest-command view ownership

Locations: `containers.rs:150` defers every open to ContainerMove; `crafting.rs:1093-1095` also defers it to WorkbenchLifecycle; `crafting.rs:1301-1305` clears the current container lease whenever a bench open settles. Actual production drains occur at `src/core/step.rs:438` then442. This specific chronology defect was not in the earlier assembly consolidation.

Minimum transcript: same Active actor at `[0.5,64,0.5]`, Ready workbench at `(0,63,0)` and Ready chest slot0/gen1 at `(0,65,-1)`. Admit `[OpenContainer(look down) seq1, OpenContainer(look toward chest) seq2]` into both accepted providers as production does. Drain ContainerMove, then WorkbenchLifecycle as production does. Rust first opens the newer chest, then the older bench clears that lease; final viewer=None. Go processes the two opens in deferred interaction order, so final viewer=chest and bench size remains Workbench. Temporary `review_latest_chest_open_must_win_across_provider_drains` fails with None vs expected chest lease.

The existing `tests/server_replay/crafting.rs:2293-2423` preservation test settles the bench first and drains crafting before containers for the second open. That cannot catch the actual shared-queue ordering problem. A related minimum command list `[OpenBench seq1, CloseContainer seq2]` also lets the older deferred bench run after the newer close under the current grouping (source reasoning; not separately executed).

Required acceptance: real ticks for bench→chest, chest→bench, bench→close, and interleaved moves/opens across two sessions; exact final grid size/anchor/view, single close events, and generation-safe transfers. Needs controller-owned cross-provider chronology integration, not an unreviewed scheduler reorder.

### P2: sneaking opens a workbench despite the source refusal

Location: `crafting.rs:1270-1282`, bench-hit branch proceeds directly to widened grid/anchor; no controls.sneaking gate exists anywhere in settle_bench_open. In contrast, container opens at `containers.rs:365-372` already check it. Go's common openContainer sneak gate at `packages/server/sim/entity/container.go:106-108` precedes the workbench arm118.

Minimum transcript: Active player with Runtime.controls.actions.sneaking=true and Personal grid, Ready bench within downward ray reach. Admit/drain OpenContainer. Rust returns applied1 and widens to Workbench; Go refuses InvalidInput without changing grid/anchor/view. Temporary `review_sneaking_bench_must_refuse` fails Workbench vs Personal.

Required acceptance: sneaking bench open preserves complete runtime/inventory and any existing exact chest/furnace lease; unheld and explicitly non-sneaking controls allow opens; no packet must use retained held controls. Include a real tick path.

### P1: Depths workbench opens are unconditionally refused

Location: `crafting.rs:1240-1242`. Workbench opening requires Overworld before any ray lookup even though the provider's ray and anchor validator already accept a dimension. Go `container.go:65` looks up session.dimension and the workbench arm118-129 has no Overworld restriction.

Minimum transcript: Active actor in DEPTHS, Personal grid, Ready DEPTHS chunk at `(0,0)` with workbench `(0,63,0)`, player `[0.5,64,0.5]`, downward open ray. Rust reports rejected1 and keeps Personal; Go opens the workbench in the player's dimension. Temporary `review_depths_bench_must_open` fails Personal vs Workbench.

Required acceptance: identical real bench flows in both dimensions, anchor reach/mining/unready invalidation there, and a dimension move into a same-coordinate bench vs an absent one. Current test `crafting.rs:2088-2143` explicitly tolerates refused opens off-overworld; that expectation is not source parity.

Related shared-boundary gap, distinguished from the local workbench bug: containers.rs hard-gates Overworld in settle_open and move/viewer basis because accepted Rust ContainerRef (`mornlea_domain/src/locations.rs:166`) omits dimension. Go ContainerRef carries Dimension and `containerChunk` resolves it. This is broader than the known furnace-interest dimension-loss issue; container views cannot represent the Go Depths references. It requires controller contract reconciliation and consumer revalidation. No contract design is supplied here.

### P2: ordinary inventory and crafting mutation omit the Active lifecycle gate

Locations: `inventory.rs:374-381` resolves only inventory; `crafting.rs:1022-1032` and TakeCraftingOutput1060 onward likewise never inspect actor/lifecycle. Go tick.go139-160, 305 onward, 412-418, 568-578 and600-610 all refuse non-Active players. Rust container settlement and tools already check Active, so this is not a consistently chosen authority policy.

Minimum transcripts: (1) Respawning actor + empty inventory, SelectHotbar1 → Rust applies and changes selected to1; Go refuses PlayerNotReady. (2) Respawning actor + Personal grid containing oak log1 at cell0 → Rust TakeCraftingOutput applies and creates oak planks4 in inventory0; Go refuses PlayerNotReady. Temporary `review_respawning_inventory_must_refuse` and `review_respawning_crafting_must_refuse` both fail.

The selected-slot case needs no impossible nonempty post-death inventory. The crafting-output case is a provider-level seeded-state gate proof; a normal death lifecycle usually empties its grid. Production session Active status and actor Respawning status are separate, and reducer intake does not prefilter actor lifecycle, so session admission is not a replacement for the omitted gate. Missing respawn activation in the earlier report remains a separate assembly defect.

Required acceptance: Active/Dead/Respawning/Pending/absent-actor matrix for inventory and crafting operations, exact no-effect refusals, plus normal death→next input live ticks. Update actor-less inventory replay fixtures; their current ability to settle with no actor masks this omission.

## Checked production branches and what passed

Read all four owned production files (inventory425 lines, containers1432, crafting1566, tools337), their directly cited current Go sources, and associated test setup/coverage and representative assertions. This is complete reading of this four-file production scope, not an exhaustive audit of all server rules or every line of the ~6361 associated replay-test lines.

- Inventory: hotbar idempotence; empty/full/same-item/unlike whole moves; single and ceil-half truncation/no swap; both quick-move region directions, merge-before-empty ordering, no-fit; item count/durability conservation; four armor mappings, zero historical forms, wear boundaries and melee/projectile-only reduction. Arithmetic is bounded by accepted domain slot constructors and valid stack ceilings; no new arithmetic overflow or count-duplication mismatch was found here.
- Containers: exact Ready generation/reference/viewer basis; active lifecycle/refusal order; authority ray/open and sneaking; close with extended-only grid repack; chest and furnace whole/partial pack and cross-region transfers; unlike swaps and target constraints; output-only-as-source; input-kind progress resets; coal fuel; registered output whitelist; quick transfer candidate order and partial absorption; whole drops from pack and each material slot; compound inventory/container/drop staging; shared viewers and no double debit; full drop slots and exhausted durable chunk revision; reach invalidation delayed to publication rather than per transfer. The existing focused19 tests pass. Publication and chunk revision advancement themselves are separate earlier assembly findings.
- Crafting: checked all25 recipe shapes, outputs/counts/durability/mirror flags against current recipe.go; normalization, outer trim/interior holes, mirror retry, rotation/vertical-flip rejection, strict consumption and durable-ingredient refusal; effective personal/workbench cell bounds; whole grid movement with non-swapping unlike target; four-phase output insertion plus post-consume repack rehearsal; extended-only close and all-grid death helper; Ready/ray/reach/block/anchor lifecycle; re-anchor and grid preservation; saved-grid/anchor blindness. Existing15 tests pass while omitting the verified inputs above.
- Tools: hoe material/durability and last-use broken form; above-air and world-height checks; wheat/potato/carrot immature-stage bone meal; water-source-only collection, shared solid blocking and flowing transparency; adjacent placement, source/solid refusal and flowing replacement; bucket singleton swap; inventory and block mutation transaction; charge/mining suppression preflight; success sequence emission. Existing9 tests include charge/suppression capacity and foreign-call refusal and pass. No additional local tool conservation mismatch was confirmed. The earlier lost late exhaustion receipt and generic absent wire result projection are deliberately not counted again.

## Remaining coverage and uncertainties

- No real endpoint/worker/socket gameplay or Go-vs-Rust executable differential replay was accepted here. New temporary cases use actual providers with test-owned scenes; they cannot establish production startup/chunk hydration/persistence/visibility/event reachability.
- Counterexamples use valid domain inputs and source behaviors, but inventory record constructors/restore validators, damage providers, world mutation/concurrent publication atomicity and drop provider algorithms are outside this bounded scope. Their broader ownership issues remain open in the prior core review.
- Source-only defensive-policy difference: Rust crafting auto-close records a rejected close and keeps the bench when repack is impossible (`crafting.rs:1136-1210`), whereas Go `advanceWorkbenchLifecycle` panics on this internal invariant break (`crafting.go:312-315`). Existing Rust replay explicitly pins the softer behavior. Do not call these equal without an approved error-policy ruling; no new natural-gameplay witness was executed here, so this is not counted among the six behavioral findings.
- Both current Go and Rust ordinary inventory movements skip the grid-repack rehearsal. Their item-multiset conservation does not by itself prove the order-sensitive four-phase repack invariant: moving the sole empty cell between hotbar and backpack can cause an earlier grid item to use that cell before an available backpack merge, stranding a later grid item. This is an inherited source/invariant concern rather than a new translation mismatch; a complete reachable command transcript and controller requirement/error-policy reconciliation are still needed before treating it as a verified repair node.
- Missing actor retirement, command-loss/error swallowing, environment resets, absent chunk acquisition, persistence snapshots, sleep ingress, dynamic projection and furnace-interest dimension reduction were already reported. They are not duplicated as provider-local findings.

Current acceptance inventory therefore still cannot support zero-gap F2. Passing existing provider suites plus source identity checks miss valid command families, lifecycle gates, dimensional behavior and phase-sensitive source order.

## Continuation source census, 2026-10-03

The continuation starts from remote92ccbb45 and independently rechecks the current consumers. Packet94 corrects source-player trample capture only. The following joins remain open; existing component tests and checked task totals do not accept them:

- Source Snow needs a retained per-player/passive tracker and source capture timing before Safe/death. The reducer still creates a new generic FootprintSchedule each tick and collects late. Source reset clears the player tracker. External actual-native calibration with neutral input and velocity4 reset each tick reaches travel0.599998474 after eight grounded ticks, below the f32 stride threshold; an eight-tick crossing expectation would be incorrect. Exhaustive positive-finite-f32 radicand comparison found no sqrt narrowing difference on this platform, so the different arithmetic expression alone is not a demonstrated mismatch.
- Source subscription reconciliation and visibility ownership remain absent. Manually supplied chunk wants and the fixed source radius do not implement the source pending/active/reset, companion, retry and revision policies.
- Final terrain/entity/inventory/crafting/container dirty publication lacks the production visible-set scheduler. Component encoders and player-state output are separate evidence.
- Successful late till costs are queued after the existing post-physics receipt drain and are not carried to another tick; production mining has no corresponding cost receipt. Exact successful-action settlement and final publication order still require integration.
- The four actor projections have no automatic live save-target consumer. Actor revision/ACK eligibility, retired-player cache/relogin handoff and disconnect/final-flush projection remain open. Managed live selection currently owns chunk targets.
- Actual startup loading and resident installation of nonempty companion/hostile/passive families remain absent from the background player/chunk load owner.
- The selected executable still reserves the game listener and serves control without assembling gameplay admissions, ticks, subscriptions, publication and automatic saves. Control/lease/rollback component acceptance does not establish an authoritative gameplay endpoint.

The current full-corpus source mappings and Rust-versus-Rust repeat tests do not execute every supported Go-versus-Rust outcome. Inventory mapping reconciliation, nonempty save/restart and real endpoint acceptance remain required under3.7/3.8/4.1. Source and calibration reports are retained in continuation scratch; this appendix records verified scope rather than a new implementation plan or task closure.

The Linux supported-package quality script passes in this environment. The required generic make dev-check fails while importing the Darwin-only client app, before its Rust stages. Native macOS dependency checks also fail for missing codesign/install_name_tool. These are platform acceptance blockers; the supported Linux gate cannot replace the full six-module/macOS stage evidence, and4.2 remains open. No client or Godot changes are authorized to bypass them.

### Verified continuation delta after player Snow

The preceding continuation census records the a9cb341d frontier. Source player Snow is now accepted at e6923f6d (reviewed7c9552e8): its inline tracker persists across actual ticks, its fixed8 candidate batch captures before Safe/death, reset clears only the tracker, and fresh late settlement excludes actual source players from legacy collection. Author/reviewer1208actual+3docs and ROOT1396actual+3docs are qualified node evidence; they are not full F2 acceptance.

Remaining source facts are separately verified: passive Snow still lacks retained ownership/pre-death capture, and passive native movement omits Go thick-Snow walk-speed scaling0.7. Final action costs still lose late Till receipts and lack successful Mining receipts; direct melee100 already settles. Actual freeze_environment resets tunables to source defaults, so configured nondefault tick tuning has no actual runtime owner. Source subscriptions remain manual; declared login view distance is not retained. Retired player records remain in the actor vector while active-player/environment/mining enumerations lack a live-session filter; persistence/cache/retirement integration must resolve their production ownership. These are source findings, not newly executed failed runtime cases. Automatic targets/cache/bootstrap, visible dirty publication, assembled gameplay endpoint and real every-outcome inventory/parity remain open. No F3/client/render/assets change is authorized by this census.

### Verified continuation delta after action costs

Packet96 closes late successful source-human Till/Mining scalar settlement at f9afd5b9 (independently reviewed bc1d068c), with ten intended compiled assertion RED plus two preservation controls and ROOT1408actual/3docs including49 current-release activation cases. The preceding census remains historical; these two missing cost consumers are now implemented. Direct melee100 remains unchanged, and actual successful human melee endpoint acceptance remains separately open. Prepared custom thresholds do not close configured runtime tuning. Independent nonplayer census additionally verifies that all three actual companion/hostile/passive native exits omit thick-Snow0.7 scaling, while accepted player motion already applies it. No other remaining source/save/publication/bootstrap/executable/every-outcome or full platform gate is closed by this node.
