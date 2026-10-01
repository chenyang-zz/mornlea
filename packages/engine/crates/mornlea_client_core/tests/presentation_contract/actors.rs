//! Complete actor family assembly contract tests.
//!
//! The table pins the serial actor family assembly against the six accepted
//! real per-kind providers over real committed observations: the six checked
//! vectors combine into one coherent `actors@1` vector where retained order
//! keys — never the parts order and never equal or absent source ticks —
//! determine the interleaving; equal underlying bytes or digits behind
//! different tagged identities never collide because the stable key carries
//! the actor kind and the typed identity; one identity may legally carry
//! several records (spawn, state, remove and reuse) while a removal without
//! a live identity, a duplicate removal, an envelope that disagrees with its
//! record, a foreign epoch or revision, and a family count or frame byte cap
//! plus one each reject the whole family with the previous output retained.

use mornlea_client_core::contracts::{
    ClientError, ClientLimits, ConfirmedRevision, FAMILY_ACTORS, FamilyKey, FamilyOperation,
    ObservationKey, RecordHeader, SessionEpoch,
};
use mornlea_client_core::input::InputProjectionState;
use mornlea_client_core::prediction::PlayerProjectionState;
use mornlea_client_core::presentation::actors::companion::project_companion;
use mornlea_client_core::presentation::actors::drop::project_drop;
use mornlea_client_core::presentation::actors::hostile::project_hostile;
use mornlea_client_core::presentation::actors::passive::project_passive;
use mornlea_client_core::presentation::actors::projectile::project_projectile;
use mornlea_client_core::presentation::actors::remote_player::project_remote_player;
use mornlea_client_core::presentation::family_actors::assemble_actors;
use mornlea_client_core::presentation::frame::{
    ActorRecord, FamilyFrame, FamilyRecords, PresentationFrame,
};
use mornlea_client_core::presentation::{
    AcceptedObservation, ActorDimension, ActorId, ActorKind, AudioProjectionState,
    DiagnosticProjectionState, ErrorClassCounters, LifecycleProjectionState, MovementIntent,
    OrderedRecord, Pose, ProducerIdentity, ProjectionOrder, ProjectionView, QueueCounters,
    StableRecordKey,
};
use mornlea_client_core::session::mirror::MirrorProvider;
use mornlea_domain::{
    ChunkPos, CompanionId, CompanionName, CompanionSpawn, CompanionSpawnParts, Dimension,
    DisplayName, DropId, Event, FiniteVec3, HostileId, HostileKind, HostileSpawn,
    HostileSpawnParts, HostileSpawnRecord, HostileSpawnRecordParts, HostileState,
    HostileStateParts, HostileStateRecord, HostileStateRecordParts, ItemDrop, ItemDropParts,
    ItemDropRemoves, ItemDropRemovesParts, ItemDropUpserts, ItemDropUpsertsParts, ItemStack,
    LookAngles, PassiveId, PassiveSpawn, PassiveSpawnParts, PassiveSpawnRecord,
    PassiveSpawnRecordParts, PlayerId, ProjectileId, ProjectileKind, ProjectileSpawn,
    ProjectileSpawnParts, ProjectileSpawnRecord, ProjectileSpawnRecordParts, RemotePlayerDespawn,
    RemotePlayerSpawn, RemotePlayerSpawnParts,
};
use mornlea_protocol::ServerPacket;

/// The epoch every fixture mirror and observation key carries.
const EPOCH: u64 = 7;

/// The candidate frame revision the hand-built envelope fixtures rebase onto.
const FRAME_REVISION: u64 = 10;

/// A checked UUIDv4 identity whose raw byte order follows `first`.
fn player_id(first: u8) -> PlayerId {
    let mut bytes = [0u8; 16];
    bytes[0] = first;
    bytes[6] = 0x40;
    bytes[8] = 0x80;
    PlayerId::try_from_bytes(bytes).expect("checked uuid v4 player identity")
}

/// A checked UUIDv4 companion identity whose raw bytes are the exact `bytes`
/// given, so a player fixture can carry the same underlying byte pattern.
fn companion_from(bytes: [u8; 16]) -> CompanionId {
    CompanionId::try_from_bytes(bytes).expect("checked uuid v4 companion identity")
}

/// A checked UUIDv4 player identity from the same raw byte pattern.
fn player_from(bytes: [u8; 16]) -> PlayerId {
    PlayerId::try_from_bytes(bytes).expect("checked uuid v4 player identity")
}

fn display_name(text: &str) -> DisplayName {
    DisplayName::try_from_canonical(text.to_owned()).expect("canonical display name")
}

fn companion_name(text: &str) -> CompanionName {
    CompanionName::try_from_canonical(text.to_owned()).expect("canonical companion name")
}

fn finite_pos(components: [f32; 3]) -> FiniteVec3 {
    FiniteVec3::try_new(components).expect("finite position")
}

fn finite_look(yaw: f32, pitch: f32) -> LookAngles {
    LookAngles::try_new(yaw, pitch).expect("finite look angles")
}

fn drop_id(generation: u32) -> DropId {
    DropId::try_new(0, ChunkPos::new(1, 1), 5, generation).expect("checked drop identity")
}

fn stack(count: u8) -> ItemStack {
    ItemStack::try_new(1, count, 0).expect("checked item stack")
}

fn remote_spawn_event(id: PlayerId, name: &str, tick: u64, position: [f32; 3]) -> Event {
    Event::RemotePlayerSpawn(RemotePlayerSpawn::new(RemotePlayerSpawnParts {
        player_id: id,
        display_name: display_name(name),
        server_tick: tick,
        dimension: Dimension::OVERWORLD,
        position: finite_pos(position),
        look: finite_look(0.0, 0.0),
    }))
}

fn hostile_spawn_event(tick: u64, id: HostileId, position: [f32; 3]) -> Event {
    Event::HostileSpawn(
        HostileSpawn::try_new(HostileSpawnParts {
            server_tick: tick,
            spawns: vec![
                HostileSpawnRecord::try_new(HostileSpawnRecordParts {
                    id,
                    dimension: Dimension::OVERWORLD,
                    position: finite_pos(position),
                    yaw: 0.5,
                    health: 20,
                    kind: HostileKind::Nightwalker,
                })
                .expect("checked hostile spawn record"),
            ]
            .into_boxed_slice(),
        })
        .expect("checked hostile spawn batch"),
    )
}

fn hostile_state_event(tick: u64, id: HostileId, position: [f32; 3]) -> Event {
    Event::HostileState(
        HostileState::try_new(HostileStateParts {
            server_tick: tick,
            states: vec![
                HostileStateRecord::try_new(HostileStateRecordParts {
                    id,
                    position: finite_pos(position),
                    velocity: finite_pos([0.5, 0.0, 0.0]),
                    yaw: 0.6,
                    health: 18,
                    kind: HostileKind::Nightwalker,
                })
                .expect("checked hostile state record"),
            ]
            .into_boxed_slice(),
        })
        .expect("checked hostile state batch"),
    )
}

fn passive_spawn_event(tick: u64, id: PassiveId, position: [f32; 3]) -> Event {
    Event::PassiveSpawn(
        PassiveSpawn::try_new(PassiveSpawnParts {
            server_tick: tick,
            spawns: vec![
                PassiveSpawnRecord::try_new(PassiveSpawnRecordParts {
                    id,
                    dimension: Dimension::OVERWORLD,
                    position: finite_pos(position),
                    yaw: 0.25,
                    health: 10,
                })
                .expect("checked passive spawn record"),
            ]
            .into_boxed_slice(),
        })
        .expect("checked passive spawn batch"),
    )
}

fn projectile_spawn_event(tick: u64, id: ProjectileId, position: [f32; 3]) -> Event {
    Event::ProjectileSpawn(
        ProjectileSpawn::try_new(ProjectileSpawnParts {
            server_tick: tick,
            spawns: vec![ProjectileSpawnRecord::new(ProjectileSpawnRecordParts {
                id,
                kind: ProjectileKind::Shard,
                dimension: Dimension::OVERWORLD,
                position: finite_pos(position),
                velocity: finite_pos([1.0, 0.5, 0.0]),
            })]
            .into_boxed_slice(),
        })
        .expect("checked projectile spawn batch"),
    )
}

fn companion_spawn_event(id: CompanionId, name: &str, tick: u64, position: [f32; 3]) -> Event {
    Event::CompanionSpawn(
        CompanionSpawn::try_new(CompanionSpawnParts {
            id,
            name: companion_name(name),
            server_tick: tick,
            dimension: Dimension::OVERWORLD,
            position: finite_pos(position),
            look: finite_look(0.0, 0.125),
        })
        .expect("checked companion spawn"),
    )
}

fn drop_upserts_event(tick: u64, id: DropId) -> Event {
    Event::ItemDropUpserts(
        ItemDropUpserts::try_new(ItemDropUpsertsParts {
            server_tick: tick,
            drops: vec![
                ItemDrop::try_new(ItemDropParts {
                    id,
                    block_index: 4,
                    stack: stack(3),
                })
                .expect("checked drop record"),
            ]
            .into_boxed_slice(),
        })
        .expect("checked drop upsert batch"),
    )
}

fn drop_removes_event(tick: u64, id: DropId) -> Event {
    Event::ItemDropRemoves(
        ItemDropRemoves::try_new(ItemDropRemovesParts {
            server_tick: tick,
            ids: vec![id].into_boxed_slice(),
        })
        .expect("checked drop remove batch"),
    )
}

fn remote_despawn_event(id: PlayerId) -> Event {
    Event::RemotePlayerDespawn(RemotePlayerDespawn::new(id))
}

fn packet(event: Event) -> ServerPacket {
    ServerPacket::try_from(event).expect("publication converts to its packet")
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
/// observation queue carries the actual source keys and identity resolution.
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

    /// The candidate view of the provider's committed state: the next
    /// revision the coherent frame would carry.
    fn view_of<'a>(&'a self, provider: &'a MirrorProvider) -> ProjectionView<'a> {
        let epoch = SessionEpoch::try_new(EPOCH).expect("epoch");
        let revision = ConfirmedRevision::new(provider.mirror().revision().get() + 1);
        ProjectionView::try_new(
            provider.mirror(),
            provider.observations(),
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
}

fn view_fixture() -> ViewFixture {
    ViewFixture::new(
        SessionEpoch::try_new(EPOCH).expect("epoch"),
        ConfirmedRevision::new(1),
    )
}

/// The six provider vectors of one view, in the packet's provider order.
fn provider_parts(view: &ProjectionView<'_>) -> [Vec<OrderedRecord<ActorRecord>>; 6] {
    [
        project_remote_player(view).expect("remote-player projection"),
        project_hostile(view).expect("hostile projection"),
        project_passive(view).expect("passive projection"),
        project_projectile(view).expect("projectile projection"),
        project_companion(view).expect("companion projection"),
        project_drop(view).expect("drop projection"),
    ]
}

/// Assembles the six real provider vectors of one committed fixture state
/// under the supplied limits.
fn assemble(
    fixture: &ViewFixture,
    provider: &MirrorProvider,
    limits: &ClientLimits,
) -> Result<Vec<ActorRecord>, ClientError> {
    let view = fixture.view_of(provider);
    let parts = provider_parts(&view);
    assemble_actors(view.frame_epoch(), view.frame_revision(), parts, limits)
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

/// The exact minimal actors-only frame size of one assembled vector, through
/// the accepted frame accounting the assembler itself uses.
fn minimal_frame_size(records: &[ActorRecord]) -> usize {
    PresentationFrame::try_new(
        SessionEpoch::try_new(EPOCH).expect("epoch"),
        ConfirmedRevision::new(FRAME_REVISION),
        0,
        vec![
            FamilyFrame::try_new(
                FamilyKey::try_new(FAMILY_ACTORS).expect("actors family key"),
                FamilyRecords::Actors(records.to_vec()),
            )
            .expect("actors family entry"),
        ],
    )
    .expect("minimal actors-only candidate frame")
    .validated_size()
    .expect("checked size")
}

// --- hand-built envelope fixtures for sequences the mirror can never commit ---

fn header(operation: FamilyOperation) -> RecordHeader {
    RecordHeader::try_new(
        SessionEpoch::try_new(EPOCH).expect("epoch"),
        ConfirmedRevision::new(FRAME_REVISION),
        None,
        operation,
    )
    .expect("checked header")
}

fn envelope(
    observation_revision: u64,
    record_ordinal: u32,
    stable_key: StableRecordKey,
    record: ActorRecord,
) -> OrderedRecord<ActorRecord> {
    OrderedRecord::try_new(
        ProjectionOrder::Confirmed {
            observation: ObservationKey::try_new(
                SessionEpoch::try_new(EPOCH).expect("epoch"),
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

/// A minimal checked passive upsert record for the hand-built sequences.
fn passive_upsert(id: PassiveId) -> ActorRecord {
    ActorRecord::try_new(
        header(FamilyOperation::Upsert),
        ActorKind::Passive,
        ActorId::Passive(id),
        ActorDimension::Known(Dimension::OVERWORLD),
        None,
        None,
        None,
        None,
        None,
    )
    .expect("checked passive upsert record")
}

/// A minimal checked passive remove record for the hand-built sequences.
fn passive_remove(id: PassiveId) -> ActorRecord {
    ActorRecord::try_new(
        header(FamilyOperation::Remove),
        ActorKind::Passive,
        ActorId::Passive(id),
        ActorDimension::Known(Dimension::OVERWORLD),
        None,
        None,
        None,
        None,
        None,
    )
    .expect("checked passive remove record")
}

fn passive_key(id: PassiveId) -> StableRecordKey {
    StableRecordKey::Actor {
        kind: ActorKind::Passive,
        dimension: ActorDimension::Known(Dimension::OVERWORLD),
        id: ActorId::Passive(id),
    }
}

fn empty_parts() -> [Vec<OrderedRecord<ActorRecord>>; 6] {
    [
        Vec::new(),
        Vec::new(),
        Vec::new(),
        Vec::new(),
        Vec::new(),
        Vec::new(),
    ]
}

fn assemble_hand_built(
    parts: [Vec<OrderedRecord<ActorRecord>>; 6],
) -> Result<Vec<ActorRecord>, ClientError> {
    assemble_actors(
        SessionEpoch::try_new(EPOCH).expect("epoch"),
        ConfirmedRevision::new(FRAME_REVISION),
        parts,
        &ClientLimits::try_new().expect("limits"),
    )
}

// --- the table ---

/// `all-six-kinds`: the six real providers' vectors of one committed view
/// assemble into one family vector in actual observation order, every kind
/// present exactly once, and the parts order — not the observation order —
/// is free: a rotated parts array yields the identical vector.
#[test]
fn all_six_kinds_assemble_in_observation_order_any_parts_order() {
    let mut provider = admitted_mirror();
    let player = player_id(0x11);
    let hostile = HostileId::try_new(41).expect("checked hostile identity");
    let passive = PassiveId::try_new(42).expect("checked passive identity");
    let projectile = ProjectileId::try_new(43).expect("checked projectile identity");
    let mut companion_bytes = [0u8; 16];
    companion_bytes[0] = 0x44;
    companion_bytes[6] = 0x40;
    companion_bytes[8] = 0x80;
    let companion = companion_from(companion_bytes);
    let drop = drop_id(1);
    commit(
        &mut provider,
        remote_spawn_event(player, "Aria", 10, [1.0, 70.0, 1.0]),
    );
    commit(
        &mut provider,
        hostile_spawn_event(11, hostile, [2.0, 65.0, 2.0]),
    );
    commit(
        &mut provider,
        passive_spawn_event(12, passive, [3.0, 64.0, 3.0]),
    );
    commit(
        &mut provider,
        projectile_spawn_event(13, projectile, [4.0, 66.0, 4.0]),
    );
    commit(
        &mut provider,
        companion_spawn_event(companion, "Bram", 14, [5.0, 64.0, 5.0]),
    );
    commit(&mut provider, drop_upserts_event(15, drop));

    let fixture = view_fixture();
    let view = fixture.view_of(&provider);
    let parts = provider_parts(&view);
    let limits = ClientLimits::try_new().expect("limits");
    let assembled = assemble_actors(
        view.frame_epoch(),
        view.frame_revision(),
        parts.clone(),
        &limits,
    )
    .expect("all six kinds assemble");

    assert_eq!(assembled.len(), 6, "every kind contributes its record");
    let expected_kinds = [
        ActorKind::RemotePlayer,
        ActorKind::Hostile,
        ActorKind::Passive,
        ActorKind::Projectile,
        ActorKind::Companion,
        ActorKind::Drop,
    ];
    for (record, kind) in assembled.iter().zip(&expected_kinds) {
        assert_eq!(record.kind(), *kind, "observation order, not parts order");
    }
    assert_eq!(*assembled[0].id(), ActorId::RemotePlayer(player));
    assert_eq!(*assembled[1].id(), ActorId::Hostile(hostile));
    assert_eq!(*assembled[2].id(), ActorId::Passive(passive));
    assert_eq!(*assembled[3].id(), ActorId::Projectile(projectile));
    assert_eq!(*assembled[4].id(), ActorId::Companion(companion));
    assert_eq!(*assembled[5].id(), ActorId::Drop(drop));

    // The emitted vector is the unchanged record payload: the envelopes are
    // stripped and nothing else about a record changes.
    let mut expected: Vec<OrderedRecord<ActorRecord>> = parts.concat();
    expected.sort_by(|left, right| {
        left.order()
            .cmp(right.order())
            .then_with(|| left.stable_key().cmp(right.stable_key()))
    });
    let expected: Vec<ActorRecord> = expected
        .into_iter()
        .map(|entry| entry.into_record())
        .collect();
    assert_eq!(assembled, expected, "records are emitted unchanged");

    // A rotated parts array — the drop vector first — yields the identical
    // output because retained order keys, not parts order, interleave.
    let rotated = [
        parts[5].clone(),
        parts[0].clone(),
        parts[1].clone(),
        parts[2].clone(),
        parts[3].clone(),
        parts[4].clone(),
    ];
    let rotated = assemble_actors(view.frame_epoch(), view.frame_revision(), rotated, &limits)
        .expect("rotated parts assemble");
    assert_eq!(assembled, rotated, "parts order never reorders the family");
}

/// `cross-kind collision impossible`: equal underlying digits behind the
/// hostile, passive and projectile tags and equal underlying UUID bytes
/// behind the player and companion tags stay distinct identities, because
/// the stable key carries the actor kind and the typed identity.
#[test]
fn equal_underlying_digits_and_bytes_across_tags_stay_distinct() {
    let mut provider = admitted_mirror();
    let digits = 9u64;
    let hostile = HostileId::try_new(digits).expect("checked hostile identity");
    let passive = PassiveId::try_new(digits).expect("checked passive identity");
    let projectile = ProjectileId::try_new(digits).expect("checked projectile identity");
    let mut shared = [0u8; 16];
    shared[0] = 0x99;
    shared[6] = 0x40;
    shared[8] = 0x80;
    let player = player_from(shared);
    let companion = companion_from(shared);
    commit(
        &mut provider,
        hostile_spawn_event(20, hostile, [1.0, 65.0, 1.0]),
    );
    commit(
        &mut provider,
        passive_spawn_event(21, passive, [2.0, 64.0, 2.0]),
    );
    commit(
        &mut provider,
        projectile_spawn_event(22, projectile, [3.0, 66.0, 3.0]),
    );
    commit(
        &mut provider,
        remote_spawn_event(player, "Same", 23, [4.0, 70.0, 4.0]),
    );
    commit(
        &mut provider,
        companion_spawn_event(companion, "Bytes", 24, [5.0, 64.0, 5.0]),
    );

    let fixture = view_fixture();
    let limits = ClientLimits::try_new().expect("limits");
    let assembled = assemble(&fixture, &provider, &limits).expect("distinct identities assemble");
    assert_eq!(assembled.len(), 5, "no cross-kind identity collapse");

    let expected = [
        (ActorKind::Hostile, ActorId::Hostile(hostile)),
        (ActorKind::Passive, ActorId::Passive(passive)),
        (ActorKind::Projectile, ActorId::Projectile(projectile)),
        (ActorKind::RemotePlayer, ActorId::RemotePlayer(player)),
        (ActorKind::Companion, ActorId::Companion(companion)),
    ];
    for (record, (kind, id)) in assembled.iter().zip(&expected) {
        assert_eq!(record.kind(), *kind);
        assert_eq!(*record.id(), *id);
    }
    let mut kinds: Vec<ActorKind> = assembled.iter().map(|record| record.kind()).collect();
    kinds.dedup();
    assert_eq!(
        kinds.len(),
        5,
        "five distinct kind-tagged identities remain"
    );
}

/// `interleaving`: player, hostile and drop observations carrying one equal
/// source tick — and one despawn carrying none — keep their private
/// observation-key order after the merge; the output interleaves by actual
/// source order instead of batching by topic.
#[test]
fn interleaved_equal_and_absent_ticks_keep_private_source_order() {
    let mut provider = admitted_mirror();
    let player = player_id(0x21);
    let hostile = HostileId::try_new(51).expect("checked hostile identity");
    let drop = drop_id(2);
    let equal_tick = 77u64;
    commit(
        &mut provider,
        remote_spawn_event(player, "Ash", equal_tick, [1.0, 70.0, 1.0]),
    );
    commit(
        &mut provider,
        hostile_spawn_event(equal_tick, hostile, [2.0, 65.0, 2.0]),
    );
    commit(&mut provider, drop_upserts_event(equal_tick, drop));
    commit(&mut provider, remote_despawn_event(player));
    commit(
        &mut provider,
        hostile_state_event(equal_tick, hostile, [2.5, 65.5, 2.5]),
    );
    commit(&mut provider, drop_removes_event(equal_tick, drop));

    let fixture = view_fixture();
    let limits = ClientLimits::try_new().expect("limits");
    let assembled = assemble(&fixture, &provider, &limits).expect("interleaved family assembles");
    assert_eq!(assembled.len(), 6);

    let expected = [
        (
            ActorKind::RemotePlayer,
            ActorId::RemotePlayer(player),
            FamilyOperation::Upsert,
        ),
        (
            ActorKind::Hostile,
            ActorId::Hostile(hostile),
            FamilyOperation::Upsert,
        ),
        (
            ActorKind::Drop,
            ActorId::Drop(drop),
            FamilyOperation::Upsert,
        ),
        (
            ActorKind::RemotePlayer,
            ActorId::RemotePlayer(player),
            FamilyOperation::Remove,
        ),
        (
            ActorKind::Hostile,
            ActorId::Hostile(hostile),
            FamilyOperation::Upsert,
        ),
        (
            ActorKind::Drop,
            ActorId::Drop(drop),
            FamilyOperation::Remove,
        ),
    ];
    let expected_ticks = [
        Some(equal_tick),
        Some(equal_tick),
        Some(equal_tick),
        None,
        Some(equal_tick),
        Some(equal_tick),
    ];
    for ((record, (kind, id, operation)), tick) in
        assembled.iter().zip(&expected).zip(&expected_ticks)
    {
        assert_eq!(record.kind(), *kind, "interleaved by actual source order");
        assert_eq!(*record.id(), *id);
        assert_eq!(record.header().operation(), *operation);
        assert_eq!(record.header().source_tick(), *tick);
        assert_eq!(record.header().epoch().get(), EPOCH);
    }
    assert_eq!(
        assembled
            .iter()
            .map(|record| record.kind())
            .collect::<Vec<_>>(),
        vec![
            ActorKind::RemotePlayer,
            ActorKind::Hostile,
            ActorKind::Drop,
            ActorKind::RemotePlayer,
            ActorKind::Hostile,
            ActorKind::Drop,
        ],
        "never topic-batched"
    );
}

/// `remove before reuse`: one identity legally carries several records — a
/// spawn, its removal and the reuse spawn after it — in retained source
/// order with one shared stable identity.
#[test]
fn remove_precedes_reused_identity_lifecycle() {
    let mut provider = admitted_mirror();
    let player = player_id(0x31);
    commit(
        &mut provider,
        remote_spawn_event(player, "Bram", 5, [4.0, 64.0, 4.0]),
    );
    commit(&mut provider, remote_despawn_event(player));
    commit(
        &mut provider,
        remote_spawn_event(player, "BramAgain", 6, [5.0, 64.0, 5.0]),
    );

    let fixture = view_fixture();
    let limits = ClientLimits::try_new().expect("limits");
    let assembled = assemble(&fixture, &provider, &limits).expect("reuse lifecycle assembles");
    assert_eq!(assembled.len(), 3, "one identity, three records");

    let expected = [
        (FamilyOperation::Upsert, Some(5u64)),
        (FamilyOperation::Remove, None),
        (FamilyOperation::Upsert, Some(6u64)),
    ];
    for (record, (operation, tick)) in assembled.iter().zip(&expected) {
        assert_eq!(*record.id(), ActorId::RemotePlayer(player));
        assert_eq!(record.kind(), ActorKind::RemotePlayer);
        assert_eq!(record.header().operation(), *operation);
        assert_eq!(record.header().source_tick(), *tick);
    }
    assert!(
        assembled[1].position().is_none(),
        "the remove carries no pose"
    );
}

/// `misordered reuse`: a removal with no live identity — a remove-first
/// envelope and a duplicated removal after the removal — rejects the whole
/// family before any output exists.
#[test]
fn misordered_or_duplicate_removal_rejects_whole_family() {
    let id = PassiveId::try_new(77).expect("checked passive identity");

    let mut remove_first = empty_parts();
    remove_first[2] = vec![envelope(1, 0, passive_key(id), passive_remove(id))];
    let error =
        assemble_hand_built(remove_first).expect_err("a removal without a live identity rejects");
    assert_eq!(error, ClientError::InvalidInput);

    let mut double_remove = empty_parts();
    double_remove[2] = vec![
        envelope(1, 0, passive_key(id), passive_upsert(id)),
        envelope(2, 0, passive_key(id), passive_remove(id)),
        envelope(3, 0, passive_key(id), passive_remove(id)),
    ];
    let error = assemble_hand_built(double_remove)
        .expect_err("a duplicated removal after the removal rejects");
    assert_eq!(error, ClientError::InvalidInput);
}

/// `kind/typed ID/dimension validation`: an envelope whose stable identity
/// disagrees with the record it wraps — a foreign kind tag or a foreign
/// dimension — rejects the whole family.
#[test]
fn envelope_identity_disagreeing_with_record_rejects() {
    let passive = PassiveId::try_new(88).expect("checked passive identity");
    let hostile = HostileId::try_new(88).expect("checked hostile identity");

    let mut foreign_kind = empty_parts();
    foreign_kind[1] = vec![envelope(
        1,
        0,
        StableRecordKey::Actor {
            kind: ActorKind::Hostile,
            dimension: ActorDimension::Known(Dimension::OVERWORLD),
            id: ActorId::Hostile(hostile),
        },
        passive_upsert(passive),
    )];
    let error =
        assemble_hand_built(foreign_kind).expect_err("an envelope naming another kind rejects");
    assert_eq!(error, ClientError::InvalidInput);

    let mut foreign_dimension = empty_parts();
    foreign_dimension[2] = vec![envelope(
        1,
        0,
        StableRecordKey::Actor {
            kind: ActorKind::Passive,
            dimension: ActorDimension::Known(Dimension::DEPTHS),
            id: ActorId::Passive(passive),
        },
        passive_upsert(passive),
    )];
    let error = assemble_hand_built(foreign_dimension)
        .expect_err("an envelope naming another dimension rejects");
    assert_eq!(error, ClientError::InvalidInput);
}

/// `coherent epoch and revision`: parts projected for one epoch and
/// candidate revision assemble only under exactly that identity — a foreign
/// epoch is stale and a foreign revision is incoherent, both without any
/// partial output.
#[test]
fn foreign_epoch_or_revision_rejects_without_partial_output() {
    let mut provider = admitted_mirror();
    let player = player_id(0x41);
    commit(
        &mut provider,
        remote_spawn_event(player, "Cora", 30, [7.0, 68.0, 7.0]),
    );

    let fixture = view_fixture();
    let view = fixture.view_of(&provider);
    let parts = provider_parts(&view);
    let limits = ClientLimits::try_new().expect("limits");

    let stale = assemble_actors(
        SessionEpoch::try_new(EPOCH + 1).expect("foreign epoch"),
        view.frame_revision(),
        parts.clone(),
        &limits,
    )
    .expect_err("a part from another epoch is stale");
    assert_eq!(stale, ClientError::StaleEpoch);

    let incoherent = assemble_actors(
        view.frame_epoch(),
        ConfirmedRevision::new(view.frame_revision().get() + 1),
        parts,
        &limits,
    )
    .expect_err("a part rebased onto another revision is incoherent");
    assert_eq!(incoherent, ClientError::InvalidInput);
}

/// `family count cap plus one`: an assembly at exactly the frozen family
/// record count publishes, one record more is a typed capacity rejection,
/// and the caller's retained previous output stays exactly as it was.
#[test]
fn family_count_cap_plus_one_retains_old_output() {
    let mut provider = admitted_mirror();
    let first = PassiveId::try_new(61).expect("checked passive identity");
    let second = PassiveId::try_new(62).expect("checked passive identity");
    let third = PassiveId::try_new(63).expect("checked passive identity");
    let fourth = PassiveId::try_new(64).expect("checked passive identity");
    commit(
        &mut provider,
        passive_spawn_event(30, first, [1.0, 64.0, 1.0]),
    );
    commit(
        &mut provider,
        passive_spawn_event(31, second, [2.0, 64.0, 2.0]),
    );
    commit(
        &mut provider,
        passive_spawn_event(32, third, [3.0, 64.0, 3.0]),
    );

    let fixture = view_fixture();
    let frozen = ClientLimits::try_new().expect("frozen limits");
    let at_cap = limits_with(3, frozen.frame_bytes());
    let old = assemble(&fixture, &provider, &at_cap)
        .expect("exactly three records fit the family count cap");
    assert_eq!(old.len(), 3);

    commit(
        &mut provider,
        passive_spawn_event(33, fourth, [4.0, 64.0, 4.0]),
    );
    let error = assemble(&fixture, &provider, &at_cap)
        .expect_err("one record past the family count cap rejects");
    assert_eq!(error, ClientError::Capacity);
    assert_eq!(old.len(), 3, "the retained previous output is unchanged");
    assert_eq!(old[2].id(), &ActorId::Passive(third));
}

/// `frame byte cap plus one`: an assembly whose minimal actors-only frame
/// exactly fits the frame byte cap publishes, one record more is a typed
/// capacity rejection, and the retained previous output stays intact.
#[test]
fn frame_byte_cap_plus_one_retains_old_output() {
    let mut provider = admitted_mirror();
    let first = drop_id(11);
    let second = drop_id(12);
    let third = drop_id(13);
    let fourth = drop_id(14);
    commit(&mut provider, drop_upserts_event(40, first));
    commit(&mut provider, drop_upserts_event(41, second));
    commit(&mut provider, drop_upserts_event(42, third));

    let fixture = view_fixture();
    let frozen = ClientLimits::try_new().expect("frozen limits");
    let old = assemble(&fixture, &provider, &frozen)
        .expect("three drop records assemble under the frozen caps");
    assert_eq!(old.len(), 3);

    let exact_cap = minimal_frame_size(&old);
    let at_cap = limits_with(frozen.family_records(), exact_cap);
    let fitted = assemble(&fixture, &provider, &at_cap)
        .expect("the exact minimal actors frame fits its own byte cap");
    assert_eq!(fitted.len(), 3);

    commit(&mut provider, drop_upserts_event(43, fourth));
    let error = assemble(&fixture, &provider, &at_cap)
        .expect_err("one record past the frame byte cap rejects");
    assert_eq!(error, ClientError::Capacity);
    assert_eq!(fitted.len(), 3, "the retained previous output is unchanged");
    assert_eq!(fitted[2].id(), &ActorId::Drop(third));
}
