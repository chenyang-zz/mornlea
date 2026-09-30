# World provider review

Repository: `/Users/chen/work/mornlea-f2-server`. Initial source SHA `5ab842a6784d50425001ba28be4c6fe3f24a04da`; final verified HEAD `ec9b83f4a130d590afc91e2c574166d91ca3ef15`. `git diff 5ab842a6 HEAD --` all nine owned production files is empty. The controller committed other work concurrently; no repository file was edited by this reviewer.

Owned paths are relative to `/Users/chen/work/mornlea-f2-server/packages/engine/crates/mornlea_server/` below. Entire production files inspected: `src/rules/environment.rs`, `sleep.rs`, `crops.rs`, `farmland.rs`, `fluids.rs`, `random_blocks.rs`, `supports.rs`, `furnaces.rs`, `world_mutation.rs`. Applicable config, server/engine guides and approved rust-authoritative-server delta specification were read. Existing executable/projection/persistence integration blockers and repaired resident climate/sleep continuity are excluded from new findings.

## Evidence and limits

Disposable crate `/tmp/mornlea-f2-world-review-probe` links the actual checkout server/domain/protocol/storage crates and copies only the nine matching server_replay modules plus their real root helpers. Original module names/assertions remain; new cases have `review_` prefixes. No production source is copied or mocked. These are actual **direct provider** tests with fixture TickContext setup; they do not execute an authoritative server process, TCP, disk worker, or an independent Go replay. Expected Go results below are explicitly **source-derived**, not claimed as executed Go differential evidence.

Commands/results:

- `cargo test --manifest-path /tmp/mornlea-f2-world-review-probe/Cargo.toml --offline -- --skip review_`: 68 original tests PASS, log `/tmp/mornlea-f2-world-provider-existing-green.log` (rerun after all ten review probes were added).
- `cargo test --manifest-path /tmp/mornlea-f2-world-review-probe/Cargo.toml --offline review_ -- --nocapture`: ten new probes FAIL, log `/tmp/mornlea-f2-world-provider-red.log`.
- No process integration, live persistence, exhaustive Go differential suite, statistics, or performance timing was run by this reviewer. Parent full-stage evidence must remain distinct.

## Confirmed actionable findings

### 1. Fluid rescan discards distinct sections of one chunk (P1)

`src/rules/fluids.rs:254–258` deduplicates solely on `held.key == work.key`. Each work item is one `section_y` and `rescan:338–377` removes it after scanning that section; no remaining section is queued or scanned. This copies Go chunk dedup without Go's whole-height cursor. Current Go `packages/server/sim/realm/environment.go:660–702` loops whole-chunk/neighbor-plane scanning, with section continuation in `scanPlane:708–751`.

RED `fluids::review_rescan_keeps_distinct_sections_of_one_chunk`: actual Ready chunk sources at(2,4,3) and(2,20,3); enqueue section_y0 and16, same key; rescan. Second source has `fluid_due=None`, expectedSome(1). This is provider work loss independently of the already-known absence of live rescan seeding. An integration owner cannot recover it merely by submitting all24 sections in one batch.

### 2. New-chunk fluid rescan omits adjacent ready water planes (P1)

`src/rules/fluids.rs:428–471` scans water only within the supplied section and enqueues neighbors only after finding its own fluid. It never scans the ready neighboring chunk planes. Go `environment.go:660–697` explicitly scans center and four ready horizontal boundary planes; source `fluidBoundaryPlanes:645–650` fixes their positions.

RED `fluids::review_new_chunk_rescan_reawakens_adjacent_water`: new empty chunk's boundary air(15,10,0), ready neighbor water source(16,10,0), both keys in scope, only new chunk rescan requested. Rust enqueues neither source nor boundary air; source_dueNone insteadSome(1). Source-derived Go result reawakens neighbor water so it can enter newly ready air. No world process path is claimed.

### 3. Sleep refusal changes respawn runtime, and the full-record case is reachable (P1)

`src/rules/sleep.rs:223–227` stages Runtime before validating enlarged beds at237. `SleepState::try_new` (`src/core/contracts.rs:1208`) limits beds to8. At a full record, a ninth distinct active session's valid bed interaction returns Err but leaves its respawn runtime staged. This violates the provider's own zero-effect refusal documentation at136–138.

RED `sleep::review_session_churn_bed_refusal_is_atomic`: admit8 actual sessions, retire first, admit replacement, actual key9; use full old8-anchor SleepState, active ninth actor and valid night bed ray. `enter` returnsErr, runtime changesNone→Some with respawn(Overworld,(3,1,5)). Go stores anchors on each actual player, and `executeInteractBed` (Go `sim/entity/sleep.go:65–72`) has no historical8-session table refusal.

`tests/server_replay/sleep.rs:1160–1167` falsely claims no ninth distinct key can exist. `src/core/state.rs:1105–1112` monotonically increments IDs; retire:468–481 releases occupied capacity without resetting next_session. Current provider/record lifecycle cannot accept replacement players once stale anchors occupy8 rows. The controller owns the pruning/anchor ownership policy; this review does not design a new shared record.

### 4. Certified finite boundary coordinates panic in two additional providers (P1)

- `src/rules/crops.rs:400,402`: `ceil(...) as i32 - 1` overflows at i32::MIN. RED `crops::review_trample_minimum_x_does_not_panic` uses admitted actual player, airborne pre-step and landed post-step at [i32::MIN as f32,1,0.5], valid environment. Debug panics400. In release, wrapping end can create an enormous range rather than the promised at-most2x2 candidates. This extends the prior player fluid_upper defect, and is not a separately invented save acceptance rule.
- `src/rules/sleep.rs:478` (also495) adds ±1 without checking bed-partner coordinate overflow. RED `sleep::review_bed_foot_at_minimum_x_does_not_panic`: certified observed west-facing foot77 at(i32::MIN,2,5), actor eye already in that cell, valid night environment. `enter` panics478 before any normal refusal. Source Go uses int32 wrapping, so this is a Rust overflow/bounded refusal defect; no claim that Go handles this extreme position safely or that current real restore activates it.

Both repros use actual ActorRecord/FiniteVec3 and BlockObservation admission; a file schema accepting finite coordinates alone is not proof of a live restore/physics route.

### 5. Sneak-held door interaction still toggles (P2)

`src/rules/world_mutation.rs:228–259` never reads held controls. Its docs26–31 and replay docs11–16 claim the held-controls surface is unavailable; this is stale since `AuthorityReadView::runtime` and `ActorRuntime.controls` are live and sleep already checks them. Go `packages/server/sim/entity/door.go:203–206` refuses a sneaking actor only after the ray hits a door (non-door stays silent success).

RED `world_mutation::review_sneaking_door_refuses`: valid source door ray, well-formed observed pair, actual staged controls.sneaking=true; run succeeds and toggles lower. Expected source refusal, no cell/inventory/event change. Do not move the gate ahead of the source non-door/no-target decisions when repairing.

### 6. Companion/hostile footsteps are accepted by the snow provider (P2)

`src/rules/crops.rs:417–456` filters only lifecycle/ground, accepts every ActorKey. Provider docs33–35 and334–336 correctly promise players/passives. Current Go only invokes `noteSnowFootprint` from `sim/entity/player.go:679` and `passive.go:302`; no companion/hostile caller exists (whole packages/server search checked).

RED `crops::review_companion_does_not_trample_snow`: real Companion ActorBody with grounded pre/post positions x0.1→0.9, snow88 at foot cell. Rust applied1, tier88→87; expected applied0/unchanged snow. Hostile consequence is source inference from identical missing actor-kind filter, not a second executed hostile probe.

### 7. Farmland reservations are charged as full reads even on early hydration (P2)

`src/rules/farmland.rs:438` spends162 before `neighborhood_is_wet:472–493` can exit on the first fluid. Go `environment.go:494–498` only checks enough remaining capacity for worst case, then `farmlandIsWet:299–316` increments counter for each actual read and exits immediately at water. Rust conservatively charges the entire reservation permanently, so same budget/input schedule yields different hydration/carry checkpoints and less tail rescan progress.

RED `farmland::review_wet_short_circuit_preserves_remaining_budget`: read budget165, check budget8; dry candidates(8,1,8),(9,1,8), first-enumerated water for each at x-4,z-4. Source Go target+firstwater costs2 each: both settle (4reads); after first, there remain163, enough for the second target read plus its162-read worst-case guard. Rust spends163 on first,1 target on second then defers: applied1/carry1 instead2/0. Existing reads_162_163 only exercises dry worst case and therefore hides the distinction. Expected Go behavior is source-derived.

### 8. Rescan changes queue seeding and omits source fixed-point suppression (P2)

`src/rules/fluids.rs:443–449` unconditionally enqueues every fluid. Source Go's actual `fluid.ScanRescanRegion` kernel and accepted engine `src/fluid_rescan.rs:470–479` suppress water sources with below and four horizontal neighbors all unreplaceable; uniform-source sealed sections also charge1 via446–459 instead of dense4096. Rust never calls the accepted rescan operation, though NativeFluidEval is used for updates.

RED `fluids::review_sealed_source_is_not_enqueued_by_rescan`: source(2,4,3), below/cardinal5neighbors stone. Rust schedules sourceSome(1); expectedNone. It also queues observed-air neighbors at fluids.rs450–468, whereas Go environment.go739–740 enqueues only the fluid positions emitted by the native rescan. RED `fluids::review_rescan_does_not_queue_air_neighbors`: one source plus observed air below gives pending2 instead of source-derived1; the below air is incorrectly due. Existing rescan_section_overshoot expects applied4 for two sources+two air cells and therefore pins this incorrect extra seeding.

This queue inflation consumes later fixed update budget ahead of productive cells; repeated restore/rescan fixed-point cases do not have source-equivalent work/carry. Uniform-section cost difference is source inspection, not an additional executed uniform-section probe.

### 9. Bounded update counters scan the entire due backlog (P1, source-confirmed)

`src/rules/fluids.rs:217–223`, used532 after update-budget refusal, and `farmland.rs:209–215`, used458 every tick, compute count_due via BTreeMap.keys().take_while(...).count(). For N same-due outstanding entries, a zero work budget still performs Θ(N) traversal outside `TickContext::charge`. Schedules have no enqueue capacity enforcement. Thus budgets bound evaluated items but not actual hot-path work.

This is direct source-confirmed complexity, not a timing inference or measured latency. Minimum discriminating test needs counted queue iteration/operation instrumentation, so no timing threshold or misleading timing RED was added. Go queue's AdvanceKinds and per-domain backlog ownership intentionally bound examination independent of total queue size (`packages/server/fluid/queue.go:156–164`, `packages/server/updates/queue.go`), mirrored by Go farmland_moisture_budget_test.go:59–94. Controller owns a bounded reporting policy; review does not select one.

## Known boundary defects retained as context, not new standalone claims

- Furnace provider `furnaces.rs:129–150` takes dimensionless ContainerRef, reads `view.container` and stages RuleEffect::Container; `state.rs:1466–1467` defaults that API to Overworld. `core/step.rs:638–651` loses dimension from interest. Go furnace.go:11–27 iterates dimension-bearing active keys. Internal Depths advancement is missing even though wire ContainerRef is **intentionally Overworld-only** (`packages/shared/network/protocol/message_container.go:134,151`). Do not broaden wire/domain acceptance to fix internal furnace enumeration. world_container already owns dimension. This is the previously controller-recorded assembly/shared-boundary issue, not a new provider regression probe here.
- Furnace literal1600/200 (`furnaces.rs:75,80,215,221`) ignores non-default immutable tuning; Go furnace.go:13–14 and44–74 consumes supplied snapshot. Controller has already opened tuning getter/consumer nodes. No duplicate finding or extra design.
- FootprintSchedule trackers (`crops.rs:231,429–431`) are never pruned/reset; Go tracker is owned by each resident player/passive and player reset clears it (`player.go:847`). A retained provider schedule can grow with lifetime actor IDs and suppress same-cell post-reset footprints. **Source inference only:** current reducer creates a fresh schedule at `core/step.rs:437`, so this retained-schedule lifecycle path is not current executable behavior. Fresh-per-tick loss of travel is an existing assembly defect, excluded from the new findings. Both facts must be reconciled by the eventual integration owner, not waived by provider docs.
- Fluid update snapshots read all observed Ready cells, not Go's active scope barrier (`environment.go:514–540`); update has no scope argument. This is a consumer/shared boundary concern alongside already-recorded active-set assembly, not a separate independently accepted live regression in this report.

## Reviewed algorithm branches and remaining coverage gaps

| Provider | Entire production source checked | Existing tests executed and branches reviewed | Uncovered / limits |
|---|---|---|---|
| environment |258lines | shape/no snapshot; weather remaining>1/1/0, salts/ranges; negative seed bit pattern; restore boundary, season roll, nonwrappingu64 edge, source-derived KAT rows | No new local mismatch found. Saturation deliberately strengthens Go wrap per frozen acceptance; resident continuity excluded. No independent exhaustive random Go comparison. |
| sleep |602lines | exact entry order, ray transparent cells/unobserved/no target, non-bed silent success, sneak/night, either half, anchor/workbench retention, movement/damage wake, empty/partial/full/disconnect/pending active roster, seasonal inverse/morning, record/full gates | New churn/refusal and coordinate probes above. Wake set O(sleeping×damaged)/eligibility contains searches are bounded by current8-player set; no new performance finding. No proof every admitted record is internally unique; real disconnect/respawn ownership remains integration scope. |
| crops |583lines | player landing edge/pre-step, strict coverage/X-Z order; bare farmland, three crops and deterministic wheat outputs, capacity/merge/stale ground and intentional GroundOnly second-write fault; snow stride and tier settlement source inspected | Existing four crops tests contain no actual snow stride family/lifecycle cases. New companion and bounds probes cover omissions. No process travel accumulation or passive-ID churn proof. |
| farmland |566lines | earliest due/order, global dimensions, scope skip, target read, whole judgement deferral, dry/wet bounds; radius4/dy0..1 and reverse wake; full-height24x24 halo order/cursor/carry | Existing2 tests cover dry reservation and global original-due carry only, no wet-shortcircuit or full halo work instrumentation. Checked overflow returnsNone rather than Go wrap at extreme chunk coordinate; controller policy needed only if supported edge fixtures demand parity. |
| fluids |704lines | earliest-due dedup/order, per-dimension ceilings, snapshot-beforewrite, realNativeFluidEval merge/sorted commit, retries/drop capacity, all crop/sapling flooding and two dimensions, due/carry, section overshoot | New section/neighbor/fixedpoint probes expose missing whole Go rescan algorithm. No actual active-scope ingress, full-height restart, uniform-source cost or queue-examination measurement test. Saturatingdue vsGo wrappingdue at u64 edge needs explicit source/spec ruling; not asserted defect here. |
| random_blocks |353lines | sampler hash/dimension/negativebits, replacement samples/order24sections, attempts0/64/>64; preflightbounds, all crops wet/sky/mature, dryfarmland30%, tree source kernel/geometry/obstruction/root<=311/allReady, cardinal grass spread, snowallowlist/layers/roof/hysteresis/weather/temperature | No new local mismatch found. Existing17 tests and source oracles check declared branches; direct climate floating-point drift not exhaustively measured. No fullGo replay/real-world publication proof. |
| supports |281lines | four pass source order/snapshots; no recursive ownpass, laterpasses see clears; sapling/shortgrass policy; all5 torch orientations/collisionallowlist; bedfoot/head/directions/pair clear/drop/capacity, unready skip; currentGo malformed partner behavior compared | No new local mismatch found. Atomicpair intentionally source rollback result; DropSource target integer avoids cell-center precision loss by shared transaction. Extreme X/Z wrapping matches source but live valid world traversal not tested. |
| furnaces |252lines | input/smelting table, full/different output pause, empty/no coal, same-tickignite1599/progress1, completion/reset/slotclear/outputcount, once-with-overlap/order/chest preflight no-prefix; original4 tests | Default provider algorithm matches source. Non-defaulttimings andinternalDepths ownership remain controller-open nodes. No corruptprogressu32 overflow claim for schema-rejected input. |
| world_mutation |282lines | actor/env/lifecycle basis, sharedraytargetclassifier/unobserved/reach/non-door, upper/lower wellformedpair/togglebit, actualplacementresolver, stale/duplicate/contention/two-celldoor/bed footprints | Sneak omission confirmed. Broader resolver command families remain their owners; has_view/sessionReady liveassembly not accepted by geometric fixtures. No ray budget defect asserted without an admitted tuning/cell path. |

The original68 replay passes do not close any of the nine new finding groups and do not satisfy full rule/integration acceptance. No scope waiver or completed task/ledger edit was made.
