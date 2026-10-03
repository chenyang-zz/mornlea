//! Serial authoritative tick reducer.
//!
//! One tick owns one ordered dispatch. The endpoint claims the tick, then
//! this reducer drains the mailbox, seeds residents and logins into the
//! overlay, feeds every accepted provider in the frozen Go order, commits
//! the viewer and resident overlays with retired-session pruning on the
//! viewer leg, and delivers successful normal ticks once. Final execution
//! uses the same engine without delivery. The replay order suite pins the
//! dispatch call sequence per function below, so a swapped row fails loudly;
//! keep provider call sites spelled as plain provider paths.
//!
//! The context holds the only mutable authority borrow for the whole
//! dispatch, so authority writes happen exclusively before construction
//! (mailbox drain, companion feed, login scan) and after the context drops
//! (viewer and resident commits, publication delivery). The context also owns
//! input-acknowledgment bookkeeping before semantic control validation.
//! A mid-tick viewer commit would be
//! observationally void: providers read the staged overlay, and the closing
//! commit replaces the full set. A hard failure fences the authority and
//! stops successful advancement. Accepted partial writes retain ownership
//! without claiming whole-tick rollback or a fresh durable capture.

use std::collections::BTreeSet;
use std::panic::{AssertUnwindSafe, catch_unwind};

use mornlea_domain::{
    BlockPos, ChunkPos, CommandEnvelope, CommandOrderScratch, ContainerRef, Dimension,
    order_commands,
};

use super::contracts::{
    ActorKey, ActorLifecycle, AuthorityInteraction, ChunkKey, ContainerSlots, FinalReducer,
    InteractionKind, PhaseReport, RuleCall, RulePhase, ServerError, ServerPhase, SessionKey,
    SessionPhase, TickBudget, TickCounters, TickPublication,
};
use super::publication;
use super::source_player_restore::{self, SourcePlayerBook};
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

#[cfg(test)]
type DispatchHook = fn(&mut TickContext<'_>) -> Result<(), ServerError>;

#[cfg(test)]
thread_local! {
    static DISPATCH_HOOK: std::cell::Cell<Option<DispatchHook>> = const { std::cell::Cell::new(None) };
}

#[cfg(test)]
pub(crate) fn set_dispatch_hook(hook: Option<DispatchHook>) {
    DISPATCH_HOOK.with(|slot| slot.set(hook));
}

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

/// Executes a running tick with checked budgets, retaining any trusted failure.
/// The endpoint advances its counter only after successful reduction.
pub fn reduce_tick(
    state: &mut AuthorityState,
    budget: TickBudget,
) -> Result<TickPublication, ServerError> {
    reduce_tick_mode(state, budget, true)
}

/// Runs the actual full phase engine once without appending publication frames.
/// `AuthorityState::run_final` owns the successful endpoint counter and consumption.
pub struct AuthoritativeFinalReducer;

impl FinalReducer for AuthoritativeFinalReducer {
    fn reduce_final(&mut self, state: &mut AuthorityState) -> Result<u64, ServerError> {
        Ok(reduce_tick_mode(state, TickBudget::full(), false)?.tick)
    }
}

/// Validation refusals precede execution and do not poison a healthy authority.
fn reduce_tick_mode(
    state: &mut AuthorityState,
    budget: TickBudget,
    publish: bool,
) -> Result<TickPublication, ServerError> {
    if let Some(error) = state.tick_failure() {
        return Err(error);
    }
    if state.phase() == ServerPhase::Closed || (publish && state.phase() != ServerPhase::Running) {
        return Err(ServerError::InvalidState {
            phase: state.phase(),
        });
    }
    TickBudget::try_new(
        budget.commands(),
        budget.fluid_updates_per_dimension(),
        budget.fluid_rescan_target_per_dimension(),
        budget.farmland_checks(),
        budget.farmland_block_reads(),
    )?;
    // Trusted Rust provider unwinds stop the owner; native aborts and UB are outside this boundary.
    match catch_unwind(AssertUnwindSafe(|| {
        reduce_tick_inner(state, budget, publish)
    })) {
        Ok(Ok(publication)) => Ok(publication),
        Ok(Err(error)) => Err(state.fail_tick(error)),
        Err(_) => Err(state.fail_tick(ServerError::Internal {
            invariant: "authoritative tick panic",
        })),
    }
}

fn reduce_tick_inner(
    state: &mut AuthorityState,
    budget: TickBudget,
    publish: bool,
) -> Result<TickPublication, ServerError> {
    let tick = state.next_tick();
    let drained = drain_mailbox(state, tick, budget.commands());
    let companions = state.drain_companions(COMPANION_FEED);
    // The login scan runs before the context borrows the authority: Active
    // sessions with a save body and no player actor seed initial actors.
    let logins = state.login_seeds();
    // The carried schedules leave authority ownership for the tick: the
    // context holds the only mutable authority borrow during dispatch, so
    // they travel as locals and return after the context drops.
    let mut fluid_schedule =
        std::mem::replace(state.fluid_schedule_mut(), fluids::FluidSchedule::new());
    let mut farmland_schedule = std::mem::replace(
        state.farmland_schedule_mut(),
        farmland::FarmlandSchedule::new(),
    );
    let mut source_players = std::mem::take(state.source_players_mut());
    let mut context = TickContext::for_tick(state, budget);
    let result = catch_unwind(AssertUnwindSafe(|| {
        // Seeded logins land before the first provider row, so this tick's own
        // dispatch already observes freshly joined players.
        for seeded in logins {
            context.stage_login(seeded);
        }
        context.freeze_environment(tick);
        for action in companions {
            context.push_companion_action(action);
        }
        dispatch_rows(
            &mut context,
            tick,
            &drained.dispatched,
            &mut fluid_schedule,
            &mut farmland_schedule,
            &mut source_players,
        )?;
        let overlay = context.viewer_leases();
        // Private observations are projected after every settlement. Provider
        // observations remain useful to fixtures but cannot publish an early pose.
        let (hits, events): (Vec<_>, Vec<_>) = context
            .events()
            .iter()
            .filter(|event| !matches!(event.event(), mornlea_domain::Event::PlayerState(_)))
            .cloned()
            .partition(|event| matches!(event.event(), mornlea_domain::Event::CombatHit(_)));
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
        context.commit_carried();
        Ok::<_, ServerError>((overlay, hits, events, counters))
    }));
    // Context recovery precedes restoration of all three exclusively moved owners.
    drop(context);
    *state.fluid_schedule_mut() = fluid_schedule;
    *state.farmland_schedule_mut() = farmland_schedule;
    *state.source_players_mut() = source_players;
    state.prune_source_players();
    let (mut overlay, hits, mut events, counters) = match result {
        Ok(result) => result?,
        Err(panic) => std::panic::resume_unwind(panic),
    };
    overlay.retain(
        |key, _| matches!(state.session(*key), Some(facts) if facts.phase == SessionPhase::Active),
    );
    state.commit_viewers(overlay);
    events.extend(state.project_player_updates(tick));
    events.extend(hits);
    let publication = TickPublication {
        tick,
        events,
        // Control stays empty: handshake replies are transport-owned and
        // never synthesized by the tick.
        control: Vec::new(),
        counters,
    };
    if publish {
        publication::publish_tick(state, publication.clone())?;
    }
    Ok(publication)
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
/// rows and prevents successful commit and delivery. The carried schedules
/// arrive as locals because the context owns the authority borrow.
#[allow(clippy::too_many_lines)]
fn dispatch_rows(
    context: &mut TickContext<'_>,
    tick: u64,
    dispatched: &[CommandEnvelope],
    fluid_schedule: &mut fluids::FluidSchedule,
    farmland_schedule: &mut farmland::FarmlandSchedule,
    source_players: &mut SourcePlayerBook,
) -> Result<(), ServerError> {
    for envelope in dispatched {
        admit_command(context, envelope)?;
    }
    #[cfg(test)]
    DISPATCH_HOOK.with(|slot| {
        if let Some(hook) = slot.take() {
            hook(context)?;
        }
        Ok::<(), ServerError>(())
    })?;
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
    source_player_restore::advance(source_players, context)?;
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
        // Regen and action interruption still run during reset. Physics and
        // its oxygen/exhaustion settlement resume after the reset tick.
        if context
            .read()
            .runtime(actor)
            .is_some_and(|runtime| runtime.reset)
        {
            continue;
        }
        if source_player_restore::recover(source_players, context, session)? {
            continue;
        }
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
        // Capture coordinates and sample Safe after fall settlement, before later death resets the pose.
        source_player_restore::capture_trample(source_players, context, session)?;
        source_player_restore::capture_snow(source_players, context, session)?;
        source_player_restore::checkpoint_safe(source_players, context, session)?;
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
    // The sole source book consumer settles after all damage and defers scan advancement.
    source_player_restore::settle_deaths(source_players, context)?;
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
        route_interaction(context, &envelope)?;
    }
    // Late Till costs precede every later interaction-region hunger consumer.
    source_player_restore::settle_action_costs(source_players, context)?;
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
    source_player_restore::settle_tramples(source_players, context)?;
    // Source candidates settle from captured coordinates; legacy collection stays late.
    // Source Snow retains its own bounded tracker; the generic schedule remains per tick.
    let mut footprints = crops::FootprintSchedule::new();
    crops::settle_tramples(&mut footprints, context)?;
    crops::run(context, batch_call(RulePhase::SnowFootprint))?;
    source_player_restore::settle_snow(source_players, context)?;
    crops::settle_snow_footprints(&mut footprints, context)?;
    random_blocks::run(context, batch_call(RulePhase::RandomBlock))?;
    random_blocks::advance(context, &active_keys)?;
    containers::run(context, batch_call(RulePhase::ContainerMove))?;
    for actor in mining_actors(context) {
        per_actor(context, RulePhase::MiningStep, actor, mining::run)?;
    }
    // Completed human Mining costs are visible to final player projection.
    source_player_restore::settle_action_costs(source_players, context)?;
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
    if !context.source_player_command_ready(envelope)? {
        return Ok(());
    }
    context.record_player_input(envelope);
    let admits: [ProviderCall; 4] = [
        player_motion::run,
        inventory::run,
        containers::run,
        crafting::run,
    ];
    admit_command_with(context, envelope, admits)
}

fn admit_command_with(
    context: &mut TickContext<'_>,
    envelope: &CommandEnvelope,
    admits: [ProviderCall; 4],
) -> Result<(), ServerError> {
    let call = RuleCall {
        phase: RulePhase::PlayerCommand,
        actor: None,
        command: Some(envelope),
        internal: None,
    };
    let mut admitted = false;
    for admit in admits {
        match admit(context, call) {
            Ok(_) => admitted = true,
            Err(ServerError::InvalidInput { .. }) => {}
            Err(error) => return Err(error),
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
fn route_interaction(
    context: &mut TickContext<'_>,
    envelope: &CommandEnvelope,
) -> Result<(), ServerError> {
    let gates: [ProviderCall; 3] = [world_mutation::run, tools::run, drops::run];
    route_interaction_with(context, envelope, gates)
}

fn route_interaction_with(
    context: &mut TickContext<'_>,
    envelope: &CommandEnvelope,
    gates: [ProviderCall; 3],
) -> Result<(), ServerError> {
    let call = RuleCall {
        phase: RulePhase::Interaction,
        actor: None,
        command: Some(envelope),
        internal: None,
    };
    for gate in gates {
        match gate(context, call) {
            Ok(_) => return Ok(()),
            Err(ServerError::InvalidInput { .. }) => {}
            Err(error) => return Err(error),
        }
    }
    Ok(())
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

#[cfg(test)]
mod routing_tests {
    use super::super::contracts::{Resource, ServerLimits};
    use super::*;
    use mornlea_domain::{Command, CommandEnvelopeParts};
    use std::cell::RefCell;
    thread_local! {
        static CALLS: RefCell<Vec<u8>> = const { RefCell::new(Vec::new()) };
        static FIRST: std::cell::Cell<Result<(),ServerError>> = const { std::cell::Cell::new(Ok(())) };
        static SECOND: std::cell::Cell<Result<(),ServerError>> = const { std::cell::Cell::new(Ok(())) };
    }
    fn report() -> PhaseReport {
        PhaseReport {
            examined: 1,
            applied: 0,
            carried: 0,
            rejected: 0,
        }
    }
    fn first(_: &mut TickContext<'_>, _: RuleCall<'_>) -> Result<PhaseReport, ServerError> {
        CALLS.with(|c| c.borrow_mut().push(1));
        FIRST.with(|r| r.get().map(|()| report()))
    }
    fn second(_: &mut TickContext<'_>, _: RuleCall<'_>) -> Result<PhaseReport, ServerError> {
        CALLS.with(|c| c.borrow_mut().push(2));
        SECOND.with(|r| r.get().map(|()| report()))
    }
    fn third(_: &mut TickContext<'_>, _: RuleCall<'_>) -> Result<PhaseReport, ServerError> {
        CALLS.with(|c| c.borrow_mut().push(3));
        Ok(report())
    }
    fn fourth(_: &mut TickContext<'_>, _: RuleCall<'_>) -> Result<PhaseReport, ServerError> {
        CALLS.with(|c| c.borrow_mut().push(4));
        Ok(report())
    }
    fn invalid(_: &mut TickContext<'_>, _: RuleCall<'_>) -> Result<PhaseReport, ServerError> {
        Err(ServerError::InvalidInput { field: "gate" })
    }
    fn reset(result: Result<(), ServerError>) {
        CALLS.with(|c| c.borrow_mut().clear());
        FIRST.with(|r| r.set(result));
        SECOND.with(|r| r.set(Ok(())));
    }
    fn calls(expected: &[u8]) {
        CALLS.with(|c| assert_eq!(&*c.borrow(), expected));
    }
    fn fixture() -> (AuthorityState, CommandEnvelope) {
        let mut a = AuthorityState::try_new(
            ServerLimits::try_new(8, 4096, 512, 64, 64, 1_048_576).unwrap(),
            42,
        )
        .unwrap();
        let id = mornlea_domain::PlayerId::try_from_bytes([
            1, 0, 0, 0, 0, 0, 64, 0, 128, 0, 0, 0, 0, 0, 0, 1,
        ])
        .unwrap();
        let start = mornlea_protocol::LoginStart::new(id, "Ada", 8).unwrap();
        let session = a
            .admit(
                mornlea_protocol::admit_login(
                    mornlea_protocol::LoginStart::decode_inbound(&start.encode().unwrap()).unwrap(),
                )
                .unwrap(),
                super::super::contracts::TransportKind::Memory,
            )
            .unwrap();
        let envelope = CommandEnvelope::try_new(CommandEnvelopeParts {
            tick: 0,
            session: session.get(),
            sequence: 1,
            arrival_index: 0,
            command: Command::CloseContainer,
        })
        .unwrap();
        (a, envelope)
    }
    #[test]
    fn invalid_input_reaches_next_gate_and_unowned_admission_defers() {
        let (mut a, envelope) = fixture();
        let mut context = TickContext::for_tick(&mut a, TickBudget::full());
        reset(Err(ServerError::InvalidInput { field: "gate" }));
        assert_eq!(
            route_interaction_with(&mut context, &envelope, [first, second, third]),
            Ok(())
        );
        calls(&[1, 2]);
        reset(Err(ServerError::InvalidInput { field: "gate" }));
        assert_eq!(
            admit_command_with(&mut context, &envelope, [first, second, third, fourth]),
            Ok(())
        );
        calls(&[1, 2, 3, 4]);
        assert_eq!(
            admit_command_with(&mut context, &envelope, [invalid; 4]),
            Ok(())
        );
        let deferred_phase = RulePhase::Interaction;
        assert_eq!(context.deferred(deferred_phase), vec![envelope]);
        assert_eq!(
            route_interaction_with(&mut context, &envelope, [invalid; 3]),
            Ok(())
        );
        assert!(context.events().is_empty());
    }
    #[test]
    fn hard_gate_errors_stop_every_later_admission_and_route() {
        let errors = [
            ServerError::Capacity {
                resource: Resource::Commands,
                limit: 4096,
                observed: 4097,
            },
            ServerError::Internal {
                invariant: "injected gate",
            },
            ServerError::Disconnected,
            ServerError::StaleSession {
                session: SessionKey::from_raw(999_999).unwrap(),
            },
        ];
        let (mut a, envelope) = fixture();
        let mut context = TickContext::for_tick(&mut a, TickBudget::full());
        for error in errors {
            reset(Err(error));
            assert_eq!(
                admit_command_with(&mut context, &envelope, [first, second, third, fourth]),
                Err(error),
                "every hard admission error propagates"
            );
            calls(&[1]);
            reset(Err(error));
            assert_eq!(
                route_interaction_with(&mut context, &envelope, [first, second, third]),
                Err(error),
                "every hard route error propagates"
            );
            calls(&[1]);
        }
    }
    #[test]
    fn routing_ok_owns_once_while_admission_ok_keeps_all_roles() {
        let (mut a, envelope) = fixture();
        let mut context = TickContext::for_tick(&mut a, TickBudget::full());
        reset(Ok(()));
        assert_eq!(
            route_interaction_with(&mut context, &envelope, [first, second, third]),
            Ok(())
        );
        calls(&[1]);
        reset(Ok(()));
        assert_eq!(
            admit_command_with(&mut context, &envelope, [first, second, third, fourth]),
            Ok(())
        );
        calls(&[1, 2, 3, 4]);
        reset(Ok(()));
        let error = ServerError::Disconnected;
        SECOND.with(|r| r.set(Err(error)));
        assert_eq!(
            admit_command_with(&mut context, &envelope, [first, second, third, fourth]),
            Err(error),
            "later hard error stops remaining roles"
        );
        calls(&[1, 2]);
    }
}
