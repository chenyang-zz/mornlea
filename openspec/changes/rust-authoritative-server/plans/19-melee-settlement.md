# Bounded melee settlement implementation plan

Required execution skill: executing-plans. Status belongs only to tasks.md.
Goal: implement source-compatible bounded combat freeze and settlement for node
2.7c1, without claiming death, hostile action production or reducer acceptance.
Architecture: readonly bounded snapshot/intents, followed by live identity
validation and one compound per winning hit. Rust 1.97.1, existing domain,
protocol, storage and numerical ports only. Spec: ../design.md and ../specs/.

## Ownership and accepted prerequisites

S = packages/engine/crates/mornlea_server. Worker edits only:
- S/src/rules/hostile_outcomes.rs (reserved provider, including private unit
  module for lowered capacity tests).
- S/tests/server_replay/hostile_outcomes.rs (reserved topic).
- S/src/rules/player_survival.rs: extract the pure exhaustion helper described
  below and thread the configured threshold through its existing call sites.
- S/tests/server_replay/player_survival.rs: configured-threshold regressions.
- S/src/core/contracts.rs: only the exhaustion threshold getter below.
All other files read-only; main owns guides, artifacts, integration and rollback.
Accepted HostileMeleeBatch contract SHA df3b4ac7; interaction target classifier
SHA 5f8325b2. Existing inventory damage, mining suppression and compound ports
are mandatory. No new dependencies, wire/save/ABI changes, Go source edits,
fixture-only production fallbacks, death/reset, ranged decisions, sleep routing,
new shared damage lane or world scans. Inherit crate guide: no new directory.
No generated/source-hashed consumer covers these Rust files; full server tests
and clippy cover existing consumers of the narrow threshold extraction.

## Frozen interfaces

In hostile_outcomes.rs add:
- pub struct CombatFrame with private tick:u64 and private bounded actor and
  intent vectors. Do not derive Clone; only freeze may construct it.
- pub struct CombatOutcome { pub report:PhaseReport,
  pub damaged_players:Vec<SessionKey> }. Victims are unique actual successfully
  damaged player sessions in settlement order, at most8; never event recipients.
- pub fn freeze(ctx:&TickContext<'_>, attacks:&HostileMeleeBatch)
  ->Result<CombatFrame,ServerError>.
- pub fn settle_frame(ctx:&mut TickContext<'_>, frame:CombatFrame)
  ->Result<CombatOutcome,ServerError>.
- pub fn advance(ctx:&mut TickContext<'_>, attacks:&HostileMeleeBatch)
  ->Result<CombatOutcome,ServerError> = freeze then settle_frame.
- pub fn run(ctx:&mut TickContext<'_>,call:RuleCall<'_>)
  ->Result<PhaseReport,ServerError> accepts only empty Combat shape, using an
  empty batch at ctx.read().tick(); invalid shape is InvalidInput field
  "combat_call". The actual reducer must call advance with produced choices.

Batch/frame tick mismatch is InvalidInput field "combat_tick", before effects.
Readonly private freeze_with_limits uses lower test ceilings, never exceeds
104 actors or72 intents. Overflow is Capacity {resource:RuleEffects,limit:
selected ceiling,observed:next count}; player count>8 is Resource::Players
limit8; hostile>64/passive>32 are RuleEffects with their family ceiling.
Validate counts before owned snapshot allocation; no truncation. Missing required
environment/player inventory/runtime or wrong actor runtime aux is Internal
with a concise invariant string, before any effects. Missing blocks during a
ray merely suppress that intent, as source. Source zero-health active actors
remain in snapshots for cooldowns but cannot attack or be selected; Dead and
non-Active players are excluded, companions excluded. Hostile/passive existing
records use their body identity. Check coordinates in f64 against i32 floor
bounds (eye too) before numerical walks; malformed records return InvalidInput
field "combat_actor" before effects. No hidden tick-once state: reducer owns
one invocation and discards the overlay on a fatal error.

Report examined is snapshot count, applied is successful intent count,
rejected is reserved-victim losers plus failed live-identity settlements,
carried0. Cooldown-only changes are not applied hits. Event tick0 is omitted,
matching existing projectile fixture policy; real ticks are nonzero.

## Source algorithm and decisions

Oracle: packages/server/sim/entity/combat.go, combat_*_test.go,
passive.go DamagePassive, hunger.go applyExhaustion, core WeaponDamage and
physics PlayerBounds. Freeze snapshots in hostile-ID, player-SessionKey,
passive-ID order. Decrement attack/hurt cooldown saturating in COPIES. Hostile
cooldowns belong to HostileMob body u8 fields (ActorRuntime neutral cooldowns
are not this authority); player cooldowns belong to runtime. Passives have0.
Use current inventory armor_points, not stale projected points.

Produce hostile intents first, then players. A chosen walker attacks only if
healthy, decremented attack cooldown0, chosen SessionKey identifies a healthy
player with decremented hurt cooldown0 in same dimension and current horizontal
distance squared <=1.8*1.8. No LOS or vertical gate. Hurler never melees. Absent
batch entry cannot infer an attack from saved chase UUID. Damage3.

Player primary held controls, healthy, decremented cooldown0; both bow forms62
and65 excluded. Eye = current position + tunable eye height. LookDirection uses
Go f64 sin/cos then f32 casts and yaw0 faces -Z. All target boxes are source
PlayerBounds width0.6,height1.8, including passives. f32 slab ray/AABB near/far,
parallel abs<1e-6, near=max(near,0), far>=0; reach3 inclusive. Choose nearest by
distance then wire kind(Player1,Hostile2,Passive3) then numeric ID. Select BEFORE
hurt protection check; a protected nearest target blocks farther targets.
DDA uses NativeRaycast with direction normalization f64 hypot chain and f32
inverse, and accepted core::interaction::target_block. A missing original cell
suppresses the intent. First block distance strictly less than target distance
occludes; equality does not. Closed doors block; fluids/open doors do not.
Raw damage swords47/48/49 =>4/5/6, every other held item including broken swords2.
Do not duplicate any shared classifier or domain armor table.

Build all intents and preflight suppression capacity before committing any
cooldown. Then commit decremented cooldowns for each still-present matching live
actor (read latest record and change owned fields only). Each intent reserves
its victim before live validation; another intent to that victim is rejected.
Validate attacker and victim IDs, same frozen dimension and live Active player
lifecycle. A player attack additionally matches selected slot/item/count;
durability intentionally is not frozen identity. Do not recheck live health:
mutual lethal frozen intents both settle before a later death stage. No stale
intent may partially spend a sword, change cooldown or damage a target.

Each successful hit uses one Compound containing latest target/attacker actor,
inventory and runtime edits (omit irrelevant arms). Player targets call
inventory::settle_damage(Melee,raw,frozen armor points,&mut armor), saturating
health, synchronize body armor/health and survival armor points, reset runtime
since_damage_ticks0/eatingNone/bowNone. Hostile targets update body health and
velocity, not runtime cooldown authority. Passive targets update body health,
velocity and aux flee_ticks60/flee_from=frozen attacker position/graze_ticks0.
Knockback adds horizontal0.35 normalized(target frozen position - attacker
frozen position) to current velocity; overlapping XZ uses attacker's yaw look;
vertical velocity is unchanged. Keep all mirrored body/motion fields coherent.

Player attackers: cooldown10, victim player/hostile hurt10, exhaustion100 now,
intact sword durability consumes1 (durability0 or1 becomes corresponding broken
50/51/52 count1 durability0); all other item forms do not wear. Emit CombatHit
with raw damage to ATTACKER only, then suppress mining using existing checked
receipt. Never clear bucket suppression and never enqueue already-settled
DamageIntent. Hostile attacker: cooldown20, player target hurt20, no CombatHit.
Collect damaged player victim keys separately for later sleep integration.

Exhaustion is charged from the latest attacker state, even if it was damaged by
an earlier intent. Add RuleTunables::exhaustion_threshold_milli(self)->u16.
Extract pub(crate) fn exhausted_state(hunger:u8,saturation:u16,exhaustion:u16,
milli:u16,threshold:u16)->(u8,u16,u16) in player_survival.rs. Use u32 total and
threshold.max(1); for each threshold consume1000 saturation, else clear a
positive fractional saturation, else hunger.saturating_sub(1). Return remainder.
Existing private settle_exhaustion wraps it and takes threshold; all existing
calls use current environment tunable. Combat calls the pure helper directly
and synchronizes runtime/body/survival inside its compound; do not note_charge
for a phase that already ran. No unrelated survival timing/death refactor.

## RED/GREEN steps and exact acceptance

1. Write replay tests against missing/inert API before provider body. Verify
   named RED output; zero filtered tests is not evidence.
2. Implement freeze bounded readonly staging, plus private lower-cap tests:
   actor ceiling1 with2 actors, intent ceiling0 with one valid attack both
   refuse without decremented cooldown, inventory, events or suppression edits.
   Tick mismatch and malformed/missing prerequisites preserve snapshot.
3. Geometry tests:3-unit exact reach hit and just-outside miss; nearest kind/ID
   tie; protected nearest suppresses farther; Ready closed wall before target
   blocks, equal distance permits; actual water/open lower+upper pass; unavailable
   traversed chunk suppresses. Both bow forms excluded. No sparse-air shortcut.
4. Hostile choices: target choice differs from nearest and is honored; absent,
   old tick, wrong dimension, out-of-range, dead and hurler excluded; cooldown1
   decrements and permits,2 decrements and refuses. Family ID order wins one
   victim, source hostile-before-player reservation. Health0 cooldown advances.
5. Settlement tests freeze then change selected slot/item/count/dimension or
   lifecycle; each stale hit leaves all hit effects absent. Durability-only
   change uses latest durability. Two facing health2 players both reach0 and
   receive cooldown10; no death/removal or duplicate damage lane entry here.
6. Pin armor raw6 vs full15 points => effective2, each intact armor wears1;
   raw2 at points0 effective2 => no wear; the accepted helper
   lower-bound case raw1 at points2 effective1 also does not wear. Sword1=>broken, non-sword no wear;
   knockback and passive60-flee/graze reset; victim eating/bow interruption;
   attacker-only CombatHit and exact damaged_players victim routing. Two
   attackers to one victim cause one hit/exhaustion/sword wear. Bucket mining
   receipt survives and successful melee suppresses mining.
7. Exhaustion tests:3999+100, saturation500 -> exhaustion99,saturation0,hunger
   unchanged; regen6000 crosses twice at prior3999; custom threshold2000 and
   zero=>1 pinned through real survival/ combat calls, other runtime lanes stay.
8. Run pinned focused hostile_outcomes and player_survival, full server,
   clippy all-targets -D warnings, fmt, diff; make rust before focused Go
   entity Combat|Melee|Armor|Exhaustion source tests (list actual matches).
   Independent review covers source parity, capacity failure and mirror updates.
   Worker scoped English commit, main integrates, updates guide/tasks/ledger and
   reruns integration gates. Refusal/conflict returns evidence to main; no
   invented policy. Main owns rollback and revalidation of affected providers.

## Review focus

Protected closest actors, missing Ready geometry, stale frozen item identity,
mutual lethal attacks, and bounded failure before cooldown are explicitly
covered above. Actual cross-phase runtime preservation and sleep wake-up remain
reducer gates: current motion providers rebuild neutral unowned runtime fields
and must be reconciled there. A passing provider is not integrated combat.
