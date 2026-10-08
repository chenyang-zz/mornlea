//! Source companion registration, checked body mapping, the bounded
//! radius-16 pending restore book, and the serial scan advance and reset
//! publication the reducer consumes around its context loan.

use super::actor_placement::RestoreCandidate;
use super::actor_projection::project_companion;
use super::contracts::{
    ActorAux, ActorBody, ActorKey, ActorLifecycle, ActorRecord, ActorRuntime, InventoryRecord,
    RuleEffect, ServerError,
};
use super::pending_restore::{PendingRestore, RestoreKind, RestoreProgress};
use super::state::{ResidentTickState, TickContext};
use mornlea_domain::{
    ChunkPos, CompanionId, CraftingSize, Dimension, FiniteVec3, HotbarSlot, LookAngles,
    MotionState, MotionStateParts, SurvivalState, SurvivalStateParts,
};
use mornlea_storage::{
    BACKPACK_SLOTS, CompanionBody, HOTBAR_SLOTS, Inventory, ItemStack, StoredCompanionTask,
};
use std::collections::BTreeMap;

/// Exclusive retained-scan owner. Entries are bounded by checked
/// registration and the book is never cloned; a completed entry stays until
/// authority destruction and contributes no pending keys.
#[derive(Default)]
pub(crate) struct SourceCompanionBook {
    pub(crate) entries: BTreeMap<CompanionId, PendingRestore>,
}

/// One fully checked companion owner before any authority mutation.
pub(crate) struct PreparedCompanion {
    pub(crate) actor: ActorRecord,
    pub(crate) runtime: ActorRuntime,
    pub(crate) inventory: InventoryRecord,
    pub(crate) restore: PendingRestore,
}

/// Existing save-shape refusal field for every checked body mapping below.
const BODY: ServerError = ServerError::InvalidInput {
    field: "actor_save",
};

/// Neutral survival for one pending companion: companions never consume
/// player survival, so health, oxygen and hunger start full and saturation
/// starts empty.
fn neutral_survival() -> Result<SurvivalState, ServerError> {
    SurvivalState::try_new(SurvivalStateParts {
        health: 20,
        oxygen: 300,
        hunger: 20,
        saturation_zero: true,
        armor_points: 0,
    })
    .map_err(|_| BODY)
}

/// Canonical anchor capture for a companion without a saved body: Overworld,
/// the anchor column center, one block above the world top, neutral look and
/// an empty inventory. Startup merge and pending restore share this body.
pub(crate) fn anchor_body(id: CompanionId, anchor: ChunkPos) -> CompanionBody {
    CompanionBody {
        id: mornlea_storage::PlayerId::from_bytes(id.bytes()),
        dimension: 0,
        position: [
            (anchor.x() as f32) * 16.0 + 0.5,
            321.0,
            (anchor.z() as f32) * 16.0 + 0.5,
        ],
        yaw: 0.0,
        pitch: 0.0,
        inventory: Inventory::default(),
    }
}

/// Prepares every companion owner before the registration seam mutates any
/// authority collection. A saved body keeps its captured pose, look and
/// inventory; a missing body falls back to the canonical anchor capture.
pub(crate) fn prepare(
    id: CompanionId,
    anchor: ChunkPos,
    body: Option<CompanionBody>,
) -> Result<PreparedCompanion, ServerError> {
    let supplied = body.is_some();
    let body = body.unwrap_or_else(|| anchor_body(id, anchor));
    // Identity belongs to the registration seam; every later shape refusal
    // keeps the source save field.
    if body.id.to_bytes() != id.bytes() {
        return Err(ServerError::InvalidInput {
            field: "source_companion_body",
        });
    }
    // Runtime dimension policy is fixed: companions live in the Overworld,
    // and a raw non-Overworld body must refuse before projection can
    // silently normalize it back.
    if body.dimension != 0 {
        return Err(BODY);
    }
    let position = FiniteVec3::try_new(body.position).map_err(|_| BODY)?;
    let motion = MotionState::new(MotionStateParts {
        position,
        velocity: FiniteVec3::try_new([0.0; 3]).map_err(|_| BODY)?,
        on_ground: false,
    });
    let look = LookAngles::try_new(body.yaw, body.pitch).map_err(|_| BODY)?;
    let survival = neutral_survival()?;
    let pending = ActorRecord::try_new(
        ActorKey::Companion(id),
        ActorLifecycle::Pending,
        Dimension::OVERWORLD,
        motion,
        look,
        survival,
        ActorBody::Companion(body.clone()),
    )
    .map_err(|_| BODY)?;
    let mut slots = [ItemStack::default(); 36];
    slots[0..HOTBAR_SLOTS].copy_from_slice(&body.inventory.hotbar.slots);
    slots[HOTBAR_SLOTS..HOTBAR_SLOTS + BACKPACK_SLOTS].copy_from_slice(&body.inventory.backpack);
    let selected = HotbarSlot::new(body.inventory.hotbar.selected).map_err(|_| BODY)?;
    // Armor and the personal grid never persist for companions.
    let inventory = InventoryRecord::try_new(
        slots,
        selected,
        [ItemStack::default(); 4],
        [ItemStack::default(); 9],
        CraftingSize::Personal,
    )
    .map_err(|_| BODY)?;
    // The projection helper is the one body validator; its checked output
    // becomes the canonical body so an invalid source shape is never
    // normalized into a resident record.
    let canonical = project_companion(&pending, Some(&inventory))?;
    let actor = ActorRecord::try_new(
        ActorKey::Companion(id),
        ActorLifecycle::Pending,
        Dimension::OVERWORLD,
        motion,
        look,
        survival,
        ActorBody::Companion(canonical),
    )
    .map_err(|_| BODY)?;
    let runtime = ActorRuntime {
        key: ActorKey::Companion(id),
        controls: None,
        has_view: false,
        reset: false,
        attack_cooldown: 0,
        hurt_cooldown: 0,
        burn_cooldown: 0,
        oxygen: 300,
        peak_y: body.position[1],
        exhaustion_milli: 0,
        saturation_milli: 0,
        since_damage_ticks: 0,
        drown_ticks: 0,
        starvation_ticks: 0,
        eating: None,
        bow: None,
        path: None,
        aux: ActorAux::Companion {
            generation: 0,
            attempt: 0,
            task: StoredCompanionTask::default(),
            mining_target: None,
        },
    };
    // A saved body contributes exactly one restore candidate at its captured
    // pose; a missing body relies on the anchor spawn scan alone.
    let candidates = if supplied {
        vec![RestoreCandidate {
            dimension: Dimension::OVERWORLD,
            position: body.position,
            require_support: false,
        }]
    } else {
        Vec::new()
    };
    Ok(PreparedCompanion {
        actor,
        runtime,
        inventory,
        restore: PendingRestore::try_new(
            RestoreKind::Companion,
            Dimension::OVERWORLD,
            anchor,
            16,
            candidates,
        )?,
    })
}

/// Advances every retained pending companion scan in id byte order. Only a
/// Pending actor with its indexed companion runtime and inventory advances;
/// Active and other lifecycles skip without new policy, and Waiting or
/// Exhausted progress stays retained. Activation clones this fixed
/// actor/runtime only, stages one constant two-effect compound at the chosen
/// pose with zero velocity, ground certificate and reset marker, preserving
/// look, body, inventory and companion aux, then discards only this id's
/// already-examined inactive action envelopes.
pub(crate) fn advance(
    book: &mut SourceCompanionBook,
    context: &mut TickContext<'_>,
) -> Result<(), ServerError> {
    for (id, entry) in book.entries.iter_mut() {
        let effects = {
            let view = context.read();
            let key = ActorKey::Companion(*id);
            let actor = view.actor(key).ok_or(ServerError::Internal {
                invariant: "source companion registration",
            })?;
            let runtime = view.runtime(key).ok_or(ServerError::Internal {
                invariant: "source companion registration",
            })?;
            view.inventory(key).ok_or(ServerError::Internal {
                invariant: "source companion registration",
            })?;
            if actor.key != key
                || !matches!(actor.body, ActorBody::Companion(_))
                || runtime.key != key
                || !matches!(runtime.aux, ActorAux::Companion { .. })
            {
                return Err(ServerError::Internal {
                    invariant: "source companion registration",
                });
            }
            if actor.lifecycle != ActorLifecycle::Pending {
                continue;
            }
            let environment = view.environment().ok_or(ServerError::Internal {
                invariant: "source companion environment",
            })?;
            let progress = entry.advance(&view, environment.tunables.eye_height())?;
            let RestoreProgress::Activated(chosen) = progress else {
                continue;
            };
            let mut actor = actor.clone();
            actor.dimension = chosen.dimension;
            actor.lifecycle = ActorLifecycle::Active;
            actor.motion = MotionState::new(MotionStateParts {
                position: FiniteVec3::try_new(chosen.position).map_err(|_| {
                    ServerError::Internal {
                        invariant: "source companion activation",
                    }
                })?,
                velocity: FiniteVec3::try_new([0.0; 3]).map_err(|_| ServerError::Internal {
                    invariant: "source companion activation",
                })?,
                on_ground: chosen.on_ground,
            });
            let mut runtime = runtime.clone();
            runtime.controls = None;
            runtime.peak_y = chosen.position[1];
            runtime.reset = true;
            vec![RuleEffect::Actor(actor), RuleEffect::Runtime(runtime)]
        };
        // One constant-size compound publishes both prepared owners or neither.
        context
            .stage(RuleEffect::Compound(effects))
            .map_err(|_| ServerError::Internal {
                invariant: "source companion activation",
            })?;
        context.discard_source_companion_actions(*id);
    }
    Ok(())
}

/// Clears the publication reset marker for every registered companion whose
/// resident actor is currently Active, preserving the source result cadence
/// even with no observer. Only registered book identities are touched; no
/// other runtime lane changes.
pub(crate) fn finish_publication(book: &SourceCompanionBook, residents: &mut ResidentTickState) {
    for id in book.entries.keys() {
        let key = ActorKey::Companion(*id);
        let active = residents
            .actors
            .iter()
            .any(|actor| actor.key == key && actor.lifecycle == ActorLifecycle::Active);
        if !active {
            continue;
        }
        if let Some(runtime) = residents.runtimes.get_mut(&key) {
            runtime.reset = false;
        }
    }
}
