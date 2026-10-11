//! Serial composition reuses the accepted family providers and actual owner facts.

use super::LiveSession;
use crate::contracts::*;
use crate::input::ClientIntentKind;
use crate::presentation::family_actors::assemble_actors;
use crate::presentation::family_audio::project_audio;
use crate::presentation::family_diagnostics::project_diagnostics;
use crate::presentation::family_inventory_ui::assemble_inventory_ui;
use crate::presentation::family_player_view::project_player_view;
use crate::presentation::family_world_ui::assemble_world_ui;
use crate::presentation::frame::{InputReceiptState as Receipt, TerrainRecord};
use crate::presentation::*;
use mornlea_domain::PlayerId;

fn digest<const N: usize>(value: &str) -> Result<[u8; N], ClientError> {
    let mut output = [0; N];
    for (index, byte) in output.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&value[index * 2..index * 2 + 2], 16)
            .map_err(|_| ClientError::Internal)?;
    }
    Ok(output)
}

pub(super) fn observe_queues(session: &mut LiveSession) {
    let input = session.admission.projection();
    let control_bytes: usize = session.outbound.iter().map(Vec::len).sum();
    let held = &mut session.queue_high_water;
    held.input = held
        .input
        .max((input.admitted().len() + input.rejected().len()) as u64);
    held.inbound_records = held
        .inbound_records
        .max(session.mirror.observations().len() as u64);
    held.inbound_bytes = held.inbound_bytes.max(
        session
            .mirror
            .held_observation_bytes()
            .expect("admitted observation storage") as u64,
    );
    held.outbound_records = held
        .outbound_records
        .max((session.admission.outbound_records() + session.outbound.len()) as u64);
    held.outbound_bytes = held
        .outbound_bytes
        .max((session.admission.outbound_bytes() + control_bytes) as u64);
    held.journal = held.journal.max(session.admission.journal_entries() as u64);
    held.preparation = held
        .preparation
        .max(session.preparation.pending_jobs() as u64);
}

pub(super) fn diagnostics(session: &LiveSession) -> Result<DiagnosticProjectionState, ClientError> {
    DiagnosticProjectionState::try_new(
        ProducerIdentity::try_new(
            digest(env!("MORNLEA_CORE_SOURCE_SHA"))?,
            digest(env!("MORNLEA_CORE_CONTRACT_SHA"))?,
        )?,
        session.queue_high_water,
        ErrorClassCounters {
            invalid: session.rejected_inbound,
            ..Default::default()
        },
    )
}

pub(super) fn diagnostic_record(
    session: &LiveSession,
    index: u64,
) -> Result<DiagnosticRecord, ClientError> {
    let state = diagnostics(session)?;
    DiagnosticRecord::try_new(
        RecordHeader::try_new(
            session.epoch,
            session.mirror.mirror().revision(),
            None,
            FamilyOperation::Upsert,
        )?,
        *state.producer(),
        index,
        *state.queues(),
        *state.rejected(),
    )
}
use crate::presentation::actors::{
    companion::project_companion, drop::project_drop, hostile::project_hostile,
    passive::project_passive, projectile::project_projectile, remote_player::project_remote_player,
};
use crate::presentation::inventory_ui::{
    container::project_container, crafting::project_crafting, furnace::project_furnace,
    inventory::project_inventory,
};
use crate::presentation::world_ui::{
    chat::project_chat, environment::project_environment, prompt::project_prompt,
    survival::project_survival, task::project_task,
};

pub(super) struct BuiltFrame {
    pub(super) frame: PresentationFrame,
    pub(super) audio: AudioDedupDelta,
}

pub(super) fn complete(
    view: &ProjectionView<'_>,
    phase: SessionPhase,
    player_id: Option<PlayerId>,
    terminal: Option<CloseReason>,
    terrain: Vec<TerrainRecord>,
    lifecycle: Vec<LifecycleRecord>,
    emit_player: bool,
    confirmations: &[(u64, ClientIntentKind, Option<u64>)],
) -> Result<BuiltFrame, ClientError> {
    let revision = view.frame_revision();
    let session = vec![SessionRecord::try_new(
        RecordHeader::try_new(view.frame_epoch(), revision, None, FamilyOperation::Upsert)?,
        phase,
        player_id,
        terminal,
    )?];
    let upsert = FamilyOperation::Upsert;
    let mut families: Vec<FamilyFrame> = Vec::new();

    families.push(FamilyFrame::try_new(
        FamilyKey::try_new(FAMILY_SESSION)?,
        FamilyRecords::Session(session),
    )?);

    // The input family: the pending input metadata in the frozen
    // admitted-first-then-rejected order, the records the publication
    // consumes with its input cursor.
    let mut input_records = Vec::new();
    for (sequence, kind) in view.input().admitted() {
        input_records.push(InputRecord::try_new(
            RecordHeader::try_new(view.frame_epoch(), revision, None, upsert)?,
            Some(*sequence),
            *kind,
            Receipt::Queued,
        )?);
    }
    for (sequence, class) in view.input().rejected() {
        input_records.push(InputRecord::try_new(
            RecordHeader::try_new(view.frame_epoch(), revision, None, upsert)?,
            Some(*sequence),
            // A rejection metadata entry carries no action kind; the neutral
            // sequenced tag names the journal entry it refuses.
            ClientIntentKind::PlayerInput,
            Receipt::Rejected { class: *class },
        )?);
    }
    // Correlated authority acknowledgments retain their actual tick, when supplied.
    for (sequence, kind, tick) in confirmations {
        input_records.push(InputRecord::try_new(
            RecordHeader::try_new(view.frame_epoch(), revision, *tick, upsert)?,
            Some(*sequence),
            *kind,
            Receipt::Confirmed { server_tick: *tick },
        )?);
    }
    families.push(FamilyFrame::try_new(
        FamilyKey::try_new(FAMILY_INPUT)?,
        FamilyRecords::Input(input_records),
    )?);

    families.push(FamilyFrame::try_new(
        FamilyKey::try_new(FAMILY_TERRAIN)?,
        FamilyRecords::Terrain(terrain),
    )?);

    let actor_parts = [
        project_remote_player(view)?,
        project_hostile(view)?,
        project_passive(view)?,
        project_projectile(view)?,
        project_companion(view)?,
        project_drop(view)?,
    ];
    let actor_records = assemble_actors(
        view.frame_epoch(),
        revision,
        actor_parts,
        view.limits(),
        view.observations(),
    )?;
    families.push(FamilyFrame::try_new(
        FamilyKey::try_new(FAMILY_ACTORS)?,
        FamilyRecords::Actors(actor_records),
    )?);

    let player_records = if emit_player {
        project_player_view(view)?
    } else {
        Vec::new()
    };
    families.push(FamilyFrame::try_new(
        FamilyKey::try_new(FAMILY_PLAYER_VIEW)?,
        FamilyRecords::PlayerView(player_records),
    )?);

    let inventory_records = assemble_inventory_ui(
        view.frame_epoch(),
        revision,
        [
            project_inventory(view)?,
            project_container(view)?,
            project_crafting(view)?,
            project_furnace(view)?,
        ],
        view.limits(),
    )?;
    families.push(FamilyFrame::try_new(
        FamilyKey::try_new(FAMILY_INVENTORY_UI)?,
        FamilyRecords::InventoryUi(inventory_records),
    )?);

    let world_records = assemble_world_ui(
        view.frame_epoch(),
        revision,
        [
            project_environment(view)?,
            project_survival(view)?,
            project_chat(view)?,
            project_task(view)?,
            project_prompt(view)?,
        ],
        view.limits(),
    )?;
    families.push(FamilyFrame::try_new(
        FamilyKey::try_new(FAMILY_WORLD_UI)?,
        FamilyRecords::WorldUi(world_records),
    )?);

    let audio = project_audio(view)?;
    families.push(FamilyFrame::try_new(
        FamilyKey::try_new(FAMILY_AUDIO_CUES)?,
        FamilyRecords::AudioCues(audio.records().to_vec()),
    )?);

    families.push(FamilyFrame::try_new(
        FamilyKey::try_new(FAMILY_LIFECYCLE)?,
        FamilyRecords::Lifecycle(lifecycle),
    )?);

    let diagnostic_records = project_diagnostics(view)?;
    families.push(FamilyFrame::try_new(
        FamilyKey::try_new(FAMILY_DIAGNOSTICS)?,
        FamilyRecords::Diagnostics(diagnostic_records),
    )?);

    let frame =
        PresentationFrame::try_new(view.frame_epoch(), revision, view.frame_index(), families)?;
    Ok(BuiltFrame {
        frame,
        audio: audio.proposed_dedup().clone(),
    })
}
