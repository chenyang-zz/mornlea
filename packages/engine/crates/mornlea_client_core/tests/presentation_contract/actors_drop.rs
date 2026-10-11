//! Item-drop actor projection contract tests.
//!
//! The table pins the accepted drop semantics against the landed mirror
//! provider, following the measured Go item-drop mirror transcript: an upsert
//! adds or wholly replaces one stack by identity — a partial pickup republishes
//! the same identity with the smaller count and a tool drop keeps its exact
//! durability — and every upsert observation records its own authoritative
//! tick, so the newest observation's tick is the one the latest record
//! carries; a remove batch naming any identity the mirror never confirmed is
//! refused whole with no partial application, and a duplicate remove after the
//! removal is refused the same way, leaving the committed revision, the
//! observation queue and the projected output exactly as they were; the same
//! identity may be upserted again after its removal, so repeated stable keys
//! across observations are the legal reuse lifecycle. The identity keeps its
//! raw dimension, chunk, slot and generation with no normalization — the raw
//! dimension is never coerced into a known world — and the world position is
//! the exact geometry-helper value of the chunk and chunk-local block index,
//! including the extreme i32 chunk range. Invalid counts, items, durabilities
//! and block indexes reject typed at the checked domain constructors, so no
//! invalid value can reach an accepted observation at all. Order keys are the
//! actual observation keys with packet record ordinals; equal, absent or
//! backwards source ticks never reconstruct order, a foreign publication of
//! another kind contributes no drop record and invents no attribution, a
//! control packet that is not a checked publication rejects the whole
//! projection, and an old-epoch observation rejects without partial output.

use mornlea_client_core::contracts::{
    ClientError, ClientLimits, ConfirmedRevision, FamilyOperation, ObservationKey, SessionEpoch,
};
use mornlea_client_core::drop_position;
use mornlea_client_core::input::InputProjectionState;
use mornlea_client_core::prediction::PlayerProjectionState;
use mornlea_client_core::presentation::actors::drop::project_drop;
use mornlea_client_core::presentation::frame::ActorRecord;
use mornlea_client_core::presentation::{
    AcceptedObservation, ActorDetail, ActorDimension, ActorId, ActorKind, AudioProjectionState,
    DiagnosticProjectionState, ErrorClassCounters, LifecycleProjectionState, MovementIntent,
    OrderedRecord, Pose, ProducerIdentity, ProjectionOrder, ProjectionView, QueueCounters,
    StableRecordKey,
};
use mornlea_client_core::session::ConfirmedMirror;
use mornlea_client_core::session::mirror::MirrorProvider;
use mornlea_domain::{
    ChunkPos, CombatHit, CombatTarget, DomainError, DropId, Event, Identities, ItemDrop,
    ItemDropParts, ItemDropRemoves, ItemDropRemovesParts, ItemDropUpserts, ItemDropUpsertsParts,
    ItemStack, durability_max,
};
use mornlea_protocol::{ServerHello, ServerPacket};

/// The epoch every fixture mirror and observation key carries.
const EPOCH: u64 = 7;

/// The registered stone item number, the Go `core.ItemStone` iota value: stone
/// stacks to 64 and carries no durability budget.
const ITEM_STONE: u16 = 1;

/// The registered stone pickaxe number, the Go `core.ItemStonePickaxe` iota
/// value: it stacks to one and carries the measured durability budget.
const ITEM_STONE_PICKAXE: u16 = 10;

/// The overworld raw wire dimension of the transcript fixtures; a drop keeps
/// it as the open raw value, never as a known-world tag.
const RAW_OVERWORLD: i32 = 0;

fn drop_id(dimension: i32, chunk_x: i32, chunk_z: i32, slot: u8, generation: u32) -> DropId {
    DropId::try_new(dimension, ChunkPos::new(chunk_x, chunk_z), slot, generation)
        .expect("checked drop identity")
}

fn stack(item: u16, count: u8, durability: u16) -> ItemStack {
    ItemStack::try_new(item, count, durability).expect("checked item stack")
}

/// One transcript upsert body: the identity, the chunk-local block cell and
/// the checked item/count/durability stack.
fn drop_record(id: DropId, block_index: u32, stack: ItemStack) -> ItemDrop {
    ItemDrop::try_new(ItemDropParts {
        id,
        block_index,
        stack,
    })
    .expect("checked dropped stack")
}

fn upserts_event(tick: u64, drops: Vec<ItemDrop>) -> Event {
    Event::ItemDropUpserts(
        ItemDropUpserts::try_new(ItemDropUpsertsParts {
            server_tick: tick,
            drops: drops.into_boxed_slice(),
        })
        .expect("checked item-drop upserts batch"),
    )
}

fn removes_event(tick: u64, ids: Vec<DropId>) -> Event {
    Event::ItemDropRemoves(
        ItemDropRemoves::try_new(ItemDropRemovesParts {
            server_tick: tick,
            ids: ids.into_boxed_slice(),
        })
        .expect("checked item-drop removes batch"),
    )
}

/// A combat hit against a hostile-kind target: the publication names a target
/// category alone and no drop identity, so no pickup attribution can ever be
/// drawn from it.
fn combat_hit_event(tick: u64, damage: u8) -> Event {
    Event::CombatHit(
        CombatHit::try_new(tick, damage, CombatTarget::Hostile).expect("checked combat hit"),
    )
}

fn packet(event: Event) -> ServerPacket {
    ServerPacket::try_from(event).expect("publication converts to its packet")
}

/// A transport control packet: the semantic publication set deliberately
/// excludes the control families, so this packet is never a checked
/// publication an actor provider could project.
fn hello_packet() -> ServerPacket {
    ServerPacket::ServerHello(
        ServerHello::new(Identities::current().protocol).expect("checked server hello"),
    )
}

/// An admitted real mirror provider with no observations yet.
fn admitted_mirror() -> MirrorProvider {
    let epoch = SessionEpoch::try_new(EPOCH).expect("epoch");
    let mut provider =
        MirrorProvider::new(epoch, ClientLimits::try_new().expect("limits")).expect("mirror");
    provider.admit().expect("admitted session");
    provider
}

/// Commits one publication through the real mirror provider, so the retained
/// observation queue carries the actual source keys. A drop publication names
/// its own raw dimension inside the identity, so the mirror records no
/// separate identity resolution for it.
fn commit(provider: &mut MirrorProvider, event: Event) -> ConfirmedRevision {
    let next = provider.mirror().revision().get() + 1;
    let key = ObservationKey::try_new(
        SessionEpoch::try_new(EPOCH).expect("epoch"),
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
fn staged(packet: ServerPacket, revision: u64) -> AcceptedObservation {
    AcceptedObservation::try_new(
        ObservationKey::try_new(
            SessionEpoch::try_new(EPOCH).expect("epoch"),
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
    fn view_of<'a>(&'a self, provider: &'a MirrorProvider) -> ProjectionView<'a> {
        let epoch = SessionEpoch::try_new(EPOCH).expect("epoch");
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
fn assert_confirmed_order(entry: &OrderedRecord<ActorRecord>, revision: u64, ordinal: u32) {
    match entry.order() {
        ProjectionOrder::Confirmed {
            observation,
            record_ordinal,
        } => {
            assert_eq!(observation.epoch().get(), EPOCH);
            assert_eq!(observation.confirmed_revision().get(), revision);
            assert_eq!(*record_ordinal, ordinal);
        }
        other => panic!("drop record is server-sourced: {other:?}"),
    }
}

/// The drop stack detail, refusing a foreign detail tag: `None` means the
/// record carries no detail at all, which is exactly the shape an
/// identity-only remove must keep.
fn drop_detail(entry: &OrderedRecord<ActorRecord>) -> Option<(u32, ItemStack)> {
    match entry.record().detail() {
        None => None,
        Some(ActorDetail::Drop { block_index, stack }) => Some((*block_index, *stack)),
        Some(other) => panic!("drop record carries a foreign detail tag: {other:?}"),
    }
}

/// Asserts every record of one projection keeps the drop shape: no yaw, no
/// pitch and no velocity, because the drop publication carries none — the
/// world position is the geometry value of the identity's chunk and the
/// record's block index alone.
fn assert_no_motion(records: &[OrderedRecord<ActorRecord>]) {
    for entry in records {
        assert_eq!(entry.record().yaw(), None, "drops carry no yaw");
        assert_eq!(entry.record().pitch(), None, "drops carry no pitch");
        assert_eq!(entry.record().velocity(), None, "drops carry no velocity");
    }
}

/// `upsert`: one upsert per drop record carrying the typed identity with its
/// raw dimension, chunk, slot and generation preserved exactly, the geometry
/// helper's exact finite world position, and the exact item/count/durability
/// stack detail — the published values and nothing else.
#[test]
fn upsert_carries_raw_identity_stack_and_exact_position() {
    let mut provider = admitted_mirror();
    // A raw dimension no known world owns: the identity keeps it verbatim
    // instead of being coerced into a known-world tag.
    let id = drop_id(-190, -1, 2, 3, 9);
    commit(
        &mut provider,
        upserts_event(42, vec![drop_record(id, 0, stack(ITEM_STONE, 5, 0))]),
    );

    let fixture = view_fixture();
    let records = project_drop(&fixture.view_of(&provider)).expect("upsert projects one record");
    assert_eq!(records.len(), 1);

    let entry = &records[0];
    assert_eq!(entry.record().kind(), ActorKind::Drop);
    let ActorId::Drop(published) = *entry.record().id() else {
        panic!("drop record carries a foreign identity tag");
    };
    assert_eq!(published.dimension(), -190, "the raw dimension survives");
    assert_eq!(
        published.chunk(),
        ChunkPos::new(-1, 2),
        "the raw chunk survives"
    );
    assert_eq!(published.slot(), 3, "the raw slot survives");
    assert_eq!(published.generation(), 9, "the raw generation survives");
    assert_eq!(
        *entry.record().dimension(),
        ActorDimension::DropRaw(-190),
        "the raw dimension is never coerced into a known world"
    );
    // The exact geometry value of chunk(-1,2) block index 0.
    assert_eq!(entry.record().position(), Some([-16.0, -64.0, 32.0]));
    assert_eq!(drop_detail(entry), Some((0, stack(ITEM_STONE, 5, 0))));
    assert_eq!(entry.record().header().operation(), FamilyOperation::Upsert);
    assert_eq!(entry.record().header().source_tick(), Some(42));
    assert_eq!(entry.record().header().epoch().get(), EPOCH);
    assert_eq!(
        entry.record().header().revision(),
        fixture.view_of(&provider).frame_revision(),
        "headers are rebased onto the coherent candidate revision"
    );
    assert_eq!(
        *entry.stable_key(),
        StableRecordKey::Actor {
            kind: ActorKind::Drop,
            dimension: ActorDimension::DropRaw(-190),
            id: ActorId::Drop(id),
        }
    );
    assert_confirmed_order(entry, 1, 0);
    assert_no_motion(&records);
    // The mirror's live identity store keeps the same raw identity once: an
    // upsert adds or wholly replaces, never duplicates.
    assert_eq!(provider.mirror().actors().drops(), &[id]);
}

/// The extreme geometry rows through the accepted helper: the last chunk cell
/// of the negative i32 extreme chunk, a mid-chunk decomposition and the
/// positive i32 extreme chunk all resolve to their exact finite f64 world
/// coordinates, in published packet order.
#[test]
fn upsert_extreme_geometry_is_exact() {
    let mut provider = admitted_mirror();
    let negative_extreme = drop_id(0, i32::MIN, i32::MIN, 0, 1);
    let mid_chunk = drop_id(5, 3, -4, 1, 2);
    let positive_extreme = drop_id(5, i32::MAX, 0, 0, 1);
    // The mid-chunk cross-check index of the geometry contract: section 5,
    // local y 7, z 3, x 11.
    let mid_index = 5 * 4096 + 7 * 256 + 3 * 16 + 11;
    commit(
        &mut provider,
        upserts_event(
            10,
            vec![
                drop_record(negative_extreme, 98303, stack(ITEM_STONE, 1, 0)),
                drop_record(mid_chunk, mid_index, stack(ITEM_STONE, 1, 0)),
                drop_record(positive_extreme, 0, stack(ITEM_STONE, 1, 0)),
            ],
        ),
    );

    let fixture = view_fixture();
    let records =
        project_drop(&fixture.view_of(&provider)).expect("extreme batch projects three records");
    assert_eq!(records.len(), 3);

    let expected = [
        (
            negative_extreme,
            [-34359738353.0, 319.0, -34359738353.0],
            98303u32,
        ),
        (mid_chunk, [59.0, 23.0, -61.0], mid_index),
        (positive_extreme, [34359738352.0, -64.0, 0.0], 0),
    ];
    for (entry, (id, position, index)) in records.iter().zip(&expected) {
        assert_eq!(*entry.record().id(), ActorId::Drop(*id));
        assert_eq!(entry.record().position(), Some(*position));
        assert_eq!(drop_detail(entry), Some((*index, stack(ITEM_STONE, 1, 0))));
    }
    assert_eq!(*records[0].record().id(), ActorId::Drop(negative_extreme));
    assert_eq!(*records[1].record().id(), ActorId::Drop(mid_chunk));
    assert_eq!(*records[2].record().id(), ActorId::Drop(positive_extreme));
    assert!(
        records
            .windows(2)
            .all(|pair| pair[0].order() < pair[1].order()),
        "records keep the published packet order"
    );
    for (ordinal, entry) in records.iter().enumerate() {
        assert_confirmed_order(entry, 1, ordinal as u32);
    }
    assert_no_motion(&records);
}

/// The transcript's replace rows: an upsert wholly replaces the same
/// identity's stack — a partial pickup republishes the smaller count and a
/// tool drop keeps its exact durability — and each upsert observation records
/// its own authoritative tick, so the newest record carries the newest tick
/// while every record keeps its own batch's values.
#[test]
fn repeated_upsert_replaces_stack_and_refreshes_tick() {
    let mut provider = admitted_mirror();
    let id = drop_id(RAW_OVERWORLD, 0, 0, 1, 1);
    let pickaxe_durability = durability_max(ITEM_STONE_PICKAXE).expect("measured pickaxe budget");
    commit(
        &mut provider,
        upserts_event(42, vec![drop_record(id, 7, stack(ITEM_STONE, 5, 0))]),
    );
    // The partial-pickup row: the same identity republished with count 3.
    commit(
        &mut provider,
        upserts_event(50, vec![drop_record(id, 9, stack(ITEM_STONE, 3, 0))]),
    );
    // The tool row: the exact durability budget survives, at another cell.
    commit(
        &mut provider,
        upserts_event(
            51,
            vec![drop_record(
                id,
                7,
                stack(ITEM_STONE_PICKAXE, 1, pickaxe_durability),
            )],
        ),
    );

    let fixture = view_fixture();
    let records =
        project_drop(&fixture.view_of(&provider)).expect("three upserts project three records");
    assert_eq!(records.len(), 3);

    let expected = [
        (
            42u64,
            Some((7, stack(ITEM_STONE, 5, 0))),
            Some([7.0, -64.0, 0.0]),
        ),
        (
            50,
            Some((9, stack(ITEM_STONE, 3, 0))),
            Some([9.0, -64.0, 0.0]),
        ),
        (
            51,
            Some((7, stack(ITEM_STONE_PICKAXE, 1, pickaxe_durability))),
            Some([7.0, -64.0, 0.0]),
        ),
    ];
    for (entry, (tick, detail, position)) in records.iter().zip(&expected) {
        assert_eq!(entry.record().header().source_tick(), Some(*tick));
        assert_eq!(drop_detail(entry), *detail);
        assert_eq!(entry.record().position(), *position);
    }
    for entry in &records {
        assert_eq!(
            *entry.stable_key(),
            StableRecordKey::Actor {
                kind: ActorKind::Drop,
                dimension: ActorDimension::DropRaw(RAW_OVERWORLD),
                id: ActorId::Drop(id),
            },
            "the replaced stack keeps the typed identity"
        );
    }
    assert_eq!(
        provider.mirror().actors().drops().len(),
        1,
        "an upsert wholly replaces, so one identity stays exactly once"
    );
    assert_no_motion(&records);
}

/// The transcript's pickup and reuse rows: a remove batch publishes one
/// remove-only record per identity — no position, no stack and no cause — and
/// the same identity may be upserted again after its removal, keeping its
/// stable key through the reuse.
#[test]
fn remove_publishes_identity_only_and_reuse_keeps_identity() {
    let mut provider = admitted_mirror();
    let first = drop_id(RAW_OVERWORLD, 0, 0, 1, 1);
    let second = drop_id(RAW_OVERWORLD, 0, 0, 2, 1);
    commit(
        &mut provider,
        upserts_event(
            42,
            vec![
                drop_record(first, 7, stack(ITEM_STONE, 2, 0)),
                drop_record(second, 9, stack(ITEM_STONE, 4, 0)),
            ],
        ),
    );
    commit(&mut provider, removes_event(60, vec![first]));
    // Reuse: the removed identity is upserted again at another cell.
    commit(
        &mut provider,
        upserts_event(61, vec![drop_record(first, 3, stack(ITEM_STONE, 8, 0))]),
    );

    let fixture = view_fixture();
    let records =
        project_drop(&fixture.view_of(&provider)).expect("the lifecycle projects four records");
    assert_eq!(records.len(), 4);

    let remove = &records[2];
    assert_eq!(
        remove.record().header().operation(),
        FamilyOperation::Remove
    );
    assert_eq!(remove.record().header().source_tick(), Some(60));
    assert_eq!(*remove.record().id(), ActorId::Drop(first));
    assert_eq!(
        *remove.record().dimension(),
        ActorDimension::DropRaw(RAW_OVERWORLD)
    );
    assert_eq!(remove.record().position(), None, "a remove names no cell");
    assert!(
        remove.record().detail().is_none(),
        "a remove publishes the identity alone"
    );
    assert_eq!(
        *remove.stable_key(),
        StableRecordKey::Actor {
            kind: ActorKind::Drop,
            dimension: ActorDimension::DropRaw(RAW_OVERWORLD),
            id: ActorId::Drop(first),
        }
    );
    assert_confirmed_order(remove, 2, 0);

    let reuse = &records[3];
    assert_eq!(reuse.record().header().operation(), FamilyOperation::Upsert);
    assert_eq!(reuse.record().header().source_tick(), Some(61));
    assert_eq!(drop_detail(reuse), Some((3, stack(ITEM_STONE, 8, 0))));
    assert_eq!(reuse.record().position(), Some([3.0, -64.0, 0.0]));
    assert_eq!(
        *reuse.stable_key(),
        *remove.stable_key(),
        "reuse keeps the typed identity"
    );
    assert_confirmed_order(reuse, 3, 0);

    assert!(
        records
            .windows(2)
            .all(|pair| pair[0].order() < pair[1].order()),
        "records keep actual source order across the batches"
    );
    let live = provider.mirror().actors().drops();
    assert_eq!(live.len(), 2, "both identities are live after the reuse");
    assert!(live.contains(&first) && live.contains(&second));
    assert_no_motion(&records);
}

/// An observation key from another epoch rejects the whole projection with no
/// partial output — even when a valid drop observation precedes it.
#[test]
fn old_epoch_rejects_whole_projection_without_partial_output() {
    let epoch = SessionEpoch::try_new(EPOCH).expect("epoch");
    let fixture = view_fixture();
    let mut provider = admitted_mirror();
    let id = drop_id(RAW_OVERWORLD, 0, 0, 1, 1);
    commit(
        &mut provider,
        upserts_event(42, vec![drop_record(id, 7, stack(ITEM_STONE, 1, 0))]),
    );

    let stale_epoch = AcceptedObservation::try_new(
        ObservationKey::try_new(
            SessionEpoch::try_new(EPOCH + 1).expect("next epoch"),
            ConfirmedRevision::new(2),
            0,
        )
        .expect("key"),
        None,
        packet(upserts_event(
            99,
            vec![drop_record(id, 9, stack(ITEM_STONE, 2, 0))],
        )),
        Vec::new(),
    )
    .expect("staged in another epoch");
    // The valid committed observation precedes the stale one: the rejection
    // still swallows the whole candidate, never a partial prefix.
    let mut queue: Vec<AcceptedObservation> = provider.observations().to_vec();
    queue.push(stale_epoch);
    assert_eq!(
        project_drop(&fixture.view(provider.mirror(), &queue, epoch, ConfirmedRevision::new(2))),
        Err(ClientError::StaleEpoch),
        "an observation from another epoch rejects without partial output"
    );

    let records = project_drop(&fixture.view_of(&provider)).expect("the queue still projects");
    assert_eq!(records.len(), 1);
    assert_eq!(*records[0].record().id(), ActorId::Drop(id));
}

/// The transcript's unchanged-state rows through the real mirror: a remove
/// batch naming one unknown identity alongside a known one is refused whole —
/// no partial application — and a duplicate remove after the removal is
/// refused the same way; both leave the committed revision, the observation
/// queue and the projected output exactly as they were, while a fresh epoch
/// starts empty and accepts an upsert again.
#[test]
fn orphan_and_duplicate_removals_leave_committed_state_unchanged() {
    let mut provider = admitted_mirror();
    let known = drop_id(RAW_OVERWORLD, 0, 0, 1, 1);
    let unknown = drop_id(RAW_OVERWORLD, 0, 0, 2, 1);
    commit(
        &mut provider,
        upserts_event(42, vec![drop_record(known, 7, stack(ITEM_STONE, 2, 0))]),
    );

    let fixture = view_fixture();
    let baseline = project_drop(&fixture.view_of(&provider)).expect("baseline projects");
    assert_eq!(baseline.len(), 1);
    assert_eq!(
        drop_detail(&baseline[0]),
        Some((7, stack(ITEM_STONE, 2, 0)))
    );

    let mixed_removal = packet(removes_event(50, vec![known, unknown]));
    assert!(
        provider.commit(&staged(mixed_removal, 2)).is_err(),
        "a remove naming one unknown identity refuses the whole batch"
    );
    assert_eq!(provider.mirror().revision().get(), 1);
    assert_eq!(provider.observations().len(), 1);
    assert_eq!(
        provider.mirror().actors().drops(),
        &[known],
        "the refused batch applied no partial removal"
    );
    assert_eq!(
        project_drop(&fixture.view_of(&provider)).expect("projection still succeeds"),
        baseline,
        "the refused batch leaves the projection unchanged"
    );

    // The valid pickup commits; a duplicate remove afterwards is an orphan
    // removal and leaves everything unchanged again.
    commit(&mut provider, removes_event(60, vec![known]));
    let removed = project_drop(&fixture.view_of(&provider)).expect("remove projects");
    assert_eq!(removed.len(), 2);
    assert_eq!(
        removed
            .last()
            .expect("remove record")
            .record()
            .header()
            .operation(),
        FamilyOperation::Remove
    );

    let duplicate_removal = packet(removes_event(61, vec![known]));
    assert!(
        provider.commit(&staged(duplicate_removal, 3)).is_err(),
        "a duplicate remove is an orphan removal"
    );
    assert_eq!(provider.mirror().revision().get(), 2);
    assert_eq!(provider.observations().len(), 2);
    assert!(provider.mirror().actors().drops().is_empty());
    assert_eq!(
        project_drop(&fixture.view_of(&provider)).expect("projection still succeeds"),
        removed,
        "the refused late input leaves the projection unchanged"
    );

    // The reset row of the transcript: a fresh epoch's mirror starts empty
    // and accepts an upsert again.
    let fresh_epoch = SessionEpoch::try_new(EPOCH + 1).expect("next epoch");
    let mut fresh = MirrorProvider::new(fresh_epoch, ClientLimits::try_new().expect("limits"))
        .expect("fresh mirror");
    fresh.admit().expect("admitted session");
    let relaunched = AcceptedObservation::try_new(
        ObservationKey::try_new(fresh_epoch, ConfirmedRevision::new(1), 0).expect("staged key"),
        None,
        packet(upserts_event(
            2,
            vec![drop_record(unknown, 1, stack(ITEM_STONE, 1, 0))],
        )),
        Vec::new(),
    )
    .expect("staged");
    fresh.commit(&relaunched).expect("fresh epoch commits");
    let records = project_drop(&fixture.view(
        fresh.mirror(),
        fresh.observations(),
        fresh_epoch,
        ConfirmedRevision::new(2),
    ))
    .expect("fresh epoch projects");
    assert_eq!(records.len(), 1);
    assert_eq!(
        records[0].record().header().epoch().get(),
        EPOCH + 1,
        "the fresh epoch's records carry their own epoch"
    );
}

/// The transcript's invalid-batch classes reject typed at the checked domain
/// constructors — a zero or over-limit count, an unregistered item, a
/// durability violation and an out-of-bounds block index never become a
/// publication at all — and the batch relations refuse an empty batch and a
/// non-increasing identity sequence; the geometry helper enforces the same
/// index bound the provider's world position reads.
#[test]
fn invalid_count_index_and_batch_order_reject_typed() {
    assert_eq!(
        ItemStack::try_new(ITEM_STONE, 0, 0),
        Err(DomainError::InvalidCount),
        "a zero count is typed-invalid"
    );
    assert_eq!(
        ItemStack::try_new(ITEM_STONE, 65, 0),
        Err(DomainError::InvalidCount),
        "a count above the stack limit is typed-invalid"
    );
    assert_eq!(
        ItemStack::try_new(4242, 1, 0),
        Err(DomainError::InvalidItem),
        "an unregistered item number is typed-invalid"
    );
    assert_eq!(
        ItemStack::try_new(ITEM_STONE_PICKAXE, 1, 0),
        Err(DomainError::InvalidDurability),
        "a durable item at durability zero is typed-invalid"
    );

    let id = drop_id(RAW_OVERWORLD, 0, 0, 1, 1);
    assert_eq!(
        ItemDrop::try_new(ItemDropParts {
            id,
            block_index: 98304,
            stack: stack(ITEM_STONE, 1, 0),
        }),
        Err(DomainError::InvalidBlockIndex),
        "the first index outside the chunk's cells is typed-invalid"
    );
    // The provider's world position reads the same bound, so no invalid
    // index can produce a coordinate.
    assert_eq!(drop_position(id, 98304), Err(ClientError::InvalidInput));

    let other = drop_id(RAW_OVERWORLD, 0, 0, 2, 1);
    assert_eq!(
        ItemDropUpserts::try_new(ItemDropUpsertsParts {
            server_tick: 1,
            drops: vec![
                drop_record(id, 0, stack(ITEM_STONE, 1, 0)),
                drop_record(id, 1, stack(ITEM_STONE, 1, 0)),
            ]
            .into_boxed_slice(),
        }),
        Err(DomainError::InvalidStateOrder),
        "a duplicate identity in one batch is typed-invalid"
    );
    assert_eq!(
        ItemDropUpserts::try_new(ItemDropUpsertsParts {
            server_tick: 1,
            drops: vec![
                drop_record(other, 0, stack(ITEM_STONE, 1, 0)),
                drop_record(id, 1, stack(ITEM_STONE, 1, 0)),
            ]
            .into_boxed_slice(),
        }),
        Err(DomainError::InvalidStateOrder),
        "a decreasing identity sequence is typed-invalid"
    );
    assert_eq!(
        ItemDropUpserts::try_new(ItemDropUpsertsParts {
            server_tick: 1,
            drops: Vec::new().into_boxed_slice(),
        }),
        Err(DomainError::EmptyStateBatch),
        "an empty upserts batch is typed-invalid"
    );
    assert_eq!(
        ItemDropRemoves::try_new(ItemDropRemovesParts {
            server_tick: 1,
            ids: Vec::new().into_boxed_slice(),
        }),
        Err(DomainError::EmptyStateBatch),
        "an empty removes batch is typed-invalid"
    );
}

/// A foreign publication of another kind contributes no drop record and
/// invents no attribution, and a control packet that is not a checked
/// publication rejects the whole projection with no partial output.
#[test]
fn foreign_publications_contribute_nothing_and_control_packets_reject() {
    let mut provider = admitted_mirror();
    let id = drop_id(RAW_OVERWORLD, 0, 0, 1, 1);
    commit(
        &mut provider,
        upserts_event(42, vec![drop_record(id, 7, stack(ITEM_STONE, 1, 0))]),
    );
    commit(&mut provider, combat_hit_event(43, 2));

    let fixture = view_fixture();
    let records = project_drop(&fixture.view_of(&provider)).expect("projection succeeds");
    assert_eq!(records.len(), 1, "a combat hit publishes no drop record");
    assert_eq!(drop_detail(&records[0]), Some((7, stack(ITEM_STONE, 1, 0))));
    assert_eq!(records[0].record().position(), Some([7.0, -64.0, 0.0]));
    assert_no_motion(&records);

    // A transport control packet is not a checked publication: no semantic
    // event exists for it, so the whole candidate rejects, never a partial
    // prefix beside the valid committed observation.
    let epoch = SessionEpoch::try_new(EPOCH).expect("epoch");
    let mut queue: Vec<AcceptedObservation> = provider.observations().to_vec();
    queue.push(staged(hello_packet(), 3));
    assert_eq!(
        project_drop(&fixture.view(provider.mirror(), &queue, epoch, ConfirmedRevision::new(3))),
        Err(ClientError::InvalidInput),
        "a control packet rejects without partial output"
    );
    assert_eq!(
        project_drop(&fixture.view_of(&provider))
            .expect("the committed queue still projects")
            .len(),
        1
    );
}
