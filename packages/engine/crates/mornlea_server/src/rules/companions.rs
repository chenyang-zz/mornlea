//! Companion action execution: intent validation, shared-exit motion, atomic placement.
//!
//! This provider owns the three companion batch calls, each with
//! actor/command/internal all `None`; any other shape refuses with
//! [`ServerError::InvalidInput`] field `"companion_call"` before effects:
//!
//! - `RulePhase::CompanionIntent` validates every envelope on
//!   [`AuthorityReadView::companion_actions`] in arrival order and selects the
//!   first valid action per companion ID. Invalid payloads, unknown or
//!   inactive companions, and same-ID later duplicates are ignored with zero
//!   effects and zero staged state; they count `rejected`, never error.
//! - `RulePhase::CompanionMotion` steps every active companion through the
//!   accepted motion kernel path: a selected `Move` steers with its
//!   components, jump and normalized yaw, while any other selection or no
//!   selection steps neutral input retaining the current yaw.
//! - `RulePhase::CompanionPlacement` settles selected `Place` intents in
//!   companion-ID byte order through the shared transaction.
//!
//! Selection is recomputed from the view on every call and shared with mining.
//! Movement and placement last one tick; mining owns its separately retained
//! hold in companion runtime, so an interrupted progress record cannot release it.
//!
//! Mirrored Go rows, each cited at its site:
//!
//! - `packages/server/sim/entity/companion_action.go` (`validCompanionAction`,
//!   `applyCompanionActions`, `advanceActiveCompanions`): the payload defense
//!   (move components in [-1, 1] with finite yaw, mine/place target Y in
//!   [-64, 320), place block non-air and registered), first-valid-per-ID in
//!   arrival order, neutral input retaining yaw for non-`Move` companions with
//!   no persistence across ticks, and the shared player physics exit
//!   (`TestCompanionActionSharesPlayerPhysicsExit`).
//! - `packages/server/sim/entity/companion_placement.go`
//!   (`settleCompanionPlacements`, `completeCompanionPlacement`): ID byte-order
//!   settle, first-commit-wins on competing claims, and silent refusal with no
//!   staged failure entry.
//! - `packages/server/sim/entity/placement.go` (`normalizeYaw`): the yaw
//!   convention `atan2(-x, -z)` normalized into [-pi, pi).
//! - `packages/shared/physics/step.go`, `collision.go`, `submersion.go` and
//!   `types.go`: the kernel-facing sweep, prism, grid and submersion mirrors,
//!   identical in shape to the accepted player-motion plumbing because the Go
//!   companion advance runs the same `StepWithTunables`.
//!
//! Deliberate boundaries. Provenance (generation/attempt/digest/run/snapshot)
//! stays ingress-owned at admit time: no provider-visible gate accessor
//! exists, so the provider never re-reads gates. Placement reuses
//! [`resolve_companion_place`] plus [`MutationTxn::try_place`] as-is; every
//! refusal stages nothing and counts `rejected`. Placement-before-interaction
//! order is a reducer constraint, not provider code.

use mornlea_domain::{
    BlockPos, CompanionId, Dimension, FiniteVec3, LookAngles, MotionState, MotionStateParts,
    registered_block,
};
use mornlea_engine::native::contracts::collision::{Aabb, CollisionCell, CollisionGrid};
use mornlea_engine::native::contracts::physics::{
    PhysicsControls, PhysicsOp, PhysicsRequest, PhysicsState, PhysicsTuning, SweepBounds,
};
use mornlea_engine::native::physics::NativePhysics;

use crate::core::contracts::{
    ActorKey, ActorLifecycle, ActorRecord, CompanionAction, PhaseReport, RuleCall, RuleEffect,
    RulePhase, ServerError,
};
use crate::core::mutation::resolve_companion_place;
use crate::core::state::{AuthorityReadView, TickContext};

/// Lowest world Y, inclusive (`core.MinY`, `packages/shared/core/pos.go`).
const MIN_Y: i32 = -64;
/// Highest world Y, exclusive (`core.MaxY`).
const MAX_Y: i32 = 320;
/// Air (`core.AirID`, `packages/shared/core/block.go`).
const AIR: u16 = 0;

/// Player half width (`PlayerWidth / 2`, `packages/shared/physics/types.go`).
/// Companions share the player body: the oracle names no companion-specific
/// extents.
const HALF_WIDTH: f32 = 0.3;
/// Player height (`PlayerHeight`).
const PLAYER_HEIGHT: f32 = 1.8;
/// Sweep/prism padding (`CollisionEpsilon`).
const COLLISION_EPSILON: f32 = 1e-5;
/// Ground support probe (`GroundProbe`).
const GROUND_PROBE: f32 = 1e-4;
/// Fluid range, zero collision (`WaterSourceID..=WaterLevel7ID`,
/// `core.IsFluid` in `packages/shared/core/fluid.go`).
const FLUID_FIRST: u16 = 27;
const FLUID_LAST: u16 = 34;
/// Farmland pair, top face at 15/16 (`IsFarmland`, `farmlandCollisionHeight`).
const FARMLAND_FIRST: u16 = 35;
const FARMLAND_LAST: u16 = 36;
const FARMLAND_TOP: f32 = 0.9375;
/// Crop stages, all zero collision (`IsCrop` in
/// `packages/shared/core/farming.go`: wheat 37..44, potato 46..53, carrot
/// 54..61; 45 is the solid workbench).
const WHEAT_FIRST: u16 = 37;
const WHEAT_LAST: u16 = 44;
const POTATO_FIRST: u16 = 46;
const POTATO_LAST: u16 = 53;
const CARROT_FIRST: u16 = 54;
const CARROT_LAST: u16 = 61;
/// Door lower range and upper form (`DoorLowerSouthClosed..=DoorUpper`).
const DOOR_LOWER_FIRST: u16 = 62;
const DOOR_LOWER_LAST: u16 = 69;
const DOOR_UPPER: u16 = 70;
/// Closed/open leaf thickness 3/16 (`BlockCollisionBoxes`).
const DOOR_THICKNESS: f32 = 3.0 / 16.0;
/// Torch forms, zero collision (`TorchStandingID..=TorchWallNegZID`).
const TORCH_FIRST: u16 = 71;
const TORCH_LAST: u16 = 75;
/// Bed forms, half box at 9/16 (`BedFootSouthID..=BedHeadEastID`,
/// `bedCollisionHeight`).
const BED_FIRST: u16 = 76;
const BED_LAST: u16 = 83;
const BED_TOP: f32 = 0.5625;
/// Wild grass, zero collision (`ShortGrassID`, `IsWildGrass`).
const SHORT_GRASS: u16 = 84;
/// Snow layers, zero collision (`SnowLayer1BlockID..=SnowLayer4BlockID`).
const SNOW_FIRST: u16 = 85;
const SNOW_LAST: u16 = 88;
/// Sapling, zero collision (`SaplingID`, `IsSapling`).
const SAPLING: u16 = 89;

/// Grid cell cap mirror (`collisionMaxCells`,
/// `packages/shared/physics/collision.go`).
const PRISM_MAX_CELLS: u64 = 4096;

/// The three batch calls this provider owns: intent validation, motion
/// advance, and placement settle. Every call carries no actor, command or
/// internal handle; any other shape refuses with the frozen field before
/// staging anything.
pub fn run(ctx: &mut TickContext<'_>, call: RuleCall<'_>) -> Result<PhaseReport, ServerError> {
    match call.phase {
        RulePhase::CompanionIntent | RulePhase::CompanionMotion | RulePhase::CompanionPlacement => {
        }
        _ => {
            return Err(ServerError::InvalidInput {
                field: "companion_call",
            });
        }
    }
    if call.actor.is_some() || call.command.is_some() || call.internal.is_some() {
        return Err(ServerError::InvalidInput {
            field: "companion_call",
        });
    }
    match call.phase {
        RulePhase::CompanionIntent => run_intent(ctx),
        RulePhase::CompanionMotion => run_motion(ctx),
        RulePhase::CompanionPlacement => run_placement(ctx),
        _ => Err(ServerError::InvalidInput {
            field: "companion_call",
        }),
    }
}

/// Payload defense mirror (`validCompanionAction` in
/// `packages/server/sim/entity/companion_action.go`): `Move` components in
/// [-1, 1] with finite yaw, mine/place targets inside the world vertical
/// range, and `Place` additionally non-air and registered. `MineRelease`
/// carries no payload constraint. The envelope constructor already enforces
/// the structural half; the provider re-checks because a raw literal can
/// bypass it.
fn valid_action(action: &CompanionAction) -> bool {
    match action {
        CompanionAction::Move {
            move_x,
            move_z,
            yaw,
            ..
        } => (-1..=1).contains(move_x) && (-1..=1).contains(move_z) && yaw.is_finite(),
        CompanionAction::MineHold { target } => valid_target(*target),
        CompanionAction::MineRelease => true,
        CompanionAction::Place { target, block } => {
            valid_target(*target) && *block != AIR && registered_block(*block)
        }
    }
}

/// Target vertical bound mirror (`validCompanionActionTarget`): X/Z name any
/// column while Y must land inside the world height range.
fn valid_target(target: BlockPos) -> bool {
    (MIN_Y..MAX_Y).contains(&target.y())
}

/// First valid action per companion ID in arrival order
/// (`applyCompanionActions`): an envelope already holding a selection for its
/// ID is a duplicate, an invalid payload is refused, and only `Active`
/// [`ActorKey::Companion`] records are selectable. Returns the selections
/// with the envelope count; the caller derives `rejected` as the ignored
/// remainder.
pub(crate) fn select(view: &AuthorityReadView<'_>) -> (Vec<(CompanionId, CompanionAction)>, usize) {
    let envelopes = view.companion_actions();
    let examined = envelopes.len();
    let mut selected = Vec::new();
    for envelope in envelopes {
        let id = envelope.companion_id;
        if selected.iter().any(|(known, _)| *known == id) {
            continue;
        }
        if !valid_action(&envelope.action) {
            continue;
        }
        match view.actor(ActorKey::Companion(id)) {
            Some(record) if record.lifecycle == ActorLifecycle::Active => {}
            _ => continue,
        }
        selected.push((id, envelope.action.clone()));
    }
    (selected, examined)
}

/// Intent validation: count the arrival-order selections without staging
/// anything, so ignored envelopes leave no effects and no staged state.
fn run_intent(ctx: &mut TickContext<'_>) -> Result<PhaseReport, ServerError> {
    let view = ctx.read();
    let (selected, examined) = select(&view);
    let applied = selected.len();
    Ok(PhaseReport {
        examined,
        applied,
        carried: 0,
        rejected: examined - applied,
    })
}

/// Motion advance (`advanceActiveCompanions`): every active companion steps
/// once through the shared kernels in ID byte order. A selected `Move`
/// steers with its components, jump passthrough and normalized yaw; any
/// other selection or none steps neutral input retaining the current yaw,
/// following the player-motion neutral convention. Inputs never persist:
/// the selection is recomputed from the view and only stepped actor records
/// are staged.
fn run_motion(ctx: &mut TickContext<'_>) -> Result<PhaseReport, ServerError> {
    let tunables = ctx
        .read()
        .environment()
        .cloned()
        .ok_or(ServerError::Internal {
            invariant: "companion motion snapshot",
        })?
        .tunables;
    let tuning: PhysicsTuning = tunables.physics();
    let view = ctx.read();
    let (selected, _) = select(&view);
    let mut companions: Vec<(ActorKey, ActorRecord)> = view
        .actors()
        .iter()
        .filter(|record| {
            record.lifecycle == ActorLifecycle::Active
                && matches!(record.key, ActorKey::Companion(_))
        })
        .map(|record| (record.key, record.clone()))
        .collect();
    companions.sort_by_key(|(key, _)| match key {
        ActorKey::Companion(id) => id.bytes(),
        _ => [u8::MAX; 16],
    });
    let mut stepped = 0;
    for (key, record) in companions {
        let action = selected
            .iter()
            .find(|(id, _)| ActorKey::Companion(*id) == key)
            .map(|(_, action)| action);
        step_companion(ctx, key, &record, action, tunables.eye_height(), tuning)?;
        stepped += 1;
    }
    Ok(PhaseReport {
        examined: stepped,
        applied: stepped,
        carried: 0,
        rejected: 0,
    })
}

/// One authoritative physics step for a companion, mirroring the player
/// advance shape: finite-pose gate, tick-start tunables, the kernel call
/// over the staged grid, and the advanced actor record. A `Move` selection
/// also turns the staged facing to the normalized action yaw; a neutral
/// step keeps the current look.
fn step_companion(
    ctx: &mut TickContext<'_>,
    actor: ActorKey,
    record: &ActorRecord,
    action: Option<&CompanionAction>,
    eye_height: f32,
    tuning: PhysicsTuning,
) -> Result<(), ServerError> {
    let (move_x, move_z, jump, yaw) = match action {
        Some(CompanionAction::Move {
            move_x,
            move_z,
            jump,
            yaw,
        }) => (*move_x, *move_z, *jump, normalize_yaw(*yaw)),
        _ => (0, 0, false, record.look.yaw()),
    };
    let position = record.motion.position().get();
    let velocity = record.motion.velocity().get();
    let on_ground = record.motion.on_ground();
    // Explicit pose rule mirror (`ValidState` in
    // `packages/shared/physics/types.go`): a non-finite position or velocity
    // refuses as invalid input with nothing staged, keeping a future
    // relaxation from reaching the kernel path.
    if !position
        .iter()
        .chain(&velocity)
        .all(|component| component.is_finite())
    {
        return Err(ServerError::InvalidInput { field: "actor" });
    }
    let dimension = record.dimension;
    let step = HeldStep {
        move_x,
        move_z,
        jump,
        sprinting: false,
        sneaking: false,
        yaw_sin: f64::from(yaw).sin() as f32,
        yaw_cos: f64::from(yaw).cos() as f32,
    };
    let view = ctx.read();
    let (body_in_fluid, _) = submersion_flags(&view, dimension, position, eye_height)?;
    let (sweep_min, sweep_max) = sweep_bounds(velocity, on_ground, step, body_in_fluid, tuning);
    let (origin, dimensions) = step_prism(position, sweep_min, sweep_max, tuning.step_height)?;
    let view = ctx.read();
    let cells = prism_cells(&view, dimension, origin, dimensions)?;
    let grid =
        CollisionGrid::try_new(origin, dimensions, &cells).map_err(|_| ServerError::Internal {
            invariant: "companion motion grid",
        })?;
    let stepped = NativePhysics
        .step(&PhysicsRequest {
            state: PhysicsState {
                position,
                velocity,
                on_ground,
            },
            controls: PhysicsControls {
                move_x: step.move_x,
                move_z: step.move_z,
                jump: step.jump,
                yaw_sin: step.yaw_sin,
                yaw_cos: step.yaw_cos,
                body_in_fluid,
                sprinting: step.sprinting,
                sneaking: step.sneaking,
            },
            tuning,
            sweep: SweepBounds {
                minimum: sweep_min,
                maximum: sweep_max,
            },
            grid,
        })
        .map_err(|_| ServerError::Internal {
            invariant: "companion motion step",
        })?;
    let motion = MotionState::new(MotionStateParts {
        position: FiniteVec3::try_new(stepped.state.position).map_err(|_| {
            ServerError::Internal {
                invariant: "companion motion step",
            }
        })?,
        velocity: FiniteVec3::try_new(stepped.state.velocity).map_err(|_| {
            ServerError::Internal {
                invariant: "companion motion step",
            }
        })?,
        on_ground: stepped.state.on_ground,
    });
    let look =
        LookAngles::try_new(yaw, record.look.pitch()).map_err(|_| ServerError::Internal {
            invariant: "companion motion staging",
        })?;
    let advanced = ActorRecord::try_new(
        actor,
        record.lifecycle,
        dimension,
        motion,
        look,
        record.survival,
        record.body.clone(),
    )
    .map_err(|_| ServerError::Internal {
        invariant: "companion motion staging",
    })?;
    ctx.stage(RuleEffect::Actor(advanced))
        .map_err(|_| ServerError::Internal {
            invariant: "companion motion staging",
        })
}

/// Placement settle (`settleCompanionPlacements`): selected `Place` intents
/// commit in companion-ID byte order through the shared transaction, so
/// competing claims resolve first-commit-wins through the ordinary revision
/// advance. Every refusal (stale, unobserved, consumed, missing item,
/// competing write) stages nothing and counts `rejected`; no failure entry
/// is staged anywhere.
fn run_placement(ctx: &mut TickContext<'_>) -> Result<PhaseReport, ServerError> {
    let view = ctx.read();
    let (selected, _) = select(&view);
    let mut proposals: Vec<(CompanionId, BlockPos, u16)> = selected
        .iter()
        .filter_map(|(id, action)| match action {
            CompanionAction::Place { target, block } => Some((*id, *target, *block)),
            _ => None,
        })
        .collect();
    proposals.sort_by_key(|(id, _, _)| id.bytes());
    let mut applied = 0;
    let mut rejected = 0;
    for (id, target, block) in proposals {
        let resolved = match resolve_companion_place(id, target, block, &ctx.read()) {
            Ok(resolved) => resolved,
            Err(_) => {
                rejected += 1;
                continue;
            }
        };
        match ctx.transaction().try_place(resolved) {
            Ok(_) => applied += 1,
            Err(_) => rejected += 1,
        }
    }
    Ok(PhaseReport {
        examined: applied + rejected,
        applied,
        carried: 0,
        rejected,
    })
}

/// Yaw normalization mirror (`normalizeYaw` in
/// `packages/server/sim/entity/placement.go`): float64 remainder into
/// [-pi, pi), the shared `atan2(-x, -z)` facing convention.
fn normalize_yaw(yaw: f32) -> f32 {
    let mut normalized = (f64::from(yaw) + std::f64::consts::PI) % (2.0 * std::f64::consts::PI);
    if normalized < 0.0 {
        normalized += 2.0 * std::f64::consts::PI;
    }
    (normalized - std::f64::consts::PI) as f32
}

// -- Kernel-facing mirrors (`packages/shared/physics/step.go`,
// `collision.go`, `submersion.go`), identical in shape to the accepted
// player-motion plumbing because the Go companion advance runs the same
// `StepWithTunables`.

/// One step's resolved intent (`StepWithTunables` input projection).
#[derive(Clone, Copy)]
struct HeldStep {
    move_x: i8,
    move_z: i8,
    jump: bool,
    sprinting: bool,
    sneaking: bool,
    yaw_sin: f32,
    yaw_cos: f32,
}

fn is_fluid(block: u16) -> bool {
    (FLUID_FIRST..=FLUID_LAST).contains(&block)
}

/// Body immersion mirror (`SubmersionFlagsWithTunables` in
/// `packages/shared/physics/submersion.go`): the feet-centered box over staged
/// blocks with the snapshot eye height. Unobserved cells read as non-fluid.
fn submersion_flags(
    view: &AuthorityReadView<'_>,
    dimension: Dimension,
    position: [f32; 3],
    eye_height: f32,
) -> Result<(bool, bool), ServerError> {
    let eye_in_fluid = is_fluid_at(
        view,
        dimension,
        BlockPos::new(
            checked_floor(position[0])?,
            checked_floor(position[1] + eye_height)?,
            checked_floor(position[2])?,
        ),
    );
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
                    return Ok((true, eye_in_fluid));
                }
            }
        }
    }
    Ok((false, eye_in_fluid))
}

fn is_fluid_at(view: &AuthorityReadView<'_>, dimension: Dimension, pos: BlockPos) -> bool {
    view.observation(dimension, pos)
        .is_some_and(|observed| is_fluid(observed.block))
}

/// AABB upper-cell mirror (`fluidCellUpperBound`): ceil minus one so a box
/// merely touching a boundary claims no neighbor, clamped to at least one cell.
fn fluid_upper(maximum: f32, lower: i32) -> Result<i32, ServerError> {
    Ok((checked_ceil(maximum)? - 1).max(lower))
}

/// int32 span of the checked floor/ceil domain (`collisionCheckedFloor`).
const COORD_MIN: f64 = i32::MIN as f64;
const COORD_MAX: f64 = i32::MAX as f64;

fn checked_floor(value: f32) -> Result<i32, ServerError> {
    let floored = f64::from(value).floor();
    if !floored.is_finite() || !(COORD_MIN..=COORD_MAX).contains(&floored) {
        return Err(ServerError::InvalidInput { field: "actor" });
    }
    Ok(floored as i32)
}

fn checked_ceil(value: f32) -> Result<i32, ServerError> {
    let ceiling = f64::from(value).ceil();
    if !ceiling.is_finite() || !(COORD_MIN..=COORD_MAX).contains(&ceiling) {
        return Err(ServerError::InvalidInput { field: "actor" });
    }
    Ok(ceiling as i32)
}

/// Sweep hull mirror (`stepSweepBounds`): the integrated displacement's convex
/// bound with the exact Go operation order, including the fused vector length
/// and the two-stage target scaling.
fn sweep_bounds(
    velocity: [f32; 3],
    on_ground: bool,
    step: HeldStep,
    body_in_fluid: bool,
    tuning: PhysicsTuning,
) -> ([f32; 3], [f32; 3]) {
    let dt = tuning.fixed_delta_seconds;
    let mut walk_speed = tuning.walk_speed;
    if step.sneaking && on_ground && !body_in_fluid {
        walk_speed *= tuning.sneak_speed_multiplier;
    } else if step.sprinting && step.move_z > 0 && on_ground && !body_in_fluid {
        walk_speed *= tuning.sprint_speed_multiplier;
    }
    let target = movement_target(
        step.move_x,
        step.move_z,
        walk_speed,
        step.yaw_sin,
        step.yaw_cos,
    );
    let mut horizontal = [velocity[0], 0.0, velocity[2]];
    if on_ground {
        if step_vector_length(target) == 0.0 {
            horizontal = move_toward(horizontal, [0.0; 3], tuning.ground_deceleration * dt);
        } else {
            horizontal = move_toward(horizontal, target, tuning.ground_acceleration * dt);
        }
    } else {
        horizontal = move_toward(horizontal, target, tuning.air_acceleration * dt);
        let length = step_vector_length(horizontal);
        if length > tuning.walk_speed {
            let inverse = 1.0 / length;
            horizontal = [
                horizontal[0] * inverse * tuning.walk_speed,
                horizontal[1] * inverse * tuning.walk_speed,
                horizontal[2] * inverse * tuning.walk_speed,
            ];
        }
    }
    if body_in_fluid {
        horizontal = [
            horizontal[0] * tuning.fluid_horizontal_drag,
            horizontal[1] * tuning.fluid_horizontal_drag,
            horizontal[2] * tuning.fluid_horizontal_drag,
        ];
    }
    let (tx, tz) = (horizontal[0], horizontal[2]);
    let minimum = [
        min3(0.0, velocity[0], tx) * dt,
        sweep_vertical_min(velocity[1], step.jump, on_ground, body_in_fluid, tuning, dt),
        min3(0.0, velocity[2], tz) * dt,
    ];
    let maximum = [
        max3(0.0, velocity[0], tx) * dt,
        sweep_vertical_max(velocity[1], step.jump, on_ground, body_in_fluid, tuning, dt),
        max3(0.0, velocity[2], tz) * dt,
    ];
    (minimum, maximum)
}

fn sweep_vertical_min(
    velocity_y: f32,
    jump: bool,
    on_ground: bool,
    body_in_fluid: bool,
    tuning: PhysicsTuning,
    dt: f32,
) -> f32 {
    if body_in_fluid && jump {
        return 0.0f32.min(tuning.fluid_ascend_speed * dt);
    }
    if on_ground && jump {
        return 0.0;
    }
    let (gravity, terminal) = if body_in_fluid {
        (tuning.fluid_gravity, tuning.fluid_sink_speed)
    } else {
        (tuning.gravity, tuning.terminal_fall_speed)
    };
    let fallen = velocity_y - gravity * dt;
    if fallen >= -terminal {
        min3(0.0, velocity_y, fallen) * dt
    } else {
        min3(0.0, velocity_y, -terminal) * dt
    }
}

fn sweep_vertical_max(
    velocity_y: f32,
    jump: bool,
    on_ground: bool,
    body_in_fluid: bool,
    tuning: PhysicsTuning,
    dt: f32,
) -> f32 {
    if body_in_fluid && jump {
        return 0.0f32.max(tuning.fluid_ascend_speed * dt);
    }
    if on_ground && jump {
        return tuning.jump_speed * dt;
    }
    let (gravity, terminal) = if body_in_fluid {
        (tuning.fluid_gravity, tuning.fluid_sink_speed)
    } else {
        (tuning.gravity, tuning.terminal_fall_speed)
    };
    let fallen = velocity_y - gravity * dt;
    if fallen >= -terminal {
        max3(0.0, velocity_y, fallen) * dt
    } else {
        max3(0.0, velocity_y, -terminal) * dt
    }
}

/// Yaw-relative movement target (`movementTargetFromYaw`) with the two-stage
/// normalization the Go row performs.
fn movement_target(
    move_x: i8,
    move_z: i8,
    walk_speed: f32,
    yaw_sin: f32,
    yaw_cos: f32,
) -> [f32; 3] {
    let forward = [-yaw_sin, 0.0, -yaw_cos];
    let right = [yaw_cos, 0.0, -yaw_sin];
    let intent = [
        right[0] * f32::from(move_x) + forward[0] * f32::from(move_z),
        right[1] * f32::from(move_x) + forward[1] * f32::from(move_z),
        right[2] * f32::from(move_x) + forward[2] * f32::from(move_z),
    ];
    let length = step_vector_length(intent);
    if length == 0.0 {
        return [0.0; 3];
    }
    let inverse = 1.0 / length;
    [
        intent[0] * inverse * walk_speed,
        intent[1] * inverse * walk_speed,
        intent[2] * inverse * walk_speed,
    ]
}

/// Fused vector length (`stepVectorLength`).
fn step_vector_length(v: [f32; 3]) -> f32 {
    let inner = f64::mul_add(f64::from(v[0]), f64::from(v[0]), f64::from(v[1] * v[1])) as f32;
    let sum = f64::mul_add(f64::from(v[2]), f64::from(v[2]), f64::from(inner)) as f32;
    f64::from(sum).sqrt() as f32
}

/// Approach mirror (`moveToward`, `packages/shared/physics/motion.go`).
fn move_toward(current: [f32; 3], target: [f32; 3], maximum_delta: f32) -> [f32; 3] {
    let delta = [
        target[0] - current[0],
        target[1] - current[1],
        target[2] - current[2],
    ];
    let length = step_vector_length(delta);
    if length <= maximum_delta {
        return target;
    }
    let scale = maximum_delta / length;
    [
        current[0] + delta[0] * scale,
        current[1] + delta[1] * scale,
        current[2] + delta[2] * scale,
    ]
}

fn min3(a: f32, b: f32, c: f32) -> f32 {
    a.min(b.min(c))
}

fn max3(a: f32, b: f32, c: f32) -> f32 {
    a.max(b.max(c))
}

/// Prism mirror (`stepPrismFor`): the sweep hull padded by the body box, probe
/// and epsilon, floored to cells; an over-cap prism refuses without effect.
fn step_prism(
    position: [f32; 3],
    sweep_min: [f32; 3],
    sweep_max: [f32; 3],
    step_height: f32,
) -> Result<([i32; 3], [u32; 3]), ServerError> {
    let minimum = [
        position[0] + sweep_min[0] - HALF_WIDTH - COLLISION_EPSILON,
        position[1] + 0.0f32.min(sweep_min[1]).min(step_height) - GROUND_PROBE - COLLISION_EPSILON,
        position[2] + sweep_min[2] - HALF_WIDTH - COLLISION_EPSILON,
    ];
    let maximum = [
        position[0] + sweep_max[0] + HALF_WIDTH + COLLISION_EPSILON,
        position[1] + 0.0f32.max(sweep_max[1]).max(step_height) + PLAYER_HEIGHT + COLLISION_EPSILON,
        position[2] + sweep_max[2] + HALF_WIDTH + COLLISION_EPSILON,
    ];
    let origin = [
        checked_floor(minimum[0])?,
        checked_floor(minimum[1])?,
        checked_floor(minimum[2])?,
    ];
    let end = [
        checked_floor(maximum[0])?,
        checked_floor(maximum[1])?,
        checked_floor(maximum[2])?,
    ];
    let mut dimensions = [0u32; 3];
    let mut cells: u64 = 1;
    for axis in 0..3 {
        let span = i64::from(end[axis]) - i64::from(origin[axis]) + 1;
        if span <= 0 || span > u64::from(u32::MAX) as i64 {
            return Err(ServerError::InvalidInput { field: "actor" });
        }
        if i64::from(origin[axis]) + span - 1 > i64::from(i32::MAX) {
            return Err(ServerError::InvalidInput { field: "actor" });
        }
        dimensions[axis] = span as u32;
        cells *= span as u64;
        if cells > PRISM_MAX_CELLS {
            return Err(ServerError::InvalidInput { field: "actor" });
        }
    }
    Ok((origin, dimensions))
}

/// Grid mirror (`encodeStepInput` cell loop, y/x/z order): staged blocks map
/// through the collision shapes while unobserved cells stay unloaded, the
/// kernel's unknown-as-blocking policy.
/// Managed height planes are air; unavailable in-height and disabled cells stay blocking.
fn prism_cells(
    view: &AuthorityReadView<'_>,
    dimension: Dimension,
    origin: [i32; 3],
    dimensions: [u32; 3],
) -> Result<Vec<CollisionCell>, ServerError> {
    let total = u64::from(dimensions[0]) * u64::from(dimensions[1]) * u64::from(dimensions[2]);
    let mut cells = Vec::new();
    cells
        .try_reserve_exact(total as usize)
        .map_err(|_| ServerError::InvalidInput { field: "actor" })?;
    for y in 0..dimensions[1] {
        for x in 0..dimensions[0] {
            for z in 0..dimensions[2] {
                let pos = BlockPos::new(
                    origin[0] + x as i32,
                    origin[1] + y as i32,
                    origin[2] + z as i32,
                );
                match view.live_collision_block(dimension, pos) {
                    None => cells.push(CollisionCell::default()),
                    Some(block) => cells.push(collision_cell(block)?),
                }
            }
        }
    }
    Ok(cells)
}

/// Per-block shape mirror (`BlockCollisionBoxes`,
/// `packages/shared/physics/types.go`): air, fluids, plants, torches, snow and
/// door uppers carry no box; beds, doors and farmland carry their reduced
/// boxes; everything else is a full cube.
fn collision_cell(block: u16) -> Result<CollisionCell, ServerError> {
    if block == AIR
        || is_fluid(block)
        || (WHEAT_FIRST..=WHEAT_LAST).contains(&block)
        || (POTATO_FIRST..=POTATO_LAST).contains(&block)
        || (CARROT_FIRST..=CARROT_LAST).contains(&block)
        || block == SHORT_GRASS
        || block == SAPLING
        || (TORCH_FIRST..=TORCH_LAST).contains(&block)
        || (SNOW_FIRST..=SNOW_LAST).contains(&block)
        || block == DOOR_UPPER
    {
        return empty_cell();
    }
    if (BED_FIRST..=BED_LAST).contains(&block) {
        return boxed_cell([[[0.0, 0.0, 0.0], [1.0, BED_TOP, 1.0]]]);
    }
    if (DOOR_LOWER_FIRST..=DOOR_LOWER_LAST).contains(&block) {
        let index = block - DOOR_LOWER_FIRST;
        let direction = index / 2;
        let open = index % 2 == 1;
        let thin = DOOR_THICKNESS;
        let wide = 1.0 - DOOR_THICKNESS;
        let shape = match (direction, open) {
            (0, false) | (1, true) => [[0.0, 0.0, wide], [1.0, 1.0, 1.0]],
            (1, false) | (2, true) => [[0.0, 0.0, 0.0], [thin, 1.0, 1.0]],
            (2, false) | (3, true) => [[0.0, 0.0, 0.0], [1.0, 1.0, thin]],
            _ => [[wide, 0.0, 0.0], [1.0, 1.0, 1.0]],
        };
        return boxed_cell([[shape[0], shape[1]]]);
    }
    if (FARMLAND_FIRST..=FARMLAND_LAST).contains(&block) {
        return boxed_cell([[[0.0, 0.0, 0.0], [1.0, FARMLAND_TOP, 1.0]]]);
    }
    boxed_cell([[[0.0, 0.0, 0.0], [1.0, 1.0, 1.0]]])
}

const EMPTY_BOX: Aabb = Aabb {
    minimum: [0.0; 3],
    maximum: [0.0; 3],
};

fn empty_cell() -> Result<CollisionCell, ServerError> {
    CollisionCell::try_new(true, [EMPTY_BOX; 8], 0).map_err(|_| ServerError::Internal {
        invariant: "companion motion grid",
    })
}

fn boxed_cell(local: [[[f32; 3]; 2]; 1]) -> Result<CollisionCell, ServerError> {
    let mut boxes = [EMPTY_BOX; 8];
    boxes[0] = Aabb {
        minimum: local[0][0],
        maximum: local[0][1],
    };
    CollisionCell::try_new(true, boxes, 1).map_err(|_| ServerError::Internal {
        invariant: "companion motion grid",
    })
}
