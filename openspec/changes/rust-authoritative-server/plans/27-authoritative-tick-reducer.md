# Authoritative tick reducer

Required execution skill: executing-plans. Status belongs only to tasks.md.
Goal: implement the one serial tick reducer for node 3.1 in
S/src/core/step.rs, wiring every accepted provider in Go order with
retired-session filtering, explicit damage-victim routing, and bounded-scan
proof. No new gameplay, no save cadence changes, no agent install wiring.
Oracle: plans/01-server-slices.md:67 (3.1 slice),
plans/04-refined-nodes.md rows 1-11 + serial-integration obligations
(~430-446), plans/02-core-seams.md:123 (phase order),
plans/16-container-drop-views.md:39 (commit pruning).

## Ownership

S = packages/engine/crates/mornlea_server. Worker edits only:
- S/src/core/step.rs: the reducer.
- S/src/core/state.rs: narrow serial-owner additions ONLY — `sleep` and
  sleeping-set fields with accessors, `Sleep`-effect commit arm, spent
  readers, `push_companion_action`, `advance_tick` delegation to
  `step::reduce_tick`. No other behavior change.
- S/src/rules/sleep.rs: `settle` takes explicit `damaged: &[SessionKey]`
  instead of computing `damaged_sessions` (deleted to enforce the ban); no
  other behavior change.
- S/src/rules/projectiles.rs: `advance` returns
  `ProjectileOutcome { report, damaged_players }` (new struct beside
  `CombatOutcome`); no behavior change.
- S/tests/server_replay/phase_order.rs, S/tests/server_contract/tick.rs:
  order and mailbox tests.
- Mechanical call-site updates ONLY in S/tests/server_replay/sleep.rs
  (explicit damaged vecs; inference-based tests rewritten to explicit) and
  S/tests/server_replay/projectiles.rs (new return shape).
All other files read-only. Main owns guides, artifacts, integration and
rollback. No new phases, effects (beyond none), wire/save changes, Go
edits, or world scans. Single serial node by slice assignment; no parallel
sub-landing.

## Frozen entry and skeleton

`pub fn reduce_tick(state: &mut AuthorityState, budget: TickBudget)
-> TickPublication` in step.rs; `advance_tick` keeps its validations
(Running phase, budget shape, tick bump) then delegates. Tick flow:
drain mailbox (`freeze_eligible`, `budgeted_batch` with F1 `order_commands`
sort, `carry` remainder) → drop retired sessions' commands via
`state.session(key).phase` (count `stale`, never carried: retired never
returns) → run rows below accumulating reports → commit viewers with
retired pruning (retain Active sessions only; the 8-player ceiling is
structural, never arbitrary drops) → build publication
(`tick`, `ctx.events()`, `control` empty with transport-owned comment,
counters below) → `publish_tick`. EnvironmentEnd is called exactly once by
caller sequencing. Shutdown `run_final` stays injected; final-tick
suppression belongs to 3.8.

Counters: `executed_tick`, `commands` (drained), `carried`/`stale`
(mailbox-exact); `fluid_by_dimension`/`rescan_by_dimension` attributed per
dimension-scoped invocation; farmland checks/reads, snapshot bytes/chunks
from new spent readers. Control stays empty (handshake-owned).

## Row dispatch (04 table, resolutions frozen)

1. PlayerCommand in sorted order: intake admits (motion/inventory/containers/
   crafting) plus immediate single-envelope providers; placement/tools/drop
   admission defers (ContainerMove/WorkbenchLifecycle/Interaction bags
   preserve admission order).
2. CompanionIntent: `state.drain_companions(4)` fed via
   `push_companion_action`, then Intent selection. Then Acquire stub call.
3. Per sorted active player: RegenStarvation→Eating→BowDraw→Oxygen→Motion→
   PostPhysics with per-actor calls; then CompanionMotion batch.
4. `commit_viewers` (the only view-commit mechanism; no separate reconcile
   exists — recorded, companion placement still postdates it per table).
5. HostileActions FIRST (`plan` → `apply` → batch), then HostileMotion→
   Combat via `advance(ctx, &batch)` (collect `damaged_players`)→
   BurnDistant→ProjectileStep (`advance` with scopes, collect victims)→
   HostilePlayerDeaths.
6. PassiveStepDeaths batch.
7. CompanionPlacement batch, then ONE deferred Interaction loop merging bags
   by envelope sequence in original sorted order, routing each envelope to
   the single provider whose gate accepts it (place/door→world_mutation,
   tools→tools, drops→drops, bed-internal→`sleep::enter` threading the
   sleep record); inventory select stays PlayerCommand (its gate admits no
   Interaction — the plan row's "select" word does not override gates).
8. SleepSettlement via `settle(ctx, &record, &sleeping_minus_victims,
   &active)` with reducer-tracked record/sleepers persisted to state;
   then DropStep (`advance` with active keys) and FurnaceStep (refs from
   per-key `container_refs` filtered to furnaces, deduped).
9. FluidRescan→FluidUpdate→Farmland→Trample→SnowFootprint→RandomBlock with
   tick active scope; persisted schedule state passes through unmodified.
10. ContainerMove drain, MiningStep per-actor (Player→human,
    Companion→companion, others refused by provider), WorkbenchLifecycle
    advance, Support pass.
11. Commit overlay, Publish, EnvironmentEnd once.

Active-key set (shared by DropStep/Random/scopes): radius-2 chunk columns
around active players in their dimensions, sorted/deduped, >200 refused
like `drops::advance`. Projectile scopes and fluid/farm/crop schedule
derivation follow each provider's tests/docs read-only; cross-tick schedule
state passes through; anything undefined stops with evidence (no invented
derivation).

Victim routing: union of CombatOutcome and projectile victims feeds
`settle` explicitly; wire-`CombatHit` inference is deleted. Movers stay
computed inside `settle` from controls.

## Bounded-scan proof (the observation gate)

No new index structures: the reducer proves boundedness through existing
caps — active keys ≤200, players ≤8, combat 104/72/64/8, inbox 4,
512-cell flight, per-dimension fluid budgets — plus one adversarial test
(full residents, 200 keys, 8 players) asserting termination and budget
compliance. Any unbounded scan found stops with evidence.

## RED/GREEN and acceptance

RED: empty-table dispatch (no provider runs, empty publication); retired
session executes (fails); stale batch ordering; swapped-row order guard;
budget carry over two ticks; final-shaped EnvironmentEnd double-call.
GREEN: 01-slice red cases (subscription-edge visibility, place→fluid/
support same tick, death-before-passive-pickup, shared revision events,
valid/failed 1.6 mutations, shutdown-tick behavior stays injected-only)
plus 04 named tests (`combat_projectile_death_sandwich`,
`eating_before_motion`, `deferred_interactions_keep_order`,
`containers_before_mining_after_random` with full-fixture asserts).
Run full server_replay + server_contract, clippy all-targets -D warnings,
fmt, diff; `make rust` before Go `StepWithTunablesPinsBlockUpdatesSubOrder`,
`StepSequenceGuardRejectsSwappedOrder`, `PlaceBlockThroughFluid`,
`FarmlandMoisture` oracles (list actual matches). Independent review covers
order fidelity, victim routing, inference deletion, bounds proof, and
counter attribution. Worker scoped English commits (skeleton then
mechanisms allowed as two commits), main integrates, updates
guide/tasks/ledger and reruns integration gates. Refusal/conflict returns
evidence to main; no invented policy. Main owns rollback and revalidation.

Save cadence stays scheduler-owned (3.4b); agent install stays
endpoint-driven (3.6d); subscriber despawn projection stays
publication-owned — all recorded pointers, none pinned here.
