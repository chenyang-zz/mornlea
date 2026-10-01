//! Semantic input admission: the twenty closed actions, whole-batch token
//! rules, the F1 mapping table and the single-owner atomic commit.
//!
//! The C1 action set is the closed F1 domain command set plus separate chat.
//! A whole batch validates before any queue, journal, sequence or overlay
//! owner changes: `InputTranslator::validate_batch` is read-only over the
//! confirmed mirror, `commit` simulates the local view-validity overlay and
//! all capacities, encodes every action into reserved temporary buffers, and
//! only then advances sequence, journal, queue, overlay and local cue-source
//! metadata in one critical section. A rejected batch leaves every owner
//! unchanged. Chat is never sequenced and a local receipt is admission, not
//! server confirmation.

use mornlea_domain::{
    ChatIntent, Command, ContainerMove, ContainerRef, CraftingMove, CraftingSize, DomainError,
    HotbarSlot, InventoryMove, LookAngles, PartialMove, PlacementIntent, PlayerControl,
    ResyncIntent, StackSource, StackView,
};
use mornlea_protocol::{ClientPacket, PlayIntent, ProtocolError};

use crate::contracts::{ClientError, ClientLimits, ConfirmedRevision, InputReceipt, SessionEpoch};
use crate::session::ConfirmedMirror;

/// One semantic client action with its optional view tokens.
///
/// The tokens ride outside the unchanged F1 payload. Inventory-only
/// operations require both token fields to be `None`; a supplied irrelevant
/// token or a missing required token rejects the entire batch.
#[derive(Clone, Debug, PartialEq)]
pub struct InputAction {
    pub intent: ClientIntent,
    pub container: Option<ContainerToken>,
    pub crafting: Option<CraftingViewToken>,
}

/// The bounded semantic input batch: 0..=128 checked actions for one epoch.
#[derive(Clone, Debug, PartialEq)]
pub struct InputBatch {
    epoch: SessionEpoch,
    actions: Vec<InputAction>,
}

impl InputBatch {
    /// Wraps a batch, enforcing the 128-action ceiling before any per-action
    /// scan. Every action's tokens are already checked values by type.
    pub fn try_new(epoch: SessionEpoch, actions: Vec<InputAction>) -> Result<Self, ClientError> {
        if actions.len() > crate::contracts::ClientLimits::MAX_QUEUED_INPUT_EVENTS {
            return Err(ClientError::Capacity);
        }
        Ok(Self { epoch, actions })
    }

    pub fn epoch(&self) -> SessionEpoch {
        self.epoch
    }

    pub fn actions(&self) -> &[InputAction] {
        &self.actions
    }

    pub fn into_actions(self) -> Vec<InputAction> {
        self.actions
    }
}

/// The local attribution of one confirmed external container view.
///
/// The confirmed revision is mirror attribution, not a wire field; the
/// reference itself is the accepted overworld-only F1 value.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ContainerToken {
    epoch: SessionEpoch,
    reference: ContainerRef,
    confirmed_revision: ConfirmedRevision,
}

impl ContainerToken {
    pub fn try_new(
        epoch: SessionEpoch,
        reference: ContainerRef,
        confirmed_revision: ConfirmedRevision,
    ) -> Result<Self, ClientError> {
        Ok(Self {
            epoch,
            reference,
            confirmed_revision,
        })
    }

    pub fn epoch(&self) -> SessionEpoch {
        self.epoch
    }

    pub fn reference(&self) -> ContainerRef {
        self.reference
    }

    pub fn confirmed_revision(&self) -> ConfirmedRevision {
        self.confirmed_revision
    }
}

/// The local attribution of one confirmed crafting view, personal or
/// workbench. It deliberately carries no container reference or generation:
/// crafting never fabricates a workbench `ContainerRef`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CraftingViewToken {
    epoch: SessionEpoch,
    confirmed_revision: ConfirmedRevision,
    size: CraftingSize,
}

impl CraftingViewToken {
    pub fn try_new(
        epoch: SessionEpoch,
        confirmed_revision: ConfirmedRevision,
        size: CraftingSize,
    ) -> Result<Self, ClientError> {
        Ok(Self {
            epoch,
            confirmed_revision,
            size,
        })
    }

    pub fn epoch(&self) -> SessionEpoch {
        self.epoch
    }

    pub fn confirmed_revision(&self) -> ConfirmedRevision {
        self.confirmed_revision
    }

    pub fn size(&self) -> CraftingSize {
        self.size
    }
}

/// The closed C1 semantic action set in exact contract order: the nineteen
/// F1 domain commands plus separate chat. `KeepAliveReply` is a session
/// control reply and is never a batch action.
#[derive(Clone, Debug, PartialEq)]
pub enum ClientIntent {
    PlayerInput(PlayerControl),
    PlaceBlock(PlacementIntent),
    Resync(ResyncIntent),
    SelectHotbar(HotbarSlot),
    OpenContainer(LookAngles),
    TillSoil(LookAngles),
    BoneMeal(LookAngles),
    CollectWater(LookAngles),
    PlaceWater(LookAngles),
    MoveInventory(InventoryMove),
    MoveCrafting(CraftingMove),
    MoveContainer(ContainerMove),
    CloseContainer,
    DropSelectedItem,
    TakeCraftingOutput,
    EquipArmor,
    MovePartial(PartialMove),
    QuickMove(StackSource),
    DropStack(StackSource),
    Chat(ChatIntent),
}

/// One unit tag per accepted semantic action, in the exact `ClientIntent`
/// order. The conversion is exhaustive and the contract test rejects
/// registry drift between the two sets.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ClientIntentKind {
    PlayerInput,
    PlaceBlock,
    Resync,
    SelectHotbar,
    OpenContainer,
    TillSoil,
    BoneMeal,
    CollectWater,
    PlaceWater,
    MoveInventory,
    MoveCrafting,
    MoveContainer,
    CloseContainer,
    DropSelectedItem,
    TakeCraftingOutput,
    EquipArmor,
    MovePartial,
    QuickMove,
    DropStack,
    Chat,
}

impl ClientIntent {
    /// The exhaustive kind tag of this action.
    pub fn kind(&self) -> ClientIntentKind {
        match self {
            ClientIntent::PlayerInput(_) => ClientIntentKind::PlayerInput,
            ClientIntent::PlaceBlock(_) => ClientIntentKind::PlaceBlock,
            ClientIntent::Resync(_) => ClientIntentKind::Resync,
            ClientIntent::SelectHotbar(_) => ClientIntentKind::SelectHotbar,
            ClientIntent::OpenContainer(_) => ClientIntentKind::OpenContainer,
            ClientIntent::TillSoil(_) => ClientIntentKind::TillSoil,
            ClientIntent::BoneMeal(_) => ClientIntentKind::BoneMeal,
            ClientIntent::CollectWater(_) => ClientIntentKind::CollectWater,
            ClientIntent::PlaceWater(_) => ClientIntentKind::PlaceWater,
            ClientIntent::MoveInventory(_) => ClientIntentKind::MoveInventory,
            ClientIntent::MoveCrafting(_) => ClientIntentKind::MoveCrafting,
            ClientIntent::MoveContainer(_) => ClientIntentKind::MoveContainer,
            ClientIntent::CloseContainer => ClientIntentKind::CloseContainer,
            ClientIntent::DropSelectedItem => ClientIntentKind::DropSelectedItem,
            ClientIntent::TakeCraftingOutput => ClientIntentKind::TakeCraftingOutput,
            ClientIntent::EquipArmor => ClientIntentKind::EquipArmor,
            ClientIntent::MovePartial(_) => ClientIntentKind::MovePartial,
            ClientIntent::QuickMove(_) => ClientIntentKind::QuickMove,
            ClientIntent::DropStack(_) => ClientIntentKind::DropStack,
            ClientIntent::Chat(_) => ClientIntentKind::Chat,
        }
    }

    /// The F1 mapping table, outbound half: the intent becomes the original
    /// F1 play intent, sequenced for the nineteen commands and unsequenced
    /// for chat. The caller owns the sequence; chat never takes one.
    pub fn to_play_intent(&self, sequence: u64) -> PlayIntent {
        match self {
            ClientIntent::PlayerInput(control) => {
                sequenced(sequence, Command::PlayerInput(*control))
            }
            ClientIntent::PlaceBlock(intent) => sequenced(sequence, Command::PlaceBlock(*intent)),
            ClientIntent::Resync(intent) => sequenced(sequence, Command::Resync(*intent)),
            ClientIntent::SelectHotbar(slot) => sequenced(sequence, Command::SelectHotbar(*slot)),
            ClientIntent::OpenContainer(look) => sequenced(sequence, Command::OpenContainer(*look)),
            ClientIntent::TillSoil(look) => sequenced(sequence, Command::TillSoil(*look)),
            ClientIntent::BoneMeal(look) => sequenced(sequence, Command::BoneMeal(*look)),
            ClientIntent::CollectWater(look) => sequenced(sequence, Command::CollectWater(*look)),
            ClientIntent::PlaceWater(look) => sequenced(sequence, Command::PlaceWater(*look)),
            ClientIntent::MoveInventory(move_) => {
                sequenced(sequence, Command::MoveInventory(*move_))
            }
            ClientIntent::MoveCrafting(move_) => sequenced(sequence, Command::MoveCrafting(*move_)),
            ClientIntent::MoveContainer(move_) => {
                sequenced(sequence, Command::MoveContainer(*move_))
            }
            ClientIntent::CloseContainer => sequenced(sequence, Command::CloseContainer),
            ClientIntent::DropSelectedItem => sequenced(sequence, Command::DropSelectedItem),
            ClientIntent::TakeCraftingOutput => sequenced(sequence, Command::TakeCraftingOutput),
            ClientIntent::EquipArmor => sequenced(sequence, Command::EquipArmor),
            ClientIntent::MovePartial(partial) => {
                sequenced(sequence, Command::MovePartial(*partial))
            }
            ClientIntent::QuickMove(source) => sequenced(sequence, Command::QuickMove(*source)),
            ClientIntent::DropStack(source) => sequenced(sequence, Command::DropStack(*source)),
            ClientIntent::Chat(chat) => PlayIntent::Chat(chat.clone()),
        }
    }

    /// The F1 mapping table, inbound half: one decoded play intent becomes
    /// the semantic action it names, or `None` for the keep-alive reply that
    /// is session control rather than play.
    pub fn from_play_intent(intent: &PlayIntent) -> Option<ClientIntent> {
        match intent {
            PlayIntent::Sequenced { command, .. } => Some(match command {
                Command::PlayerInput(control) => ClientIntent::PlayerInput(*control),
                Command::PlaceBlock(intent) => ClientIntent::PlaceBlock(*intent),
                Command::Resync(intent) => ClientIntent::Resync(*intent),
                Command::SelectHotbar(slot) => ClientIntent::SelectHotbar(*slot),
                Command::OpenContainer(look) => ClientIntent::OpenContainer(*look),
                Command::TillSoil(look) => ClientIntent::TillSoil(*look),
                Command::BoneMeal(look) => ClientIntent::BoneMeal(*look),
                Command::CollectWater(look) => ClientIntent::CollectWater(*look),
                Command::PlaceWater(look) => ClientIntent::PlaceWater(*look),
                Command::MoveInventory(move_) => ClientIntent::MoveInventory(*move_),
                Command::MoveCrafting(move_) => ClientIntent::MoveCrafting(*move_),
                Command::MoveContainer(move_) => ClientIntent::MoveContainer(*move_),
                Command::CloseContainer => ClientIntent::CloseContainer,
                Command::DropSelectedItem => ClientIntent::DropSelectedItem,
                Command::TakeCraftingOutput => ClientIntent::TakeCraftingOutput,
                Command::EquipArmor => ClientIntent::EquipArmor,
                Command::MovePartial(partial) => ClientIntent::MovePartial(*partial),
                Command::QuickMove(source) => ClientIntent::QuickMove(*source),
                Command::DropStack(source) => ClientIntent::DropStack(*source),
            }),
            PlayIntent::Chat(chat) => Some(ClientIntent::Chat(chat.clone())),
            PlayIntent::KeepAliveReply { .. } => None,
        }
    }

    /// Encodes this action as one complete prefix-inclusive F1 frame.
    ///
    /// The concrete packet constructor is the family's own gate, so a value
    /// the wire refuses is refused here rather than published. The returned
    /// frame length is the outbound byte charge.
    pub fn encode_frame(&self, sequence: u64, scratch: &mut Vec<u8>) -> Result<usize, ClientError> {
        let packet = ClientPacket::try_from(self.to_play_intent(sequence)).map_err(wire_error)?;
        let payload = encode_payload(&packet)?;
        let framed =
            mornlea_protocol::write_frame(packet.key().id, &payload).map_err(wire_error)?;
        let length = framed.len();
        scratch.clear();
        scratch.extend_from_slice(&framed);
        Ok(length)
    }
}

fn sequenced(sequence: u64, command: Command) -> PlayIntent {
    PlayIntent::Sequenced { sequence, command }
}

/// Encodes one packet payload into an owned buffer via the registry's
/// exhaustive dispatcher, so no second encoder exists on this path.
fn encode_payload(packet: &ClientPacket) -> Result<Vec<u8>, ClientError> {
    // The largest client payload is bounded by the small-payload ceiling, so
    // one bounded scratch is always sufficient for a single record.
    let mut buffer = vec![0u8; mornlea_protocol::MAX_SMALL_PAYLOAD_BYTES + 16];
    let written = mornlea_protocol::encode_client_into(packet, &mut buffer).map_err(wire_error)?;
    buffer.truncate(written);
    Ok(buffer)
}

/// Maps the protocol failure classes onto the client error set: a capacity
/// or sizing refusal is `Capacity`, every other wire refusal is a malformed
/// input.
pub(crate) fn wire_error(error: ProtocolError) -> ClientError {
    match error {
        ProtocolError::OutputTooSmall { .. } | ProtocolError::Allocation => ClientError::Capacity,
        _ => ClientError::InvalidInput,
    }
}

impl From<DomainError> for ClientError {
    fn from(_: DomainError) -> Self {
        // Every checked domain rejection of a client-supplied payload is an
        // invalid input at this boundary; the payload types make the finer
        // domain classification unrepresentable here.
        ClientError::InvalidInput
    }
}

/// The checked classification of the container-addressed intents. It is not
/// a second wire command; a token is required for the container and close
/// intents and for partial/quick/drop actions whose accepted source region
/// is the current external container.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ContainerOperation {
    Move(ContainerMove),
    Partial(PartialMove),
    QuickMove(StackSource),
    DropStack(StackSource),
    Close,
}

impl ContainerOperation {
    /// The checked classification of one intent's container operation, or
    /// `None` when the intent addresses no container region at all.
    pub fn classify(intent: &ClientIntent) -> Option<ContainerOperation> {
        match intent {
            ClientIntent::MoveContainer(move_) => Some(ContainerOperation::Move(*move_)),
            ClientIntent::CloseContainer => Some(ContainerOperation::Close),
            ClientIntent::MovePartial(partial) => match partial.view() {
                StackView::Container(_) => Some(ContainerOperation::Partial(*partial)),
                _ => None,
            },
            ClientIntent::QuickMove(source) => match source.view() {
                StackView::Container(_) => Some(ContainerOperation::QuickMove(*source)),
                _ => None,
            },
            ClientIntent::DropStack(source) => match source.view() {
                StackView::Container(_) => Some(ContainerOperation::DropStack(*source)),
                _ => None,
            },
            _ => None,
        }
    }
}

/// The outcome of the read-only validator: everything `commit` needs,
/// precomputed, with no owner mutated and no owner borrowed.
#[derive(Clone, Debug, PartialEq)]
pub struct ValidatedInputBatch {
    epoch: SessionEpoch,
    actions: Vec<InputAction>,
    required_encoded_bytes: usize,
    sequenced_count: u16,
    chat_count: u16,
    required_journal: usize,
}

impl ValidatedInputBatch {
    pub fn epoch(&self) -> SessionEpoch {
        self.epoch
    }

    pub fn actions(&self) -> &[InputAction] {
        &self.actions
    }

    pub fn required_encoded_bytes(&self) -> usize {
        self.required_encoded_bytes
    }

    pub fn sequenced_count(&self) -> u16 {
        self.sequenced_count
    }

    pub fn chat_count(&self) -> u16 {
        self.chat_count
    }

    pub fn required_journal(&self) -> usize {
        self.required_journal
    }
}

/// One encoded outbound command owned by the admission queue, carrying the
/// complete prefix-inclusive F1 frame and the local sequence it attests.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct OutboundRecord {
    pub sequence: Option<u64>,
    pub frame: Vec<u8>,
}

/// One prediction journal entry, counted once per accepted local sequence.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct JournalEntry {
    pub epoch: SessionEpoch,
    pub sequence: u64,
    pub kind: ClientIntentKind,
}

/// A pending local cue-source event emitted by an accepted semantic UI
/// action. The audio projection reads these without consuming them until a
/// successful publication commits them.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LocalCueSource {
    pub local_event_sequence: u64,
    pub kind: ClientIntentKind,
}

/// The bounded epoch-scoped tombstone of locally closed external container
/// attribution.
///
/// It is not authoritative inventory state: the confirmed mirror is untouched
/// by a local close. Each tombstone records the confirmed revision the view
/// carried when it was locally closed. It retires when the confirmed view is
/// removed (an accepted confirmed close) or replaced by a newer revision, so
/// a later explicit reopened or replaced view supplies fresh attribution; an
/// unrelated observation leaves a still-locally-closed view closed, and a
/// reset clears the epoch overlay entirely.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LocalViewValidity {
    epoch: SessionEpoch,
    tombstones: Vec<(ContainerRef, ConfirmedRevision)>,
}

impl LocalViewValidity {
    pub fn try_new(epoch: SessionEpoch) -> Result<Self, ClientError> {
        Ok(Self {
            epoch,
            tombstones: Vec::new(),
        })
    }

    pub fn epoch(&self) -> SessionEpoch {
        self.epoch
    }

    pub fn is_closed(&self, reference: &ContainerRef) -> bool {
        self.tombstones
            .iter()
            .any(|(staged, _)| staged == reference)
    }

    pub fn tombstones(&self) -> &[(ContainerRef, ConfirmedRevision)] {
        &self.tombstones
    }

    /// Retires tombstones whose confirmed view has been removed or replaced.
    /// A view still present at the revision it carried when it was locally
    /// closed stays closed.
    pub fn retire_confirmed(&mut self, mirror: &ConfirmedMirror) {
        self.tombstones
            .retain(|(reference, closing)| mirror.container_revision(reference) == Some(*closing));
    }

    pub(crate) fn mark_closed(&mut self, reference: ContainerRef, revision: ConfirmedRevision) {
        if !self.is_closed(&reference) {
            self.tombstones.push((reference, revision));
        }
    }

    pub(crate) fn clear(&mut self) {
        self.tombstones.clear();
    }
}

/// The C1-owned single-owner admission state: outbound queue, prediction
/// journal, next sequence, the local view-validity overlay and the pending
/// local cue-source metadata. No projection or host callback obtains this
/// owner; `commit` is its only writer.
pub struct InputAdmissionState {
    outbound: Vec<OutboundRecord>,
    outbound_bytes: usize,
    journal: Vec<JournalEntry>,
    next_sequence: u64,
    overlay: LocalViewValidity,
    pending_local_cues: Vec<LocalCueSource>,
}

impl InputAdmissionState {
    /// Publishes the empty owner for one epoch. The sequence space starts at
    /// one so every sequenced packet's nonzero-sequence gate holds.
    pub fn try_new(epoch: SessionEpoch) -> Result<Self, ClientError> {
        Ok(Self {
            outbound: Vec::new(),
            outbound_bytes: 0,
            journal: Vec::new(),
            next_sequence: 1,
            overlay: LocalViewValidity::try_new(epoch)?,
            pending_local_cues: Vec::new(),
        })
    }

    pub fn overlay(&self) -> &LocalViewValidity {
        &self.overlay
    }

    pub fn outbound_records(&self) -> usize {
        self.outbound.len()
    }

    pub fn outbound_bytes(&self) -> usize {
        self.outbound_bytes
    }

    pub fn journal_entries(&self) -> usize {
        self.journal.len()
    }

    pub fn next_sequence(&self) -> u64 {
        self.next_sequence
    }

    pub fn pending_local_cues(&self) -> &[LocalCueSource] {
        &self.pending_local_cues
    }

    /// The framed bytes of the queue head, for retry after a transport
    /// `Capacity`/`Io` failure. The whole head is retained, never a partial
    /// record.
    pub fn outbound_head_frame(&self) -> Option<&[u8]> {
        self.outbound.first().map(|record| record.frame.as_slice())
    }

    /// Clears every epoch-scoped owner. Reset calls this; old sequences,
    /// journal entries and tombstones cannot cross into the new epoch.
    pub fn reset_epoch(&mut self, epoch: SessionEpoch) -> Result<(), ClientError> {
        self.outbound.clear();
        self.outbound_bytes = 0;
        self.journal.clear();
        self.next_sequence = 1;
        self.overlay = LocalViewValidity::try_new(epoch)?;
        self.pending_local_cues.clear();
        Ok(())
    }
}

/// The C1 validator and committer. All methods are associated functions over
/// checked inputs; neither path invents a sequence, revives a closed view or
/// publishes a partial admission.
pub struct InputTranslator;

impl InputTranslator {
    /// Read-only whole-batch validation over the confirmed mirror.
    ///
    /// Checks session phase and epoch, the action-count bound, every finite
    /// and ranged control value (the payload constructors already checked
    /// them), every current container/crafting token and revision, and every
    /// token relevance rule. It precomputes the encoded byte demand, the
    /// sequenced/chat counts and the journal demand without mutating any
    /// owner. A zero-action batch is valid and commits as `Noop`.
    pub fn validate_batch(
        batch: &InputBatch,
        mirror: &ConfirmedMirror,
        limits: &ClientLimits,
    ) -> Result<ValidatedInputBatch, ClientError> {
        if mirror.phase() != &crate::contracts::SessionPhase::Admitted {
            return Err(ClientError::InvalidState);
        }
        if mirror.epoch() != batch.epoch() {
            return Err(ClientError::StaleEpoch);
        }
        if batch.actions().len() > limits.queued_input_events() {
            return Err(ClientError::Capacity);
        }
        let mut required_encoded_bytes = 0usize;
        let mut sequenced_count = 0u16;
        let mut chat_count = 0u16;
        let mut scratch = Vec::new();
        for action in batch.actions() {
            check_token_relevance(action)?;
            match &action.intent {
                ClientIntent::Chat(_) => {
                    chat_count = chat_count.checked_add(1).ok_or(ClientError::Capacity)?;
                    let bytes = action.intent.encode_frame(0, &mut scratch)?;
                    required_encoded_bytes = required_encoded_bytes
                        .checked_add(bytes)
                        .ok_or(ClientError::Capacity)?;
                }
                _ => {
                    sequenced_count = sequenced_count
                        .checked_add(1)
                        .ok_or(ClientError::Capacity)?;
                    // The exact sequence value is assigned at commit; encoding
                    // here only measures the byte demand with a placeholder.
                    let bytes = action.intent.encode_frame(1, &mut scratch)?;
                    required_encoded_bytes = required_encoded_bytes
                        .checked_add(bytes)
                        .ok_or(ClientError::Capacity)?;
                }
            }
            check_token_current(action, mirror)?;
        }
        Ok(ValidatedInputBatch {
            epoch: batch.epoch(),
            actions: batch.actions().to_vec(),
            required_encoded_bytes,
            sequenced_count,
            chat_count,
            required_journal: sequenced_count as usize,
        })
    }

    /// The single-owner atomic commit.
    ///
    /// Rechecks every capacity (outbound records and bytes, journal, checked
    /// contiguous sequence range), simulates the local view-validity overlay
    /// over a temporary copy, encodes all actions into owned reserved
    /// temporary buffers, and only then appends records, journal entries and
    /// local cue metadata, advances the sequence and swaps the overlay in one
    /// critical section. Any earlier failure leaves every owner unchanged.
    pub fn commit(
        batch: ValidatedInputBatch,
        admission: &mut InputAdmissionState,
        mirror: &ConfirmedMirror,
        limits: &ClientLimits,
    ) -> Result<InputReceipt, ClientError> {
        if batch.actions().is_empty() {
            return Ok(InputReceipt::Noop);
        }
        let new_records = usize::from(batch.sequenced_count) + usize::from(batch.chat_count);
        if admission.outbound.len() + new_records > limits.outbound_commands() {
            return Err(ClientError::Capacity);
        }
        let new_bytes = admission
            .outbound_bytes
            .checked_add(batch.required_encoded_bytes())
            .ok_or(ClientError::Capacity)?;
        if new_bytes > limits.outbound_bytes() {
            return Err(ClientError::Capacity);
        }
        if admission.journal.len() + batch.required_journal() > limits.prediction_journal() {
            return Err(ClientError::Capacity);
        }
        let first_sequence = if batch.sequenced_count() == 0 {
            None
        } else {
            let span = u64::from(batch.sequenced_count()) - 1;
            let last = admission
                .next_sequence
                .checked_add(span)
                .ok_or(ClientError::Capacity)?;
            if last == 0 {
                return Err(ClientError::Capacity);
            }
            Some(admission.next_sequence)
        };

        // The temporary overlay simulation: a locally closed external view
        // rejects any later external-view action in this batch, and an
        // already-committed close rejects every subsequent batch until the
        // confirmed close/reopen retires it. The confirmed mirror is never
        // mutated here.
        let mut simulated = admission.overlay.clone();
        simulated.retire_confirmed(mirror);
        for action in batch.actions() {
            simulate_local_close(&action, &mut simulated)?;
        }

        // Tentative encode into reserved temporary buffers; an encoding
        // failure here publishes nothing and reserves nothing. The real
        // encoded sizes are recomputed because the assigned sequence values
        // (not the validator's placeholder) are part of the frame.
        let mut records = Vec::with_capacity(new_records);
        let mut entries = Vec::with_capacity(batch.required_journal());
        let mut sequence = admission.next_sequence;
        let mut actual_bytes = 0usize;
        let mut scratch = Vec::new();
        for action in batch.actions() {
            match &action.intent {
                ClientIntent::Chat(_) => {
                    let mut frame = Vec::new();
                    let bytes = action.intent.encode_frame(0, &mut scratch)?;
                    frame.extend_from_slice(&scratch[..bytes]);
                    actual_bytes = actual_bytes
                        .checked_add(bytes)
                        .ok_or(ClientError::Capacity)?;
                    records.push(OutboundRecord {
                        sequence: None,
                        frame,
                    });
                }
                _ => {
                    let mut frame = Vec::new();
                    let bytes = action.intent.encode_frame(sequence, &mut scratch)?;
                    frame.extend_from_slice(&scratch[..bytes]);
                    actual_bytes = actual_bytes
                        .checked_add(bytes)
                        .ok_or(ClientError::Capacity)?;
                    records.push(OutboundRecord {
                        sequence: Some(sequence),
                        frame,
                    });
                    entries.push(JournalEntry {
                        epoch: batch.epoch(),
                        sequence,
                        kind: action.intent.kind(),
                    });
                    sequence += 1;
                }
            }
        }
        let final_bytes = admission
            .outbound_bytes
            .checked_add(actual_bytes)
            .ok_or(ClientError::Capacity)?;
        if final_bytes > limits.outbound_bytes() {
            return Err(ClientError::Capacity);
        }

        // The critical section: all owners change together or not at all.
        admission.outbound.extend(records);
        admission.outbound_bytes = final_bytes;
        admission.journal.extend(entries);
        admission.next_sequence = sequence;
        admission.overlay = simulated;
        Ok(InputReceipt::Queued {
            epoch: batch.epoch(),
            first_sequence,
            sequenced_count: batch.sequenced_count(),
            chat_count: batch.chat_count(),
        })
    }
}

/// Token relevance rules: a token is required for `MoveContainer` and
/// `CloseContainer` and for the partial/quick/drop intents whose accepted
/// source region is the current external container; inventory-only and
/// crafting operations require both token fields `None`. Supplying either
/// irrelevant token rejects the batch.
fn check_token_relevance(action: &InputAction) -> Result<(), ClientError> {
    let container_required = matches!(
        action.intent,
        ClientIntent::MoveContainer(_) | ClientIntent::CloseContainer
    );
    let crafting_required = matches!(
        action.intent,
        ClientIntent::MoveCrafting(_) | ClientIntent::TakeCraftingOutput
    );
    let container_region =
        matches!(
            action.intent,
            ClientIntent::MovePartial(_) | ClientIntent::QuickMove(_) | ClientIntent::DropStack(_)
        ) && matches!(container_region_of(action), Some(StackView::Container(_)));
    let crafting_region = matches!(
        action.intent,
        ClientIntent::MovePartial(_) | ClientIntent::QuickMove(_) | ClientIntent::DropStack(_)
    ) && matches!(container_region_of(action), Some(StackView::Crafting));
    if action.container.is_some() && !(container_required || container_region) {
        return Err(ClientError::InvalidInput);
    }
    if action.crafting.is_some() && !(crafting_required || crafting_region) {
        return Err(ClientError::InvalidInput);
    }
    if (container_required || container_region) && action.container.is_none() {
        return Err(ClientError::InvalidInput);
    }
    if (crafting_required || crafting_region) && action.crafting.is_none() {
        return Err(ClientError::InvalidInput);
    }
    // Crafting moves never name an external container reference, so a
    // crafting intent with a container token would fabricate a workbench
    // reference.
    if matches!(action.intent, ClientIntent::MoveCrafting(_)) && action.container.is_some() {
        return Err(ClientError::InvalidInput);
    }
    Ok(())
}

/// The stack view a partial/quick/drop intent addresses.
fn container_region_of(action: &InputAction) -> Option<StackView> {
    match &action.intent {
        ClientIntent::MovePartial(partial) => Some(partial.view()),
        ClientIntent::QuickMove(source) => Some(source.view()),
        ClientIntent::DropStack(source) => Some(source.view()),
        _ => None,
    }
}

/// Current-token checks against the confirmed mirror: the token's epoch and
/// reference must equal the F1 payload reference and the mirror's current
/// attribution, and the crafting token must match the current confirmed
/// crafting view. Wrong epoch is stale; a reference the mirror does not
/// confirm or a stale revision is stale too; a mismatched payload reference
/// is an invalid shape.
fn check_token_current(action: &InputAction, mirror: &ConfirmedMirror) -> Result<(), ClientError> {
    if let Some(token) = &action.container {
        if token.epoch() != mirror.epoch() {
            return Err(ClientError::StaleEpoch);
        }
        if let Some(expected) = payload_container_reference(&action.intent) {
            if expected != token.reference() {
                return Err(ClientError::InvalidInput);
            }
        }
        if mirror.container_revision(&token.reference()) != Some(token.confirmed_revision()) {
            return Err(ClientError::StaleEpoch);
        }
    }
    if let Some(token) = &action.crafting {
        if token.epoch() != mirror.epoch() {
            return Err(ClientError::StaleEpoch);
        }
        if mirror.crafting_revision(token.size()) != Some(token.confirmed_revision()) {
            return Err(ClientError::StaleEpoch);
        }
    }
    Ok(())
}

/// The container reference the F1 payload itself names, for the intents whose
/// payload carries one. The token reference must equal it.
fn payload_container_reference(intent: &ClientIntent) -> Option<ContainerRef> {
    match intent {
        ClientIntent::MoveContainer(move_) => Some(move_.container()),
        ClientIntent::MovePartial(partial) => partial.view().container(),
        ClientIntent::QuickMove(source) => source.view().container(),
        ClientIntent::DropStack(source) => source.view().container(),
        _ => None,
    }
}

/// One simulated action against the temporary overlay. `CloseContainer`
/// marks the current external token closed; any later external-view action in
/// the same simulated batch fails, and opening a view never revives an old
/// token. Crafting validity changes only through its own confirmed-view
/// lifecycle, so crafting intents never consult the container overlay.
fn simulate_local_close(
    action: &InputAction,
    overlay: &mut LocalViewValidity,
) -> Result<(), ClientError> {
    if let Some(operation) = ContainerOperation::classify(&action.intent) {
        match operation {
            ContainerOperation::Close => {
                let token = action.container.as_ref().ok_or(ClientError::InvalidInput)?;
                overlay.mark_closed(token.reference(), token.confirmed_revision());
            }
            ContainerOperation::Move(move_) => {
                if overlay.is_closed(&move_.container()) {
                    return Err(ClientError::InvalidState);
                }
            }
            ContainerOperation::Partial(partial) => {
                if let Some(reference) = partial.view().container() {
                    if overlay.is_closed(&reference) {
                        return Err(ClientError::InvalidState);
                    }
                }
            }
            ContainerOperation::QuickMove(source) => {
                if let Some(reference) = source.view().container() {
                    if overlay.is_closed(&reference) {
                        return Err(ClientError::InvalidState);
                    }
                }
            }
            ContainerOperation::DropStack(source) => {
                if let Some(reference) = source.view().container() {
                    if overlay.is_closed(&reference) {
                        return Err(ClientError::InvalidState);
                    }
                }
            }
        }
    }
    Ok(())
}

/// The private per-batch input projection state owner declared by the
/// contract landing: checked admitted/rejected input metadata, the sequence
/// hint and the pending local cue-source events. Its actual owner replaces
/// the deterministic double before provider acceptance.
#[derive(Clone, Debug, Default)]
pub struct InputProjectionState {
    admitted: Vec<(u64, ClientIntentKind)>,
    rejected: Vec<(u64, ClientError)>,
    sequence_hint: u64,
    pending_local_cues: Vec<LocalCueSource>,
}

impl InputProjectionState {
    pub fn try_new(sequence_hint: u64) -> Result<Self, ClientError> {
        Ok(Self {
            admitted: Vec::new(),
            rejected: Vec::new(),
            sequence_hint,
            pending_local_cues: Vec::new(),
        })
    }

    pub fn record_admitted(&mut self, sequence: u64, kind: ClientIntentKind) {
        self.admitted.push((sequence, kind));
    }

    pub fn record_rejected(&mut self, sequence: u64, class: ClientError) {
        self.rejected.push((sequence, class));
    }

    pub fn admitted(&self) -> &[(u64, ClientIntentKind)] {
        &self.admitted
    }

    pub fn rejected(&self) -> &[(u64, ClientError)] {
        &self.rejected
    }

    pub fn sequence_hint(&self) -> u64 {
        self.sequence_hint
    }

    pub fn pending_local_cues(&self) -> &[LocalCueSource] {
        &self.pending_local_cues
    }
}
