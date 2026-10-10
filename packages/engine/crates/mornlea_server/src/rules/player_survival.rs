//! Player survival authority: regen, starvation, oxygen, fall and legacy respawn.
//!
//! This provider owns three per-actor calls: `PlayerRegenStarvation` (pre-motion
//! regen and starvation), `PlayerPrePhysicsOxygen` (pre-physics eye-submersion
//! oxygen plus raw held-control staging), and `PlayerPostPhysics`
//! (post-step peak tracking, fall settlement and the confirmed-pose emission).
//! It consumes the accepted motion provider's post-movement state and settles
//! only survival state and damage. Eating settlement, inventory, mining and
//! sleep belong to their own nodes; this provider clears the eating progress
//! marker and bow progress that real damage interrupts and emits the damage
//! observation the sleep node consumes to wake sleepers.
//!
//! Mirrored Go rows, each cited at its site:
//!
//! - `packages/server/sim/entity/health_regen.go` (`advanceHealthRegen`,
//!   `regenHungerThreshold`, `restoreFullHunger`, `resetRegenTimer`): the
//!   since-damage counter, the hunger gate after the increment, the
//!   above-100 modulo-40 heal with its 6000 exhaustion charge, and the
//!   peaceful full-hunger restore after the shared charge.
//! - `packages/server/sim/entity/hunger.go` (`advanceStarvation`,
//!   `applyExhaustion`, `resetHunger`): the starvation timer with its peaceful
//!   skip and non-hard one-health floor, the wide-accumulation threshold loop,
//!   and the fixed respawn hunger values.
//! - `packages/server/sim/entity/oxygen.go` (`advanceOxygen`): immediate
//!   refill out of water, one-per-tick drain while submerged, and the
//!   20-tick damage interval at zero oxygen.
//! - `packages/server/sim/entity/player.go` (`advanceActivePlayers`,
//!   `applyFallDamage`, `applyDamage`, `beginReset`): the sprint gates, the
//!   fall curve through the shared damage entry, the damage side effects, and
//!   the transient half of the reset this provider replays.
//! - `packages/server/sim/entity/death.go` (`settleDeath`): legacy death restores
//!   full health and fixed hunger while keeping the unverified bed record.
//!   Actual source death belongs to the serial consumer after all damage.
//! - `packages/server/sim/entity/placement.go` (`validPlayerInput`,
//!   `validPlayerLook`): the held-input validity this provider reuses to pick
//!   the suppressed control.
//! - `packages/shared/physics/submersion.go`
//!   (`SubmersionFlagsWithTunables`): eye and body immersion derived from the
//!   staged blocks; unobserved cells read as non-fluid, so immersion is never
//!   invented from missing data.
//! - `packages/server/sim/contract/contract.go` (`PlayerUpdate`,
//!   `TickResult`): the confirmed pose carries the tick and the last input
//!   sequence. Environmental damage emits one victim-routed combat hit naming
//!   the existing tick, the applied damage and the player kind: the frozen
//!   `CombatHit` shape carries no target identity to invent, and the sleep
//!   node consumes these observations to wake sleepers (Go emits `CombatHit`
//!   only for melee and projectile hits; the victim-side observation is the
//!   new publication surface the sleep transition reads).
//!
//! Timing and hunger gates consume one immutable `RuleTunables` snapshot.
//! Zero intervals normalize to one at this consumer boundary, matching the
//! source tuning invariant without changing the checked snapshot shape.
//! Jump, swim and sprint charges read the pre-step
//! motion snapshot the context takes at construction (held controls plus
//! pre-step ground, fluid and displacement — no physics re-derivation);
//! waking sleepers is consumed by the sleep node via the damage events below,
//! and this provider never writes sleep state. Legacy death keeps the position
//! and unverified bed; the source consumer owns spatial reset and next-tick
//! captured-scan advancement. The mining lane has no read port, so the
//! emitted pose carries idle mining for the serial reducer to merge.

use mornlea_domain::{
    BlockPos, CombatHit, CombatTarget, Command, Dimension, Event, EventRecipient, FiniteVec3,
    MiningState, MiningStateParts, MotionState, MotionStateParts, PlayerControl, PlayerState,
    PlayerStateParts, SurvivalState, SurvivalStateParts,
};

use crate::core::contracts::{
    ActorAux, ActorBody, ActorKey, ActorLifecycle, ActorRecord, ActorRuntime, BowProgress,
    EatingProgress, PhaseReport, RuleCall, RuleEffect, RulePhase, RuleTunables, ServerError,
    SessionKey,
};
use crate::core::state::{ActionKind, AuthorityReadView, TickContext};

/// Authoritative health maximum (`core.MaxHealth`,
/// `packages/shared/core/health.go`).
const MAX_HEALTH: u8 = 20;
/// Authoritative hunger maximum (`core.MaxHunger`,
/// `packages/shared/core/hunger.go`).
const MAX_HUNGER: u8 = 20;
/// Authoritative oxygen maximum (`core.MaxOxygenTicks`,
/// `packages/shared/core/health.go`).
const MAX_OXYGEN: u16 = 300;
/// Fixed new/respawn saturation (`core.InitialSaturationMilli`,
/// `packages/shared/core/hunger.go`).
const INITIAL_SATURATION_MILLI: u16 = 5_000;
/// Peaceful post-heal saturation (`core.MaxHunger` times
/// `core.SaturationMilliPerPoint`).
const FULL_SATURATION_MILLI: u16 = 20_000;
/// One saturation point in milli units (`core.SaturationMilliPerPoint`).
const SATURATION_MILLI_PER_POINT: u16 = 1_000;
/// Exhaustion charged per healed health (`exhaustionRegenPerHealthMilli`).
const REGEN_EXHAUSTION_MILLI: u16 = 6_000;
/// Exhaustion charged per successful mining completion
/// (`exhaustionMiningMilli`).
const MINING_EXHAUSTION_MILLI: u16 = 5;
/// Exhaustion charged per successful till (`exhaustionTillMilli`).
const TILL_EXHAUSTION_MILLI: u16 = 5;
/// Exhaustion charged per successful melee hit (`exhaustionMeleeMilli`).
const MELEE_EXHAUSTION_MILLI: u16 = 100;
/// Exhaustion charged per real-ground jump (`exhaustionJumpMilli` in
/// `packages/server/sim/entity/hunger.go`).
const JUMP_EXHAUSTION_MILLI: u16 = 50;
/// Exhaustion charged per sprint tick at full acceleration
/// (`exhaustionSprintMilli`).
const SPRINT_EXHAUSTION_MILLI: u16 = 80;
/// Exhaustion per swum block (`exhaustionSwimMilliPerBlock`): the only float
/// in the hunger domain, confined to the displacement conversion below.
const SWIM_MILLI_PER_BLOCK: f64 = 10.0;
/// Hunger below which sprint never triggers, matching the reference behavior
/// (`advanceActivePlayers` in `packages/server/sim/entity/player.go`).
const SPRINT_HUNGER_GATE: u8 = 6;

/// Difficulty wire values (`core.Difficulty` in
/// `packages/shared/core/difficulty.go`): normal is the zero value, peaceful
/// skips starvation and lifts the regen gate, hard removes the one-health
/// floor.
const DIFFICULTY_NORMAL: u8 = 0;
const DIFFICULTY_PEACEFUL: u8 = 1;
const DIFFICULTY_HARD: u8 = 2;

/// Fluid range, zero collision (`WaterSourceID..=WaterLevel7ID`, `core.IsFluid`
/// in `packages/shared/core/fluid.go`).
const FLUID_FIRST: u16 = 27;
const FLUID_LAST: u16 = 34;
/// Player half width (`PlayerWidth / 2`, `packages/shared/physics/types.go`).
const HALF_WIDTH: f32 = 0.3;
/// Player height (`PlayerHeight`).
const PLAYER_HEIGHT: f32 = 1.8;

/// Pitch bound mirror of `validPlayerLook` in
/// `packages/server/sim/entity/placement.go`: `float32(math.Pi/2 - 0.01)`.
const MAX_PITCH: f32 = (std::f64::consts::PI / 2.0 - 0.01) as f32;

/// int32 span of the checked floor/ceil domain, mirroring the Go row
/// (`floored < -1<<31 || floored > 1<<31-1` in
/// `packages/shared/physics/collision.go`).
const COORD_MIN: f64 = i32::MIN as f64;
const COORD_MAX: f64 = i32::MAX as f64;

/// Difficulty the provider settles under, decoded from the staged snapshot.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Difficulty {
    Normal,
    Peaceful,
    Hard,
}

/// Mutable survival lanes one call settles: the wire scalars and the
/// reducer-owned transients. Narrowing the wide runtime lanes back to the
/// domain widths clamps rather than wraps, so a stale lane can never smuggle
/// an out-of-range scalar past the checked constructors below.
struct SurvivalWork {
    health: u8,
    hunger: u8,
    saturation_milli: u16,
    exhaustion_milli: u16,
    oxygen: u16,
    since_damage_ticks: u32,
    drown_ticks: u32,
    starvation_ticks: u32,
    eating: Option<EatingProgress>,
    bow: Option<BowProgress>,
}

/// Settles one per-actor survival call on exactly one of the three owned
/// phases.
///
/// Shape refusals (wrong phase, missing or non-player actor, any command or
/// internal payload, non-active lifecycle) return
/// [`ServerError::InvalidInput`] with nothing staged. A missing environment
/// snapshot or a non-finite peak is a reducer sequencing bug and returns
/// [`ServerError::Internal`], also with nothing staged. An active actor at
/// zero health settles death exactly once before any phase work, so damage
/// observed this tick is never settled twice.
pub fn run(ctx: &mut TickContext<'_>, call: RuleCall<'_>) -> Result<PhaseReport, ServerError> {
    match call.phase {
        RulePhase::PlayerRegenStarvation
        | RulePhase::PlayerPrePhysicsOxygen
        | RulePhase::PlayerPostPhysics => {}
        _ => return Err(ServerError::InvalidInput { field: "phase" }),
    }
    if call.command.is_some() || call.internal.is_some() {
        return Err(ServerError::InvalidInput { field: "phase" });
    }
    let actor = call
        .actor
        .ok_or(ServerError::InvalidInput { field: "actor" })?;
    let ActorKey::Player(session) = actor else {
        return Err(ServerError::InvalidInput { field: "actor" });
    };
    let record = {
        let view = ctx.read();
        let record = view
            .actor(actor)
            .ok_or(ServerError::InvalidInput { field: "actor" })?;
        if record.lifecycle != ActorLifecycle::Active {
            return Err(ServerError::InvalidInput { field: "actor" });
        }
        // Source deaths retain the borrowed pair until the sole late consumer runs.
        if record.survival.health() == 0 && ctx.source_player_death_deferred(session) {
            return Ok(PhaseReport {
                examined: 1,
                applied: 0,
                carried: 0,
                rejected: 0,
            });
        }
        record.clone()
    };
    let environment = ctx
        .read()
        .environment()
        .cloned()
        .ok_or(ServerError::Internal {
            invariant: "player survival snapshot",
        })?;
    let difficulty = match environment.difficulty {
        DIFFICULTY_NORMAL => Difficulty::Normal,
        DIFFICULTY_PEACEFUL => Difficulty::Peaceful,
        DIFFICULTY_HARD => Difficulty::Hard,
        _ => {
            return Err(ServerError::InvalidInput {
                field: "difficulty",
            });
        }
    };
    let runtime = merged_runtime(&ctx.read(), &record)?;
    // Legacy death settles before phase work and its lifecycle flip prevents
    // repetition. Actual source death waits for the serial late consumer.
    if record.survival.health() == 0 {
        return settle_death(ctx, &record, &runtime);
    }
    match call.phase {
        RulePhase::PlayerRegenStarvation => regen_starvation(
            ctx,
            session,
            &record,
            &runtime,
            difficulty,
            environment.tunables,
        ),
        RulePhase::PlayerPrePhysicsOxygen => {
            oxygen(ctx, session, &record, &runtime, environment.tunables)
        }
        RulePhase::PlayerPostPhysics => post_physics(
            ctx,
            session,
            &record,
            &runtime,
            environment.tunables.exhaustion_threshold_milli(),
        ),
        _ => Err(ServerError::InvalidInput { field: "phase" }),
    }
}

/// Runtime lanes with the staged record winning: on the first tick no runtime
/// exists yet, so the transients default from the actor itself instead of
/// refusing a normal sequencing.
pub(crate) fn merged_runtime(
    view: &AuthorityReadView<'_>,
    record: &ActorRecord,
) -> Result<ActorRuntime, ServerError> {
    if let Some(staged) = view.runtime(record.key) {
        return Ok(staged.clone());
    }
    let ActorBody::Player(save) = &record.body else {
        return Err(ServerError::Internal {
            invariant: "player survival body",
        });
    };
    Ok(ActorRuntime {
        key: record.key,
        controls: None,
        has_view: false,
        reset: false,
        attack_cooldown: 0,
        hurt_cooldown: 0,
        burn_cooldown: 0,
        oxygen: record.survival.oxygen(),
        peak_y: record.motion.position().get()[1],
        exhaustion_milli: u32::from(save.exhaustion_milli),
        saturation_milli: u32::from(save.saturation_milli),
        since_damage_ticks: 0,
        drown_ticks: 0,
        starvation_ticks: 0,
        eating: None,
        bow: None,
        path: None,
        aux: ActorAux::Player {
            respawn: bed_record(save),
            workbench: None,
        },
    })
}

/// Unverified bed record carried from the save body: the death settlement
/// keeps it without validating the bed, exactly like `beginReset` leaving the
/// respawn fields alone (`packages/server/sim/entity/player.go`).
/// Clamp-or-drop is explicit: a non-finite or out-of-int32 component drops the
/// record instead of saturating silently through a narrowing cast.
fn bed_record(save: &mornlea_storage::PlayerSave) -> Option<(Dimension, BlockPos)> {
    if !save.respawn_present {
        return None;
    }
    let dimension = Dimension::new(u8::try_from(save.respawn_dimension).ok()?).ok()?;
    let [x, y, z] = save.respawn_position;
    Some((
        dimension,
        BlockPos::new(clamp_block(x)?, clamp_block(y)?, clamp_block(z)?),
    ))
}

/// Explicit block-coordinate clamp-or-drop: the floor must be finite and inside
/// the int32 span before narrowing, so the cast below is exact and never a
/// silent saturation.
fn clamp_block(value: f32) -> Option<i32> {
    let floored = f64::from(value).floor();
    if !floored.is_finite() || !(COORD_MIN..=COORD_MAX).contains(&floored) {
        return None;
    }
    Some(floored as i32)
}

/// Loads the mutable lanes from the staged wire record and the runtime lanes.
/// The runtime owns the transients; the wire record owns health and hunger.
fn load_work(record: &ActorRecord, runtime: &ActorRuntime) -> SurvivalWork {
    SurvivalWork {
        health: record.survival.health(),
        hunger: record.survival.hunger(),
        saturation_milli: runtime.saturation_milli.min(u32::from(u16::MAX)) as u16,
        exhaustion_milli: runtime.exhaustion_milli.min(u32::from(u16::MAX)) as u16,
        oxygen: runtime.oxygen,
        since_damage_ticks: runtime.since_damage_ticks,
        drown_ticks: runtime.drown_ticks,
        starvation_ticks: runtime.starvation_ticks,
        eating: runtime.eating,
        bow: runtime.bow,
    }
}

/// Shared damage entry mirror (`applyDamage` in
/// `packages/server/sim/entity/player.go`): non-positive damage is a no-op so
/// a safe landing never interrupts anything; real damage resets the
/// since-damage counter, clears eating and bow progress, clamps health at zero,
/// and reports the applied damage for the observation below. The sleep node
/// wakes sleepers by consuming the emitted observation.
fn apply_damage(work: &mut SurvivalWork, amount: i32) -> u8 {
    if amount <= 0 {
        return 0;
    }
    work.since_damage_ticks = 0;
    work.eating = None;
    work.bow = None;
    if amount >= i32::from(work.health) {
        let dealt = work.health;
        work.health = 0;
        dealt
    } else {
        work.health -= amount as u8;
        amount as u8
    }
}

/// Damage observation: every real damage emits one victim-routed combat hit
/// naming the existing tick, the applied damage and the player kind. The
/// applied damage never exceeds the pre-damage health, so it always fits the
/// frozen 1..=20 damage range. Tick zero admits no combat-hit publication
/// under the frozen `CombatHit.Validate` rule, so the opening tick stays
/// pose-only while still staging the damage.
fn emit_damage(
    ctx: &mut TickContext<'_>,
    session: SessionKey,
    dealt: u8,
) -> Result<(), ServerError> {
    if dealt == 0 {
        return Ok(());
    }
    let tick = ctx.read().tick();
    if tick == 0 {
        return Ok(());
    }
    let hit = CombatHit::try_new(tick, dealt, CombatTarget::Player).map_err(|_| {
        ServerError::Internal {
            invariant: "player survival damage",
        }
    })?;
    ctx.emit(mornlea_domain::RoutedEvent::new(
        EventRecipient::Session(session.get()),
        Event::CombatHit(hit),
    ))
    .map_err(|_| ServerError::Internal {
        invariant: "player survival emission",
    })?;
    Ok(())
}

/// Action-receipt drain: settles this actor's noted mining/till/melee charges
/// through the shared loop and re-notes every other actor's receipts, so one
/// drain never starves a later per-actor pass in the same tick. Re-noting a
/// drained log only shrinks it below the budget it already fit, so the error
/// arm is unreachable but loud.
fn take_own_charges(
    ctx: &mut TickContext<'_>,
    actor: ActorKey,
) -> Result<Vec<ActionKind>, ServerError> {
    let mut own = Vec::new();
    for (key, kind) in ctx.take_charges() {
        if key == actor {
            own.push(kind);
        } else if ctx.note_charge(key, kind).is_err() {
            return Err(ServerError::Internal {
                invariant: "player survival charges",
            });
        }
    }
    Ok(own)
}

/// Charge amount per receipt kind, from the fixed exhaustion table in
/// `packages/server/sim/entity/hunger.go` (deliberately not tunable: the
/// ratios are the gameplay).
/// The late source-player consumer reuses these exact provider charge units.
pub(crate) fn charge_milli(kind: ActionKind) -> u16 {
    match kind {
        ActionKind::Mining => MINING_EXHAUSTION_MILLI,
        ActionKind::Till => TILL_EXHAUSTION_MILLI,
        ActionKind::Melee => MELEE_EXHAUSTION_MILLI,
    }
}

/// Threshold settlement mirror (`applyExhaustion` in
/// `packages/server/sim/entity/hunger.go`): wide accumulation, then one
/// resource per crossed threshold — full saturation points first, then a
/// partial remainder cleared alone, then hunger. The loop (not a single step)
/// is load-bearing because one regen charge crosses twice.
pub(crate) fn exhausted_state(
    hunger: u8,
    saturation: u16,
    exhaustion: u16,
    milli: u16,
    threshold: u16,
) -> (u8, u16, u16) {
    let threshold = u32::from(threshold.max(1));
    let mut hunger = hunger;
    let mut saturation = saturation;
    let mut total = u32::from(exhaustion) + u32::from(milli);
    while total >= threshold {
        total -= threshold;
        if saturation >= SATURATION_MILLI_PER_POINT {
            saturation -= SATURATION_MILLI_PER_POINT;
        } else if saturation > 0 {
            saturation = 0;
        } else {
            hunger = hunger.saturating_sub(1);
        }
    }
    (hunger, saturation, total as u16)
}

fn settle_exhaustion(work: &mut SurvivalWork, milli: u16, threshold: u16) {
    (work.hunger, work.saturation_milli, work.exhaustion_milli) = exhausted_state(
        work.hunger,
        work.saturation_milli,
        work.exhaustion_milli,
        milli,
        threshold,
    );
}

/// Writes the settled lanes back to the staged actor, body and runtime
/// records. Survival owns the health, hunger, saturation, exhaustion, oxygen
/// and eating lanes, plus bow interruption on damage; every other lane carries
/// forward untouched so the latest-wins overlay never clobbers a sibling
/// provider's record.
fn write_back(
    ctx: &mut TickContext<'_>,
    record: &ActorRecord,
    runtime: &ActorRuntime,
    work: &SurvivalWork,
    controls: Option<PlayerControl>,
    peak_y: f32,
) -> Result<(), ServerError> {
    let ActorBody::Player(save) = &record.body else {
        return Err(ServerError::Internal {
            invariant: "player survival body",
        });
    };
    let survival = SurvivalState::try_new(SurvivalStateParts {
        health: work.health,
        hunger: work.hunger,
        oxygen: work.oxygen,
        saturation_zero: work.saturation_milli == 0,
        armor_points: record.survival.armor_points(),
    })
    .map_err(|_| ServerError::Internal {
        invariant: "player survival staging",
    })?;
    let mut body = save.clone();
    body.health = work.health;
    body.hunger = work.hunger;
    body.saturation_milli = work.saturation_milli;
    body.exhaustion_milli = work.exhaustion_milli;
    let settled = ActorRecord::try_new(
        record.key,
        record.lifecycle,
        record.dimension,
        record.motion,
        record.look,
        survival,
        ActorBody::Player(body),
    )
    .map_err(|_| ServerError::Internal {
        invariant: "player survival staging",
    })?;
    ctx.stage(RuleEffect::Actor(settled))
        .map_err(|_| ServerError::Internal {
            invariant: "player survival staging",
        })?;
    ctx.stage(RuleEffect::Runtime(ActorRuntime {
        oxygen: work.oxygen,
        peak_y,
        exhaustion_milli: u32::from(work.exhaustion_milli),
        saturation_milli: u32::from(work.saturation_milli),
        since_damage_ticks: work.since_damage_ticks,
        drown_ticks: work.drown_ticks,
        starvation_ticks: work.starvation_ticks,
        eating: work.eating,
        bow: work.bow,
        controls,
        ..runtime.clone()
    }))
    .map_err(|_| ServerError::Internal {
        invariant: "player survival staging",
    })?;
    Ok(())
}

/// Death settlement mirror (`settleDeath` in
/// `packages/server/sim/entity/death.go` with `beginReset` in
/// `packages/server/sim/entity/player.go`): full health, fixed hunger,
/// cleared transients and a non-active lifecycle, staged exactly once. The
/// position stays because spatial restore to the anchor or bed needs the
/// pending restore scan; the velocity zeroes like the reference reset, and
/// the bed record carries forward unverified. Drops and crafting recovery
/// belong to their own nodes. Staged silently: the tick's confirmed pose is
/// assembled after settlement, so no half-dead record is ever emitted.
fn settle_death(
    ctx: &mut TickContext<'_>,
    record: &ActorRecord,
    runtime: &ActorRuntime,
) -> Result<PhaseReport, ServerError> {
    let ActorBody::Player(save) = &record.body else {
        return Err(ServerError::Internal {
            invariant: "player survival body",
        });
    };
    // Death resets the hunger state to fixed values, discarding exhaustion
    // accounting with it: pending charge receipts for this actor drop here,
    // mirroring `resetHunger` zeroing the exhaustion lanes.
    take_own_charges(ctx, record.key)?;
    let position = record.motion.position().get();
    let motion = MotionState::new(MotionStateParts {
        position: FiniteVec3::try_new(position).map_err(|_| ServerError::Internal {
            invariant: "player survival staging",
        })?,
        velocity: FiniteVec3::try_new([0.0; 3]).map_err(|_| ServerError::Internal {
            invariant: "player survival staging",
        })?,
        on_ground: record.motion.on_ground(),
    });
    let survival = SurvivalState::try_new(SurvivalStateParts {
        health: MAX_HEALTH,
        hunger: MAX_HUNGER,
        oxygen: MAX_OXYGEN,
        saturation_zero: false,
        armor_points: record.survival.armor_points(),
    })
    .map_err(|_| ServerError::Internal {
        invariant: "player survival staging",
    })?;
    let mut body = save.clone();
    body.health = MAX_HEALTH;
    body.hunger = MAX_HUNGER;
    body.saturation_milli = INITIAL_SATURATION_MILLI;
    body.exhaustion_milli = 0;
    let settled = ActorRecord::try_new(
        record.key,
        ActorLifecycle::Respawning,
        record.dimension,
        motion,
        record.look,
        survival,
        ActorBody::Player(body),
    )
    .map_err(|_| ServerError::Internal {
        invariant: "player survival staging",
    })?;
    ctx.stage(RuleEffect::Actor(settled))
        .map_err(|_| ServerError::Internal {
            invariant: "player survival staging",
        })?;
    ctx.stage(RuleEffect::Runtime(ActorRuntime {
        oxygen: MAX_OXYGEN,
        peak_y: position[1],
        exhaustion_milli: 0,
        saturation_milli: u32::from(INITIAL_SATURATION_MILLI),
        since_damage_ticks: 0,
        drown_ticks: 0,
        starvation_ticks: 0,
        eating: None,
        ..runtime.clone()
    }))
    .map_err(|_| ServerError::Internal {
        invariant: "player survival staging",
    })?;
    Ok(PhaseReport {
        examined: 1,
        applied: 1,
        carried: 0,
        rejected: 0,
    })
}

/// Pre-motion regen and starvation mirror (`advanceHealthRegen` with
/// `regenHungerThreshold` and `restoreFullHunger`, then `advanceStarvation`):
/// the counter increments below max health before the gate, the heal lands
/// above the configured delay on its interval with a 6000 charge shared across
/// difficulties, peaceful restores after the charge, and starvation clears
/// above zero hunger, skips peaceful, freezes non-hard at one health, and
/// damages on the configured interval otherwise.
fn regen_starvation(
    ctx: &mut TickContext<'_>,
    session: SessionKey,
    record: &ActorRecord,
    runtime: &ActorRuntime,
    difficulty: Difficulty,
    tunables: RuleTunables,
) -> Result<PhaseReport, ServerError> {
    let exhaustion_threshold = tunables.exhaustion_threshold_milli();
    let mut work = load_work(record, runtime);
    if work.health < MAX_HEALTH {
        work.since_damage_ticks = work.since_damage_ticks.wrapping_add(1);
        let threshold = match difficulty {
            Difficulty::Peaceful => 0,
            Difficulty::Normal | Difficulty::Hard => tunables.regen_hunger_threshold(),
        };
        if work.hunger >= threshold
            && work.since_damage_ticks > tunables.regen_delay_ticks()
            && (work.since_damage_ticks - tunables.regen_delay_ticks())
                .is_multiple_of(tunables.regen_interval_ticks().max(1))
        {
            work.health += 1;
            settle_exhaustion(&mut work, REGEN_EXHAUSTION_MILLI, exhaustion_threshold);
            if difficulty == Difficulty::Peaceful {
                work.hunger = MAX_HUNGER;
                work.saturation_milli = FULL_SATURATION_MILLI;
            }
        }
    }
    if work.hunger > 0 {
        work.starvation_ticks = 0;
    } else if difficulty != Difficulty::Peaceful {
        // Non-hard keeps the reference floor: at one health the timer freezes
        // instead of advancing, so no banked damage lands after recovery.
        if difficulty != Difficulty::Hard && work.health <= 1 {
            // Timer frozen by leaving it untouched.
        } else {
            work.starvation_ticks = work.starvation_ticks.wrapping_add(1);
            if work.starvation_ticks >= tunables.starvation_interval_ticks().max(1) {
                work.starvation_ticks = 0;
                let dealt = apply_damage(&mut work, 1);
                emit_damage(ctx, session, dealt)?;
            }
        }
    }
    write_back(
        ctx,
        record,
        runtime,
        &work,
        runtime.controls,
        runtime.peak_y,
    )?;
    Ok(PhaseReport {
        examined: 1,
        applied: 1,
        carried: 0,
        rejected: 0,
    })
}

/// Pre-physics eye-submersion mirror (`advanceOxygen` in
/// `packages/server/sim/entity/oxygen.go`, called with the tick-start eye
/// flag in `advanceActivePlayers`): dry eyes refill immediately, wet ticks
/// drain one oxygen, and zero oxygen uses the configured damage interval.
/// Held controls remain raw; hunger and sneak gates belong to local physics.
fn oxygen(
    ctx: &mut TickContext<'_>,
    session: SessionKey,
    record: &ActorRecord,
    runtime: &ActorRuntime,
    tunables: RuleTunables,
) -> Result<PhaseReport, ServerError> {
    let position = record.motion.position().get();
    let eye = BlockPos::new(
        checked_floor(position[0])?,
        checked_floor(position[1] + tunables.eye_height())?,
        checked_floor(position[2])?,
    );
    let eye_in_fluid = is_fluid_at(&ctx.read(), record.dimension, eye);
    let mut work = load_work(record, runtime);
    if !eye_in_fluid {
        work.oxygen = MAX_OXYGEN;
        work.drown_ticks = 0;
    } else if work.oxygen > 0 {
        work.oxygen -= 1;
    } else {
        work.drown_ticks = work.drown_ticks.wrapping_add(1);
        if work.drown_ticks >= tunables.drown_interval_ticks().max(1) {
            work.drown_ticks = 0;
            let dealt = apply_damage(&mut work, 1);
            emit_damage(ctx, session, dealt)?;
        }
    }
    let controls = held_control(ctx, session, runtime.controls);
    write_back(ctx, record, runtime, &work, controls, runtime.peak_y)?;
    Ok(PhaseReport {
        examined: 1,
        applied: 1,
        carried: 0,
        rejected: 0,
    })
}

/// Post-step settlement: motion charges, peak tracking with the landing fall
/// curve, then the confirmed-pose emission.
///
/// Jump, swim and sprint mirror the two motion charge sites in
/// `advanceActivePlayers` against the pre-step snapshot: a real-ground jump
/// (jump held, pre-step grounded and dry, post-step airborne) charges 50 once
/// per takeoff; body-submerged steps charge the exact fixed-point displacement
/// in thousandths, capped at 65535; held sprint with forward intent on dry
/// pre-step ground charges 80. The peak resets on ground or body water and
/// otherwise tracks the maximum, and the landing damage is
/// `max(0, floor(peak - land) - 3)` through the shared damage entry, so armor
/// never reduces it. The pre-step snapshot identifies the landing edge and
/// resets the peak for ground or newly arrived fluid before movement. Direct
/// provider harnesses without a snapshot retain the explicit landing fallback.
///
/// A call that kills stages the zero-health record silently: like
/// `settleDeaths` running before publication, no half-dead pose is emitted;
/// the later settling call stages the respawn. The damage observation still
/// fires on the killing blow. Otherwise, with a staged world record, the call
/// emits one confirmed pose carrying the existing tick and input sequence.
fn post_physics(
    ctx: &mut TickContext<'_>,
    session: SessionKey,
    record: &ActorRecord,
    runtime: &ActorRuntime,
    exhaustion_threshold: u16,
) -> Result<PhaseReport, ServerError> {
    let position = record.motion.position().get();
    if !runtime.peak_y.is_finite() {
        return Err(ServerError::Internal {
            invariant: "player survival peak",
        });
    }
    let body_in_fluid = body_submerged(&ctx.read(), record.dimension, position)?;
    // Validate both prisms before draining receipts or staging effects: an
    // unrepresentable pre-step body must refuse with the same pending work.
    let pre_step = ctx
        .read()
        .pre_step_motion(record.key)
        .map(|pre| {
            let pre_position = pre.position().get();
            body_submerged(&ctx.read(), record.dimension, pre_position)
                .map(|pre_fluid| (pre, pre_position, pre_fluid))
        })
        .transpose()?;
    let mut peak_baseline = runtime.peak_y;
    if let Some((pre, pre_position, pre_fluid)) = pre_step
        && (pre.on_ground() || pre_fluid)
    {
        peak_baseline = pre_position[1];
    }
    if body_in_fluid {
        peak_baseline = position[1];
    }
    let mut work = load_work(record, runtime);
    let held = held_control(ctx, session, runtime.controls);
    // Sprint eligibility belongs to the motion input, before any exhaustion
    // debit can consume the hunger point that admitted this step's sprint.
    let sprinting = held.is_some_and(|control| {
        control.actions().sprinting
            && work.hunger >= SPRINT_HUNGER_GATE
            && !control.actions().sneaking
    });
    // Noted mining/till/melee charges settle before this tick's fall, closest
    // to the reference earn-time settlement a phased tick allows. Charge order
    // is immaterial: the loop result depends only on the summed charges, never
    // their sequence.
    for kind in take_own_charges(ctx, record.key)? {
        settle_exhaustion(&mut work, charge_milli(kind), exhaustion_threshold);
    }
    // Motion charges read the pre-step snapshot the context took at
    // construction: without it (a context built empty) no charge fires, so a
    // missing snapshot never invents exhaustion.
    if let Some((pre, pre_position, pre_fluid)) = pre_step {
        let movement = held.map(|control| control.movement());
        // Source settlement order is jump, swim, then sprint. Jump uses the
        // dry real-ground takeoff edge, not an airborne held jump.
        if movement.is_some_and(|movement| movement.jump)
            && pre.on_ground()
            && !pre_fluid
            && !record.motion.on_ground()
        {
            settle_exhaustion(&mut work, JUMP_EXHAUSTION_MILLI, exhaustion_threshold);
        }
        // Swimming: body-submerged pre-step steps charge the exact
        // fixed-point horizontal displacement, independent of held input;
        // still water naturally converts to zero with no extra branch.
        if pre_fluid {
            let post_position = record.motion.position().get();
            settle_exhaustion(
                &mut work,
                swim_exhaustion_milli(pre_position, post_position),
                exhaustion_threshold,
            );
        }
        if sprinting
            && movement.is_some_and(|movement| movement.move_z > 0)
            && pre.on_ground()
            && !pre_fluid
        {
            settle_exhaustion(&mut work, SPRINT_EXHAUSTION_MILLI, exhaustion_threshold);
        }
    }
    if record.motion.on_ground() && pre_step.is_none_or(|(pre, _, _)| !pre.on_ground()) {
        // The source subtracts float32 positions before flooring. Bound the
        // conversion and subtraction so extreme finite heights stay defined.
        let height = f64::from(peak_baseline - position[1]).floor().max(0.0) as i32;
        let fall = height.saturating_sub(3).max(0);
        let dealt = apply_damage(&mut work, fall);
        emit_damage(ctx, session, dealt)?;
    }
    let peak_y = if record.motion.on_ground() || body_in_fluid {
        position[1]
    } else {
        peak_baseline.max(position[1])
    };
    write_back(ctx, record, runtime, &work, held, peak_y)?;
    // A fresh kill stays silent for the later settling call, exactly like the
    // reference death settlement running before publication.
    if work.health == 0 {
        return Ok(PhaseReport {
            examined: 1,
            applied: 1,
            carried: 0,
            rejected: 0,
        });
    }
    if let Some(world) = ctx.read().world() {
        emit_pose(ctx, session, world)?;
    }
    Ok(PhaseReport {
        examined: 1,
        applied: 1,
        carried: 0,
        rejected: 0,
    })
}

/// Initialized command dispatch retains its canonical controls, including a
/// later placement look, across oxygen and post-physics writeback. Raw direct
/// calls use the latest valid deferred input; an invalid latest clears to
/// neutral, and no incoming input retains the staged basis.
fn held_control(
    ctx: &TickContext<'_>,
    session: SessionKey,
    staged: Option<PlayerControl>,
) -> Option<PlayerControl> {
    let mut latest: Option<mornlea_domain::CommandEnvelope> = None;
    for envelope in ctx.deferred(RulePhase::PlayerMotion) {
        if envelope.session() != session.get() {
            continue;
        }
        match latest {
            Some(current) if current.sequence() >= envelope.sequence() => {}
            _ => latest = Some(envelope),
        }
    }
    match latest {
        Some(envelope) => match envelope.command() {
            Command::PlayerInput(control) if valid_control(control) => {
                if ctx.owns_command_prefix() {
                    staged
                } else {
                    Some(control)
                }
            }
            _ => None,
        },
        None => staged,
    }
}

/// Swim displacement conversion mirror (`swimExhaustionMilli` in
/// `packages/server/sim/entity/hunger.go`): the only float in the hunger
/// domain. Horizontal distance only, truncated down to whole thousandths
/// (never rounded, so resting jitter accrues nothing), capped at the `uint16`
/// ceiling instead of wrapping. Zero and NaN both convert to zero: NaN needs
/// its own arm because every NaN comparison is false, exactly like the
/// reference guard the two arms spell out.
fn swim_exhaustion_milli(before: [f32; 3], after: [f32; 3]) -> u16 {
    let dx = f64::from(after[0]) - f64::from(before[0]);
    let dz = f64::from(after[2]) - f64::from(before[2]);
    let distance = (dx * dx + dz * dz).sqrt();
    if distance.is_nan() || distance <= 0.0 {
        return 0;
    }
    let scaled = distance * 1000.0 * SWIM_MILLI_PER_BLOCK;
    if scaled >= f64::from(u16::MAX) * 1000.0 {
        return u16::MAX;
    }
    (scaled as i64 / 1000) as u16
}

/// Latest input sequence for pose attribution: the maximum deferred sequence
/// for the session, recorded before validation exactly like
/// `lastInputSequence` in `ApplyPlayerCommands`
/// (`packages/server/sim/entity/tick.go`).
fn latest_sequence(ctx: &TickContext<'_>, session: SessionKey) -> u64 {
    let mut sequence = 0;
    for envelope in ctx.deferred(RulePhase::PlayerMotion) {
        if envelope.session() == session.get() {
            sequence = sequence.max(envelope.sequence());
        }
    }
    sequence
}

/// Confirmed-pose emission: one `PlayerState` routed to the session, carrying
/// the existing tick and input sequence over the just-staged actor. Mining
/// stays idle because the mining lane has no read port yet; the serial
/// reducer merges the real record.
fn emit_pose(
    ctx: &mut TickContext<'_>,
    session: SessionKey,
    world: mornlea_domain::WorldState,
) -> Result<(), ServerError> {
    let tick = ctx.read().tick();
    let sequence = latest_sequence(ctx, session);
    let record =
        ctx.read()
            .actor(ActorKey::Player(session))
            .cloned()
            .ok_or(ServerError::Internal {
                invariant: "player survival pose",
            })?;
    let mining = MiningState::try_new(MiningStateParts {
        active: false,
        target: BlockPos::ORIGIN,
        progress: 0,
        required: 0,
        harvestable: false,
    })
    .map_err(|_| ServerError::Internal {
        invariant: "player survival pose",
    })?;
    let pose = PlayerState::new(PlayerStateParts {
        server_tick: tick,
        last_input_sequence: sequence,
        dimension: record.dimension,
        motion: record.motion,
        look: record.look,
        ready: true,
        reset: ctx
            .read()
            .runtime(record.key)
            .map(|runtime| runtime.reset)
            .unwrap_or(false),
        mining,
        survival: record.survival,
        world,
    });
    ctx.emit(mornlea_domain::RoutedEvent::new(
        EventRecipient::Session(session.get()),
        Event::PlayerState(pose),
    ))
    .map_err(|_| ServerError::Internal {
        invariant: "player survival emission",
    })?;
    Ok(())
}

/// Authority input rule mirror (`validPlayerInput` and `validPlayerLook` in
/// `packages/server/sim/entity/placement.go`): move axes in -1..=1 with a
/// finite yaw and a pitch inside +/-(pi/2 - 0.01). Yaw needs no range gate:
/// any finite rotation normalizes into [-pi, pi).
fn valid_control(control: PlayerControl) -> bool {
    let movement = control.movement();
    (-1..=1).contains(&movement.move_x)
        && (-1..=1).contains(&movement.move_z)
        && control.look().yaw().is_finite()
        && control.look().pitch().is_finite()
        && control.look().pitch() >= -MAX_PITCH
        && control.look().pitch() <= MAX_PITCH
}

/// Fluid view mirror (`IsFluidAt` on the authority dimension): staged and
/// fluid, never invented from an unobserved cell.
fn is_fluid_at(view: &AuthorityReadView<'_>, dimension: Dimension, pos: BlockPos) -> bool {
    view.observation(dimension, pos)
        .is_some_and(|observed| (FLUID_FIRST..=FLUID_LAST).contains(&observed.block))
}

/// Body immersion mirror (`SubmersionFlagsWithTunables` in
/// `packages/shared/physics/submersion.go`): the feet-centered box over staged
/// blocks. Unobserved cells read as non-fluid, matching the motion provider.
fn body_submerged(
    view: &AuthorityReadView<'_>,
    dimension: Dimension,
    position: [f32; 3],
) -> Result<bool, ServerError> {
    let minimum = [
        checked_floor(position[0] - HALF_WIDTH)?,
        checked_floor(position[1])?,
        checked_floor(position[2] - HALF_WIDTH)?,
    ];
    let maximum = [
        fluid_upper(position[0] + HALF_WIDTH, minimum[0])?,
        fluid_upper(position[1] + PLAYER_HEIGHT, minimum[1])?,
        fluid_upper(position[2] + HALF_WIDTH, minimum[2])?,
    ];
    for y in minimum[1]..=maximum[1] {
        for x in minimum[0]..=maximum[0] {
            for z in minimum[2]..=maximum[2] {
                if is_fluid_at(view, dimension, BlockPos::new(x, y, z)) {
                    return Ok(true);
                }
            }
        }
    }
    Ok(false)
}

/// AABB upper-cell mirror (`fluidCellUpperBound`): `ceil - 1` so a box merely
/// touching a cell boundary does not claim the neighbor, clamped to scan at
/// least one cell.
fn fluid_upper(maximum: f32, lower: i32) -> Result<i32, ServerError> {
    Ok(checked_ceil(maximum)?
        .checked_sub(1)
        .ok_or(ServerError::InvalidInput { field: "actor" })?
        .max(lower))
}

/// Checked floor mirror (`collisionCheckedFloor` in
/// `packages/shared/physics/collision.go`): outside int32 the cell is
/// unrepresentable and the call refuses without effect.
fn checked_floor(value: f32) -> Result<i32, ServerError> {
    let floored = f64::from(value).floor();
    if !floored.is_finite() || !(COORD_MIN..=COORD_MAX).contains(&floored) {
        return Err(ServerError::InvalidInput { field: "actor" });
    }
    Ok(floored as i32)
}

/// Checked ceil mirror (`collisionCheckedCeil`), same refusal policy.
fn checked_ceil(value: f32) -> Result<i32, ServerError> {
    let ceiling = f64::from(value).ceil();
    if !ceiling.is_finite() || !(COORD_MIN..=COORD_MAX).contains(&ceiling) {
        return Err(ServerError::InvalidInput { field: "actor" });
    }
    Ok(ceiling as i32)
}
