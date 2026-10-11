//! Authority-owned farming tools and bucket settlement.
//!
//! The command supplies a look direction only. Current actor pose, ray cells,
//! selected item and changed block all come from the tick's authority view.
//! A successful write and inventory change share the accepted actor mutation
//! transaction; failed preflight never spends a tool or item. Tilling creates
//! dry farmland, and the later moisture phase reconciles hydration from the
//! queued block change, matching the Go tick order.

use mornlea_domain::{
    BlockPos, Command, CommandEnvelope, Dimension, Event, EventRecipient, LookAngles,
    PlacementSuccess, RejectReason, RoutedEvent,
};
use mornlea_engine::native::contracts::raycast::{Ray, RayCursor, RayFace, RaycastOp};
use mornlea_engine::native::raycast::NativeRaycast;
use mornlea_storage::ItemStack;

use crate::core::command_outcome::{CommandDisposition, CommandResult};
use crate::core::contracts::{
    ActorKey, ActorLifecycle, BlockObservation, BlockTxn, BlockWrite, InventoryPatch,
    InventoryRecord, MutationProducer, PhaseReport, ResolvedPlacement, RuleCall, RulePhase,
    ServerError, SessionKey, WorkKind,
};
use crate::core::interaction::{look_direction, normalized_direction, target_block};
use crate::core::state::{ActionKind, AuthorityReadView, ObservationTrace, TickContext};
use std::cell::RefCell;

const AIR: u16 = 0;
const DIRT: u16 = 3;
const GRASS: u16 = 4;
const WATER_SOURCE: u16 = 27;
const WATER_LEVEL_7: u16 = 34;
const FARMLAND_DRY: u16 = 35;
const WHEAT_FIRST: u16 = 37;
const WHEAT_LAST: u16 = 44;
const POTATO_FIRST: u16 = 46;
const POTATO_LAST: u16 = 53;
const CARROT_FIRST: u16 = 54;
const CARROT_LAST: u16 = 61;
const ITEM_STONE_HOE: u16 = 30;
const ITEM_IRON_HOE: u16 = 31;
const ITEM_BROKEN_STONE_HOE: u16 = 32;
const ITEM_BROKEN_IRON_HOE: u16 = 33;
const ITEM_BONE_MEAL: u16 = 39;
const ITEM_EMPTY_BUCKET: u16 = 55;
const ITEM_WATER_BUCKET: u16 = 56;
const WORLD_MIN_Y: i32 = -64;
const WORLD_MAX_Y: i32 = 320;

const REFUSAL: ServerError = ServerError::InvalidInput {
    field: "interaction",
};

// Semantic refusals are tagged at their source; owned-state and mutation faults stay hard.
#[derive(Debug)]
enum ToolFailure {
    Refused(RejectReason),
    Trusted(ServerError),
}

impl ToolFailure {
    fn into_server_error(self) -> ServerError {
        match self {
            Self::Refused(_) => REFUSAL,
            Self::Trusted(error) => error,
        }
    }
}

fn trusted(invariant: &'static str) -> ToolFailure {
    ToolFailure::Trusted(ServerError::Internal { invariant })
}

struct ToolPrepared {
    txn: BlockTxn,
    till: bool,
    bucket: bool,
}

#[derive(Clone, Copy)]
struct RayHit {
    observed: BlockObservation,
    face: RayFace,
}

fn tool_look(command: Command) -> Option<(LookAngles, bool)> {
    match command {
        Command::TillSoil(look) | Command::BoneMeal(look) | Command::PlaceWater(look) => {
            Some((look, false))
        }
        Command::CollectWater(look) => Some((look, true)),
        _ => None,
    }
}

fn is_fluid(block: u16) -> bool {
    (WATER_SOURCE..=WATER_LEVEL_7).contains(&block)
}

fn is_immature_crop(block: u16) -> bool {
    (WHEAT_FIRST..WHEAT_LAST).contains(&block)
        || (POTATO_FIRST..POTATO_LAST).contains(&block)
        || (CARROT_FIRST..CARROT_LAST).contains(&block)
}

/// Collection adds source water to the shared solid classifier. Flowing water
/// and open doors remain transparent; an unavailable in-height cell refuses.
fn ray_target(
    view: &AuthorityReadView<'_>,
    dimension: Dimension,
    observed: BlockObservation,
    collect: bool,
) -> bool {
    (collect && observed.block == WATER_SOURCE)
        || target_block(view, dimension, observed.pos, observed.block)
}

fn cast_ray(
    view: &AuthorityReadView<'_>,
    actor: ActorKey,
    look: LookAngles,
    collect: bool,
) -> Result<(Dimension, RayHit), ToolFailure> {
    let record = view
        .actor(actor)
        .filter(|record| record.lifecycle == ActorLifecycle::Active)
        .ok_or(ToolFailure::Refused(RejectReason::PlayerNotReady))?;
    let dimension = record.dimension;
    let tunables = view
        .environment()
        .ok_or_else(|| trusted("tool environment"))?
        .tunables;
    let position = record.motion.position().get();
    let origin = [
        position[0],
        position[1] + tunables.eye_height(),
        position[2],
    ];
    let direction = normalized_direction(look_direction(look.yaw(), look.pitch()))
        .ok_or(ToolFailure::Refused(RejectReason::InvalidRay))?;
    let mut cursor = RayCursor::try_new(Ray {
        origin,
        direction,
        maximum: tunables.interaction_reach(),
    })
    .map_err(|_| ToolFailure::Refused(RejectReason::InvalidRay))?;
    loop {
        let batch = NativeRaycast
            .next_batch(&mut cursor)
            .map_err(|_| ToolFailure::Refused(RejectReason::InvalidRay))?;
        for record in batch.records() {
            let pos = BlockPos::new(record.cell[0], record.cell[1], record.cell[2]);
            // Source world reads outside height are immutable air, without mutation observations.
            if !(WORLD_MIN_Y..WORLD_MAX_Y).contains(&pos.y()) {
                continue;
            }
            let observed = view
                .observation(dimension, pos)
                .ok_or(ToolFailure::Refused(RejectReason::ChunkNotReady))?;
            if ray_target(view, dimension, observed, collect) {
                return Ok((
                    dimension,
                    RayHit {
                        observed,
                        face: record.face,
                    },
                ));
            }
        }
        if batch.is_done() {
            return Err(ToolFailure::Refused(RejectReason::NoTarget));
        }
    }
}

fn adjacent(pos: BlockPos, face: RayFace) -> Result<BlockPos, ToolFailure> {
    let (dx, dy, dz) = match face {
        RayFace::NegX => (-1, 0, 0),
        RayFace::PosX => (1, 0, 0),
        RayFace::NegY => (0, -1, 0),
        RayFace::PosY => (0, 1, 0),
        RayFace::NegZ => (0, 0, -1),
        RayFace::PosZ => (0, 0, 1),
        RayFace::Origin => return Err(ToolFailure::Refused(RejectReason::Occupied)),
    };
    let invalid = || ToolFailure::Refused(RejectReason::InvalidBlock);
    let x = pos.x().checked_add(dx).ok_or_else(invalid)?;
    let y = pos.y().checked_add(dy).ok_or_else(invalid)?;
    let z = pos.z().checked_add(dz).ok_or_else(invalid)?;
    if !(WORLD_MIN_Y..WORLD_MAX_Y).contains(&y) {
        return Err(invalid());
    }
    Ok(BlockPos::new(x, y, z))
}

fn selected(
    view: &AuthorityReadView<'_>,
    actor: ActorKey,
) -> Result<(InventoryRecord, usize, ItemStack), ToolFailure> {
    let inventory = *view
        .inventory(actor)
        .ok_or_else(|| trusted("tool inventory"))?;
    let index = usize::from(inventory.selected.get());
    Ok((inventory, index, inventory.slots[index]))
}

fn prepare_tool(
    ctx: &TickContext<'_>,
    actor: ActorKey,
    command: Command,
) -> Result<ToolPrepared, ToolFailure> {
    let (look, collect) = tool_look(command).ok_or_else(|| trusted("tool command owner"))?;
    let trace = RefCell::new(ObservationTrace::default());
    let base = ctx.read();
    let view = base.with_observation_trace(&trace);
    let result = (|| {
        let (dimension, hit) = cast_ray(&view, actor, look, collect)?;
        let (inventory, index, stack) = selected(&view, actor)?;
        let invalid = || ToolFailure::Refused(RejectReason::InvalidBlock);
        let (observed, replacement, next_stack, bucket, till) = match command {
            Command::TillSoil(_) => {
                if !matches!(hit.observed.block, DIRT | GRASS) {
                    return Err(invalid());
                }
                let y = hit.observed.pos.y().checked_add(1).ok_or_else(invalid)?;
                // The upper boundary is an air read, never a writable support address.
                let above = if y >= WORLD_MAX_Y {
                    AIR
                } else {
                    view.observation(
                        dimension,
                        BlockPos::new(hit.observed.pos.x(), y, hit.observed.pos.z()),
                    )
                    .ok_or(ToolFailure::Refused(RejectReason::ChunkNotReady))?
                    .block
                };
                if above != AIR {
                    return Err(ToolFailure::Refused(RejectReason::Occupied));
                }
                let broken = match stack.item {
                    ITEM_STONE_HOE if stack.count == 1 && stack.durability > 0 => {
                        ITEM_BROKEN_STONE_HOE
                    }
                    ITEM_IRON_HOE if stack.count == 1 && stack.durability > 0 => {
                        ITEM_BROKEN_IRON_HOE
                    }
                    _ => return Err(invalid()),
                };
                let next = if stack.durability == 1 {
                    ItemStack {
                        item: broken,
                        count: 1,
                        durability: 0,
                    }
                } else {
                    ItemStack {
                        item: stack.item,
                        count: 1,
                        durability: stack.durability - 1,
                    }
                };
                (hit.observed, FARMLAND_DRY, next, false, true)
            }
            Command::BoneMeal(_) => {
                if !is_immature_crop(hit.observed.block)
                    || stack.item != ITEM_BONE_MEAL
                    || stack.count == 0
                {
                    return Err(invalid());
                }
                let next = if stack.count == 1 {
                    ItemStack::default()
                } else {
                    ItemStack {
                        item: stack.item,
                        count: stack.count - 1,
                        durability: 0,
                    }
                };
                (hit.observed, hit.observed.block + 1, next, false, false)
            }
            Command::CollectWater(_) => {
                if hit.observed.block != WATER_SOURCE {
                    return Err(ToolFailure::Refused(RejectReason::NotFluidSource));
                }
                if stack.item != ITEM_EMPTY_BUCKET || stack.count != 1 {
                    return Err(ToolFailure::Refused(RejectReason::BucketMismatch));
                }
                (
                    hit.observed,
                    AIR,
                    ItemStack {
                        item: ITEM_WATER_BUCKET,
                        count: 1,
                        durability: 0,
                    },
                    true,
                    false,
                )
            }
            Command::PlaceWater(_) => {
                let target = adjacent(hit.observed.pos, hit.face)?;
                let observed = view
                    .observation(dimension, target)
                    .ok_or(ToolFailure::Refused(RejectReason::ChunkNotReady))?;
                if observed.block != AIR
                    && (!is_fluid(observed.block) || observed.block == WATER_SOURCE)
                {
                    return Err(ToolFailure::Refused(RejectReason::Occupied));
                }
                if stack.item != ITEM_WATER_BUCKET || stack.count != 1 {
                    return Err(ToolFailure::Refused(RejectReason::BucketMismatch));
                }
                (
                    observed,
                    WATER_SOURCE,
                    ItemStack {
                        item: ITEM_EMPTY_BUCKET,
                        count: 1,
                        durability: 0,
                    },
                    true,
                    false,
                )
            }
            _ => unreachable!("tool family checked before lookup"),
        };
        let mut after = inventory;
        after.slots[index] = next_stack;
        let patch = InventoryPatch::try_new(actor, inventory, after)
            .map_err(|_| trusted("tool preparation"))?;
        let write =
            BlockWrite::try_new(observed, replacement).map_err(|_| trusted("tool preparation"))?;
        let read_basis = view
            .mutation_basis(actor, &trace.borrow())
            .map_err(|_| trusted("tool preparation"))?;
        Ok(ToolPrepared {
            txn: BlockTxn {
                producer: MutationProducer::Actor(actor),
                tick: ctx.read().tick(),
                read_basis: Some(read_basis),
                writes: vec![write],
                inventory: Some(patch),
                containers: Vec::new(),
                drops: None,
                mining: None,
            },
            till,
            bucket,
        })
    })();
    // Exhausted trusted observation storage takes priority over an apparent semantic miss.
    trace
        .borrow()
        .check_capacity()
        .map_err(|_| trusted("tool observations"))?;
    result
}

fn settle_checked(
    ctx: &mut TickContext<'_>,
    actor: ActorKey,
    session: SessionKey,
    sequence: u64,
    command: Command,
) -> Result<PhaseReport, ToolFailure> {
    let prepared = prepare_tool(ctx, actor, command)?;
    if prepared.till {
        ctx.check_charge_capacity().map_err(ToolFailure::Trusted)?;
    }
    if prepared.bucket {
        ctx.check_mining_suppression(actor)
            .map_err(ToolFailure::Trusted)?;
    }
    ctx.charge(WorkKind::Effects, 1)
        .map_err(ToolFailure::Trusted)?;
    ctx.transaction()
        .try_place(ResolvedPlacement { txn: prepared.txn })
        .map_err(|_| trusted("tool staging"))?;
    if prepared.till {
        ctx.note_charge(actor, ActionKind::Till)
            .map_err(ToolFailure::Trusted)?;
    }
    if prepared.bucket {
        ctx.suppress_mining(actor).map_err(ToolFailure::Trusted)?;
    }
    // Accepted intent survives a later same-tick exchange back to the original inventory.
    ctx.record_inventory_publication_dirty(session);
    if prepared.bucket {
        ctx.emit(RoutedEvent::new(
            EventRecipient::Session(session.get()),
            Event::PlaceBlockSucceeded(PlacementSuccess::new(sequence)),
        ))
        .map_err(ToolFailure::Trusted)?;
    }
    Ok(PhaseReport {
        examined: 1,
        applied: 1,
        carried: 0,
        rejected: 0,
    })
}

/// Tool look validates admission without changing retained actor motion or input acknowledgment.
pub(crate) fn admit_command(
    ctx: &mut TickContext<'_>,
    envelope: &CommandEnvelope,
) -> CommandResult {
    let Some((look, _)) = tool_look(envelope.command()) else {
        return Ok(CommandDisposition::Unowned);
    };
    let session = SessionKey::from_raw(envelope.session()).ok_or(ServerError::Internal {
        invariant: "tool admission session",
    })?;
    if !matches!(ctx.read().actor(ActorKey::Player(session)),Some(record) if record.lifecycle == ActorLifecycle::Active)
    {
        return Ok(CommandDisposition::Refused(RejectReason::PlayerNotReady));
    }
    const MAX_PITCH: f32 = (std::f64::consts::PI / 2.0 - 0.01) as f32;
    if !(-MAX_PITCH..=MAX_PITCH).contains(&look.pitch()) {
        return Ok(CommandDisposition::Refused(RejectReason::InvalidInput));
    }
    ctx.defer(*envelope, RulePhase::Interaction)?;
    Ok(CommandDisposition::Settled(PhaseReport {
        examined: 1,
        applied: 1,
        carried: 0,
        rejected: 0,
    }))
}

/// Live tools retain contextual refusals; current source geometry has no separate view gate.
pub(crate) fn settle_command(
    ctx: &mut TickContext<'_>,
    envelope: &CommandEnvelope,
) -> CommandResult {
    if tool_look(envelope.command()).is_none() {
        return Ok(CommandDisposition::Unowned);
    }
    let session = SessionKey::from_raw(envelope.session()).ok_or(ServerError::Internal {
        invariant: "tool session",
    })?;
    match settle_checked(
        ctx,
        ActorKey::Player(session),
        session,
        envelope.sequence(),
        envelope.command(),
    ) {
        Ok(report) => Ok(CommandDisposition::Settled(report)),
        Err(ToolFailure::Refused(reason)) => Ok(CommandDisposition::Refused(reason)),
        Err(ToolFailure::Trusted(error)) => Err(error),
    }
}

/// Direct provider callers retain generic semantic errors and share the checked atomic core.
pub fn run(ctx: &mut TickContext<'_>, call: RuleCall<'_>) -> Result<PhaseReport, ServerError> {
    if call.phase != RulePhase::Interaction || call.internal.is_some() || call.actor.is_some() {
        return Err(ServerError::InvalidInput { field: "phase" });
    }
    let envelope = call
        .command
        .ok_or(ServerError::InvalidInput { field: "command" })?;
    if tool_look(envelope.command()).is_none() {
        return Err(ServerError::InvalidInput { field: "command" });
    }
    let session = SessionKey::from_raw(envelope.session()).ok_or(REFUSAL)?;
    settle_checked(
        ctx,
        ActorKey::Player(session),
        session,
        envelope.sequence(),
        envelope.command(),
    )
    .map_err(ToolFailure::into_server_error)
}

#[cfg(test)]
mod checked_tool_command_tests {
    use super::*;
    use crate::core::command_outcome::CommandDisposition;
    use crate::core::contracts::{
        ActorBody, ActorRecord, EnvironmentState, RuleEffect, RuleTunables, ServerLimits,
        TickBudget,
    };
    use crate::core::state::AuthorityState;
    use mornlea_domain::{
        ChunkPos, CommandEnvelope, CommandEnvelopeParts, FiniteVec3, MotionState, MotionStateParts,
        SurvivalState, SurvivalStateParts, Weather,
    };
    use mornlea_storage::{PlayerLocation, PlayerSave};

    fn owned_actor(session: SessionKey) -> ActorRecord {
        let position = [0.5, 64.0, 0.5];
        let mut id = [0u8; 16];
        id[0] = 1;
        id[6] = 0x40;
        id[8] = 0x80;
        ActorRecord::try_new(
            ActorKey::Player(session),
            ActorLifecycle::Active,
            Dimension::OVERWORLD,
            MotionState::new(MotionStateParts {
                position: FiniteVec3::try_new(position).unwrap(),
                velocity: FiniteVec3::try_new([0.0; 3]).unwrap(),
                on_ground: true,
            }),
            LookAngles::try_new(std::f32::consts::PI, 0.0).unwrap(),
            SurvivalState::try_new(SurvivalStateParts {
                health: 20,
                oxygen: 300,
                hunger: 20,
                saturation_zero: false,
                armor_points: 0,
            })
            .unwrap(),
            ActorBody::Player(PlayerSave {
                player_id: mornlea_storage::PlayerId::from_bytes(id),
                revision: 1,
                display_name: "Tester".to_owned(),
                current: PlayerLocation {
                    dimension: 0,
                    position,
                },
                yaw: std::f32::consts::PI,
                pitch: 0.0,
                safe: None,
                inventory: mornlea_storage::Inventory::default(),
                health: 20,
                hunger: 20,
                saturation_milli: 5_000,
                exhaustion_milli: 0,
                respawn_present: false,
                respawn_position: [0.0; 3],
                respawn_dimension: 0,
                armor: [ItemStack::default(); 4],
            }),
        )
        .unwrap()
    }

    fn authority() -> AuthorityState {
        AuthorityState::try_new(
            ServerLimits::try_new(8, 4096, 512, 64, 64, 1_048_576).unwrap(),
            7,
        )
        .unwrap()
    }

    fn envelope(command: Command) -> CommandEnvelope {
        CommandEnvelope::try_new(CommandEnvelopeParts {
            tick: 0,
            session: 1,
            sequence: 0,
            arrival_index: 0,
            command,
        })
        .unwrap()
    }

    #[test]
    fn checked_tool_command_contract_example() {
        let mut state = authority();
        let mut context = TickContext::harness(&mut state, TickBudget::full());
        let foreign = envelope(Command::CloseContainer);
        assert_eq!(
            admit_command(&mut context, &foreign),
            Ok(CommandDisposition::Unowned)
        );
        assert_eq!(
            settle_command(&mut context, &foreign),
            Ok(CommandDisposition::Unowned)
        );
        for command in [
            Command::TillSoil,
            Command::BoneMeal,
            Command::CollectWater,
            Command::PlaceWater,
        ] {
            let action = envelope(command(LookAngles::try_new(0.0, 0.0).unwrap()));
            assert_eq!(
                admit_command(&mut context, &action),
                Ok(CommandDisposition::Refused(
                    mornlea_domain::RejectReason::PlayerNotReady
                ))
            );
            assert_eq!(
                settle_command(&mut context, &action),
                Ok(CommandDisposition::Refused(
                    mornlea_domain::RejectReason::PlayerNotReady
                ))
            );
        }
        let session = SessionKey::from_raw(1).unwrap();
        context
            .stage(RuleEffect::Actor(owned_actor(session)))
            .unwrap();
        let before = context
            .read()
            .actor(ActorKey::Player(session))
            .unwrap()
            .look;
        for command in [
            Command::TillSoil,
            Command::BoneMeal,
            Command::CollectWater,
            Command::PlaceWater,
        ] {
            let action = envelope(command(
                LookAngles::try_new(0.0, std::f32::consts::FRAC_PI_2).unwrap(),
            ));
            assert_eq!(
                admit_command(&mut context, &action),
                Ok(CommandDisposition::Refused(
                    mornlea_domain::RejectReason::InvalidInput
                ))
            );
        }
        assert_eq!(
            context
                .read()
                .actor(ActorKey::Player(session))
                .unwrap()
                .look,
            before
        );
        assert!(context.events().is_empty());
    }

    #[test]
    fn checked_tool_trace_capacity_contract_example() {
        let mut state = authority();
        let mut context = TickContext::harness(&mut state, TickBudget::full());
        let session = SessionKey::from_raw(1).unwrap();
        let actor = ActorKey::Player(session);
        context
            .stage(RuleEffect::Actor(owned_actor(session)))
            .unwrap();
        context.preload_inventory(actor, InventoryRecord::empty());
        let d = RuleTunables::source_defaults();
        let tunables = RuleTunables::try_new(
            d.physics(),
            d.regen_delay_ticks(),
            d.regen_interval_ticks(),
            d.drown_interval_ticks(),
            d.starvation_interval_ticks(),
            d.regen_hunger_threshold(),
            d.exhaustion_threshold_milli(),
            d.eating_ticks(),
            d.furnace_burn_ticks(),
            d.furnace_smelt_ticks(),
            d.fluid_delay(),
            d.random_attempts(),
            d.crop_growth_percent(),
            600.0,
            d.eye_height(),
            d.drop_pickup_delay_ticks(),
            d.player_drop_pickup_delay_ticks(),
            d.drop_lifetime_ticks(),
            d.drop_pickup_range(),
        )
        .unwrap();
        context
            .stage(RuleEffect::Environment(EnvironmentState {
                seed: 7,
                next_tick: 0,
                world_time: 0,
                day_phase_offset: 0,
                season_offset: 0,
                weather: Weather::Clear,
                weather_remaining: 0,
                difficulty: 0,
                tunables,
            }))
            .unwrap();
        for z in 0..=512 {
            let pos = BlockPos::new(0, 65, z);
            let key = crate::core::contracts::ChunkKey {
                dimension: Dimension::OVERWORLD,
                pos: ChunkPos::new(0, z >> 4),
            };
            context.preload_block(BlockObservation::try_new(key, 1, 1, pos, AIR).unwrap());
        }
        // Extended reach is a prepared trace consumer, not a default gameplay producer.
        assert!(matches!(
            prepare_tool(
                &context,
                actor,
                Command::TillSoil(LookAngles::try_new(std::f32::consts::PI, 0.0).unwrap())
            ),
            Err(ToolFailure::Trusted(ServerError::Internal {
                invariant: "tool observations"
            }))
        ));
        assert_eq!(
            context.read().inventory(actor),
            Some(&InventoryRecord::empty())
        );
        assert!(context.events().is_empty());
    }
}
