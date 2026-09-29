//! Authority-owned farming tools and bucket settlement.
//!
//! The command supplies a look direction only. Current actor pose, ray cells,
//! selected item and changed block all come from the tick's authority view.
//! A successful write and inventory change share the accepted actor mutation
//! transaction; failed preflight never spends a tool or item. Tilling creates
//! dry farmland, and the later moisture phase reconciles hydration from the
//! queued block change, matching the Go tick order.

use mornlea_domain::{
    BlockPos, Command, Dimension, Event, EventRecipient, LookAngles, PlacementSuccess, RoutedEvent,
};
use mornlea_engine::native::contracts::raycast::{Ray, RayCursor, RayFace, RaycastOp};
use mornlea_engine::native::raycast::NativeRaycast;
use mornlea_storage::ItemStack;

use crate::core::contracts::{
    ActorKey, ActorLifecycle, BlockObservation, BlockTxn, BlockWrite, InventoryPatch,
    InventoryRecord, MutationProducer, PhaseReport, ResolvedPlacement, RuleCall, RulePhase,
    ServerError, SessionKey, WorkKind,
};
use crate::core::state::{ActionKind, AuthorityReadView, TickContext};

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
const DOOR_LOWER_FIRST: u16 = 62;
const DOOR_LOWER_LAST: u16 = 69;
const DOOR_UPPER: u16 = 70;
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

#[derive(Clone, Copy)]
struct RayHit {
    observed: BlockObservation,
    face: RayFace,
}

fn is_fluid(block: u16) -> bool {
    (WATER_SOURCE..=WATER_LEVEL_7).contains(&block)
}

fn is_immature_crop(block: u16) -> bool {
    (WHEAT_FIRST..WHEAT_LAST).contains(&block)
        || (POTATO_FIRST..POTATO_LAST).contains(&block)
        || (CARROT_FIRST..CARROT_LAST).contains(&block)
}

fn look_direction(look: LookAngles) -> [f32; 3] {
    let yaw = look.yaw();
    let pitch = look.pitch();
    let cos_pitch = pitch.cos();
    [-yaw.sin() * cos_pitch, pitch.sin(), -yaw.cos() * cos_pitch]
}

/// Mirrors `blockRaycastSampler`: fluid and open lower doors are transparent;
/// an upper door delegates to its lower half and an unready partner blocks.
/// Collection alone turns a source cell into a hit while flowing water stays
/// transparent. Every traversed unready cell refuses the whole command.
fn ray_target(
    view: &AuthorityReadView<'_>,
    dimension: Dimension,
    observed: BlockObservation,
    collect: bool,
) -> bool {
    let block = observed.block;
    if collect && block == WATER_SOURCE {
        return true;
    }
    if block == AIR || is_fluid(block) {
        return false;
    }
    if (DOOR_LOWER_FIRST..=DOOR_LOWER_LAST).contains(&block) {
        return block.is_multiple_of(2);
    }
    if block == DOOR_UPPER {
        let Some(y) = observed.pos.y().checked_sub(1) else {
            return true;
        };
        let lower = view.observation(
            dimension,
            BlockPos::new(observed.pos.x(), y, observed.pos.z()),
        );
        return !lower.is_some_and(|cell| {
            (DOOR_LOWER_FIRST..=DOOR_LOWER_LAST).contains(&cell.block)
                && !cell.block.is_multiple_of(2)
        });
    }
    true
}

fn cast_ray(
    view: &AuthorityReadView<'_>,
    actor: ActorKey,
    look: LookAngles,
    collect: bool,
) -> Result<(Dimension, RayHit), ServerError> {
    let record = view.actor(actor).ok_or(REFUSAL)?;
    if record.lifecycle != ActorLifecycle::Active {
        return Err(REFUSAL);
    }
    let dimension = record.dimension;
    let tunables = view.environment().ok_or(REFUSAL)?.tunables;
    let position = record.motion.position().get();
    let origin = [
        position[0],
        position[1] + tunables.eye_height(),
        position[2],
    ];
    let mut cursor = RayCursor::try_new(Ray {
        origin,
        direction: look_direction(look),
        maximum: tunables.interaction_reach(),
    })
    .map_err(|_| REFUSAL)?;
    loop {
        let batch = NativeRaycast.next_batch(&mut cursor).map_err(|_| REFUSAL)?;
        for record in batch.records() {
            let pos = BlockPos::new(record.cell[0], record.cell[1], record.cell[2]);
            let observed = view.observation(dimension, pos).ok_or(REFUSAL)?;
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
            return Err(REFUSAL);
        }
    }
}

fn adjacent(pos: BlockPos, face: RayFace) -> Result<BlockPos, ServerError> {
    let (dx, dy, dz) = match face {
        RayFace::NegX => (-1, 0, 0),
        RayFace::PosX => (1, 0, 0),
        RayFace::NegY => (0, -1, 0),
        RayFace::PosY => (0, 1, 0),
        RayFace::NegZ => (0, 0, -1),
        RayFace::PosZ => (0, 0, 1),
        RayFace::Origin => return Err(REFUSAL),
    };
    let x = pos.x().checked_add(dx).ok_or(REFUSAL)?;
    let y = pos.y().checked_add(dy).ok_or(REFUSAL)?;
    let z = pos.z().checked_add(dz).ok_or(REFUSAL)?;
    if !(WORLD_MIN_Y..WORLD_MAX_Y).contains(&y) {
        return Err(REFUSAL);
    }
    Ok(BlockPos::new(x, y, z))
}

fn selected(
    view: &AuthorityReadView<'_>,
    actor: ActorKey,
) -> Result<(InventoryRecord, usize, ItemStack), ServerError> {
    let inventory = *view.inventory(actor).ok_or(REFUSAL)?;
    let index = usize::from(inventory.selected.get());
    Ok((inventory, index, inventory.slots[index]))
}

fn settle(
    ctx: &mut TickContext<'_>,
    actor: ActorKey,
    session: SessionKey,
    sequence: u64,
    command: Command,
) -> Result<PhaseReport, ServerError> {
    let (look, collect) = match command {
        Command::TillSoil(look) | Command::BoneMeal(look) | Command::PlaceWater(look) => {
            (look, false)
        }
        Command::CollectWater(look) => (look, true),
        _ => return Err(ServerError::InvalidInput { field: "command" }),
    };
    let view = ctx.read();
    let (dimension, hit) = cast_ray(&view, actor, look, collect)?;
    let (inventory, index, stack) = selected(&view, actor)?;
    let (observed, replacement, next_stack, bucket, till) = match command {
        Command::TillSoil(_) => {
            if !matches!(hit.observed.block, DIRT | GRASS) {
                return Err(REFUSAL);
            }
            let y = hit.observed.pos.y().checked_add(1).ok_or(REFUSAL)?;
            if y >= WORLD_MAX_Y
                || view
                    .observation(
                        dimension,
                        BlockPos::new(hit.observed.pos.x(), y, hit.observed.pos.z()),
                    )
                    .ok_or(REFUSAL)?
                    .block
                    != AIR
            {
                return Err(REFUSAL);
            }
            let broken = match stack.item {
                ITEM_STONE_HOE if stack.count == 1 && stack.durability > 0 => ITEM_BROKEN_STONE_HOE,
                ITEM_IRON_HOE if stack.count == 1 && stack.durability > 0 => ITEM_BROKEN_IRON_HOE,
                _ => return Err(REFUSAL),
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
                return Err(REFUSAL);
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
            if hit.observed.block != WATER_SOURCE
                || stack.item != ITEM_EMPTY_BUCKET
                || stack.count != 1
            {
                return Err(REFUSAL);
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
            let observed = view.observation(dimension, target).ok_or(REFUSAL)?;
            if observed.block != AIR
                && !(WATER_SOURCE + 1..=WATER_LEVEL_7).contains(&observed.block)
            {
                return Err(REFUSAL);
            }
            if stack.item != ITEM_WATER_BUCKET || stack.count != 1 {
                return Err(REFUSAL);
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
        _ => unreachable!("command family checked above"),
    };
    let mut after = inventory;
    after.slots[index] = next_stack;
    let patch = InventoryPatch::try_new(actor, inventory, after).map_err(|_| REFUSAL)?;
    let write = BlockWrite::try_new(observed, replacement).map_err(|_| REFUSAL)?;
    if till {
        ctx.check_charge_capacity()?;
    }
    if bucket {
        ctx.check_mining_suppression(actor)?;
    }
    ctx.charge(WorkKind::Effects, 1)?;
    let txn = BlockTxn {
        producer: MutationProducer::Actor(actor),
        tick: ctx.read().tick(),
        writes: vec![write],
        inventory: Some(patch),
        containers: Vec::new(),
        drops: None,
        mining: None,
    };
    ctx.transaction()
        .try_place(ResolvedPlacement { txn })
        .map_err(|_| REFUSAL)?;
    if till {
        ctx.note_charge(actor, ActionKind::Till)?;
    }
    if bucket {
        ctx.suppress_mining(actor)?;
        ctx.emit(RoutedEvent::new(
            EventRecipient::Session(session.get()),
            Event::PlaceBlockSucceeded(PlacementSuccess::new(sequence)),
        ))?;
    }
    Ok(PhaseReport {
        examined: 1,
        applied: 1,
        carried: 0,
        rejected: 0,
    })
}

/// Settles exactly one ordered human tool command on the interaction phase.
pub fn run(ctx: &mut TickContext<'_>, call: RuleCall<'_>) -> Result<PhaseReport, ServerError> {
    if call.phase != RulePhase::Interaction || call.internal.is_some() || call.actor.is_some() {
        return Err(ServerError::InvalidInput { field: "phase" });
    }
    let envelope = call
        .command
        .ok_or(ServerError::InvalidInput { field: "command" })?;
    let session = SessionKey::from_raw(envelope.session()).ok_or(REFUSAL)?;
    settle(
        ctx,
        ActorKey::Player(session),
        session,
        envelope.sequence(),
        envelope.command(),
    )
}
