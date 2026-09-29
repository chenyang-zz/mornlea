# Player, hostile and passive death outputs and reset

Required execution skill: executing-plans. Status belongs only to tasks.md.
Goal: implement source-compatible death outputs and reset for node 2.7c2:
one-time player deaths (repack, per-slot ring drops, stat reset, anchor
teleport), hostile deaths (deterministic loot, whole-batch first-fit, same-tick
removal), and passive deaths (fixed loot, ring discipline). No combat-hit
changes, no reducer acceptance. Oracle:
packages/server/sim/entity/death.go, hostile.go, passive.go, crafting.go,
player.go, hunger.go, mining.go, tick.go. Baselines: melee 2.7c1, drops
2.8b1/2.8b2, crafting 2.6c/2.6c1, survival 2.5b, passives 2.8a, transaction 1.6.

## Ownership and accepted prerequisites

S = packages/engine/crates/mornlea_server. Worker edits only:
- S/src/rules/hostile_outcomes.rs: `HostilePlayerDeaths` phase runner plus
  private death helpers (hostile loot tables, hurler rolls, ring walk, player
  death assembly).
- S/tests/server_replay/hostile_outcomes.rs: death regressions.
- S/src/rules/passives.rs: loot staging inside the existing `settle_deaths`
  only; lifecycle termination untouched.
- S/tests/server_replay/passives.rs: passive loot regressions.
- S/src/rules/crafting.rs: one `pub(crate)` repack-all helper reusing
  `can_repack`/`add_stack` primitives (all 9 grid slots into inventory plus
  size Personal); no open/lifecycle changes.
All other files read-only, including S/src/rules/drops.rs (accepted
`DropBatch`/`check_drop_batch`/`try_system_with_drops` ports),
S/src/rules/player_survival.rs (accepted stat reset),
S/src/core/contracts.rs, S/src/core/state.rs and guides. Main owns guides,
artifacts, integration and rollback. No new phases (both death phases
exist), effects, dependencies, wire/save changes, Go edits, fixture
fallbacks or world scans. No new directory. Single-task boundary: the
accepted drop/transaction ports are the contract, no separate landing.

## Frozen interfaces

`hostile_outcomes::run` additionally accepts `RulePhase::HostilePlayerDeaths`
with actor/command/internal all `None`; any other new shape is `InvalidInput`
field `"combat_call"`, before effects. Order inside the call mirrors Go
`tick.go:695-697`: hostile deaths in hostile-ID ascending order, then player
deaths in session order. `PhaseReport`: examined is death candidates seen,
applied is settled deaths, rejected is skipped-but-eligible actors
(repack-impossible, all-chunks-full loot omits nothing—see below), carried 0.

One-time gates: hostiles with health 0 and not already `Dead`; players with
health 0 still `Active` (already-reset `Respawning` players are skipped, so
the survival-phase reset and this runner never double-settle); passives with
health 0 not yet `Dead`. Settlement is atomic per actor (one `Compound` of
actor/inventory/runtime/drops): never a looted-but-present or
removed-but-unlooted actor.

## Source algorithm and decisions

Player death mirrors `death.go:35-69`: (1) repack all 9 grid slots via the
new helper and drop size to Personal; repack-impossible preserves everything,
settles nothing for that actor this tick, counts `rejected` (same lossless
mapping the workbench node uses for the Go panic; the gate still matches
next tick so retry is automatic). (2) Per-slot world drops over slots 0..35
then armor 0..3: each slot independently rehearsed with `check_drop_batch`
against Ready-chunk candidates in Chebyshev ring order from the death chunk
(`death.go:115-153`, clamp to nearest column), first success commits through
the shared transaction; armor keeps durability; unplaceable slots stay with
the player. Drop pickup delay is the `player_drop_pickup_delay_ticks`
tunable; source is `Death{actor, tick}`. (3) Stat reset reuses the accepted
survival settle (full health/hunger/saturation, exhaustion/oxygen/transients
cleared, lifecycle `Respawning`); this runner stages the same reset values
rather than calling into the survival provider. (4) Teleport to the anchor
column at `MaxY+1` with zeroed velocity: prefer the live body respawn
position/dimension when present, else the world spawn anchor through the
same read path the accepted spawn provider uses (worker cites it read-only;
if no provider-visible spawn source exists, stop this sub-item and report,
everything else proceeds). Eating/bow clearing, regen-timer reset and bed
record preservation ride the reset values. Bed-tail restore re-mapping and
subscription-dirty signaling have no provider-visible ports and stay
owner-gated constraints for session lifecycle and 3.1; death emits no events.

Hostile death mirrors `hostile.go:449-525`: health-0 non-`Dead` hostiles in
ID order; loot batch walker 1 rotten flesh, hurler deterministic bones 0..2
plus bow 1/8 at full durability from a SplitMix64 chain over
`(seed, world_time, id)` with Go KATs (no re-roll on refusal); empty batch
skips pre-sim; otherwise first ring chunk whose rehearsal succeeds takes the
whole batch, all-full omits loot while death still completes; removal stages
lifecycle-`Dead` in the same `Compound` (slice excision stays
store/reducer-owned, consistent with the passives `Dead`-record precedent).
Passive death mirrors `passive.go:607-653`: fixed 1 raw beef batch with the
same ring/first-fit/omit discipline beside the existing `Dead` staging;
fall-out (invalid/Y-below-min) removal stays lootless as today. The passive
tick death-set has no consumer port: the set is derivable from same-tick
`Dead` records, and despawn-cause projection stays a 3.1 constraint.

Hostile-on-player `CombatHit` absence stands per the accepted plan19 ruling;
no hit-path change belongs here.

## RED/GREEN steps and exact acceptance

1. Write replay tests against the missing runner before its body (player
   repack-first, per-slot ring drops with clear-on-success, unplaceable
   stays, anchor teleport, double-settle no-op across both providers;
   hostile walker/hurler tables, empty-batch skip, ring-to-neighbor,
   all-full-omit, same-tick `Dead`; passive beef batch and lootless
   fall-out). Verify named RED output; zero filtered tests is not evidence.
2. Run pinned focused hostile_outcomes and passives replay, drops replay
   regression, survival replay regression, full server, clippy all-targets
   -D warnings, fmt, diff; `make rust` before focused Go entity
   `Hostile|Death|Drop|Pickup` oracles (list actual matches). Independent
   review covers one-time gates, ring order, whole-batch atomicity,
   lossless repack failure, and teleport sourcing. Worker scoped English
   commit, main integrates, updates guide/tasks/ledger and reruns
   integration gates. Refusal/conflict returns evidence to main; no invented
   policy. Main owns rollback and revalidation of affected providers.

## Review focus

Double-settle exclusion between the survival reset and this runner,
repack-impossible preservation with automatic retry, ring-order first-fit
with per-slot independence, deterministic hurler rolls without re-roll, and
same-`Compound` loot-plus-`Dead` atomicity are explicitly covered above.
Node 2.7c owns the Combat-to-death same-tick integration proofs on top of
this provider.
