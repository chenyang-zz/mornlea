//! Furnace UI projection contract tests.
//!
//! The table pins the accepted furnace semantics against the landed mirror
//! provider, following the measured Go furnace-mirror oracle: an
//! authoritative furnace state publishes one complete checked view — the
//! exact input, fuel and output slots beside the typed kind/chunk/slot/
//! generation reference and the two authoritative timers, preserved verbatim
//! with no rebuilt copy and no invented field — and the fixed smelting
//! materials raw iron to iron ingot, sand to glass and clay to brick all
//! project their complete states. The checked tick bounds are the packet 05
//! rows: the progress admits strictly below 200 and the burn time up to 1600
//! inclusive, so a plus-one at either bound rejects typed before any state
//! can reach the observation queue, and a wire packet mutated past a bound
//! rejects the whole projection with no partial output. The completion
//! transition is exact: a swing that reaches the requirement is finished, so
//! the authority republishes with the progress reset and the output updated,
//! and each publication projects whole — never a partial record mixing the
//! old output with the new progress. Exhausted fuel extinguishes the burn
//! per the schema: an empty fuel slot beside a zero burn time publishes
//! exactly that state, and no timer-versus-stack relation is invented. A
//! republication at a new generation is the replace lifecycle — the mirror
//! retires the old view, the records keep actual source order and the stable
//! key stays the furnace topic tag plus the actual furnace identity, never
//! the absent source tick. A matching close retires the mirror view while
//! contributing no furnace record, because the closed topic belongs to the
//! container provider; a stale close and a late state at an
//! already-replaced generation are refused or tolerated by the real mirror
//! with the committed revision, the observation queue and the projected
//! records exactly as they were. A foreign-epoch observation rejects the
//! whole projection with no partial output, a fresh epoch starts with no
//! furnace views and attributes its own epoch in its tokens, foreign
//! publications of other topics contribute no furnace record, and a control
//! packet that is not a checked publication rejects the whole projection.

use mornlea_client_core::contracts::{
    ClientError, ClientLimits, ConfirmedRevision, FamilyOperation, ObservationKey, SessionEpoch,
};
use mornlea_client_core::input::{ContainerToken, InputProjectionState};
use mornlea_client_core::prediction::PlayerProjectionState;
use mornlea_client_core::presentation::frame::InventoryUiRecord;
use mornlea_client_core::presentation::inventory_ui::furnace::project_furnace;
use mornlea_client_core::presentation::{
    AcceptedObservation, AudioProjectionState, DiagnosticProjectionState, ErrorClassCounters,
    InventoryTopic, InventoryUiView, LifecycleProjectionState, MovementIntent, OrderedRecord, Pose,
    ProducerIdentity, ProjectionOrder, ProjectionView, QueueCounters, StableRecordKey, UiOutcome,
};
use mornlea_client_core::session::ConfirmedMirror;
use mornlea_client_core::session::mirror::MirrorProvider;
use mornlea_domain::{
    ChunkPos, CombatHit, CombatTarget, ContainerClosed, ContainerKind, ContainerRef, DomainError,
    Event, FurnaceState, FurnaceStateParts, HotbarSlot, Identities, InventoryState,
    InventoryStateParts, ItemStack,
};
use mornlea_protocol::{ServerHello, ServerPacket};

/// The epoch every fixture mirror and observation key carries.
const EPOCH: u64 = 7;

/// The registered raw iron item number, the Go `core.ItemRawIron` iota value.
const ITEM_RAW_IRON: u16 = 6;

/// The registered coal item number, the Go `core.ItemCoal` iota value and the
/// one fuel a furnace admits beside the empty stack.
const ITEM_COAL: u16 = 5;

/// The registered iron ingot item number, the Go `core.ItemIronIngot` value.
const ITEM_IRON_INGOT: u16 = 7;

/// The registered sand item number, the Go `core.ItemSand` iota value.
const ITEM_SAND: u16 = 18;

/// The registered glass item number, the Go `core.ItemGlass` iota value.
const ITEM_GLASS: u16 = 23;

/// The registered clay item number, the Go `core.ItemClay` iota value.
const ITEM_CLAY: u16 = 27;

/// The registered brick item number, the Go `core.ItemBrick` iota value.
const ITEM_BRICK: u16 = 24;

/// The registered stone item number, the Go `core.ItemStone` iota value.
const ITEM_STONE: u16 = 1;

/// The transcript furnace reference: chunk(-3,7), slot 5, matching the Go
/// `testFurnaceRef` fixture position.
fn furnace_ref(generation: u32) -> ContainerRef {
    ContainerRef::try_new(ChunkPos::new(-3, 7), ContainerKind::Furnace, 5, generation)
        .expect("checked furnace reference")
}

fn stack(item: u16, count: u8) -> ItemStack {
    ItemStack::try_new(item, count, 0).expect("checked item stack")
}

/// One complete checked furnace publication: the reference beside its three
/// slots and two timers.
fn furnace_state(
    generation: u32,
    input: ItemStack,
    fuel: ItemStack,
    output: ItemStack,
    progress_ticks: u8,
    burn_ticks: u16,
) -> FurnaceState {
    FurnaceState::try_new(FurnaceStateParts {
        container: furnace_ref(generation),
        input,
        fuel,
        output,
        progress_ticks,
        burn_ticks,
    })
    .expect("checked furnace state")
}

/// The transcript furnace state from the Go oracle fixture: raw iron input,
/// coal fuel, iron ingot output, progress 137 and burn 1463.
fn transcript_state(generation: u32) -> FurnaceState {
    furnace_state(
        generation,
        stack(ITEM_RAW_IRON, 7),
        stack(ITEM_COAL, 2),
        stack(ITEM_IRON_INGOT, 5),
        137,
        1463,
    )
}

fn packet(event: Event) -> ServerPacket {
    ServerPacket::try_from(event).expect("publication converts to its packet")
}

/// A wire furnace packet whose timers were mutated past a bound, the double
/// only a deliberately corrupted record can produce: the checked domain
/// constructors would refuse the same values, so the malformed observation is
/// staged without ever being committed.
fn timer_mutated_packet(progress_ticks: u8, burn_ticks: u16) -> ServerPacket {
    let mut record = mornlea_protocol::FurnaceState::new(
        mornlea_protocol::ContainerRef {
            dimension: 0,
            chunk_x: -3,
            chunk_z: 7,
            kind: mornlea_protocol::CONTAINER_KIND_FURNACE,
            slot: 5,
            generation: 9,
        },
        stack(ITEM_RAW_IRON, 7),
        stack(ITEM_COAL, 2),
        stack(ITEM_IRON_INGOT, 5),
        137,
        1463,
    )
    .expect("checked wire record before the mutation");
    record.progress_ticks = progress_ticks;
    record.burn_ticks = burn_ticks;
    ServerPacket::FurnaceState(record)
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
        other => panic!("furnace record is server-sourced: {other:?}"),
    }
}

/// The furnace view one record carries, refusing a foreign view tag: the
/// exact complete checked payload, never a rebuilt copy with invented fields.
fn furnace_view(entry: &OrderedRecord<InventoryUiRecord>) -> FurnaceState {
    match entry.record().view() {
        InventoryUiView::Furnace(state) => *state,
        other => panic!("furnace record carries a foreign view tag: {other:?}"),
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

/// The open row: one authoritative furnace publication projects one complete
/// view record — the exact input, fuel and output slots, the typed
/// kind/chunk/slot/generation reference and the two timers preserved —
/// carrying the local view token: this epoch, the view's reference and the
/// confirmed revision the mirror actually staged it at. The token is mirror
/// attribution, never a wire field: the furnace publication carries no
/// revision, and the record's rebased header revision stays distinct from
/// the token's view revision. The stable key is the furnace topic tag plus
/// the actual furnace identity, and no command outcome is fabricated.
#[test]
fn furnace_open_projects_complete_state_with_local_token() {
    let mut provider = admitted_mirror(EPOCH);
    let reference = furnace_ref(9);
    let opened = transcript_state(9);
    let staged_revision = commit(&mut provider, EPOCH, Event::FurnaceState(opened));
    assert_eq!(staged_revision.get(), 1);
    // The mirror's local attribution of the view it staged.
    assert_eq!(
        provider.mirror().container_revision(&reference),
        Some(ConfirmedRevision::new(1))
    );

    let fixture = view_fixture();
    let view = fixture.view_of(&provider, EPOCH);
    let records = project_furnace(&view).expect("one furnace publication projects one record");
    assert_eq!(records.len(), 1);

    let entry = &records[0];
    assert_eq!(
        furnace_view(entry),
        opened,
        "the complete checked payload survives"
    );
    assert_eq!(furnace_view(entry).input(), stack(ITEM_RAW_IRON, 7));
    assert_eq!(furnace_view(entry).fuel(), stack(ITEM_COAL, 2));
    assert_eq!(furnace_view(entry).output(), stack(ITEM_IRON_INGOT, 5));
    assert_eq!(furnace_view(entry).progress_ticks(), 137);
    assert_eq!(furnace_view(entry).burn_ticks(), 1463);
    assert_eq!(furnace_view(entry).container(), reference);
    assert_eq!(
        furnace_view(entry).container().kind(),
        ContainerKind::Furnace
    );
    assert_eq!(
        furnace_view(entry).container().chunk(),
        ChunkPos::new(-3, 7),
        "the chunk position survives"
    );
    assert_eq!(furnace_view(entry).container().slot(), 5);
    assert_eq!(furnace_view(entry).container().generation(), 9);

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
        "a furnace publication carries no server tick and none is invented"
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
        "a furnace publication carries no command outcome"
    );
    assert!(
        !matches!(
            entry.record().outcome(),
            Some(UiOutcome::Rejected { .. }) | Some(UiOutcome::PlacementAccepted { .. })
        ),
        "no furnace publication fabricates a command outcome"
    );
    assert_eq!(
        *entry.stable_key(),
        StableRecordKey::Inventory {
            topic: InventoryTopic::Furnace,
            identity: Some(reference),
        }
    );
    assert_confirmed_order(entry, 1, 0);
}

/// The material-states row of the Go oracle: every fixed smelting material —
/// raw iron to iron ingot, sand to glass, clay to brick — projects its
/// complete checked state verbatim, beside the one admitted fuel.
#[test]
fn furnace_material_states_project_verbatim() {
    let materials = [
        (
            stack(ITEM_RAW_IRON, 7),
            stack(ITEM_IRON_INGOT, 5),
            "raw iron smelts to an iron ingot",
        ),
        (
            stack(ITEM_SAND, 6),
            stack(ITEM_GLASS, 4),
            "sand smelts to glass",
        ),
        (
            stack(ITEM_CLAY, 3),
            stack(ITEM_BRICK, 2),
            "clay smelts to brick",
        ),
    ];
    for (input, output, label) in materials {
        let mut provider = admitted_mirror(EPOCH);
        let published = furnace_state(9, input, stack(ITEM_COAL, 2), output, 137, 1463);
        commit(&mut provider, EPOCH, Event::FurnaceState(published));

        let fixture = view_fixture();
        let records = project_furnace(&fixture.view_of(&provider, EPOCH)).expect(label);
        assert_eq!(records.len(), 1, "{label}");
        assert_eq!(furnace_view(&records[0]), published, "{label}");
        assert_eq!(furnace_view(&records[0]).input(), input, "{label}");
        assert_eq!(furnace_view(&records[0]).output(), output, "{label}");
        assert_eq!(
            furnace_view(&records[0]).fuel(),
            stack(ITEM_COAL, 2),
            "{label}"
        );
    }
}

/// The checked tick bounds row: the progress admits strictly below 200 and
/// the burn time up to 1600 inclusive, so the extreme checked values project
/// while a plus-one at either bound rejects typed at the domain constructor —
/// no such state can reach the observation queue — and a wire packet mutated
/// past a bound rejects the whole projection with no partial output.
#[test]
fn checked_timer_bounds_admit_extremes_reject_plus_one() {
    // The admitted extremes: one tick below the smelt requirement and a full
    // burn tank, the inclusive maximum.
    let mut provider = admitted_mirror(EPOCH);
    let extremes = furnace_state(
        9,
        stack(ITEM_RAW_IRON, 7),
        stack(ITEM_COAL, 2),
        stack(ITEM_IRON_INGOT, 5),
        199,
        1600,
    );
    commit(&mut provider, EPOCH, Event::FurnaceState(extremes));

    let fixture = view_fixture();
    let records = project_furnace(&fixture.view_of(&provider, EPOCH))
        .expect("the admitted timer extremes project");
    assert_eq!(records.len(), 1);
    assert_eq!(furnace_view(&records[0]), extremes);
    assert_eq!(furnace_view(&records[0]).progress_ticks(), 199);
    assert_eq!(furnace_view(&records[0]).burn_ticks(), 1600);

    // The plus-one rejections: a progress at the requirement is finished, and
    // a burn time above the full tank is out of range; both reject typed.
    let parts = |progress_ticks: u8, burn_ticks: u16| FurnaceStateParts {
        container: furnace_ref(9),
        input: stack(ITEM_RAW_IRON, 7),
        fuel: stack(ITEM_COAL, 2),
        output: stack(ITEM_IRON_INGOT, 5),
        progress_ticks,
        burn_ticks,
    };
    assert_eq!(
        FurnaceState::try_new(parts(200, 1463)),
        Err(DomainError::InvalidFurnaceTimers),
        "a progress at the smelt requirement rejects typed"
    );
    assert_eq!(
        FurnaceState::try_new(parts(137, 1601)),
        Err(DomainError::InvalidFurnaceTimers),
        "a burn time above the full tank rejects typed"
    );

    // A wire packet mutated past a bound is not a checked publication: the
    // whole candidate rejects, never a partial prefix beside the valid
    // committed observation.
    for mutated in [
        timer_mutated_packet(200, 1463),
        timer_mutated_packet(137, 1601),
    ] {
        let epoch = SessionEpoch::try_new(EPOCH).expect("epoch");
        let mut queue: Vec<AcceptedObservation> = provider.observations().to_vec();
        queue.push(staged(EPOCH, mutated, 2));
        assert_eq!(
            project_furnace(&fixture.view(
                provider.mirror(),
                &queue,
                epoch,
                ConfirmedRevision::new(2)
            )),
            Err(ClientError::InvalidInput),
            "a timer mutated past a bound rejects without partial output"
        );
    }
    assert_eq!(
        project_furnace(&fixture.view_of(&provider, EPOCH))
            .expect("the committed queue still projects")
            .len(),
        1
    );
}

/// The completion row: a swing that reaches the requirement is finished, so
/// the authority republishes with the progress reset and the output updated
/// by one product. Each publication projects whole in actual source order —
/// the completed record carries the complete new state, never a partial
/// projection mixing the old output with the new progress — under the shared
/// stable key of the legal replace lifecycle, and each token names its own
/// observation's revision.
#[test]
fn completion_republication_updates_output_whole_never_partial() {
    let mut provider = admitted_mirror(EPOCH);
    let reference = furnace_ref(9);
    let smelting = transcript_state(9);
    commit(&mut provider, EPOCH, Event::FurnaceState(smelting));
    // The completed republication: the smelt consumed one raw iron, produced
    // one iron ingot, reset the progress and burned the tank down.
    let completed = furnace_state(
        9,
        stack(ITEM_RAW_IRON, 6),
        stack(ITEM_COAL, 1),
        stack(ITEM_IRON_INGOT, 6),
        0,
        1207,
    );
    commit(&mut provider, EPOCH, Event::FurnaceState(completed));

    let fixture = view_fixture();
    let view = fixture.view_of(&provider, EPOCH);
    let records = project_furnace(&view).expect("the smelt and its completion project");
    assert_eq!(records.len(), 2);

    assert_eq!(furnace_view(&records[0]), smelting);
    assert_eq!(furnace_view(&records[1]), completed);
    assert_eq!(
        furnace_view(&records[1]).output(),
        stack(ITEM_IRON_INGOT, 6),
        "the completed publication carries the updated output"
    );
    assert_eq!(
        furnace_view(&records[1]).progress_ticks(),
        0,
        "the completed publication carries the reset progress"
    );
    assert_ne!(
        furnace_view(&records[0]).output(),
        furnace_view(&records[1]).output(),
        "the completion is a whole new publication, never a partial mix"
    );
    assert_eq!(
        records[1]
            .record()
            .token()
            .expect("the completed view carries its own attribution")
            .confirmed_revision()
            .get(),
        2,
        "the completion's token names its own observation's revision"
    );
    // The replace lifecycle keeps one typed stable key across observations;
    // the mirror holds the view exactly once at the newest attribution.
    assert_eq!(*records[0].stable_key(), *records[1].stable_key());
    assert_eq!(
        *records[0].stable_key(),
        StableRecordKey::Inventory {
            topic: InventoryTopic::Furnace,
            identity: Some(reference),
        }
    );
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

/// The exhausted-fuel row: an empty fuel slot beside a zero burn time is the
/// extinguished state the schema admits, and it projects exactly — no burn
/// time is fabricated, and no timer-versus-stack relation is invented.
#[test]
fn exhausted_fuel_extinguished_burn_projects_exact_state() {
    let mut provider = admitted_mirror(EPOCH);
    let extinguished = furnace_state(
        9,
        stack(ITEM_RAW_IRON, 7),
        ItemStack::EMPTY,
        stack(ITEM_IRON_INGOT, 2),
        84,
        0,
    );
    commit(&mut provider, EPOCH, Event::FurnaceState(extinguished));

    let fixture = view_fixture();
    let records = project_furnace(&fixture.view_of(&provider, EPOCH))
        .expect("the extinguished state projects");
    assert_eq!(records.len(), 1);
    assert_eq!(furnace_view(&records[0]), extinguished);
    assert_eq!(
        furnace_view(&records[0]).fuel(),
        ItemStack::EMPTY,
        "the exhausted fuel slot publishes as the empty stack"
    );
    assert_eq!(
        furnace_view(&records[0]).burn_ticks(),
        0,
        "exhausted fuel extinguishes the burn per the schema"
    );
    assert_eq!(furnace_view(&records[0]).input(), stack(ITEM_RAW_IRON, 7));
    assert_eq!(
        furnace_view(&records[0]).output(),
        stack(ITEM_IRON_INGOT, 2)
    );
    assert_eq!(furnace_view(&records[0]).progress_ticks(), 84);
}

/// The replace row of the Go oracle: a republication at a new generation
/// replaces the view — the mirror retires the old position, the records keep
/// actual source order under their own actual identities, and a late state at
/// the already-replaced generation is refused whole, leaving the committed
/// revision, the observation queue and the projection exactly as they were.
#[test]
fn new_generation_replaces_view_and_late_state_refuses_whole() {
    let mut provider = admitted_mirror(EPOCH);
    let current_reference = furnace_ref(9);
    let opened = transcript_state(9);
    commit(&mut provider, EPOCH, Event::FurnaceState(opened));

    let next = furnace_state(
        10,
        stack(ITEM_SAND, 6),
        stack(ITEM_COAL, 1),
        stack(ITEM_GLASS, 1),
        1,
        1599,
    );
    commit(&mut provider, EPOCH, Event::FurnaceState(next));

    let fixture = view_fixture();
    let view = fixture.view_of(&provider, EPOCH);
    let records = project_furnace(&view).expect("the replace lifecycle projects");
    assert_eq!(records.len(), 2);
    assert_eq!(furnace_view(&records[0]), opened);
    assert_eq!(furnace_view(&records[1]), next);
    assert_eq!(
        provider.mirror().container_revision(&current_reference),
        None,
        "the replaced generation's view is retired"
    );
    assert_eq!(
        provider.mirror().container_revision(&furnace_ref(10)),
        Some(ConfirmedRevision::new(2)),
        "the new generation is the mirror's current view"
    );
    // A distinct generation is a distinct reference, hence a distinct stable
    // key: the two views never collide.
    assert_ne!(*records[0].stable_key(), *records[1].stable_key());
    assert!(
        records
            .windows(2)
            .all(|pair| pair[0].order() < pair[1].order())
    );

    // The late reply at the already-replaced generation is refused whole by
    // the real mirror: no partial application and no resurrected old view.
    let baseline = records;
    let late = packet(Event::FurnaceState(transcript_state(9)));
    assert!(
        provider.commit(&staged(EPOCH, late, 3)).is_err(),
        "a furnace state at an already-replaced generation refuses the whole observation"
    );
    assert_eq!(provider.mirror().revision().get(), 2);
    assert_eq!(provider.observations().len(), 2);
    assert_eq!(
        provider.mirror().inventory().container_count(),
        1,
        "the refused late state applied no partial view"
    );
    assert_eq!(
        project_furnace(&fixture.view_of(&provider, EPOCH)).expect("projection still succeeds"),
        baseline,
        "the refused late state leaves the projection unchanged"
    );
}

/// The closed-view row: a matching closed notification retires the mirror's
/// furnace view while contributing no furnace record — the closed topic
/// belongs to the container provider — and the tolerated stale close naming
/// an already-replaced generation changes no store and no projected record.
#[test]
fn matching_close_retires_view_and_stale_close_changes_nothing() {
    let mut provider = admitted_mirror(EPOCH);
    let reference = furnace_ref(10);
    let current = furnace_state(
        10,
        stack(ITEM_RAW_IRON, 7),
        stack(ITEM_COAL, 2),
        stack(ITEM_IRON_INGOT, 5),
        137,
        1463,
    );
    commit(&mut provider, EPOCH, Event::FurnaceState(current));

    let fixture = view_fixture();
    let baseline =
        project_furnace(&fixture.view_of(&provider, EPOCH)).expect("the current view projects");
    assert_eq!(baseline.len(), 1);

    // The stale close names the replaced generation 9 of the furnace
    // position; the mirror tolerates it with every store unchanged.
    let stale = furnace_ref(9);
    commit(
        &mut provider,
        EPOCH,
        Event::ContainerClosed(ContainerClosed::new(stale)),
    );
    assert_eq!(
        provider.mirror().container_revision(&reference),
        Some(ConfirmedRevision::new(1)),
        "a stale-generation close changes no store"
    );
    let after_stale = project_furnace(&fixture.view_of(&provider, EPOCH))
        .expect("the tolerated stale close still projects");
    assert_eq!(after_stale.len(), 1);
    assert_same_source_record(&after_stale[0], &baseline[0]);

    // The matching close retires the mirror's furnace view and contributes no
    // furnace record of its own: the closed topic belongs to the container
    // provider, never this one.
    commit(
        &mut provider,
        EPOCH,
        Event::ContainerClosed(ContainerClosed::new(reference)),
    );
    assert_eq!(
        provider.mirror().container_revision(&reference),
        None,
        "the matching close retired the mirror's furnace view"
    );
    let after_close = project_furnace(&fixture.view_of(&provider, EPOCH))
        .expect("the close observation still projects the queue's furnace records");
    assert_eq!(after_close.len(), 1);
    assert_same_source_record(&after_close[0], &baseline[0]);
}

/// The stale-revision row: an observation key from another epoch rejects the
/// whole projection with no partial output — even when a valid furnace
/// observation precedes it — and the committed queue still projects.
#[test]
fn old_epoch_rejects_whole_projection_without_partial_output() {
    let epoch = SessionEpoch::try_new(EPOCH).expect("epoch");
    let fixture = view_fixture();
    let mut provider = admitted_mirror(EPOCH);
    let opened = transcript_state(9);
    commit(&mut provider, EPOCH, Event::FurnaceState(opened));

    let stale_epoch = AcceptedObservation::try_new(
        ObservationKey::try_new(
            SessionEpoch::try_new(EPOCH + 1).expect("next epoch"),
            ConfirmedRevision::new(2),
            0,
        )
        .expect("key"),
        None,
        packet(Event::FurnaceState(transcript_state(9))),
        Vec::new(),
    )
    .expect("staged in another epoch");
    // The valid committed observation precedes the stale one: the rejection
    // still swallows the whole candidate, never a partial prefix.
    let mut queue: Vec<AcceptedObservation> = provider.observations().to_vec();
    queue.push(stale_epoch);
    assert_eq!(
        project_furnace(
            &fixture.view(provider.mirror(), &queue, epoch, ConfirmedRevision::new(2),)
        ),
        Err(ClientError::StaleEpoch),
        "an observation from another epoch rejects without partial output"
    );

    let records = project_furnace(&fixture.view_of(&provider, EPOCH))
        .expect("the committed queue still projects");
    assert_eq!(records.len(), 1);
    assert_eq!(furnace_view(&records[0]), opened);
}

/// A foreign publication of another topic contributes no furnace record and
/// invents no attribution, and a control packet that is not a checked
/// publication rejects the whole projection with no partial output.
#[test]
fn foreign_publications_contribute_nothing_and_control_packets_reject() {
    let mut provider = admitted_mirror(EPOCH);
    commit(
        &mut provider,
        EPOCH,
        Event::FurnaceState(transcript_state(9)),
    );
    // A chest publication belongs to the container topic.
    let mut chest_items = [ItemStack::EMPTY; 27];
    chest_items[0] = stack(ITEM_STONE, 5);
    commit(
        &mut provider,
        EPOCH,
        Event::ChestState(
            mornlea_domain::ChestState::try_new(mornlea_domain::ChestStateParts {
                container: ContainerRef::try_new(ChunkPos::new(-3, 7), ContainerKind::Chest, 5, 9)
                    .expect("checked chest reference"),
                items: chest_items,
            })
            .expect("checked chest state"),
        ),
    );
    // A personal-inventory publication belongs to the inventory topic.
    commit(
        &mut provider,
        EPOCH,
        Event::InventoryState(InventoryState::new(InventoryStateParts {
            selected: HotbarSlot::new(0).expect("checked hotbar slot"),
            hotbar: [ItemStack::EMPTY; 9],
            backpack: [ItemStack::EMPTY; 27],
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
    let records = project_furnace(&fixture.view_of(&provider, EPOCH))
        .expect("foreign publications still project");
    assert_eq!(
        records.len(),
        1,
        "only the furnace publication contributes a furnace record"
    );

    // A transport control packet is not a checked publication: no semantic
    // event exists for it, so the whole candidate rejects, never a partial
    // prefix beside the valid committed observations.
    let epoch = SessionEpoch::try_new(EPOCH).expect("epoch");
    let mut queue: Vec<AcceptedObservation> = provider.observations().to_vec();
    queue.push(staged(EPOCH, hello_packet(), 5));
    assert_eq!(
        project_furnace(
            &fixture.view(provider.mirror(), &queue, epoch, ConfirmedRevision::new(5),)
        ),
        Err(ClientError::InvalidInput),
        "a control packet rejects without partial output"
    );
    assert_eq!(
        project_furnace(&fixture.view_of(&provider, EPOCH))
            .expect("the committed queue still projects")
            .len(),
        1
    );
}

/// The reset row of the Go oracle: a fresh epoch's mirror starts with no
/// furnace views, commits and projects a furnace state with its own epoch's
/// token attribution, and refuses an observation staged in the old epoch.
#[test]
fn fresh_epoch_starts_empty_and_attributes_its_own_epoch() {
    let mut previous = admitted_mirror(EPOCH);
    commit(
        &mut previous,
        EPOCH,
        Event::FurnaceState(transcript_state(9)),
    );

    let fresh_epoch_value = EPOCH + 1;
    let fresh_epoch = SessionEpoch::try_new(fresh_epoch_value).expect("next epoch");
    let mut fresh = MirrorProvider::new(fresh_epoch, ClientLimits::try_new().expect("limits"))
        .expect("fresh mirror");
    fresh.admit().expect("admitted session");
    assert_eq!(
        fresh.mirror().inventory().container_count(),
        0,
        "a fresh epoch holds no furnace views"
    );

    let reopened = transcript_state(9);
    let reference = reopened.container();
    commit(&mut fresh, fresh_epoch_value, Event::FurnaceState(reopened));

    let fixture = view_fixture();
    let records = project_furnace(&fixture.view_of(&fresh, fresh_epoch_value))
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
        packet(Event::FurnaceState(transcript_state(9))),
        Vec::new(),
    )
    .expect("staged in the old epoch");
    let mut queue: Vec<AcceptedObservation> = fresh.observations().to_vec();
    queue.push(stale);
    assert_eq!(
        project_furnace(&fixture.view(
            fresh.mirror(),
            &queue,
            fresh_epoch,
            ConfirmedRevision::new(2),
        )),
        Err(ClientError::StaleEpoch),
        "an old-epoch observation never enters the fresh epoch's frame"
    );
}
