//! Container views: open, whole and partial moves, quick moves and close.
//!
//! This provider settles the unified container views against the accepted
//! inventory debit and credit policy: inventory `0..35`, furnace input `36`,
//! fuel `37` and output `38`, chest cells `36..62`. Crafting views (grid
//! `0..8`, inventory `9..44`) belong to the crafting provider, which also
//! owns workbench opens; this provider owns container panel drops.
//!
//! Mirrored Go rows, each cited at its site:
//!
//! - `packages/server/sim/entity/container.go` (`openContainer`): the open
//!   ray classifies the hit block, furnace and chest opens establish the
//!   single viewed container. Workbench anchor and view mutual exclusion
//!   belong to the crafting lifecycle integration.
//! - `packages/server/sim/entity/container.go` (`applyContainerMove`) and
//!   `packages/server/sim/entity/furnace.go` (`moveFurnaceStack`,
//!   `moveFurnaceStackAmount`, `setFurnaceViewSlot`): view and exact reference
//!   validation, whole and partial transfer semantics, the furnace slot
//!   constraints (smeltable input with progress reset on kind change, coal
//!   fuel, output whitelist), and the output slot refused as a whole or
//!   partial destination.
//! - `packages/server/sim/entity/quick_move.go` (`quickMoveChestStack`,
//!   `quickMoveFurnaceStack`): container sources credit through the pickup
//!   order with the remainder kept at the source, backpack sources land in
//!   the first fitting chest cell in ascending order, and furnace backpack
//!   sources land smeltable input before coal fuel, never the output.
//! - `packages/shared/core/inventory.go` (`Inventory.AddStack`): the
//!   four-phase pickup order the container-to-backpack credit and the bench
//!   repack preview share.
//! - `packages/server/sim/entity/crafting.go` (`canRepackCrafting`,
//!   `closeWorkbench`): the repack preview every commit runs, and the close
//!   gate that invalidates the view only after the bench still repacks.
//! - `packages/server/sim/entity/tick.go` (`CommandCloseFurnace`): closing a
//!   viewed container clears the view.
//!
/// The context owns the complete net viewer set. Open binds the exact Ready
/// record immediately, and close removes it; the reducer commits the set and
/// prunes retired sessions. Publication owns later reach invalidation.
use mornlea_domain::{
    BlockPos, Command, CommandEnvelope, ContainerKind, ContainerMove, ContainerRef, CraftingSize,
    Dimension, LookAngles, PartialMove, RejectReason, StackSource, StackView,
};
use mornlea_engine::native::contracts::raycast::{Ray, RayCursor, RaycastOp};
use mornlea_engine::native::raycast::NativeRaycast;
use mornlea_storage::ItemStack;

use crate::contracts::{
    ActorKey, ActorLifecycle, ActorRecord, BlockObservation, ContainerRecord, ContainerSlots,
    InventoryPatch, InventoryRecord, PhaseReport, RuleCall, RuleEffect, RulePhase, RuleReject,
    ServerError, SessionKey, ViewLease,
};
use crate::core::command_outcome::{CommandDisposition, CommandResult, RejectionStage};
use crate::core::interaction::{look_direction, normalized_direction, target_block};
use crate::rules::{crafting, drops};
use crate::state::{AuthorityReadView, TickContext};

/// Unified player inventory length: hotbar `0..8` plus backpack `9..35`
/// (`core.InventorySlots`, `packages/shared/core/inventory.go`).
const INVENTORY_SLOTS: usize = 36;

/// First chest cell in the unified chest view (`core.ChestFirstSlot`,
/// `packages/shared/core/chest.go`).
const CHEST_FIRST: usize = 36;

/// Chest cell count (`core.ChestSlots`, `packages/shared/core/chest.go`).
const CHEST_SLOTS: usize = 27;

/// Unified chest view length (`core.ChestViewSlots`).
const CHEST_VIEW_SLOTS: usize = 63;

/// Furnace input cell (`core.FurnaceInputSlot`,
/// `packages/shared/core/furnace.go`).
const FURNACE_INPUT: usize = 36;

/// Furnace fuel cell (`core.FurnaceFuelSlot`).
const FURNACE_FUEL: usize = 37;

/// Furnace output cell (`core.FurnaceOutputSlot`): taking only, never a move
/// destination.
const FURNACE_OUTPUT: usize = 38;

/// Unified furnace view length (`core.FurnaceViewSlots`).
const FURNACE_VIEW_SLOTS: usize = 39;

/// The absent item number (`core.ItemNone`).
const ITEM_NONE: u16 = 0;

/// The only fuel the fuel cell accepts (`core.ItemCoal`).
const ITEM_COAL: u16 = 5;

/// Furnace stack ceiling (`core.MaxStackCount`,
/// `packages/shared/core/item.go`).
const MAX_STACK_COUNT: u8 = 64;

/// Furnace block (`core.FurnaceID`).
const FURNACE_BLOCK: u16 = 9;

/// Chest block (`core.ChestID`).
const CHEST_BLOCK: u16 = 11;

/// Settles one container admission or runs the settlement pass, exactly one
/// of which the call shape names.
///
/// Admission (`RulePhase::PlayerCommand` with a command) routes the container
/// family — open, close, whole moves, container-view partial and quick moves,
/// and container drops — into the deferred container phase and reports applied. It
/// stages nothing observable, so a deferred move settles against the state
/// the drain sees, including a generation the chunk replaced after admission.
/// Every other command kind, including the inventory and crafting stack views,
/// belongs to its own provider and refuses
/// without effect.
///
/// Drain (`RulePhase::ContainerMove` with no command, actor or internal
/// value) settles the whole deferred queue in admission order against the
/// staging overlay and reports the examined, applied and rejected counts. One
/// refused envelope never stops the pass, so same-tick multi-player moves
/// settle in stable order. Replaying a queue re-validates every envelope, and
/// an already-settled move then refuses idempotently with nothing staged. Any
/// other shape refuses without effect.
pub fn run(ctx: &mut TickContext<'_>, call: RuleCall<'_>) -> Result<PhaseReport, ServerError> {
    match call.phase {
        RulePhase::PlayerCommand => admit(ctx, &call),
        RulePhase::ContainerMove => drain(ctx, &call),
        _ => Err(ServerError::InvalidInput { field: "phase" }),
    }
}

/// One admission: route a container-family envelope into the deferred
/// container phase. The envelope's session must already be a routable key;
/// every authority check waits for the drain.
fn admit(ctx: &mut TickContext<'_>, call: &RuleCall<'_>) -> Result<PhaseReport, ServerError> {
    if call.actor.is_some() || call.internal.is_some() {
        return Err(ServerError::InvalidInput { field: "command" });
    }
    let envelope = call
        .command
        .ok_or(ServerError::InvalidInput { field: "command" })?;
    let owned = match envelope.command() {
        Command::OpenContainer(_) | Command::CloseContainer | Command::MoveContainer(_) => true,
        Command::MovePartial(partial) => matches!(partial.view(), StackView::Container(_)),
        Command::QuickMove(source) | Command::DropStack(source) => {
            matches!(source.view(), StackView::Container(_))
        }
        _ => false,
    };
    if !owned {
        return Err(ServerError::InvalidInput { field: "command" });
    }
    SessionKey::from_raw(envelope.session())
        .ok_or(ServerError::InvalidInput { field: "session" })?;
    ctx.defer(*envelope, RulePhase::ContainerMove)?;
    Ok(PhaseReport {
        examined: 1,
        applied: 1,
        carried: 0,
        rejected: 0,
    })
}

/// The single settlement pass over the deferred container queue in admission
/// order. Each envelope settles against the overlay the earlier envelopes
/// left and records its outcome; open binds the exact view and close clears it.
/// Production runs this once per tick with a fresh staging context.
fn drain(ctx: &mut TickContext<'_>, call: &RuleCall<'_>) -> Result<PhaseReport, ServerError> {
    if call.actor.is_some() || call.command.is_some() || call.internal.is_some() {
        return Err(ServerError::InvalidInput { field: "phase" });
    }
    let queued = ctx.deferred(RulePhase::ContainerMove);
    let mut examined = 0;
    let mut applied = 0;
    let mut rejected = 0;
    for envelope in queued {
        examined += 1;
        if settle_command(ctx, &envelope).is_ok() {
            applied += 1;
        } else {
            rejected += 1;
        }
    }
    Ok(PhaseReport {
        examined,
        applied,
        carried: 0,
        rejected,
    })
}

/// Settles one command against the current authority view without queue history.
/// Every refusal leaves the inventory, container, drops and lease unchanged.
pub fn settle_command(
    ctx: &mut TickContext<'_>,
    envelope: &CommandEnvelope,
) -> Result<PhaseReport, RuleReject> {
    let session = SessionKey::from_raw(envelope.session())
        .ok_or(RuleReject::Wire(RejectReason::InvalidInput))?;
    let supported = match envelope.command() {
        Command::OpenContainer(_) | Command::CloseContainer | Command::MoveContainer(_) => true,
        Command::MovePartial(partial) => matches!(partial.view(), StackView::Container(_)),
        Command::QuickMove(source) | Command::DropStack(source) => {
            matches!(source.view(), StackView::Container(_))
        }
        _ => false,
    };
    if !supported {
        return Err(RuleReject::Wire(RejectReason::InvalidInput));
    }
    if matches!(
        envelope.command(),
        Command::MoveContainer(_)
            | Command::MovePartial(_)
            | Command::QuickMove(_)
            | Command::DropStack(_)
    ) {
        let view = ctx.read();
        let actor = ActorKey::Player(session);
        let record = view
            .actor(actor)
            .filter(|record| record.lifecycle == ActorLifecycle::Active)
            .ok_or(RuleReject::Wire(RejectReason::PlayerNotReady))?;
        if record.dimension != Dimension::OVERWORLD {
            return Err(RuleReject::Wire(RejectReason::InvalidInput));
        }
        view.inventory(actor)
            .ok_or(RuleReject::Wire(RejectReason::PlayerNotReady))?;
    }
    let accepted = match envelope.command() {
        Command::OpenContainer(look) => settle_open(ctx, session, look)?,
        Command::CloseContainer => settle_close(ctx, session)?,
        Command::MoveContainer(movement) => {
            settle_whole(ctx, *envelope, movement).map_err(TransferFailure::into_raw)?
        }
        Command::MovePartial(partial) => {
            let StackView::Container(reference) = partial.view() else {
                return Err(RuleReject::Wire(RejectReason::InvalidInput));
            };
            settle_partial(ctx, *envelope, reference, partial).map_err(TransferFailure::into_raw)?
        }
        Command::QuickMove(source) => {
            let StackView::Container(reference) = source.view() else {
                return Err(RuleReject::Wire(RejectReason::InvalidInput));
            };
            settle_quick(ctx, *envelope, reference, source).map_err(TransferFailure::into_raw)?
        }
        Command::DropStack(source) => {
            let StackView::Container(reference) = source.view() else {
                return Err(RuleReject::Wire(RejectReason::InvalidInput));
            };
            return settle_drop(ctx, *envelope, reference, source)
                .map_err(TransferFailure::into_raw);
        }
        _ => return Err(RuleReject::Wire(RejectReason::InvalidInput)),
    };
    if !accepted {
        return Err(RuleReject::Wire(RejectReason::InvalidInput));
    }
    Ok(PhaseReport {
        examined: 1,
        applied: 1,
        carried: 0,
        rejected: 0,
    })
}

/// Ordinary transfer refusals retain their provenance across the raw wrapper.
/// A wire-shaped staging failure is still an invariant failure for the live tick.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TransferFailure {
    Refused(RejectReason),
    Trusted(RuleReject),
}

impl TransferFailure {
    fn into_raw(self) -> RuleReject {
        match self {
            Self::Refused(reason) => RuleReject::Wire(reason),
            Self::Trusted(error) => error,
        }
    }

    fn into_live(self, invariant: &'static str) -> CommandResult {
        match self {
            Self::Refused(reason) => Ok(CommandDisposition::Refused(reason)),
            Self::Trusted(_) => Err(ServerError::Internal { invariant }),
        }
    }
}

/// The live late transfer owner consumes only its three command families.
/// Authority readiness is read here, after combat and before final view invalidation.
pub(crate) fn settle_transfer(
    ctx: &mut TickContext<'_>,
    envelope: &CommandEnvelope,
) -> CommandResult {
    let owned = match envelope.command() {
        Command::MoveContainer(_) => true,
        Command::MovePartial(partial) => matches!(partial.view(), StackView::Container(_)),
        Command::QuickMove(source) => matches!(source.view(), StackView::Container(_)),
        _ => false,
    };
    if !owned {
        return Ok(CommandDisposition::Unowned);
    }
    let session = SessionKey::from_raw(envelope.session()).ok_or(ServerError::Internal {
        invariant: "container transfer session",
    })?;
    {
        let view = ctx.read();
        let actor = ActorKey::Player(session);
        let Some(record) = view
            .actor(actor)
            .filter(|record| record.lifecycle == ActorLifecycle::Active)
        else {
            return Ok(CommandDisposition::Refused(RejectReason::PlayerNotReady));
        };
        if record.dimension != Dimension::OVERWORLD {
            return Ok(CommandDisposition::Refused(RejectReason::InvalidInput));
        }
        if view.inventory(actor).is_none() {
            return Err(ServerError::Internal {
                invariant: "container transfer owner",
            });
        }
    }
    let result = match envelope.command() {
        Command::MoveContainer(movement) => settle_whole(ctx, *envelope, movement),
        Command::MovePartial(partial) => {
            let StackView::Container(reference) = partial.view() else {
                unreachable!()
            };
            settle_partial(ctx, *envelope, reference, partial)
        }
        Command::QuickMove(source) => {
            let StackView::Container(reference) = source.view() else {
                unreachable!()
            };
            settle_quick(ctx, *envelope, reference, source)
        }
        _ => unreachable!(),
    };
    match result {
        Ok(true) => Ok(CommandDisposition::Settled(PhaseReport {
            examined: 1,
            applied: 1,
            carried: 0,
            rejected: 0,
        })),
        Ok(false) => Ok(CommandDisposition::Refused(RejectReason::InvalidInput)),
        Err(error) => error.into_live("container transfer staging"),
    }
}

/// Container drops consume their exact late lease and debit only after the
/// foot output and durable source touch pass one atomic staging boundary.
pub(crate) fn settle_drop_command(
    ctx: &mut TickContext<'_>,
    envelope: &CommandEnvelope,
) -> CommandResult {
    let Command::DropStack(source) = envelope.command() else {
        return Ok(CommandDisposition::Unowned);
    };
    let StackView::Container(reference) = source.view() else {
        return Ok(CommandDisposition::Unowned);
    };
    let session = SessionKey::from_raw(envelope.session()).ok_or(ServerError::Internal {
        invariant: "container drop session",
    })?;
    {
        let view = ctx.read();
        let actor = ActorKey::Player(session);
        let Some(record) = view
            .actor(actor)
            .filter(|record| record.lifecycle == ActorLifecycle::Active)
        else {
            return Ok(CommandDisposition::Refused(RejectReason::PlayerNotReady));
        };
        if record.dimension != Dimension::OVERWORLD {
            return Ok(CommandDisposition::Refused(RejectReason::InvalidInput));
        }
        if view.inventory(actor).is_none() {
            return Err(ServerError::Internal {
                invariant: "container drop owner",
            });
        }
    }
    match settle_drop(ctx, *envelope, reference, source) {
        Ok(report) => Ok(CommandDisposition::Settled(report)),
        Err(error) => error.into_live("container drop staging"),
    }
}

/// Drain the admitted bag once. Only semantic owned outcomes enter the wire
/// collector; raw compatibility families keep their separately qualified policy.
pub(crate) fn drain_commands(ctx: &mut TickContext<'_>) -> Result<PhaseReport, ServerError> {
    let mut report = PhaseReport {
        examined: 0,
        applied: 0,
        carried: 0,
        rejected: 0,
    };
    for envelope in ctx.deferred(RulePhase::ContainerMove) {
        report.examined += 1;
        match settle_transfer(ctx, &envelope)? {
            CommandDisposition::Settled(settled) => {
                report.applied += settled.applied;
                report.carried += settled.carried;
                report.rejected += settled.rejected;
            }
            CommandDisposition::Refused(reason) => {
                ctx.record_command_rejection(&envelope, reason, RejectionStage::Settlement)?;
                report.rejected += 1;
            }
            CommandDisposition::Unowned => match settle_drop_command(ctx, &envelope)? {
                CommandDisposition::Settled(settled) => {
                    report.applied += settled.applied;
                    report.carried += settled.carried;
                    report.rejected += settled.rejected;
                }
                CommandDisposition::Refused(reason) => {
                    ctx.record_command_rejection(&envelope, reason, RejectionStage::Settlement)?;
                    report.rejected += 1;
                }
                CommandDisposition::Unowned => {
                    if settle_command(ctx, &envelope).is_ok() {
                        report.applied += 1;
                    } else {
                        report.rejected += 1;
                    }
                }
            },
        }
    }
    Ok(report)
}

/// The viewer basis one settlement needs: an active player in the container
/// dimension with a staged inventory. Container references are inherently
/// overworld values (`ContainerRef` in `mornlea_domain::locations`), so a
/// viewer anywhere else retains the existing refusal policy. Cross-dimension
/// parity is separately unqualified.
struct ViewerBasis {
    session: SessionKey,
    actor: ActorKey,
    inventory: InventoryRecord,
}

fn viewer_basis(ctx: &TickContext<'_>, envelope: CommandEnvelope) -> Option<ViewerBasis> {
    let session = SessionKey::from_raw(envelope.session())?;
    let actor = ActorKey::Player(session);
    let record = ctx.read().actor(actor).cloned()?;
    if record.lifecycle != ActorLifecycle::Active {
        return None;
    }
    if record.dimension != Dimension::OVERWORLD {
        return None;
    }
    let inventory = ctx.read().inventory(actor).copied()?;
    Some(ViewerBasis {
        session,
        actor,
        inventory,
    })
}

/// The move basis one transfer needs: the viewer plus the live container the
/// reference names. The lookup key carries chunk, kind, slot and generation,
/// so a retired generation or an unknown slot misses exactly like the Go
/// generation mismatch in `chestView` / `furnaceView`.
struct MoveBasis {
    viewer: ViewerBasis,
    stored: ContainerRecord,
}

fn move_basis(
    ctx: &TickContext<'_>,
    envelope: CommandEnvelope,
    reference: ContainerRef,
) -> Option<MoveBasis> {
    let viewer = viewer_basis(ctx, envelope)?;
    let view = ctx.read();
    if view.viewer(viewer.session).map(|lease| lease.reference()) != Some(reference)
        || !view.ready_chunk(crate::contracts::ChunkKey {
            dimension: Dimension::OVERWORLD,
            pos: reference.chunk(),
        })
    {
        return None;
    }
    let stored = view.container(reference)?;
    match (&stored.slots, reference.kind()) {
        (ContainerSlots::Chest(_), ContainerKind::Chest)
        | (ContainerSlots::Furnace { .. }, ContainerKind::Furnace) => {}
        _ => return None,
    }
    Some(MoveBasis { viewer, stored })
}

/// Raw callers retain their typed rejects; live admission separates semantic
/// refusal from trusted atomic staging before publishing an outcome.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LifecycleFailure {
    Refused(RejectReason),
    Trusted(RuleReject),
}

impl LifecycleFailure {
    fn into_raw(self) -> RuleReject {
        match self {
            Self::Refused(reason) => RuleReject::Wire(reason),
            Self::Trusted(reject) => reject,
        }
    }

    fn into_live(self) -> CommandResult {
        match self {
            Self::Refused(reason) => Ok(CommandDisposition::Refused(reason)),
            Self::Trusted(_) => Err(ServerError::Internal {
                invariant: "container lifecycle staging",
            }),
        }
    }
}

fn stage_lifecycle_effects(
    ctx: &mut TickContext<'_>,
    effects: Vec<RuleEffect>,
) -> Result<(), LifecycleFailure> {
    ctx.stage(RuleEffect::Compound(effects))
        .map_err(LifecycleFailure::Trusted)
}

/// Only a semantic no-target result may try the bench owner. A failed trusted
/// mutation never falls through, and explicit close has no Active-player gate.
pub(crate) fn admit_lifecycle(
    ctx: &mut TickContext<'_>,
    envelope: &CommandEnvelope,
) -> CommandResult {
    if !matches!(
        envelope.command(),
        Command::OpenContainer(_) | Command::CloseContainer
    ) {
        return Ok(CommandDisposition::Unowned);
    }
    let session = SessionKey::from_raw(envelope.session()).ok_or(ServerError::Internal {
        invariant: "container lifecycle session",
    })?;
    let result = match envelope.command() {
        Command::OpenContainer(look) => {
            let view = ctx.read();
            let actor = ActorKey::Player(session);
            if !view
                .actor(actor)
                .is_some_and(|record| record.lifecycle == ActorLifecycle::Active)
            {
                return Ok(CommandDisposition::Refused(RejectReason::PlayerNotReady));
            }
            view.inventory(actor).ok_or(ServerError::Internal {
                invariant: "container lifecycle owner",
            })?;
            view.environment().ok_or(ServerError::Internal {
                invariant: "container lifecycle environment",
            })?;
            match checked_open(ctx, session, look) {
                Err(LifecycleFailure::Refused(RejectReason::NoTarget)) => {
                    return crafting::admit_bench_open(ctx, envelope, look);
                }
                result => result,
            }
        }
        Command::CloseContainer => checked_close(ctx, session),
        _ => return Ok(CommandDisposition::Unowned),
    };
    match result {
        Ok(true) => Ok(CommandDisposition::Settled(PhaseReport {
            examined: 1,
            applied: 1,
            carried: 0,
            rejected: 0,
        })),
        Ok(false) => Ok(CommandDisposition::Refused(RejectReason::InvalidInput)),
        Err(failure) => failure.into_live(),
    }
}

/// Settles one open: the authority ray from the current look must meet a
/// furnace or a chest block within reach. A workbench hit refuses without
/// effect — the workbench is an ordinary block whose opens only widen the
/// crafting grid, owned by the crafting provider — as does a miss or an
/// unobserved cell the authority cannot certify. A successful open immediately
/// owns the single viewed container. The Ready slot binds now,
/// so a later move cannot choose a different live record.
fn settle_open(
    ctx: &mut TickContext<'_>,
    session: SessionKey,
    look: LookAngles,
) -> Result<bool, RuleReject> {
    checked_open(ctx, session, look).map_err(LifecycleFailure::into_raw)
}

fn checked_open(
    ctx: &mut TickContext<'_>,
    session: SessionKey,
    look: LookAngles,
) -> Result<bool, LifecycleFailure> {
    let actor = ActorKey::Player(session);
    let view = ctx.read();
    let record = match view.actor(actor) {
        Some(record) if record.lifecycle == ActorLifecycle::Active => record.clone(),
        _ => return Err(LifecycleFailure::Refused(RejectReason::PlayerNotReady)),
    };
    view.inventory(actor)
        .ok_or(LifecycleFailure::Refused(RejectReason::PlayerNotReady))?;
    if record.dimension != Dimension::OVERWORLD {
        return Err(LifecycleFailure::Refused(RejectReason::InvalidInput));
    }
    let basis = match actor_basis(&view, &record) {
        Some(basis) => basis,
        None => return Err(LifecycleFailure::Refused(RejectReason::InvalidInput)),
    };
    let direction = look_direction(look.yaw(), look.pitch());
    let hit = match cast_ray(&view, basis.dimension, basis.eye, direction, basis.reach) {
        Ok(hit) => hit,
        Err(RayFailure::Unavailable) => {
            return Err(LifecycleFailure::Refused(RejectReason::ChunkNotReady));
        }
        Err(RayFailure::Invalid) => {
            return Err(LifecycleFailure::Refused(RejectReason::InvalidRay));
        }
    };
    let hit = hit.ok_or(LifecycleFailure::Refused(RejectReason::NoTarget))?;
    let kind = match hit.observed.block {
        FURNACE_BLOCK => ContainerKind::Furnace,
        CHEST_BLOCK => ContainerKind::Chest,
        _ => return Err(LifecycleFailure::Refused(RejectReason::NoTarget)),
    };
    // A recognized physical target is sneak-ineligible even without an active slot.
    if view
        .runtime(actor)
        .and_then(|runtime| runtime.controls)
        .is_some_and(|control| control.actions().sneaking)
    {
        return Err(LifecycleFailure::Refused(RejectReason::InvalidInput));
    }
    if !view.ready_chunk(hit.observed.key) {
        return Err(LifecycleFailure::Refused(RejectReason::ChunkNotReady));
    }
    let stored = view
        .container_at(Dimension::OVERWORLD, hit.observed.pos, kind)
        .ok_or(LifecycleFailure::Refused(RejectReason::NoTarget))?;
    ctx.stage(RuleEffect::Viewer {
        session,
        view: Some(ViewLease::new(session, stored.reference)),
    })
    .map_err(LifecycleFailure::Trusted)?;
    Ok(true)
}

/// Settles one close: the bench repack preview runs over the current pack
/// and grid, and only a successful preview clears the staged lease, mirroring
/// the Go close row that refuses while the workbench grid cannot reclaim.
/// The preview mutates nothing until the inventory and lease stage together.
fn settle_close(ctx: &mut TickContext<'_>, session: SessionKey) -> Result<bool, RuleReject> {
    checked_close(ctx, session).map_err(LifecycleFailure::into_raw)
}

fn checked_close(ctx: &mut TickContext<'_>, session: SessionKey) -> Result<bool, LifecycleFailure> {
    let actor = ActorKey::Player(session);
    let mut effects = Vec::with_capacity(2);
    let mut was_workbench = false;
    if let Some(before) = ctx.read().inventory(actor).copied() {
        was_workbench = before.crafting_size == CraftingSize::Workbench;
        let after = crafting::closed_inventory(before)
            .ok_or(LifecycleFailure::Refused(RejectReason::InvalidInput))?;
        if after != before {
            effects.push(RuleEffect::Inventory(
                InventoryPatch::try_new(actor, before, after).map_err(LifecycleFailure::Trusted)?,
            ));
        }
    }
    effects.push(RuleEffect::Viewer {
        session,
        view: None,
    });
    stage_lifecycle_effects(ctx, effects)?;
    // Retain source close intent only after inventory and lease settle together.
    if was_workbench {
        ctx.record_crafting_command_publication_dirty(session);
    }
    Ok(true)
}

/// Settles one whole-stack container move, the exact `moveChestStack` /
/// `moveFurnaceStack` row: both ends inside the pack reuse the inventory
/// whole-move semantics, and any cross-region transfer merges through the
/// kind's slot constraints with copies committed only after the repack
/// preview.
fn settle_whole(
    ctx: &mut TickContext<'_>,
    envelope: CommandEnvelope,
    movement: ContainerMove,
) -> Result<bool, TransferFailure> {
    let basis = move_basis(ctx, envelope, movement.container())
        .ok_or(TransferFailure::Refused(RejectReason::InvalidInput))?;
    let from = movement.from() as usize;
    let to = movement.to() as usize;
    let computed = match &basis.stored.slots {
        ContainerSlots::Chest(cells) => move_chest_whole(&basis.viewer.inventory, cells, from, to)
            .map(|(inventory, cells)| (inventory, ContainerSlots::Chest(cells))),
        ContainerSlots::Furnace {
            slots,
            fuel,
            progress,
        } => {
            let view = FurnaceView {
                input: slots[0],
                fuel_slot: slots[1],
                output: slots[2],
                fuel: *fuel,
                progress: *progress,
            };
            move_furnace_whole(&basis.viewer.inventory, &view, from, to).map(|(inventory, next)| {
                (
                    inventory,
                    ContainerSlots::Furnace {
                        slots: [next.input, next.fuel_slot, next.output],
                        fuel: next.fuel,
                        progress: next.progress,
                    },
                )
            })
        }
    };
    let (inventory, slots) =
        computed.ok_or(TransferFailure::Refused(RejectReason::InvalidInput))?;
    commit(ctx, &basis.viewer, &basis.stored, inventory, slots)?;
    Ok(true)
}

/// Settles one container-view partial move, the exact
/// `moveChestStackAmount` / `moveFurnaceStackAmount` row: the amount derives
/// from the settlement-time source stack, a partial move never swaps, and the
/// furnace output stays refused as a destination even though the protocol
/// admits the index.
fn settle_partial(
    ctx: &mut TickContext<'_>,
    envelope: CommandEnvelope,
    reference: ContainerRef,
    partial: PartialMove,
) -> Result<bool, TransferFailure> {
    let basis = move_basis(ctx, envelope, reference)
        .ok_or(TransferFailure::Refused(RejectReason::InvalidInput))?;
    let from = partial.from() as usize;
    let to = partial.to() as usize;
    let computed = match &basis.stored.slots {
        ContainerSlots::Chest(cells) => {
            move_chest_amount(&basis.viewer.inventory, cells, from, to, partial.single())
                .map(|(inventory, cells)| (inventory, ContainerSlots::Chest(cells)))
        }
        ContainerSlots::Furnace {
            slots,
            fuel,
            progress,
        } => {
            let view = FurnaceView {
                input: slots[0],
                fuel_slot: slots[1],
                output: slots[2],
                fuel: *fuel,
                progress: *progress,
            };
            move_furnace_amount(&basis.viewer.inventory, &view, from, to, partial.single()).map(
                |(inventory, next)| {
                    (
                        inventory,
                        ContainerSlots::Furnace {
                            slots: [next.input, next.fuel_slot, next.output],
                            fuel: next.fuel,
                            progress: next.progress,
                        },
                    )
                },
            )
        }
    };
    let (inventory, slots) =
        computed.ok_or(TransferFailure::Refused(RejectReason::InvalidInput))?;
    commit(ctx, &basis.viewer, &basis.stored, inventory, slots)?;
    Ok(true)
}

/// Settles one container-view quick move, the exact `quickMoveChestStack` /
/// `quickMoveFurnaceStack` row: container sources credit through the pickup
/// order with the remainder kept at the source, and pack sources land by the
/// kind's target order.
fn settle_quick(
    ctx: &mut TickContext<'_>,
    envelope: CommandEnvelope,
    reference: ContainerRef,
    source: StackSource,
) -> Result<bool, TransferFailure> {
    let basis = move_basis(ctx, envelope, reference)
        .ok_or(TransferFailure::Refused(RejectReason::InvalidInput))?;
    let from = source.slot() as usize;
    let computed = match &basis.stored.slots {
        ContainerSlots::Chest(cells) => quick_chest(&basis.viewer.inventory, cells, from)
            .map(|(inventory, cells)| (inventory, ContainerSlots::Chest(cells))),
        ContainerSlots::Furnace {
            slots,
            fuel,
            progress,
        } => {
            let view = FurnaceView {
                input: slots[0],
                fuel_slot: slots[1],
                output: slots[2],
                fuel: *fuel,
                progress: *progress,
            };
            quick_furnace(&basis.viewer.inventory, &view, from).map(|(inventory, next)| {
                (
                    inventory,
                    ContainerSlots::Furnace {
                        slots: [next.input, next.fuel_slot, next.output],
                        fuel: next.fuel,
                        progress: next.progress,
                    },
                )
            })
        }
    };
    let (inventory, slots) =
        computed.ok_or(TransferFailure::Refused(RejectReason::InvalidInput))?;
    commit(ctx, &basis.viewer, &basis.stored, inventory, slots)?;
    Ok(true)
}

/// Debits the current unified source and publishes a foot-position drop in
/// the same compound as the durable container touch.
fn settle_drop(
    ctx: &mut TickContext<'_>,
    envelope: CommandEnvelope,
    reference: ContainerRef,
    source: StackSource,
) -> Result<PhaseReport, TransferFailure> {
    let basis = move_basis(ctx, envelope, reference)
        .ok_or(TransferFailure::Refused(RejectReason::InvalidInput))?;
    let slot = usize::from(source.slot());
    let mut inventory = basis.viewer.inventory;
    let mut slots = basis.stored.slots.clone();
    let held = match &mut slots {
        ContainerSlots::Chest(cells) => {
            let held = chest_slot(&inventory.slots, cells, slot)
                .ok_or(TransferFailure::Refused(RejectReason::InvalidSlot))?;
            if !set_chest_slot(&mut inventory.slots, cells, slot, ItemStack::default()) {
                return Err(TransferFailure::Refused(RejectReason::InvalidSlot));
            }
            held
        }
        ContainerSlots::Furnace {
            slots,
            fuel,
            progress,
        } => {
            let mut view = FurnaceView {
                input: slots[0],
                fuel_slot: slots[1],
                output: slots[2],
                fuel: *fuel,
                progress: *progress,
            };
            let held = furnace_slot(&inventory.slots, &view, slot)
                .ok_or(TransferFailure::Refused(RejectReason::InvalidSlot))?;
            if !set_furnace_slot(&mut inventory.slots, &mut view, slot, ItemStack::default()) {
                return Err(TransferFailure::Refused(RejectReason::InvalidSlot));
            }
            *slots = [view.input, view.fuel_slot, view.output];
            *fuel = view.fuel;
            *progress = view.progress;
            held
        }
    };
    if held.count == 0 {
        return Err(TransferFailure::Refused(RejectReason::InvalidSlot));
    }
    if !stacks_valid(&inventory, &slots) {
        return Err(TransferFailure::Refused(RejectReason::InvalidInput));
    }
    let batch =
        drops::prepare_player_drop_checked(ctx, basis.viewer.session, envelope.sequence(), held)
            .map_err(|error| match error {
                drops::PlayerDropFailure::Refused(reason) => TransferFailure::Refused(reason),
                drops::PlayerDropFailure::Trusted(error) => TransferFailure::Trusted(error),
            })?;
    let patch = InventoryPatch::try_new(basis.viewer.actor, basis.viewer.inventory, inventory)
        .map_err(TransferFailure::Trusted)?;
    let next = ContainerRecord {
        reference: basis.stored.reference,
        revision: basis.stored.revision,
        slots,
    };
    ctx.stage(RuleEffect::Compound(vec![
        RuleEffect::Inventory(patch),
        RuleEffect::Container {
            before: basis.stored,
            after: next,
        },
        RuleEffect::Drops(batch),
    ]))
    .map_err(TransferFailure::Trusted)?;
    ctx.record_inventory_publication_dirty(basis.viewer.session);
    Ok(PhaseReport {
        examined: 1,
        applied: 1,
        carried: 0,
        rejected: 0,
    })
}

/// Commits one computed settlement: the final validity sweep and the bench
/// repack preview run over the copies first, and only then do the inventory
/// patch and the container delta stage together. Staged container deltas are
/// what the serial reducer commits to world at overlay commit. An equal slot
/// record still stages a durable source touch for inventory-only transfers.
fn commit(
    ctx: &mut TickContext<'_>,
    viewer: &ViewerBasis,
    stored: &ContainerRecord,
    inventory: InventoryRecord,
    slots: ContainerSlots,
) -> Result<(), TransferFailure> {
    if !stacks_valid(&inventory, &slots) {
        return Err(TransferFailure::Refused(RejectReason::InvalidInput));
    }
    if !can_repack(&inventory.slots, &inventory.crafting) {
        return Err(TransferFailure::Refused(RejectReason::InvalidInput));
    }
    let mut effects = Vec::with_capacity(2);
    if inventory != viewer.inventory {
        let patch = InventoryPatch::try_new(viewer.actor, viewer.inventory, inventory)
            .map_err(TransferFailure::Trusted)?;
        effects.push(RuleEffect::Inventory(patch));
    }
    let next = ContainerRecord {
        reference: stored.reference,
        revision: stored.revision,
        slots,
    };
    effects.push(RuleEffect::Container {
        before: stored.clone(),
        after: next,
    });
    // A rejected durable container write must not spend the source inventory.
    ctx.stage(RuleEffect::Compound(effects))
        .map_err(TransferFailure::Trusted)?;
    ctx.record_inventory_publication_dirty(viewer.session);
    Ok(())
}

/// Final validity sweep over the computed sides, the `Valid` conjunction the
/// Go transfer rows check before committing.
fn stacks_valid(inventory: &InventoryRecord, slots: &ContainerSlots) -> bool {
    if !inventory.slots.iter().all(ItemStack::is_valid) {
        return false;
    }
    match slots {
        ContainerSlots::Chest(cells) => cells.iter().all(ItemStack::is_valid),
        ContainerSlots::Furnace { slots, .. } => slots.iter().all(ItemStack::is_valid),
    }
}

/// Bench repack preview, the exact `canRepackCrafting` row
/// (`packages/server/sim/entity/crafting.go`): every nonempty grid cell must
/// still credit fully into the computed pack through the pickup order, so a
/// container move can never manufacture a grid the pack cannot take back.
fn can_repack(slots: &[ItemStack; INVENTORY_SLOTS], grid: &[ItemStack; 9]) -> bool {
    let mut staged = *slots;
    for stack in grid.iter() {
        if stack.item == ITEM_NONE {
            continue;
        }
        let (next, leftover) = add_stack(&staged, *stack);
        if leftover.count != 0 {
            return false;
        }
        staged = next;
    }
    true
}

/// Credits one source stack into the pack, the exact `Inventory.AddStack`
/// row (`packages/shared/core/inventory.go`): hotbar merge, hotbar empty,
/// backpack merge, backpack empty, each in ascending slot order. An
/// empty-slot landing inherits the source durability, and the unabsorbed
/// remainder keeps the source form.
fn add_stack(
    slots: &[ItemStack; INVENTORY_SLOTS],
    source: ItemStack,
) -> ([ItemStack; INVENTORY_SLOTS], ItemStack) {
    let Some(limit) = mornlea_domain::item_stack_limit(source.item) else {
        return (*slots, source);
    };
    if source.item == ITEM_NONE || source.count == 0 {
        return (*slots, source);
    }
    let mut next = *slots;
    let mut remaining = source.count;
    for (merge, first, last) in [(true, 0, 8), (false, 0, 8), (true, 9, 35), (false, 9, 35)] {
        for index in first..=last {
            if remaining == 0 {
                return (next, ItemStack::default());
            }
            let held = next[index];
            if merge {
                if held.item != source.item || held.count >= limit {
                    continue;
                }
            } else if held.item != ITEM_NONE {
                continue;
            }
            let base = if merge { held.count } else { 0 };
            let moved = (limit - base).min(remaining);
            next[index] = ItemStack {
                item: source.item,
                count: base + moved,
                durability: if merge {
                    held.durability
                } else {
                    source.durability
                },
            };
            remaining -= moved;
        }
    }
    (
        next,
        ItemStack {
            item: source.item,
            count: remaining,
            durability: source.durability,
        },
    )
}

/// Whole-stack move inside the pack half of a container view, the exact
/// `core.Inventory.MoveStack` row the Go container rows reuse for
/// pack-internal ends: an empty target receives the whole stack, a same-item
/// target merges up to the cap with the remainder kept at the source, an
/// unlike target swaps, and an empty source or a full same-item target
/// refuses with zero change.
fn move_whole(slots: &mut [ItemStack; INVENTORY_SLOTS], from: usize, to: usize) -> bool {
    let mut source = slots[from];
    if source.item == ITEM_NONE {
        return false;
    }
    let target = slots[to];
    if target.item == ITEM_NONE {
        slots[to] = source;
        slots[from] = ItemStack::default();
        return true;
    }
    if target.item == source.item {
        let Some(limit) = mornlea_domain::item_stack_limit(source.item) else {
            return false;
        };
        let space = limit.saturating_sub(target.count);
        if space == 0 {
            return false;
        }
        let moved = space.min(source.count);
        let mut merged = target;
        merged.count += moved;
        source.count -= moved;
        slots[to] = merged;
        slots[from] = if source.count == 0 {
            ItemStack::default()
        } else {
            source
        };
        return true;
    }
    slots[to] = source;
    slots[from] = target;
    true
}

/// Partial move inside the pack half of a container view, the exact
/// `core.Inventory.MoveStackAmount` row: an empty target inherits the source
/// item and durability before the capacity-truncated move, a same-item target
/// merges with the remainder kept at the source, and an unlike non-empty
/// target refuses because a partial move never swaps.
fn move_amount(
    slots: &mut [ItemStack; INVENTORY_SLOTS],
    from: usize,
    to: usize,
    amount: u8,
) -> bool {
    if amount == 0 {
        return false;
    }
    let mut source = slots[from];
    if source.item == ITEM_NONE {
        return false;
    }
    let mut target = slots[to];
    if target.item != ITEM_NONE && target.item != source.item {
        return false;
    }
    if target.item == ITEM_NONE {
        target = source;
        target.count = 0;
    }
    let Some(limit) = mornlea_domain::item_stack_limit(source.item) else {
        return false;
    };
    let moved = amount
        .min(source.count)
        .min(limit.saturating_sub(target.count));
    if moved == 0 {
        return false;
    }
    target.count += moved;
    source.count -= moved;
    slots[to] = target;
    slots[from] = if source.count == 0 {
        ItemStack::default()
    } else {
        source
    };
    true
}

/// Derives the partial amount from the settlement-time source stack, the
/// exact `stackSplitAmount` row (`packages/server/sim/entity/container.go`):
/// a single move takes one, a half move takes the ceiling half, and an empty
/// source refuses. The amount is authority derived; no client count exists on
/// the wire.
fn split_amount(source: ItemStack, single: bool) -> Option<u8> {
    if source.item == ITEM_NONE || source.count == 0 {
        return None;
    }
    Some(if single {
        1
    } else {
        // Overflow-free form of `(count + 1) / 2`; valid counts never leave
        // the stack domain so both forms agree.
        source.count / 2 + source.count % 2
    })
}

/// Merges one source stack into one target cell for whole moves, the exact
/// `mergeStacks` row (`packages/server/sim/entity/container.go`): an empty
/// target receives the whole stack, a same-item target merges up to the
/// target item's cap with the remainder kept at the source, an unlike target
/// swaps, and a full same-item target refuses.
fn merge_stacks(source: ItemStack, target: ItemStack) -> Option<(ItemStack, ItemStack)> {
    if target.item == ITEM_NONE {
        return Some((ItemStack::default(), source));
    }
    if target.item == source.item {
        let limit = mornlea_domain::item_stack_limit(target.item)?;
        if target.count >= limit {
            return None;
        }
        let moved = (limit - target.count).min(source.count);
        let mut merged = target;
        merged.count += moved;
        let leftover = if source.count > moved {
            ItemStack {
                item: source.item,
                count: source.count - moved,
                durability: source.durability,
            }
        } else {
            ItemStack::default()
        };
        return Some((leftover, merged));
    }
    Some((target, source))
}

/// Merges at most `amount` items into one target cell for partial moves, the
/// exact `mergeStacksAmount` row: an empty target inherits the source item
/// and durability before the truncated move, a same-item target merges with
/// the remainder kept at the source, and an unlike non-empty target refuses
/// because a partial move never swaps.
fn merge_stacks_amount(
    source: ItemStack,
    target: ItemStack,
    amount: u8,
) -> Option<(ItemStack, ItemStack)> {
    if amount == 0 {
        return None;
    }
    if target.item == ITEM_NONE {
        let moved = amount.min(source.count);
        if moved == 0 {
            return None;
        }
        let mut landed = source;
        landed.count = moved;
        let leftover = if source.count > moved {
            ItemStack {
                item: source.item,
                count: source.count - moved,
                durability: source.durability,
            }
        } else {
            ItemStack::default()
        };
        return Some((leftover, landed));
    }
    if target.item != source.item {
        return None;
    }
    let limit = mornlea_domain::item_stack_limit(target.item)?;
    if target.count >= limit {
        return None;
    }
    let moved = amount.min(source.count).min(limit - target.count);
    let mut merged = target;
    merged.count += moved;
    let leftover = if source.count > moved {
        ItemStack {
            item: source.item,
            count: source.count - moved,
            durability: source.durability,
        }
    } else {
        ItemStack::default()
    };
    Some((leftover, merged))
}

/// Reports whether one target cell can take a quick-move source and, when it
/// can, the landed cell plus the remainder, the exact `quickMoveMergeInto`
/// row (`packages/server/sim/entity/quick_move.go`): an empty cell migrates
/// the whole fitting stack with the source durability, a same-item non-full
/// cell merges, and the remainder drops its durability field exactly like the
/// Go row builds the bare item-count leftover.
fn merge_into(source: ItemStack, target: ItemStack) -> Option<(ItemStack, ItemStack)> {
    let limit = mornlea_domain::item_stack_limit(source.item)?;
    if target.item != ITEM_NONE && (target.item != source.item || target.count >= limit) {
        return None;
    }
    let mut landed = if target.item == ITEM_NONE {
        ItemStack {
            item: source.item,
            count: 0,
            durability: source.durability,
        }
    } else {
        target
    };
    let moved = source.count.min(limit - landed.count);
    landed.count += moved;
    let leftover = if source.count > moved {
        ItemStack {
            item: source.item,
            count: source.count - moved,
            durability: 0,
        }
    } else {
        ItemStack::default()
    };
    Some((landed, leftover))
}

/// Reads one unified chest cell: `0..35` is the pack, `36..62` the chest.
fn chest_slot(
    slots: &[ItemStack; INVENTORY_SLOTS],
    cells: &[ItemStack; CHEST_SLOTS],
    slot: usize,
) -> Option<ItemStack> {
    if slot < INVENTORY_SLOTS {
        return Some(slots[slot]);
    }
    cells.get(slot - CHEST_FIRST).copied()
}

/// Writes one unified chest cell. Chest cells accept any already-valid stack;
/// the chest carries no item constraint beyond validity, mirroring
/// `setChestViewSlot`.
fn set_chest_slot(
    slots: &mut [ItemStack; INVENTORY_SLOTS],
    cells: &mut [ItemStack; CHEST_SLOTS],
    slot: usize,
    stack: ItemStack,
) -> bool {
    if !stack.is_valid() {
        return false;
    }
    if slot < INVENTORY_SLOTS {
        slots[slot] = stack;
        return true;
    }
    let Some(cell) = cells.get_mut(slot - CHEST_FIRST) else {
        return false;
    };
    *cell = stack;
    true
}

/// One whole-stack chest transfer over value copies, the exact
/// `moveChestStack` row. Pack-internal ends reuse the inventory primitive;
/// cross-region ends merge through the shared whole-move merge.
fn move_chest_whole(
    inventory: &InventoryRecord,
    cells: &[ItemStack; CHEST_SLOTS],
    from: usize,
    to: usize,
) -> Option<(InventoryRecord, [ItemStack; CHEST_SLOTS])> {
    if from >= CHEST_VIEW_SLOTS || to >= CHEST_VIEW_SLOTS || from == to {
        return None;
    }
    let mut next_inventory = *inventory;
    if from < INVENTORY_SLOTS && to < INVENTORY_SLOTS {
        if !move_whole(&mut next_inventory.slots, from, to) {
            return None;
        }
        return Some((next_inventory, *cells));
    }
    let source = chest_slot(&next_inventory.slots, cells, from)?;
    if source.item == ITEM_NONE {
        return None;
    }
    let target = chest_slot(&next_inventory.slots, cells, to)?;
    let (next_source, next_target) = merge_stacks(source, target)?;
    let mut next_cells = *cells;
    if !set_chest_slot(
        &mut next_inventory.slots,
        &mut next_cells,
        from,
        next_source,
    ) {
        return None;
    }
    if !set_chest_slot(&mut next_inventory.slots, &mut next_cells, to, next_target) {
        return None;
    }
    Some((next_inventory, next_cells))
}

/// One partial chest transfer over value copies, the exact
/// `moveChestStackAmount` row: same routing as the whole variant with the
/// amount derived from the settlement-time source and the partial merge that
/// never swaps.
fn move_chest_amount(
    inventory: &InventoryRecord,
    cells: &[ItemStack; CHEST_SLOTS],
    from: usize,
    to: usize,
    single: bool,
) -> Option<(InventoryRecord, [ItemStack; CHEST_SLOTS])> {
    if from >= CHEST_VIEW_SLOTS || to >= CHEST_VIEW_SLOTS || from == to {
        return None;
    }
    let mut next_inventory = *inventory;
    if from < INVENTORY_SLOTS && to < INVENTORY_SLOTS {
        let amount = split_amount(next_inventory.slots[from], single)?;
        if !move_amount(&mut next_inventory.slots, from, to, amount) {
            return None;
        }
        return Some((next_inventory, *cells));
    }
    let source = chest_slot(&next_inventory.slots, cells, from)?;
    let amount = split_amount(source, single)?;
    let target = chest_slot(&next_inventory.slots, cells, to)?;
    let (next_source, next_target) = merge_stacks_amount(source, target, amount)?;
    let mut next_cells = *cells;
    if !set_chest_slot(
        &mut next_inventory.slots,
        &mut next_cells,
        from,
        next_source,
    ) {
        return None;
    }
    if !set_chest_slot(&mut next_inventory.slots, &mut next_cells, to, next_target) {
        return None;
    }
    Some((next_inventory, next_cells))
}

/// One chest quick move over value copies, the exact `quickMoveChestStack`
/// row: a chest source credits through the pickup order with the remainder
/// kept at the source cell, and a pack source lands in the first fitting
/// chest cell in ascending order with the remainder kept at the pack slot.
/// Zero absorption refuses the whole request.
fn quick_chest(
    inventory: &InventoryRecord,
    cells: &[ItemStack; CHEST_SLOTS],
    from: usize,
) -> Option<(InventoryRecord, [ItemStack; CHEST_SLOTS])> {
    if from >= CHEST_VIEW_SLOTS {
        return None;
    }
    if from >= CHEST_FIRST {
        let source = chest_slot(&inventory.slots, cells, from)?;
        if source.item == ITEM_NONE {
            return None;
        }
        let (slots, leftover) = add_stack(&inventory.slots, source);
        if leftover.count == source.count {
            return None;
        }
        let mut next_inventory = *inventory;
        next_inventory.slots = slots;
        let mut next_cells = *cells;
        if !set_chest_slot(&mut next_inventory.slots, &mut next_cells, from, leftover) {
            return None;
        }
        return Some((next_inventory, next_cells));
    }
    let source = inventory.slots[from];
    if source.item == ITEM_NONE {
        return None;
    }
    for index in 0..CHEST_SLOTS {
        let (landed, leftover) = match merge_into(source, cells[index]) {
            Some(fit) => fit,
            None => continue,
        };
        let mut next_inventory = *inventory;
        if !leftover.is_valid() {
            return None;
        }
        next_inventory.slots[from] = leftover;
        let mut next_cells = *cells;
        if !landed.is_valid() {
            return None;
        }
        next_cells[index] = landed;
        return Some((next_inventory, next_cells));
    }
    None
}

/// The furnace working set: the three material cells plus the burn and smelt
/// counters the material writes preserve.
#[derive(Clone, Copy, Debug)]
struct FurnaceView {
    input: ItemStack,
    fuel_slot: ItemStack,
    output: ItemStack,
    fuel: u32,
    progress: u32,
}

/// Reads one unified furnace cell: `0..35` is the pack, then input, fuel and
/// output.
fn furnace_slot(
    slots: &[ItemStack; INVENTORY_SLOTS],
    view: &FurnaceView,
    slot: usize,
) -> Option<ItemStack> {
    match slot {
        FURNACE_INPUT => Some(view.input),
        FURNACE_FUEL => Some(view.fuel_slot),
        FURNACE_OUTPUT => Some(view.output),
        _ => slots.get(slot).copied(),
    }
}

/// Writes one unified furnace cell with the material constraints, the exact
/// `setFurnaceViewSlot` row (`packages/server/sim/entity/furnace.go`): the
/// input takes only registered smelting inputs and resets the smelt progress
/// whenever the written item differs from the held one, including a full
/// removal; the fuel takes only coal; the output takes only the registered
/// smelting products read from the same single table that defines them, so
/// the whitelist cannot drift from the smelting rows.
fn set_furnace_slot(
    slots: &mut [ItemStack; INVENTORY_SLOTS],
    view: &mut FurnaceView,
    slot: usize,
    stack: ItemStack,
) -> bool {
    if !stack.is_valid() {
        return false;
    }
    match slot {
        FURNACE_INPUT => {
            if stack.item != ITEM_NONE && mornlea_domain::smelting_output(stack.item).is_none() {
                return false;
            }
            if view.input.item != stack.item {
                view.progress = 0;
            }
            view.input = stack;
            true
        }
        FURNACE_FUEL => {
            if stack.item != ITEM_NONE
                && (stack.item != ITEM_COAL || stack.count == 0 || stack.count > MAX_STACK_COUNT)
            {
                return false;
            }
            view.fuel_slot = stack;
            true
        }
        FURNACE_OUTPUT => {
            if stack.item != ITEM_NONE && !mornlea_domain::is_smelting_product(stack.item) {
                return false;
            }
            view.output = stack;
            true
        }
        _ => {
            if slot >= INVENTORY_SLOTS {
                return false;
            }
            slots[slot] = stack;
            true
        }
    }
}

/// One whole-stack furnace transfer over value copies, the exact
/// `moveFurnaceStack` row: same routing as the chest variant, but the output
/// refuses as a destination and every cross-region write passes the material
/// constraints.
fn move_furnace_whole(
    inventory: &InventoryRecord,
    view: &FurnaceView,
    from: usize,
    to: usize,
) -> Option<(InventoryRecord, FurnaceView)> {
    if from >= FURNACE_VIEW_SLOTS || to >= FURNACE_VIEW_SLOTS || from == to {
        return None;
    }
    if to == FURNACE_OUTPUT {
        return None;
    }
    let mut next_inventory = *inventory;
    if from < INVENTORY_SLOTS && to < INVENTORY_SLOTS {
        if !move_whole(&mut next_inventory.slots, from, to) {
            return None;
        }
        return Some((next_inventory, *view));
    }
    let source = furnace_slot(&next_inventory.slots, view, from)?;
    if source.item == ITEM_NONE {
        return None;
    }
    let target = furnace_slot(&next_inventory.slots, view, to)?;
    let (next_source, next_target) = merge_stacks(source, target)?;
    let mut next_view = *view;
    if !set_furnace_slot(&mut next_inventory.slots, &mut next_view, from, next_source) {
        return None;
    }
    if !set_furnace_slot(&mut next_inventory.slots, &mut next_view, to, next_target) {
        return None;
    }
    Some((next_inventory, next_view))
}

/// One partial furnace transfer over value copies, the exact
/// `moveFurnaceStackAmount` row: same shape as the whole variant with the
/// derived amount and the never-swapping partial merge. The output refuses
/// as a destination here as well, because the protocol admits the index and
/// leaves the judgment to the authority.
fn move_furnace_amount(
    inventory: &InventoryRecord,
    view: &FurnaceView,
    from: usize,
    to: usize,
    single: bool,
) -> Option<(InventoryRecord, FurnaceView)> {
    if from >= FURNACE_VIEW_SLOTS || to >= FURNACE_VIEW_SLOTS || from == to {
        return None;
    }
    if to == FURNACE_OUTPUT {
        return None;
    }
    let mut next_inventory = *inventory;
    if from < INVENTORY_SLOTS && to < INVENTORY_SLOTS {
        let amount = split_amount(next_inventory.slots[from], single)?;
        if !move_amount(&mut next_inventory.slots, from, to, amount) {
            return None;
        }
        return Some((next_inventory, *view));
    }
    let source = furnace_slot(&next_inventory.slots, view, from)?;
    let amount = split_amount(source, single)?;
    let target = furnace_slot(&next_inventory.slots, view, to)?;
    let (next_source, next_target) = merge_stacks_amount(source, target, amount)?;
    let mut next_view = *view;
    if !set_furnace_slot(&mut next_inventory.slots, &mut next_view, from, next_source) {
        return None;
    }
    if !set_furnace_slot(&mut next_inventory.slots, &mut next_view, to, next_target) {
        return None;
    }
    Some((next_inventory, next_view))
}

/// One furnace quick move over value copies, the exact
/// `quickMoveFurnaceStack` row: a material source (output only ever as a
/// source) credits through the pickup order with the remainder written back
/// through the material constraints, and a pack source lands smeltable input
/// before coal fuel with the earlier candidate tried first and the later
/// candidate taken when the earlier has no fitting capacity. Zero absorption
/// refuses the whole request.
fn quick_furnace(
    inventory: &InventoryRecord,
    view: &FurnaceView,
    from: usize,
) -> Option<(InventoryRecord, FurnaceView)> {
    if from >= FURNACE_VIEW_SLOTS {
        return None;
    }
    if from >= INVENTORY_SLOTS {
        let source = furnace_slot(&inventory.slots, view, from)?;
        if source.item == ITEM_NONE {
            return None;
        }
        let (slots, leftover) = add_stack(&inventory.slots, source);
        if leftover.count == source.count {
            return None;
        }
        let mut next_inventory = *inventory;
        next_inventory.slots = slots;
        let mut next_view = *view;
        if !set_furnace_slot(&mut next_inventory.slots, &mut next_view, from, leftover) {
            return None;
        }
        return Some((next_inventory, next_view));
    }
    let source = inventory.slots[from];
    if source.item == ITEM_NONE {
        return None;
    }
    let smeltable = mornlea_domain::smelting_output(source.item).is_some();
    let fuel = source.item == ITEM_COAL;
    if !smeltable && !fuel {
        return None;
    }
    for step in 0..2 {
        let slot = if step == 0 {
            if !smeltable {
                continue;
            }
            FURNACE_INPUT
        } else {
            if !fuel {
                break;
            }
            FURNACE_FUEL
        };
        let target = furnace_slot(&inventory.slots, view, slot)?;
        let (landed, leftover) = match merge_into(source, target) {
            Some(fit) => fit,
            None => continue,
        };
        let mut next_inventory = *inventory;
        if !leftover.is_valid() {
            return None;
        }
        next_inventory.slots[from] = leftover;
        let mut next_view = *view;
        if !set_furnace_slot(&mut next_inventory.slots, &mut next_view, slot, landed) {
            return None;
        }
        return Some((next_inventory, next_view));
    }
    None
}

/// The observation basis the open needs: an active actor record with its
/// current pose and dimension, plus the environment tunables. A missing actor
/// or environment refuses exactly like the accepted resolvers treat an
/// unavailable authority basis.
struct ActorBasis {
    dimension: Dimension,
    eye: [f32; 3],
    reach: f32,
}

fn actor_basis(view: &AuthorityReadView<'_>, record: &ActorRecord) -> Option<ActorBasis> {
    let environment = view.environment()?;
    let position = record.motion.position().get();
    Some(ActorBasis {
        dimension: record.dimension,
        eye: [
            position[0],
            position[1] + environment.tunables.eye_height(),
            position[2],
        ],
        reach: environment.tunables.interaction_reach(),
    })
}

/// One classified ray hit: the full observation of the first interaction target.
struct RayHit {
    observed: BlockObservation,
}

enum RayFailure {
    Unavailable,
    Invalid,
}

/// Walks the numerical ray kernel batch by batch and classifies every
/// traversed cell against the authority view, the same walk the accepted
/// resolvers perform: an unobserved cell refuses because the authority cannot
/// certify geometry it has not observed. Fluids and open doors pass through
/// under the shared interaction classifier. No hit within reach reports empty.
fn cast_ray(
    view: &AuthorityReadView<'_>,
    dimension: Dimension,
    origin: [f32; 3],
    direction: [f32; 3],
    reach: f32,
) -> Result<Option<RayHit>, RayFailure> {
    let normalized = normalized_direction(direction).ok_or(RayFailure::Invalid)?;
    let mut cursor = RayCursor::try_new(Ray {
        origin,
        direction: normalized,
        maximum: reach,
    })
    .map_err(|_| RayFailure::Invalid)?;
    loop {
        let batch = NativeRaycast
            .next_batch(&mut cursor)
            .map_err(|_| RayFailure::Invalid)?;
        for record in batch.records() {
            let cell = BlockPos::new(record.cell[0], record.cell[1], record.cell[2]);
            match view.observation(dimension, cell) {
                None => return Err(RayFailure::Unavailable),
                Some(observed) if !target_block(view, dimension, cell, observed.block) => {}
                Some(observed) => return Ok(Some(RayHit { observed })),
            }
        }
        if batch.is_done() {
            return Ok(None);
        }
    }
}

#[cfg(test)]
mod lifecycle_tests {
    use super::*;
    use crate::contracts::{ServerLimits, TickBudget};
    use crate::state::AuthorityState;
    use mornlea_domain::HotbarSlot;

    #[test]
    fn lifecycle_stale_compound_preserves_trusted_failure_identity() {
        let mut authority = AuthorityState::try_new(
            ServerLimits::try_new(8, 4096, 512, 64, 64, 1_048_576).unwrap(),
            7,
        )
        .unwrap();
        let mut context = TickContext::harness(&mut authority, TickBudget::full());
        let session = SessionKey::from_raw(1).unwrap();
        let actor = ActorKey::Player(session);
        let before = InventoryRecord::empty();
        let current = before.with_selected(HotbarSlot::new(2).unwrap());
        let after = before.with_selected(HotbarSlot::new(1).unwrap());
        context.preload_inventory(actor, current);
        let failure = stage_lifecycle_effects(
            &mut context,
            vec![
                RuleEffect::Inventory(InventoryPatch::try_new(actor, before, after).unwrap()),
                RuleEffect::Viewer {
                    session,
                    view: None,
                },
            ],
        )
        .unwrap_err();
        assert_eq!(failure.into_raw(), RuleReject::StaleObservation);
        assert_eq!(
            failure.into_live(),
            Err(ServerError::Internal {
                invariant: "container lifecycle staging"
            })
        );
        assert_eq!(context.read().inventory(actor), Some(&current));
        assert!(context.read().viewer(session).is_none());
        assert!(context.events().is_empty());
    }

    #[test]
    fn transfer_adapter_ownership_and_missing_actor() {
        use mornlea_domain::CommandEnvelopeParts;
        let mut authority = AuthorityState::try_new(
            ServerLimits::try_new(8, 4096, 512, 64, 64, 1_048_576).unwrap(),
            7,
        )
        .unwrap();
        let mut context = TickContext::harness(&mut authority, TickBudget::full());
        let make = |command| {
            CommandEnvelope::try_new(CommandEnvelopeParts {
                tick: 0,
                session: 1,
                sequence: 1,
                arrival_index: 1,
                command,
            })
            .unwrap()
        };
        let movement = ContainerMove::try_new(
            mornlea_domain::ChunkPos::new(0, -1),
            ContainerKind::Chest,
            0,
            1,
            36,
            0,
        )
        .unwrap();
        assert_eq!(
            settle_transfer(&mut context, &make(Command::MoveContainer(movement))),
            Ok(CommandDisposition::Refused(RejectReason::PlayerNotReady))
        );
        for command in [
            Command::CloseContainer,
            Command::MoveInventory(mornlea_domain::InventoryMove::try_new(0, 1).unwrap()),
        ] {
            assert_eq!(
                settle_transfer(&mut context, &make(command)),
                Ok(CommandDisposition::Unowned)
            );
        }
        assert!(context.events().is_empty());
    }

    #[test]
    fn drop_adapter_ownership_and_preparation_readiness() {
        use mornlea_domain::CommandEnvelopeParts;
        let mut authority = AuthorityState::try_new(
            ServerLimits::try_new(8, 4096, 512, 64, 64, 1_048_576).unwrap(),
            7,
        )
        .unwrap();
        let mut context = TickContext::harness(&mut authority, TickBudget::full());
        let session = SessionKey::from_raw(1).unwrap();
        let reference = ContainerRef::try_new(
            mornlea_domain::ChunkPos::new(0, -1),
            ContainerKind::Chest,
            0,
            1,
        )
        .unwrap();
        let make = |command| {
            CommandEnvelope::try_new(CommandEnvelopeParts {
                tick: 0,
                session: 1,
                sequence: 1,
                arrival_index: 1,
                command,
            })
            .unwrap()
        };
        let source = StackSource::try_new(StackView::Container(reference), 36).unwrap();
        assert_eq!(
            settle_drop_command(&mut context, &make(Command::DropStack(source))),
            Ok(CommandDisposition::Refused(RejectReason::PlayerNotReady))
        );
        for command in [Command::CloseContainer, Command::QuickMove(source)] {
            assert_eq!(
                settle_drop_command(&mut context, &make(command)),
                Ok(CommandDisposition::Unowned)
            );
        }
        assert!(matches!(
            drops::prepare_player_drop_checked(
                &context,
                session,
                1,
                ItemStack {
                    item: 2,
                    count: 5,
                    durability: 0
                }
            ),
            Err(drops::PlayerDropFailure::Refused(
                RejectReason::PlayerNotReady
            ))
        ));
        assert!(context.events().is_empty());
    }
}
