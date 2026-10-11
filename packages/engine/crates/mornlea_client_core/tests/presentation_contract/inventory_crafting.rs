//! Crafting UI projection contract tests.
//!
//! The table pins the accepted crafting semantics against the landed mirror
//! provider, following the measured Go crafting-mirror oracle: one confirmed
//! crafting publication projects one upsert carrying the complete published
//! value — the personal or workbench size, the nine grid slots with the
//! personal grid's five unused cells published empty, and the output the
//! authority derived — and a confirmed command rejection projects one separate
//! sequence-bound `Rejected` record that never mutates the confirmed view.
//! The personal grid admits exactly its four usable cells and refuses a
//! residue beyond them at the checked domain boundary; the workbench grid
//! publishes all nine. The output is server-derived: an unavailable recipe
//! publishes the empty output and the projection has no recipe table, so an
//! unavailable recipe is never fabricated into a valid one. The crafting view
//! keeps its separate token from containers — epoch, confirmed revision and
//! size through the real input admission path, never a container reference —
//! and a container publication or close never mutates the crafting
//! attribution or its projected records. A later confirmed publication
//! advances the per-size confirmed revision and stales the old crafting
//! attribution, a rejected or locally admitted pending crafting action
//! changes no projected contents, and late, stale and reset inputs leave the
//! committed state unchanged. Order keys are the actual observation keys with
//! packet record ordinals; the crafting and rejection wires carry no source
//! tick and none is ever invented.

use mornlea_client_core::contracts::{
    ClientError, ClientLimits, ConfirmedRevision, FamilyOperation, ObservationKey, SessionEpoch,
};
use mornlea_client_core::input::{
    ClientIntent, ClientIntentKind, ContainerToken, CraftingViewToken, InputAction, InputBatch,
    InputProjectionState, InputTranslator,
};
use mornlea_client_core::prediction::PlayerProjectionState;
use mornlea_client_core::presentation::frame::InventoryUiRecord;
use mornlea_client_core::presentation::inventory_ui::crafting::project_crafting;
use mornlea_client_core::presentation::{
    AcceptedObservation, AudioProjectionState, DiagnosticProjectionState, ErrorClassCounters,
    InventoryTopic, InventoryUiView, LifecycleProjectionState, MovementIntent, OrderedRecord, Pose,
    ProducerIdentity, ProjectionOrder, ProjectionView, QueueCounters, StableRecordKey, UiOutcome,
};
use mornlea_client_core::session::ConfirmedMirror;
use mornlea_client_core::session::mirror::MirrorProvider;
use mornlea_domain::{
    ChestState, ChestStateParts, ChunkPos, CommandRejection, ContainerClosed, ContainerKind,
    ContainerRef, CraftingMove, CraftingSize, CraftingState, CraftingStateParts, DomainError,
    Event, HotbarSlot, InventoryState, InventoryStateParts, ItemStack, RejectReason,
};
use mornlea_protocol::{KeepAlive, ServerPacket};

/// The epoch every fixture mirror and observation key carries.
const EPOCH: u64 = 33;

/// The registered stone item number, the Go `core.ItemStone` iota value.
const ITEM_STONE: u16 = 1;

/// The registered stone-brick item number, the Go `core.ItemStoneBrick` iota
/// value, the stocked Go crafting fixture's published output.
const ITEM_STONE_BRICK: u16 = 4;

/// The registered stick item number, the Go `core.ItemStick` iota value.
const ITEM_STICK: u16 = 37;

/// One checked nondurable stack, the ordinary slot value of the fixtures.
fn stack(item: u16, count: u8) -> ItemStack {
    ItemStack::try_new(item, count, 0).expect("checked nondurable stack")
}

/// One complete crafting publication: the grid size, the named grid cells and
/// the authority-derived output; every unnamed cell is the canonical empty
/// stack, which is exactly how the personal grid's unused cells publish.
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

/// The stocked Go crafting-mirror fixture: a workbench grid holding stones
/// and a stick with a derived stone-brick output.
fn stocked_workbench() -> CraftingState {
    crafting_state(
        CraftingSize::Workbench,
        &[(0, stack(ITEM_STONE, 2)), (3, stack(ITEM_STICK, 1))],
        stack(ITEM_STONE_BRICK, 4),
    )
}

/// The transcript chest reference: chunk(-3,7), slot 5, matching the Go
/// chest fixture position for the container publications that must never
/// disturb the crafting view.
fn container_ref(kind: ContainerKind, generation: u32) -> ContainerRef {
    ContainerRef::try_new(ChunkPos::new(-3, 7), kind, 5, generation)
        .expect("checked container reference")
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
/// observation queue carries the actual source keys and the inventory store
/// carries the actual per-size crafting attribution.
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

    /// Stages one locally admitted but unconfirmed crafting action, the
    /// pending state an unconfirmed grid change would have to come from.
    fn admit_pending(&mut self, sequence: u64, kind: ClientIntentKind) {
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
        self.view(
            provider.mirror(),
            provider.observations(),
            provider.mirror().epoch(),
            ConfirmedRevision::new(provider.mirror().revision().get() + 1),
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

/// The crafting payload of one record, refusing a foreign view tag: this
/// projection publishes the crafting and rejection views alone.
fn crafting_view(entry: &OrderedRecord<InventoryUiRecord>) -> CraftingState {
    match entry.record().view() {
        InventoryUiView::Crafting(state) => *state,
        other => panic!("crafting record carries a foreign view tag: {other:?}"),
    }
}

/// The shared stable-key assertion: the crafting topic tag with no container
/// identity, because a crafting view is a session singleton that never
/// occupies a container reference.
fn assert_crafting_stable_key(entry: &OrderedRecord<InventoryUiRecord>) {
    assert_eq!(
        *entry.stable_key(),
        StableRecordKey::Inventory {
            topic: InventoryTopic::Crafting,
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
        other => panic!("crafting record is server-sourced: {other:?}"),
    }
}

/// Asserts two entries name the same source record: identical view payload,
/// token attribution, outcome, stable key and source order. The rebased
/// header revision is deliberately excluded, because it legitimately advances
/// with the coherent candidate frame across projections of a growing queue.
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

/// `personal/workbench checked size`: one confirmed publication of either
/// grid size projects exactly one upsert carrying the complete published
/// value — the size, all nine slots and the output — with the personal grid
/// admitting exactly its four usable cells and refusing a residue beyond
/// them, and the checked stack rules refusing the Go oracle's invalid output
/// shapes before any observation exists. Both publications keep their arrival
/// order with strictly increasing source orders and no invented tick.
#[test]
fn personal_and_workbench_checked_sizes_project_exactly() {
    // The checked domain boundary: a personal grid publishes its five unused
    // cells empty, so a residue there is a typed refusal; an over-maximum or
    // unregistered output stack refuses inside the stack rule itself.
    assert_eq!(
        CraftingState::try_new(CraftingStateParts {
            size: CraftingSize::Personal,
            slots: [
                stack(ITEM_STONE, 1),
                ItemStack::EMPTY,
                ItemStack::EMPTY,
                ItemStack::EMPTY,
                stack(ITEM_STONE, 1),
                ItemStack::EMPTY,
                ItemStack::EMPTY,
                ItemStack::EMPTY,
                ItemStack::EMPTY,
            ],
            output: ItemStack::EMPTY,
        }),
        Err(DomainError::InvalidCraftingResidue),
        "the personal grid has exactly four usable cells"
    );
    assert_eq!(
        ItemStack::try_new(ITEM_STONE_BRICK, 65, 0),
        Err(DomainError::InvalidCount),
        "the Go oracle's over-maximum output refuses at the stack boundary"
    );
    assert_eq!(
        ItemStack::try_new(9999, 1, 0),
        Err(DomainError::InvalidItem),
        "the Go oracle's unregistered item refuses at the stack boundary"
    );

    let personal = crafting_state(
        CraftingSize::Personal,
        &[
            (0, stack(ITEM_STONE, 1)),
            (1, stack(ITEM_STONE, 1)),
            (2, stack(ITEM_STICK, 1)),
            (3, stack(ITEM_STICK, 1)),
        ],
        ItemStack::EMPTY,
    );
    let mut provider = admitted_mirror(EPOCH);
    commit(&mut provider, Event::CraftingState(personal));
    commit(&mut provider, Event::CraftingState(stocked_workbench()));

    let fixture = view_fixture();
    let records = project_crafting(&fixture.view_of(&provider)).expect("two grids project");
    assert_eq!(records.len(), 2, "one record per confirmed publication");

    let projected_personal = crafting_view(&records[0]);
    assert_eq!(projected_personal.size(), CraftingSize::Personal);
    assert_eq!(
        projected_personal.slots(),
        &[
            stack(ITEM_STONE, 1),
            stack(ITEM_STONE, 1),
            stack(ITEM_STICK, 1),
            stack(ITEM_STICK, 1),
            ItemStack::EMPTY,
            ItemStack::EMPTY,
            ItemStack::EMPTY,
            ItemStack::EMPTY,
            ItemStack::EMPTY,
        ][..],
        "the personal grid carries its four usable cells and publishes the rest empty"
    );
    assert_eq!(projected_personal.output(), ItemStack::EMPTY);
    assert_eq!(
        projected_personal, personal,
        "the complete value, not a delta"
    );

    let projected_workbench = crafting_view(&records[1]);
    assert_eq!(projected_workbench.size(), CraftingSize::Workbench);
    assert_eq!(projected_workbench.slots().len(), 9, "nine grid cells");
    assert_eq!(projected_workbench.slots()[0], stack(ITEM_STONE, 2));
    assert_eq!(projected_workbench.slots()[3], stack(ITEM_STICK, 1));
    assert_eq!(projected_workbench, stocked_workbench());

    let entry = &records[0];
    assert_eq!(
        entry.record().token(),
        None,
        "a crafting view never fabricates a container reference"
    );
    assert_eq!(
        entry.record().outcome(),
        None,
        "a grid publication carries no outcome"
    );
    assert_eq!(entry.record().header().operation(), FamilyOperation::Upsert);
    assert_eq!(
        entry.record().header().source_tick(),
        None,
        "the crafting wire carries no tick and none is invented"
    );
    assert_eq!(entry.record().header().epoch().get(), EPOCH);
    assert_eq!(
        entry.record().header().revision(),
        fixture.view_of(&provider).frame_revision(),
        "headers are rebased onto the coherent candidate revision"
    );
    for projected in &records {
        assert_crafting_stable_key(projected);
        assert_eq!(projected.record().header().source_tick(), None);
    }
    assert_confirmed_order(&records[0], 1, 0);
    assert_confirmed_order(&records[1], 2, 0);
    assert!(records[0].order() < records[1].order());
}

/// `valid/unavailable recipe`: the output is derived by the authority and
/// published inside the state, so the projection passes it through verbatim —
/// a valid recipe's derived output survives exactly, and an unavailable
/// recipe publishes the empty output exactly, because no recipe table exists
/// in the checked domain and none is invented here.
#[test]
fn recipe_availability_projects_exactly_as_published() {
    let mut provider = admitted_mirror(EPOCH);
    let valid = stocked_workbench();
    // The same stocked grid with the output taken: the authority publishes
    // the empty output and the projection must not second-guess it.
    let unavailable = crafting_state(
        CraftingSize::Workbench,
        &[(0, stack(ITEM_STONE, 2)), (3, stack(ITEM_STICK, 1))],
        ItemStack::EMPTY,
    );
    let idle = crafting_state(CraftingSize::Personal, &[], ItemStack::EMPTY);
    commit(&mut provider, Event::CraftingState(valid));
    commit(&mut provider, Event::CraftingState(unavailable));
    commit(&mut provider, Event::CraftingState(idle));

    let fixture = view_fixture();
    let records = project_crafting(&fixture.view_of(&provider)).expect("three grids project");
    assert_eq!(records.len(), 3);
    assert_eq!(
        crafting_view(&records[0]).output(),
        stack(ITEM_STONE_BRICK, 4)
    );
    assert_eq!(
        crafting_view(&records[1]).output(),
        ItemStack::EMPTY,
        "an unavailable recipe stays unavailable, never fabricated into valid"
    );
    assert_eq!(crafting_view(&records[2]).output(), ItemStack::EMPTY);
    assert_eq!(crafting_view(&records[0]), valid);
    assert_eq!(crafting_view(&records[1]), unavailable);
    assert_eq!(crafting_view(&records[2]), idle);
}

/// `separate sequence rejection`: a confirmed rejection of a crafting action
/// projects one separate sequence-bound `Rejected` record while the confirmed
/// crafting view stays exactly as published — the rejection's sequence never
/// advances the confirmed crafting revision, and the crafting record keeps
/// its source identity untouched. Container publications and closes never
/// mutate the crafting state either: they are ignored wholesale here and
/// leave the crafting attribution exactly as it was.
#[test]
fn rejection_projects_separate_and_never_advances_the_confirmed_view() {
    let mut provider = admitted_mirror(EPOCH);
    let confirmed = stocked_workbench();
    commit(&mut provider, Event::CraftingState(confirmed));
    assert_eq!(
        provider.mirror().crafting_revision(CraftingSize::Workbench),
        Some(ConfirmedRevision::new(1))
    );

    let fixture = view_fixture();
    let baseline = project_crafting(&fixture.view_of(&provider)).expect("baseline projects");
    assert_eq!(baseline.len(), 1);

    // The server refused the crafting action at its own sequence: the Go
    // grid script's refused take-output shape.
    commit(
        &mut provider,
        Event::CommandRejected(CommandRejection::new(7, RejectReason::InvalidInput)),
    );
    assert_eq!(
        provider.mirror().crafting_revision(CraftingSize::Workbench),
        Some(ConfirmedRevision::new(1)),
        "a rejected crafting action's sequence never advances the confirmed view"
    );

    let after = project_crafting(&fixture.view_of(&provider)).expect("projection still succeeds");
    assert_eq!(after.len(), 2, "the rejection is one separate record");
    assert_same_source_record(&after[0], &baseline[0]);
    assert_eq!(crafting_view(&after[0]), confirmed);
    assert_crafting_stable_key(&after[0]);

    let rejection = &after[1];
    assert_eq!(
        rejection.record().view(),
        &InventoryUiView::Rejected(CommandRejection::new(7, RejectReason::InvalidInput)),
        "the rejection carries its exact checked payload"
    );
    assert_eq!(
        rejection.record().outcome(),
        Some(&UiOutcome::Rejected {
            sequence: 7,
            reason: RejectReason::InvalidInput,
        }),
        "the outcome keeps the refused action's actual sequence"
    );
    assert_eq!(rejection.record().token(), None);
    assert_eq!(
        rejection.record().header().operation(),
        FamilyOperation::Upsert
    );
    assert_eq!(rejection.record().header().source_tick(), None);
    assert_eq!(
        *rejection.stable_key(),
        StableRecordKey::Inventory {
            topic: InventoryTopic::Rejected,
            identity: None,
        }
    );
    assert_confirmed_order(rejection, 2, 0);

    // Container publications and closes are other topics' inputs: no crafting
    // record, no crafting attribution and no crafting revision changes.
    let reference = container_ref(ContainerKind::Chest, 9);
    let chest = ChestState::try_new(ChestStateParts {
        container: reference,
        items: [ItemStack::EMPTY; 27],
    })
    .expect("checked chest state");
    commit(&mut provider, Event::ChestState(chest));
    commit(
        &mut provider,
        Event::ContainerClosed(ContainerClosed::new(reference)),
    );
    assert_eq!(
        provider.mirror().crafting_revision(CraftingSize::Workbench),
        Some(ConfirmedRevision::new(1)),
        "a container-token event never mutates the crafting attribution"
    );
    let after_containers =
        project_crafting(&fixture.view_of(&provider)).expect("projection still succeeds");
    assert_eq!(
        after_containers.len(),
        2,
        "container events are ignored wholesale"
    );
    assert_same_source_record(&after_containers[0], &baseline[0]);
    assert_same_source_record(&after_containers[1], &after[1]);
}

/// `separate token`: the crafting view's local attribution is the separate
/// crafting token of the accepted input rules — epoch, confirmed revision and
/// size — verified here through the real admission validator: the current
/// token passes, a stale revision refuses, a container token on a crafting
/// intent refuses because it would fabricate a workbench reference, and the
/// container token path beside it never disturbs the crafting attribution.
#[test]
fn crafting_token_stays_separate_from_container_tokens() {
    let mut provider = admitted_mirror(EPOCH);
    let personal = crafting_state(
        CraftingSize::Personal,
        &[(0, stack(ITEM_STONE, 1))],
        ItemStack::EMPTY,
    );
    commit(&mut provider, Event::CraftingState(personal));
    let epoch = provider.mirror().epoch();
    let limits = ClientLimits::try_new().expect("limits");
    let move_crafting = || InputAction {
        intent: ClientIntent::MoveCrafting(
            CraftingMove::try_new(9, 0).expect("checked crafting move"),
        ),
        container: None,
        crafting: None,
    };

    let current =
        CraftingViewToken::try_new(epoch, ConfirmedRevision::new(1), CraftingSize::Personal)
            .expect("checked crafting token");
    let mut action = move_crafting();
    action.crafting = Some(current);
    let batch = InputBatch::try_new(epoch, vec![action]).expect("batch");
    assert!(
        InputTranslator::validate_batch(&batch, provider.mirror(), &limits).is_ok(),
        "the current crafting attribution admits the crafting intent"
    );

    let mut stale = move_crafting();
    stale.crafting = Some(
        CraftingViewToken::try_new(epoch, ConfirmedRevision::new(0), CraftingSize::Personal)
            .expect("checked crafting token"),
    );
    let batch = InputBatch::try_new(epoch, vec![stale]).expect("batch");
    assert_eq!(
        InputTranslator::validate_batch(&batch, provider.mirror(), &limits),
        Err(ClientError::StaleEpoch),
        "a stale crafting revision never admits a crafting intent"
    );

    let reference = container_ref(ContainerKind::Chest, 9);
    let mut wrong_kind = move_crafting();
    wrong_kind.container = Some(
        ContainerToken::try_new(epoch, reference, ConfirmedRevision::new(1))
            .expect("checked container token"),
    );
    let batch = InputBatch::try_new(epoch, vec![wrong_kind]).expect("batch");
    assert_eq!(
        InputTranslator::validate_batch(&batch, provider.mirror(), &limits),
        Err(ClientError::InvalidInput),
        "a crafting intent with a container token fabricates a workbench reference"
    );

    // The container token path lives beside the crafting path without ever
    // intersecting it: the chest stages its own attribution, a container
    // close names it, and the crafting attribution is untouched throughout.
    let chest = ChestState::try_new(ChestStateParts {
        container: reference,
        items: [ItemStack::EMPTY; 27],
    })
    .expect("checked chest state");
    commit(&mut provider, Event::ChestState(chest));
    assert_eq!(
        provider.mirror().container_revision(&reference),
        Some(ConfirmedRevision::new(2))
    );
    assert_eq!(
        provider.mirror().crafting_revision(CraftingSize::Personal),
        Some(ConfirmedRevision::new(1)),
        "staging a container view changes no crafting attribution"
    );
    let close = InputAction {
        intent: ClientIntent::CloseContainer,
        container: Some(
            ContainerToken::try_new(epoch, reference, ConfirmedRevision::new(2))
                .expect("checked container token"),
        ),
        crafting: None,
    };
    let batch = InputBatch::try_new(epoch, vec![close]).expect("batch");
    assert!(
        InputTranslator::validate_batch(&batch, provider.mirror(), &limits).is_ok(),
        "the separate container token path keeps working beside the crafting path"
    );
    assert_eq!(
        provider.mirror().crafting_revision(CraftingSize::Personal),
        Some(ConfirmedRevision::new(1))
    );

    // The projection agrees: through the container staging and the container
    // close path, the one confirmed crafting record stays exactly as
    // published, with no second record drawn from the container events.
    let fixture = view_fixture();
    let records = project_crafting(&fixture.view_of(&provider)).expect("one crafting record");
    assert_eq!(records.len(), 1);
    assert_eq!(crafting_view(&records[0]), personal);
    assert_crafting_stable_key(&records[0]);
    assert_confirmed_order(&records[0], 1, 0);
}

/// `revision change`: a later confirmed publication of the same size advances
/// that size's confirmed revision — staling the old crafting attribution for
/// the input path — while the other size's attribution and every already
/// projected record stay exactly as they were, in actual source order.
#[test]
fn later_publication_advances_the_confirmed_revision_and_stales_old_tokens() {
    let mut provider = admitted_mirror(EPOCH);
    let personal = crafting_state(
        CraftingSize::Personal,
        &[(0, stack(ITEM_STONE, 1))],
        ItemStack::EMPTY,
    );
    let workbench = stocked_workbench();
    let cleared = crafting_state(CraftingSize::Personal, &[], ItemStack::EMPTY);
    commit(&mut provider, Event::CraftingState(personal));
    commit(&mut provider, Event::CraftingState(workbench));
    assert_eq!(
        provider.mirror().crafting_revision(CraftingSize::Personal),
        Some(ConfirmedRevision::new(1))
    );
    assert_eq!(
        provider.mirror().crafting_revision(CraftingSize::Workbench),
        Some(ConfirmedRevision::new(2)),
        "each grid size carries its own confirmed attribution"
    );

    let epoch = provider.mirror().epoch();
    let limits = ClientLimits::try_new().expect("limits");
    let action_with = |token: CraftingViewToken| {
        let mut action = InputAction {
            intent: ClientIntent::MoveCrafting(
                CraftingMove::try_new(9, 0).expect("checked crafting move"),
            ),
            container: None,
            crafting: None,
        };
        action.crafting = Some(token);
        InputBatch::try_new(epoch, vec![action]).expect("batch")
    };
    let old_token =
        CraftingViewToken::try_new(epoch, ConfirmedRevision::new(1), CraftingSize::Personal)
            .expect("checked crafting token");
    assert!(
        InputTranslator::validate_batch(&action_with(old_token), provider.mirror(), &limits)
            .is_ok()
    );

    commit(&mut provider, Event::CraftingState(cleared));
    assert_eq!(
        provider.mirror().crafting_revision(CraftingSize::Personal),
        Some(ConfirmedRevision::new(3)),
        "the later publication advances the personal view's revision"
    );
    assert_eq!(
        provider.mirror().crafting_revision(CraftingSize::Workbench),
        Some(ConfirmedRevision::new(2)),
        "the other size's attribution is untouched"
    );
    assert_eq!(
        InputTranslator::validate_batch(&action_with(old_token), provider.mirror(), &limits),
        Err(ClientError::StaleEpoch),
        "the superseded crafting attribution is stale after the revision change"
    );
    let new_token =
        CraftingViewToken::try_new(epoch, ConfirmedRevision::new(3), CraftingSize::Personal)
            .expect("checked crafting token");
    assert!(
        InputTranslator::validate_batch(&action_with(new_token), provider.mirror(), &limits)
            .is_ok(),
        "the current crafting attribution still admits the intent"
    );

    let fixture = view_fixture();
    let records = project_crafting(&fixture.view_of(&provider)).expect("three records project");
    assert_eq!(records.len(), 3);
    assert_eq!(crafting_view(&records[0]), personal);
    assert_eq!(crafting_view(&records[1]), workbench);
    assert_eq!(
        crafting_view(&records[2]),
        cleared,
        "the latest publication per size is the live one; earlier records keep their source order"
    );
    assert!(
        records
            .windows(2)
            .all(|pair| pair[0].order() < pair[1].order()),
        "order follows the observation keys alone"
    );
}

/// `pending action not consumed`: locally admitted unconfirmed crafting
/// actions debit nothing — the projected grid keeps the confirmed contents
/// through the pending state, through a refusal of one pending sequence and
/// through an inventory publication, and only the next complete
/// authoritative crafting publication replaces it.
#[test]
fn pending_local_crafting_actions_never_debit_the_projection() {
    let mut provider = admitted_mirror(EPOCH);
    let confirmed = stocked_workbench();
    commit(&mut provider, Event::CraftingState(confirmed));

    let mut fixture = view_fixture();
    fixture.admit_pending(9, ClientIntentKind::MoveCrafting);
    fixture.admit_pending(10, ClientIntentKind::TakeCraftingOutput);
    let baseline = project_crafting(&fixture.view_of(&provider)).expect("baseline projects");
    assert_eq!(baseline.len(), 1);
    assert_eq!(crafting_view(&baseline[0]), confirmed);

    let unchanged = |records: &[OrderedRecord<InventoryUiRecord>], context: &str| {
        assert_eq!(records.len(), 1, "{context}");
        assert_eq!(crafting_view(&records[0]), confirmed, "{context}");
        assert_crafting_stable_key(&records[0]);
        assert_confirmed_order(&records[0], 1, 0);
    };
    unchanged(
        &project_crafting(&fixture.view_of(&provider)).expect("pending input projects"),
        "a pending local crafting action leaves the confirmed projection untouched",
    );

    // The server refused one pending sequence: the crafting record keeps its
    // confirmed contents and source identity, and the refusal is exactly the
    // one separate sequence-bound outcome record — never a second grid.
    commit(
        &mut provider,
        Event::CommandRejected(CommandRejection::new(9, RejectReason::InvalidInput)),
    );
    let after_refusal =
        project_crafting(&fixture.view_of(&provider)).expect("projection still succeeds");
    assert_eq!(after_refusal.len(), 2, "the refusal is one separate record");
    assert_same_source_record(&after_refusal[0], &baseline[0]);
    assert_eq!(
        after_refusal[1].record().outcome(),
        Some(&UiOutcome::Rejected {
            sequence: 9,
            reason: RejectReason::InvalidInput,
        }),
        "a refused crafting action changes no projected contents"
    );

    // An inventory publication is another topic's input: the crafting grid
    // keeps the confirmed contents and the record set stays exactly the
    // confirmed grid beside its refusal.
    let mut hotbar = [ItemStack::EMPTY; 9];
    hotbar[0] = stack(ITEM_STONE, 3);
    commit(
        &mut provider,
        Event::InventoryState(InventoryState::new(InventoryStateParts {
            selected: HotbarSlot::new(0).expect("checked hotbar slot"),
            hotbar,
            backpack: [ItemStack::EMPTY; 27],
        })),
    );
    let after_inventory =
        project_crafting(&fixture.view_of(&provider)).expect("projection still succeeds");
    assert_eq!(
        after_inventory.len(),
        2,
        "an inventory publication changes no projected crafting contents"
    );
    assert_same_source_record(&after_inventory[0], &baseline[0]);
    assert_same_source_record(&after_inventory[1], &after_refusal[1]);

    // Only the next complete authoritative crafting publication replaces the
    // projected grid.
    let taken = crafting_state(
        CraftingSize::Workbench,
        &[(0, stack(ITEM_STONE, 2))],
        ItemStack::EMPTY,
    );
    commit(&mut provider, Event::CraftingState(taken));
    let records = project_crafting(&fixture.view_of(&provider)).expect("authoritative projects");
    assert_eq!(
        records.len(),
        3,
        "one grid record per confirmed publication beside the refusal"
    );
    assert_eq!(crafting_view(&records[0]), confirmed);
    assert_eq!(crafting_view(&records[2]), taken);
    assert_same_source_record(&records[0], &baseline[0]);
}

/// `reset`: a fresh epoch's mirror starts with an empty queue and projects no
/// crafting record, an observation from the old epoch rejects the whole
/// projection under the new frame epoch, and a queue that mixes one valid
/// observation with an old-epoch one rejects without partial output.
#[test]
fn reset_clears_projection_and_old_epoch_rejects_whole() {
    let mut provider = admitted_mirror(EPOCH);
    commit(&mut provider, Event::CraftingState(stocked_workbench()));

    let next_epoch_value = EPOCH + 1;
    let reset_provider = admitted_mirror(next_epoch_value);
    let fixture = view_fixture();
    assert!(
        project_crafting(&fixture.view_of(&reset_provider))
            .expect("empty queue projects")
            .is_empty(),
        "a reset mirror carries no previous-session crafting view"
    );

    let next_epoch = SessionEpoch::try_new(next_epoch_value).expect("next epoch");
    let old_epoch_observation = provider.observations()[0].clone();
    assert_eq!(
        project_crafting(&fixture.view(
            reset_provider.mirror(),
            &[old_epoch_observation],
            next_epoch,
            ConfirmedRevision::new(1)
        )),
        Err(ClientError::StaleEpoch),
        "an old-epoch observation never enters the new epoch's frame"
    );

    let mut fresh = admitted_mirror(next_epoch_value);
    let personal = crafting_state(
        CraftingSize::Personal,
        &[(0, stack(ITEM_STONE, 1))],
        ItemStack::EMPTY,
    );
    let valid = commit(&mut fresh, Event::CraftingState(personal));
    assert_eq!(valid.get(), 1);
    let stale = provider.observations()[0].clone();
    assert_ne!(
        stale.key().epoch().get(),
        next_epoch_value,
        "the stale observation belongs to the reset-away epoch"
    );
    let mixed = vec![fresh.observations()[0].clone(), stale];
    assert_eq!(
        project_crafting(&fixture.view(
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
    let confirmed = stocked_workbench();
    commit(&mut provider, Event::CraftingState(confirmed));

    let fixture = view_fixture();
    let baseline = project_crafting(&fixture.view_of(&provider)).expect("baseline projects");
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
    let later = crafting_state(
        CraftingSize::Personal,
        &[(1, stack(ITEM_STICK, 2))],
        ItemStack::EMPTY,
    );
    assert!(
        provider
            .commit(&staged(&provider, 1, Event::CraftingState(later)))
            .is_err(),
        "a revision the mirror already issued is a duplicate"
    );
    assert!(
        provider
            .commit(&staged(&provider, 0, Event::CraftingState(later)))
            .is_err(),
        "a backward revision is a malformed observation identity"
    );
    assert_eq!(provider.mirror().revision().get(), 1);
    assert_eq!(provider.observations().len(), 1);
    assert_eq!(
        project_crafting(&fixture.view_of(&provider)).expect("projection still succeeds"),
        baseline,
        "the refused inputs leave the projection unchanged"
    );
}

/// A packet that is not a checked publication rejects the whole projection
/// before any record is returned, so no partial output exists.
#[test]
fn non_publication_packet_rejects_whole_projection() {
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
        project_crafting(&fixture.view(
            provider.mirror(),
            &[non_publication],
            epoch,
            ConfirmedRevision::new(1)
        )),
        Err(ClientError::InvalidInput),
        "a packet that is not an event publication rejects without a record"
    );
}
