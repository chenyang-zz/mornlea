# Frozen pre-physics hostile targeting, melee and ranged action production

Required execution skill: executing-plans. Status belongs only to tasks.md.
Goal: implement source-compatible hostile action production for node 2.7a1:
pre-physics target facts, per-tick melee intent batch, and hurler ranged shot
production with exact spread, cooldown and spawn semantics. No death, no hit
settlement, no reducer acceptance. Rust 1.97.1+, existing domain, protocol,
storage and numerical ports only. Oracle: packages/server/sim/entity/hostile.go,
hostile_action.go, hostile_manager.go (server/hostile_manager*.go),
sim/contract/contract.go, sim/runtime/hostile_action.go, updates/sampler.go,
projectile.go. Consumer contract: plans/17-combat-input-contract.md (accepted
SHA df3b4ac7). Motion baseline: rules/hostile_actors.rs (accepted node 2.7a).

## Ownership and accepted prerequisites

S = packages/engine/crates/mornlea_server. Worker creates and owns:
- S/src/rules/hostile_actions.rs (new reserved provider module).
- S/tests/server_replay/hostile_actions.rs (new reserved topic).
Worker narrowly edits:
- S/src/rules/hostile_actors.rs: kind-branched hurler band steering only (see
  below) plus exposing the existing nearest-target helper as `pub(crate)`.
  No other motion, spawn, burn, distant, path or runtime-field changes.
- S/tests/server_replay/hostile_actors.rs: hurler band steering regressions
  only.
- S/src/rules.rs (or equivalent module index): register the new module.
All other files read-only, including S/src/core/contracts.rs,
S/src/rules/hostile_outcomes.rs, S/src/rules/player_survival.rs,
S/src/rules/projectiles.rs and every other replay/contract topic. Main owns
guides, artifacts, integration and rollback. Accepted HostileMeleeBatch
contract df3b4ac7; interaction classifier 5f8325b2 and math helpers 24a42ee7
for the line-of-sight walk. No new dependencies, wire/save/ABI changes, Go
source edits, fixture-only fallbacks, death/reset, sleep routing, new shared
damage lane or world scans. No new directory.

## Frozen interfaces

In hostile_actions.rs add:
- `pub struct HostileActionPlan` with private tick and private ordered melee
  entries plus private shot stagings. Only planning may construct it.
- `pub fn plan(ctx: &TickContext<'_>) -> Result<HostileActionPlan, ServerError>`.
  Pure read-only planning from the pre-step overlay: selects targets, admits
  melee intents, decides hurler shots. No staging, no mutation.
- `pub fn apply(ctx: &mut TickContext<'_>, plan: HostileActionPlan)
  -> Result<PhaseReport, ServerError>`. Stages exactly what planning admitted:
  no re-selection, no new admission.
- `pub fn run(ctx: &mut TickContext<'_>, call: RuleCall<'_>)
  -> Result<PhaseReport, ServerError>` accepts only a new empty
  `RulePhase::HostileActions` shape (see contract note below); invalid shape
  is InvalidInput field "hostile_action_call". Add the
  `RulePhase::HostileActions` variant in contracts.rs ONLY as an additive
  enum arm with no other contract change; the reducer (node 3.1, future) must
  call this provider strictly before HostileMotion each tick and pass
  `ctx.read().tick()` through. Batch/plan tick mismatch with the current tick
  at apply time is InvalidInput field "hostile_action_tick", before effects.
- `PhaseReport`: examined is admitted-candidate count, applied is staged
  intent count (melee entries plus successful shots), rejected is
  suppressed/duplicate/unknown/refused intents, carried 0.

Melee batch construction uses `HostileMeleeBatch::try_new` with the current
tick and entries in hostile-ID order. Overflow is impossible by construction:
resident hostiles are capped at 64 by the accepted motion provider, one entry
per hostile at most, and the constructor still guards. Missing environment or
tick/world_time observations are Internal with a concise invariant string,
before any effects. Fresh spawns (`fresh` set) produce no intent this tick.
No hidden tick-once state: the reducer owns one invocation per tick.

## Source algorithm and decisions

Target facts mirror the accepted motion selector: nearest Active player in the
same dimension by squared horizontal distance, exact tie by smaller PlayerID
bytes. Reuse it by extracting the existing private selector in
hostile_actors.rs to `pub(crate)` without changing its metric; do not write a
second selector. Cross-dimension players are excluded and clear nothing here.
Zero-health hostiles neither attack nor shoot; Dead/non-Active players are
never targets. Hurlers never receive melee entries even within reach, matching
`TestHurlerNeverSubmitsMeleeIntentWhenWithinReach`.

Walkers submit a melee entry every tick their selected target is within
1.8 squared horizontal distance, regardless of attack cooldown: cooldown
gating belongs to settlement (plan19), which revalidates from its frozen
snapshot. The producer never reads or writes attack cooldowns and never
damages. First-valid-known-ID wins falls out of unique-ID iteration order;
unknown or malformed candidates cannot occur from the read view, so no inbox
map is needed. This preserves Go's submit-while-cooling timing
(`TestHostileChaseHoldsAttackWhileCooldownActive`,
`TestHostileChaseCooldownOneAttacksSameTick` are consumer-side oracles; the
producer-side proof is entry presence with cooldowns untouched).

Hurler motion reconciliation (hostile_actors.rs only): replace the uniform
1.8 attack stop for hurler kind with source bands by squared horizontal
distance to the selected target: beyond 14 approach through the existing path
machinery; 6..14 hold position (inclusive edges hold, per
`TestHurlerHoldsAtBandEdges`); below 6 retreat along the straight
target-minus-hostile horizontal line without pathfinding. Band checks run
where the current `within_attack` stop runs, before path dispatch. No new
runtime fields, no path-struct changes, no walker behavior change.

Hurler shots: a hurler shoots only when its pre-step `shoot_cooldown == 0`
(cooldown 1 shoots nothing this tick even though motion later decrements it),
its target is live in the same dimension, and the eye-to-eye segment is
unobstructed. Eye is position plus the same tunable eye height the combat
consumer uses; reuse that accessor, do not add a second constant. Aim is the
normalized eye-to-eye direction; velocity follows Go `hostileShardVelocity`
with `HostileShotSpread` SplitMix64 yaw/pitch offsets uniform in plus/minus
0.06 rad keyed by seed, world_time (not tick), and hostile ID. Damage 3,
speed 22. Line of sight uses accepted `NativeRaycast` with
`target_block` occlusion: any blocking cell strictly before the target eye
blocks; equality with the target cell does not. This narrow occlusion helper
duplicates the consumer's rule by necessity (parallel work owns the
consumer's copy); unification is main's integration job, recorded below, not
the worker's.

Shot settlement stages through accepted ports only: spawn the shard with
`projectiles::spawn` (before None, after Some record; ID from
`derive_spawn_id`), and on success stage `Runtime` with `shoot_cooldown = 40`.
Spawn refusal (port capacity eviction) stages nothing and consumes no
cooldown. Cooldown/alive/kind rechecks at apply time use latest records; a
stale candidate stages nothing for that intent and never blocks other intents.

## RED/GREEN steps and exact acceptance

1. Write replay tests against the missing provider before its body. Verify
   named RED output; zero filtered tests is not evidence.
2. Melee production: in-range walker entry present every tick including while
   attack cooldown is active, with all cooldowns and health unchanged by the
   producer; out-of-range, dead-walker, fresh-spawn, cross-dimension and
   tie-break (smaller PlayerID bytes wins) cases; hurler within reach submits
   no melee entry. Full 64-resident house produces an accepted 64-entry batch.
3. Hurler bands: far approaches along path, mid-band and both edges hold,
   close retreats straight-line; walker stop unchanged. Band tests live in the
   hostile_actors topic and assert positions only, never intents.
4. Shots: cooldown 0 plus clear LOS stages exactly one shard spawn and a
   40-cooldown runtime; cooldown 1 stages nothing; LOS-blocked stages nothing;
   spawn-refused (exhausted projectile capacity) stages no cooldown; eye math
   and spread bits match Go KATs from sampler and hurler shot tests
   (worker reads vectors from those Go tests, never invents them).
5. Run pinned focused hostile_actions and hostile_actors, contract
   combat_input, replay projectiles regression, full server, clippy
   all-targets -D warnings, fmt, diff; `make rust` before focused Go
   sim entity HostileAction/HostileMelee/HostileShot and server
   HostileChase/Hurler plus updates sampler spread oracles (list actual
   matches). Independent review covers source parity, pre/post-decrement
   timing, band edges, and spawn-port discipline. Worker scoped English
   commit, main integrates, updates guide/tasks/ledger and reruns integration
   gates. Refusal/conflict returns evidence to main; no invented policy. Main
   owns rollback and revalidation of affected providers.

## Review focus and known follow-ups

Pre-decrement shot timing, band-edge inclusivity, stale-candidate suppression
without cross-intent blocking, and spawn-refusal-without-cooldown are
explicitly covered above. The LOS helper duplication with the in-flight melee
consumer is intentional parallel safety; main unifies the copies at
integration (node 3.1) and records the surviving owner. Reducer order
HostileActions before HostileMotion is a 3.1 constraint owned by this packet;
motion's current standalone acceptance does not claim it.
