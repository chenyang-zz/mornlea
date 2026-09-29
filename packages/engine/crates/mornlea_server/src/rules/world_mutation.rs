//! Placement geometry and door interaction settlement.
//!
//! This provider owns the interaction-phase geometry: a human `PlaceBlock`
//! command resolves its authority ray from the current look plus the selected
//! hotbar slot and settles through the accepted atomic transaction, while an
//! authority-owned door interaction toggles the door the authority-owned look
//! ray meets. Neither path accepts a client-supplied target or revision; every
//! target cell, footprint, generation, revision and inventory debit is derived
//! from authority state by the shared resolvers.
//!
//! Mirrored Go rows, each cited at its site:
//!
//! - `packages/server/sim/entity/placement.go` (`executePlacement`): the eye
//!   origin, the `LookDirection` formula, the faceless-hit refusal, and the
//!   door/bed footprint dispatch. The crop, sapling, torch and container
//!   branches of that file stay with their own providers; this module consumes
//!   the accepted resolver exactly as it stands.
//! - `packages/server/sim/entity/door.go` (`tryPlaceDoor`,
//!   `handleInteractDoor`, `executeInteractDoor`): the two-cell atomic door
//!   footprint, the lower-only toggle with the upper half keeping its single
//!   form, the well-formed partner requirement, and the silent no-op when the
//!   ray meets a non-door block.
//! - `packages/shared/core/block.go`: the door lower order (south, west,
//!   north, east, closed before open) and the single upper form.
//!
//! Deliberate boundaries. The sneak gate in `executeInteractDoor` reads
//! the held-controls bit the movement provider stores, and the frozen tick
//! context exposes no held-controls surface, so this module stages no sneak
//! branch: a sneaking actor toggles exactly like a non-sneaking one until the
//! movement-owned surface lands. The bed interaction kind belongs to the sleep
//! provider and refuses here without effect. The toggle commits through the
//! system transaction entry because no actor-transaction resolver covers an
//! interaction toggle; the frozen producer vocabulary names simulation causes
//! only, and the commit drops the label without any overlay trace, so the
//! choice carries no downstream meaning.

use mornlea_domain::{BlockPos, Command, Dimension};
use mornlea_engine::native::contracts::raycast::{Ray, RayCursor, RaycastOp};
use mornlea_engine::native::raycast::NativeRaycast;

use crate::core::contracts::{
    ActorKey, ActorLifecycle, AuthorityInteraction, BlockObservation, BlockWrite, InteractionKind,
    PhaseReport, RuleCall, RulePhase, ServerError, SessionKey, SystemRule,
};
use crate::core::interaction::{look_direction, normalized_direction, target_block};
use crate::core::mutation::resolve_place;
use crate::core::state::{AuthorityReadView, TickContext};

/// First door-lower form (`core.DoorLowerSouthClosed`).
const DOOR_LOWER_FIRST: u16 = 62;

/// Last door-lower form (`core.DoorLowerEastOpen`).
const DOOR_LOWER_LAST: u16 = 69;

/// Single door-upper form (`core.DoorUpper`).
const DOOR_UPPER: u16 = 70;

/// Reports whether a block number is any door half (`core.IsDoor`).
fn is_door(block: u16) -> bool {
    (DOOR_LOWER_FIRST..=DOOR_UPPER).contains(&block)
}

/// The observation basis the geometry needs: an active actor record with its
/// current pose and dimension, plus the environment tunables. A missing actor
/// or environment refuses exactly like the accepted resolvers treat an
/// unavailable authority basis.
struct ActorBasis {
    dimension: Dimension,
    eye: [f32; 3],
    reach: f32,
}

fn actor_basis(view: &AuthorityReadView<'_>, actor: ActorKey) -> Result<ActorBasis, ServerError> {
    let record = view
        .actor(actor)
        .ok_or(ServerError::InvalidInput { field: "actor" })?;
    if record.lifecycle != ActorLifecycle::Active {
        return Err(ServerError::InvalidInput { field: "actor" });
    }
    let environment = view
        .environment()
        .ok_or(ServerError::InvalidInput { field: "actor" })?;
    let position = record.motion.position().get();
    Ok(ActorBasis {
        dimension: record.dimension,
        eye: [
            position[0],
            position[1] + environment.tunables.eye_height(),
            position[2],
        ],
        reach: environment.tunables.interaction_reach(),
    })
}

/// One classified ray hit: the full observation of the first non-air cell.
struct RayHit {
    observed: BlockObservation,
}

/// Walks the numerical ray kernel batch by batch and classifies every
/// traversed cell against the authority view, the same walk the accepted
/// resolvers perform: an unobserved cell refuses because the authority cannot
/// certify geometry it has not observed, cells the shared `target_block`
/// classifier passes over continue the walk, and the first target cell is the
/// hit. No hit within reach refuses.
fn cast_ray(
    view: &AuthorityReadView<'_>,
    dimension: Dimension,
    origin: [f32; 3],
    direction: [f32; 3],
    reach: f32,
) -> Result<Option<RayHit>, ServerError> {
    const REFUSAL: ServerError = ServerError::InvalidInput {
        field: "interaction",
    };
    let normalized = normalized_direction(direction).ok_or(REFUSAL)?;
    let mut cursor = RayCursor::try_new(Ray {
        origin,
        direction: normalized,
        maximum: reach,
    })
    .map_err(|_| REFUSAL)?;
    loop {
        let batch = NativeRaycast.next_batch(&mut cursor).map_err(|_| REFUSAL)?;
        for record in batch.records() {
            let cell = BlockPos::new(record.cell[0], record.cell[1], record.cell[2]);
            match view.observation(dimension, cell) {
                None => return Err(REFUSAL),
                Some(observed) if !target_block(view, dimension, cell, observed.block) => {}
                Some(observed) => return Ok(Some(RayHit { observed })),
            }
        }
        if batch.is_done() {
            return Ok(None);
        }
    }
}

/// Derives the door-toggle write from one authority-owned look: the ray meets
/// a door cell, the lower half carries a valid direction, and the partner
/// half is observed and well formed (`handleInteractDoor`,
/// `packages/server/sim/entity/door.go`). Only the lower half flips; the
/// upper half keeps its single form. A ray that meets a non-door block is the
/// silent no-op the oracle pins, reported as no write.
fn toggle_write(
    view: &AuthorityReadView<'_>,
    actor: ActorKey,
    look: mornlea_domain::LookAngles,
) -> Result<Option<BlockWrite>, ServerError> {
    const REFUSAL: ServerError = ServerError::InvalidInput {
        field: "interaction",
    };
    let basis = actor_basis(view, actor)?;
    let direction = look_direction(look.yaw(), look.pitch());
    let Some(hit) = cast_ray(view, basis.dimension, basis.eye, direction, basis.reach)? else {
        return Err(REFUSAL);
    };
    if !is_door(hit.observed.block) {
        return Ok(None);
    }
    // The hit names the pair: a lower hit pairs upward, an upper hit pairs
    // downward. Both halves are re-read so a malformed or unready partner
    // refuses instead of toggling half a door.
    let (lower_pos, upper_pos) = if hit.observed.block == DOOR_UPPER {
        (
            BlockPos::new(
                hit.observed.pos.x(),
                hit.observed.pos.y() - 1,
                hit.observed.pos.z(),
            ),
            hit.observed.pos,
        )
    } else {
        (
            hit.observed.pos,
            BlockPos::new(
                hit.observed.pos.x(),
                hit.observed.pos.y() + 1,
                hit.observed.pos.z(),
            ),
        )
    };
    let lower = view
        .observation(basis.dimension, lower_pos)
        .ok_or(REFUSAL)?;
    let upper = view
        .observation(basis.dimension, upper_pos)
        .ok_or(REFUSAL)?;
    if !(DOOR_LOWER_FIRST..=DOOR_LOWER_LAST).contains(&lower.block) || upper.block != DOOR_UPPER {
        return Err(REFUSAL);
    }
    // Lower forms pair closed before open, so one bit flips the open state
    // while the direction bits stay untouched.
    let toggled = lower.block ^ 1;
    BlockWrite::try_new(lower, toggled)
        .map(Some)
        .map_err(|_| REFUSAL)
}

/// Settles one human placement: the envelope carries look plus hotbar slot
/// only, and the accepted resolver derives the ray, the footprint and the
/// debit before the shared transaction commits them atomically.
fn settle_place(
    ctx: &mut TickContext<'_>,
    actor: ActorKey,
    call: &RuleCall<'_>,
) -> Result<PhaseReport, ServerError> {
    const REFUSAL: ServerError = ServerError::InvalidInput { field: "placement" };
    let envelope = call
        .command
        .ok_or(ServerError::InvalidInput { field: "command" })?;
    let Command::PlaceBlock(intent) = envelope.command() else {
        return Err(ServerError::InvalidInput { field: "command" });
    };
    let resolved = resolve_place(actor, &intent, &ctx.read()).map_err(|_| REFUSAL)?;
    ctx.transaction().try_place(resolved).map_err(|_| REFUSAL)?;
    Ok(PhaseReport {
        examined: 1,
        applied: 1,
        carried: 0,
        rejected: 0,
    })
}

/// Settles one authority-owned door interaction: the look ray finds the door
/// and the shared system entry commits the toggle atomically. The bed kind
/// belongs to the sleep provider and refuses here without effect.
fn settle_interaction(
    ctx: &mut TickContext<'_>,
    actor: ActorKey,
    interaction: &AuthorityInteraction,
) -> Result<PhaseReport, ServerError> {
    if interaction.kind != InteractionKind::Door {
        return Err(ServerError::InvalidInput { field: "command" });
    }
    let write = toggle_write(&ctx.read(), actor, interaction.look)?;
    let Some(write) = write else {
        return Ok(PhaseReport {
            examined: 1,
            applied: 0,
            carried: 0,
            rejected: 0,
        });
    };
    ctx.transaction()
        // No actor-transaction resolver covers an interaction toggle, so the
        // toggle commits through the system entry whose validation is the
        // same atomic basis check; the producer label is dropped by the
        // commit and leaves no overlay trace.
        .try_system(SystemRule::Support, vec![write])
        .map_err(|_| ServerError::InvalidInput {
            field: "interaction",
        })?;
    Ok(PhaseReport {
        examined: 1,
        applied: 1,
        carried: 0,
        rejected: 0,
    })
}

/// The interaction-phase entry: exactly one of a placement command or an
/// authority-owned door interaction settles; every other shape refuses
/// without effect.
pub fn run(ctx: &mut TickContext<'_>, call: RuleCall<'_>) -> Result<PhaseReport, ServerError> {
    if call.phase != RulePhase::Interaction {
        return Err(ServerError::InvalidInput { field: "phase" });
    }
    match (call.command, call.internal) {
        (Some(_), Some(_)) => Err(ServerError::InvalidInput { field: "command" }),
        (Some(envelope), None) => {
            let session = SessionKey::from_raw(envelope.session())
                .ok_or(ServerError::InvalidInput { field: "session" })?;
            settle_place(ctx, ActorKey::Player(session), &call)
        }
        (None, Some(interaction)) => {
            let interaction = *interaction;
            settle_interaction(ctx, ActorKey::Player(interaction.session), &interaction)
        }
        (None, None) => Err(ServerError::InvalidInput { field: "command" }),
    }
}
