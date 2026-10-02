//! The confirmed mirror cases: the sole owner of confirmed-revision
//! increments and actor identity resolution.
//!
//! The table drives the mirror under test through one shared driver surface —
//! commit one complete checked server packet for the current epoch and read
//! the confirmed mirror back — so the same assertions run against the real
//! mirror provider and, in the wrong-behavior artifact at the bottom, the
//! contract double it replaces. Every rejection row also asserts the whole
//! prior mirror is unchanged: a duplicate, backward, malformed, old-epoch or
//! old-generation observation never publishes a partial mirror or a partial
//! revision.

use crate::support::{DoubleMode, ReplayHarness};
use mornlea_client_core::ClientIdentity;
use mornlea_client_core::contracts::{
    ClientError, ClientLimits, ClientWorkBudget, ConfirmedRevision, ObservationKey, SessionEpoch,
    StepReport,
};
use mornlea_client_core::presentation::{
    AcceptedObservation, ActorId, ActorKind, PublicationConsumption, ResolvedActor,
};
use mornlea_client_core::session::mirror::MirrorProvider;
use mornlea_client_core::session::{ConfirmedMirror, ConfirmedMirrorParts};
use mornlea_domain::{
    ChunkPos, CompanionId, CraftingSize, Dimension, DropId, HostileId, PassiveId, PlayerId,
    ProjectileId,
};
use mornlea_protocol::{
    BlockChange, BlockChanges, CHAT_EVENT_REJECTED, CHAT_REJECT_INVALID_FORMAT, CHEST_SLOTS,
    CONTAINER_KIND_CHEST, CONTAINER_KIND_FURNACE, CRAFTING_GRID_SIZE_PERSONAL, ChatEvent,
    ChestState, ChunkSnapshot, CompanionDespawn, CompanionSpawn, CompanionState, CompanionStates,
    ContainerClosed, ContainerRef, CraftingState, ForgetChunks, FurnaceState,
    HOSTILE_KIND_NIGHTWALKER, HostileDespawn, HostileSpawn, HostileSpawnRecord, HostileState,
    HostileStateRecord, ItemDrop, ItemDropRemoves, ItemDropUpserts, ItemStack, KeepAlive,
    LoginStart, MIN_Y, PASSIVE_DESPAWN_VANISHED, PROJECTILE_KIND_SHARD, PassiveDespawn,
    PassiveDespawnRecord, PassiveSpawn, PassiveSpawnRecord, PassiveState, PassiveStateRecord,
    PlayerState, ProjectileDespawn, ProjectileSpawn, ProjectileSpawnRecord, ProjectileState,
    ProjectileStateRecord, RemotePlayerDespawn, RemotePlayerSpawn, RemotePlayerState,
    RemotePlayerStates, SectionData, ServerPacket,
};

/// Air, the empty single-storage section block; the registered numbering
/// lives in `mornlea_protocol::block` beside the world geometry.
const BLOCK_AIR: u16 = 0;
/// Stone, the fixture block of the frozen golden payloads.
const BLOCK_STONE: u16 = 2;

/// Which mirror implementation the table drives.
enum Driver {
    /// The contract landing's deterministic double, whose mirror behavior is
    /// the recorded wrong behavior the real provider replaces.
    Double,
    /// The real confirmed mirror provider.
    Provider,
}

/// The mirror implementation the named cases run against. The red run of
/// this table drove `Double` — the only mirror behavior that existed at the
/// contract landing — and its recorded wrong behavior is kept executable by
/// the artifact case below.
const DRIVER: Driver = Driver::Provider;

/// The mirror under test beside its deterministic session harness.
struct UnderTest {
    driver: Driver,
    harness: ReplayHarness,
    epoch: SessionEpoch,
    provider: Option<MirrorProvider>,
    /// The login-style staged-observation ordinal counter.
    staged: u32,
}

impl UnderTest {
    fn new() -> Self {
        match DRIVER {
            Driver::Double => Self::double(),
            Driver::Provider => Self::provider(),
        }
    }

    /// The contract double's mirror path: the staged-observation and step
    /// surfaces of the landed deterministic consumer.
    fn double() -> Self {
        let mut harness = ReplayHarness::new(DoubleMode::Contract).expect("harness");
        let epoch = harness
            .connect(identity("mirror", 3))
            .expect("pending epoch");
        harness.double.admit().expect("admitted double");
        Self {
            driver: Driver::Double,
            harness,
            epoch,
            provider: None,
            staged: 0,
        }
    }

    /// The real mirror provider over its own epoch, admitted before play.
    fn provider() -> Self {
        let harness = ReplayHarness::new(DoubleMode::Contract).expect("harness");
        let epoch = SessionEpoch::try_new(1).expect("provider epoch");
        let mut provider = MirrorProvider::new(epoch, ClientLimits::try_new().expect("limits"))
            .expect("pending mirror");
        provider.admit().expect("admitted mirror");
        Self {
            driver: Driver::Provider,
            harness,
            epoch,
            provider: Some(provider),
            staged: 0,
        }
    }

    /// Commits one complete checked server packet for the session's own
    /// epoch and reports the confirmed revision the commit produced.
    fn commit(&mut self, packet: ServerPacket) -> Result<ConfirmedRevision, ClientError> {
        self.commit_from_epoch(self.epoch, packet)
    }

    /// Commits one complete checked server packet staged under `epoch`, which
    /// the old-epoch row uses to present a foreign epoch's observation.
    fn commit_from_epoch(
        &mut self,
        epoch: SessionEpoch,
        packet: ServerPacket,
    ) -> Result<ConfirmedRevision, ClientError> {
        match self.driver {
            Driver::Double => {
                self.harness
                    .double
                    .stage_observation(epoch, packet)
                    .expect("staged");
                let report: StepReport = self
                    .harness
                    .step(epoch, ClientWorkBudget::try_new(1, 0).expect("budget"))?;
                Ok(report.confirmed_revision())
            }
            Driver::Provider => {
                let staged = self.stage(epoch, packet);
                self.staged += 1;
                self.provider
                    .as_mut()
                    .expect("provider driver")
                    .commit(&staged)
            }
        }
    }

    /// Builds one structurally staged observation the way the login provider
    /// stages decoded play packets: the next expected revision, the FIFO
    /// ordinal, no tick and no resolution entries.
    fn stage(&self, epoch: SessionEpoch, packet: ServerPacket) -> AcceptedObservation {
        let provider = self.provider.as_ref().expect("provider driver");
        let next = provider.mirror().revision().get() + 1;
        let key =
            ObservationKey::try_new(epoch, ConfirmedRevision::new(next), self.staged).expect("key");
        AcceptedObservation::try_new(key, None, packet, Vec::new()).expect("staged")
    }

    /// The confirmed mirror of the implementation under test.
    fn mirror(&self) -> ConfirmedMirror {
        match self.driver {
            Driver::Double => self.harness.double.mirror().expect("mirror").clone(),
            Driver::Provider => self
                .provider
                .as_ref()
                .expect("provider driver")
                .mirror()
                .clone(),
        }
    }

    fn revision(&self) -> u64 {
        self.mirror().revision().get()
    }
}

fn identity(name: &str, byte: u8) -> ClientIdentity {
    let login = LoginStart::new(player(byte), name, 8).expect("login");
    ClientIdentity::try_new(login).expect("identity")
}

fn player(byte: u8) -> PlayerId {
    PlayerId::try_from_bytes([0, 0, 0, 0, 0, 0, 0x40, 0, 0x80, 0, 0, 0, 0, 0, 0, byte])
        .expect("uuid v4")
}

fn companion(byte: u8) -> CompanionId {
    CompanionId::try_from_bytes([0, 0, 0, 0, 0, 0, 0x40, 0, 0x80, 0, 0, 0, 0, 0, 0, byte])
        .expect("uuid v4")
}

/// Asserts one rejected commit left the whole prior mirror unchanged.
fn assert_rejected_unchanged(test: &mut UnderTest, before: ConfirmedMirror, packet: ServerPacket) {
    let error = test
        .commit(packet)
        .expect_err("the malformed observation is rejected");
    assert_eq!(error, ClientError::InvalidInput);
    assert_eq!(
        test.mirror(),
        before,
        "the prior accepted state is preserved"
    );
}

/// `mirror::snapshot_block_change_forget`: a snapshot establishes one chunk
/// at its own content revision, a block-change batch continues exactly that
/// history, and a forget retires the chunk — one confirmed revision per
/// complete observation throughout.
#[test]
fn snapshot_block_change_forget() {
    let mut test = UnderTest::new();
    assert_eq!(test.revision(), 0, "no observation before play");

    let revision = test
        .commit(snapshot(Dimension::OVERWORLD, 1, 2, 7))
        .expect("the snapshot establishes the chunk");
    assert_eq!(revision.get(), 1);
    assert_eq!(
        test.mirror().world().chunks(),
        &chunk_map(vec![((1, 2), 7)]),
        "the chunk is held at the snapshot revision"
    );

    let revision = test
        .commit(block_changes(Dimension::OVERWORLD, 1, 2, 7, BLOCK_STONE))
        .expect("the batch continues the stored history");
    assert_eq!(revision.get(), 2);
    assert_eq!(
        test.mirror().world().chunks(),
        &chunk_map(vec![((1, 2), 8)]),
        "the stored content revision is exactly the batch's new revision"
    );

    let revision = test
        .commit(forget(Dimension::OVERWORLD, vec![(1, 2)]))
        .expect("the forget retires the chunk");
    assert_eq!(revision.get(), 3);
    assert_eq!(
        test.mirror().world().chunk_count(),
        0,
        "the forgotten chunk is no longer confirmed"
    );
}

/// `mirror::actor_spawn_update_despawn_reuse`: every actor kind walks spawn,
/// update, despawn and reuse; removal precedes reuse of the same stable
/// identity, and a spawn naming an already-live identity rejects.
#[test]
fn actor_spawn_update_despawn_reuse() {
    let mut test = UnderTest::new();

    // Remote players: a dimensioned spawn, a dimensioned state update, a
    // dimensionless despawn, then reuse of the same identity.
    let peer = player(9);
    test.commit(remote_spawn(peer, 5))
        .expect("the peer becomes visible");
    assert_eq!(
        test.mirror().actors().remote_players(),
        &[(peer, Dimension::OVERWORLD)]
    );
    test.commit(remote_states(6, peer, Dimension::OVERWORLD))
        .expect("the peer update matches the confirmed identity");
    test.commit(remote_despawn(peer))
        .expect("the peer despawn resolves one live identity");
    assert_eq!(test.mirror().actors().remote_players(), &[]);
    test.commit(remote_spawn(peer, 9))
        .expect("removal precedes reuse of the same identity");
    assert_eq!(
        test.mirror().actors().remote_players(),
        &[(peer, Dimension::OVERWORLD)]
    );

    // Companions: overworld dimensioned spawn, state update, despawn, reuse.
    let buddy = companion(11);
    test.commit(companion_spawn(buddy, 5))
        .expect("companion visible");
    assert_eq!(
        test.mirror().actors().companions(),
        &[(buddy, Dimension::OVERWORLD)]
    );
    test.commit(companion_states(6, buddy))
        .expect("the companion update matches the confirmed identity");
    test.commit(companion_despawn(buddy))
        .expect("the companion despawn resolves one live identity");
    assert_eq!(test.mirror().actors().companions(), &[]);
    test.commit(companion_spawn(buddy, 9))
        .expect("removal precedes reuse of the same companion identity");
    assert_eq!(
        test.mirror().actors().companions(),
        &[(buddy, Dimension::OVERWORLD)]
    );

    // Hostiles: dimensioned spawn, dimensionless state, dimensionless
    // despawn, then reuse of the same stable id.
    let hostile = HostileId::try_new(1).expect("nonzero");
    test.commit(hostile_spawn(7, hostile))
        .expect("hostile visible");
    assert_eq!(
        test.mirror().actors().hostiles(),
        &[(hostile, Dimension::OVERWORLD)]
    );
    test.commit(hostile_state(8, hostile))
        .expect("the dimensionless state resolves one live identity");
    test.commit(hostile_despawn(9, hostile))
        .expect("the dimensionless despawn resolves one live identity");
    assert_eq!(test.mirror().actors().hostiles(), &[]);
    test.commit(hostile_spawn(10, hostile))
        .expect("removal precedes reuse of the same hostile identity");
    assert_eq!(
        test.mirror().actors().hostiles(),
        &[(hostile, Dimension::OVERWORLD)]
    );

    // Passives: the same walk with a despawn reason record.
    let passive = PassiveId::try_new(2).expect("nonzero");
    test.commit(passive_spawn(7, passive))
        .expect("passive visible");
    test.commit(passive_state(8, passive))
        .expect("the dimensionless state resolves one live identity");
    test.commit(passive_despawn(9, passive))
        .expect("the dimensionless despawn resolves one live identity");
    assert_eq!(test.mirror().actors().passives(), &[]);
    test.commit(passive_spawn(10, passive))
        .expect("removal precedes reuse of the same passive identity");

    // Projectiles: the same walk for the third dimensionless kind.
    let projectile = ProjectileId::try_new(3).expect("nonzero");
    test.commit(projectile_spawn(7, projectile))
        .expect("projectile visible");
    test.commit(projectile_state(8, projectile))
        .expect("the dimensionless state resolves one live identity");
    test.commit(projectile_despawn(9, projectile))
        .expect("the dimensionless despawn resolves one live identity");
    assert_eq!(test.mirror().actors().projectiles(), &[]);
    test.commit(projectile_spawn(10, projectile))
        .expect("removal precedes reuse of the same projectile identity");

    // Drops: an upsert adds or wholly replaces, a remove retires, and a
    // later upsert reuses the same identity.
    let drop = DropId::try_new(0, ChunkPos::new(1, 2), 3, 1).expect("drop identity");
    test.commit(drop_upserts(11, drop)).expect("drop visible");
    assert_eq!(test.mirror().actors().drops(), &[drop]);
    test.commit(drop_upserts(12, drop))
        .expect("a repeated upsert replaces, never duplicates");
    assert_eq!(test.mirror().actors().drops(), &[drop]);
    test.commit(drop_removes(13, drop))
        .expect("the remove names one live identity");
    assert_eq!(test.mirror().actors().drops(), &[]);
    test.commit(drop_upserts(14, drop))
        .expect("removal precedes reuse of the same drop identity");
    assert_eq!(test.mirror().actors().drops(), &[drop]);

    // A spawn naming an already-live identity is not reuse: it rejects
    // without a despawn first, and the live identity is preserved.
    let before = test.mirror();
    assert_rejected_unchanged(&mut test, before, hostile_spawn(15, hostile));
}

/// `mirror::duplicate_backwards_and_control_unchanged`: a control-family
/// packet is not a semantic observation, so it rejects and preserves the
/// prior accepted state; the driver-level duplicate and backward staged
/// revisions are pinned by the provider's own key row below.
#[test]
fn duplicate_backwards_and_control_unchanged() {
    let mut test = UnderTest::new();
    test.commit(snapshot(Dimension::OVERWORLD, 0, 0, 1))
        .expect("one complete observation");
    let before = test.mirror();
    assert_eq!(before.revision().get(), 1);

    // A keepalive is a transport fact, never a semantic observation.
    let error = test
        .commit(ServerPacket::KeepAlive(
            KeepAlive::new(44).expect("keepalive"),
        ))
        .expect_err("the control family is not a semantic observation");
    assert_eq!(error, ClientError::InvalidInput);
    assert_eq!(
        test.mirror(),
        before,
        "the prior accepted state is preserved"
    );
}

/// `mirror::tickless_forget_increments_once_without_tick`: a `ForgetChunks`
/// packet carries no source tick, and committing it increments the confirmed
/// revision exactly once without inventing one.
#[test]
fn tickless_forget_increments_once_without_tick() {
    let mut test = UnderTest::new();
    test.commit(snapshot(Dimension::OVERWORLD, 4, 5, 3))
        .expect("the chunk is established");
    assert_eq!(test.revision(), 1);

    let revision = test
        .commit(forget(Dimension::OVERWORLD, vec![(4, 5)]))
        .expect("the tickless forget commits");
    assert_eq!(revision.get(), 2, "exactly one increment");
    assert_eq!(
        test.revision(),
        2,
        "the increment is committed exactly once"
    );
    assert_eq!(test.mirror().world().chunk_count(), 0);
}

/// `mirror::reset_old_epoch_unchanged`: an observation staged under a foreign
/// epoch never enters this epoch's mirror, however valid its packet is.
#[test]
fn reset_old_epoch_unchanged() {
    let mut test = UnderTest::new();
    test.commit(snapshot(Dimension::OVERWORLD, 0, 0, 1))
        .expect("one complete observation");
    let before = test.mirror();

    let old = SessionEpoch::try_new(test.epoch.get() + 1).expect("foreign epoch");
    let error = test
        .commit_from_epoch(old, snapshot(Dimension::OVERWORLD, 8, 8, 1))
        .expect_err("an old-epoch observation never enters this mirror");
    assert_eq!(error, ClientError::StaleEpoch);
    assert_eq!(
        test.mirror(),
        before,
        "the prior accepted state is preserved"
    );
}

/// `mirror::identity_resolution_rejects_orphans`: a dimensionless state or
/// despawn packet that names no live typed identity rejects, and so does a
/// dimensioned state whose record dimension disagrees with the confirmed
/// identity's dimension.
#[test]
fn identity_resolution_rejects_orphans() {
    let mut test = UnderTest::new();
    let hostile = HostileId::try_new(1).expect("nonzero");
    test.commit(hostile_spawn(5, hostile))
        .expect("hostile visible");
    let drop = DropId::try_new(0, ChunkPos::new(0, 0), 1, 1).expect("drop identity");
    test.commit(drop_upserts(5, drop)).expect("drop visible");
    let peer = player(9);
    test.commit(remote_spawn(peer, 5)).expect("peer visible");
    let buddy = companion(11);
    test.commit(companion_spawn(buddy, 5))
        .expect("companion visible");
    let before = test.mirror();

    // Orphan dimensionless state and despawn records name no live identity.
    let stranger = HostileId::try_new(77).expect("nonzero");
    assert_rejected_unchanged(&mut test, before.clone(), hostile_state(6, stranger));
    assert_rejected_unchanged(&mut test, before.clone(), hostile_despawn(6, stranger));
    let unknown_drop = DropId::try_new(0, ChunkPos::new(0, 0), 2, 1).expect("drop identity");
    assert_rejected_unchanged(&mut test, before.clone(), drop_removes(6, unknown_drop));
    assert_rejected_unchanged(&mut test, before.clone(), remote_despawn(player(99)));
    assert_rejected_unchanged(&mut test, before.clone(), companion_despawn(companion(99)));

    // A dimensioned state record that disagrees with the confirmed
    // dimension of the live identity rejects.
    assert_rejected_unchanged(
        &mut test,
        before.clone(),
        remote_states(6, peer, Dimension::DEPTHS),
    );

    // A despawn of an already-despawned identity cannot resurrect it.
    test.commit(hostile_despawn(7, hostile))
        .expect("the live hostile despawns");
    let after_despawn = test.mirror();
    assert_rejected_unchanged(&mut test, after_despawn, hostile_despawn(8, hostile));
}

/// `mirror::old_generation_rejected`: a block-change batch that does not
/// continue the stored chunk history and a snapshot at or below the stored
/// content revision are old-generation observations, and a replaced
/// container generation cannot come back.
#[test]
fn old_generation_rejected() {
    let mut test = UnderTest::new();
    test.commit(snapshot(Dimension::OVERWORLD, 1, 1, 5))
        .expect("the chunk is established at revision 5");
    test.commit(block_changes(Dimension::OVERWORLD, 1, 1, 5, BLOCK_STONE))
        .expect("the batch continues the stored history");
    let before = test.mirror();
    assert_eq!(before.world().chunks(), &chunk_map(vec![((1, 1), 6)]));

    // The same batch again is an old-generation duplicate.
    assert_rejected_unchanged(
        &mut test,
        before.clone(),
        block_changes(Dimension::OVERWORLD, 1, 1, 5, BLOCK_STONE),
    );
    // A batch ahead of the stored history is not a continuation either.
    assert_rejected_unchanged(
        &mut test,
        before.clone(),
        block_changes(Dimension::OVERWORLD, 1, 1, 7, BLOCK_STONE),
    );
    // A snapshot at or below the stored content revision cannot replace it.
    assert_rejected_unchanged(
        &mut test,
        before.clone(),
        snapshot(Dimension::OVERWORLD, 1, 1, 6),
    );
    // A newer snapshot replaces the stored generation wholesale.
    test.commit(snapshot(Dimension::OVERWORLD, 1, 1, 40))
        .expect("a newer snapshot replaces the generation");
    assert_eq!(
        test.mirror().world().chunks(),
        &chunk_map(vec![((1, 1), 40)])
    );

    // A replaced container generation cannot come back: generation two
    // replaces generation one at the same position, and the retired
    // generation's state is an old-generation observation.
    let position = (3, 3, CONTAINER_KIND_CHEST, 4);
    test.commit(chest(position, 1))
        .expect("the first chest view");
    test.commit(chest(position, 2))
        .expect("the replaced chest view");
    let held = test.mirror();
    assert_eq!(
        held.inventory().container_count(),
        1,
        "exactly one view per container position"
    );
    assert_rejected_unchanged(&mut test, held, chest(position, 1));
}

/// `mirror::invented_dimension_rejected`: a block-change batch for a chunk
/// the mirror does not hold has no history to continue, so it rejects
/// instead of inventing world state in a dimension never established.
#[test]
fn invented_dimension_rejected() {
    let mut test = UnderTest::new();
    test.commit(snapshot(Dimension::OVERWORLD, 0, 0, 1))
        .expect("one overworld chunk");
    let before = test.mirror();

    assert_rejected_unchanged(
        &mut test,
        before.clone(),
        block_changes(Dimension::OVERWORLD, 9, 9, 1, BLOCK_STONE),
    );
    assert_rejected_unchanged(
        &mut test,
        before,
        block_changes(Dimension::DEPTHS, 0, 0, 1, BLOCK_STONE),
    );
}

/// `mirror::whole_observation_atomic`: a multi-record packet whose later
/// record names an unknown identity rejects as a whole — no partial mirror,
/// no partial revision.
#[test]
fn whole_observation_atomic() {
    let mut test = UnderTest::new();
    let known = HostileId::try_new(1).expect("nonzero");
    test.commit(hostile_spawn(5, known))
        .expect("hostile visible");
    let before = test.mirror();
    assert_eq!(before.revision().get(), 1);

    let unknown = HostileId::try_new(88).expect("nonzero");
    let mixed = HostileDespawn::new(6, vec![known, unknown]).expect("sorted ids");
    let error = test
        .commit(ServerPacket::HostileDespawn(mixed))
        .expect_err("the whole batch rejects");
    assert_eq!(error, ClientError::InvalidInput);
    assert_eq!(test.mirror(), before, "the known identity is still live");
    assert_eq!(
        test.mirror().actors().hostiles(),
        &[(known, Dimension::OVERWORLD)]
    );
}

/// `mirror::container_lifecycle`: a container view is confirmed at the
/// observation's revision, a matching close retires it, a close naming an
/// unheld container changes nothing, and a crafting view follows the same
/// confirmed-revision attribution.
#[test]
fn container_lifecycle() {
    let mut test = UnderTest::new();
    let position = (2, 2, CONTAINER_KIND_CHEST, 1);

    let revision = test
        .commit(chest(position, 4))
        .expect("the chest view is confirmed");
    assert_eq!(revision.get(), 1);
    let reference = domain_container_ref(position, 4);
    assert_eq!(
        test.mirror().inventory().container_revision(&reference),
        Some(ConfirmedRevision::new(1)),
        "the view is attributed to its confirmed revision"
    );

    // A close naming an unheld container is tolerated and changes nothing.
    let before = test.mirror();
    let revision = test
        .commit(closed((9, 9, CONTAINER_KIND_CHEST, 1), 7))
        .expect("an unrelated close changes no view");
    assert_eq!(revision.get(), 2);
    assert_eq!(
        test.mirror().inventory(),
        before.inventory(),
        "an unrelated close changes no confirmed view"
    );

    // The matching close retires the confirmed view.
    test.commit(closed(position, 4))
        .expect("the matching close retires the view");
    assert_eq!(
        test.mirror().inventory().container_revision(&reference),
        None,
        "the confirmed view is retired"
    );

    // A crafting view is attributed the same way, one view per size.
    let revision = test
        .commit(crafting(CRAFTING_GRID_SIZE_PERSONAL))
        .expect("the crafting view is confirmed");
    assert_eq!(revision.get(), 4);
    assert_eq!(
        test.mirror()
            .inventory()
            .crafting_revision(CraftingSize::Personal),
        Some(ConfirmedRevision::new(4)),
        "the crafting view carries its confirmed revision"
    );

    // A furnace view is confirmed beside the chest path, keyed by its own
    // reference.
    let furnace_position = (2, 3, CONTAINER_KIND_FURNACE, 0);
    let revision = test
        .commit(furnace(furnace_position, 2))
        .expect("the furnace view is confirmed");
    assert_eq!(revision.get(), 5);
    assert_eq!(
        test.mirror()
            .inventory()
            .container_revision(&domain_container_ref(furnace_position, 2)),
        Some(ConfirmedRevision::new(5)),
        "the furnace view carries its confirmed revision"
    );

    // One confirmed chat fact is kept in arrival order.
    let revision = test.commit(chat(21)).expect("the chat fact is confirmed");
    assert_eq!(revision.get(), 6);
    assert_eq!(
        test.mirror().world_ui().chat().len(),
        1,
        "the confirmed chat fact is kept"
    );
    assert_eq!(
        test.mirror().world_ui().chat()[0].event_id(),
        21,
        "the chat fact is preserved unchanged"
    );
}

/// `mirror::chat_window_bounded`: the confirmed chat store keeps exactly the
/// newest 32 accepted chat facts in source order — appending the 33rd evicts
/// the oldest, the measured chat ring capacity — and reset still clears the
/// store wholesale, because the controller replaces the mirror owner with a
/// fresh epoch whose chat store starts empty.
#[test]
fn chat_window_bounded() {
    let mut test = UnderTest::provider();
    for event_id in 1..=33u64 {
        test.commit(chat(event_id))
            .expect("every chat fact is confirmed");
    }
    let mirror = test.mirror();
    let retained = mirror.world_ui().chat();
    assert_eq!(
        retained.len(),
        32,
        "exactly the chat window capacity is kept"
    );
    let ids: Vec<u64> = retained.iter().map(|fact| fact.event_id()).collect();
    let newest: Vec<u64> = (2..=33).collect();
    assert_eq!(
        ids, newest,
        "the newest 32 facts in source order, the oldest evicted"
    );

    // Reset clears wholesale: a fresh epoch's mirror holds no chat facts, so
    // no retained window ever crosses an epoch boundary.
    let epoch = SessionEpoch::try_new(test.epoch.get() + 1).expect("fresh epoch");
    let mut fresh = MirrorProvider::new(epoch, ClientLimits::try_new().expect("limits"))
        .expect("pending mirror");
    fresh.admit().expect("admitted mirror");
    assert_eq!(
        fresh.mirror().world_ui().chat().len(),
        0,
        "a fresh epoch holds no confirmed chat facts"
    );
}

/// The wrong-behavior artifact: the contract double's mirror path — the
/// behavior the real provider replaces — keeps no chunk facts, commits
/// invented-dimension block deltas, commits stale duplicate deltas,
/// resurrects despawned identities and appends duplicate live identities.
#[test]
fn wrong_double_mirror_rejected() {
    // Missing world facts: the double's mirror holds no chunk at all.
    let mut test = UnderTest::double();
    test.commit(snapshot(Dimension::OVERWORLD, 1, 2, 7))
        .expect("the double accepts the snapshot");
    assert_eq!(
        test.mirror().world().chunk_count(),
        0,
        "the double keeps no chunk facts"
    );

    // Invented dimension: a delta for a never-established chunk commits.
    let mut test = UnderTest::double();
    let revision = test
        .commit(block_changes(Dimension::DEPTHS, 9, 9, 1, BLOCK_STONE))
        .expect("the double commits the invented-dimension delta");
    assert_eq!(revision.get(), 1, "the wrong double advances the revision");

    // Stale resurrection: the same despawn commits twice.
    let mut test = UnderTest::double();
    let hostile = HostileId::try_new(1).expect("nonzero");
    test.commit(hostile_spawn(5, hostile))
        .expect("the double accepts the spawn");
    test.commit(hostile_despawn(6, hostile))
        .expect("the double accepts the despawn");
    test.commit(hostile_despawn(7, hostile))
        .expect("the double commits the despawn again");

    // Partial mirror: a duplicate spawn appends the identity twice.
    let mut test = UnderTest::double();
    test.commit(hostile_spawn(5, hostile))
        .expect("the double accepts the spawn");
    test.commit(hostile_spawn(6, hostile))
        .expect("the double accepts the duplicate spawn");
    assert_eq!(
        test.mirror().actors().hostiles().len(),
        2,
        "the same live identity is appended twice"
    );
}

/// `mirror::observation_key_discipline_unchanged`: a staged key naming the
/// current revision (a duplicate), one behind it (backward) or one ahead of
/// the sequence (a gap) is a malformed observation identity; each rejects
/// and preserves the prior accepted state.
#[test]
fn observation_key_discipline_unchanged() {
    let mut test = UnderTest::provider();
    test.commit(snapshot(Dimension::OVERWORLD, 0, 0, 1))
        .expect("one complete observation");
    let before = test.mirror();

    for revision in [1u64, 0, 3] {
        let key =
            ObservationKey::try_new(test.epoch, ConfirmedRevision::new(revision), test.staged)
                .expect("key");
        let staged = AcceptedObservation::try_new(
            key,
            None,
            snapshot(Dimension::OVERWORLD, 8, 8, 1),
            Vec::new(),
        )
        .expect("staged");
        let provider = test.provider.as_mut().expect("provider driver");
        let error = provider
            .commit(&staged)
            .expect_err("the malformed staged key rejects");
        assert_eq!(error, ClientError::InvalidInput);
        assert_eq!(
            provider.mirror(),
            &before,
            "the prior accepted state is preserved"
        );
        assert_eq!(provider.observations().len(), 1, "nothing is retained");
    }
}

/// `mirror::unadmitted_mirror_rejects_commit`: nothing is confirmed before
/// the login exchange completes, and admission opens exactly once.
#[test]
fn unadmitted_mirror_rejects_commit() {
    let epoch = SessionEpoch::try_new(5).expect("epoch");
    let mut provider = MirrorProvider::new(epoch, ClientLimits::try_new().expect("limits"))
        .expect("pending mirror");
    let staged = AcceptedObservation::try_new(
        ObservationKey::try_new(epoch, ConfirmedRevision::new(1), 0).expect("key"),
        None,
        snapshot(Dimension::OVERWORLD, 0, 0, 1),
        Vec::new(),
    )
    .expect("staged");
    assert_eq!(
        provider.commit(&staged),
        Err(ClientError::InvalidState),
        "a pending mirror confirms nothing"
    );
    provider.admit().expect("admission opens the mirror");
    assert_eq!(
        provider.admit(),
        Err(ClientError::InvalidState),
        "admission happens exactly once"
    );
    assert_eq!(
        provider
            .commit(&staged)
            .expect("the admitted mirror commits")
            .get(),
        1
    );
}

/// `mirror::ambiguous_identity_rejected`: an adopted mirror that holds the
/// same live identity twice cannot resolve a dimensionless record to exactly
/// one identity, so the observation rejects without changing either entry.
#[test]
fn ambiguous_identity_rejected() {
    let epoch = SessionEpoch::try_new(6).expect("epoch");
    let hostile = HostileId::try_new(1).expect("nonzero");
    let actors = mornlea_client_core::session::ActorConfirmed::try_new()
        .expect("empty actors")
        .with_hostile(hostile, Dimension::OVERWORLD)
        .with_hostile(hostile, Dimension::DEPTHS);
    let mut provider = MirrorProvider::from_parts(
        ConfirmedMirrorParts {
            epoch,
            revision: ConfirmedRevision::new(0),
            phase: mornlea_client_core::contracts::SessionPhase::Admitted,
            world: None,
            actors: Some(actors),
            inventory: None,
            world_ui: None,
        },
        ClientLimits::try_new().expect("limits"),
    )
    .expect("adopted mirror");
    let before = provider.mirror().clone();
    let staged = AcceptedObservation::try_new(
        ObservationKey::try_new(epoch, ConfirmedRevision::new(1), 0).expect("key"),
        None,
        hostile_state(8, hostile),
        Vec::new(),
    )
    .expect("staged");
    assert_eq!(
        provider.commit(&staged),
        Err(ClientError::InvalidInput),
        "an ambiguous identity never resolves"
    );
    assert_eq!(
        &before,
        provider.mirror(),
        "both live entries are preserved"
    );
}

/// `mirror::projection_context_keys_ticks_resolution`: every committed
/// observation keeps its issued key, the packet's own optional tick and the
/// resolutions of its dimensionless records, in actual source order.
#[test]
fn projection_context_keys_ticks_resolution() {
    let mut test = UnderTest::provider();
    let hostile = HostileId::try_new(1).expect("nonzero");
    let drop = DropId::try_new(0, ChunkPos::new(0, 0), 1, 1).expect("drop identity");

    test.commit(snapshot(Dimension::OVERWORLD, 1, 1, 5))
        .expect("revision 1");
    test.commit(hostile_spawn(7, hostile)).expect("revision 2");
    test.commit(hostile_state(8, hostile)).expect("revision 3");
    test.commit(hostile_despawn(9, hostile))
        .expect("revision 4");
    test.commit(drop_upserts(10, drop)).expect("revision 5");
    test.commit(drop_removes(11, drop)).expect("revision 6");
    test.commit(forget(Dimension::OVERWORLD, vec![(1, 1)]))
        .expect("revision 7");
    test.commit(player_state(12)).expect("revision 8");

    let provider = test.provider.as_ref().expect("provider driver");
    let observations = provider.observations();
    assert_eq!(observations.len(), 8, "actual source order is retained");
    for (index, observation) in observations.iter().enumerate() {
        let key = ObservationKey::try_new(
            test.epoch,
            ConfirmedRevision::new(u64::try_from(index + 1).expect("revision")),
            u32::try_from(index).expect("ordinal"),
        )
        .expect("key");
        assert_eq!(*observation.key(), key, "the issued key is the staged key");
    }

    // The packet's own optional tick is preserved and never invented.
    assert_eq!(observations[1].source_tick(), Some(7), "the spawn tick");
    assert_eq!(observations[2].source_tick(), Some(8), "the state tick");
    assert_eq!(observations[3].source_tick(), Some(9), "the despawn tick");
    assert_eq!(observations[5].source_tick(), Some(11), "the remove tick");
    assert_eq!(
        observations[6].source_tick(),
        None,
        "a tickless forget invents no tick"
    );
    assert_eq!(
        observations[7].source_tick(),
        Some(12),
        "the private body observation keeps its tick"
    );

    // Dimensionless records resolve to exactly one live typed identity.
    let resolution = ResolvedActor::try_new(
        ActorKind::Hostile,
        ActorId::Hostile(hostile),
        Dimension::OVERWORLD,
    )
    .expect("resolution");
    assert_eq!(
        observations[2].resolved(),
        &[resolution],
        "the dimensionless state resolution"
    );
    assert_eq!(
        observations[3].resolved(),
        &[resolution],
        "the dimensionless despawn resolution"
    );
    for (index, observation) in observations.iter().enumerate() {
        if index != 2 && index != 3 {
            assert!(
                observation.resolved().is_empty(),
                "dimensioned and world observations resolve nothing"
            );
        }
    }
}

/// `mirror::bounded_history_and_consumption`: the committed observation
/// queue is bounded by the frozen inbound observation limit, a full queue
/// refuses before any mutation, and only the publication consumption cursor
/// retires consumed observations.
#[test]
fn bounded_history_and_consumption() {
    let epoch = SessionEpoch::try_new(7).expect("epoch");
    let frozen = ClientLimits::try_new().expect("frozen limits");
    let tight = ClientLimits::try_new_with(
        frozen.queued_input_events(),
        1,
        frozen.inbound_bytes(),
        frozen.outbound_commands(),
        frozen.outbound_bytes(),
        frozen.prediction_journal(),
        frozen.message_work(),
        frozen.mesh_work(),
        frozen.preparation_results(),
        frozen.preparation_bytes(),
        frozen.family_records(),
        frozen.frame_bytes(),
    )
    .expect("tighter configuration");
    let mut provider = MirrorProvider::new(epoch, tight).expect("pending mirror");
    provider.admit().expect("admitted mirror");

    let first = AcceptedObservation::try_new(
        ObservationKey::try_new(epoch, ConfirmedRevision::new(1), 0).expect("key"),
        None,
        snapshot(Dimension::OVERWORLD, 0, 0, 1),
        Vec::new(),
    )
    .expect("staged");
    assert_eq!(
        provider
            .commit(&first)
            .expect("the first observation commits")
            .get(),
        1
    );
    assert_eq!(provider.observations().len(), 1);

    let second = AcceptedObservation::try_new(
        ObservationKey::try_new(epoch, ConfirmedRevision::new(2), 1).expect("key"),
        None,
        snapshot(Dimension::OVERWORLD, 2, 2, 1),
        Vec::new(),
    )
    .expect("staged");
    let before = provider.mirror().clone();
    assert_eq!(
        provider.commit(&second),
        Err(ClientError::Capacity),
        "a full queue refuses before any mutation"
    );
    assert_eq!(&before, provider.mirror(), "the mirror is unchanged");
    assert_eq!(provider.observations().len(), 1, "nothing is retained");

    // The publication cursor retires exactly the consumed prefix.
    provider
        .consume(&PublicationConsumption::try_new(1, 0, 0, 0).expect("cursor"))
        .expect("consumed");
    assert_eq!(provider.observations().len(), 0);
    assert_eq!(
        provider.commit(&second).expect("the retry commits").get(),
        2,
        "the refused observation stays retry-owned"
    );

    // An over-long cursor consumes nothing.
    assert_eq!(
        provider.consume(&PublicationConsumption::try_new(2, 0, 0, 0).expect("cursor")),
        Err(ClientError::InvalidInput)
    );
    assert_eq!(provider.observations().len(), 1);
}

// --- fixtures ---

/// The overworld world-store key map of the given chunk positions.
fn chunk_map(chunks: Vec<((i32, i32), u64)>) -> std::collections::BTreeMap<(u8, i64, i64), u64> {
    let mut held = std::collections::BTreeMap::new();
    for ((x, z), revision) in chunks {
        held.insert((0u8, i64::from(x), i64::from(z)), revision);
    }
    held
}

fn snapshot(dimension: Dimension, chunk_x: i32, chunk_z: i32, revision: u64) -> ServerPacket {
    let sections = (0..24).map(|y| SectionData::single(y, BLOCK_AIR)).collect();
    ServerPacket::ChunkSnapshot(
        ChunkSnapshot::new(dimension, chunk_x, chunk_z, revision, sections).expect("snapshot"),
    )
}

fn block_changes(
    dimension: Dimension,
    chunk_x: i32,
    chunk_z: i32,
    base_revision: u64,
    block: u16,
) -> ServerPacket {
    ServerPacket::BlockChanges(
        BlockChanges::new(
            dimension,
            chunk_x,
            chunk_z,
            base_revision,
            base_revision + 1,
            vec![BlockChange {
                x: chunk_x << 4,
                y: MIN_Y,
                z: chunk_z << 4,
                block,
            }],
        )
        .expect("block changes"),
    )
}

fn forget(dimension: Dimension, chunks: Vec<(i32, i32)>) -> ServerPacket {
    ServerPacket::ForgetChunks(ForgetChunks::new(dimension, chunks).expect("forget"))
}

/// One private body observation with a known tick; every field sits at its
/// empty or boundary-safe value so the packet's own validation admits it.
fn player_state(tick: u64) -> ServerPacket {
    ServerPacket::PlayerState(
        PlayerState::new(
            tick,
            0,
            Dimension::OVERWORLD,
            [0.0, 64.0, 0.0],
            [0.0, 0.0, 0.0],
            0.0,
            0.0,
            true,
            false,
            false,
            false,
            mornlea_protocol::BlockPos::ZERO,
            0,
            0,
            false,
            20,
            0,
            0,
            false,
            0,
            0,
            0,
            0,
            0,
            0,
            0,
        )
        .expect("player state"),
    )
}

fn remote_spawn(peer: PlayerId, tick: u64) -> ServerPacket {
    ServerPacket::RemotePlayerSpawn(
        RemotePlayerSpawn::new(
            peer,
            "pilot".to_string(),
            tick,
            Dimension::OVERWORLD,
            [0.0, 64.0, 0.0],
            0.0,
            0.0,
        )
        .expect("remote spawn"),
    )
}

fn remote_states(tick: u64, peer: PlayerId, dimension: Dimension) -> ServerPacket {
    ServerPacket::RemotePlayerStates(
        RemotePlayerStates::new(
            tick,
            vec![RemotePlayerState {
                player_id: peer,
                dimension,
                position: [0.0, 64.0, 0.0],
                yaw: 0.0,
                pitch: 0.0,
                reset: false,
            }],
        )
        .expect("remote states"),
    )
}

fn remote_despawn(peer: PlayerId) -> ServerPacket {
    ServerPacket::RemotePlayerDespawn(RemotePlayerDespawn::new(peer))
}

fn companion_spawn(buddy: CompanionId, tick: u64) -> ServerPacket {
    ServerPacket::CompanionSpawn(
        CompanionSpawn::new(
            buddy,
            "buddy".to_string(),
            tick,
            Dimension::OVERWORLD,
            [1.0, 64.0, 1.0],
            0.0,
            0.0,
        )
        .expect("companion spawn"),
    )
}

fn companion_states(tick: u64, buddy: CompanionId) -> ServerPacket {
    ServerPacket::CompanionStates(
        CompanionStates::new(
            tick,
            vec![CompanionState {
                companion_id: buddy,
                dimension: Dimension::OVERWORLD,
                position: [1.0, 64.0, 1.0],
                yaw: 0.0,
                pitch: 0.0,
                reset: false,
            }],
        )
        .expect("companion states"),
    )
}

fn companion_despawn(buddy: CompanionId) -> ServerPacket {
    ServerPacket::CompanionDespawn(CompanionDespawn::new(buddy))
}

fn hostile_spawn(tick: u64, id: HostileId) -> ServerPacket {
    ServerPacket::HostileSpawn(
        HostileSpawn::new(
            tick,
            vec![HostileSpawnRecord {
                id,
                dimension: Dimension::OVERWORLD,
                position: [2.0, 64.0, 2.0],
                yaw: 0.0,
                health: 20,
                kind: HOSTILE_KIND_NIGHTWALKER,
            }],
        )
        .expect("hostile spawn"),
    )
}

fn hostile_state(tick: u64, id: HostileId) -> ServerPacket {
    ServerPacket::HostileState(
        HostileState::new(
            tick,
            vec![HostileStateRecord {
                id,
                position: [2.0, 64.0, 2.0],
                velocity: [0.0, 0.0, 0.0],
                yaw: 0.0,
                health: 19,
                kind: HOSTILE_KIND_NIGHTWALKER,
            }],
        )
        .expect("hostile state"),
    )
}

fn hostile_despawn(tick: u64, id: HostileId) -> ServerPacket {
    ServerPacket::HostileDespawn(HostileDespawn::new(tick, vec![id]).expect("hostile despawn"))
}

fn passive_spawn(tick: u64, id: PassiveId) -> ServerPacket {
    ServerPacket::PassiveSpawn(
        PassiveSpawn::new(
            tick,
            vec![PassiveSpawnRecord {
                id,
                dimension: Dimension::OVERWORLD,
                position: [3.0, 64.0, 3.0],
                yaw: 0.0,
                health: 10,
            }],
        )
        .expect("passive spawn"),
    )
}

fn passive_state(tick: u64, id: PassiveId) -> ServerPacket {
    ServerPacket::PassiveState(
        PassiveState::new(
            tick,
            vec![PassiveStateRecord {
                id,
                position: [3.0, 64.0, 3.0],
                velocity: [0.0, 0.0, 0.0],
                yaw: 0.0,
                health: 9,
                grazing: 1,
            }],
        )
        .expect("passive state"),
    )
}

fn passive_despawn(tick: u64, id: PassiveId) -> ServerPacket {
    ServerPacket::PassiveDespawn(
        PassiveDespawn::new(
            tick,
            vec![PassiveDespawnRecord {
                id,
                reason: PASSIVE_DESPAWN_VANISHED,
            }],
        )
        .expect("passive despawn"),
    )
}

fn projectile_spawn(tick: u64, id: ProjectileId) -> ServerPacket {
    ServerPacket::ProjectileSpawn(
        ProjectileSpawn::new(
            tick,
            vec![ProjectileSpawnRecord {
                id,
                kind: PROJECTILE_KIND_SHARD,
                dimension: Dimension::OVERWORLD,
                position: [4.0, 64.0, 4.0],
                velocity: [0.0, 0.0, 0.0],
            }],
        )
        .expect("projectile spawn"),
    )
}

fn projectile_state(tick: u64, id: ProjectileId) -> ServerPacket {
    ServerPacket::ProjectileState(
        ProjectileState::new(
            tick,
            vec![ProjectileStateRecord {
                id,
                position: [4.0, 64.0, 4.0],
            }],
        )
        .expect("projectile state"),
    )
}

fn projectile_despawn(tick: u64, id: ProjectileId) -> ServerPacket {
    ServerPacket::ProjectileDespawn(
        ProjectileDespawn::new(tick, vec![id]).expect("projectile despawn"),
    )
}

fn drop_upserts(tick: u64, id: DropId) -> ServerPacket {
    ServerPacket::ItemDropUpserts(
        ItemDropUpserts::new(
            tick,
            vec![ItemDrop {
                id,
                block_index: 0,
                item: 1,
                count: 1,
                durability: 0,
            }],
        )
        .expect("drop upserts"),
    )
}

fn drop_removes(tick: u64, id: DropId) -> ServerPacket {
    ServerPacket::ItemDropRemoves(ItemDropRemoves::new(tick, vec![id]).expect("drop removes"))
}

/// One chest view for a `(chunk_x, chunk_z, kind, slot)` position and
/// generation.
fn chest(position: (i32, i32, u8, u8), generation: u32) -> ServerPacket {
    ServerPacket::ChestState(
        ChestState::new(
            container_ref(position, generation),
            [ItemStack::EMPTY; CHEST_SLOTS],
        )
        .expect("chest state"),
    )
}

/// One idle furnace view for a position and generation.
fn furnace(position: (i32, i32, u8, u8), generation: u32) -> ServerPacket {
    ServerPacket::FurnaceState(
        FurnaceState::new(
            container_ref(position, generation),
            ItemStack::EMPTY,
            ItemStack::EMPTY,
            ItemStack::EMPTY,
            0,
            0,
        )
        .expect("furnace state"),
    )
}

/// One confirmed chat fact: a malformed-command rejection, the outcome branch
/// that addresses no companion.
fn chat(event_id: u64) -> ServerPacket {
    let event = ChatEvent {
        event_id,
        player_id: player(3),
        player_name: "pilot".to_string(),
        companion_id: [0; 16],
        companion_name: String::new(),
        kind: CHAT_EVENT_REJECTED,
        reject_reason: CHAT_REJECT_INVALID_FORMAT,
        command: String::new(),
        speech: String::new(),
    };
    ServerPacket::ChatEvent(ChatEvent::new(event).expect("chat event"))
}

fn closed(position: (i32, i32, u8, u8), generation: u32) -> ServerPacket {
    ServerPacket::ContainerClosed(
        ContainerClosed::new(container_ref(position, generation)).expect("closed"),
    )
}

fn crafting(size: u8) -> ServerPacket {
    ServerPacket::CraftingState(
        CraftingState::new(size, [ItemStack::EMPTY; 9], ItemStack::EMPTY).expect("crafting"),
    )
}

fn container_ref(position: (i32, i32, u8, u8), generation: u32) -> ContainerRef {
    ContainerRef {
        dimension: 0,
        chunk_x: position.0,
        chunk_z: position.1,
        kind: position.2,
        slot: position.3,
        generation,
    }
}

/// The checked domain reference of one wire container position, which is what
/// the confirmed inventory store keys its views by.
fn domain_container_ref(
    position: (i32, i32, u8, u8),
    generation: u32,
) -> mornlea_domain::ContainerRef {
    container_ref(position, generation)
        .to_domain_present()
        .expect("present container reference")
}
