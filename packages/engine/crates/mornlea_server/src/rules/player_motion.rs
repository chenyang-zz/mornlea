//! Player motion authority: validated intake, kernel advance, held staging.
//!
//! This provider owns two calls: the `PlayerCommand`-phase control intake for
//! `PlayerInput` commands, and the per-actor `PlayerMotion`-phase advance. The
//! intake validates one envelope, commits held controls and look before action
//! advancement, and defers it to the motion phase through [`TickContext::defer`].
//! The advance resolves the latest deferred envelope per session and steps the
//! actor through the accepted F1 kernels. A tick without a new envelope retains
//! the runtime's held controls. Intake order
//! is arrival order, so the first envelope kept at a tied sequence is the
//! earliest arrival, matching the ordering layer's tie rule (`order_commands`
//! in `mornlea_domain`).
//!
//! Mirrored Go rows, each cited at its site:
//!
//! - `packages/server/sim/entity/placement.go` (`validPlayerInput`,
//!   `validPlayerLook`, `normalizeYaw`): authority input range -1..=1 per move
//!   axis, finite yaw, finite pitch inside +/-(pi/2 - 0.01).
//! - `packages/server/sim/entity/tick.go` (`ApplyPlayerCommands`): the intake
//!   admits only active players, records the sequence before validating, and an
//!   invalid input zeroes movement, keeps yaw, and clears held bits.
//! - `packages/server/sim/runtime/engine_step.go`: the ordering layer admits a
//!   command only above the session's last admitted sequence; the silent
//!   stale/duplicate skip surfaces here as the rejected report.
//! - `packages/shared/physics/step.go` (`StepWithTunables`, `stepSweepBounds`,
//!   `stepPrismFor`, `encodeStepInput`): the fixed-step snapshot discipline,
//!   the sweep convex hull, and the collision prism this provider rebuilds
//!   natively before calling the kernel.
//! - `packages/shared/physics/types.go` (`BlockCollisionBoxes`,
//!   `PlayerBounds`): the per-block collision shapes and the feet-centered
//!   player box used for the grid and the submersion scan.
//! - `packages/shared/physics/submersion.go`
//!   (`SubmersionFlagsWithTunables`): body immersion derived from the staged
//!   blocks; unobserved cells read as non-fluid, so immersion is never invented
//!   from missing data.
//!
//! The advance runs the accepted F1 kernels: [`NativePhysics`] steps one fixed
//! step at the snapshot `fixed_delta_seconds` (0.05 at source defaults) while
//! `NativeCollision` resolves against the staged grid, where unobserved cells
//! stay unloaded so the kernel treats them as blocking. Tunables
//! ([`RuleTunables`]) are snapshotted from the staged environment once per
//! motion call and never reread mid-tick. Publication motion is the staged
//! kernel output itself, never a prediction, and this provider emits no events.
//!
//! Held controls live in
//! [`ActorRuntime::controls`](crate::core::contracts::ActorRuntime::controls) for the
//! Interaction-phase sneak gate: this provider is the single writer, staging
//! the resolved control (`Some` for the latest validated input, `None` once
//! cleared or never held) during intake and on every successful advance. Invalid
//! input also interrupts eating, bow and mining progress before action providers
//! run. The remaining runtime fields retain the current record unchanged
//! because the shared staging surface replaces the whole record. A first
//! control call uses the survival
//! initializer to preserve the saved hunger and respawn defaults.
//!
//! Oxygen, fall damage and exhaustion belong to the survival provider;
//! trample and snow-footprint collection belong to the crops provider.
//! Sneak-edge clamping and safe-location tracking still require separate
//! qualification at their motion and lifecycle boundaries.

use mornlea_domain::{
    BlockPos, Command, Dimension, FiniteVec3, LookAngles, MotionState, MotionStateParts,
    PlayerControl,
};
use mornlea_engine::native::contracts::collision::{Aabb, CollisionCell, CollisionGrid};
use mornlea_engine::native::contracts::physics::{
    PhysicsControls, PhysicsOp, PhysicsRequest, PhysicsState, PhysicsTuning, SweepBounds,
};
use mornlea_engine::native::physics::NativePhysics;

use crate::core::contracts::{
    ActorKey, ActorLifecycle, ActorRecord, PhaseReport, RuleCall, RuleEffect, RulePhase,
    ServerError, SessionKey,
};
use crate::core::state::{AuthorityReadView, TickContext};
use crate::rules::player_survival::merged_runtime;

/// Pitch bound mirror of `validPlayerLook` in
/// `packages/server/sim/entity/placement.go`: `float32(math.Pi/2 - 0.01)`.
const MAX_PITCH: f32 = (std::f64::consts::PI / 2.0 - 0.01) as f32;

/// Player half width (`PlayerWidth / 2`, `packages/shared/physics/types.go`).
const HALF_WIDTH: f32 = 0.3;
/// Player height (`PlayerHeight`).
const PLAYER_HEIGHT: f32 = 1.8;
/// Sweep/prism padding (`CollisionEpsilon`).
const COLLISION_EPSILON: f32 = 1e-5;
/// Ground support probe (`GroundProbe`).
const GROUND_PROBE: f32 = 1e-4;

/// Air never collides (`core.AirID`, `packages/shared/core/block.go`).
const AIR: u16 = 0;
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

/// Settles one `PlayerInput` envelope or advances one active player, exactly
/// one of which the call shape names.
///
/// Intake (`RulePhase::PlayerCommand` with a command and no actor) admits the
/// envelope only above the session's last deferred sequence, then validates
/// the control: a valid envelope stages controls and look, then reports applied;
/// a stale or duplicate envelope reports rejected with nothing deferred, and an invalid
/// envelope defers its tombstone, clears controls and action progress, and
/// returns [`ServerError::InvalidInput`]. This admitted cleanup keeps pose and
/// look unchanged. Malformed shapes and missing sessions stage nothing.
///
/// Advance (`RulePhase::PlayerMotion` with a player actor and no command)
/// snapshots the tick-start tunables, resolves the latest deferred control for
/// the session (retaining held input when absent), steps the F1 kernels, and
/// stages the advanced actor plus the held-controls runtime record. Any other
/// shape refuses without effect.
pub fn run(ctx: &mut TickContext<'_>, call: RuleCall<'_>) -> Result<PhaseReport, ServerError> {
    match call.phase {
        RulePhase::PlayerCommand => run_intake(ctx, &call),
        RulePhase::PlayerMotion => run_motion(ctx, &call),
        _ => Err(ServerError::InvalidInput { field: "phase" }),
    }
}

/// One intake: validate a `PlayerInput` envelope and commit controls and look
/// before action advancement, mirroring `ApplyPlayerCommands`
/// (`packages/server/sim/entity/tick.go`).
fn run_intake(ctx: &mut TickContext<'_>, call: &RuleCall<'_>) -> Result<PhaseReport, ServerError> {
    if call.actor.is_some() || call.internal.is_some() {
        return Err(ServerError::InvalidInput { field: "phase" });
    }
    let envelope = call
        .command
        .ok_or(ServerError::InvalidInput { field: "command" })?;
    let control = match envelope.command() {
        Command::PlayerInput(control) => control,
        _ => return Err(ServerError::InvalidInput { field: "command" }),
    };
    let session = SessionKey::from_raw(envelope.session())
        .ok_or(ServerError::InvalidInput { field: "session" })?;
    let actor = ActorKey::Player(session);
    let mut record = match ctx.read().actor(actor) {
        Some(record) if record.lifecycle == ActorLifecycle::Active => record.clone(),
        _ => return Err(ServerError::InvalidInput { field: "session" }),
    };
    if let Some(latest) = latest_deferred(ctx, session)
        && latest.sequence() >= envelope.sequence()
    {
        return Ok(PhaseReport {
            examined: 1,
            applied: 0,
            carried: 0,
            rejected: 1,
        });
    }
    // Reserve the admitted sequence before changing state. An invalid latest
    // is a tombstone, not a release: clear draw progress before BowDraw can
    // interpret the cleared primary bit as firing an arrow.
    ctx.defer(*envelope, RulePhase::PlayerMotion)?;
    let mut runtime = merged_runtime(&ctx.read(), &record)?;
    if !valid_control(control) {
        runtime.controls = None;
        runtime.eating = None;
        runtime.bow = None;
        ctx.stage(RuleEffect::Compound(vec![
            RuleEffect::Runtime(runtime),
            RuleEffect::Mining {
                actor,
                progress: None,
            },
        ]))
        .map_err(|_| ServerError::Internal {
            invariant: "player intake staging",
        })?;
        return Err(ServerError::InvalidInput { field: "control" });
    }
    runtime.controls = Some(control);
    record.look = LookAngles::try_new(normalize_yaw(control.look().yaw()), control.look().pitch())
        .map_err(|_| ServerError::Internal {
            invariant: "player intake look",
        })?;
    ctx.stage(RuleEffect::Compound(vec![
        RuleEffect::Actor(record),
        RuleEffect::Runtime(runtime),
    ]))
    .map_err(|_| ServerError::Internal {
        invariant: "player intake staging",
    })?;
    Ok(PhaseReport {
        examined: 1,
        applied: 1,
        carried: 0,
        rejected: 0,
    })
}

/// One advance: step the latest held control through the F1 kernels and stage
/// the result, mirroring the player leg of the Go movement advance
/// (`packages/server/sim/entity/player.go`) minus the survival-adjacent
/// settlement the module docs assign to later nodes.
fn run_motion(ctx: &mut TickContext<'_>, call: &RuleCall<'_>) -> Result<PhaseReport, ServerError> {
    if call.command.is_some() || call.internal.is_some() {
        return Err(ServerError::InvalidInput { field: "phase" });
    }
    let actor = call
        .actor
        .ok_or(ServerError::InvalidInput { field: "actor" })?;
    let ActorKey::Player(session) = actor else {
        return Err(ServerError::InvalidInput { field: "actor" });
    };
    // The tick-start snapshot names every tunable this step consumes; a
    // missing environment is a reducer sequencing bug, refused before any
    // staging so a failed advance leaves staged state untouched.
    let tunables = ctx
        .read()
        .environment()
        .cloned()
        .ok_or(ServerError::Internal {
            invariant: "player motion snapshot",
        })?
        .tunables;
    let record = ctx
        .read()
        .actor(actor)
        .cloned()
        .ok_or(ServerError::InvalidInput { field: "actor" })?;
    if record.lifecycle != ActorLifecycle::Active {
        return Err(ServerError::InvalidInput { field: "actor" });
    }
    let dimension = record.dimension;
    let position = record.motion.position().get();
    let velocity = record.motion.velocity().get();
    let on_ground = record.motion.on_ground();
    // Explicit pose rule mirror (`ValidState` in
    // `packages/shared/physics/types.go`): a non-finite position or velocity
    // refuses as invalid input with nothing staged. The domain rotation and
    // vector gates (`LookAngles`, `FiniteVec3`) already enforce finiteness at
    // construction and stay as defense-in-depth; this gate keeps a future
    // relaxation from reaching the kernel path, where it would surface as
    // Internal instead of the provider-level refusal the hash rule requires.
    if !position
        .iter()
        .chain(&velocity)
        .all(|component| component.is_finite())
    {
        return Err(ServerError::InvalidInput { field: "actor" });
    }

    let mut runtime = merged_runtime(&ctx.read(), &record)?;
    // Only a new envelope changes held intent: absence retains it, while an
    // invalid latest explicitly clears it (`ApplyPlayerCommands` in tick.go).
    let held = match latest_deferred(ctx, session) {
        Some(envelope) => match envelope.command() {
            Command::PlayerInput(control) if valid_control(control) => Some(control),
            _ => None,
        },
        None => runtime.controls,
    };
    if runtime.reset {
        // Reset actors still accept raw controls, but their pose and look must
        // not enter geometry or integration until the reset tick ends.
        runtime.controls = held;
        ctx.stage(RuleEffect::Runtime(runtime))
            .map_err(|_| ServerError::Internal {
                invariant: "player motion staging",
            })?;
        return Ok(PhaseReport {
            examined: 1,
            applied: 1,
            carried: 0,
            rejected: 0,
        });
    }
    let (yaw, pitch, move_x, move_z, jump, sprinting, sneaking) = match held {
        Some(control) => {
            let movement = control.movement();
            let actions = control.actions();
            (
                normalize_yaw(control.look().yaw()),
                control.look().pitch(),
                movement.move_x,
                movement.move_z,
                movement.jump,
                actions.sprinting,
                actions.sneaking,
            )
        }
        None => (
            record.look.yaw(),
            record.look.pitch(),
            0,
            0,
            false,
            false,
            false,
        ),
    };

    // Source gates change the local physics input, never the held record: a
    // packet-free tick can resume sprint after hunger recovers.
    let sprinting = sprinting && record.survival.hunger() >= 6 && !sneaking;
    let tuning: PhysicsTuning = tunables.physics();
    let step = HeldStep {
        move_x,
        move_z,
        jump,
        sprinting,
        sneaking,
        yaw_sin: f64::from(yaw).sin() as f32,
        yaw_cos: f64::from(yaw).cos() as f32,
    };
    let view = ctx.read();
    let (body_in_fluid, _) = submersion_flags(&view, dimension, position, tunables.eye_height())?;
    let (sweep_min, sweep_max) = sweep_bounds(velocity, on_ground, step, body_in_fluid, tuning);
    let (origin, dimensions) = step_prism(position, sweep_min, sweep_max, tuning.step_height)?;
    let cells = prism_cells(&view, dimension, origin, dimensions)?;
    let grid = CollisionGrid::try_new(origin, dimensions, &cells)
        .map_err(|_| ServerError::InvalidInput { field: "actor" })?;
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
            invariant: "player motion step",
        })?;

    let motion = MotionState::new(MotionStateParts {
        position: FiniteVec3::try_new(stepped.state.position).map_err(|_| {
            ServerError::Internal {
                invariant: "player motion step",
            }
        })?,
        velocity: FiniteVec3::try_new(stepped.state.velocity).map_err(|_| {
            ServerError::Internal {
                invariant: "player motion step",
            }
        })?,
        on_ground: stepped.state.on_ground,
    });
    let look = LookAngles::try_new(yaw, pitch).map_err(|_| ServerError::Internal {
        invariant: "player motion step",
    })?;
    let advanced = ActorRecord::try_new(
        actor,
        record.lifecycle,
        dimension,
        motion,
        look,
        record.survival,
        record.body,
    )
    .map_err(|_| ServerError::Internal {
        invariant: "player motion staging",
    })?;
    ctx.stage(RuleEffect::Actor(advanced))
        .map_err(|_| ServerError::Internal {
            invariant: "player motion staging",
        })?;
    // Movement owns only controls; whole-record replacement must preserve
    // every sibling provider's lanes, including actions staged earlier.
    runtime.controls = held;
    ctx.stage(RuleEffect::Runtime(runtime))
        .map_err(|_| ServerError::Internal {
            invariant: "player motion staging",
        })?;
    Ok(PhaseReport {
        examined: 1,
        applied: 1,
        carried: 0,
        rejected: 0,
    })
}

/// Latest deferred envelope for a session, first-kept on a sequence tie so
/// the earliest arrival wins, mirroring the ordering layer's tie rule.
fn latest_deferred(
    ctx: &TickContext<'_>,
    session: SessionKey,
) -> Option<mornlea_domain::CommandEnvelope> {
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
    latest
}

/// Authority input rule mirror (`validPlayerInput` in
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

/// Yaw normalization mirror (`normalizeYaw` in
/// `packages/server/sim/entity/placement.go`): float64 remainder, then into
/// [-pi, pi).
fn normalize_yaw(yaw: f32) -> f32 {
    let mut normalized = (f64::from(yaw) + std::f64::consts::PI) % (2.0 * std::f64::consts::PI);
    if normalized < 0.0 {
        normalized += 2.0 * std::f64::consts::PI;
    }
    (normalized - std::f64::consts::PI) as f32
}

/// Body immersion mirror (`SubmersionFlagsWithTunables` in
/// `packages/shared/physics/submersion.go`): the feet-centered box over staged
/// blocks with the snapshot eye height. Unobserved cells read as non-fluid.
/// The eye flag is computed for the mirror but only the body flag enters the
/// kernel: like the Go ABI, the eye bit is caller-side settlement data.
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

/// Fluid view mirror (`IsFluidAt` on the authority dimension): staged and
/// fluid, never invented from an unobserved cell.
fn is_fluid_at(view: &AuthorityReadView<'_>, dimension: Dimension, pos: BlockPos) -> bool {
    view.observation(dimension, pos)
        .is_some_and(|observed| (FLUID_FIRST..=FLUID_LAST).contains(&observed.block))
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

/// int32 span of the checked floor/ceil domain, mirroring the Go row
/// (`floored < -1<<31 || floored > 1<<31-1` in
/// `packages/shared/physics/collision.go`): the maximum admits only exactly
/// representable values, so the narrowing `as i32` below can never saturate.
const COORD_MIN: f64 = i32::MIN as f64;
const COORD_MAX: f64 = i32::MAX as f64;

/// Checked floor mirror (`collisionCheckedFloor` in
/// `packages/shared/physics/collision.go`): outside int32 the prism is
/// unrepresentable and the advance refuses without effect.
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

/// One step's resolved control intent: the kernel-facing projection of the
/// held input plus the precomputed yaw trigonometry the Go caller threads
/// through (`StepWithTunables` in `packages/shared/physics/step.go`).
#[derive(Clone, Copy, Debug)]
struct HeldStep {
    move_x: i8,
    move_z: i8,
    jump: bool,
    sprinting: bool,
    sneaking: bool,
    yaw_sin: f32,
    yaw_cos: f32,
}

/// Sweep hull mirror (`stepSweepBounds` in `packages/shared/physics/step.go`):
/// the integrated displacement's convex bound, which the kernel self-checks.
/// Operation order matches the Go row exactly, including the fused vector
/// length and the two-stage target scaling.
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

/// Vertical sweep branch mirror: fluid ascent assigns, ground jump reaches,
/// otherwise the gravity-fallen convex hull with the terminal clamp.
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

/// Vertical sweep upper mirror, same branch order as the lower.
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

/// Movement target mirror (`movementTargetFromYaw`): yaw-relative intent at
/// walk speed, with the two-stage normalization the Go row performs.
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

/// Fused vector length mirror (`stepVectorLength`): the explicit fused
/// boundaries keep the sweep envelope aligned with the Go row.
fn step_vector_length(v: [f32; 3]) -> f32 {
    let inner = f64::mul_add(f64::from(v[0]), f64::from(v[0]), f64::from(v[1] * v[1])) as f32;
    let sum = f64::mul_add(f64::from(v[2]), f64::from(v[2]), f64::from(inner)) as f32;
    f64::from(sum).sqrt() as f32
}

/// Approach mirror (`moveToward` in `packages/shared/physics/motion.go`).
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

/// Prism mirror (`stepPrismFor`): the sweep hull padded by the player box,
/// probe, and epsilon, floored to cells. An over-cap prism refuses without
/// effect instead of panicking like the Go row.
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
/// through the collision shapes while unobserved cells stay unloaded, which is
/// exactly the `NativeCollision` unknown-as-blocking policy.
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
                match view.observation(dimension, pos) {
                    None => cells.push(CollisionCell::default()),
                    Some(observed) => cells.push(collision_cell(observed.block)?),
                }
            }
        }
    }
    Ok(cells)
}

/// Per-block shape mirror (`BlockCollisionBoxes` in
/// `packages/shared/physics/types.go`): air, fluids, plants, torches, snow,
/// and door uppers carry no box; beds, doors, and farmland carry their reduced
/// boxes; everything else is a full cube. Local boxes; the kernel offsets by
/// the cell position.
fn collision_cell(block: u16) -> Result<CollisionCell, ServerError> {
    if block == AIR
        || (FLUID_FIRST..=FLUID_LAST).contains(&block)
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

/// Empty local box both helpers share.
const EMPTY_BOX: Aabb = Aabb {
    minimum: [0.0; 3],
    maximum: [0.0; 3],
};

/// Loaded cell with no boxes.
fn empty_cell() -> Result<CollisionCell, ServerError> {
    CollisionCell::try_new(true, [EMPTY_BOX; 8], 0).map_err(|_| ServerError::Internal {
        invariant: "player motion grid",
    })
}

/// Loaded cell with one local box.
fn boxed_cell(local: [[[f32; 3]; 2]; 1]) -> Result<CollisionCell, ServerError> {
    let mut boxes = [EMPTY_BOX; 8];
    boxes[0] = Aabb {
        minimum: local[0][0],
        maximum: local[0][1],
    };
    CollisionCell::try_new(true, boxes, 1).map_err(|_| ServerError::Internal {
        invariant: "player motion grid",
    })
}
