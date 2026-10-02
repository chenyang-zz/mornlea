//! Complete inventory UI family assembly contract tests.
//!
//! The table pins the serial inventory-UI family assembly against the four
//! accepted real providers over real committed observations: the four checked
//! vectors combine into one coherent `inventory-ui@1` vector where retained
//! order keys — never the parts order and never equal or absent source ticks —
//! determine the interleaving; an accepted move republication and a command
//! rejection remain distinct records, the rejection merged exactly as the
//! family-wide rejection projector emitted it; the token and outcome union of
//! the parts survives unchanged with every container attribution naming its
//! own observation's epoch and revision; a close before its open and a
//! rejection before any crafting publication keep their actual source order
//! under the accepted close tolerance; and a stale, missing or irrelevant
//! token, an ambiguous duplicate order slot, an envelope disagreeing with its
//! record, a foreign epoch or revision, and a family count or frame byte cap
//! plus one each reject the whole family with the previous output retained.
//! The per-size crafting grids share one latest-wins topic identity, so
//! personal and workbench publications interleave by source order alone.

use mornlea_client_core::contracts::{
    ClientError, ClientLimits, ConfirmedRevision, FAMILY_INVENTORY_UI, FamilyKey, FamilyOperation,
    ObservationKey, RecordHeader, SessionEpoch,
};
use mornlea_client_core::input::{ContainerToken, InputProjectionState};
use mornlea_client_core::prediction::PlayerProjectionState;
use mornlea_client_core::presentation::family_inventory_ui::assemble_inventory_ui;
use mornlea_client_core::presentation::frame::{
    FamilyFrame, FamilyRecords, InventoryUiRecord, PresentationFrame,
};
use mornlea_client_core::presentation::inventory_ui::container::project_container;
use mornlea_client_core::presentation::inventory_ui::crafting::project_crafting;
use mornlea_client_core::presentation::inventory_ui::furnace::project_furnace;
use mornlea_client_core::presentation::inventory_ui::inventory::project_inventory;
use mornlea_client_core::presentation::{
    AcceptedObservation, AudioProjectionState, DiagnosticProjectionState, ErrorClassCounters,
    InventoryTopic, InventoryUiView, LifecycleProjectionState, MovementIntent, OrderedRecord, Pose,
    ProducerIdentity, ProjectionOrder, ProjectionView, QueueCounters, StableRecordKey, UiOutcome,
};
use mornlea_client_core::session::mirror::MirrorProvider;
use mornlea_domain::{
    ChestState, ChestStateParts, ChunkPos, CommandRejection, ContainerClosed, ContainerKind,
    ContainerRef, CraftingSize, CraftingState, CraftingStateParts, Event, FurnaceState,
    FurnaceStateParts, HotbarSlot, InventoryState, InventoryStateParts, ItemStack, RejectReason,
};
use mornlea_protocol::ServerPacket;

/// The epoch every fixture mirror and observation key carries.
const EPOCH: u64 = 45;

/// The candidate frame revision the hand-built envelope fixtures rebase onto.
const FRAME_REVISION: u64 = 10;

/// The registered stone item number, the Go `core.ItemStone` iota value.
const ITEM_STONE: u16 = 1;

/// The registered coal item number, the Go `core.ItemCoal` iota value.
const ITEM_COAL: u16 = 5;

/// The registered raw-iron item number, the Go `core.ItemRawIron` iota
/// value and a registered furnace smelting input.
const ITEM_RAW_IRON: u16 = 6;

/// The registered stick item number, the Go `core.ItemStick` iota value.
const ITEM_STICK: u16 = 37;

fn epoch() -> SessionEpoch {
    SessionEpoch::try_new(EPOCH).expect("epoch")
}

/// The transcript container position: chunk(-3,7), slot 5, matching the Go
/// container fixture position for both container kinds.
fn container_ref(kind: ContainerKind, generation: u32) -> ContainerRef {
    ContainerRef::try_new(ChunkPos::new(-3, 7), kind, 5, generation)
        .expect("checked container reference")
}

/// One checked nondurable stack, the ordinary slot value of the fixtures.
fn stack(item: u16, count: u8) -> ItemStack {
    ItemStack::try_new(item, count, 0).expect("checked nondurable stack")
}

/// One complete checked chest publication: the reference beside its
/// twenty-seven slots, all but the named entries empty.
fn chest_state(generation: u32, filled: Vec<(usize, ItemStack)>) -> ChestState {
    let mut items = [ItemStack::EMPTY; 27];
    for (index, value) in filled {
        items[index] = value;
    }
    ChestState::try_new(ChestStateParts {
        container: container_ref(ContainerKind::Chest, generation),
        items,
    })
    .expect("checked chest state")
}

/// One complete checked furnace publication with the named input slot and
/// idle timers.
fn furnace_state(generation: u32, input: ItemStack) -> FurnaceState {
    FurnaceState::try_new(FurnaceStateParts {
        container: container_ref(ContainerKind::Furnace, generation),
        input,
        fuel: ItemStack::EMPTY,
        output: ItemStack::EMPTY,
        progress_ticks: 0,
        burn_ticks: 0,
    })
    .expect("checked furnace state")
}

/// One complete checked crafting publication: the grid size, the named cells
/// and the authority-derived output; every unnamed cell is the canonical
/// empty stack, which is exactly how the personal grid's unused cells publish.
fn crafting_state(
    size: CraftingSize,
    cells: &[(usize, ItemStack)],
    output: ItemStack,
) -> CraftingState {
    let mut slots = [ItemStack::EMPTY; 9];
    for (index, value) in cells {
        slots[*index] = *value;
    }
    CraftingState::try_new(CraftingStateParts {
        size,
        slots,
        output,
    })
    .expect("checked crafting state")
}

/// One complete checked personal-inventory publication: the selected index,
/// the named hotbar placements and every other slot the canonical empty
/// stack.
fn inventory_state(selected: u8, hotbar: Vec<(usize, ItemStack)>) -> InventoryState {
    let mut hotbar_slots = [ItemStack::EMPTY; 9];
    for (index, value) in hotbar {
        hotbar_slots[index] = value;
    }
    InventoryState::new(InventoryStateParts {
        selected: HotbarSlot::new(selected).expect("checked hotbar slot"),
        hotbar: hotbar_slots,
        backpack: [ItemStack::EMPTY; 27],
    })
}

fn packet(event: Event) -> ServerPacket {
    ServerPacket::try_from(event).expect("publication converts to its packet")
}

/// An admitted real mirror provider with no observations yet.
fn admitted_mirror() -> MirrorProvider {
    let mut provider =
        MirrorProvider::new(epoch(), ClientLimits::try_new().expect("limits")).expect("mirror");
    provider.admit().expect("admitted session");
    provider
}

/// Commits one publication through the real mirror provider, so the retained
/// observation queue carries the actual source keys and the inventory store
/// carries the actual local view attribution.
fn commit(provider: &mut MirrorProvider, event: Event) -> ConfirmedRevision {
    let next = provider.mirror().revision().get() + 1;
    let key = ObservationKey::try_new(provider.mirror().epoch(), ConfirmedRevision::new(next), 0)
        .expect("staged key");
    let staged =
        AcceptedObservation::try_new(key, None, packet(event), Vec::new()).expect("staged");
    provider.commit(&staged).expect("committed observation")
}

/// The projection-view state owners a fixture keeps alive beside the
/// borrowed mirror and observation queue.
struct ViewFixture {
    input: InputProjectionState,
    player: PlayerProjectionState,
    audio: AudioProjectionState,
    lifecycle: LifecycleProjectionState,
    diagnostics: DiagnosticProjectionState,
    limits: ClientLimits,
}

impl ViewFixture {
    fn new() -> Self {
        Self {
            input: InputProjectionState::try_new(1).expect("input projection state"),
            player: PlayerProjectionState::try_new(
                epoch(),
                ConfirmedRevision::new(1),
                Pose::try_new([0.0, 64.0, 0.0], 0.0, 0.0).expect("finite pose"),
                None,
                Vec::new(),
                None,
                None,
                MovementIntent::try_new(None, false).expect("movement intent"),
            )
            .expect("player projection state"),
            audio: AudioProjectionState::try_new().expect("audio projection state"),
            lifecycle: LifecycleProjectionState::try_new(1).expect("lifecycle projection state"),
            diagnostics: DiagnosticProjectionState::try_new(
                ProducerIdentity::try_new([0u8; 20], [0u8; 32]).expect("producer identity"),
                QueueCounters::default(),
                ErrorClassCounters::default(),
            )
            .expect("diagnostics projection state"),
            limits: ClientLimits::try_new().expect("limits"),
        }
    }

    /// The candidate view of the provider's committed state: the next
    /// revision the coherent frame would carry.
    fn view_of<'a>(&'a self, provider: &'a MirrorProvider) -> ProjectionView<'a> {
        let revision = ConfirmedRevision::new(provider.mirror().revision().get() + 1);
        ProjectionView::try_new(
            provider.mirror(),
            provider.observations(),
            &self.input,
            &self.player,
            &self.audio,
            &self.lifecycle,
            &self.diagnostics,
            provider.mirror().epoch(),
            revision,
            5,
            &self.limits,
        )
        .expect("projection view")
    }
}

fn view_fixture() -> ViewFixture {
    ViewFixture::new()
}

/// The four provider vectors of one view, in the packet's provider order:
/// inventory, container, crafting, furnace.
fn provider_parts(view: &ProjectionView<'_>) -> [Vec<OrderedRecord<InventoryUiRecord>>; 4] {
    [
        project_inventory(view).expect("inventory projection"),
        project_container(view).expect("container projection"),
        project_crafting(view).expect("crafting projection"),
        project_furnace(view).expect("furnace projection"),
    ]
}

/// Assembles the four real provider vectors of one committed fixture state
/// under the supplied limits.
fn assemble(
    fixture: &ViewFixture,
    provider: &MirrorProvider,
    limits: &ClientLimits,
) -> Result<Vec<InventoryUiRecord>, ClientError> {
    let view = fixture.view_of(provider);
    let parts = provider_parts(&view);
    assemble_inventory_ui(view.frame_epoch(), view.frame_revision(), parts, limits)
}

/// Tightened frozen-ceiling limits: every bound at its frozen value except
/// the family record count and the frame byte cap.
fn limits_with(family_records: usize, frame_bytes: usize) -> ClientLimits {
    let frozen = ClientLimits::try_new().expect("frozen limits");
    ClientLimits::try_new_with(
        frozen.queued_input_events(),
        frozen.inbound_observations(),
        frozen.inbound_bytes(),
        frozen.outbound_commands(),
        frozen.outbound_bytes(),
        frozen.prediction_journal(),
        frozen.message_work(),
        frozen.mesh_work(),
        frozen.preparation_results(),
        frozen.preparation_bytes(),
        family_records,
        frame_bytes,
    )
    .expect("tightened limits")
}

/// The exact minimal inventory-ui-only frame size of one assembled vector at
/// the assembly identity, through the accepted frame accounting the
/// assembler itself uses.
fn minimal_frame_size(
    epoch: SessionEpoch,
    revision: ConfirmedRevision,
    records: &[InventoryUiRecord],
) -> usize {
    PresentationFrame::try_new(
        epoch,
        revision,
        0,
        vec![
            FamilyFrame::try_new(
                FamilyKey::try_new(FAMILY_INVENTORY_UI).expect("inventory-ui family key"),
                FamilyRecords::InventoryUi(records.to_vec()),
            )
            .expect("inventory-ui family entry"),
        ],
    )
    .expect("minimal inventory-ui-only candidate frame")
    .validated_size()
    .expect("checked size")
}

// --- hand-built envelope fixtures for sequences the mirror can never commit ---

/// The rebased coherent candidate header of one hand-built record.
fn hand_header(operation: FamilyOperation) -> RecordHeader {
    RecordHeader::try_new(
        epoch(),
        ConfirmedRevision::new(FRAME_REVISION),
        None,
        operation,
    )
    .expect("checked header")
}

fn inventory_key(topic: InventoryTopic, identity: Option<ContainerRef>) -> StableRecordKey {
    StableRecordKey::Inventory { topic, identity }
}

/// One confirmed envelope at the named observation revision and packet record
/// ordinal, wrapping the given record under the given stable identity.
fn envelope(
    observation_revision: u64,
    record_ordinal: u32,
    stable_key: StableRecordKey,
    record: InventoryUiRecord,
) -> OrderedRecord<InventoryUiRecord> {
    OrderedRecord::try_new(
        ProjectionOrder::Confirmed {
            observation: ObservationKey::try_new(
                epoch(),
                ConfirmedRevision::new(observation_revision),
                0,
            )
            .expect("staged key"),
            record_ordinal,
        },
        stable_key,
        record,
    )
    .expect("checked envelope")
}

/// The local attribution a hand-built container view carries: this epoch, the
/// view's reference and the revision its own observation staged it at.
fn view_token(reference: ContainerRef, observation_revision: u64) -> ContainerToken {
    ContainerToken::try_new(
        epoch(),
        reference,
        ConfirmedRevision::new(observation_revision),
    )
    .expect("checked container token")
}

/// A minimal checked chest upsert record for the hand-built sequences.
fn chest_record(reference: ContainerRef, token: Option<ContainerToken>) -> InventoryUiRecord {
    InventoryUiRecord::try_new(
        hand_header(FamilyOperation::Upsert),
        InventoryUiView::Chest(
            ChestState::try_new(ChestStateParts {
                container: reference,
                items: [ItemStack::EMPTY; 27],
            })
            .expect("checked chest state"),
        ),
        token,
        None,
    )
    .expect("checked chest record")
}

/// A minimal checked furnace upsert record for the hand-built sequences.
fn furnace_record(reference: ContainerRef, token: Option<ContainerToken>) -> InventoryUiRecord {
    InventoryUiRecord::try_new(
        hand_header(FamilyOperation::Upsert),
        InventoryUiView::Furnace(
            FurnaceState::try_new(FurnaceStateParts {
                container: reference,
                input: ItemStack::EMPTY,
                fuel: ItemStack::EMPTY,
                output: ItemStack::EMPTY,
                progress_ticks: 0,
                burn_ticks: 0,
            })
            .expect("checked furnace state"),
        ),
        token,
        None,
    )
    .expect("checked furnace record")
}

/// A minimal checked closed removal record for the hand-built sequences.
fn closed_record(reference: ContainerRef, token: Option<ContainerToken>) -> InventoryUiRecord {
    InventoryUiRecord::try_new(
        hand_header(FamilyOperation::Remove),
        InventoryUiView::Closed(reference),
        token,
        None,
    )
    .expect("checked closed record")
}

/// A minimal checked personal-inventory upsert record for the hand-built
/// sequences.
fn inventory_record(
    token: Option<ContainerToken>,
    outcome: Option<UiOutcome>,
) -> InventoryUiRecord {
    InventoryUiRecord::try_new(
        hand_header(FamilyOperation::Upsert),
        InventoryUiView::Inventory(inventory_state(0, Vec::new())),
        token,
        outcome,
    )
    .expect("checked inventory record")
}

/// A minimal checked rejection record for the hand-built sequences, carrying
/// the payload-matching outcome the family-wide rejection projector emits.
fn rejected_record(sequence: u64) -> InventoryUiRecord {
    let rejection = CommandRejection::new(sequence, RejectReason::InvalidInput);
    InventoryUiRecord::try_new(
        hand_header(FamilyOperation::Upsert),
        InventoryUiView::Rejected(rejection),
        None,
        Some(UiOutcome::Rejected {
            sequence: rejection.sequence(),
            reason: rejection.reason(),
        }),
    )
    .expect("checked rejection record")
}

fn empty_parts() -> [Vec<OrderedRecord<InventoryUiRecord>>; 4] {
    [Vec::new(), Vec::new(), Vec::new(), Vec::new()]
}

fn assemble_hand_built(
    parts: [Vec<OrderedRecord<InventoryUiRecord>>; 4],
) -> Result<Vec<InventoryUiRecord>, ClientError> {
    assemble_inventory_ui(
        epoch(),
        ConfirmedRevision::new(FRAME_REVISION),
        parts,
        &ClientLimits::try_new().expect("limits"),
    )
}

/// The view tag one assembled record carries, for interleaving assertions
/// that never depend on payload details.
fn view_tag(record: &InventoryUiRecord) -> InventoryTopic {
    match record.view() {
        InventoryUiView::Inventory(_) => InventoryTopic::Inventory,
        InventoryUiView::Crafting(_) => InventoryTopic::Crafting,
        InventoryUiView::Furnace(_) => InventoryTopic::Furnace,
        InventoryUiView::Chest(_) => InventoryTopic::Chest,
        InventoryUiView::Closed(_) => InventoryTopic::Closed,
        InventoryUiView::Rejected(_) => InventoryTopic::Rejected,
    }
}

// --- the table ---

/// `four providers interleave`: the four real providers' vectors of one
/// committed view assemble into one family vector in actual observation
/// order — a close interleaved with inventory, crafting and furnace
/// publications — and the parts order is free: a rotated parts array yields
/// the identical vector. Every inventory-ui publication carries no source
/// tick, so the preserved order comes from the retained envelopes alone, and
/// the emitted vector is the unchanged record payload with the envelopes
/// stripped.
#[test]
fn four_providers_interleave_by_source_order_not_parts_order() {
    let mut provider = admitted_mirror();
    let chest = container_ref(ContainerKind::Chest, 9);
    commit(
        &mut provider,
        Event::InventoryState(inventory_state(1, vec![(0, stack(ITEM_STONE, 5))])),
    );
    commit(
        &mut provider,
        Event::ChestState(chest_state(9, vec![(0, stack(ITEM_STONE, 5))])),
    );
    commit(
        &mut provider,
        Event::CraftingState(crafting_state(
            CraftingSize::Personal,
            &[(0, stack(ITEM_STONE, 1))],
            ItemStack::EMPTY,
        )),
    );
    commit(
        &mut provider,
        Event::ContainerClosed(ContainerClosed::new(chest)),
    );
    commit(
        &mut provider,
        Event::FurnaceState(furnace_state(4, stack(ITEM_RAW_IRON, 3))),
    );
    commit(
        &mut provider,
        Event::InventoryState(inventory_state(2, vec![(1, stack(ITEM_STICK, 2))])),
    );

    let fixture = view_fixture();
    let view = fixture.view_of(&provider);
    let parts = provider_parts(&view);
    let limits = ClientLimits::try_new().expect("limits");
    let assembled = assemble_inventory_ui(
        view.frame_epoch(),
        view.frame_revision(),
        parts.clone(),
        &limits,
    )
    .expect("the four providers assemble");

    let expected = [
        InventoryTopic::Inventory,
        InventoryTopic::Chest,
        InventoryTopic::Crafting,
        InventoryTopic::Closed,
        InventoryTopic::Furnace,
        InventoryTopic::Inventory,
    ];
    assert_eq!(assembled.len(), expected.len());
    for (record, topic) in assembled.iter().zip(&expected) {
        assert_eq!(
            view_tag(record),
            *topic,
            "actual source order, not topic batching"
        );
        assert_eq!(
            record.header().source_tick(),
            None,
            "no inventory-ui publication carries a tick and none is invented"
        );
        assert_eq!(record.header().epoch(), view.frame_epoch());
        assert_eq!(
            record.header().revision(),
            view.frame_revision(),
            "one coherent candidate revision across the merged family"
        );
    }

    // The emitted vector is the unchanged record payload: the envelopes are
    // stripped after validation and nothing else about a record changes.
    let mut expected_records: Vec<OrderedRecord<InventoryUiRecord>> = parts.concat();
    expected_records.sort_by(|left, right| {
        left.order()
            .cmp(right.order())
            .then_with(|| left.stable_key().cmp(right.stable_key()))
    });
    let expected_records: Vec<InventoryUiRecord> = expected_records
        .into_iter()
        .map(|entry| entry.into_record())
        .collect();
    assert_eq!(assembled, expected_records, "records are emitted unchanged");

    // A rotated parts array — the furnace vector first — yields the identical
    // output because retained order keys, not parts order, interleave.
    let rotated = [
        parts[3].clone(),
        parts[0].clone(),
        parts[1].clone(),
        parts[2].clone(),
    ];
    let rotated =
        assemble_inventory_ui(view.frame_epoch(), view.frame_revision(), rotated, &limits)
            .expect("rotated parts assemble");
    assert_eq!(assembled, rotated, "parts order never reorders the family");
}

/// `close and rejection against their open`: a close ordered before its own
/// open — the accepted stale-close tolerance shape — and a rejection ordered
/// before any crafting publication both keep their actual source order; the
/// ordinary open-then-close keeps its order the same way. None of the three
/// shapes rejects, because the close tolerance and the standalone rejection
/// are accepted provider semantics, not assembler malformations.
#[test]
fn close_then_open_and_reject_before_open_keep_retained_order() {
    // A close commits with no live view of its reference; the later
    // publication of the same reference then opens it.
    let mut close_first = admitted_mirror();
    let reference = container_ref(ContainerKind::Chest, 9);
    commit(
        &mut close_first,
        Event::ContainerClosed(ContainerClosed::new(reference)),
    );
    commit(
        &mut close_first,
        Event::ChestState(chest_state(9, vec![(0, stack(ITEM_STONE, 5))])),
    );
    let fixture = view_fixture();
    let assembled = assemble(
        &fixture,
        &close_first,
        &ClientLimits::try_new().expect("limits"),
    )
    .expect("the tolerated close-first shape assembles");
    assert_eq!(
        assembled.iter().map(view_tag).collect::<Vec<_>>(),
        vec![InventoryTopic::Closed, InventoryTopic::Chest],
        "the close keeps its actual source order before the open"
    );

    // A rejection commits with no crafting publication before it: the refusal
    // is an outcome record, never a contents change, so it stands alone.
    let mut reject_first = admitted_mirror();
    commit(
        &mut reject_first,
        Event::CommandRejected(CommandRejection::new(7, RejectReason::InvalidInput)),
    );
    commit(
        &mut reject_first,
        Event::CraftingState(crafting_state(
            CraftingSize::Workbench,
            &[(0, stack(ITEM_STONE, 2))],
            ItemStack::EMPTY,
        )),
    );
    let assembled = assemble(
        &fixture,
        &reject_first,
        &ClientLimits::try_new().expect("limits"),
    )
    .expect("a standalone rejection assembles");
    assert_eq!(
        assembled.iter().map(view_tag).collect::<Vec<_>>(),
        vec![InventoryTopic::Rejected, InventoryTopic::Crafting],
        "the rejection keeps its actual source order before the grid"
    );

    // The ordinary shape: the open precedes its own close.
    let mut open_first = admitted_mirror();
    commit(
        &mut open_first,
        Event::ChestState(chest_state(9, vec![(0, stack(ITEM_STONE, 5))])),
    );
    commit(
        &mut open_first,
        Event::ContainerClosed(ContainerClosed::new(reference)),
    );
    let assembled = assemble(
        &fixture,
        &open_first,
        &ClientLimits::try_new().expect("limits"),
    )
    .expect("the ordinary lifecycle assembles");
    assert_eq!(
        assembled.iter().map(view_tag).collect::<Vec<_>>(),
        vec![InventoryTopic::Chest, InventoryTopic::Closed]
    );
}

/// `accepted move and rejection stay distinct`: an authoritative move
/// republication of one container and a command rejection of a refused
/// action assemble as distinct records — both chest republications survive
/// with their own attributions and the rejection is merged exactly as the
/// family-wide rejection projector emitted it: one record, unchanged payload,
/// unchanged outcome, never produced here and never filtered away.
#[test]
fn accepted_move_and_rejection_remain_distinct_records() {
    let mut provider = admitted_mirror();
    let opened = chest_state(9, vec![(0, stack(ITEM_STONE, 5))]);
    let moved = chest_state(9, vec![(1, stack(ITEM_COAL, 2))]);
    commit(&mut provider, Event::ChestState(opened));
    commit(&mut provider, Event::ChestState(moved));
    commit(
        &mut provider,
        Event::CommandRejected(CommandRejection::new(7, RejectReason::InvalidInput)),
    );

    let fixture = view_fixture();
    let view = fixture.view_of(&provider);
    let parts = provider_parts(&view);
    let assembled = assemble(
        &fixture,
        &provider,
        &ClientLimits::try_new().expect("limits"),
    )
    .expect("the move and the rejection assemble");

    assert_eq!(
        assembled.iter().map(view_tag).collect::<Vec<_>>(),
        vec![
            InventoryTopic::Chest,
            InventoryTopic::Chest,
            InventoryTopic::Rejected
        ],
        "the accepted move and the rejection never collapse into one record"
    );
    assert_eq!(assembled[0].view(), &InventoryUiView::Chest(opened));
    assert_eq!(assembled[1].view(), &InventoryUiView::Chest(moved));
    assert_eq!(
        assembled[0]
            .token()
            .expect("the opened view carries its attribution")
            .confirmed_revision()
            .get(),
        1,
        "each republication's token names its own observation's revision"
    );
    assert_eq!(
        assembled[1]
            .token()
            .expect("the moved view carries its attribution")
            .confirmed_revision()
            .get(),
        2
    );

    // The rejection is the projector's own record merged as-is: exactly one
    // rejected record exists in the output for the one the provider emitted.
    let emitted = parts
        .iter()
        .flatten()
        .filter(|entry| matches!(entry.record().view(), InventoryUiView::Rejected(_)))
        .collect::<Vec<_>>();
    assert_eq!(
        emitted.len(),
        1,
        "the crafting provider emitted one rejection"
    );
    let merged_rejections = assembled
        .iter()
        .filter(|record| matches!(record.view(), InventoryUiView::Rejected(_)))
        .collect::<Vec<_>>();
    assert_eq!(
        merged_rejections.len(),
        emitted.len(),
        "the assembler never duplicates, filters or produces rejections"
    );
    assert_eq!(
        merged_rejections[0].view(),
        emitted[0].record().view(),
        "the merged rejection keeps its exact checked payload"
    );
    assert_eq!(
        merged_rejections[0].outcome(),
        emitted[0].record().outcome(),
        "the merged rejection keeps its exact sequence-bound outcome"
    );
    assert_eq!(
        merged_rejections[0].header().operation(),
        emitted[0].record().header().operation()
    );
    assert_eq!(merged_rejections[0].token(), None);
}

/// `token and outcome union`: the merged family carries the parts' token and
/// outcome union unchanged — the chest and furnace attributions name their
/// own epoch, reference and staged revision, and no other view carries a
/// token or an outcome, because no accepted provider attaches one.
#[test]
fn token_and_outcome_union_merged_unchanged() {
    let mut provider = admitted_mirror();
    let chest = container_ref(ContainerKind::Chest, 9);
    let furnace = container_ref(ContainerKind::Furnace, 4);
    commit(
        &mut provider,
        Event::ChestState(chest_state(9, vec![(0, stack(ITEM_STONE, 5))])),
    );
    commit(
        &mut provider,
        Event::FurnaceState(furnace_state(4, stack(ITEM_RAW_IRON, 3))),
    );
    commit(
        &mut provider,
        Event::CommandRejected(CommandRejection::new(9, RejectReason::InvalidInput)),
    );
    commit(
        &mut provider,
        Event::CraftingState(crafting_state(
            CraftingSize::Personal,
            &[(0, stack(ITEM_STONE, 1))],
            ItemStack::EMPTY,
        )),
    );
    commit(
        &mut provider,
        Event::InventoryState(inventory_state(0, vec![(0, stack(ITEM_STONE, 2))])),
    );
    commit(
        &mut provider,
        Event::ContainerClosed(ContainerClosed::new(chest)),
    );

    let fixture = view_fixture();
    let assembled = assemble(
        &fixture,
        &provider,
        &ClientLimits::try_new().expect("limits"),
    )
    .expect("the union assembles");
    assert_eq!(assembled.len(), 6);

    for record in &assembled {
        match view_tag(record) {
            InventoryTopic::Chest => {
                let token = record
                    .token()
                    .expect("a chest view carries its attribution");
                assert_eq!(*token, view_token(chest, 1));
            }
            InventoryTopic::Furnace => {
                let token = record
                    .token()
                    .expect("a furnace view carries its attribution");
                assert_eq!(*token, view_token(furnace, 2));
            }
            InventoryTopic::Rejected => {
                assert_eq!(record.token(), None);
                assert_eq!(
                    record.outcome(),
                    Some(&UiOutcome::Rejected {
                        sequence: 9,
                        reason: RejectReason::InvalidInput,
                    })
                );
            }
            _ => {
                assert_eq!(
                    record.token(),
                    None,
                    "no singleton or retired view carries a container attribution"
                );
                assert_eq!(
                    record.outcome(),
                    None,
                    "no contents publication carries a command outcome"
                );
            }
        }
    }
}

/// `stale, missing or irrelevant token`: a container attribution from another
/// epoch is stale; one naming a revision its own observation never staged is
/// fabricated; a container view without its attribution, a singleton or
/// retired view carrying one, and a token naming a foreign reference are
/// malformed. Each rejects the whole family typed, before any record is
/// emitted.
#[test]
fn stale_missing_or_irrelevant_token_rejects_typed() {
    let reference = container_ref(ContainerKind::Chest, 9);
    let other = container_ref(ContainerKind::Chest, 10);

    // A token from another epoch is stale attribution.
    let stale_epoch_token = ContainerToken::try_new(
        SessionEpoch::try_new(EPOCH + 1).expect("foreign epoch"),
        reference,
        ConfirmedRevision::new(1),
    )
    .expect("checked foreign token");
    let mut stale = empty_parts();
    stale[1] = vec![envelope(
        1,
        0,
        inventory_key(InventoryTopic::Chest, Some(reference)),
        chest_record(reference, Some(stale_epoch_token)),
    )];
    assert_eq!(
        assemble_hand_built(stale).expect_err("a token from another epoch is stale"),
        ClientError::StaleEpoch
    );

    // A token naming a revision its own observation never staged fabricates
    // mirror attribution.
    let mut fabricated = empty_parts();
    fabricated[1] = vec![envelope(
        2,
        0,
        inventory_key(InventoryTopic::Chest, Some(reference)),
        chest_record(reference, Some(view_token(reference, 3))),
    )];
    assert_eq!(
        assemble_hand_built(fabricated)
            .expect_err("a token naming a foreign staged revision is fabricated"),
        ClientError::InvalidInput
    );

    // A container view without its local attribution is malformed.
    let mut missing = empty_parts();
    missing[1] = vec![envelope(
        1,
        0,
        inventory_key(InventoryTopic::Chest, Some(reference)),
        chest_record(reference, None),
    )];
    assert_eq!(
        assemble_hand_built(missing).expect_err("a missing token rejects"),
        ClientError::InvalidInput
    );

    // A token on a singleton topic is irrelevant: the personal inventory
    // never occupies a container reference.
    let mut irrelevant_singleton = empty_parts();
    irrelevant_singleton[0] = vec![envelope(
        1,
        0,
        inventory_key(InventoryTopic::Inventory, None),
        inventory_record(Some(view_token(reference, 1)), None),
    )];
    assert_eq!(
        assemble_hand_built(irrelevant_singleton)
            .expect_err("a token on a singleton topic is irrelevant"),
        ClientError::InvalidInput
    );

    // A token on a retired view is irrelevant: a close carries the identity
    // alone.
    let mut irrelevant_closed = empty_parts();
    irrelevant_closed[1] = vec![envelope(
        1,
        0,
        inventory_key(InventoryTopic::Closed, Some(reference)),
        closed_record(reference, Some(view_token(reference, 1))),
    )];
    assert_eq!(
        assemble_hand_built(irrelevant_closed)
            .expect_err("a token on a retired view is irrelevant"),
        ClientError::InvalidInput
    );

    // A token naming a foreign reference disagrees with the view it rides.
    let mut foreign_reference = empty_parts();
    foreign_reference[1] = vec![envelope(
        1,
        0,
        inventory_key(InventoryTopic::Chest, Some(reference)),
        chest_record(reference, Some(view_token(other, 1))),
    )];
    assert_eq!(
        assemble_hand_built(foreign_reference)
            .expect_err("a token naming a foreign reference rejects"),
        ClientError::InvalidInput
    );
}

/// `ambiguous, duplicate and disagreeing envelopes`: two envelopes claiming
/// one retained order slot for one stable identity — in one part or across
/// two parts — an envelope whose stable identity disagrees with the record it
/// wraps, a view under the wrong operation tag, and an outcome that
/// disagrees with its rejection payload each reject the whole family typed.
/// Two envelopes at one order slot under distinct stable identities remain
/// legal and interleave by the stable-key tiebreak, which is the frozen
/// deterministic resolution of otherwise equal retained order.
#[test]
fn ambiguous_duplicate_or_disagreeing_envelopes_reject_typed() {
    let chest = container_ref(ContainerKind::Chest, 9);
    let furnace = container_ref(ContainerKind::Furnace, 4);
    let chest_key = inventory_key(InventoryTopic::Chest, Some(chest));
    let furnace_key = inventory_key(InventoryTopic::Furnace, Some(furnace));

    // Two envelopes claiming one order slot for one identity in one part.
    let mut duplicate_in_part = empty_parts();
    duplicate_in_part[1] = vec![
        envelope(
            1,
            0,
            chest_key,
            chest_record(chest, Some(view_token(chest, 1))),
        ),
        envelope(
            1,
            0,
            chest_key,
            chest_record(chest, Some(view_token(chest, 1))),
        ),
    ];
    assert_eq!(
        assemble_hand_built(duplicate_in_part)
            .expect_err("two envelopes claiming one order slot reject"),
        ClientError::InvalidInput
    );

    // The same duplicated slot spread across two parts is the same
    // malformation: the parts order must never decide which record wins.
    let mut duplicate_across_parts = empty_parts();
    duplicate_across_parts[1] = vec![envelope(
        1,
        0,
        chest_key,
        chest_record(chest, Some(view_token(chest, 1))),
    )];
    duplicate_across_parts[2] = vec![envelope(
        1,
        0,
        chest_key,
        chest_record(chest, Some(view_token(chest, 1))),
    )];
    assert_eq!(
        assemble_hand_built(duplicate_across_parts)
            .expect_err("a duplicated order slot across parts rejects"),
        ClientError::InvalidInput
    );

    // A duplicated rejection envelope — the family-wide projector's record
    // arriving twice at one order slot — is the same ambiguity: the assembler
    // merges rejection records as-is and never silently folds a duplicated
    // one into a single output record.
    let rejected_key = inventory_key(InventoryTopic::Rejected, None);
    let mut duplicate_rejection = empty_parts();
    duplicate_rejection[2] = vec![envelope(1, 0, rejected_key, rejected_record(7))];
    duplicate_rejection[0] = vec![envelope(1, 0, rejected_key, rejected_record(7))];
    assert_eq!(
        assemble_hand_built(duplicate_rejection)
            .expect_err("a duplicated rejection envelope rejects"),
        ClientError::InvalidInput
    );

    // An envelope naming another topic's identity for its record.
    let mut foreign_topic = empty_parts();
    foreign_topic[1] = vec![envelope(
        1,
        0,
        furnace_key,
        chest_record(chest, Some(view_token(chest, 1))),
    )];
    assert_eq!(
        assemble_hand_built(foreign_topic).expect_err("an envelope naming another topic rejects"),
        ClientError::InvalidInput
    );

    // A removal view under an upsert tag and a view under a removal tag.
    let mut close_upserted = empty_parts();
    close_upserted[1] = vec![envelope(
        1,
        0,
        inventory_key(InventoryTopic::Closed, Some(chest)),
        InventoryUiRecord::try_new(
            hand_header(FamilyOperation::Upsert),
            InventoryUiView::Closed(chest),
            None,
            None,
        )
        .expect("checked close record"),
    )];
    assert_eq!(
        assemble_hand_built(close_upserted)
            .expect_err("a removal view under an upsert tag rejects"),
        ClientError::InvalidInput
    );

    let mut chest_removed = empty_parts();
    chest_removed[1] = vec![envelope(
        1,
        0,
        chest_key,
        InventoryUiRecord::try_new(
            hand_header(FamilyOperation::Remove),
            InventoryUiView::Chest(
                ChestState::try_new(ChestStateParts {
                    container: chest,
                    items: [ItemStack::EMPTY; 27],
                })
                .expect("checked chest state"),
            ),
            Some(view_token(chest, 1)),
            None,
        )
        .expect("checked chest record"),
    )];
    assert_eq!(
        assemble_hand_built(chest_removed).expect_err("a view under a removal tag rejects"),
        ClientError::InvalidInput
    );

    // An outcome that disagrees with its rejection payload.
    let mut wrong_outcome = empty_parts();
    wrong_outcome[2] = vec![envelope(
        1,
        0,
        inventory_key(InventoryTopic::Rejected, None),
        InventoryUiRecord::try_new(
            hand_header(FamilyOperation::Upsert),
            InventoryUiView::Rejected(CommandRejection::new(7, RejectReason::InvalidInput)),
            None,
            Some(UiOutcome::Rejected {
                sequence: 8,
                reason: RejectReason::InvalidInput,
            }),
        )
        .expect("checked rejection record"),
    )];
    assert_eq!(
        assemble_hand_built(wrong_outcome)
            .expect_err("an outcome disagreeing with its payload rejects"),
        ClientError::InvalidInput
    );

    // An outcome on a contents publication no accepted provider attaches.
    let mut fabricated_outcome = empty_parts();
    fabricated_outcome[0] = vec![envelope(
        1,
        0,
        inventory_key(InventoryTopic::Inventory, None),
        inventory_record(None, Some(UiOutcome::PlacementAccepted { sequence: 4 })),
    )];
    assert_eq!(
        assemble_hand_built(fabricated_outcome)
            .expect_err("a fabricated outcome on a contents record rejects"),
        ClientError::InvalidInput
    );

    // The legal tie: distinct stable identities at one retained order slot
    // interleave by the stable-key tiebreak, furnace before chest in topic
    // variant order.
    let mut tied = empty_parts();
    tied[1] = vec![envelope(
        1,
        0,
        chest_key,
        chest_record(chest, Some(view_token(chest, 1))),
    )];
    tied[3] = vec![envelope(
        1,
        0,
        furnace_key,
        furnace_record(furnace, Some(view_token(furnace, 1))),
    )];
    let assembled = assemble_hand_built(tied).expect("distinct identities at one slot assemble");
    assert_eq!(
        assembled.iter().map(view_tag).collect::<Vec<_>>(),
        vec![InventoryTopic::Furnace, InventoryTopic::Chest],
        "the stable-key tiebreak resolves equal retained order deterministically"
    );
}

/// `coherent epoch and revision`: parts projected for one epoch and candidate
/// revision assemble only under exactly that identity — a foreign epoch is
/// stale and a foreign revision is incoherent, both without any partial
/// output.
#[test]
fn foreign_epoch_or_revision_rejects_without_partial_output() {
    let mut provider = admitted_mirror();
    commit(
        &mut provider,
        Event::ChestState(chest_state(9, vec![(0, stack(ITEM_STONE, 5))])),
    );

    let fixture = view_fixture();
    let view = fixture.view_of(&provider);
    let parts = provider_parts(&view);
    let limits = ClientLimits::try_new().expect("limits");

    let stale = assemble_inventory_ui(
        SessionEpoch::try_new(EPOCH + 1).expect("foreign epoch"),
        view.frame_revision(),
        parts.clone(),
        &limits,
    )
    .expect_err("a part from another epoch is stale");
    assert_eq!(stale, ClientError::StaleEpoch);

    let incoherent = assemble_inventory_ui(
        view.frame_epoch(),
        ConfirmedRevision::new(view.frame_revision().get() + 1),
        parts,
        &limits,
    )
    .expect_err("a part rebased onto another revision is incoherent");
    assert_eq!(incoherent, ClientError::InvalidInput);
}

/// `family count cap plus one`: an assembly at exactly the tightened family
/// record count publishes, one record more is a typed capacity rejection,
/// and the caller's retained previous output stays exactly as it was.
#[test]
fn family_count_cap_plus_one_retains_old_output() {
    let mut provider = admitted_mirror();
    commit(
        &mut provider,
        Event::ChestState(chest_state(11, vec![(0, stack(ITEM_STONE, 5))])),
    );
    commit(
        &mut provider,
        Event::ChestState(chest_state(12, vec![(1, stack(ITEM_COAL, 2))])),
    );
    commit(
        &mut provider,
        Event::ChestState(chest_state(13, vec![(2, stack(ITEM_STICK, 3))])),
    );

    let fixture = view_fixture();
    let frozen = ClientLimits::try_new().expect("frozen limits");
    let at_cap = limits_with(3, frozen.frame_bytes());
    let old = assemble(&fixture, &provider, &at_cap)
        .expect("exactly three records fit the family count cap");
    assert_eq!(old.len(), 3);

    commit(
        &mut provider,
        Event::ChestState(chest_state(14, vec![(3, stack(ITEM_STONE, 1))])),
    );
    let error = assemble(&fixture, &provider, &at_cap)
        .expect_err("one record past the family count cap rejects");
    assert_eq!(error, ClientError::Capacity);
    assert_eq!(old.len(), 3, "the retained previous output is unchanged");
    assert_eq!(
        old[2].view(),
        &InventoryUiView::Chest(chest_state(13, vec![(2, stack(ITEM_STICK, 3))])),
        "the retained previous output keeps its newest record"
    );
}

/// `frame byte cap plus one`: an assembly whose minimal inventory-ui-only
/// frame exactly fits the frame byte cap publishes, one record more is a
/// typed capacity rejection, and the retained previous output stays intact.
#[test]
fn frame_byte_cap_plus_one_retains_old_output() {
    let mut provider = admitted_mirror();
    commit(
        &mut provider,
        Event::InventoryState(inventory_state(0, Vec::new())),
    );
    commit(
        &mut provider,
        Event::InventoryState(inventory_state(1, Vec::new())),
    );
    commit(
        &mut provider,
        Event::InventoryState(inventory_state(2, Vec::new())),
    );

    let fixture = view_fixture();
    let frozen = ClientLimits::try_new().expect("frozen limits");
    let old = assemble(&fixture, &provider, &frozen)
        .expect("three inventory records assemble under the frozen caps");
    assert_eq!(old.len(), 3);

    let view = fixture.view_of(&provider);
    let exact_cap = minimal_frame_size(view.frame_epoch(), view.frame_revision(), &old);
    let at_cap = limits_with(frozen.family_records(), exact_cap);
    let fitted = assemble(&fixture, &provider, &at_cap)
        .expect("the exact minimal inventory-ui frame fits its own byte cap");
    assert_eq!(fitted.len(), 3);

    commit(
        &mut provider,
        Event::InventoryState(inventory_state(3, Vec::new())),
    );
    let error = assemble(&fixture, &provider, &at_cap)
        .expect_err("one record past the frame byte cap rejects");
    assert_eq!(error, ClientError::Capacity);
    assert_eq!(fitted.len(), 3, "the retained previous output is unchanged");
    assert_eq!(
        fitted[2].view(),
        &InventoryUiView::Inventory(inventory_state(2, Vec::new()))
    );
}

/// `per-size crafting grids share one topic`: the accepted schema binds the
/// crafting records to a single latest-wins topic identity with no container
/// identity, so personal and workbench publications of one committed queue
/// assemble under that one shared stable key and interleave by their actual
/// source order alone — each record restating the complete published value,
/// the latest publication per size carrying that size's live contents.
#[test]
fn per_size_crafting_publications_share_the_latest_wins_topic() {
    let mut provider = admitted_mirror();
    let personal = crafting_state(
        CraftingSize::Personal,
        &[(0, stack(ITEM_STONE, 1))],
        ItemStack::EMPTY,
    );
    let workbench = crafting_state(
        CraftingSize::Workbench,
        &[(0, stack(ITEM_STONE, 2)), (3, stack(ITEM_STICK, 1))],
        ItemStack::EMPTY,
    );
    let cleared = crafting_state(CraftingSize::Personal, &[], ItemStack::EMPTY);
    commit(&mut provider, Event::CraftingState(personal));
    commit(&mut provider, Event::CraftingState(workbench));
    commit(&mut provider, Event::CraftingState(cleared));

    let fixture = view_fixture();
    let view = fixture.view_of(&provider);
    let parts = provider_parts(&view);
    let assembled = assemble(
        &fixture,
        &provider,
        &ClientLimits::try_new().expect("limits"),
    )
    .expect("the shared topic assembles");
    assert_eq!(assembled.len(), 3, "one record per confirmed publication");
    assert_eq!(assembled[0].view(), &InventoryUiView::Crafting(personal));
    assert_eq!(assembled[1].view(), &InventoryUiView::Crafting(workbench));
    assert_eq!(
        assembled[2].view(),
        &InventoryUiView::Crafting(cleared),
        "the latest personal publication carries the live personal contents"
    );
    for entry in parts[2].iter() {
        assert_eq!(
            *entry.stable_key(),
            inventory_key(InventoryTopic::Crafting, None),
            "every crafting record shares the one topic identity"
        );
    }
}
