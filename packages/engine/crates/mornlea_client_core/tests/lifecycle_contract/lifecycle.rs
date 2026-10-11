//! The lifecycle contract cases: repeatable Reset/Invalidate/local-close
//! projections, queue/journal/preparation invalidation and the generation
//! rules of one pure-core connection generation.
//!
//! The table drives the real lifecycle owner over the real epoch-scoped
//! consumer owners — the real admission path with its journal, outbound
//! queue, local cue sources and view-validity overlay, the real audio dedup
//! state, the real pending lifecycle projection state, the real preparation
//! queue — and the real login provider for the pending-login and terminal
//! rows. Every publication goes through the real serial assembly
//! transaction, so the staged lifecycle records are consumed exactly once by
//! the same commit that swaps the visible frame. The clocks and transports
//! are the deterministic doubles; no case sleeps or touches a host.

use std::num::NonZeroU64;
use std::sync::Arc;
use std::time::Duration;

use crate::support::{
    DeterministicClock, MemoryConnectorDouble, frame_server_packet, registry_with_memory,
};
use mornlea_client_core::ClientEndpoint;
use mornlea_client_core::contracts::{
    ClientConfig, ClientError, ClientIdentity, ClientLimits, ClientWorkBudget, CloseReason,
    ConfirmedRevision, Connector, Endpoint, FAMILY_LIFECYCLE, FAMILY_SESSION, FamilyKey,
    FamilyOperation, InputReceipt, ObservationKey, PreparationPort, RecordHeader, SessionEpoch,
    SessionPhase, TransportPoll, TransportTicket,
};
use mornlea_client_core::input::{
    ClientIntent, ContainerToken, InputAction, InputAdmissionState, InputBatch, InputTranslator,
};
use mornlea_client_core::preparation::{
    LodStep, OwnedLodRequest, PreparationJob, PreparationPayload, PreparationQueue,
    PreparedResourceKey, TerrainKey, TilePos,
};
use mornlea_client_core::presentation::assembly::{
    commit_publication, prepare_publication, publication_owners,
};
use mornlea_client_core::presentation::frame::{
    FamilyFrame, FamilyRecords, LifecycleRecord, LifecycleTransition, PresentationFrame,
    SessionRecord,
};
use mornlea_client_core::presentation::{
    AcceptedObservation, AudioDedupDelta, AudioDedupKey, AudioProjectionState, CueId,
    InputAdmissionOwner, LifecycleProjectionState, PublicationConsumption, ResourceKey,
};
use mornlea_client_core::session::io::MemoryConnector;
use mornlea_client_core::session::lifecycle::{
    LifecycleInvalidation, LifecycleOwner, ResetOutcome,
};
use mornlea_client_core::session::login::LoginSession;
use mornlea_client_core::session::{ConfirmedMirror, ConfirmedMirrorParts, InventoryConfirmed};
use mornlea_domain::{
    ChatBody, ChatEvent, ChatEventParts, ChunkPos, ContainerKind, ContainerRef, Dimension,
    DisplayName, HotbarSlot, Identities, LookAngles, PlayerId,
};
use mornlea_engine::native::contracts::world::{Materials, WorldgenParams};
use mornlea_protocol::{ClientHello, LoginStart, ServerPacket, write_frame};

/// The core-owned resource release order every lifecycle record's
/// `Invalidate` half carries: consumers before providers.
const CORE_ORDER: [ResourceKey; 3] = [
    ResourceKey::InputJournal,
    ResourceKey::PreparationQueue,
    ResourceKey::PresentationFrames,
];

/// The accepted hello/login deadline policy the replay configuration freezes.
const HELLO_TIMEOUT: Duration = Duration::from_secs(5);
const LOGIN_TIMEOUT: Duration = Duration::from_secs(10);

// --- checked-value helpers ---

fn epoch(value: u64) -> SessionEpoch {
    SessionEpoch::try_new(value).expect("nonzero epoch")
}

fn limits() -> ClientLimits {
    ClientLimits::try_new().expect("frozen limits")
}

fn player(byte: u8) -> PlayerId {
    PlayerId::try_from_bytes([0, 0, 0, 0, 0, 0, 0x40, 0, 0x80, 0, 0, 0, 0, 0, 0, byte])
        .expect("uuid v4")
}

fn identity(name: &str, byte: u8) -> ClientIdentity {
    ClientIdentity::try_new(LoginStart::new(player(byte), name, 8).expect("login"))
        .expect("identity")
}

fn memory_endpoint() -> Endpoint {
    Endpoint::Memory {
        connector_id: NonZeroU64::new(1).expect("one"),
    }
}

fn budget() -> ClientWorkBudget {
    ClientWorkBudget::try_new(4, 0).expect("budget")
}

fn look(yaw: f32, pitch: f32) -> LookAngles {
    LookAngles::try_new(yaw, pitch).expect("finite fixture look")
}

fn collect_water() -> InputAction {
    InputAction {
        intent: ClientIntent::CollectWater(look(0.0, 0.0)),
        container: None,
        crafting: None,
    }
}

fn select_hotbar() -> InputAction {
    InputAction {
        intent: ClientIntent::SelectHotbar(HotbarSlot::new(0).expect("slot")),
        container: None,
        crafting: None,
    }
}

fn drop_item() -> InputAction {
    InputAction {
        intent: ClientIntent::DropSelectedItem,
        container: None,
        crafting: None,
    }
}

/// An admitted mirror for one epoch with no confirmed stores.
fn admitted_mirror(value: u64) -> ConfirmedMirror {
    ConfirmedMirror::try_new(ConfirmedMirrorParts {
        epoch: epoch(value),
        revision: ConfirmedRevision::new(0),
        phase: SessionPhase::Admitted,
        world: None,
        actors: None,
        inventory: None,
        world_ui: None,
    })
    .expect("admitted mirror")
}

/// An admitted mirror that confirms one external chest view at revision
/// zero, the attribution a `CloseContainer` token requires.
fn mirror_with_container(value: u64, reference: ContainerRef) -> ConfirmedMirror {
    ConfirmedMirror::try_new(ConfirmedMirrorParts {
        epoch: epoch(value),
        revision: ConfirmedRevision::new(0),
        phase: SessionPhase::Admitted,
        world: None,
        actors: None,
        inventory: Some(
            InventoryConfirmed::try_new()
                .expect("inventory store")
                .with_container_view(reference, ConfirmedRevision::new(0)),
        ),
        world_ui: None,
    })
    .expect("admitted mirror with container view")
}

fn chat_packet(event_id: u64) -> ServerPacket {
    let event = ChatEvent::try_new(ChatEventParts {
        event_id,
        player_id: player(3),
        player_name: DisplayName::try_from_canonical("pilot".to_string())
            .expect("canonical fixture name"),
        body: ChatBody::InvalidFormat,
    })
    .expect("chat event");
    ServerPacket::try_from(mornlea_domain::Event::Chat(event)).expect("chat packet")
}

/// One staged accepted observation of a chat packet at one epoch's next key.
fn chat_observation(value: u64, ordinal: u32) -> AcceptedObservation {
    let key = ObservationKey::try_new(epoch(value), ConfirmedRevision::new(1), ordinal)
        .expect("observation key");
    AcceptedObservation::try_new(key, None, chat_packet(7), Vec::new()).expect("observation")
}

/// One committed local audio dedup key of one epoch.
fn audio_key(value: u64, local_event_sequence: u64) -> AudioDedupKey {
    AudioDedupKey::Local {
        epoch: epoch(value),
        local_event_sequence,
        cue: CueId::try_new(4).expect("water splash cue"),
    }
}

/// The shared worldgen parameters the far-LOD preparation jobs reference.
fn far_params() -> Arc<WorldgenParams> {
    let materials = Materials {
        air: 0,
        stone: 1,
        dirt: 2,
        grass: 3,
        bedrock: 4,
        snow: 5,
        sand: 6,
        clay: 7,
        gravel: 8,
        iron_ore: 9,
        coal_ore: 10,
        oak_log: 11,
        leaves: 12,
        water: 13,
        short_grass: 14,
    };
    let mut perm = [0u8; 512];
    for (index, entry) in perm.iter_mut().enumerate() {
        *entry = (index % 256) as u8;
    }
    Arc::new(WorldgenParams::try_new(1, materials, perm).expect("params"))
}

/// One far-LOD preparation job of one epoch at one tile.
fn far_job(value: u64, job_id: u64, tile: [i32; 2]) -> PreparationJob {
    let key = PreparedResourceKey::try_new(
        epoch(value),
        Dimension::OVERWORLD,
        TerrainKey::LodTile(TilePos::new(tile[0], tile[1])),
        1,
        5,
        job_id,
    )
    .expect("checked far key");
    let request = OwnedLodRequest::try_new(far_params(), tile, LodStep::Four).expect("lod request");
    PreparationJob::try_new(key, PreparationPayload::Far(request)).expect("far job")
}

/// The client's current-version hello frame, the exact bytes a pending
/// epoch sends.
fn hello_frame_bytes() -> Vec<u8> {
    let hello = ClientHello::new(Identities::current().protocol).expect("current hello");
    write_frame(
        ClientHello::PACKET_ID,
        &hello.encode().expect("hello payload"),
    )
    .expect("hello frame")
}

/// The bootstrap visible frame of one epoch: the checked pending-connection
/// publication at index zero the controller holds before the first
/// transaction.
fn bootstrap(value: u64) -> Arc<PresentationFrame> {
    let header = RecordHeader::try_new(
        epoch(value),
        ConfirmedRevision::new(0),
        None,
        FamilyOperation::Upsert,
    )
    .expect("bootstrap header");
    let frame = PresentationFrame::try_new(
        epoch(value),
        ConfirmedRevision::new(0),
        0,
        vec![
            FamilyFrame::try_new(
                FamilyKey::try_new(FAMILY_SESSION).expect("session key"),
                FamilyRecords::Session(vec![
                    SessionRecord::try_new(header, SessionPhase::Connecting, None, None)
                        .expect("bootstrap session record"),
                ]),
            )
            .expect("bootstrap family"),
        ],
    )
    .expect("bootstrap frame");
    frame.validate(&limits()).expect("bootstrap validates");
    Arc::new(frame)
}

/// The lifecycle records of one frame in family order.
fn lifecycle_records(frame: &PresentationFrame) -> Vec<LifecycleRecord> {
    let mut records = Vec::new();
    for family in frame.families() {
        if let FamilyRecords::Lifecycle(family_records) = family.records() {
            records.extend(family_records.iter().cloned());
        }
    }
    records
}

/// Publishes one candidate carrying the pending lifecycle records verbatim
/// at revision zero through the real serial assembly transaction, consuming
/// exactly the pending records. The controller role the wiring will hold.
fn publish(
    value: u64,
    next_index: u64,
    phase: SessionPhase,
    visible: &mut Arc<PresentationFrame>,
    observations: &mut Vec<AcceptedObservation>,
    admission: &mut InputAdmissionState,
    audio: &mut AudioProjectionState,
    lifecycle: &mut LifecycleProjectionState,
) -> Arc<PresentationFrame> {
    let owned = lifecycle.pending().to_vec();
    let header = RecordHeader::try_new(
        epoch(value),
        ConfirmedRevision::new(0),
        None,
        FamilyOperation::Upsert,
    )
    .expect("session header");
    let mut families = vec![
        FamilyFrame::try_new(
            FamilyKey::try_new(FAMILY_SESSION).expect("session key"),
            FamilyRecords::Session(vec![
                SessionRecord::try_new(header, phase, None, None).expect("session record"),
            ]),
        )
        .expect("session family"),
    ];
    if !owned.is_empty() {
        families.push(
            FamilyFrame::try_new(
                FamilyKey::try_new(FAMILY_LIFECYCLE).expect("lifecycle key"),
                FamilyRecords::Lifecycle(owned),
            )
            .expect("lifecycle family"),
        );
    }
    let candidate = PresentationFrame::try_new(
        epoch(value),
        ConfirmedRevision::new(0),
        next_index,
        families,
    )
    .expect("candidate frame");
    let mut input_owner =
        InputAdmissionOwner::try_new(admission.projection().clone()).expect("input owner");
    let consume =
        PublicationConsumption::try_new(0, 0, 0, lifecycle.pending().len()).expect("cursors");
    let delta = AudioDedupDelta::try_new(Vec::new(), Vec::new()).expect("empty delta");
    let reservation = {
        let bundle = publication_owners(visible, observations, &mut input_owner, audio, lifecycle)
            .expect("owners bundle");
        prepare_publication(&bundle, candidate, delta, consume, &limits())
            .expect("the lifecycle candidate reserves")
    };
    let mut bundle = publication_owners(visible, observations, &mut input_owner, audio, lifecycle)
        .expect("owners bundle");
    commit_publication(&mut bundle, reservation)
}

// --- the controller-side owner stack one pure-core run drives ---

/// The real epoch-scoped owners beside the real lifecycle owner and one
/// deterministic memory transport: the admission owner, the audio dedup
/// state, the pending lifecycle projection state, the staged observation
/// queue and the real preparation queue.
struct Stack {
    connector: Arc<MemoryConnectorDouble>,
    owner: LifecycleOwner,
    admission: InputAdmissionState,
    audio: AudioProjectionState,
    lifecycle: LifecycleProjectionState,
    observations: Vec<AcceptedObservation>,
    preparation: PreparationQueue,
    visible: Arc<PresentationFrame>,
    mirror: Option<ConfirmedMirror>,
    current: Option<SessionEpoch>,
    ticket: Option<TransportTicket>,
    job_id: u64,
}

impl Stack {
    fn new() -> Self {
        Self::with_connector(Arc::new(MemoryConnectorDouble::new()))
    }

    fn with_connector(connector: Arc<MemoryConnectorDouble>) -> Self {
        Self {
            connector,
            owner: LifecycleOwner::try_new(limits()).expect("lifecycle owner"),
            admission: InputAdmissionState::try_new(epoch(1), limits()).expect("admission owner"),
            audio: AudioProjectionState::try_new().expect("audio state"),
            lifecycle: LifecycleProjectionState::try_new(1).expect("lifecycle state"),
            observations: Vec::new(),
            preparation: PreparationQueue::new(limits()),
            visible: bootstrap(1),
            mirror: None,
            current: None,
            ticket: None,
            job_id: 0,
        }
    }

    /// Acquires a fresh transport and opens the next epoch through the real
    /// owner, publishing the epoch's Open record through the real
    /// transaction.
    fn connect(&mut self) -> SessionEpoch {
        let ticket = self
            .connector
            .try_connect(&memory_endpoint(), &identity("cycle", 3))
            .expect("transport ticket");
        let value = self
            .owner
            .open(ticket, &mut self.admission, &mut self.lifecycle)
            .expect("fresh epoch");
        assert!(
            self.lifecycle.pending().len() == 1
                && self.lifecycle.pending()[0].transition() == LifecycleTransition::Open,
            "the fresh epoch stages exactly one Open record"
        );
        self.visible = bootstrap(value.get());
        let committed = publish(
            value.get(),
            1,
            SessionPhase::Admitted,
            &mut self.visible,
            &mut self.observations,
            &mut self.admission,
            &mut self.audio,
            &mut self.lifecycle,
        );
        assert!(
            self.lifecycle.pending().is_empty(),
            "the Open publication consumes its record exactly once"
        );
        assert_eq!(committed.frame_index(), 1);
        self.mirror = Some(admitted_mirror(value.get()));
        self.current = Some(value);
        self.ticket = Some(ticket);
        value
    }

    /// The controller-lending invalidation bundle over the stack's owners.
    fn invalidation(&mut self) -> LifecycleInvalidation<'_> {
        LifecycleInvalidation::try_new(
            &mut self.observations,
            &mut self.admission,
            &mut self.audio,
            &mut self.lifecycle,
        )
        .expect("invalidation bundle")
    }

    /// Runs the real reset projection over the stack's owners.
    fn reset(&mut self) -> ResetOutcome {
        let value = self.current.expect("live epoch");
        let outcome = {
            let Self {
                owner,
                connector,
                admission,
                audio,
                lifecycle,
                observations,
                preparation,
                ..
            } = self;
            let bundle = LifecycleInvalidation::try_new(observations, admission, audio, lifecycle)
                .expect("invalidation bundle");
            owner
                .reset(value, connector.as_ref(), bundle, preparation)
                .expect("reset projection")
        };
        self.current = None;
        self.ticket = None;
        self.mirror = None;
        outcome
    }

    /// Runs the real local-close projection over the stack's owners.
    fn close(&mut self) -> Vec<LifecycleRecord> {
        let value = self.current.expect("live epoch");
        let Self {
            owner,
            connector,
            admission,
            audio,
            lifecycle,
            observations,
            preparation,
            ..
        } = self;
        let bundle = LifecycleInvalidation::try_new(observations, admission, audio, lifecycle)
            .expect("invalidation bundle");
        owner
            .close(value, connector.as_ref(), bundle, preparation)
            .expect("local close")
    }

    /// The refused reset of one epoch: its typed error, with nothing changed.
    fn reset_refused(&mut self, epoch: SessionEpoch) -> Option<ClientError> {
        let Self {
            owner,
            connector,
            admission,
            audio,
            lifecycle,
            observations,
            preparation,
            ..
        } = self;
        let bundle = LifecycleInvalidation::try_new(observations, admission, audio, lifecycle)
            .expect("invalidation bundle");
        owner
            .reset(epoch, connector.as_ref(), bundle, preparation)
            .err()
    }

    /// Runs the real reset projection over the stack's owners against one
    /// caller-named transport, for the cases whose transport is the real
    /// Memory capability rather than the stack's own double.
    fn reset_over(&mut self, connector: &dyn Connector) -> ResetOutcome {
        let value = self.current.expect("live epoch");
        let outcome = {
            let Self {
                owner,
                admission,
                audio,
                lifecycle,
                observations,
                preparation,
                ..
            } = self;
            let bundle = LifecycleInvalidation::try_new(observations, admission, audio, lifecycle)
                .expect("invalidation bundle");
            owner
                .reset(value, connector, bundle, preparation)
                .expect("reset projection")
        };
        self.current = None;
        self.ticket = None;
        self.mirror = None;
        outcome
    }

    /// Publishes the dying epoch's staged records through the real
    /// transaction at the dying epoch identity.
    fn publish_dying(&mut self, dying: SessionEpoch, next_index: u64) -> Arc<PresentationFrame> {
        publish(
            dying.get(),
            next_index,
            SessionPhase::Admitted,
            &mut self.visible,
            &mut self.observations,
            &mut self.admission,
            &mut self.audio,
            &mut self.lifecycle,
        )
    }

    /// One real whole-batch admission through the real validate/commit path.
    fn submit(&mut self, actions: Vec<InputAction>) -> InputReceipt {
        let value = self.current.expect("live epoch");
        let mirror = self.mirror.clone().expect("admitted mirror");
        let batch = InputBatch::try_new(value, actions).expect("batch");
        let validated =
            InputTranslator::validate_batch(&batch, &mirror, &limits()).expect("validated batch");
        InputTranslator::commit(validated, &mut self.admission).expect("admitted batch")
    }

    /// Submits one far-LOD preparation job of the current epoch.
    fn submit_far_job(&mut self, tile: [i32; 2]) -> PreparedResourceKey {
        self.job_id += 1;
        let value = self.current.expect("live epoch");
        let job = far_job(value.get(), self.job_id, tile);
        let key = *job.key();
        self.preparation.try_submit(job).expect("admitted job");
        key
    }
}

// --- the named cases ---

/// `lifecycle::old_work_cannot_cross_reset`: one reset invalidates every
/// epoch-scoped consumer before the next epoch exists. The old epoch's
/// journal, outbound queue, cue sources, overlay tombstones, staged
/// observations, committed audio keys, preparation work and transport
/// ticket are all gone or typed-dead, and the old epoch's input cannot
/// enter the new epoch's owners.
#[test]
fn old_work_cannot_cross_reset() {
    let mut stack = Stack::new();
    let first = stack.connect();
    let old_ticket = stack.ticket.expect("live ticket");

    // Real admitted work in every epoch-scoped owner: two sequenced actions
    // (one emitting a native local cue), a locally closed container view,
    // one staged observation and one committed audio dedup key.
    let chest = ContainerRef::try_new(ChunkPos::new(0, 0), ContainerKind::Chest, 0, 1)
        .expect("chest reference");
    stack.mirror = Some(mirror_with_container(first.get(), chest));
    stack.submit(vec![collect_water(), select_hotbar()]);
    let token =
        ContainerToken::try_new(first, chest, ConfirmedRevision::new(0)).expect("container token");
    stack.submit(vec![InputAction {
        intent: ClientIntent::CloseContainer,
        container: Some(token),
        crafting: None,
    }]);
    stack.observations.push(chat_observation(first.get(), 0));
    stack.audio = AudioProjectionState::try_new()
        .expect("audio state")
        .with_committed(vec![audio_key(first.get(), 1)]);

    // Preparation work in all three queue states: delivered and retained,
    // completed but undelivered, and queue-held pending.
    let retained_key = {
        stack.submit_far_job([0, 0]);
        stack
            .preparation
            .work(ClientWorkBudget::try_new(0, 4).expect("budget"));
        let result = stack.preparation.poll_ready().expect("delivered result");
        *result.key()
    };
    stack.submit_far_job([1, 0]);
    stack
        .preparation
        .work(ClientWorkBudget::try_new(0, 4).expect("budget"));
    stack.submit_far_job([2, 0]);
    assert_eq!(stack.preparation.retained_resources(), 1);
    assert_eq!(stack.preparation.completed_results(), 1);
    assert_eq!(stack.preparation.pending_jobs(), 1);
    assert!(stack.preparation.prepared_resource(&retained_key).is_ok());

    // Pre-reset facts the invalidation must erase.
    assert_eq!(stack.admission.journal_entries(), 3);
    assert_eq!(stack.admission.outbound_records(), 3);
    assert!(stack.admission.overlay().is_closed(&chest));
    assert_eq!(stack.admission.pending_local_cues().len(), 1);

    // The reset projection.
    let outcome = stack.reset();
    assert_eq!(outcome.dying(), first);
    let records = outcome.records();
    assert_eq!(records.len(), 2, "the ordered reset pair");
    assert_eq!(records[0].transition(), LifecycleTransition::Reset);
    assert!(records[0].resource_order().is_empty());
    assert_eq!(records[1].transition(), LifecycleTransition::Invalidate);
    assert_eq!(records[1].resource_order(), CORE_ORDER);
    assert_eq!(records[0].generation(), first.get());
    assert_eq!(records[1].generation(), first.get());
    assert_eq!(
        outcome.preparation().jobs_cancelled(),
        1,
        "the queue-held job cancelled"
    );
    assert_eq!(
        outcome.preparation().results_stale(),
        1,
        "the undelivered result released"
    );
    assert!(
        outcome.preparation().bytes_released() > 0,
        "the retained geometry's bytes released"
    );

    // Every consumer invalidated before the new epoch.
    assert!(stack.observations.is_empty());
    assert_eq!(stack.admission.journal_entries(), 0);
    assert_eq!(stack.admission.outbound_records(), 0);
    assert_eq!(stack.admission.outbound_bytes(), 0);
    assert_eq!(stack.admission.next_sequence(), 1);
    assert!(stack.admission.overlay().tombstones().is_empty());
    assert_eq!(stack.admission.pending_local_cues().len(), 0);
    assert!(stack.audio.committed().is_empty());
    assert!(stack.audio.pending_cancellations().is_empty());
    assert_eq!(stack.preparation.retained_resources(), 0);
    assert_eq!(stack.preparation.completed_results(), 0);
    assert_eq!(stack.preparation.pending_jobs(), 0);
    assert_eq!(stack.preparation.charge(), 0);
    assert_eq!(stack.connector.releases(), 1, "release exactly once");

    // The old epoch's identities are typed-dead: the retained resource and
    // the old ticket can never re-enter.
    assert_eq!(
        stack.preparation.prepared_resource(&retained_key),
        Err(ClientError::StaleEpoch)
    );
    assert!(matches!(
        stack.connector.poll(old_ticket),
        TransportPoll::Closed(_)
    ));
    assert_eq!(
        stack.connector.try_send(old_ticket, &hello_frame_bytes()),
        Err(ClientError::InvalidState)
    );

    // The dying pair publishes exactly once through the real transaction.
    let dying = stack.publish_dying(first, 2);
    let published = lifecycle_records(&dying);
    assert_eq!(published.len(), 2);
    assert_eq!(published[0].transition(), LifecycleTransition::Reset);
    assert_eq!(published[1].transition(), LifecycleTransition::Invalidate);
    assert_eq!(published[1].resource_order(), CORE_ORDER);
    assert!(stack.lifecycle.pending().is_empty());
    assert_eq!(
        stack.connector.releases(),
        1,
        "publication releases nothing"
    );

    // The new epoch is strictly next and the old epoch's input cannot enter
    // its owners.
    let second = stack.connect();
    assert_eq!(second.get(), first.get() + 1);
    let stale = InputBatch::try_new(first, vec![drop_item()]).expect("stale batch");
    assert_eq!(
        InputTranslator::validate_batch(
            &stale,
            stack.mirror.as_ref().expect("admitted mirror"),
            &limits()
        )
        .err(),
        Some(ClientError::StaleEpoch),
        "old-epoch work cannot cross the reset"
    );
    let receipt = stack.submit(vec![drop_item()]);
    assert_eq!(
        receipt,
        InputReceipt::Queued {
            epoch: second,
            first_sequence: Some(1),
            sequenced_count: 1,
            chat_count: 0,
        },
        "the new epoch's sequence space restarted at one"
    );
}

/// `lifecycle::pending_login_reset`: a reset while the login exchange is
/// still pending releases the pending transport exactly once, drops the old
/// epoch's deadlines and retained frames, and the fresh epoch runs under its
/// own deadlines only. Driven over the real bounded Memory transport.
#[test]
fn pending_login_reset() {
    let (connector, peer) = MemoryConnector::pair(limits(), NonZeroU64::new(1).expect("one"));
    let registry = {
        let mut registry = mornlea_client_core::contracts::ConnectorRegistry::new();
        registry
            .register(NonZeroU64::new(1).expect("one"), connector.clone())
            .expect("fresh registry");
        Arc::new(registry)
    };
    let clock = Arc::new(DeterministicClock::new());
    let config = ClientConfig::try_new(
        limits(),
        HELLO_TIMEOUT,
        LOGIN_TIMEOUT,
        clock.clone(),
        registry,
    )
    .expect("checked config");
    let mut session = LoginSession::new(config).expect("login provider");
    let first = session
        .connect(memory_endpoint(), identity("pending", 5))
        .expect("pending epoch");
    assert_eq!(peer.drain_sent(), vec![hello_frame_bytes()]);

    // The server hello admits the handshake and the login start goes out;
    // no login answer ever arrives, so the epoch stays pending.
    peer.feed(&hello_frame_bytes()).expect("server hello fed");
    let report = session.step(first, budget()).expect("handshake step");
    assert!(report.terminal().is_none());
    assert_eq!(peer.drain_sent().len(), 1, "the login start went out");

    // The pending epoch refuses input: nothing is admitted before the login
    // exchange completes.
    assert_eq!(
        session
            .submit_input(
                first,
                InputBatch::try_new(first, vec![drop_item()]).expect("batch")
            )
            .err(),
        Some(ClientError::InvalidState)
    );

    // Reset at t=3s: the pending transport is released exactly once and the
    // fresh epoch is strictly next.
    clock.advance(Duration::from_secs(3));
    let second = session.reset(first).expect("fresh epoch");
    assert_eq!(second.get(), first.get() + 1);
    let stale_ticket = TransportTicket::try_new(
        NonZeroU64::new(1).expect("one"),
        NonZeroU64::new(1).expect("one"),
    )
    .expect("old ticket shape");
    assert!(matches!(
        connector.poll(stale_ticket),
        TransportPoll::Closed(_)
    ));
    assert_eq!(
        connector.close(stale_ticket),
        Err(ClientError::InvalidState),
        "the released pending transport cannot release twice"
    );

    // The old epoch is dead to the provider and the new epoch steps under
    // its own hello deadline, which began at the reset.
    assert_eq!(
        session.step(first, budget()).err(),
        Some(ClientError::StaleEpoch)
    );
    let report = session.step(second, budget()).expect("fresh step");
    assert!(report.terminal().is_none());
    assert_eq!(
        peer.drain_sent(),
        vec![hello_frame_bytes()],
        "exactly the fresh epoch's hello"
    );

    // The old epoch's boundaries (hello t=5s, login t=10s) must not reach
    // the fresh epoch, whose own hello deadline began at the reset (t=8s).
    clock.advance(Duration::from_millis(4_500));
    let report = session.step(second, budget()).expect("still connecting");
    assert!(
        report.terminal().is_none(),
        "the old epoch's deadlines cannot terminate the fresh epoch"
    );

    // The fresh epoch's own hello deadline does terminate it, once.
    clock.advance(Duration::from_millis(500));
    let report = session.step(second, budget()).expect("terminal step");
    assert_eq!(report.terminal(), Some(&CloseReason::Timeout));
    assert_eq!(
        session.step(second, budget()).err(),
        Some(ClientError::Disconnected),
        "terminal exactly once"
    );
}

/// `lifecycle::reconnect_after_terminal`: after a terminal, the dead epoch
/// can neither step, reset nor submit; the real lifecycle owner reconnects
/// with continued epoch numbering, the new epoch sequences from one, a local
/// close is terminal exactly once and a repeated close succeeds.
#[test]
fn reconnect_after_terminal() {
    let connector = Arc::new(MemoryConnectorDouble::new());
    let clock = Arc::new(DeterministicClock::new());
    let config = ClientConfig::try_new(
        limits(),
        HELLO_TIMEOUT,
        LOGIN_TIMEOUT,
        clock,
        registry_with_memory(connector.clone()),
    )
    .expect("checked config");
    let mut session = LoginSession::new(config).expect("login provider");
    let first = session
        .connect(memory_endpoint(), identity("reconnect", 6))
        .expect("pending epoch");
    connector.feed_frame(hello_frame_bytes());
    session.step(first, budget()).expect("handshake step");
    connector.feed_frame(frame_server_packet(&ServerPacket::LoginReject(
        mornlea_protocol::LoginReject::new(
            mornlea_protocol::LOGIN_SERVER_FULL,
            "server full".to_string(),
        )
        .expect("login reject"),
    )));
    let terminal = session.step(first, budget()).expect("terminal step");
    assert!(matches!(
        terminal.terminal(),
        Some(CloseReason::LoginRejected(_))
    ));
    assert_eq!(connector.releases(), 1, "one terminal release");

    // Terminal once: every further step and reset refuses typed, and no
    // second release happens.
    assert_eq!(
        session.step(first, budget()).err(),
        Some(ClientError::Disconnected)
    );
    assert_eq!(
        session.reset(first).err(),
        Some(ClientError::Disconnected),
        "a terminal epoch cannot reset"
    );
    assert_eq!(connector.releases(), 1);

    // The terminal pair published exactly once with the exact shape.
    let frame = session.snapshot(first).expect("terminal frame");
    let records = lifecycle_records(&frame);
    assert_eq!(records.len(), 2);
    assert_eq!(records[0].transition(), LifecycleTransition::Close);
    assert_eq!(records[1].transition(), LifecycleTransition::Invalidate);
    assert_eq!(records[1].resource_order(), CORE_ORDER);
    assert_eq!(records[0].generation(), first.get());

    // Reconnect: the lifecycle owner adopts the dead epoch's numbering —
    // terminal, because the provider already ended it — and the next epoch
    // continues it, so the dead epoch never repeats.
    let mut stack = Stack::with_connector(connector.clone());
    stack
        .owner
        .adopt(first, terminal.terminal().cloned())
        .expect("adopt the dead numbering");
    let second = stack.connect();
    assert_eq!(second.get(), first.get() + 1, "continued epoch numbering");

    // The dead epoch cannot enter the fresh one: its batch is stale and the
    // fresh epoch sequences from one.
    let stale = InputBatch::try_new(first, vec![drop_item()]).expect("stale batch");
    assert_eq!(
        InputTranslator::validate_batch(
            &stale,
            stack.mirror.as_ref().expect("admitted mirror"),
            &limits()
        )
        .err(),
        Some(ClientError::StaleEpoch)
    );
    assert_eq!(
        stack.submit(vec![drop_item()]),
        InputReceipt::Queued {
            epoch: second,
            first_sequence: Some(1),
            sequenced_count: 1,
            chat_count: 0,
        }
    );

    // Local close: terminal exactly once, one release, and the repeated
    // close succeeds without staging a second pair.
    let ticket = stack.ticket.expect("live ticket");
    let staged = stack.close();
    assert_eq!(staged.len(), 2);
    assert_eq!(staged[0].transition(), LifecycleTransition::Close);
    assert_eq!(staged[1].transition(), LifecycleTransition::Invalidate);
    assert_eq!(staged[1].resource_order(), CORE_ORDER);
    assert_eq!(staged[1].generation(), second.get());
    assert_eq!(connector.releases(), 2, "one release for the close");
    assert_eq!(stack.owner.terminal(), Some(&CloseReason::LocalClose));
    stack.close();
    assert_eq!(connector.releases(), 2, "no second release");
    assert_eq!(
        stack.lifecycle.pending().len(),
        2,
        "exactly one terminal pair staged"
    );
    assert_eq!(
        stack.connector.poll(ticket),
        TransportPoll::Closed(ClientError::Disconnected),
        "the closed transport reports the close"
    );
    // The terminal pair publishes through the real transaction before the
    // reconnect opens the next epoch.
    stack.publish_dying(second, 2);
    assert!(stack.lifecycle.pending().is_empty());

    // A terminal epoch cannot reset; the reconnect continues through open.
    assert_eq!(
        stack.reset_refused(second),
        Some(ClientError::Disconnected),
        "a terminal epoch cannot reset"
    );
    let third = stack.connect();
    assert_eq!(third.get(), second.get() + 1, "the reconnect continues");
}

/// `lifecycle::one_hundred_cycles`: one hundred pure-core open/work/reset
/// cycles leave zero retained stale work — every epoch-scoped owner is empty
/// after each reset and after the hundredth, the epoch numbering is strictly
/// monotonic, and the first cycle's artifacts are all typed-dead.
#[test]
fn one_hundred_cycles() {
    let mut stack = Stack::new();
    let mut last = None;
    for cycle in 1..=100u64 {
        let current = stack.connect();
        assert_eq!(
            current.get(),
            cycle,
            "strictly monotonic epoch numbering across cycles"
        );

        // Real work in every epoch-scoped owner each cycle.
        stack.submit(vec![collect_water(), drop_item()]);
        stack.observations.push(chat_observation(current.get(), 0));
        stack.audio = AudioProjectionState::try_new()
            .expect("audio state")
            .with_committed(vec![audio_key(current.get(), 1)]);
        let key = stack.submit_far_job([0, 0]);
        assert_eq!(stack.admission.journal_entries(), 2);
        assert_eq!(stack.observations.len(), 1);
        assert_eq!(stack.preparation.pending_jobs(), 1);

        let outcome = stack.reset();
        assert_eq!(outcome.dying(), current);
        assert_eq!(outcome.preparation().jobs_cancelled(), 1);

        // Zero retained stale work after every cycle, before the next epoch.
        assert!(stack.observations.is_empty());
        assert_eq!(stack.admission.journal_entries(), 0);
        assert_eq!(stack.admission.outbound_records(), 0);
        assert_eq!(stack.admission.pending_local_cues().len(), 0);
        assert!(stack.admission.overlay().tombstones().is_empty());
        assert!(stack.audio.committed().is_empty());
        assert!(stack.audio.pending_cancellations().is_empty());
        assert_eq!(stack.preparation.charge(), 0);
        // The dying pair is consumed by its real publication, so the next
        // epoch opens over an empty pending projection.
        stack.publish_dying(current, 2);
        assert!(stack.lifecycle.pending().is_empty());

        if cycle == 1 {
            // The first cycle's key is dead from the first invalidation on.
            assert_eq!(
                stack.preparation.prepared_resource(&key),
                Err(ClientError::StaleEpoch)
            );
        }
        last = Some(current);
    }
    let hundredth = last.expect("cycled");
    assert_eq!(hundredth.get(), 100);
    assert_eq!(stack.connector.releases(), 100, "one release per cycle");

    // The first cycle's artifacts are all typed-dead after one hundred
    // resets: its ticket, its epoch's input and its resource keys.
    let first_ticket = TransportTicket::try_new(
        NonZeroU64::new(1).expect("one"),
        NonZeroU64::new(1).expect("one"),
    )
    .expect("first ticket shape");
    assert!(matches!(
        stack.connector.poll(first_ticket),
        TransportPoll::Closed(_)
    ));
    let stale = InputBatch::try_new(epoch(1), vec![drop_item()]).expect("stale batch");
    assert_eq!(
        InputTranslator::validate_batch(&stale, &admitted_mirror(100), &limits()).err(),
        Some(ClientError::StaleEpoch)
    );
    let first_key = *far_job(1, 1, [9, 9]).key();
    assert_eq!(
        stack.preparation.prepared_resource(&first_key),
        Err(ClientError::StaleEpoch)
    );

    // The hundred-first epoch continues the numbering.
    let next = stack.connect();
    assert_eq!(next.get(), 101);
}

// --- the exhaustion and mid-flight reset cases ---

/// A reset while the transport holds an undelivered outbound record drops
/// the record with the released transport: the old frame can never reach the
/// new epoch's peer, and the admission owner's retained commands are gone.
#[test]
fn reset_during_retained_send_drops_old_frames() {
    let (connector, peer) = MemoryConnector::pair(limits(), NonZeroU64::new(1).expect("one"));
    let mut stack = Stack::new();

    // Open the first epoch over the real transport and queue one undelivered
    // outbound frame beside the admission owner's retained commands.
    let ticket = connector
        .try_connect(&memory_endpoint(), &identity("retained", 7))
        .expect("transport ticket");
    let first = stack
        .owner
        .open(ticket, &mut stack.admission, &mut stack.lifecycle)
        .expect("first epoch");
    stack.mirror = Some(admitted_mirror(first.get()));
    stack.current = Some(first);
    stack.ticket = Some(ticket);
    let receipt = stack.submit(vec![drop_item(), select_hotbar()]);
    assert!(matches!(receipt, InputReceipt::Queued { .. }));
    connector
        .try_send(ticket, &hello_frame_bytes())
        .expect("one queued outbound frame");
    assert_eq!(peer.sent_depth(), 1, "the frame waits for the peer");
    assert_eq!(stack.admission.outbound_records(), 2);

    // The reset releases the transport with the frame still queued.
    let outcome = stack.reset_over(connector.as_ref());
    assert_eq!(outcome.dying(), first);
    assert_eq!(stack.admission.outbound_records(), 0);
    // The dying projection is consumed before the next epoch opens.
    stack.publish_dying(first, 1);
    assert!(stack.lifecycle.pending().is_empty());

    // The old frame can never reach the new epoch's peer, and the old ticket
    // cannot send anything anywhere.
    let fresh = connector
        .try_connect(&memory_endpoint(), &identity("retained", 7))
        .expect("fresh transport");
    assert!(
        peer.drain_sent().is_empty(),
        "the retained old frame never reaches the new generation's peer"
    );
    assert_eq!(peer.sent_depth(), 0);
    assert_eq!(
        connector.try_send(ticket, &hello_frame_bytes()),
        Err(ClientError::InvalidState)
    );
    assert_eq!(
        connector.close(ticket),
        Err(ClientError::InvalidState),
        "the reset already released the old transport exactly once"
    );

    // The new epoch's transport carries only what the new epoch sends.
    let second = stack
        .owner
        .open(fresh, &mut stack.admission, &mut stack.lifecycle)
        .expect("second epoch");
    assert_eq!(second.get(), first.get() + 1);
    let fresh_frame = frame_server_packet(&chat_packet(7));
    connector
        .try_send(fresh, &fresh_frame)
        .expect("the new epoch sends");
    assert_eq!(peer.drain_sent(), vec![fresh_frame]);
}

/// A reset while a receive is fragmented drops the partial frame with the
/// released transport: the fragment never completes, never leaks into the
/// new epoch's receive state, and only complete frames through the new
/// transport can publish.
#[test]
fn reset_during_fragment_never_leaks_into_new_epoch() {
    let (connector, peer) = MemoryConnector::pair(limits(), NonZeroU64::new(1).expect("one"));
    let ticket = connector
        .try_connect(&memory_endpoint(), &identity("fragment", 8))
        .expect("transport ticket");
    let mut stack = Stack::new();
    let first = stack
        .owner
        .open(ticket, &mut stack.admission, &mut stack.lifecycle)
        .expect("first epoch");
    stack.mirror = Some(admitted_mirror(first.get()));
    stack.current = Some(first);
    stack.ticket = Some(ticket);

    // Half of one server frame: the receive retains the fragment and no
    // partial observation surfaces.
    let frame = frame_server_packet(&chat_packet(7));
    let half = frame.len() / 2;
    peer.feed(&frame[..half]).expect("fragment staged");
    assert_eq!(
        connector.poll(ticket),
        TransportPoll::Connected,
        "establishment first"
    );
    assert_eq!(
        connector.poll(ticket),
        TransportPoll::Pending,
        "a fragment never surfaces"
    );

    // The reset drops the fragment with the transport.
    let outcome = stack.reset_over(connector.as_ref());
    assert_eq!(outcome.dying(), first);
    assert_eq!(
        connector.poll(ticket),
        TransportPoll::Closed(ClientError::InvalidState),
        "the fragment can never complete on the released transport"
    );
    // The dying projection is consumed before the next epoch opens.
    stack.publish_dying(first, 1);
    assert!(stack.lifecycle.pending().is_empty());

    // The new epoch's receive state starts empty: no old bytes leak in.
    let fresh = connector
        .try_connect(&memory_endpoint(), &identity("fragment", 8))
        .expect("fresh transport");
    let second = stack
        .owner
        .open(fresh, &mut stack.admission, &mut stack.lifecycle)
        .expect("second epoch");
    assert_eq!(second.get(), first.get() + 1);
    assert_eq!(connector.poll(fresh), TransportPoll::Connected);
    assert_eq!(
        connector.poll(fresh),
        TransportPoll::Pending,
        "the new epoch receives nothing of the old fragment"
    );

    // Only a complete frame through the new epoch's own transport publishes.
    peer.feed(&frame).expect("fresh complete frame");
    assert_eq!(
        connector.poll(fresh),
        TransportPoll::Frame(frame.clone()),
        "the new epoch publishes complete frames only"
    );
}

/// The generation rules: the nonzero epoch space is issued strictly
/// monotonically, its exhaustion is the typed capacity failure that changes
/// no state, and an adopted epoch at or below the issued bound is stale.
#[test]
fn generation_exhaustion_returns_typed_capacity() {
    let connector = Arc::new(MemoryConnectorDouble::new());
    let mut admission = InputAdmissionState::try_new(epoch(1), limits()).expect("admission owner");
    let mut lifecycle = LifecycleProjectionState::try_new(1).expect("lifecycle state");

    // The top of the space: no epoch can issue, and nothing changes.
    let mut owner = LifecycleOwner::try_new_after(u64::MAX, limits()).expect("owner");
    let ticket = connector
        .try_connect(&memory_endpoint(), &identity("exhausted", 9))
        .expect("transport ticket");
    assert_eq!(
        owner.open(ticket, &mut admission, &mut lifecycle).err(),
        Some(ClientError::Capacity),
        "epoch exhaustion is the typed capacity failure"
    );
    assert_eq!(owner.epoch(), None, "no epoch was issued");
    assert_eq!(
        owner.adopt(epoch(u64::MAX), None).err(),
        Some(ClientError::StaleEpoch),
        "an epoch at the issued bound is stale"
    );

    // The last issuable epoch works; the reset of the live top epoch
    // succeeds and the next issuance refuses typed with the owner unchanged.
    let mut owner = LifecycleOwner::try_new_after(u64::MAX - 1, limits()).expect("owner");
    let last = owner
        .open(ticket, &mut admission, &mut lifecycle)
        .expect("the last issuable epoch");
    assert_eq!(last.get(), u64::MAX);
    {
        let mut observations = Vec::new();
        let mut audio = AudioProjectionState::try_new().expect("audio state");
        let bundle = LifecycleInvalidation::try_new(
            &mut observations,
            &mut admission,
            &mut audio,
            &mut lifecycle,
        )
        .expect("invalidation bundle");
        let mut preparation = PreparationQueue::new(limits());
        let outcome = owner
            .reset(last, connector.as_ref(), bundle, &mut preparation)
            .expect("the top epoch still resets");
        assert_eq!(outcome.dying(), last);
        // The controller consumed the dying pair before the next open.
        lifecycle = LifecycleProjectionState::try_new(last.get()).expect("lifecycle state");
    }
    let fresh = connector
        .try_connect(&memory_endpoint(), &identity("exhausted", 9))
        .expect("transport ticket");
    assert_eq!(
        owner.open(fresh, &mut admission, &mut lifecycle).err(),
        Some(ClientError::Capacity),
        "the space is exhausted after the top epoch"
    );
    assert_eq!(owner.epoch(), None);
}

/// The sequence-space rules: the journal bound's typed capacity failure
/// leaves every owner unchanged, and a reset restarts the sequence space at
/// one so no exhausted or old sequence can follow into the new epoch.
#[test]
fn sequence_exhaustion_returns_typed_capacity_and_restarts() {
    let mut stack = Stack::new();
    let first = stack.connect();

    // Two full batches fill the prediction journal; the third sequenced
    // action refuses the whole batch with the typed capacity failure and
    // every owner unchanged.
    stack.submit(vec![drop_item(); 128]);
    stack.submit(vec![drop_item(); 128]);
    assert_eq!(stack.admission.journal_entries(), 256);
    let outbound_before = stack.admission.outbound_records();
    let exhausted = InputBatch::try_new(first, vec![drop_item()]).expect("batch");
    let validated = InputTranslator::validate_batch(
        &exhausted,
        stack.mirror.as_ref().expect("admitted mirror"),
        &limits(),
    )
    .expect("validated");
    assert_eq!(
        InputTranslator::commit(validated, &mut stack.admission).err(),
        Some(ClientError::Capacity),
        "the exhausted sequence space refuses the whole batch"
    );
    assert_eq!(stack.admission.journal_entries(), 256, "unchanged");
    assert_eq!(stack.admission.outbound_records(), outbound_before);
    assert_eq!(stack.admission.next_sequence(), 257);

    // The reset restarts the sequence space: the new epoch's first receipt
    // sequences from one, and the old space is unreachable.
    stack.reset();
    stack.publish_dying(first, 2);
    let second = stack.connect();
    assert_eq!(
        stack.submit(vec![drop_item()]),
        InputReceipt::Queued {
            epoch: second,
            first_sequence: Some(1),
            sequenced_count: 1,
            chat_count: 0,
        },
        "the new epoch sequences from one"
    );
    assert_eq!(stack.admission.next_sequence(), 2);
    let stale = InputBatch::try_new(first, vec![drop_item()]).expect("stale batch");
    assert_eq!(
        InputTranslator::validate_batch(
            &stale,
            stack.mirror.as_ref().expect("admitted mirror"),
            &limits()
        )
        .err(),
        Some(ClientError::StaleEpoch)
    );
}

/// The transport ticket rules: launch generations advance strictly across
/// reconnects so an exhausted space never wraps into reuse, and every
/// operation on a released ticket — including a second release — is the
/// typed invalid-state rejection.
#[test]
fn ticket_generations_advance_and_old_tickets_reject_typed() {
    let (connector, _peer) = MemoryConnector::pair(limits(), NonZeroU64::new(1).expect("one"));
    let identity = identity("tickets", 10);

    // Strictly advancing launch generations across one hundred reconnects:
    // the counter never wraps into a reused generation.
    let mut generations = Vec::new();
    for _ in 0..100 {
        let ticket = connector
            .try_connect(&memory_endpoint(), &identity)
            .expect("transport ticket");
        generations.push(ticket.generation().get());
        connector.close(ticket).expect("release");
    }
    for pair in generations.windows(2) {
        assert!(
            pair[1] > pair[0],
            "launch generations advance strictly ({} then {})",
            pair[0],
            pair[1]
        );
    }
    assert_eq!(generations[0], 1, "the launch counter starts at one");

    // A released ticket rejects every operation typed, including the second
    // release, while the live generation keeps working.
    let live = connector
        .try_connect(&memory_endpoint(), &identity)
        .expect("live transport");
    let dead = TransportTicket::try_new(
        NonZeroU64::new(1).expect("one"),
        NonZeroU64::new(1).expect("one"),
    )
    .expect("dead ticket shape");
    assert!(matches!(
        connector.poll(dead),
        TransportPoll::Closed(ClientError::InvalidState)
    ));
    assert_eq!(
        connector.try_send(dead, &hello_frame_bytes()),
        Err(ClientError::InvalidState)
    );
    assert_eq!(
        connector.close(dead),
        Err(ClientError::InvalidState),
        "release happens exactly once per ticket"
    );
    assert_eq!(connector.poll(live), TransportPoll::Connected);
    assert_eq!(connector.poll(live), TransportPoll::Pending);
}
