//! Serial authoritative tick reducer.
//!
//! One tick owns one ordered dispatch. The endpoint claims the tick, then
//! this reducer drains the mailbox, feeds every accepted provider in the
//! frozen Go order, commits the viewer overlay with retired-session pruning
//! and publishes exactly once. The replay order suite pins the dispatch call
//! sequence per function below, so a swapped row fails loudly; keep provider
//! call sites spelled as plain provider paths.
//!
//! The context holds the only mutable authority borrow for the whole
//! dispatch, so authority writes happen exclusively before construction
//! (mailbox drain, companion feed) and after the context drops (viewer
//! commit, publication delivery). A mid-tick viewer commit would be
//! observationally void: providers read the staged overlay, and the closing
//! commit replaces the full set. A batch failure stops later rows but never
//! the tick itself: staged work commits, the publication reports what ran,
//! and the mailbox accounting already holds.

use std::collections::BTreeSet;

use mornlea_domain::{
    BlockPos, ChunkPos, CommandEnvelope, CommandOrderScratch, ContainerRef, Dimension,
    order_commands,
};

use super::contracts::{
    ActorKey, ActorLifecycle, AuthorityInteraction, ChunkKey, ContainerSlots, InteractionKind,
    PhaseReport, RuleCall, RulePhase, ServerError, SessionKey, SessionPhase, TickBudget,
    TickCounters, TickPublication,
};
use super::publication;
use super::state::{AuthorityState, TickContext};
use crate::rules::{
    companions, containers, crafting, crops, drops, eating, environment, farmland, fluids,
    furnaces, hostile_actions, hostile_actors, hostile_outcomes, inventory, mining, passives,
    player_motion, player_survival, projectiles, random_blocks, sleep, supports, tools,
    world_acquisition, world_mutation,
};

/// Companion intake drained per tick. The ingress ceiling owns the bound;
/// this names the reducer's share of it.
const COMPANION_FEED: usize = 4;
/// Chunk-column radius around each active player for the shared active-key
/// set. Eight players cap the set at two hundred columns structurally.
const ACTIVE_KEY_RADIUS: i32 = 2;
/// Projectile scope radius per active player. The frozen contract fixes the
/// radius-two columns as the shared observation set; each flight square uses
/// the same bound until a session subscription radius lands in the contract.
const PROJECTILE_SCOPE_RADIUS: u64 = 2;
/// Scope ceiling mirroring the eight-player structural bound.
const MAX_SCOPES: usize = 8;

/// Six face neighbors in kernel slot order, mirroring the source fluid
/// neighbor table (`fluidNeighbors` in `packages/server/fluid/queue.go`).
const SIX_NEIGHBORS: [(i32, i32, i32); 6] = [
    (1, 0, 0),
    (-1, 0, 0),
    (0, 1, 0),
    (0, -1, 0),
    (0, 0, 1),
    (0, 0, -1),
];

/// One frozen provider call: the context plus its exact call record.
type ProviderCall = fn(&mut TickContext<'_>, RuleCall<'_>) -> Result<PhaseReport, ServerError>;

/// Water source through level seven, mirrored beside the farmland
/// provider's own range.
fn is_fluid(block: u16) -> bool {
    (27..=34).contains(&block)
}

/// Dry through wet farmland, mirrored beside the farmland provider's own
/// range.
fn is_farmland(block: u16) -> bool {
    (35..=36).contains(&block)
}

/// Feeds the carried fluid update queue from staged block writes, mirroring
/// the source unified enqueue facade (`EnqueueBlockWrite` in
/// `packages/server/sim/realm/environment.go`): every changed cell plus its
/// six neighbors queues due now, so the update phase below evaluates this
/// tick's dirt alongside the carried queue. Due now rather than now plus the
/// flow delay because only carried dues survive the tick: requeues the update
/// stages at now plus the flow delay wait out their ticks on the authority.
pub fn feed_fluid_schedule(
    schedule: &mut fluids::FluidSchedule,
    context: &TickContext<'_>,
    tick: u64,
) {
    for observed in context.changed_blocks() {
        schedule.enqueue_fluid(observed.key, observed.pos, tick);
        for (dx, dy, dz) in SIX_NEIGHBORS {
            let (Some(x), Some(y), Some(z)) = (
                observed.pos.x().checked_add(dx),
                observed.pos.y().checked_add(dy),
                observed.pos.z().checked_add(dz),
            ) else {
                continue;
            };
            schedule.enqueue_fluid(
                ChunkKey {
                    dimension: observed.key.dimension,
                    pos: ChunkPos::new(x >> 4, z >> 4),
                },
                BlockPos::new(x, y, z),
                tick,
            );
        }
    }
}

/// Feeds the carried farmland candidate queue from staged block writes,
/// following the moisture half of the same source facade: a fresh farmland
/// cell queues one candidate, a fluid cell wakes its hydration window, both
/// due now. The facade keys both arms on the (old, new) pair, but the staged
/// overlay carries only the new value, so the arms fire on the new value
/// instead. Over-enqueueing is safe: inspection is exact and every charge
/// stays inside the frozen budgets.
pub fn feed_farmland_schedule(
    schedule: &mut farmland::FarmlandSchedule,
    context: &TickContext<'_>,
    tick: u64,
) {
    for observed in context.changed_blocks() {
        if is_farmland(observed.block) {
            schedule.enqueue_candidate(observed.key, observed.pos, tick);
        }
        if is_fluid(observed.block) {
            schedule.enqueue_candidate_window(observed.key, observed.pos, tick);
        }
    }
}

/// Reduces one claimed tick into its owned publication.
///
/// The endpoint already validated the phase and budget shape; it bumps the
/// tick counter after this returns, so the executing tick is the current
/// counter. A direct call without the endpoint claim repeats the current
/// tick; the production path always goes through the endpoint delegation.
pub fn reduce_tick(state: &mut AuthorityState, budget: TickBudget) -> TickPublication {
    let tick = state.next_tick();
    let drained = drain_mailbox(state, tick, budget.commands());
    let companions = state.drain_companions(COMPANION_FEED);
    // The carried schedules leave authority ownership for the tick: the
    // context holds the only mutable authority borrow during dispatch, so
    // they travel as locals and return after the context drops.
    let mut fluid_schedule =
        std::mem::replace(state.fluid_schedule_mut(), fluids::FluidSchedule::new());
    let mut farmland_schedule = std::mem::replace(
        state.farmland_schedule_mut(),
        farmland::FarmlandSchedule::new(),
    );
    let mut context = TickContext::for_tick(state, budget);
    context.freeze_environment(tick);
    for action in companions {
        context.push_companion_action(action);
    }
    let _ = dispatch_rows(
        &mut context,
        tick,
        &drained.dispatched,
        &mut fluid_schedule,
        &mut farmland_schedule,
    );
    let overlay = context.viewer_leases();
    let events = context.events().to_vec();
    let counters = TickCounters {
        executed_tick: tick,
        commands: drained.commands,
        fluid_by_dimension: dimension_counters(&context, |context, dimension| {
            context.spent_fluid(dimension)
        }),
        rescan_by_dimension: dimension_counters(&context, |context, dimension| {
            context.spent_rescan(dimension)
        }),
        farmland_checks: context.spent_farmland_checks(),
        farmland_reads: context.spent_farmland_reads(),
        carried: drained.carried,
        stale: drained.stale,
    };
    drop(context);
    *state.fluid_schedule_mut() = fluid_schedule;
    *state.farmland_schedule_mut() = farmland_schedule;
    let mut overlay = overlay;
    overlay.retain(
        |key, _| matches!(state.session(*key), Some(facts) if facts.phase == SessionPhase::Active),
    );
    state.commit_viewers(overlay);
    let publication = TickPublication {
        tick,
        events,
        // Control stays empty: handshake replies are transport-owned and
        // never synthesized by the tick.
        control: Vec::new(),
        counters,
    };
    // Delivery never fails the tick: a stale recipient drops its frames
    // inside the port while the returned publication keeps the events.
    let _ = publication::publish_tick(state, publication.clone());
    publication
}

/// Mailbox-exact drain counters. Every frozen envelope is counted exactly
/// once across the three counters: dispatched into the rows, stale by
/// watermark or retirement, or carried back by the budget split.
struct MailboxDrain {
    dispatched: Vec<CommandEnvelope>,
    commands: usize,
    carried: usize,
    stale: usize,
}

/// Freezes, orders and budget-splits one tick's sequenced batch, then drops
/// retired sessions before any provider dispatch.
///
/// The composition mirrors the mailbox port piece for piece (eligible
/// freeze, domain sort with reusable scratch, budgeted prefix with the
/// remainder carried back, per-session watermark walk) with the retired
/// filter folded into the walk: a retired session never executes and never
/// returns, so its commands count stale instead of riding back. A refused
/// sort carries the whole frozen batch back and dispatches nothing.
fn drain_mailbox(state: &mut AuthorityState, tick: u64, command_budget: usize) -> MailboxDrain {
    let mut batch = state.freeze_eligible(tick);
    let commands = batch.len();
    let mut scratch = match CommandOrderScratch::try_with_capacity(batch.len()) {
        Ok(scratch) => scratch,
        Err(_) => {
            let _ = state.carry(batch);
            return MailboxDrain {
                dispatched: Vec::new(),
                commands,
                carried: commands,
                stale: 0,
            };
        }
    };
    if order_commands(&mut batch, &mut scratch).is_err() {
        let _ = state.carry(batch);
        return MailboxDrain {
            dispatched: Vec::new(),
            commands,
            carried: commands,
            stale: 0,
        };
    }
    let suffix = batch.split_off(command_budget.min(batch.len()));
    let carried = suffix.len();
    let _ = state.carry(suffix);
    let mut dispatched = Vec::with_capacity(batch.len());
    let mut stale = 0;
    for envelope in batch {
        let Some(key) = SessionKey::from_raw(envelope.session()) else {
            stale += 1;
            continue;
        };
        if !state.apply_sequence(key, envelope.sequence()) {
            stale += 1;
            continue;
        }
        if !matches!(state.session(key), Some(facts) if facts.phase == SessionPhase::Active) {
            stale += 1;
            continue;
        }
        dispatched.push(envelope);
    }
    MailboxDrain {
        dispatched,
        commands,
        carried,
        stale,
    }
}

/// Runs every dispatch row in the frozen order. The first error stops later
/// rows; the caller still commits and publishes. The carried schedules
/// arrive as locals because the context owns the authority borrow.
#[allow(clippy::too_many_lines)]
fn dispatch_rows(
    context: &mut TickContext<'_>,
    tick: u64,
    dispatched: &[CommandEnvelope],
    fluid_schedule: &mut fluids::FluidSchedule,
    farmland_schedule: &mut farmland::FarmlandSchedule,
) -> Result<(), ServerError> {
    for envelope in dispatched {
        admit_command(context, envelope)?;
    }
    // Bed-kind internal entries are collected here for the row loop below:
    // intake owns validation and queueing while execution waits for its
    // phase, the two-phase shape Go pins (`ApplyPlayerCommands` validates
    // and queues bed commands, `SettleGameplay` executes them,
    // `packages/server/sim/entity/tick.go`).
    let mut bed_entries: Vec<AuthorityInteraction> = context
        .read()
        .interactions()
        .iter()
        .copied()
        .filter(|interaction| interaction.kind == InteractionKind::Bed)
        .collect();
    companions::run(context, batch_call(RulePhase::CompanionIntent))?;
    world_acquisition::run(context, batch_call(RulePhase::Acquire))?;
    for session in active_players(context) {
        let actor = ActorKey::Player(session);
        per_actor(
            context,
            RulePhase::PlayerRegenStarvation,
            actor,
            player_survival::run,
        )?;
        per_actor(context, RulePhase::Eating, actor, eating::run)?;
        per_actor(context, RulePhase::BowDraw, actor, projectiles::run)?;
        per_actor(
            context,
            RulePhase::PlayerPrePhysicsOxygen,
            actor,
            player_survival::run,
        )?;
        per_actor(context, RulePhase::PlayerMotion, actor, player_motion::run)?;
        per_actor(
            context,
            RulePhase::PlayerPostPhysics,
            actor,
            player_survival::run,
        )?;
    }
    companions::run(context, batch_call(RulePhase::CompanionMotion))?;
    let plan = hostile_actions::plan(context)?;
    let melee = plan.melee_batch().clone();
    hostile_actions::apply(context, plan)?;
    hostile_actors::run(context, batch_call(RulePhase::HostileMotion))?;
    let combat = hostile_outcomes::advance(context, &melee)?;
    hostile_actors::run(context, batch_call(RulePhase::HostileBurnDistant))?;
    let scopes = projectile_scopes(context);
    let flight = projectiles::advance(context, &scopes)?;
    hostile_outcomes::run(context, batch_call(RulePhase::HostilePlayerDeaths))?;
    let mut victims: Vec<SessionKey> = combat
        .damaged_players
        .iter()
        .chain(flight.damaged_players.iter())
        .copied()
        .collect();
    victims.sort_unstable();
    victims.dedup();
    passives::run(context, batch_call(RulePhase::PassiveStepDeaths))?;
    companions::run(context, batch_call(RulePhase::CompanionPlacement))?;
    for envelope in context.deferred(RulePhase::Interaction) {
        route_interaction(context, &envelope);
    }
    for interaction in context.read().interactions().to_vec() {
        if interaction.kind != InteractionKind::Door {
            continue;
        }
        route_door(context, &interaction)?;
    }
    // Bed entries execute here in sequence order, threading the record the
    // settlement batch below consumes. This is the refined table's
    // deferred-loop placement and Go's `SettleGameplay` execution half; the
    // intake collection above is the `ApplyPlayerCommands` half. Ordinary
    // refusals (daytime beds, silent geometry) skip without stopping the
    // row; any other failure stops the tick.
    bed_entries.sort_by_key(|interaction| interaction.sequence);
    for interaction in &bed_entries {
        let record = context.sleep_record().clone();
        match sleep::enter(context, &record, interaction) {
            Ok((report, entered)) => {
                context.set_sleep_record(entered);
                if report.applied == 1 {
                    // A recorded anchor means the session fell asleep: entry
                    // is the only grow path for the reducer-owned sleeping
                    // set.
                    let mut sleeping = context.sleeping();
                    if !sleeping.contains(&interaction.session) {
                        sleeping.push(interaction.session);
                        context.set_sleeping(sleeping);
                    }
                }
            }
            Err(ServerError::InvalidInput { .. }) => {}
            Err(error) => return Err(error),
        }
    }
    let record = context.sleep_record().clone();
    let sleeping = context.sleeping();
    let active = active_players(context);
    let settled = sleep::settle(context, &record, &sleeping, &active, &victims)?;
    context.set_sleep_record(settled.record);
    context.set_sleeping(settled.sleeping);
    drops::run(context, batch_call(RulePhase::DropStep))?;
    let active_keys = active_keys(context);
    drops::advance(context, &active_keys)?;
    furnaces::run(context, batch_call(RulePhase::FurnaceStep))?;
    let furnace_interest = furnace_interest(context, &active_keys);
    furnaces::advance(context, &furnace_interest)?;
    fluids::run(context, batch_call(RulePhase::FluidRescan))?;
    // Rescan reads the carried cursors: sections the scope dropped leave,
    // unstarted ones carry with their cursors for the next tick.
    let mut scope: BTreeSet<ChunkKey> = active_keys.iter().copied().collect();
    for observed in context.changed_blocks() {
        scope.insert(observed.key);
    }
    let fluid_delay = context
        .read()
        .environment()
        .ok_or(ServerError::Internal {
            invariant: "tick environment snapshot",
        })?
        .tunables
        .fluid_delay();
    fluids::rescan(fluid_schedule, context, &scope, tick, fluid_delay)?;
    fluids::run(context, batch_call(RulePhase::FluidUpdate))?;
    feed_fluid_schedule(fluid_schedule, context, tick);
    fluids::update(fluid_schedule, context, tick, fluid_delay)?;
    farmland::run(context, batch_call(RulePhase::Farmland))?;
    feed_farmland_schedule(farmland_schedule, context, tick);
    farmland::advance(farmland_schedule, context, &scope, tick)?;
    crops::run(context, batch_call(RulePhase::Trample))?;
    // Footprints stay fresh: the batch collects landing edges from the
    // staged players every call and never carries, so changed blocks are
    // not an input here.
    let mut footprints = crops::FootprintSchedule::new();
    crops::settle_tramples(&mut footprints, context)?;
    crops::run(context, batch_call(RulePhase::SnowFootprint))?;
    crops::settle_snow_footprints(&mut footprints, context)?;
    random_blocks::run(context, batch_call(RulePhase::RandomBlock))?;
    random_blocks::advance(context, &active_keys)?;
    containers::run(context, batch_call(RulePhase::ContainerMove))?;
    for actor in mining_actors(context) {
        per_actor(context, RulePhase::MiningStep, actor, mining::run)?;
    }
    crafting::run(context, batch_call(RulePhase::WorkbenchLifecycle))?;
    supports::run(context, batch_call(RulePhase::Support))?;
    environment::run(context, batch_call(RulePhase::EnvironmentEnd))?;
    Ok(())
}

/// One shape-gated batch call: no actor, command or internal payload.
fn batch_call(phase: RulePhase) -> RuleCall<'static> {
    RuleCall {
        phase,
        actor: None,
        command: None,
        internal: None,
    }
}

/// Admits one sorted envelope into the intake providers. Every admit whose
/// gate accepts the envelope runs: an open lands in both the container and
/// the workbench bags. Resource and sequencing failures stop the tick; gate
/// refusals fall through. An envelope no intake owns rides the ordered
/// interaction bag the combat-close loop drains.
fn admit_command(
    context: &mut TickContext<'_>,
    envelope: &CommandEnvelope,
) -> Result<(), ServerError> {
    let call = RuleCall {
        phase: RulePhase::PlayerCommand,
        actor: None,
        command: Some(envelope),
        internal: None,
    };
    let admits: [ProviderCall; 4] = [
        player_motion::run,
        inventory::run,
        containers::run,
        crafting::run,
    ];
    let mut admitted = false;
    for admit in admits {
        match admit(context, call) {
            Ok(_) => admitted = true,
            Err(error @ (ServerError::Capacity { .. } | ServerError::Internal { .. })) => {
                return Err(error);
            }
            Err(_) => {}
        }
    }
    if !admitted {
        context.defer(*envelope, RulePhase::Interaction)?;
    }
    Ok(())
}

/// Runs one per-actor call. Death or respawn mid-row retires the actor for
/// later phases; that refusal is ordinary and never stops the row. Any other
/// failure stops the tick.
fn per_actor(
    context: &mut TickContext<'_>,
    phase: RulePhase,
    actor: ActorKey,
    settle: ProviderCall,
) -> Result<(), ServerError> {
    let call = RuleCall {
        phase,
        actor: Some(actor),
        command: None,
        internal: None,
    };
    match settle(context, call) {
        Ok(_) => Ok(()),
        Err(ServerError::InvalidInput { .. }) => Ok(()),
        Err(error) => Err(error),
    }
}

/// Routes one deferred envelope to the single provider whose gate accepts
/// it: placement and door geometry, tools and buckets, or panel drops.
/// Refused everywhere, the envelope has no owner and drops; admission
/// already bounded the bag.
fn route_interaction(context: &mut TickContext<'_>, envelope: &CommandEnvelope) {
    let call = RuleCall {
        phase: RulePhase::Interaction,
        actor: None,
        command: Some(envelope),
        internal: None,
    };
    if world_mutation::run(context, call).is_ok() {
        return;
    }
    if tools::run(context, call).is_ok() {
        return;
    }
    let _ = drops::run(context, call);
}

/// Settles one authority-owned door toggle through placement geometry. A
/// silent no-op skips without stopping the loop; any other failure stops
/// the tick.
fn route_door(
    context: &mut TickContext<'_>,
    interaction: &AuthorityInteraction,
) -> Result<(), ServerError> {
    let call = RuleCall {
        phase: RulePhase::Interaction,
        actor: None,
        command: None,
        internal: Some(interaction),
    };
    match world_mutation::run(context, call) {
        Ok(_) => Ok(()),
        Err(ServerError::InvalidInput { .. }) => Ok(()),
        Err(error) => Err(error),
    }
}

/// Sorted active player sessions: the per-actor line, the sleep roster and
/// every interest derivation share this single ascending source.
fn active_players(context: &TickContext<'_>) -> Vec<SessionKey> {
    let mut players = Vec::new();
    for actor in context.read().actors() {
        let ActorKey::Player(session) = actor.key else {
            continue;
        };
        if actor.lifecycle == ActorLifecycle::Active {
            players.push(session);
        }
    }
    players.sort_unstable();
    players
}

/// Human and companion actors in key order for the mining pass. Any other
/// kind refuses inside the provider.
fn mining_actors(context: &TickContext<'_>) -> Vec<ActorKey> {
    let mut actors = Vec::new();
    for record in context.read().actors() {
        if matches!(record.key, ActorKey::Player(_) | ActorKey::Companion(_)) {
            actors.push(record.key);
        }
    }
    actors.sort_unstable();
    actors
}

/// Radius-two chunk columns around every active player in its dimension,
/// sorted and deduped. Eight players cap the set at two hundred columns
/// structurally, so the providers' refusal bound never fires here.
fn active_keys(context: &TickContext<'_>) -> Vec<ChunkKey> {
    let mut keys = BTreeSet::new();
    for actor in context.read().actors() {
        let ActorKey::Player(_) = actor.key else {
            continue;
        };
        if actor.lifecycle != ActorLifecycle::Active {
            continue;
        }
        let position = actor.motion.position().get();
        let center_x = (position[0].floor() as i32) >> 4;
        let center_z = (position[2].floor() as i32) >> 4;
        for dz in -ACTIVE_KEY_RADIUS..=ACTIVE_KEY_RADIUS {
            for dx in -ACTIVE_KEY_RADIUS..=ACTIVE_KEY_RADIUS {
                keys.insert(ChunkKey {
                    dimension: actor.dimension,
                    pos: ChunkPos::new(center_x.saturating_add(dx), center_z.saturating_add(dz)),
                });
            }
        }
    }
    keys.into_iter().collect()
}

/// One Ready flight square per active player sharing the active-set radius.
/// Truncated at the scope ceiling the eight-player bound never reaches.
fn projectile_scopes(context: &TickContext<'_>) -> Vec<projectiles::ProjectileScope> {
    let mut scopes = Vec::new();
    for session in active_players(context) {
        let Some(actor) = context.read().actor(ActorKey::Player(session)) else {
            continue;
        };
        let position = actor.motion.position().get();
        scopes.push(projectiles::ProjectileScope {
            dimension: actor.dimension,
            center: ChunkPos::new(
                (position[0].floor() as i32) >> 4,
                (position[2].floor() as i32) >> 4,
            ),
            radius: PROJECTILE_SCOPE_RADIUS,
        });
    }
    scopes.truncate(MAX_SCOPES);
    scopes
}

/// Container references under the active keys whose staged record is a
/// furnace, deduped. Chest references never enter the interest set: the
/// furnace batch refuses a chest as a sequencing bug.
fn furnace_interest(context: &TickContext<'_>, active: &[ChunkKey]) -> Vec<ContainerRef> {
    let view = context.read();
    let mut refs = BTreeSet::new();
    for key in active {
        for reference in view.container_refs(*key) {
            let is_furnace = view
                .container(reference)
                .is_some_and(|record| matches!(record.slots, ContainerSlots::Furnace { .. }));
            if is_furnace {
                refs.insert(reference);
            }
        }
    }
    refs.into_iter().collect()
}

/// Per-dimension attribution of one spent lane. Only dimensions with charged
/// work appear; an idle tick reports neither.
fn dimension_counters(
    context: &TickContext<'_>,
    spent: impl Fn(&TickContext<'_>, Dimension) -> usize,
) -> Vec<(Dimension, usize)> {
    let mut counters = Vec::new();
    for dimension in [Dimension::OVERWORLD, Dimension::DEPTHS] {
        let used = spent(context, dimension);
        if used > 0 {
            counters.push((dimension, used));
        }
    }
    counters
}
