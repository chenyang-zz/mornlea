//! Inventory (hotbar and backpack) projection contract tests.
//!
//! The table pins the accepted personal-inventory semantics against the
//! landed mirror provider: one confirmed inventory publication projects one
//! upsert carrying the complete nine-hotbar and twenty-seven-backpack layout
//! with the selected slot, the first and last slot values exactly as
//! published, the exact per-slot stack maximum admitted and plus-one refused
//! at the checked domain boundary, a server-rejected selection never
//! projecting as a confirmed change, no unconfirmed debit from locally
//! admitted input or a placement confirmation that carries no contents, and
//! unchanged state for late, stale and reset inputs. Order keys are the
//! actual observation keys with packet record ordinals; the inventory wire
//! carries no source tick and none is ever invented.

use mornlea_client_core::contracts::{
    ClientError, ClientLimits, ConfirmedRevision, FamilyOperation, ObservationKey, SessionEpoch,
};
use mornlea_client_core::input::{ClientIntentKind, InputProjectionState};
use mornlea_client_core::prediction::PlayerProjectionState;
use mornlea_client_core::presentation::frame::InventoryUiRecord;
use mornlea_client_core::presentation::inventory_ui::inventory::project_inventory;
use mornlea_client_core::presentation::{
    AcceptedObservation, AudioProjectionState, DiagnosticProjectionState, ErrorClassCounters,
    InventoryTopic, InventoryUiView, LifecycleProjectionState, MovementIntent, OrderedRecord, Pose,
    ProducerIdentity, ProjectionOrder, ProjectionView, QueueCounters, StableRecordKey,
};
use mornlea_client_core::session::ConfirmedMirror;
use mornlea_client_core::session::mirror::MirrorProvider;
use mornlea_domain::{
    CommandRejection, DomainError, Event, HotbarSlot, InventoryState, InventoryStateParts,
    ItemStack, PlacementSuccess, RejectReason,
};
use mornlea_protocol::{KeepAlive, ServerPacket};

/// The epoch every fixture mirror and observation key carries.
const EPOCH: u64 = 21;

/// One checked nondurable stack, the ordinary slot value of the fixtures.
fn stack(item: u16, count: u8) -> ItemStack {
    ItemStack::try_new(item, count, 0).expect("checked nondurable stack")
}

/// One checked hotbar index, enforcing the published selection range.
fn hotbar_slot(slot: u8) -> HotbarSlot {
    HotbarSlot::new(slot).expect("checked hotbar slot")
}

/// One complete inventory publication: the selected index, the first and last
/// hotbar and backpack placements named by the caller, and every other slot
/// the canonical empty stack.
fn inventory_state(
    selected: u8,
    hotbar: &[(usize, ItemStack)],
    backpack: &[(usize, ItemStack)],
) -> InventoryState {
    let mut hotbar_slots = [ItemStack::EMPTY; 9];
    for (index, value) in hotbar {
        hotbar_slots[*index] = *value;
    }
    let mut backpack_slots = [ItemStack::EMPTY; 27];
    for (index, value) in backpack {
        backpack_slots[*index] = *value;
    }
    InventoryState::new(InventoryStateParts {
        selected: hotbar_slot(selected),
        hotbar: hotbar_slots,
        backpack: backpack_slots,
    })
}

fn inventory_event(state: InventoryState) -> Event {
    Event::InventoryState(state)
}

/// One server-rejected selection action: the sequence the session issued and
/// the published refusal reason.
fn rejected_selection_event(sequence: u64, reason: RejectReason) -> Event {
    Event::CommandRejected(CommandRejection::new(sequence, reason))
}

/// One authoritative placement confirmation. It names the sequence alone and
/// carries no inventory contents, because the authority publishes contents
/// only through a complete inventory publication.
fn placement_event(sequence: u64) -> Event {
    Event::PlaceBlockSucceeded(PlacementSuccess::new(sequence))
}

fn packet(event: Event) -> ServerPacket {
    ServerPacket::try_from(event).expect("publication converts to its packet")
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
/// observation queue carries the actual source keys.
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

    /// Stages one locally admitted but unconfirmed input action, the pending
    /// state an unconfirmed debit would have to come from.
    fn admit_input(&mut self, sequence: u64, kind: ClientIntentKind) {
        self.input.record_admitted(sequence, kind);
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
    fn view_of<'a>(&'a self, provider: &'a MirrorProvider) -> ProjectionView<'a> {
        let revision = ConfirmedRevision::new(provider.mirror().revision().get() + 1);
        self.view(
            provider.mirror(),
            provider.observations(),
            provider.mirror().epoch(),
            revision,
        )
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

/// The personal-inventory payload of one record, refusing a foreign view tag:
/// this projection publishes the inventory view alone.
fn inventory_view(entry: &OrderedRecord<InventoryUiRecord>) -> InventoryState {
    match entry.record().view() {
        InventoryUiView::Inventory(state) => *state,
        other => panic!("inventory record carries a foreign view tag: {other:?}"),
    }
}

/// The shared stable-key assertion: the inventory topic tag with no container
/// or event identity, because the personal inventory is a singleton with no
/// external identity of its own.
fn assert_stable_key(entry: &OrderedRecord<InventoryUiRecord>) {
    assert_eq!(
        *entry.stable_key(),
        StableRecordKey::Inventory {
            topic: InventoryTopic::Inventory,
            identity: None,
        }
    );
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
        other => panic!("inventory record is server-sourced: {other:?}"),
    }
}

/// `first/last slot`: one confirmed publication projects exactly one upsert
/// carrying the first hotbar slot and the last backpack slot values exactly
/// as published, with the selected index, no token, no outcome and no
/// invented source tick.
#[test]
fn first_and_last_slot_project_exactly() {
    let mut provider = admitted_mirror(EPOCH);
    commit(
        &mut provider,
        inventory_event(inventory_state(
            5,
            &[(0, stack(1, 12))],
            &[(26, stack(2, 30))],
        )),
    );

    let fixture = view_fixture();
    let view = fixture.view_of(&provider);
    let records = project_inventory(&view).expect("one inventory record projects");
    assert_eq!(records.len(), 1);

    let entry = &records[0];
    let state = inventory_view(entry);
    assert_eq!(state.selected(), hotbar_slot(5));
    assert_eq!(state.hotbar()[0], stack(1, 12), "the first slot value");
    assert_eq!(state.hotbar()[8], ItemStack::EMPTY);
    assert_eq!(state.backpack()[0], ItemStack::EMPTY);
    assert_eq!(state.backpack()[26], stack(2, 30), "the last slot value");
    assert_eq!(
        entry.record().token(),
        None,
        "the personal inventory is not an external container view"
    );
    assert_eq!(
        entry.record().outcome(),
        None,
        "the serial family owner unions the outcomes"
    );
    assert_eq!(entry.record().header().operation(), FamilyOperation::Upsert);
    assert_eq!(
        entry.record().header().source_tick(),
        None,
        "the inventory wire carries no tick and none is invented"
    );
    assert_eq!(entry.record().header().epoch().get(), EPOCH);
    assert_eq!(
        entry.record().header().revision(),
        view.frame_revision(),
        "headers are rebased onto the coherent candidate revision"
    );
    assert_stable_key(entry);
    assert_confirmed_order(entry, 1, 0);
}

/// `nine hotbar / 27 backpack`: the projected view carries the complete
/// published layout — nine hotbar slots and twenty-seven backpack slots —
/// with every slot value preserved exactly, never a sparse delta.
#[test]
fn complete_nine_hotbar_and_twenty_seven_backpack_layout() {
    let mut provider = admitted_mirror(EPOCH);
    let mut expected_hotbar = [ItemStack::EMPTY; 9];
    for (index, slot) in expected_hotbar.iter_mut().enumerate() {
        *slot = stack(1, u8::try_from(index + 1).expect("hotbar count"));
    }
    let mut expected_backpack = [ItemStack::EMPTY; 27];
    for (index, slot) in expected_backpack.iter_mut().enumerate() {
        *slot = stack(2, u8::try_from(index + 1).expect("backpack count"));
    }
    let state = InventoryState::new(InventoryStateParts {
        selected: hotbar_slot(8),
        hotbar: expected_hotbar,
        backpack: expected_backpack,
    });
    commit(&mut provider, inventory_event(state));

    let fixture = view_fixture();
    let records = project_inventory(&fixture.view_of(&provider)).expect("full layout projects");
    assert_eq!(records.len(), 1);

    let projected = inventory_view(&records[0]);
    assert_eq!(projected.hotbar().len(), 9, "nine hotbar slots");
    assert_eq!(
        projected.backpack().len(),
        27,
        "twenty-seven backpack slots"
    );
    assert_eq!(projected.hotbar(), &expected_hotbar[..]);
    assert_eq!(projected.backpack(), &expected_backpack[..]);
    assert_eq!(
        projected, state,
        "the complete published value, not a delta"
    );
}

/// `full checked stack`: a stackable item admits exactly its per-slot
/// maximum, a single-count tool admits exactly one, and the plus-one counts
/// refuse at the checked domain boundary before any observation exists.
#[test]
fn full_checked_stack_admits_exact_max_and_rejects_plus_one() {
    assert_eq!(ItemStack::try_new(1, 64, 0), Ok(stack(1, 64)));
    assert_eq!(
        ItemStack::try_new(1, 65, 0),
        Err(DomainError::InvalidCount),
        "one above the stackable maximum is a typed count rejection"
    );
    assert_eq!(ItemStack::try_new(10, 1, 131), Ok(stack_tool(10, 131)));
    assert_eq!(
        ItemStack::try_new(10, 2, 131),
        Err(DomainError::InvalidCount),
        "one above a tool's single-count limit is a typed count rejection"
    );

    let mut provider = admitted_mirror(EPOCH);
    commit(
        &mut provider,
        inventory_event(inventory_state(
            0,
            &[(0, stack(1, 64)), (8, stack_tool(10, 131))],
            &[],
        )),
    );
    let fixture = view_fixture();
    let records = project_inventory(&fixture.view_of(&provider)).expect("full stacks project");
    let state = inventory_view(&records[0]);
    assert_eq!(
        state.hotbar()[0],
        stack(1, 64),
        "the exact per-slot maximum is admitted and published"
    );
    assert_eq!(
        state.hotbar()[8],
        stack_tool(10, 131),
        "the intact full-budget tool is admitted and published"
    );
}

/// One checked durable stack: a tool's count is one and its durability
/// budget is intact, the ordinary-slot rule the accepted tables publish.
fn stack_tool(item: u16, durability: u16) -> ItemStack {
    ItemStack::try_new(item, 1, durability).expect("checked durable stack")
}

/// `malformed count`: the Go oracle's invalid inventory shapes refuse at the
/// checked domain boundary — an out-of-range selection, an unregistered item
/// number, a zero count on a registered item, a nonzero count on the absent
/// item, a spent durable item — so a malformed observation never exists, and
/// a packet that is not a checked publication rejects the whole projection
/// without partial output.
#[test]
fn malformed_counts_never_become_observations() {
    assert_eq!(HotbarSlot::new(9), Err(DomainError::InvalidHotbarSlot));
    assert_eq!(
        ItemStack::try_new(4242, 1, 0),
        Err(DomainError::InvalidItem),
        "an unregistered item number has no stack rule"
    );
    assert_eq!(
        ItemStack::try_new(2, 0, 0),
        Err(DomainError::InvalidCount),
        "a registered item admits no zero count"
    );
    assert_eq!(
        ItemStack::try_new(0, 2, 0),
        Err(DomainError::InvalidCount),
        "the absent item admits no nonzero count"
    );
    assert_eq!(
        ItemStack::try_new(10, 1, 0),
        Err(DomainError::InvalidDurability),
        "a durable item at durability zero is spent, not an ordinary slot"
    );

    let epoch = SessionEpoch::try_new(EPOCH).expect("epoch");
    let fixture = view_fixture();
    let provider = admitted_mirror(EPOCH);
    let non_publication = AcceptedObservation::try_new(
        ObservationKey::try_new(epoch, ConfirmedRevision::new(1), 0).expect("key"),
        None,
        ServerPacket::KeepAlive(KeepAlive::new(1).expect("checked keep-alive token")),
        Vec::new(),
    )
    .expect("staged with a non-publication packet");
    assert_eq!(
        project_inventory(&fixture.view(
            provider.mirror(),
            &[non_publication],
            epoch,
            ConfirmedRevision::new(1)
        )),
        Err(ClientError::InvalidInput),
        "a packet that is not an event publication rejects without a record"
    );
}

/// `rejected selection` and `no unconfirmed debit`: a server-rejected
/// selection action projects no confirmed change — the last confirmed
/// contents stay exactly as published — and neither locally admitted
/// unconfirmed input nor a placement confirmation that carries no contents
/// ever debits the projected stacks; only the next complete authoritative
/// publication replaces them.
#[test]
fn rejected_selection_and_unconfirmed_input_never_change_contents() {
    let mut provider = admitted_mirror(EPOCH);
    let confirmed = inventory_state(3, &[(0, stack(1, 12))], &[(2, stack(2, 30))]);
    commit(&mut provider, inventory_event(confirmed));

    let mut fixture = view_fixture();
    fixture.admit_input(9, ClientIntentKind::SelectHotbar);
    fixture.admit_input(10, ClientIntentKind::PlaceBlock);
    let baseline = project_inventory(&fixture.view_of(&provider)).expect("baseline projects");
    assert_eq!(baseline.len(), 1);
    assert_eq!(inventory_view(&baseline[0]), confirmed);

    // The unchanged-state witness: one record for the one confirmed
    // publication, with its exact contents and observation identity. The
    // rebased candidate header alone may differ between projections, because
    // the coherent frame revision advances with the mirror.
    let unchanged = |records: &[OrderedRecord<InventoryUiRecord>], context: &str| {
        assert_eq!(records.len(), 1, "{context}");
        assert_eq!(inventory_view(&records[0]), confirmed, "{context}");
        assert_stable_key(&records[0]);
        assert_confirmed_order(&records[0], 1, 0);
    };
    unchanged(&baseline, "the baseline projects the confirmed publication");

    // The server refused the selection: the refusal is a separate
    // sequence-bound outcome, never a confirmed contents change.
    commit(
        &mut provider,
        rejected_selection_event(9, RejectReason::InvalidSlot),
    );
    let after_rejection =
        project_inventory(&fixture.view_of(&provider)).expect("projection still succeeds");
    unchanged(
        &after_rejection,
        "a rejected selection changes no projected contents",
    );

    // The placement confirms the world write alone; the authority publishes
    // contents only through a complete inventory publication, so the
    // projection keeps the confirmed stacks rather than debiting locally.
    commit(&mut provider, placement_event(10));
    let after_placement =
        project_inventory(&fixture.view_of(&provider)).expect("projection still succeeds");
    unchanged(
        &after_placement,
        "no unconfirmed debit is rendered from input or placement",
    );

    // The next complete authoritative publication replaces the contents
    // wholesale, in actual source order.
    let authoritative = inventory_state(4, &[(0, stack(1, 11))], &[(2, stack(2, 30))]);
    commit(&mut provider, inventory_event(authoritative));
    let records = project_inventory(&fixture.view_of(&provider)).expect("authoritative projects");
    assert_eq!(records.len(), 2, "one record per confirmed publication");
    assert_eq!(inventory_view(&records[0]), confirmed);
    assert_eq!(inventory_view(&records[1]), authoritative);
    assert!(records[0].order() < records[1].order());
    for entry in &records {
        assert_stable_key(entry);
        assert_eq!(entry.record().header().source_tick(), None);
    }
    assert_confirmed_order(&records[0], 1, 0);
    assert_confirmed_order(&records[1], 4, 0);
}

/// `reset`: a fresh epoch's mirror starts with an empty queue and projects no
/// inventory record, an observation from the old epoch rejects the whole
/// projection under the new frame epoch, and a queue that mixes one valid
/// observation with an old-epoch one rejects without partial output.
#[test]
fn reset_clears_projection_and_old_epoch_rejects_whole() {
    let mut provider = admitted_mirror(EPOCH);
    commit(
        &mut provider,
        inventory_event(inventory_state(0, &[(0, stack(1, 12))], &[])),
    );

    let next_epoch_value = EPOCH + 1;
    let reset_provider = admitted_mirror(next_epoch_value);
    let fixture = view_fixture();
    assert!(
        project_inventory(&fixture.view_of(&reset_provider))
            .expect("empty queue projects")
            .is_empty(),
        "a reset mirror carries no previous-session inventory"
    );

    let next_epoch = SessionEpoch::try_new(next_epoch_value).expect("next epoch");
    let old_epoch_observation = provider.observations()[0].clone();
    assert_eq!(
        project_inventory(&fixture.view(
            reset_provider.mirror(),
            &[old_epoch_observation],
            next_epoch,
            ConfirmedRevision::new(1)
        )),
        Err(ClientError::StaleEpoch),
        "an old-epoch observation never enters the new epoch's frame"
    );

    let mut fresh = admitted_mirror(next_epoch_value);
    let valid = commit(
        &mut fresh,
        inventory_event(inventory_state(1, &[(0, stack(2, 5))], &[])),
    );
    assert_eq!(valid.get(), 1);
    let stale = provider.observations()[0].clone();
    assert_ne!(
        stale.key().epoch().get(),
        next_epoch_value,
        "the stale observation belongs to the reset-away epoch"
    );
    let mixed = vec![fresh.observations()[0].clone(), stale];
    assert_eq!(
        project_inventory(&fixture.view(
            fresh.mirror(),
            &mixed,
            next_epoch,
            ConfirmedRevision::new(2)
        )),
        Err(ClientError::StaleEpoch),
        "a mixed queue rejects whole, never a partial prefix"
    );
}

/// Late and duplicate observation identities are refused by the accepted
/// mirror before this projection ever sees them: a duplicate revision key
/// and a backward key both fail without changing the committed revision, the
/// observation queue or the projected output.
#[test]
fn late_or_duplicate_inputs_leave_committed_state_unchanged() {
    let mut provider = admitted_mirror(EPOCH);
    let confirmed = inventory_state(2, &[(0, stack(1, 12))], &[(26, stack(2, 30))]);
    commit(&mut provider, inventory_event(confirmed));

    let fixture = view_fixture();
    let baseline = project_inventory(&fixture.view_of(&provider)).expect("baseline projects");
    assert_eq!(baseline.len(), 1);

    let staged = |provider: &MirrorProvider, revision: u64, event: Event| {
        AcceptedObservation::try_new(
            ObservationKey::try_new(
                provider.mirror().epoch(),
                ConfirmedRevision::new(revision),
                0,
            )
            .expect("key"),
            None,
            packet(event),
            Vec::new(),
        )
        .expect("staged")
    };
    let later = inventory_state(7, &[(1, stack(1, 1))], &[]);
    assert!(
        provider
            .commit(&staged(&provider, 1, inventory_event(later),))
            .is_err(),
        "a revision the mirror already issued is a duplicate"
    );
    assert!(
        provider
            .commit(&staged(&provider, 0, inventory_event(later),))
            .is_err(),
        "a backward revision is a malformed observation identity"
    );
    assert_eq!(provider.mirror().revision().get(), 1);
    assert_eq!(provider.observations().len(), 1);
    assert_eq!(
        project_inventory(&fixture.view_of(&provider)).expect("projection still succeeds"),
        baseline,
        "the refused inputs leave the projection unchanged"
    );
}

/// Source order across several confirmed publications follows the actual
/// observation keys alone: the records keep their arrival order with
/// strictly increasing order values, and the tickless inventory wire never
/// supplies or reconstructs an order.
#[test]
fn source_order_across_publications_never_comes_from_ticks() {
    let mut provider = admitted_mirror(EPOCH);
    for selected in 0..3u8 {
        commit(
            &mut provider,
            inventory_event(inventory_state(
                selected,
                &[(usize::from(selected), stack(1, 6))],
                &[],
            )),
        );
    }

    let fixture = view_fixture();
    let records = project_inventory(&fixture.view_of(&provider)).expect("three records project");
    assert_eq!(records.len(), 3);
    for (index, entry) in records.iter().enumerate() {
        let selected = u8::try_from(index).expect("selected index");
        assert_eq!(
            inventory_view(entry).selected(),
            hotbar_slot(selected),
            "arrival order, never a tick-reconstructed order"
        );
        assert_eq!(entry.record().header().source_tick(), None);
        assert_confirmed_order(entry, u64::try_from(index + 1).expect("revision"), 0);
    }
    assert!(
        records
            .windows(2)
            .all(|pair| pair[0].order() < pair[1].order()),
        "order follows the observation keys alone"
    );
}
