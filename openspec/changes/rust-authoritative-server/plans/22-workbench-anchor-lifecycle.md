# Workbench anchor lifecycle and source container-view replacement

Required execution skill: executing-plans. Status belongs only to tasks.md.
Goal: complete source-compatible workbench anchor lifecycle for node 2.6c1:
transient anchor storage, end-of-tick revalidation (block, chunk, reach),
walk-away/mined-same-tick/dimension-change closes, and bench-open-clears-lease
with chest/furnace-open-preserves-bench. No death/disconnect repack changes,
no reducer acceptance. Oracle: packages/server/sim/entity/crafting.go,
container.go, tick.go, death.go, player.go, mining.go. Baselines: accepted
crafting provider 2.6c, container views 2.8b4b, classifier 5f8325b2, math
24a42ee7.

## Ownership and accepted prerequisites

S = packages/engine/crates/mornlea_server. Worker edits only:
- S/src/rules/crafting.rs: anchor storage, open/advance lifecycle, stale
  comment updates listed below.
- S/tests/server_replay/crafting.rs: anchor lifecycle regressions.
- S/src/core/contracts.rs: exactly one additive `ActorAux::Player` field
  `workbench: Option<BlockPos>` (see storage rule). Nothing else there.
- Mechanical `workbench: None` initializations made necessary by that field,
  and nothing else, at: S/src/rules/player_survival.rs (~285),
  S/src/rules/player_motion.rs (~384), S/src/rules/sleep.rs (~213, ~412),
  and exhaustive `ActorAux::Player` literals/comparisons in
  S/tests/server_contract/contract_double.rs (~1182),
  S/tests/server_replay/containers.rs (~2145), S/tests/server_replay/mining.rs
  (~253), S/tests/server_replay/eating.rs (~186),
  S/tests/server_replay/player_survival.rs (~215, ~1112),
  S/tests/server_replay/projectiles.rs (~357),
  S/tests/server_replay/sleep.rs (~246, ~791, ~902). These are
  zero-behavior initializers; `settle_death` already uses struct-update
  syntax and needs none. If any further literal breaks, worker stops and
  reports instead of widening scope.
All other files read-only, including S/src/rules/containers.rs (source),
S/src/core/container_store.rs, S/src/core/state.rs, death/session providers
and guides. Main owns guides, artifacts, integration and rollback. No new
dependencies, wire/save schema changes, Go source edits, fixture-only
fallbacks, new shared lanes or world scans. No new directory.

## Frozen interfaces and storage rule

The anchor is a bare `BlockPos` with no dimension and no generation,
meaningful only when `crafting_size` is Workbench, mirroring Go
`player.workbench` (player.go:147, container.go:97). It lives in
`ActorAux::Player.workbench` as runtime overlay only and is staged via
`RuleEffect::Runtime` beside the existing neutral-field convention. It is
never serialized: worker proves read-only that player save encoding never
reads `ActorRuntime`, and adds a save-roundtrip regression showing a set
anchor does not survive load. If any save path reads runtime today, stop and
report to main instead of redesigning persistence.

Stale anchors are inert by construction and never explicitly cleared: every
read gates on size Workbench first, close drops size to Personal, and each
open overwrites. This mirrors Go, where `closeWorkbench` never clears the
position.

## Source algorithm and decisions

Open (`settle_bench_open`): keep Active/overworld/ray/`WORKBENCH_BLOCK(45)`
gates. On hit, stage one `Compound` of `Inventory` (size Workbench, grid
untouched) + `Runtime` (anchor = hit block) + `Viewer{view: None}` (ends any
container lease, mirroring Go container.go:125-126 and the containers close
Compound precedent). Replace the already-bench `Carried` shortcut: re-open
always re-anchors to the new hit, always clears the lease, never touches the
grid or size. Chest/furnace open needs no change here: it already writes no
crafting state, which is exactly Go's preservation-by-absence; worker pins
that with a regression (container open leaves size Workbench and anchor
intact) without editing the container provider.

Lifecycle (`advance`, `WorkbenchLifecycle` phase, after deferred opens in
admission order): collect size-Workbench player actors sorted by session key
(reuse the existing sorted collection). For each: lapsed (non-Active) actors
close exactly as today. Active actors run `anchor_valid`: current-dimension
block lookup fails, chunk observation missing or not Ready, block not
`WORKBENCH_BLOCK`, or eye-to-block-center distance beyond
`interaction_reach` (eye = position + `eye_height` tunable) closes the bench.
Close = existing `close_bench` (repack 4..8, size Personal). Dimension change
needs no explicit rule: the lookup runs in the actor's current dimension, so
a moved actor fails unless the same coords hold a workbench there, exactly
like Go. Missing traversed observations close rather than veto, matching the
interaction missing-cell philosophy.

Repack-impossible mapping (controller ruling): Go panics, whose load-bearing
property is no-loss, not the crash. Rust keeps the bench open with grid and
size unchanged, counts `rejected`, and stages nothing for that actor; sibling
actors proceed. Never drop items to force a close. Operator surfacing of
`rejected > 0` belongs to the reducer/observability path (node 3.1/3.7),
explicitly out of scope here.

Crafting with a stale anchor is never refused: moves, takes, partials and
drops gate on size-derived slot extent only, until the lifecycle closes the
bench. Pin this with a regression. Death and disconnect repack-first hooks
stay with their owners (2.7c2, session lifecycle): this packet records them
as constraints, not code. Same-tick mined-bench close is proven by sequencing
mining settlement before the lifecycle in fixtures plus a 3.1 order
constraint (container moves, mining, workbench lifecycle last), not by a
dedicated mining hook, mirroring Go tick.go:844-862.

Update the superseded comments in crafting.rs: the anchor-stays-with-
publication note (~1121-1127) and the mutual-exclusion-deferred-to-reducer
notes (~39-40, ~1181-1183) now describe only the container-open direction
and the future same-tick cross-provider open conflict, which this packet
leaves for 3.1 (each direction is unit-tested separately from staged
fixtures; no combined same-tick open behavior is pinned here).

## RED/GREEN steps and exact acceptance

1. Write replay tests against the missing lifecycle before the provider body.
   Verify named RED output; zero filtered tests is not evidence.
2. Anchor storage: open stages anchor + size + lease-clear atomically;
   re-open re-anchors and re-clears with grid intact; save roundtrip drops
   the anchor without touching the grid.
3. Lifecycle closes with repack: walk-away (moved beyond reach), mined
   (block observation changed same tick), not-Ready chunk, wrong block,
   dimension move, lapsed actor. Each asserts repacked inventory, Personal
   size, and unchanged surroundings.
4. Preservation: repack-impossible keeps grid, size and anchor with
   `rejected` counted and siblings unaffected; stale-anchor crafting works
   until close; chest open preserves size and anchor; lapsed-only and
   already-covered close/refusal tests stay green.
5. Run pinned focused crafting replay, container replay lease/close
   regression (mechanical aux initializers only, no behavioral edits there),
   full server, clippy all-targets -D warnings, fmt, diff;
   `make rust` before focused Go workbench oracle tests
   (`TestWorkbenchOpenSetsSizeThreeWithoutContainerRef`,
   `TestWorkbenchOpenRejectsNonWorkbenchTarget`,
   `TestCraftingWalkAwayClosesWorkbenchAndRepacks`,
   `TestCraftingWorkbenchMinedClosesSameTick`,
   `TestCraftingDisconnectRepacksGridIntoSnapshotInventory`,
   `TestTunablesSnapshotAffectsWorkbench`; list actual matches). Independent
   review covers no-loss preservation, no-refusal crafting, re-anchor
   overwrite, and save-encoding blindness. Worker scoped English commit,
   main integrates, updates guide/tasks/ledger and reruns integration gates.
   Refusal/conflict returns evidence to main; no invented policy. Main owns
   rollback and revalidation of affected providers.

## Review focus

Repack-impossible preservation without cross-actor blocking, stale-anchor
crafting legality, re-open overwrite (not idempotent skip), and the
lease-clear/lease-preserve asymmetry are explicitly covered above. Death and
disconnect repack-first remain owner-gated constraints for 2.7c2 and session
lifecycle, not acceptance of this node.

## Verified source close outcome correction

Packet129 supersedes only the historical repack-impossible continuation ruling: verified Go automatic close failure is an internal invariant panic, so Rust propagates typed Internal to the accepted sticky failed-tick owner and stops later actors while preserving all content. Successful explicit/automatic close also retains source inventory/crafting dirty intent after atomic stage, including an empty extended grid. Explicit failed close remains ordinary wire InvalidInput. Original source history and accepted anchor/output scopes remain recorded.
