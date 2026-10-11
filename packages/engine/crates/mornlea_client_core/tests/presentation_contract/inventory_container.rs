//! Container UI projection contract tests.
//!
//! The table pins the accepted chest-container semantics against the landed
//! mirror provider, following the measured Go chest-mirror transcript: an
//! authoritative chest state publishes one complete checked view — the exact
//! twenty-seven slots and the typed kind/chunk/slot/generation reference
//! preserved, with no dimension a Rust reference could disagree with — and a
//! later republication of the same container is the move confirmation: only
//! the server's post-move contents are projected, never an unconfirmed local
//! debit, and each record's token carries its own observation's mirror
//! attribution. The token is local view attribution — epoch, reference and
//! the confirmed revision the view was staged at — and never a wire field:
//! the chest publication carries no revision, so nothing is fabricated, and
//! the rebased header revision of the coherent candidate stays distinct from
//! the token's view revision. A matching closed notification retires the
//! mirror view and publishes a remove-tagged `Closed` record whose stable
//! key is the closed topic plus the actual container identity; a stale
//! close naming an already-replaced generation or a furnace-kind close is
//! tolerated by the mirror with every store unchanged — the chest view the
//! client holds is not disturbed, exactly the Go stale-close and
//! furnace-close rows — and a late chest state at an already-replaced
//! generation is refused whole by the mirror, leaving the committed
//! revision, the observation queue and the projection exactly as they were.
//! A foreign-epoch observation rejects the whole projection with no partial
//! output, and a fresh epoch starts with no container views and attributes
//! its own epoch in its tokens. Foreign publications of other topics
//! contribute no container record, and a control packet that is not a
//! checked publication rejects the whole projection.

use mornlea_client_core::contracts::{
    ClientError, ClientLimits, ConfirmedRevision, FamilyOperation, ObservationKey, SessionEpoch,
};
use mornlea_client_core::input::{ContainerToken, InputProjectionState};
use mornlea_client_core::prediction::PlayerProjectionState;
use mornlea_client_core::presentation::frame::InventoryUiRecord;
use mornlea_client_core::presentation::inventory_ui::container::project_container;
use mornlea_client_core::presentation::{
    AcceptedObservation, AudioProjectionState, DiagnosticProjectionState, ErrorClassCounters,
    InventoryTopic, InventoryUiView, LifecycleProjectionState, MovementIntent, OrderedRecord, Pose,
    ProducerIdentity, ProjectionOrder, ProjectionView, QueueCounters, StableRecordKey, UiOutcome,
};
use mornlea_client_core::session::ConfirmedMirror;
use mornlea_client_core::session::mirror::MirrorProvider;
use mornlea_domain::{
    ChestState, ChestStateParts, ChunkPos, CombatHit, CombatTarget, ContainerClosed, ContainerKind,
    ContainerRef, Event, FurnaceState, FurnaceStateParts, HotbarSlot, Identities, InventoryState,
    InventoryStateParts, ItemStack,
};
use mornlea_protocol::{ServerHello, ServerPacket};

/// The epoch every fixture mirror and observation key carries.
const EPOCH: u64 = 7;

/// The registered stone item number, the Go `core.ItemStone` iota value.
const ITEM_STONE: u16 = 1;

/// The registered coal item number, the Go `core.ItemCoal` iota value.
const ITEM_COAL: u16 = 5;

/// The transcript chest reference: chunk(-3,7), slot 5, matching the Go
/// `testChestRef` fixture position for both container kinds.
fn container_ref(kind: ContainerKind, generation: u32) -> ContainerRef {
    ContainerRef::try_new(ChunkPos::new(-3, 7), kind, 5, generation)
        .expect("checked container reference")
}

/// One complete checked chest publication: the reference beside its
/// twenty-seven slots, all but the named entries empty.
fn chest_state(generation: u32, filled: Vec<(usize, ItemStack)>) -> ChestState {
    let mut items = [ItemStack::EMPTY; 27];
    for (index, stack) in filled {
        items[index] = stack;
    }
    ChestState::try_new(ChestStateParts {
        container: container_ref(ContainerKind::Chest, generation),
        items,
    })
    .expect("checked chest state")
}

/// One complete checked furnace publication with empty slots and idle
/// timers, used only as the other-kind view a furnace close retires.
fn idle_furnace_state(generation: u32) -> FurnaceState {
    FurnaceState::try_new(FurnaceStateParts {
        container: container_ref(ContainerKind::Furnace, generation),
        input: ItemStack::EMPTY,
        fuel: ItemStack::EMPTY,
        output: ItemStack::EMPTY,
        progress_ticks: 0,
        burn_ticks: 0,
    })
    .expect("checked idle furnace state")
}

fn stack(item: u16, count: u8) -> ItemStack {
    ItemStack::try_new(item, count, 0).expect("checked item stack")
}

fn packet(event: Event) -> ServerPacket {
    ServerPacket::try_from(event).expect("publication converts to its packet")
}

/// A transport control packet: the semantic publication set deliberately
/// excludes the control families, so this packet is never a checked
/// publication a UI provider could project.
fn hello_packet() -> ServerPacket {
    ServerPacket::ServerHello(
        ServerHello::new(Identities::current().protocol).expect("checked server hello"),
    )
}

/// An admitted real mirror provider with no observations yet.
fn admitted_mirror(epoch: u64) -> MirrorProvider {
    let epoch = SessionEpoch::try_new(epoch).expect("epoch");
    let mut provider =
        MirrorProvider::new(epoch, ClientLimits::try_new().expect("limits")).expect("mirror");
    provider.admit().expect("admitted session");
    provider
}

/// Commits one publication through the real mirror provider, so the retained
/// observation queue carries the actual source keys and the inventory store
/// carries the actual local view attribution.
fn commit(provider: &mut MirrorProvider, epoch: u64, event: Event) -> ConfirmedRevision {
    let next = provider.mirror().revision().get() + 1;
    let key = ObservationKey::try_new(
        SessionEpoch::try_new(epoch).expect("epoch"),
        ConfirmedRevision::new(next),
        0,
    )
    .expect("staged key");
    let staged =
        AcceptedObservation::try_new(key, None, packet(event), Vec::new()).expect("staged");
    provider.commit(&staged).expect("committed observation")
}

/// Stages one observation without committing it, for the malformed doubles
/// the checked publication set would never carry.
fn staged(epoch: u64, packet: ServerPacket, revision: u64) -> AcceptedObservation {
    AcceptedObservation::try_new(
        ObservationKey::try_new(
            SessionEpoch::try_new(epoch).expect("epoch"),
            ConfirmedRevision::new(revision),
            0,
        )
        .expect("staged key"),
        None,
        packet,
        Vec::new(),
    )
    .expect("staged")
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
    fn new(epoch: SessionEpoch, revision: ConfirmedRevision) -> Self {
        Self {
            input: InputProjectionState::try_new(1).expect("input projection state"),
            player: PlayerProjectionState::try_new(
                epoch,
                revision,
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

    /// Builds one immutable projection view over the supplied mirror and
    /// observation queue.
    fn view<'a>(
        &'a self,
        mirror: &'a ConfirmedMirror,
        observations: &'a [AcceptedObservation],
        epoch: SessionEpoch,
        revision: ConfirmedRevision,
    ) -> ProjectionView<'a> {
        ProjectionView::try_new(
            mirror,
            observations,
            &self.input,
            &self.player,
            &self.audio,
            &self.lifecycle,
            &self.diagnostics,
            epoch,
            revision,
            5,
            &self.limits,
        )
        .expect("projection view")
    }

    /// The candidate view of one provider's committed state: the next
    /// revision the coherent frame would carry.
    fn view_of<'a>(&'a self, provider: &'a MirrorProvider, epoch: u64) -> ProjectionView<'a> {
        let epoch = SessionEpoch::try_new(epoch).expect("epoch");
        let revision = ConfirmedRevision::new(provider.mirror().revision().get() + 1);
        self.view(provider.mirror(), provider.observations(), epoch, revision)
    }
}

/// The shared fixture owners; the per-view epoch and revision are supplied
/// when the view itself is built.
fn view_fixture() -> ViewFixture {
    ViewFixture::new(
        SessionEpoch::try_new(EPOCH).expect("epoch"),
        ConfirmedRevision::new(1),
    )
}

/// Asserts one entry is server-sourced with the exact observation identity
/// and packet record ordinal, never a reconstructed or local order.
fn assert_confirmed_order(entry: &OrderedRecord<InventoryUiRecord>, revision: u64, ordinal: u32) {
    match entry.order() {
        ProjectionOrder::Confirmed {
            observation,
            record_ordinal,
        } => {
            assert_eq!(observation.epoch().get(), EPOCH);
            assert_eq!(observation.confirmed_revision().get(), revision);
            assert_eq!(*record_ordinal, ordinal);
        }
        other => panic!("container record is server-sourced: {other:?}"),
    }
}

/// The chest view one record carries, refusing a foreign view tag: the exact
/// complete checked payload, never a rebuilt copy with invented fields.
fn chest_view(entry: &OrderedRecord<InventoryUiRecord>) -> ChestState {
    match entry.record().view() {
        InventoryUiView::Chest(state) => *state,
        other => panic!("container record carries a foreign view tag: {other:?}"),
    }
}

/// Asserts two entries name the same source record: identical view payload,
/// token attribution, stable key and source order. The rebased header
/// revision is deliberately excluded, because it legitimately advances with
/// the coherent candidate frame across projections of a growing queue; an
/// unchanged-state guarantee compares the record's source content, never the
/// frame it was rebased onto.
fn assert_same_source_record(
    left: &OrderedRecord<InventoryUiRecord>,
    right: &OrderedRecord<InventoryUiRecord>,
) {
    assert_eq!(left.record().view(), right.record().view());
    assert_eq!(left.record().token(), right.record().token());
    assert_eq!(left.record().outcome(), right.record().outcome());
    assert_eq!(left.stable_key(), right.stable_key());
    assert_eq!(left.order(), right.order());
}

/// `open`: one authoritative chest publication projects one complete view
/// record — the exact twenty-seven slots with the typed
/// kind/chunk/slot/generation reference preserved — carrying the local view
/// token: this epoch, the view's reference and the confirmed revision the
/// mirror actually staged it at. The token is mirror attribution, never a
/// wire field: the chest publication carries no revision, and the record's
/// rebased header revision stays distinct from the token's view revision.
#[test]
fn chest_open_projects_complete_view_with_local_token() {
    let mut provider = admitted_mirror(EPOCH);
    let reference = container_ref(ContainerKind::Chest, 9);
    let opened = chest_state(9, vec![(0, stack(ITEM_STONE, 5))]);
    let staged_revision = commit(&mut provider, EPOCH, Event::ChestState(opened));
    assert_eq!(staged_revision.get(), 1);
    // The mirror's local attribution of the view it staged.
    assert_eq!(
        provider.mirror().container_revision(&reference),
        Some(ConfirmedRevision::new(1))
    );

    let fixture = view_fixture();
    let view = fixture.view_of(&provider, EPOCH);
    let records = project_container(&view).expect("one chest publication projects one record");
    assert_eq!(records.len(), 1);

    let entry = &records[0];
    assert_eq!(
        chest_view(entry),
        opened,
        "the complete checked payload survives"
    );
    assert_eq!(chest_view(entry).items()[0], stack(ITEM_STONE, 5));
    assert_eq!(
        chest_view(entry).items()[1..],
        [ItemStack::EMPTY; 26],
        "every other slot stays empty"
    );
    assert_eq!(chest_view(entry).container(), reference);
    assert_eq!(chest_view(entry).container().kind(), ContainerKind::Chest);
    assert_eq!(
        chest_view(entry).container().chunk(),
        ChunkPos::new(-3, 7),
        "the chunk position survives"
    );
    assert_eq!(chest_view(entry).container().slot(), 5);
    assert_eq!(chest_view(entry).container().generation(), 9);

    let token = entry
        .record()
        .token()
        .expect("an open view carries its local attribution")
        .to_owned();
    assert_eq!(
        token,
        ContainerToken::try_new(
            SessionEpoch::try_new(EPOCH).expect("epoch"),
            reference,
            ConfirmedRevision::new(1),
        )
        .expect("checked token")
    );
    assert_eq!(token.epoch().get(), EPOCH);
    assert_eq!(token.reference(), reference);
    assert_eq!(
        token.confirmed_revision(),
        provider
            .mirror()
            .container_revision(&reference)
            .expect("the mirror still holds the view"),
        "the token is the mirror's actual local attribution, never fabricated"
    );
    assert!(
        token.confirmed_revision() != view.frame_revision(),
        "the local view revision is distinct from the coherent frame revision"
    );

    assert_eq!(entry.record().header().operation(), FamilyOperation::Upsert);
    assert_eq!(
        entry.record().header().source_tick(),
        None,
        "a chest publication carries no server tick and none is invented"
    );
    assert_eq!(entry.record().header().epoch().get(), EPOCH);
    assert_eq!(
        entry.record().header().revision(),
        view.frame_revision(),
        "headers are rebased onto the coherent candidate revision"
    );
    assert_eq!(
        entry.record().outcome(),
        None,
        "a chest publication carries no command outcome"
    );
    assert_eq!(
        *entry.stable_key(),
        StableRecordKey::Inventory {
            topic: InventoryTopic::Chest,
            identity: Some(reference),
        }
    );
    assert_confirmed_order(entry, 1, 0);
}

/// The transcript's move row: only the server's post-move republication is
/// projected — an unconfirmed local debit never publishes a record — and each
/// republication carries its own observation's authoritative contents and its
/// own token revision, in actual source order with the shared stable key of
/// the legal replace lifecycle.
#[test]
fn move_republication_projects_only_authoritative_contents() {
    let mut provider = admitted_mirror(EPOCH);
    let reference = container_ref(ContainerKind::Chest, 9);
    let opened = chest_state(9, vec![(0, stack(ITEM_STONE, 5))]);
    let moved = chest_state(9, vec![(1, stack(ITEM_COAL, 2))]);
    commit(&mut provider, EPOCH, Event::ChestState(opened));
    // The server's authoritative confirmation of the move: the stone left
    // slot 0 and the coal sits in slot 1. No client-side intermediate state
    // exists in the queue, so none can be projected.
    commit(&mut provider, EPOCH, Event::ChestState(moved));

    let fixture = view_fixture();
    let view = fixture.view_of(&provider, EPOCH);
    let records = project_container(&view).expect("two republications project two records");
    assert_eq!(records.len(), 2);

    assert_eq!(chest_view(&records[0]), opened);
    assert_eq!(chest_view(&records[1]), moved);
    assert_eq!(
        records[1]
            .record()
            .token()
            .expect("the moved view carries its own attribution")
            .confirmed_revision()
            .get(),
        2,
        "each republication's token names its own observation's revision"
    );
    assert_eq!(
        records[0]
            .record()
            .token()
            .expect("the opened view keeps its own attribution")
            .confirmed_revision()
            .get(),
        1
    );
    // The replace lifecycle keeps one typed stable key across observations;
    // the mirror holds the view exactly once at the newest attribution.
    assert_eq!(*records[0].stable_key(), *records[1].stable_key());
    assert_eq!(
        provider.mirror().container_revision(&reference),
        Some(ConfirmedRevision::new(2)),
        "the latest republication is the mirror's current attribution"
    );
    assert!(
        records
            .windows(2)
            .all(|pair| pair[0].order() < pair[1].order())
    );
    assert_confirmed_order(&records[0], 1, 0);
    assert_confirmed_order(&records[1], 2, 0);
}

/// The transcript's close row: a matching closed notification retires the
/// mirror view and publishes a remove-tagged `Closed` record after the open,
/// whose stable key is the closed topic plus the actual container identity —
/// a distinct topic tag from the chest view, so the close and the view it
/// removed never collide — and whose token is empty because a retired view
/// has no mirror attribution left to project.
#[test]
fn matching_close_retires_view_and_orders_after_open() {
    let mut provider = admitted_mirror(EPOCH);
    let reference = container_ref(ContainerKind::Chest, 9);
    let opened = chest_state(9, vec![(0, stack(ITEM_STONE, 5))]);
    commit(&mut provider, EPOCH, Event::ChestState(opened));
    commit(
        &mut provider,
        EPOCH,
        Event::ContainerClosed(ContainerClosed::new(reference)),
    );

    let fixture = view_fixture();
    let records =
        project_container(&fixture.view_of(&provider, EPOCH)).expect("the lifecycle projects");
    assert_eq!(records.len(), 2);

    let close = &records[1];
    assert_eq!(
        close.record().view(),
        &InventoryUiView::Closed(reference),
        "the closed record names the exact container identity"
    );
    assert_eq!(close.record().header().operation(), FamilyOperation::Remove);
    assert_eq!(
        close.record().token(),
        None,
        "a retired view has no mirror attribution to project"
    );
    assert_eq!(close.record().outcome(), None);
    assert_eq!(
        *close.stable_key(),
        StableRecordKey::Inventory {
            topic: InventoryTopic::Closed,
            identity: Some(reference),
        }
    );
    assert_ne!(
        *close.stable_key(),
        *records[0].stable_key(),
        "the closed topic tag keeps the removal distinct from the view"
    );
    assert_confirmed_order(close, 2, 0);
    assert!(
        records[0].order() < records[1].order(),
        "the close keeps actual source order after the open"
    );
    assert_eq!(
        provider.mirror().container_revision(&reference),
        None,
        "the matching close retired the mirror view"
    );
    assert_eq!(provider.mirror().inventory().container_count(), 0);
}

/// The transcript's stale-close and furnace-close rows through the real
/// mirror: a close naming an already-replaced generation and a close naming a
/// furnace reference commit under the accepted close tolerance without
/// disturbing the live chest view — the mirror's chest attribution is
/// unchanged and the projected chest records are exactly what they were —
/// while the furnace close retires only the furnace view, never the chest
/// one.
#[test]
fn stale_and_furnace_closes_leave_chest_state_unchanged() {
    let mut provider = admitted_mirror(EPOCH);
    let chest = container_ref(ContainerKind::Chest, 10);
    let furnace = container_ref(ContainerKind::Furnace, 9);
    let opened = chest_state(10, vec![(0, stack(ITEM_STONE, 5))]);
    commit(
        &mut provider,
        EPOCH,
        Event::FurnaceState(idle_furnace_state(9)),
    );
    commit(&mut provider, EPOCH, Event::ChestState(opened));

    let fixture = view_fixture();
    let baseline = project_container(&fixture.view_of(&provider, EPOCH))
        .expect("the two views project their chest record");
    assert_eq!(
        baseline.len(),
        1,
        "a furnace publication contributes no container record"
    );

    // The stale close names the replaced generation 9 of the chest position.
    let stale = container_ref(ContainerKind::Chest, 9);
    commit(
        &mut provider,
        EPOCH,
        Event::ContainerClosed(ContainerClosed::new(stale)),
    );
    assert_eq!(
        provider.mirror().container_revision(&chest),
        Some(ConfirmedRevision::new(2)),
        "a stale-generation close changes no store"
    );
    let after_stale = project_container(&fixture.view_of(&provider, EPOCH))
        .expect("the tolerated stale close still projects");
    assert_eq!(after_stale.len(), 2);
    assert_same_source_record(&after_stale[0], &baseline[0]);
    assert_eq!(
        after_stale[1].record().view(),
        &InventoryUiView::Closed(stale),
        "the tolerated close publishes under its own actual identity"
    );
    assert_eq!(
        *after_stale[1].stable_key(),
        StableRecordKey::Inventory {
            topic: InventoryTopic::Closed,
            identity: Some(stale),
        }
    );

    // The furnace close retires only the furnace view; the chest view and
    // its records are exactly what they were.
    commit(
        &mut provider,
        EPOCH,
        Event::ContainerClosed(ContainerClosed::new(furnace)),
    );
    assert_eq!(
        provider.mirror().container_revision(&chest),
        Some(ConfirmedRevision::new(2)),
        "a furnace-kind close never disturbs the chest view"
    );
    assert_eq!(
        provider.mirror().container_revision(&furnace),
        None,
        "the furnace close retired its own view"
    );
    let after_furnace = project_container(&fixture.view_of(&provider, EPOCH))
        .expect("the furnace close still projects");
    assert_eq!(after_furnace.len(), 3);
    assert_eq!(chest_view(&after_furnace[0]), opened);
    assert_same_source_record(&after_furnace[0], &after_stale[0]);
    assert_same_source_record(&after_furnace[1], &after_stale[1]);
    assert_eq!(
        after_furnace[2].record().view(),
        &InventoryUiView::Closed(furnace)
    );
}

/// The transcript's late-reply row through the real mirror: a chest state at
/// a generation a newer view already replaced is refused whole — the
/// committed revision, the observation queue and the projected output stay
/// exactly as they were, so no partial application and no resurrected old
/// view can exist.
#[test]
fn late_chest_state_refused_whole_leaves_projection_unchanged() {
    let mut provider = admitted_mirror(EPOCH);
    let current = chest_state(10, vec![(0, stack(ITEM_STONE, 5))]);
    commit(&mut provider, EPOCH, Event::ChestState(current));

    let fixture = view_fixture();
    let baseline =
        project_container(&fixture.view_of(&provider, EPOCH)).expect("the current view projects");
    assert_eq!(baseline.len(), 1);

    let late = packet(Event::ChestState(chest_state(
        9,
        vec![(2, stack(ITEM_COAL, 3))],
    )));
    assert!(
        provider.commit(&staged(EPOCH, late, 2)).is_err(),
        "a chest state at an already-replaced generation refuses the whole observation"
    );
    assert_eq!(provider.mirror().revision().get(), 1);
    assert_eq!(provider.observations().len(), 1);
    assert_eq!(
        provider.mirror().inventory().container_count(),
        1,
        "the refused late state applied no partial view"
    );
    assert_eq!(
        project_container(&fixture.view_of(&provider, EPOCH)).expect("projection still succeeds"),
        baseline,
        "the refused late state leaves the projection unchanged"
    );
}

/// The stale-revision row: an observation key from another epoch rejects the
/// whole projection with no partial output — even when a valid chest
/// observation precedes it — and the committed queue still projects.
#[test]
fn old_epoch_rejects_whole_projection_without_partial_output() {
    let epoch = SessionEpoch::try_new(EPOCH).expect("epoch");
    let fixture = view_fixture();
    let mut provider = admitted_mirror(EPOCH);
    let opened = chest_state(9, vec![(0, stack(ITEM_STONE, 5))]);
    commit(&mut provider, EPOCH, Event::ChestState(opened));

    let stale_epoch = AcceptedObservation::try_new(
        ObservationKey::try_new(
            SessionEpoch::try_new(EPOCH + 1).expect("next epoch"),
            ConfirmedRevision::new(2),
            0,
        )
        .expect("key"),
        None,
        packet(Event::ContainerClosed(ContainerClosed::new(container_ref(
            ContainerKind::Chest,
            9,
        )))),
        Vec::new(),
    )
    .expect("staged in another epoch");
    // The valid committed observation precedes the stale one: the rejection
    // still swallows the whole candidate, never a partial prefix.
    let mut queue: Vec<AcceptedObservation> = provider.observations().to_vec();
    queue.push(stale_epoch);
    assert_eq!(
        project_container(&fixture.view(
            provider.mirror(),
            &queue,
            epoch,
            ConfirmedRevision::new(2),
        )),
        Err(ClientError::StaleEpoch),
        "an observation from another epoch rejects without partial output"
    );

    let records = project_container(&fixture.view_of(&provider, EPOCH))
        .expect("the committed queue still projects");
    assert_eq!(records.len(), 1);
    assert_eq!(chest_view(&records[0]), opened);
}

/// The transcript's reset row: a fresh epoch's mirror starts with no
/// container views, commits and projects a chest state with its own epoch's
/// token attribution, and refuses an observation staged in the old epoch.
#[test]
fn fresh_epoch_starts_empty_and_attributes_its_own_epoch() {
    let mut previous = admitted_mirror(EPOCH);
    commit(
        &mut previous,
        EPOCH,
        Event::ChestState(chest_state(9, vec![(0, stack(ITEM_STONE, 5))])),
    );

    let fresh_epoch_value = EPOCH + 1;
    let fresh_epoch = SessionEpoch::try_new(fresh_epoch_value).expect("next epoch");
    let mut fresh = MirrorProvider::new(fresh_epoch, ClientLimits::try_new().expect("limits"))
        .expect("fresh mirror");
    fresh.admit().expect("admitted session");
    assert_eq!(
        fresh.mirror().inventory().container_count(),
        0,
        "a fresh epoch holds no container views"
    );

    let reopened = chest_state(9, vec![(3, stack(ITEM_COAL, 7))]);
    let reference = reopened.container();
    commit(&mut fresh, fresh_epoch_value, Event::ChestState(reopened));

    let fixture = view_fixture();
    let records = project_container(&fixture.view_of(&fresh, fresh_epoch_value))
        .expect("the fresh epoch projects its own view");
    assert_eq!(records.len(), 1);
    let token = records[0]
        .record()
        .token()
        .expect("the fresh view carries its attribution")
        .to_owned();
    assert_eq!(
        token.epoch(),
        fresh_epoch,
        "the token names the fresh epoch"
    );
    assert_eq!(token.reference(), reference);
    assert_eq!(
        token.confirmed_revision(),
        fresh
            .mirror()
            .container_revision(&reference)
            .expect("the fresh mirror holds the view")
    );
    assert_eq!(records[0].record().header().epoch(), fresh_epoch);

    // An observation staged in the old epoch never enters the fresh frame.
    let stale = AcceptedObservation::try_new(
        ObservationKey::try_new(
            SessionEpoch::try_new(EPOCH).expect("old epoch"),
            ConfirmedRevision::new(2),
            0,
        )
        .expect("key"),
        None,
        packet(Event::ContainerClosed(ContainerClosed::new(reference))),
        Vec::new(),
    )
    .expect("staged in the old epoch");
    let mut queue: Vec<AcceptedObservation> = fresh.observations().to_vec();
    queue.push(stale);
    assert_eq!(
        project_container(&fixture.view(
            fresh.mirror(),
            &queue,
            fresh_epoch,
            ConfirmedRevision::new(2),
        )),
        Err(ClientError::StaleEpoch),
        "an old-epoch observation never enters the fresh epoch's frame"
    );
}

/// A foreign publication of another topic contributes no container record and
/// invents no attribution, and a control packet that is not a checked
/// publication rejects the whole projection with no partial output.
#[test]
fn foreign_publications_contribute_nothing_and_control_packets_reject() {
    let mut provider = admitted_mirror(EPOCH);
    commit(
        &mut provider,
        EPOCH,
        Event::ChestState(chest_state(9, vec![(0, stack(ITEM_STONE, 5))])),
    );
    // A personal-inventory publication belongs to the inventory topic.
    commit(
        &mut provider,
        EPOCH,
        Event::InventoryState(InventoryState::new(InventoryStateParts {
            selected: HotbarSlot::new(0).expect("checked hotbar slot"),
            hotbar: [ItemStack::EMPTY; 9],
            backpack: [
                stack(ITEM_STONE, 2),
                ItemStack::EMPTY,
                ItemStack::EMPTY,
                ItemStack::EMPTY,
                ItemStack::EMPTY,
                ItemStack::EMPTY,
                ItemStack::EMPTY,
                ItemStack::EMPTY,
                ItemStack::EMPTY,
                ItemStack::EMPTY,
                ItemStack::EMPTY,
                ItemStack::EMPTY,
                ItemStack::EMPTY,
                ItemStack::EMPTY,
                ItemStack::EMPTY,
                ItemStack::EMPTY,
                ItemStack::EMPTY,
                ItemStack::EMPTY,
                ItemStack::EMPTY,
                ItemStack::EMPTY,
                ItemStack::EMPTY,
                ItemStack::EMPTY,
                ItemStack::EMPTY,
                ItemStack::EMPTY,
                ItemStack::EMPTY,
                ItemStack::EMPTY,
                ItemStack::EMPTY,
            ],
        })),
    );
    // A combat publication names a target category alone.
    commit(
        &mut provider,
        EPOCH,
        Event::CombatHit(
            CombatHit::try_new(43, 2, CombatTarget::Hostile).expect("checked combat hit"),
        ),
    );

    let fixture = view_fixture();
    let records = project_container(&fixture.view_of(&provider, EPOCH))
        .expect("foreign publications still project");
    assert_eq!(
        records.len(),
        1,
        "only the chest publication contributes a container record"
    );

    // A transport control packet is not a checked publication: no semantic
    // event exists for it, so the whole candidate rejects, never a partial
    // prefix beside the valid committed observation.
    let epoch = SessionEpoch::try_new(EPOCH).expect("epoch");
    let mut queue: Vec<AcceptedObservation> = provider.observations().to_vec();
    queue.push(staged(EPOCH, hello_packet(), 4));
    assert_eq!(
        project_container(&fixture.view(
            provider.mirror(),
            &queue,
            epoch,
            ConfirmedRevision::new(4),
        )),
        Err(ClientError::InvalidInput),
        "a control packet rejects without partial output"
    );
    assert_eq!(
        project_container(&fixture.view_of(&provider, EPOCH))
            .expect("the committed queue still projects")
            .len(),
        1
    );
}

/// The record schema's outcome union is never populated by the container
/// publications: a chest view and a closed notification carry no command
/// outcome, so no fabricated acceptance or rejection can appear on the
/// container topic.
#[test]
fn container_records_never_carry_outcomes() {
    let mut provider = admitted_mirror(EPOCH);
    let reference = container_ref(ContainerKind::Chest, 9);
    commit(
        &mut provider,
        EPOCH,
        Event::ChestState(chest_state(9, vec![(0, stack(ITEM_STONE, 5))])),
    );
    commit(
        &mut provider,
        EPOCH,
        Event::ContainerClosed(ContainerClosed::new(reference)),
    );

    let fixture = view_fixture();
    let records =
        project_container(&fixture.view_of(&provider, EPOCH)).expect("the lifecycle projects");
    assert_eq!(records.len(), 2);
    for entry in &records {
        assert_eq!(entry.record().outcome(), None);
        assert!(
            !matches!(
                entry.record().outcome(),
                Some(UiOutcome::Rejected { .. }) | Some(UiOutcome::PlacementAccepted { .. })
            ),
            "no container publication fabricates a command outcome"
        );
    }
}
