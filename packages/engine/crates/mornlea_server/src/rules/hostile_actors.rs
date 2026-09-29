//! Hostile lifecycle authority: night spawn admission, deterministic
//! targeting, kernel movement, and the burn/distant pass.
//!
//! This provider owns two batch calls: the `HostileMotion` phase (spawn
//! admission followed by the movement advance for the whole hostile set) and
//! the `HostileBurnDistant` phase (daylight burn followed by the distant
//! despawn counter). Any other call shape is refused without effect. The
//! hostile set is small and bounded (64 residents), so both phases examine
//! every hostile in ascending-id order and stay deterministic end to end.
//!
//! Mirrored Go rows, each cited at its site:
//!
//! - `packages/server/sim/entity/hostile.go`: the fixed numeric contract
//!   (`maxHostiles` 64, `hostileCooldownPeriodTicks` 20,
//!   `maxHostileDistantTicks` 600), the
//!   neutral-input movement advance (`advanceHostileMovement`), the daylight
//!   burn (`advanceHostileBurn`, `phaseIsDay`, `hostileSkyExposed`), the
//!   distant despawn (`advanceHostileDistant`, `hostileDistantRadius` 64),
//!   and the world-floor removal rule.
//! - `packages/server/sim/entity/hostile_spawn.go`: the night window
//!   13000..=23000 on the seasonally effective phase, the peaceful entry
//!   gate, the sorted-session `worldtime % len` anchor, the
//!   `SplitMix64(seed ^ time)` radius 24..=48 and +X/-X/+Z/-Z axis table,
//!   the candidate-hash low-byte < 13 gate, the `hash % 3` kind dispatch,
//!   the per-kind 48-block local caps (walker 8, hurler 4), the block-light
//!   limit 7, and the id rehash chain.
//! - `packages/server/sim/entity/block_light_query.go`: the 29^3 local block
//!   light field (emission seeds, opaque/unloaded blocking, fluid
//!   attenuation).
//! - `packages/server/sim/entity/hostile_action.go`: the per-tick neutral
//!   input reset and the shoot-cooldown decrement (firing itself belongs to
//!   the projectile node).
//! - `packages/server/server/hostile_manager.go`: the deterministic target
//!   selection (nearest active same-dimension player, equal distance taking
//!   the smaller `PlayerID` bytes), the 20-tick repath cadence, and the
//!   attack-range movement stop (`HostileAttackRange` 1.8 via
//!   `packages/server/sim/contract/contract.go`).
//! - `packages/shared/pathfind/pathfind.go` and
//!   `packages/server/server/companion_snapshot.go`: the 33x9x33 chase
//!   window, the standing-goal clamp, and the passable-block table shared
//!   with companion pathing.
//!
//! Pathing never reimplements search: the advance resolves goals through the
//! accepted F1 `NativePathfind` kernel over a staged-observation grid, and
//! every body displacement goes through `NativePhysics`, exactly like the
//! player motion provider. The path lives in [`ActorRuntime::path`] as
//! transient state (a restored hostile re-plans; nothing path-shaped is
//! persisted). Go revalidates path revisions when applying asynchronous A*
//! results; this provider computes and consumes the path within one
//! synchronous provider call, so the result can never be stale at use, and
//! the 20-tick replan bounds world drift afterwards.
//!
//! Deliberate boundaries (later nodes own them): combat intents and hit
//! settlement, projectile firing and stepping, death drops and the loot
//! ring. A hostile whose health reaches zero or whose distant counter
//! crosses 600 is staged with [`ActorLifecycle::Dead`]; the serial reducer
//! drops dead records from the authority set, which is this overlay's only
//! removal representation.

use mornlea_domain::{
    BlockPos, Dimension, FiniteVec3, LookAngles, MotionState, MotionStateParts, SurvivalState,
    SurvivalStateParts,
};
use mornlea_engine::native::contracts::collision::{Aabb, CollisionCell, CollisionGrid};
use mornlea_engine::native::contracts::pathfind::{
    PathBlockTable, PathCell, PathGrid, PathRevision, PathScratch, PathfindOp,
};
use mornlea_engine::native::contracts::physics::{
    PhysicsControls, PhysicsOp, PhysicsRequest, PhysicsState, PhysicsTuning, SweepBounds,
};
use mornlea_engine::native::pathfind::{NativePathfind, is_standing};
use mornlea_engine::native::physics::NativePhysics;
use mornlea_storage::{HostileMob, PlayerId};

use crate::core::contracts::{
    ActorAux, ActorBody, ActorKey, ActorLifecycle, ActorRecord, ActorRuntime, ChunkKey,
    EnvironmentState, PathState, PhaseReport, RuleCall, RuleEffect, RulePhase, ServerError,
    SessionKey,
};
use crate::core::state::{AuthorityReadView, TickContext};

// ---------------------------------------------------------------------------
// Frozen numeric contract (`hostile.go` const block and
// `hostile_spawn.go` const block). None of these scale with world or player
// counts; each mirrors its Go row exactly.
// ---------------------------------------------------------------------------

/// Global resident cap shared by both kinds (`maxHostiles`).
const MAX_HOSTILES: usize = 64;
/// Attack, hurt and burn timers share this period (`hostileCooldownPeriodTicks`).
const COOLDOWN_PERIOD_TICKS: u32 = 20;
/// Distant-despawn accumulation ceiling (`maxHostileDistantTicks`).
const MAX_DISTANT_TICKS: u16 = 600;
/// Night spawn window on the effective phase, both ends inclusive
/// (`hostileSpawnPhaseStart` / `hostileSpawnPhaseEnd`, the display night
/// window of `packages/shared/core/day_phase.go`).
const SPAWN_PHASE_START: u16 = 13000;
const SPAWN_PHASE_END: u16 = 23000;
/// Candidate-to-anchor horizontal distance window, inclusive
/// (`hostileSpawnMinRadius` / `hostileSpawnMaxRadius`).
const SPAWN_MIN_RADIUS: i64 = 24;
const SPAWN_MAX_RADIUS: i64 = 48;
/// Candidate hash low-byte gate (`hostileSpawnGateThreshold`): 13 of every
/// 256 low-byte values admit a candidate.
const SPAWN_GATE_THRESHOLD: u64 = 13;
/// Rehash budget for id conflicts (`hostileSpawnMaxRehashes`).
const SPAWN_MAX_REHASHES: usize = 64;
/// Candidate block-light darkness limit (`hostileSpawnLightLimit`).
const SPAWN_LIGHT_LIMIT: u8 = 7;
/// Per-kind caps inside the 48-block player radius (`maxHostilesNearPlayer`
/// / `maxHostilesNearHurler`); the caps never crowd each other out.
const NEAR_LIMIT_WALKER: usize = 8;
const NEAR_LIMIT_HURLER: usize = 4;
/// "Near a player" horizontal radius (`maxHostilesNearRadius`).
const NEAR_RADIUS: f32 = 48.0;
/// Distant-despawn horizontal boundary (`hostileDistantRadius`): > 64
/// accumulates, <= 64 resets, the boundary itself counts as in range.
const DISTANT_RADIUS: f32 = 64.0;
/// Full hostile health (`core.MaxHealth` in `packages/shared/core/health.go`).
const MAX_HEALTH: u8 = 20;
/// World height bounds (`core.MinY` / `core.MaxY` in
/// `packages/shared/core/pos.go`).
const WORLD_MIN_Y: i32 = -64;
const WORLD_MAX_Y: i32 = 320;

// Chase orchestration constants (`packages/server/server/hostile_manager.go`).

/// Periodic replan cadence after a successful path
/// (`hostileRepathPeriodTicks`); a failed attempt retries next tick.
const REPATH_PERIOD_TICKS: u64 = 20;
/// Movement stop boundary at the target (`HostileAttackRange` in
/// `packages/server/sim/contract/contract.go`).
const ATTACK_RANGE: f32 = 1.8;
/// Waypoint arrival threshold (`waypointArrivalRadiusSquared` in
/// `packages/server/server/companion_manager.go`, shared with companions).
const WAYPOINT_ARRIVAL_RADIUS_SQ: f32 = 0.35 * 0.35;
/// Path window radii (`PathWindowHorizontalRadius` /
/// `PathWindowVerticalRadius` in `packages/shared/pathfind/pathfind.go`):
/// the window is 33x9x33 cells.
const WINDOW_HORIZONTAL_RADIUS: i32 = 16;
const WINDOW_VERTICAL_RADIUS: i32 = 4;

// Body and light geometry (`packages/shared/physics/types.go`,
// `block_light_query.go`).

/// Hostile bodies share the player AABB.
const HALF_WIDTH: f32 = 0.3;
const PLAYER_HEIGHT: f32 = 1.8;
/// Local block-light query radius, inclusive, side 29
/// (`hostileLightRadius`).
const LIGHT_RADIUS: i32 = 14;
const LIGHT_SIDE: usize = (2 * LIGHT_RADIUS + 1) as usize;
const LIGHT_MAX_LEVEL: u8 = 15;

// Block id table (`packages/shared/core/block.go` protocol-stable order).

const AIR: u16 = 0;
const LIGHT_BLOCK: u16 = 12;
const LEAVES: u16 = 19;
const GLASS: u16 = 20;
const FLUID_FIRST: u16 = 27;
const FLUID_LAST: u16 = 34;
const FARMLAND_FIRST: u16 = 35;
const FARMLAND_LAST: u16 = 36;
const WHEAT_FIRST: u16 = 37;
const WHEAT_LAST: u16 = 44;
const POTATO_FIRST: u16 = 46;
const POTATO_LAST: u16 = 53;
const CARROT_FIRST: u16 = 54;
const CARROT_LAST: u16 = 61;
const DOOR_LOWER_FIRST: u16 = 62;
const DOOR_LOWER_LAST: u16 = 69;
const DOOR_UPPER: u16 = 70;
const TORCH_FIRST: u16 = 71;
const TORCH_LAST: u16 = 75;
const BED_FIRST: u16 = 76;
const BED_LAST: u16 = 83;
const SHORT_GRASS: u16 = 84;
const SNOW_FIRST: u16 = 85;
const SNOW_LAST: u16 = 88;
const SAPLING: u16 = 89;
/// Registered ceiling (`BlockIDMax`): sapling is the last registered id.
const BLOCK_ID_MAX: u16 = 90;

/// Sweep/prism padding and ground support probe
/// (`CollisionEpsilon` / `GroundProbe`).
const COLLISION_EPSILON: f32 = 1e-5;
const GROUND_PROBE: f32 = 1e-4;
/// Prism cell cap (`collisionMaxCells`).
const PRISM_MAX_CELLS: u64 = 4096;

/// Hostile kind bytes (`HostileKindNightwalker` / `HostileKindBoneThrower`).
const NIGHTWALKER: u8 = 0;
const HURLER: u8 = 1;

/// Settles one hostile batch: the `HostileMotion` phase (spawn admission then
/// movement) or the `HostileBurnDistant` phase (burn then distant despawn).
/// Both are batch calls; any other shape is refused without effect.
pub fn run(ctx: &mut TickContext<'_>, call: RuleCall<'_>) -> Result<PhaseReport, ServerError> {
    match call.phase {
        RulePhase::HostileMotion => batch(ctx, &call, hostile_motion),
        RulePhase::HostileBurnDistant => batch(ctx, &call, hostile_burn_distant),
        _ => Err(ServerError::InvalidInput { field: "phase" }),
    }
}

/// Enforces the batch call shape and runs one phase body.
fn batch(
    ctx: &mut TickContext<'_>,
    call: &RuleCall<'_>,
    phase: fn(&mut TickContext<'_>) -> Result<PhaseReport, ServerError>,
) -> Result<PhaseReport, ServerError> {
    if call.actor.is_some() || call.command.is_some() || call.internal.is_some() {
        return Err(ServerError::InvalidInput { field: "phase" });
    }
    phase(ctx)
}

/// One hostile's tick-time working copy: the durable body plus the runtime
/// transients this provider owns. Restored records (no staged runtime)
/// start with the body's durable timers and reset the transients, exactly
/// like Go's `RestoreHostile` leaving `shootCooldown` ready and `fresh`
/// unset.
struct HostileEntry {
    key: ActorKey,
    lifecycle: ActorLifecycle,
    dimension: Dimension,
    look: LookAngles,
    survival: SurvivalState,
    body: HostileMob,
    burn_cooldown: u32,
    distant_ticks: u16,
    shoot_cooldown: u32,
    fresh: bool,
    path: Option<PathState>,
    dirty: bool,
    dead: bool,
}

impl HostileEntry {
    /// Collects the tick-start working copy of one hostile actor.
    fn from_record(record: &ActorRecord, runtime: Option<&ActorRuntime>) -> Self {
        let ActorBody::Hostile(body) = &record.body else {
            // The actor/body pairing rule guarantees this never happens.
            return Self::dead_copy(record.key);
        };
        let (distant, shoot, fresh, path) = match runtime.map(|value| &value.aux) {
            Some(ActorAux::Hostile {
                distant_ticks,
                shoot_cooldown,
                fresh,
            }) => (
                *distant_ticks,
                *shoot_cooldown,
                *fresh,
                runtime.and_then(|value| value.path.clone()),
            ),
            _ => (body.distant_ticks, 0, false, None),
        };
        let burn = runtime
            .map(|value| value.burn_cooldown)
            .unwrap_or(u32::from(body.burn_cooldown));
        Self {
            key: record.key,
            lifecycle: record.lifecycle,
            dimension: record.dimension,
            look: record.look,
            survival: record.survival,
            body: body.clone(),
            burn_cooldown: burn,
            distant_ticks: distant,
            shoot_cooldown: shoot,
            fresh,
            path,
            dirty: false,
            dead: false,
        }
    }

    /// Placeholder for a body/key mismatch, which cannot pass
    /// `ActorRecord::try_new`; kept total for defense in depth.
    fn dead_copy(key: ActorKey) -> Self {
        Self {
            key,
            lifecycle: ActorLifecycle::Dead,
            dimension: Dimension::OVERWORLD,
            look: LookAngles::try_new(0.0, 0.0).expect("zero look"),
            survival: SurvivalState::try_new(SurvivalStateParts {
                health: 1,
                oxygen: 0,
                hunger: 20,
                saturation_zero: true,
                armor_points: 0,
            })
            .expect("survival"),
            body: HostileMob {
                id: 1,
                dimension: 0,
                position: [0.0; 3],
                velocity: [0.0; 3],
                on_ground: true,
                yaw: 0.0,
                health: 1,
                attack_cooldown: 0,
                hurt_cooldown: 0,
                burn_cooldown: 0,
                has_target: false,
                player_id: PlayerId::from_bytes([0u8; 16]),
                next_repath_ticks: 0,
                distant_ticks: 0,
                kind: 0,
            },
            burn_cooldown: 0,
            distant_ticks: 0,
            shoot_cooldown: 0,
            fresh: false,
            path: None,
            dirty: false,
            dead: true,
        }
    }

    fn id(&self) -> u64 {
        self.body.id
    }

    fn position(&self) -> [f32; 3] {
        self.body.position
    }

    fn kind(&self) -> u8 {
        self.body.kind
    }

    /// Marks the entry removed. The overlay has no deletion arm, so removal
    /// stages the `Dead` lifecycle and the serial reducer drops the record.
    fn remove(&mut self) {
        if self.lifecycle != ActorLifecycle::Dead {
            self.lifecycle = ActorLifecycle::Dead;
            self.dirty = true;
        }
        self.dead = true;
    }

    /// Rebuilds the staged actor record from the working copy.
    fn record(&self) -> Result<ActorRecord, ServerError> {
        let motion = MotionState::new(MotionStateParts {
            position: FiniteVec3::try_new(self.body.position).map_err(|_| {
                ServerError::Internal {
                    invariant: "hostile position",
                }
            })?,
            velocity: FiniteVec3::try_new(self.body.velocity).map_err(|_| {
                ServerError::Internal {
                    invariant: "hostile velocity",
                }
            })?,
            on_ground: self.body.on_ground,
        });
        let look = LookAngles::try_new(self.body.yaw, self.look.pitch()).map_err(|_| {
            ServerError::Internal {
                invariant: "hostile yaw",
            }
        })?;
        ActorRecord::try_new(
            self.key,
            self.lifecycle,
            self.dimension,
            motion,
            look,
            self.survival,
            ActorBody::Hostile(self.body.clone()),
        )
        .map_err(|_| ServerError::Internal {
            invariant: "hostile staging",
        })
    }

    /// The runtime record this provider stages for the serial reducer's
    /// per-field merge. Fields outside the hostile ownership set stage
    /// neutral, mirroring the player motion provider's contract.
    fn runtime(&self) -> ActorRuntime {
        ActorRuntime {
            key: self.key,
            controls: None,
            has_view: false,
            reset: false,
            attack_cooldown: 0,
            hurt_cooldown: 0,
            burn_cooldown: self.burn_cooldown,
            oxygen: 0,
            peak_y: 0.0,
            exhaustion_milli: 0,
            saturation_milli: 0,
            since_damage_ticks: 0,
            drown_ticks: 0,
            starvation_ticks: 0,
            eating: None,
            bow: None,
            path: self.path.clone(),
            aux: ActorAux::Hostile {
                distant_ticks: self.distant_ticks,
                shoot_cooldown: self.shoot_cooldown,
                fresh: self.fresh,
            },
        }
    }

    /// Stages the actor and runtime records when anything changed.
    fn stage(&mut self, ctx: &mut TickContext<'_>) -> Result<bool, ServerError> {
        if !self.dirty {
            return Ok(false);
        }
        self.dirty = false;
        ctx.stage(RuleEffect::Actor(self.record()?))
            .map_err(|_| ServerError::Internal {
                invariant: "hostile staging",
            })?;
        ctx.stage(RuleEffect::Runtime(self.runtime()))
            .map_err(|_| ServerError::Internal {
                invariant: "hostile runtime staging",
            })?;
        Ok(true)
    }
}

/// The `HostileMotion` batch: spawn admission first (one candidate), then the
/// movement advance in ascending-id order, mirroring `advanceHostiles`
/// (`hostile.go`) minus the action intents the combat node owns.
fn hostile_motion(ctx: &mut TickContext<'_>) -> Result<PhaseReport, ServerError> {
    let environment = ctx
        .read()
        .environment()
        .cloned()
        .ok_or(ServerError::Internal {
            invariant: "hostile environment snapshot",
        })?;
    let mut entries = collect_hostiles(ctx)?;
    let examined = entries.len();
    let mut applied = 0usize;

    // Spawn admission (`advanceHostileSpawn`): exactly one candidate is
    // derived and validated per tick; a refused candidate ends the attempt
    // for this tick.
    if let Some(spawned) = spawn_admission(ctx, &environment, &entries)? {
        // The fresh hostile joins the movement pass below, where its fresh
        // flag is consumed: spawn order runs before physics and a new body
        // moves from the next tick (`advanceHostileMovement` fresh rule).
        entries.push(spawned);
        entries.sort_by_key(|entry| entry.id());
    }

    // Movement advance (`advanceHostileMovement`): each body steps exactly
    // once, in id order, through the shared physics kernels.
    for entry in entries.iter_mut() {
        if entry.dead {
            continue;
        }
        if entry.fresh {
            // A just-spawned body skips its first movement tick; clearing
            // the flag is the skip's only observable effect.
            entry.fresh = false;
            entry.dirty = true;
            if entry.stage(ctx)? {
                applied += 1;
            }
            continue;
        }
        if advance_movement(ctx, &environment, entry)? {
            applied += 1;
        }
    }

    Ok(PhaseReport {
        examined: examined + 1,
        applied,
        carried: 0,
        rejected: 0,
    })
}

/// The `HostileBurnDistant` batch: daylight burn for the whole set, then the
/// distant-despawn counters, mirroring the Go orchestration order
/// (`advanceHostileBurn` before `advanceHostileDistant` so a burn death is
/// never preempted by a no-drop removal).
fn hostile_burn_distant(ctx: &mut TickContext<'_>) -> Result<PhaseReport, ServerError> {
    let environment = ctx
        .read()
        .environment()
        .cloned()
        .ok_or(ServerError::Internal {
            invariant: "hostile environment snapshot",
        })?;
    let mut entries = collect_hostiles(ctx)?;
    let examined = entries.len();
    let mut applied = 0usize;

    let phase = effective_day_phase_at(
        environment.world_time,
        environment.day_phase_offset,
        environment.season_offset,
    );
    // Burn window mirror (`phaseIsDay` in `hostile.go`): the sun is above
    // the horizon for effective phases 1..=11999 only.
    let is_day = (1..=11999).contains(&phase);
    let view = ctx.read();
    for entry in &mut entries {
        if entry.dead {
            continue;
        }
        let previous = (entry.burn_cooldown, entry.body.health);
        if !is_day {
            // Night or a warp-invalid phase resets every timer
            // (`resetHostileBurnTimers`).
            entry.burn_cooldown = COOLDOWN_PERIOD_TICKS;
        } else if sky_exposed(&view, entry.dimension, entry.body.position)? {
            // Exposed: the timer counts down and every zero deals one point
            // of burn damage before restarting (`advanceHostileBurn`).
            if entry.burn_cooldown > 0 {
                entry.burn_cooldown -= 1;
            }
            if entry.burn_cooldown == 0 {
                entry.burn_cooldown = COOLDOWN_PERIOD_TICKS;
                entry.body.health = entry.body.health.saturating_sub(1);
            }
        } else {
            // Roofed bodies reset exactly like night (`advanceHostileBurn`
            // covered branch).
            entry.burn_cooldown = COOLDOWN_PERIOD_TICKS;
        }
        if (entry.burn_cooldown, entry.body.health) != previous {
            entry.body.burn_cooldown = u8::try_from(entry.burn_cooldown).unwrap_or(u8::MAX);
            entry.dirty = true;
        }
    }

    // Distant pass (`advanceHostileDistant`): bodies that just burned to
    // zero are skipped so the death settlement node owns them; everyone
    // else accumulates out of range of every active same-dimension player
    // and resets inside the boundary. Health-zero bodies keep their counter
    // untouched.
    for entry in &mut entries {
        if entry.dead || entry.body.health == 0 {
            continue;
        }
        let within = player_within(
            ctx,
            entry.dimension,
            entry.body.position,
            DISTANT_RADIUS * DISTANT_RADIUS,
        );
        let previous = entry.distant_ticks;
        if within {
            entry.distant_ticks = 0;
        } else {
            entry.distant_ticks = entry.distant_ticks.saturating_add(1);
            if entry.distant_ticks >= MAX_DISTANT_TICKS {
                entry.remove();
            }
        }
        if entry.distant_ticks != previous {
            entry.body.distant_ticks = entry.distant_ticks;
            entry.dirty = true;
        }
    }

    for entry in &mut entries {
        if entry.stage(ctx)? {
            applied += 1;
        }
    }

    Ok(PhaseReport {
        examined,
        applied,
        carried: 0,
        rejected: 0,
    })
}

/// Collects active hostile actors in ascending-id order with their runtime
/// transients merged in.
fn collect_hostiles(ctx: &TickContext<'_>) -> Result<Vec<HostileEntry>, ServerError> {
    let view = ctx.read();
    let mut entries: Vec<HostileEntry> = view
        .actors()
        .iter()
        .filter(|actor| {
            matches!(actor.key, ActorKey::Hostile(_)) && actor.lifecycle == ActorLifecycle::Active
        })
        .map(|actor| HostileEntry::from_record(actor, view.runtime(actor.key)))
        .collect();
    entries.sort_by_key(|entry| entry.id());
    Ok(entries)
}

// ---------------------------------------------------------------------------
// Spawn admission (`advanceHostileSpawn` in `packages/server/sim/entity/
// hostile_spawn.go`). Every gate below mirrors the Go row order: cheap and
// world-free checks first, then anchor derivation, then the loaded-column
// and light reads.
// ---------------------------------------------------------------------------

fn spawn_admission(
    ctx: &mut TickContext<'_>,
    environment: &EnvironmentState,
    entries: &[HostileEntry],
) -> Result<Option<HostileEntry>, ServerError> {
    // Peaceful difficulty short-circuits at the entry, before any candidate
    // derivation or budget consumption.
    if environment.difficulty == 1 {
        return Ok(None);
    }
    let now = environment.world_time;
    let phase =
        effective_day_phase_at(now, environment.day_phase_offset, environment.season_offset);
    if !(SPAWN_PHASE_START..=SPAWN_PHASE_END).contains(&phase) {
        return Ok(None);
    }
    let players = active_players(&ctx.read())?;
    if players.is_empty() {
        return Ok(None);
    }
    // The global cap is the cheapest world-free gate after the window.
    if entries.len() >= MAX_HOSTILES {
        return Ok(None);
    }
    // The anchor player is the sorted-session pick at `worldtime % len`
    // (`TestHostileSpawnPicksAnchorBySortedSessionAndWorldTime`).
    let anchor = &players[(now % players.len() as u64) as usize];
    let anchor_cell = block_pos_of(anchor.position)?;
    // Candidate column: `SplitMix64(seed ^ time)` derives the radius
    // (24..=48 from the low bits) and one of the four horizontal axes
    // (+X, -X, +Z, -Z from bits 32..34).
    let base = splitmix64((environment.seed as u64) ^ now);
    let radius =
        SPAWN_MIN_RADIUS + (base % (SPAWN_MAX_RADIUS as u64 - SPAWN_MIN_RADIUS as u64 + 1)) as i64;
    let axis = ((base >> 32) & 3) as usize;
    let delta_x = [1i32, -1, 0, 0][axis];
    let delta_z = [0i32, 0, 1, -1][axis];
    let x = anchor_cell.x() + delta_x * radius as i32;
    let z = anchor_cell.z() + delta_z * radius as i32;

    // The candidate column must resolve to a standing spot on fully
    // observed data (`hostileSpawnColumnSpot`); unobserved cells refuse
    // rather than triggering any load.
    let view = ctx.read();
    let Some(y) = spawn_column_spot(&view, anchor.dimension, x, z)? else {
        return Ok(None);
    };
    // The candidate hash folds the coordinates into the seed-time chain
    // (`sampler.HostileCandidateHash`); its low byte carries the 13/256
    // gate and its residue carries the 2:1 kind dispatch.
    let hash = hostile_candidate_hash(environment.seed, now, x, y, z);
    if hash & 0xFF >= SPAWN_GATE_THRESHOLD {
        return Ok(None);
    }
    let kind = if hash.is_multiple_of(3) {
        HURLER
    } else {
        NIGHTWALKER
    };
    let candidate = [x as f32 + 0.5, y as f32, z as f32 + 0.5];
    // Per-kind local cap inside 48 blocks of any active player.
    if near_limit_exceeded(&players, entries, anchor.dimension, candidate, kind) {
        return Ok(None);
    }
    // The candidate cell must be dark: local block light at most 7.
    if local_block_light(&view, anchor.dimension, BlockPos::new(x, y, z))? > SPAWN_LIGHT_LIMIT {
        return Ok(None);
    }

    // The id is the candidate hash itself; a conflict rehashes along the
    // chain and gives up when the budget is exhausted.
    let mut id = hash;
    for _ in 0..SPAWN_MAX_REHASHES {
        let unique = id != 0 && !entries.iter().any(|entry| entry.id() == id);
        if unique {
            let body = HostileMob {
                id,
                dimension: i32::from(anchor.dimension.get()),
                position: candidate,
                velocity: [0.0; 3],
                on_ground: true,
                yaw: 0.0,
                health: MAX_HEALTH,
                attack_cooldown: 0,
                hurt_cooldown: 0,
                burn_cooldown: COOLDOWN_PERIOD_TICKS as u8,
                has_target: false,
                player_id: PlayerId::from_bytes([0u8; 16]),
                next_repath_ticks: 0,
                distant_ticks: 0,
                kind,
            };
            let record = ActorRecord::try_new(
                ActorKey::Hostile(mornlea_domain::HostileId::try_new(id).map_err(|_| {
                    ServerError::Internal {
                        invariant: "hostile spawn id",
                    }
                })?),
                ActorLifecycle::Active,
                anchor.dimension,
                MotionState::new(MotionStateParts {
                    position: FiniteVec3::try_new(candidate).map_err(|_| {
                        ServerError::Internal {
                            invariant: "hostile spawn position",
                        }
                    })?,
                    velocity: FiniteVec3::try_new([0.0; 3]).map_err(|_| ServerError::Internal {
                        invariant: "hostile spawn velocity",
                    })?,
                    on_ground: true,
                }),
                LookAngles::try_new(0.0, 0.0).map_err(|_| ServerError::Internal {
                    invariant: "hostile spawn look",
                })?,
                SurvivalState::try_new(SurvivalStateParts {
                    health: MAX_HEALTH,
                    oxygen: 300,
                    hunger: 20,
                    saturation_zero: false,
                    armor_points: 0,
                })
                .map_err(|_| ServerError::Internal {
                    invariant: "hostile spawn survival",
                })?,
                ActorBody::Hostile(body.clone()),
            )
            .map_err(|_| ServerError::Internal {
                invariant: "hostile spawn staging",
            })?;
            return Ok(Some(HostileEntry {
                key: record.key,
                lifecycle: ActorLifecycle::Active,
                dimension: anchor.dimension,
                look: record.look,
                survival: record.survival,
                body,
                burn_cooldown: u32::from(COOLDOWN_PERIOD_TICKS as u8),
                distant_ticks: 0,
                shoot_cooldown: 0,
                fresh: true,
                path: None,
                dirty: true,
                dead: false,
            }));
        }
        id = splitmix64(id);
    }
    Ok(None)
}

/// Top-down column scan for the first two-air-over-support standing spot
/// (`hostileSpawnColumnSpot`): both air cells and the support must come from
/// observed data, the support must not be fluid, and it must provide
/// collision (plants, torches, fluids and snow do not hold a body).
fn spawn_column_spot(
    view: &AuthorityReadView<'_>,
    dimension: Dimension,
    x: i32,
    z: i32,
) -> Result<Option<i32>, ServerError> {
    let mut y = WORLD_MAX_Y - 2;
    while y > WORLD_MIN_Y {
        let lower = observed_block(view, dimension, BlockPos::new(x, y, z))?;
        if lower != Some(AIR) {
            y -= 1;
            continue;
        }
        let upper = observed_block(view, dimension, BlockPos::new(x, y + 1, z))?;
        if upper != Some(AIR) {
            y -= 1;
            continue;
        }
        let support = observed_block(view, dimension, BlockPos::new(x, y - 1, z))?;
        match support {
            Some(block) if !is_fluid(block) && provides_collision(block) => return Ok(Some(y)),
            _ => {
                y -= 1;
            }
        }
    }
    Ok(None)
}

/// Per-kind local cap (`hostileNearLimitExceeded`): for every active
/// same-dimension player within 48 blocks of the candidate, count that
/// player's same-kind same-dimension hostiles inside 48 blocks; any player
/// already at the kind's cap refuses the candidate. Distances stay in the
/// squared domain.
fn near_limit_exceeded(
    players: &[PlayerFact],
    entries: &[HostileEntry],
    dimension: Dimension,
    candidate: [f32; 3],
    kind: u8,
) -> bool {
    let near_sq = NEAR_RADIUS * NEAR_RADIUS;
    let limit = if kind == HURLER {
        NEAR_LIMIT_HURLER
    } else {
        NEAR_LIMIT_WALKER
    };
    for player in players {
        if player.dimension != dimension
            || horizontal_distance_sq(player.position, candidate) > near_sq
        {
            continue;
        }
        let count = entries
            .iter()
            .filter(|entry| {
                entry.kind() == kind
                    && entry.dimension == dimension
                    && horizontal_distance_sq(entry.position(), player.position) <= near_sq
            })
            .count();
        if count >= limit {
            return true;
        }
    }
    false
}

// ---------------------------------------------------------------------------
// Movement advance (`advanceHostileMovement` in `hostile.go` plus the
// deterministic target selection and waypoint execution of
// `packages/server/server/hostile_manager.go`).
// ---------------------------------------------------------------------------

/// Advances one hostile body by one fixed physics step. The per-tick input
/// resets to neutral (own yaw only) and only a live chase path converts it
/// into a forward step toward the first waypoint not yet reached. Returns
/// whether the entry was staged.
fn advance_movement(
    ctx: &mut TickContext<'_>,
    environment: &EnvironmentState,
    entry: &mut HostileEntry,
) -> Result<bool, ServerError> {
    let now = ctx.read().tick();
    let view = ctx.read();
    let tunables = environment.tunables;
    let target = nearest_target(&view, entry.dimension, entry.body.position)?;

    // Chase bookkeeping (`dispatchSnapshots` / `applyPathOutcome` in
    // `hostile_manager.go`): a target change invalidates the path and bumps
    // the generation; repath runs on the 20-tick cadence; a failed search
    // retries next tick; losing the target clears the pair and keeps the
    // durable next-replan tick fresh.
    let mut generation_changed = false;
    // The attack boundary ruling comes before any path dispatch: inside the
    // boundary the body holds position and only the target facts are
    // written (`dispatchSnapshots` range skip after `PlanHostileChase`).
    let within_attack = matches!(target.as_ref(), Some(fact)
        if horizontal_distance_sq(entry.body.position, fact.position)
            <= ATTACK_RANGE * ATTACK_RANGE);
    match target.as_ref() {
        Some(fact) => {
            let raw_goal = block_pos_of(fact.position)?;
            let stale = match &entry.path {
                Some(path) => path.target != raw_goal,
                None => true,
            };
            if stale {
                entry.path = None;
                generation_changed = true;
            }
            let due = match &entry.path {
                Some(path) => now >= path.next_repath_tick,
                None => now >= entry.body.next_repath_ticks,
            };
            if stale || due {
                if within_attack {
                    // In range: no path is built; the facts land and the
                    // decision is revisited next tick.
                    entry.body.has_target = true;
                    entry.body.player_id = PlayerId::from_bytes(fact.id);
                    entry.body.next_repath_ticks = now.saturating_add(1);
                    entry.dirty = true;
                } else {
                    refresh_path(ctx, entry, fact, raw_goal, generation_changed, now)?;
                }
            }
        }
        None => {
            if entry.body.has_target {
                // The target vanished: clear the pair and reselect next
                // tick (`dispatchSnapshots` target-lost branch).
                entry.body.has_target = false;
                entry.body.player_id = PlayerId::from_bytes([0u8; 16]);
                entry.path = None;
                entry.body.next_repath_ticks = now.saturating_add(1);
                entry.dirty = true;
            }
        }
    }

    // Movement input: inside the attack boundary the body holds position
    // (`advanceRunners` stop rule); with a live path it walks toward the
    // first waypoint not yet reached; otherwise it advances neutrally on
    // its own yaw (`applyHostileActions` neutral reset).
    let mut move_input = MovementInput {
        move_x: 0,
        move_z: 0,
        jump: false,
        yaw: entry.body.yaw,
    };
    if entry.body.has_target && !within_attack {
        let mut path = entry.path.take();
        if let Some(path) = path.as_mut() {
            if consume_arrived(path, entry.body.position) {
                entry.dirty = true;
            }
            if let Some(waypoint) = path.waypoints.get(path.cursor) {
                if let Some(input) = input_toward(entry.body.position, *waypoint) {
                    move_input = input;
                }
            } else {
                // The path is exhausted: replan against the target's
                // current cell on the next tick.
                entry.body.next_repath_ticks = now.saturating_add(1);
                entry.dirty = true;
            }
        }
        entry.path = path;
    }

    // One authoritative physics step through the F1 kernels, identical to
    // the player advance: unobserved cells block, staged blocks collide,
    // tunables come from the tick-start snapshot.
    let tuning: PhysicsTuning = tunables.physics();
    let position = entry.body.position;
    let velocity = entry.body.velocity;
    let on_ground = entry.body.on_ground;
    if !position
        .iter()
        .chain(velocity.iter())
        .all(|v| v.is_finite())
    {
        // A distorted body has no respawn path; it is removed deterministically.
        entry.remove();
        return entry.stage(ctx);
    }
    let view = ctx.read();
    let body_in_fluid = submersion_body(&view, entry.dimension, position)?;
    let step = HeldStep {
        move_x: move_input.move_x,
        move_z: move_input.move_z,
        jump: move_input.jump,
        yaw_sin: f64::from(move_input.yaw).sin() as f32,
        yaw_cos: f64::from(move_input.yaw).cos() as f32,
    };
    let (sweep_min, sweep_max) = sweep_bounds(velocity, on_ground, step, body_in_fluid, tuning);
    let (origin, dimensions) = step_prism(position, sweep_min, sweep_max, tuning.step_height)?;
    let view = ctx.read();
    let cells = prism_cells(&view, entry.dimension, origin, dimensions)?;
    let grid =
        CollisionGrid::try_new(origin, dimensions, &cells).map_err(|_| ServerError::Internal {
            invariant: "hostile motion grid",
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
                sprinting: false,
                sneaking: false,
            },
            tuning,
            sweep: SweepBounds {
                minimum: sweep_min,
                maximum: sweep_max,
            },
            grid,
        })
        .map_err(|_| ServerError::Internal {
            invariant: "hostile motion step",
        })?;

    // Invalid or fallen-out-of-world bodies are removed (`Y < MinY`); they
    // cannot persist or respawn.
    if !stepped
        .state
        .position
        .iter()
        .chain(stepped.state.velocity.iter())
        .all(|v| v.is_finite())
        || stepped.state.position[1] < WORLD_MIN_Y as f32
    {
        entry.remove();
        return entry.stage(ctx);
    }
    entry.body.position = stepped.state.position;
    entry.body.velocity = stepped.state.velocity;
    entry.body.on_ground = stepped.state.on_ground;
    // A directed step also turns the body (`applyHostileActions` keeps the
    // facing on move intents); a neutral step keeps the current yaw.
    if move_input.move_z != 0 || move_input.move_x != 0 {
        entry.body.yaw = move_input.yaw;
    }
    entry.dirty = true;

    // The shoot cooldown drains every movement tick (`applyHostileActions`
    // decrements before physics); refiring belongs to the projectile node.
    if entry.shoot_cooldown > 0 {
        entry.shoot_cooldown -= 1;
        entry.dirty = true;
    }

    // Keep the durable replan tick beside the path so restore resumes the
    // cadence (`PlanHostileChase` persists `nextRepathTicks`).
    if let Some(path) = &entry.path {
        entry.body.next_repath_ticks = path.next_repath_tick;
    }

    entry.stage(ctx)
}

/// Resolves the chase path for one hostile through the F1 pathfinding
/// kernel over a staged-observation window (`buildChaseGrid` in
/// `hostile_manager.go`). A successful search stores the waypoints, the
/// covered-chunk revisions and the 20-tick replan tick in the runtime path;
/// a failure clears the path and retries next tick.
fn refresh_path(
    ctx: &mut TickContext<'_>,
    entry: &mut HostileEntry,
    fact: &PlayerFact,
    raw_goal: BlockPos,
    generation_changed: bool,
    now: u64,
) -> Result<(), ServerError> {
    let view = ctx.read();
    let start_cell = standing_cell(entry.body.position)?;
    let window = build_path_grid(&view, entry.dimension, start_cell)?;
    let Some((grid, origin)) = window else {
        // An uncovered window defers without spending the attempt (Go
        // defers the snapshot; the retry is implicit on the next tick).
        entry.path = None;
        entry.body.has_target = true;
        entry.body.player_id = PlayerId::from_bytes(fact.id);
        entry.body.next_repath_ticks = now.saturating_add(1);
        entry.dirty = true;
        return Ok(());
    };
    let Some(goal) = chase_goal(&grid, origin, start_cell, raw_goal) else {
        entry.path = None;
        entry.body.has_target = true;
        entry.body.player_id = PlayerId::from_bytes(fact.id);
        entry.body.next_repath_ticks = now.saturating_add(1);
        entry.dirty = true;
        return Ok(());
    };
    let cells = grid.size()[0] as usize * grid.size()[1] as usize * grid.size()[2] as usize;
    let mut scratch = PathScratch::try_with_capacity(cells).map_err(|_| ServerError::Internal {
        invariant: "hostile path scratch",
    })?;
    let previous_generation = entry.path.as_ref().map_or(0, |path| path.generation);
    match NativePathfind.find(&grid, start_cell, goal, &mut scratch) {
        Ok(result) => {
            entry.body.has_target = true;
            entry.body.player_id = PlayerId::from_bytes(fact.id);
            entry.body.next_repath_ticks = now.saturating_add(REPATH_PERIOD_TICKS);
            entry.path = Some(PathState {
                generation: previous_generation + u64::from(generation_changed),
                target: BlockPos::new(goal.x, goal.y, goal.z),
                revisions: result
                    .revisions()
                    .iter()
                    .map(|revision| {
                        (
                            ChunkKey {
                                dimension: entry.dimension,
                                pos: mornlea_domain::ChunkPos::new(
                                    revision.chunk[0],
                                    revision.chunk[1],
                                ),
                            },
                            revision.revision,
                        )
                    })
                    .collect(),
                waypoints: result
                    .waypoints()
                    .iter()
                    .map(|cell| BlockPos::new(cell.x, cell.y, cell.z))
                    .collect(),
                cursor: 0,
                next_repath_tick: entry.body.next_repath_ticks,
            });
            entry.dirty = true;
        }
        Err(_) => {
            // A failed or unreachable search clears the path and replans on
            // the next tick (`applyPathOutcome` failure branch).
            entry.path = None;
            entry.body.has_target = true;
            entry.body.player_id = PlayerId::from_bytes(fact.id);
            entry.body.next_repath_ticks = now.saturating_add(1);
            entry.dirty = true;
        }
    }
    Ok(())
}

/// Converts the position toward one waypoint into the canonical hostile move
/// input: forward charge with the body facing the waypoint
/// (`movementInputToward` in `companion_manager.go` via
/// `applyHostileActions`), jumping when the waypoint sits a full block or
/// more above the feet.
fn input_toward(position: [f32; 3], waypoint: BlockPos) -> Option<MovementInput> {
    let dx = waypoint.x() as f32 + 0.5 - position[0];
    let dz = waypoint.z() as f32 + 0.5 - position[2];
    let length = (f64::from(dx) * f64::from(dx) + f64::from(dz) * f64::from(dz)).sqrt();
    if length == 0.0 {
        return None;
    }
    let yaw = normalize_yaw(f32::atan2(
        (f64::from(-dx) / length) as f32,
        (f64::from(-dz) / length) as f32,
    ));
    Some(MovementInput {
        move_x: 0,
        move_z: 1,
        jump: waypoint.y() > floor_to_i32(position[1]),
        yaw,
    })
}

/// Consumes waypoints already reached by the body (`advanceRunners`): the
/// 0.35 arrival radius compares horizontal components only. Returns whether
/// any waypoint was consumed.
fn consume_arrived(path: &mut PathState, position: [f32; 3]) -> bool {
    let mut consumed = false;
    while path.cursor < path.waypoints.len() {
        let waypoint = path.waypoints[path.cursor];
        let dx = position[0] - (waypoint.x() as f32 + 0.5);
        let dz = position[2] - (waypoint.z() as f32 + 0.5);
        if dx * dx + dz * dz <= WAYPOINT_ARRIVAL_RADIUS_SQ {
            path.cursor += 1;
            consumed = true;
        } else {
            break;
        }
    }
    consumed
}

// ---------------------------------------------------------------------------
// Target selection (`nearestTarget` in `packages/server/server/
// hostile_manager.go`): nearest active same-dimension player by squared
// horizontal distance; an exact tie takes the smaller `PlayerID` bytes so
// the choice is stable under replay.
// ---------------------------------------------------------------------------

/// The online player facts one decision consumes.
struct PlayerFact {
    session: SessionKey,
    id: [u8; 16],
    position: [f32; 3],
    dimension: Dimension,
}

fn active_players(view: &AuthorityReadView<'_>) -> Result<Vec<PlayerFact>, ServerError> {
    let mut players = Vec::new();
    for actor in view.actors() {
        let ActorKey::Player(session) = actor.key else {
            continue;
        };
        if actor.lifecycle != ActorLifecycle::Active {
            continue;
        }
        let ActorBody::Player(body) = &actor.body else {
            return Err(ServerError::InvalidInput { field: "actor" });
        };
        players.push(PlayerFact {
            session,
            id: body.player_id.to_bytes(),
            position: actor.motion.position().get(),
            dimension: actor.dimension,
        });
    }
    // Sorted-session order (`sortedActiveSessions` in
    // `packages/server/sim/entity/drop.go:212-222`): the anchor pick indexes
    // this list at `worldtime % len`, so the key is the process session id,
    // never the player uuid. Target selection reorders by its own explicit
    // rule and does not depend on this order.
    players.sort_by_key(|player| player.session);
    Ok(players)
}

fn nearest_target(
    view: &AuthorityReadView<'_>,
    dimension: Dimension,
    position: [f32; 3],
) -> Result<Option<PlayerFact>, ServerError> {
    let players = active_players(view)?;
    let mut nearest: Option<PlayerFact> = None;
    let mut nearest_distance = 0.0f32;
    for candidate in players {
        if candidate.dimension != dimension {
            continue;
        }
        let distance = horizontal_distance_sq(position, candidate.position);
        let better = match &nearest {
            None => true,
            Some(current) => {
                distance < nearest_distance
                    || (distance == nearest_distance && candidate.id < current.id)
            }
        };
        if better {
            nearest_distance = distance;
            nearest = Some(candidate);
        }
    }
    Ok(nearest)
}

/// Reports whether any active same-dimension player sits inside the squared
/// horizontal radius (`advanceHostileDistant` within check). An empty
/// player set counts as "outside every radius".
fn player_within(
    ctx: &TickContext<'_>,
    dimension: Dimension,
    position: [f32; 3],
    radius_sq: f32,
) -> bool {
    let view = ctx.read();
    for actor in view.actors() {
        let ActorKey::Player(_) = actor.key else {
            continue;
        };
        if actor.lifecycle != ActorLifecycle::Active || actor.dimension != dimension {
            continue;
        }
        if horizontal_distance_sq(position, actor.motion.position().get()) <= radius_sq {
            return true;
        }
    }
    false
}

// ---------------------------------------------------------------------------
// Path window construction over staged observations. The grid mirrors
// `buildChaseGrid`: a 33x9x33 window centered on the standing cell, every
// covered chunk observed, unobserved cells blocking.
// ---------------------------------------------------------------------------

/// The passable-block table shared with companion pathing
/// (`productionCompanionPassableBlocks` in
/// `packages/server/server/companion_snapshot.go`): air, plants, snow
/// layers, the door upper half and torches pass; everything else blocks.
fn passable_table() -> Result<PathBlockTable, ServerError> {
    let mut passable = Vec::new();
    passable.push(AIR);
    passable.extend(WHEAT_FIRST..=WHEAT_LAST);
    passable.extend(POTATO_FIRST..=POTATO_LAST);
    passable.extend(CARROT_FIRST..=CARROT_LAST);
    passable.push(SHORT_GRASS);
    passable.push(SAPLING);
    passable.extend(SNOW_FIRST..=SNOW_LAST);
    passable.push(DOOR_UPPER);
    passable.extend(TORCH_FIRST..=TORCH_LAST);
    PathBlockTable::from_passable_ids(&passable).map_err(|_| ServerError::Internal {
        invariant: "hostile path table",
    })
}

/// Builds the chase window grid from staged observations. Returns the grid
/// plus its origin cell and the window's low y, or `None` when any covered
/// chunk has no observed cell (the all-covered-ready gate).
fn build_path_grid(
    view: &AuthorityReadView<'_>,
    dimension: Dimension,
    center: PathCell,
) -> Result<Option<(PathGrid, PathCell)>, ServerError> {
    let low_y = (center.y - WINDOW_VERTICAL_RADIUS).max(WORLD_MIN_Y);
    let high_y = (center.y + WINDOW_VERTICAL_RADIUS).min(WORLD_MAX_Y - 1);
    if high_y < low_y {
        return Ok(None);
    }
    let origin = PathCell {
        x: center.x - WINDOW_HORIZONTAL_RADIUS,
        y: low_y,
        z: center.z - WINDOW_HORIZONTAL_RADIUS,
    };
    let size = [
        (2 * WINDOW_HORIZONTAL_RADIUS + 1) as u32,
        u32::try_from(high_y - low_y + 1).map_err(|_| ServerError::Internal {
            invariant: "hostile path window",
        })?,
        (2 * WINDOW_HORIZONTAL_RADIUS + 1) as u32,
    ];
    let cells = size[0] as usize * size[1] as usize * size[2] as usize;
    // The search cap (`PathGrid::try_new` rejects grids above 131072
    // cells); the 33x9x33 window always fits.
    if cells > 131072 {
        return Ok(None);
    }
    let table = passable_table()?;
    let mut blocks = vec![u16::MAX; cells];
    let mut revisions: Vec<PathRevision> = Vec::new();
    let mut seen: Vec<([i32; 2], u64)> = Vec::new();
    // Fill in the kernel's `flat_index` order (x-outer, z-middle, y-inner)
    // so every staged cell lands on the slot `block_at` reads back.
    for lx in 0..size[0] as i32 {
        for lz in 0..size[2] as i32 {
            for ly in 0..size[1] as i32 {
                let pos = BlockPos::new(origin.x + lx, origin.y + ly, origin.z + lz);
                let Some(observed) = view.observation(dimension, pos) else {
                    continue;
                };
                let index =
                    (lx as usize * size[2] as usize + lz as usize) * size[1] as usize + ly as usize;
                blocks[index] = observed.block;
                let chunk = [pos.x() >> 4, pos.z() >> 4];
                if !seen.iter().any(|(seen_chunk, _)| *seen_chunk == chunk) {
                    seen.push((chunk, observed.revision));
                }
            }
        }
    }
    // Every covered chunk must carry at least one observed cell, mirroring
    // the all-covered-ready gate: an uncovered window defers the attempt.
    let span_x = size[0].div_ceil(16) as i32;
    let span_z = size[2].div_ceil(16) as i32;
    let base_chunk = [origin.x >> 4, origin.z >> 4];
    for dx in 0..span_x {
        for dz in 0..span_z {
            let want = [base_chunk[0] + dx, base_chunk[1] + dz];
            if !seen.iter().any(|(chunk, _)| *chunk == want) {
                return Ok(None);
            }
        }
    }
    revisions.extend(seen.iter().map(|(chunk, revision)| PathRevision {
        chunk: *chunk,
        revision: *revision,
    }));
    let grid = PathGrid::try_new(origin, size, blocks.into_boxed_slice(), table, revisions)
        .map_err(|_| ServerError::Internal {
            invariant: "hostile path grid",
        })?;
    Ok(Some((grid, origin)))
}

/// Resolves the chase goal: clamp the target's standing cell into the
/// window, then pick the standable cell in that column closest to the
/// target's own y, keeping the lower cell on a tie (`chaseGoal` in
/// `hostile_manager.go` with `hostileStandable`; the standing rule is the
/// kernel's `is_standing`, the same oracle the search validates endpoints
/// against).
fn chase_goal(
    grid: &PathGrid,
    origin: PathCell,
    center: PathCell,
    raw: BlockPos,
) -> Option<PathCell> {
    let goal_x = raw.x().clamp(
        center.x - WINDOW_HORIZONTAL_RADIUS,
        center.x + WINDOW_HORIZONTAL_RADIUS,
    );
    let goal_z = raw.z().clamp(
        center.z - WINDOW_HORIZONTAL_RADIUS,
        center.z + WINDOW_HORIZONTAL_RADIUS,
    );
    let size = grid.size();
    let mut best: Option<PathCell> = None;
    let mut best_distance = 0i32;
    for y in origin.y..origin.y + size[1] as i32 {
        let candidate = PathCell {
            x: goal_x,
            y,
            z: goal_z,
        };
        if !is_standing(grid, candidate) {
            continue;
        }
        let distance = (candidate.y - raw.y()).abs();
        if best.is_none() || distance < best_distance {
            best = Some(candidate);
            best_distance = distance;
        }
    }
    best
}

// ---------------------------------------------------------------------------
// Local block light (`block_light_query.go`): a 29^3 window around the
// candidate, emission-seeded, descending-level propagation with opaque and
// unobserved cells blocking and one extra attenuation step per fluid.
// ---------------------------------------------------------------------------

fn local_block_light(
    view: &AuthorityReadView<'_>,
    dimension: Dimension,
    center: BlockPos,
) -> Result<u8, ServerError> {
    let side = LIGHT_SIDE as i32;
    let base_x = center.x() - LIGHT_RADIUS;
    let base_y = center.y() - LIGHT_RADIUS;
    let base_z = center.z() - LIGHT_RADIUS;
    let index_of = |rx: i32, ry: i32, rz: i32| -> usize { ((rx * side + ry) * side + rz) as usize };
    let mut levels = vec![0u8; LIGHT_SIDE * LIGHT_SIDE * LIGHT_SIDE];
    // Seeds: every observed cell's emission, even inside opaque blocks.
    for rx in 0..side {
        for ry in 0..side {
            for rz in 0..side {
                let pos = BlockPos::new(base_x + rx, base_y + ry, base_z + rz);
                if let Some(observed) = view.observation(dimension, pos) {
                    let emission = block_emission(observed.block);
                    if emission > 0 {
                        levels[index_of(rx, ry, rz)] = emission;
                    }
                }
            }
        }
    }
    // Descending-level propagation. Each relaxation target is strictly
    // dimmer than its source, so a single pass per level suffices.
    for level in (1..=LIGHT_MAX_LEVEL).rev() {
        for index in 0..levels.len() {
            if levels[index] != level {
                continue;
            }
            let current = levels[index] as i32;
            if current <= 1 {
                continue;
            }
            let rz = index as i32 % side;
            let ry = (index as i32 / side) % side;
            let rx = index as i32 / (side * side);
            for (dx, dy, dz) in [
                (-1, 0, 0),
                (1, 0, 0),
                (0, -1, 0),
                (0, 1, 0),
                (0, 0, -1),
                (0, 0, 1),
            ] {
                let (nx, ny, nz) = (rx + dx, ry + dy, rz + dz);
                if nx < 0 || nx >= side || ny < 0 || ny >= side || nz < 0 || nz >= side {
                    continue;
                }
                let pos = BlockPos::new(base_x + nx, base_y + ny, base_z + nz);
                let Some(observed) = view.observation(dimension, pos) else {
                    continue;
                };
                if block_opaque(observed.block) {
                    continue;
                }
                let candidate = current - 1 - i32::from(light_attenuation(observed.block));
                if candidate <= 0 {
                    continue;
                }
                let neighbor = index_of(nx, ny, nz);
                if levels[neighbor] >= candidate as u8 {
                    continue;
                }
                levels[neighbor] = candidate as u8;
            }
        }
    }
    Ok(levels[index_of(LIGHT_RADIUS, LIGHT_RADIUS, LIGHT_RADIUS)])
}

/// Emission table (`core.BlockEmission`): the light block emits 15, the five
/// torch forms emit 14, everything else 0.
fn block_emission(block: u16) -> u8 {
    if block == LIGHT_BLOCK {
        15
    } else if (TORCH_FIRST..=TORCH_LAST).contains(&block) {
        14
    } else {
        0
    }
}

/// Extra attenuation step (`core.BlockLightAttenuation`): fluids only.
fn light_attenuation(block: u16) -> u8 {
    if is_fluid(block) { 1 } else { 0 }
}

/// Opaque table (`core.BlockOpaque`): registered solid cubes block light;
/// air, glass, leaves, fluids, plants, doors, torches, beds and snow layers
/// do not; unregistered ids do not.
fn block_opaque(block: u16) -> bool {
    block < BLOCK_ID_MAX
        && block != AIR
        && block != GLASS
        && block != LEAVES
        && !is_fluid(block)
        && !is_plant(block)
        && !is_door(block)
        && !(TORCH_FIRST..=TORCH_LAST).contains(&block)
        && !(BED_FIRST..=BED_LAST).contains(&block)
        && !(SNOW_FIRST..=SNOW_LAST).contains(&block)
}

fn is_fluid(block: u16) -> bool {
    (FLUID_FIRST..=FLUID_LAST).contains(&block)
}

fn is_plant(block: u16) -> bool {
    (WHEAT_FIRST..=WHEAT_LAST).contains(&block)
        || (POTATO_FIRST..=POTATO_LAST).contains(&block)
        || (CARROT_FIRST..=CARROT_LAST).contains(&block)
        || block == SHORT_GRASS
        || block == SAPLING
}

fn is_door(block: u16) -> bool {
    (DOOR_LOWER_FIRST..=DOOR_UPPER).contains(&block)
}

/// Whether a block provides body support (`physics.BlockCollisionBoxes`
/// carrying at least one box): fluids, plants, torches, snow layers and the
/// door upper half do not; the thin door halves, beds and farmland do.
fn provides_collision(block: u16) -> bool {
    block != AIR
        && !is_fluid(block)
        && !is_plant(block)
        && !(TORCH_FIRST..=TORCH_LAST).contains(&block)
        && !(SNOW_FIRST..=SNOW_LAST).contains(&block)
        && block != DOOR_UPPER
}

/// The observed block at one cell, or `None` when the cell was never staged
/// (the authority reads no world it was not handed).
fn observed_block(
    view: &AuthorityReadView<'_>,
    dimension: Dimension,
    pos: BlockPos,
) -> Result<Option<u16>, ServerError> {
    Ok(view
        .observation(dimension, pos)
        .map(|observed| observed.block))
}

/// Sky exposure (`hostileSkyExposed`): the column straight up from the body
/// top must reach the world top through observed non-opaque cells; any
/// unobserved cell reads as "not exposed" so burn never charges on unloaded
/// world state.
fn sky_exposed(
    view: &AuthorityReadView<'_>,
    dimension: Dimension,
    position: [f32; 3],
) -> Result<bool, ServerError> {
    let column = block_pos_of(position)?;
    let mut y = floor_to_i32(position[1] + PLAYER_HEIGHT);
    while y < WORLD_MAX_Y {
        match observed_block(view, dimension, BlockPos::new(column.x(), y, column.z()))? {
            None => return Ok(false),
            Some(block) if block_opaque(block) => return Ok(false),
            _ => y += 1,
        }
    }
    Ok(true)
}

// ---------------------------------------------------------------------------
// Physics-step scaffolding, mirrored from the player motion provider's
// copies of the shared Go rows (`StepWithTunables`, `stepSweepBounds`,
// `stepPrismFor`, `SubmersionFlagsWithTunables`).
// ---------------------------------------------------------------------------

/// One step's resolved intent fed to the kernel.
#[derive(Clone, Copy, Debug)]
struct HeldStep {
    move_x: i8,
    move_z: i8,
    jump: bool,
    yaw_sin: f32,
    yaw_cos: f32,
}

/// The resolved movement decision for one tick.
#[derive(Clone, Copy, Debug)]
struct MovementInput {
    move_x: i8,
    move_z: i8,
    jump: bool,
    yaw: f32,
}

/// Sweep hull mirror (`stepSweepBounds` in
/// `packages/shared/physics/step.go`).
fn sweep_bounds(
    velocity: [f32; 3],
    on_ground: bool,
    step: HeldStep,
    body_in_fluid: bool,
    tuning: PhysicsTuning,
) -> ([f32; 3], [f32; 3]) {
    let dt = tuning.fixed_delta_seconds;
    let walk_speed = tuning.walk_speed;
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

/// Movement target mirror (`movementTargetFromYaw`).
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

/// Fused vector length mirror (`stepVectorLength`).
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

/// Prism mirror (`stepPrismFor`).
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
        floor_to_i32(minimum[0]),
        floor_to_i32(minimum[1]),
        floor_to_i32(minimum[2]),
    ];
    let end = [
        floor_to_i32(maximum[0]),
        floor_to_i32(maximum[1]),
        floor_to_i32(maximum[2]),
    ];
    let mut dimensions = [0u32; 3];
    let mut cells: u64 = 1;
    for axis in 0..3 {
        let span = i64::from(end[axis]) - i64::from(origin[axis]) + 1;
        if span <= 0 || cells * span as u64 > PRISM_MAX_CELLS {
            return Err(ServerError::InvalidInput { field: "actor" });
        }
        dimensions[axis] = span as u32;
        cells *= span as u64;
    }
    Ok((origin, dimensions))
}

/// Staged grid cells in y/x/z order (`encodeStepInput` loop): unobserved
/// cells stay unloaded, which the collision kernel treats as blocking.
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
/// `packages/shared/physics/types.go`).
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
        return boxed_cell([[[0.0, 0.0, 0.0], [1.0, 0.5625, 1.0]]]);
    }
    if (DOOR_LOWER_FIRST..=DOOR_LOWER_LAST).contains(&block) {
        let index = block - DOOR_LOWER_FIRST;
        let direction = index / 2;
        let open = index % 2 == 1;
        let thin = 3.0f32 / 16.0;
        let wide = 1.0 - thin;
        let shape = match (direction, open) {
            (0, false) | (1, true) => [[0.0, 0.0, wide], [1.0, 1.0, 1.0]],
            (1, false) | (2, true) => [[0.0, 0.0, 0.0], [thin, 1.0, 1.0]],
            (2, false) | (3, true) => [[0.0, 0.0, 0.0], [1.0, 1.0, thin]],
            _ => [[wide, 0.0, 0.0], [1.0, 1.0, 1.0]],
        };
        return boxed_cell([shape]);
    }
    if (FARMLAND_FIRST..=FARMLAND_LAST).contains(&block) {
        return boxed_cell([[[0.0, 0.0, 0.0], [1.0, 0.9375, 1.0]]]);
    }
    boxed_cell([[[0.0, 0.0, 0.0], [1.0, 1.0, 1.0]]])
}

const EMPTY_BOX: Aabb = Aabb {
    minimum: [0.0; 3],
    maximum: [0.0; 3],
};

fn empty_cell() -> Result<CollisionCell, ServerError> {
    CollisionCell::try_new(true, [EMPTY_BOX; 8], 0).map_err(|_| ServerError::Internal {
        invariant: "hostile grid",
    })
}

fn boxed_cell(local: [[[f32; 3]; 2]; 1]) -> Result<CollisionCell, ServerError> {
    let mut boxes = [EMPTY_BOX; 8];
    boxes[0] = Aabb {
        minimum: local[0][0],
        maximum: local[0][1],
    };
    CollisionCell::try_new(true, boxes, 1).map_err(|_| ServerError::Internal {
        invariant: "hostile grid",
    })
}

/// Body immersion mirror (`SubmersionFlagsWithTunables`): the feet-centered
/// box over staged blocks; unobserved cells read as non-fluid. Only the
/// body flag enters the kernel.
fn submersion_body(
    view: &AuthorityReadView<'_>,
    dimension: Dimension,
    position: [f32; 3],
) -> Result<bool, ServerError> {
    let minimum = [
        floor_to_i32(position[0] - HALF_WIDTH),
        floor_to_i32(position[1]),
        floor_to_i32(position[2] - HALF_WIDTH),
    ];
    let maximum = [
        fluid_upper(position[0] + HALF_WIDTH, minimum[0]),
        fluid_upper(position[1] + PLAYER_HEIGHT, minimum[1]),
        fluid_upper(position[2] + HALF_WIDTH, minimum[2]),
    ];
    for y in minimum[1]..=maximum[1] {
        for x in minimum[0]..=maximum[0] {
            for z in minimum[2]..=maximum[2] {
                if let Some(observed) = view.observation(dimension, BlockPos::new(x, y, z))
                    && is_fluid(observed.block)
                {
                    return Ok(true);
                }
            }
        }
    }
    Ok(false)
}

/// AABB upper-cell mirror (`fluidCellUpperBound`).
fn fluid_upper(maximum: f32, lower: i32) -> i32 {
    (floor_to_i32(maximum) - 1).max(lower)
}

// ---------------------------------------------------------------------------
// Shared numeric mirrors.
// ---------------------------------------------------------------------------

/// Squared horizontal distance, the only comparison domain the radius rules
/// use (`horizontalDistanceSq` in `hostile.go`).
fn horizontal_distance_sq(from: [f32; 3], to: [f32; 3]) -> f32 {
    let dx = to[0] - from[0];
    let dz = to[2] - from[2];
    dx * dx + dz * dz
}

/// Floor-to-cell mirror (`blockPosOf` in `hostile.go`).
fn block_pos_of(position: [f32; 3]) -> Result<BlockPos, ServerError> {
    Ok(BlockPos::new(
        floor_to_i32(position[0]),
        floor_to_i32(position[1]),
        floor_to_i32(position[2]),
    ))
}

/// The standing cell of a body (`standingCellOf` in
/// `hostile_manager.go`): the feet cell.
fn standing_cell(position: [f32; 3]) -> Result<PathCell, ServerError> {
    let cell = block_pos_of(position)?;
    Ok(PathCell {
        x: cell.x(),
        y: cell.y(),
        z: cell.z(),
    })
}

/// Checked floor into i32; positions outside the int32 domain cannot name a
/// cell and refuse (`collisionCheckedFloor`).
fn floor_to_i32(value: f32) -> i32 {
    let floored = f64::from(value).floor();
    if !floored.is_finite() || !((i32::MIN as f64)..=(i32::MAX as f64)).contains(&floored) {
        0
    } else {
        floored as i32
    }
}

/// Yaw normalization mirror (`normalizeYaw` in
/// `packages/server/sim/entity/placement.go`): into [-pi, pi).
fn normalize_yaw(yaw: f32) -> f32 {
    let mut normalized = (f64::from(yaw) + std::f64::consts::PI) % (2.0 * std::f64::consts::PI);
    if normalized < 0.0 {
        normalized += 2.0 * std::f64::consts::PI;
    }
    (normalized - std::f64::consts::PI) as f32
}

/// The repository's shared SplitMix64 mixer (`sampler.SplitMix64` in
/// `packages/server/updates/sampler.go`, identical to the environment
/// provider's copy).
fn splitmix64(mut x: u64) -> u64 {
    x = x.wrapping_add(0x9e37_79b9_7f4a_7c15);
    x = (x ^ (x >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    x ^ (x >> 31)
}

/// Candidate hash (`sampler.HostileCandidateHash` in
/// `packages/server/updates/sampler.go`): the seed-time base folded with the
/// zero-extended coordinates; the low byte carries the spawn gate, the
/// residue the kind dispatch, and the value itself the spawned id.
fn hostile_candidate_hash(seed: i64, tick: u64, x: i32, y: i32, z: i32) -> u64 {
    let hash = splitmix64((seed as u64) ^ tick);
    let hash = splitmix64(hash ^ (x as u32 as u64) ^ (z as u32 as u64));
    splitmix64(hash ^ (y as u32 as u64))
}

/// Season warp chain, mirrored from the environment provider's copies of the
/// Go rows (`core.YearPhaseAt`, `core.DayArcTicks`, `core.EffectiveDayPhase`,
/// `core.EffectiveDayPhaseAt` in `packages/shared/core`).
const YEAR_TICKS: u64 = 288_000;
const DAY_LENGTH_TICKS: u32 = 24_000;
const HALF_DAY_TICKS: u32 = DAY_LENGTH_TICKS / 2;

fn year_index(world_time: u64, season_offset: u32) -> u64 {
    (world_time % YEAR_TICKS + u64::from(season_offset) % YEAR_TICKS) % YEAR_TICKS
}

fn day_arc_ticks(year_phase: f64) -> u16 {
    let fraction = 0.5 + 0.15 * (2.0 * std::f64::consts::PI * year_phase).sin();
    let ticks = (fraction * f64::from(DAY_LENGTH_TICKS)).round() as i64;
    let ticks = if ticks % 2 == 1 { ticks - 1 } else { ticks };
    ticks as u16
}

fn effective_day_phase(world_time: u64, offset: u16, day_arc: u16) -> u16 {
    let arc_valid = day_arc != 0 && day_arc <= DAY_LENGTH_TICKS as u16 && day_arc.is_multiple_of(2);
    if !arc_valid {
        return 0;
    }
    let p = ((world_time % u64::from(DAY_LENGTH_TICKS) + u64::from(offset))
        % u64::from(DAY_LENGTH_TICKS)) as u32;
    let arc = u32::from(day_arc);
    if p < arc {
        return (p * HALF_DAY_TICKS / arc) as u16;
    }
    (HALF_DAY_TICKS + (p - arc) * HALF_DAY_TICKS / (DAY_LENGTH_TICKS - arc)) as u16
}

fn effective_day_phase_at(world_time: u64, offset: u16, season_offset: u32) -> u16 {
    let year_phase = year_index(world_time, season_offset) as f64 / YEAR_TICKS as f64;
    effective_day_phase(world_time, offset, day_arc_ticks(year_phase))
}
