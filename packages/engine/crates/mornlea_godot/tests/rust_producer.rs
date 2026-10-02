//! The rust-producer adapter target: the safe Rust core behind its private
//! token arena, driven through the identical engine-independent routines the
//! exported Godot methods call.
//!
//! The crate root keeps the adapter modules private, so this registered
//! target includes the production sources by path (the established
//! convention of the family-registry target). Everything exercised here is
//! the real adapter source over the real C1 endpoint; the only doubles are
//! thin wrappers around that real endpoint for call counting, frame
//! injection and panic injection, which are the C1 substitution port's own
//! deterministic seams. Native Dictionary marshalling is explicitly
//! deferred: no test here claims any engine-dependent behavior.

#![allow(dead_code)]

#[path = "../src/abi.rs"]
mod abi;

#[path = "../src/client_core.rs"]
mod client_core;

#[path = "../src/feature_negotiation.rs"]
mod feature_negotiation;

mod rust_producer {
    use std::cell::{Cell, RefCell};
    use std::num::NonZeroU64;
    use std::rc::Rc;
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant};

    use mornlea_client_core::contracts::{
        ClientConfig, ClientEndpoint, ClientError, ClientIdentity, ClientLimits, ClientWorkBudget,
        ConfirmedRevision, Connector, ConnectorRegistry, Endpoint, FamilyKey, FamilyOperation,
        InputReceipt, MonotonicClock, RecordHeader, SessionEpoch, SessionPhase, StepReport,
    };
    use mornlea_client_core::input::{ClientIntent, InputBatch};
    use mornlea_client_core::preparation::{
        LightSummary, SectionKey, TerrainKey, TerrainVisibility, TilePos,
    };
    use mornlea_client_core::presentation::frame::{
        DiagnosticRecord, FamilyFrame, FamilyRecords, PlayerViewRecord, PresentationFrame,
        SessionRecord, TerrainMaterial, TerrainRecord,
    };
    use mornlea_client_core::presentation::{
        BoundedText, ErrorClassCounters, MovementIntent, Pose, ProducerIdentity, QueueCounters,
        TextKind,
    };
    use mornlea_client_core::session::io::{MemoryConnector, MemoryPeer};
    use mornlea_client_core::session::login::LoginSession;
    use mornlea_domain::{
        ActiveMining, ActiveMiningParts, BlockPos, ChunkPos, ContainerKind, ContainerRef,
        Dimension, HeldActions, HotbarSlot, Identities, LookAngles, MiningState, Movement,
        PlayerControl, PlayerControlParts, PlayerId,
    };
    use mornlea_protocol::{LoginSuccess, ServerHello, write_frame};

    use super::abi::boundary::{self, BoundaryValue, CoreTokenValue};
    use super::client_core::producer_identity;
    use super::client_core::rust_core::CoreArena;

    // ------------------------------------------------------------------
    // Deterministic fixtures
    // ------------------------------------------------------------------

    /// A deterministic monotonic clock: time advances only when the test
    /// advances it, so the login deadlines never fire unexpectedly.
    struct TestClock {
        elapsed: Mutex<Duration>,
    }

    impl TestClock {
        fn new() -> Self {
            Self {
                elapsed: Mutex::new(Duration::ZERO),
            }
        }
    }

    impl MonotonicClock for TestClock {
        fn now(&self) -> Instant {
            Instant::now() + *self.elapsed.lock().expect("clock mutex")
        }
    }

    /// One UUIDv4-shaped identity derived from a byte pattern.
    fn uuid_bytes(seed: u8) -> [u8; 16] {
        let mut bytes = [0u8; 16];
        for (index, byte) in bytes.iter_mut().enumerate() {
            *byte = seed.wrapping_add(index as u8);
        }
        bytes[6] = (bytes[6] & 0x0F) | 0x40;
        bytes[8] = (bytes[8] & 0x3F) | 0x80;
        bytes
    }

    fn player(seed: u8) -> PlayerId {
        PlayerId::try_from_bytes(uuid_bytes(seed)).expect("uuidv4 identity")
    }

    /// Lowercase hex through the boundary's own renderer.
    fn hex_of(bytes: &[u8]) -> String {
        match boundary::hex_text(bytes) {
            BoundaryValue::Text(text) => text,
            _ => unreachable!("hex rendering is text"),
        }
    }

    /// The frozen limit set as a boundary value.
    fn limits_value() -> BoundaryValue {
        let limits = ClientLimits::try_new().expect("frozen limits");
        BoundaryValue::fields([
            (
                "queued_input_events",
                boundary::u64_text(limits.queued_input_events() as u64),
            ),
            (
                "inbound_observations",
                boundary::u64_text(limits.inbound_observations() as u64),
            ),
            (
                "inbound_bytes",
                boundary::u64_text(limits.inbound_bytes() as u64),
            ),
            (
                "outbound_commands",
                boundary::u64_text(limits.outbound_commands() as u64),
            ),
            (
                "outbound_bytes",
                boundary::u64_text(limits.outbound_bytes() as u64),
            ),
            (
                "prediction_journal",
                boundary::u64_text(limits.prediction_journal() as u64),
            ),
            (
                "message_work",
                boundary::u64_text(limits.message_work() as u64),
            ),
            ("mesh_work", boundary::u64_text(limits.mesh_work() as u64)),
            (
                "preparation_results",
                boundary::u64_text(limits.preparation_results() as u64),
            ),
            (
                "preparation_bytes",
                boundary::u64_text(limits.preparation_bytes() as u64),
            ),
            (
                "family_records",
                boundary::u64_text(limits.family_records() as u64),
            ),
            (
                "frame_bytes",
                boundary::u64_text(limits.frame_bytes() as u64),
            ),
        ])
    }

    /// The checked `open_core` configuration value.
    fn config_value() -> BoundaryValue {
        BoundaryValue::fields([
            ("limits", limits_value()),
            ("hello_ms", BoundaryValue::Int(5000)),
            ("login_ms", BoundaryValue::Int(10000)),
            ("connector_capability", boundary::u64_text(1)),
        ])
    }

    fn open_spec() -> boundary::CoreOpenSpec {
        boundary::decode_open_core(&config_value()).expect("checked config")
    }

    /// The memory endpoint value under connector id one.
    fn memory_endpoint_value() -> BoundaryValue {
        BoundaryValue::tagged(
            "Memory",
            BoundaryValue::fields([("connector_id", boundary::u64_text(1))]),
        )
    }

    /// The checked identity value for one player identity.
    fn identity_value(seed: u8) -> BoundaryValue {
        BoundaryValue::fields([(
            "login",
            BoundaryValue::fields([
                ("player_id", BoundaryValue::Text(hex_of(&uuid_bytes(seed)))),
                ("display_name", BoundaryValue::Text("driver".to_string())),
                ("view_distance", BoundaryValue::Int(9)),
            ]),
        )])
    }

    fn look_value(yaw: f64, pitch: f64) -> BoundaryValue {
        BoundaryValue::fields([
            ("yaw", BoundaryValue::Float(yaw)),
            ("pitch", BoundaryValue::Float(pitch)),
        ])
    }

    fn container_ref_value(x: i64, slot: i64, generation: i64) -> BoundaryValue {
        BoundaryValue::fields([
            (
                "chunk",
                BoundaryValue::fields([
                    ("x", BoundaryValue::Int(x)),
                    ("z", BoundaryValue::Int(-4)),
                ]),
            ),
            ("kind", BoundaryValue::Text("Chest".to_string())),
            ("slot", BoundaryValue::Int(slot)),
            ("generation", BoundaryValue::Int(generation)),
        ])
    }

    fn container_token_value(epoch: u64, revision: u64) -> BoundaryValue {
        BoundaryValue::fields([
            ("epoch", boundary::u64_text(epoch)),
            ("reference", container_ref_value(3, 1, 2)),
            ("confirmed_revision", boundary::u64_text(revision)),
        ])
    }

    fn crafting_token_value(epoch: u64, revision: u64) -> BoundaryValue {
        BoundaryValue::fields([
            ("epoch", boundary::u64_text(epoch)),
            ("confirmed_revision", boundary::u64_text(revision)),
            ("size", BoundaryValue::Text("Workbench".to_string())),
        ])
    }

    fn action_value(
        tag: &str,
        payload: BoundaryValue,
        container: BoundaryValue,
        crafting: BoundaryValue,
    ) -> BoundaryValue {
        BoundaryValue::fields([
            ("intent", BoundaryValue::tagged(tag, payload)),
            ("container", container),
            ("crafting", crafting),
        ])
    }

    fn batch_value(epoch: u64, actions: Vec<BoundaryValue>) -> BoundaryValue {
        BoundaryValue::fields([
            ("epoch", boundary::u64_text(epoch)),
            ("actions", BoundaryValue::List(actions)),
        ])
    }

    fn epoch_text(epoch: u64) -> BoundaryValue {
        boundary::u64_text(epoch)
    }

    fn work_value(messages: i64, meshes: i64) -> BoundaryValue {
        BoundaryValue::fields([
            ("messages", BoundaryValue::Int(messages)),
            ("meshes", BoundaryValue::Int(meshes)),
        ])
    }

    // ------------------------------------------------------------------
    // Endpoint wrappers over the real C1 login session
    // ------------------------------------------------------------------

    /// The shared counter handle: the wrappers live inside the arena, so
    /// the test reads the counts through this cell instead.
    #[derive(Default)]
    struct CountCell {
        connects: Cell<usize>,
        submits: Cell<usize>,
        steps: Cell<usize>,
        snapshots: Cell<usize>,
        resets: Cell<usize>,
    }

    impl CountCell {
        fn bump(field: &Cell<usize>) {
            field.set(field.get() + 1);
        }
    }

    /// A counting wrapper around the real login session: the observable
    /// call surface the zero-call and exactly-one assertions read. Every
    /// method delegates unchanged.
    struct Counting {
        inner: LoginSession,
        counts: Rc<CountCell>,
    }

    impl ClientEndpoint for Counting {
        fn connect(
            &mut self,
            endpoint: Endpoint,
            identity: ClientIdentity,
        ) -> Result<SessionEpoch, ClientError> {
            CountCell::bump(&self.counts.connects);
            self.inner.connect(endpoint, identity)
        }

        fn submit_input(
            &mut self,
            epoch: SessionEpoch,
            input: InputBatch,
        ) -> Result<InputReceipt, ClientError> {
            CountCell::bump(&self.counts.submits);
            self.inner.submit_input(epoch, input)
        }

        fn step(
            &mut self,
            epoch: SessionEpoch,
            work: ClientWorkBudget,
        ) -> Result<StepReport, ClientError> {
            CountCell::bump(&self.counts.steps);
            self.inner.step(epoch, work)
        }

        fn snapshot(&self, epoch: SessionEpoch) -> Result<Arc<PresentationFrame>, ClientError> {
            CountCell::bump(&self.counts.snapshots);
            self.inner.snapshot(epoch)
        }

        fn reset(&mut self, epoch: SessionEpoch) -> Result<SessionEpoch, ClientError> {
            CountCell::bump(&self.counts.resets);
            self.inner.reset(epoch)
        }

        fn close(&mut self) -> Result<(), ClientError> {
            self.inner.close()
        }
    }

    /// A panic-injecting wrapper: `submit_input` panics, everything else
    /// delegates. The facade guard must convert the panic to `Internal`
    /// while the prior visible state survives.
    struct Panicking {
        inner: LoginSession,
    }

    impl ClientEndpoint for Panicking {
        fn connect(
            &mut self,
            endpoint: Endpoint,
            identity: ClientIdentity,
        ) -> Result<SessionEpoch, ClientError> {
            self.inner.connect(endpoint, identity)
        }

        fn submit_input(
            &mut self,
            _epoch: SessionEpoch,
            _input: InputBatch,
        ) -> Result<InputReceipt, ClientError> {
            panic!("injected producer panic");
        }

        fn step(
            &mut self,
            epoch: SessionEpoch,
            work: ClientWorkBudget,
        ) -> Result<StepReport, ClientError> {
            self.inner.step(epoch, work)
        }

        fn snapshot(&self, epoch: SessionEpoch) -> Result<Arc<PresentationFrame>, ClientError> {
            self.inner.snapshot(epoch)
        }

        fn reset(&mut self, epoch: SessionEpoch) -> Result<SessionEpoch, ClientError> {
            self.inner.reset(epoch)
        }

        fn close(&mut self) -> Result<(), ClientError> {
            self.inner.close()
        }
    }

    /// The shared injection cell of the frame-injecting wrapper.
    #[derive(Default)]
    struct InjectionCell {
        frame: RefCell<Option<PresentationFrame>>,
    }

    /// A frame-injecting wrapper: while a frame is staged, `snapshot`
    /// serves it instead of the real visible frame, so the facade's
    /// whole-frame acceptance path runs against crafted candidates. Every
    /// other method delegates to the real session.
    struct InjectedFrames {
        inner: LoginSession,
        injection: Rc<InjectionCell>,
    }

    impl ClientEndpoint for InjectedFrames {
        fn connect(
            &mut self,
            endpoint: Endpoint,
            identity: ClientIdentity,
        ) -> Result<SessionEpoch, ClientError> {
            self.inner.connect(endpoint, identity)
        }

        fn submit_input(
            &mut self,
            epoch: SessionEpoch,
            input: InputBatch,
        ) -> Result<InputReceipt, ClientError> {
            self.inner.submit_input(epoch, input)
        }

        fn step(
            &mut self,
            epoch: SessionEpoch,
            work: ClientWorkBudget,
        ) -> Result<StepReport, ClientError> {
            self.inner.step(epoch, work)
        }

        fn snapshot(&self, epoch: SessionEpoch) -> Result<Arc<PresentationFrame>, ClientError> {
            if let Some(frame) = self.injection.frame.borrow_mut().take() {
                return Ok(Arc::new(frame));
            }
            self.inner.snapshot(epoch)
        }

        fn reset(&mut self, epoch: SessionEpoch) -> Result<SessionEpoch, ClientError> {
            self.inner.reset(epoch)
        }

        fn close(&mut self) -> Result<(), ClientError> {
            self.inner.close()
        }
    }

    // ------------------------------------------------------------------
    // Server-side fixtures and arena plumbing
    // ------------------------------------------------------------------

    fn server_hello_frame() -> Vec<u8> {
        let hello = ServerHello::new(Identities::current().protocol).expect("current hello");
        write_frame(ServerHello::PACKET_ID, &hello.encode().expect("payload")).expect("hello frame")
    }

    fn login_success_frame(for_player: PlayerId) -> Vec<u8> {
        let payload = LoginSuccess::new(for_player, 7).encode().expect("payload");
        write_frame(LoginSuccess::PACKET_ID, &payload).expect("login frame")
    }

    /// One real memory stack: the connector registered under id one beside
    /// its fixture peer, plus the checked config the real session runs on.
    fn memory_stack() -> (MemoryPeer, ClientConfig) {
        let limits = ClientLimits::try_new().expect("frozen limits");
        let (connector, peer) = MemoryConnector::pair(limits, NonZeroU64::new(1).expect("one"));
        let mut registry = ConnectorRegistry::new();
        registry
            .register(
                NonZeroU64::new(1).expect("one"),
                Arc::clone(&connector) as Arc<dyn Connector>,
            )
            .expect("fresh registry");
        let config = ClientConfig::try_new(
            limits,
            Duration::from_secs(5),
            Duration::from_secs(10),
            Arc::new(TestClock::new()) as Arc<dyn MonotonicClock>,
            Arc::new(registry),
        )
        .expect("checked config");
        (peer, config)
    }

    /// Extracts the closed envelope triple: the ok flag, the value slot and
    /// the failure class when the call failed.
    fn envelope(value: &BoundaryValue) -> (bool, &BoundaryValue, Option<&str>) {
        let ok = value
            .field("ok")
            .and_then(|flag| flag.as_bool().ok())
            .expect("ok flag");
        let value_slot = value.field("value").expect("value key");
        let error_class = value
            .field("error")
            .and_then(|error| error.field("class"))
            .and_then(|class| class.as_text().ok());
        (ok, value_slot, error_class)
    }

    fn assert_failure(value: &BoundaryValue, class: &str) {
        let (ok, value_slot, error_class) = envelope(value);
        assert!(!ok, "the call must fail");
        assert!(
            matches!(value_slot, BoundaryValue::Null),
            "failure carries a null value"
        );
        assert_eq!(error_class, Some(class), "the failure class");
    }

    fn assert_success(value: &BoundaryValue) -> &BoundaryValue {
        let (ok, value_slot, error_class) = envelope(value);
        assert!(ok, "the call must succeed");
        assert_eq!(error_class, None, "success carries no error");
        value_slot
    }

    /// The token of one successful open envelope.
    fn token_of(value: &BoundaryValue) -> CoreTokenValue {
        CoreTokenValue::from_boundary(assert_success(value)).expect("token value")
    }

    fn token_boundary(token: CoreTokenValue) -> BoundaryValue {
        token.to_boundary()
    }

    /// An arena whose slot drives the real login session through a
    /// wrapper, beside the issued token.
    fn wrapped_arena<T: ClientEndpoint + 'static>(wrapper: T) -> (CoreArena, CoreTokenValue) {
        let mut arena = CoreArena::new();
        let token = arena
            .open_with_endpoint(&open_spec(), Box::new(wrapper))
            .expect("the checked open issues a token");
        (arena, token)
    }

    /// An admitted session over the real memory stack with counting.
    fn counting_admitted() -> (CoreArena, CoreTokenValue, Rc<CountCell>, u64) {
        let (peer, config) = memory_stack();
        let counts = Rc::new(CountCell::default());
        let (mut arena, token) = wrapped_arena(Counting {
            inner: LoginSession::new(config).expect("login session"),
            counts: Rc::clone(&counts),
        });
        let epoch = connect_and_admit(&mut arena, token, &peer, 3);
        (arena, token, counts, epoch)
    }

    /// Connects and admits through the real facade routines.
    fn connect_and_admit(
        arena: &mut CoreArena,
        token: CoreTokenValue,
        peer: &MemoryPeer,
        seed: u8,
    ) -> u64 {
        let connected = arena.connect_routine(
            &token_boundary(token),
            &memory_endpoint_value(),
            &identity_value(seed),
        );
        let epoch = match assert_success(&connected) {
            BoundaryValue::Text(text) => text.parse::<u64>().expect("epoch text"),
            _ => panic!("epoch is canonical text"),
        };
        peer.feed(&server_hello_frame()).expect("hello fed");
        assert_success(&arena.step_routine(
            &token_boundary(token),
            &epoch_text(epoch),
            &work_value(4, 0),
        ));
        peer.feed(&login_success_frame(player(seed)))
            .expect("login fed");
        assert_success(&arena.step_routine(
            &token_boundary(token),
            &epoch_text(epoch),
            &work_value(4, 0),
        ));
        epoch
    }

    // ------------------------------------------------------------------
    // Crafted presentation frames
    // ------------------------------------------------------------------

    fn header(epoch: u64, revision: u64) -> RecordHeader {
        RecordHeader::try_new(
            SessionEpoch::try_new(epoch).expect("nonzero epoch"),
            ConfirmedRevision::new(revision),
            Some(7),
            FamilyOperation::Upsert,
        )
        .expect("header")
    }

    /// A frame whose session-family record header revision disagrees with
    /// the frame parent: the whole-frame check must reject it.
    fn mixed_revision_frame() -> PresentationFrame {
        let epoch = SessionEpoch::try_new(1).expect("epoch");
        let mixed = header(1, 2);
        let session = FamilyFrame::try_new(
            FamilyKey::try_new("session").expect("session key"),
            FamilyRecords::Session(vec![
                SessionRecord::try_new(mixed, SessionPhase::Admitted, None, None)
                    .expect("session record"),
            ]),
        )
        .expect("session family");
        let lifecycle = FamilyFrame::try_new(
            FamilyKey::try_new("lifecycle").expect("lifecycle key"),
            FamilyRecords::Lifecycle(Vec::new()),
        )
        .expect("lifecycle family");
        PresentationFrame::try_new(
            epoch,
            ConfirmedRevision::new(1),
            4,
            vec![session, lifecycle],
        )
        .expect("candidate")
    }

    /// A frame with no session family at all: incomplete for the facade.
    fn missing_session_frame() -> PresentationFrame {
        let epoch = SessionEpoch::try_new(1).expect("epoch");
        let lifecycle = FamilyFrame::try_new(
            FamilyKey::try_new("lifecycle").expect("lifecycle key"),
            FamilyRecords::Lifecycle(Vec::new()),
        )
        .expect("lifecycle family");
        PresentationFrame::try_new(epoch, ConfirmedRevision::new(1), 5, vec![lifecycle])
            .expect("candidate")
    }

    /// The rich example frame: a session UUID identity, mining Idle and
    /// Active player-view records, near-section and far-tile terrain keys,
    /// and the diagnostics producer hashes.
    fn rich_example_frame() -> PresentationFrame {
        let epoch = SessionEpoch::try_new(2).expect("epoch");
        let revision = ConfirmedRevision::new(1);
        let upsert = header(2, 1);
        let session = FamilyFrame::try_new(
            FamilyKey::try_new("session").expect("session key"),
            FamilyRecords::Session(vec![
                SessionRecord::try_new(upsert, SessionPhase::Admitted, Some(player(9)), None)
                    .expect("session record"),
            ]),
        )
        .expect("session family");
        let near = TerrainRecord::try_new(
            upsert,
            Dimension::OVERWORLD,
            TerrainKey::Section(SectionKey::try_new(ChunkPos::new(-3, 4), 5).expect("section")),
            11,
            12,
            TerrainMaterial::Opaque,
            TerrainVisibility::Near,
            LightSummary::try_new(15, 3).expect("light"),
            None,
        )
        .expect("near terrain record");
        let far = TerrainRecord::try_new(
            upsert,
            Dimension::DEPTHS,
            TerrainKey::LodTile(TilePos::new(-9, 8)),
            13,
            14,
            TerrainMaterial::Water,
            TerrainVisibility::Far,
            LightSummary::try_new(2, 0).expect("light"),
            None,
        )
        .expect("far terrain record");
        let terrain = FamilyFrame::try_new(
            FamilyKey::try_new("terrain").expect("terrain key"),
            FamilyRecords::Terrain(vec![near, far]),
        )
        .expect("terrain family");
        let movement = || MovementIntent::try_new(None, true).expect("movement");
        let idle = PlayerViewRecord::try_new(
            upsert,
            Pose::try_new([1.0, 2.0, 3.0], 0.25, -0.5).expect("pose"),
            None,
            None,
            movement(),
            None,
            MiningState::Idle,
        )
        .expect("idle player view");
        let active = PlayerViewRecord::try_new(
            upsert,
            Pose::try_new([4.0, 5.0, 6.0], 0.0, 0.0).expect("pose"),
            None,
            None,
            movement(),
            None,
            MiningState::Active(
                ActiveMining::try_new(ActiveMiningParts {
                    target: BlockPos::new(7, -8, 9),
                    progress: 3,
                    required: 9,
                    harvestable: true,
                })
                .expect("active mining"),
            ),
        )
        .expect("active player view");
        let player_view = FamilyFrame::try_new(
            FamilyKey::try_new("player-view").expect("player-view key"),
            FamilyRecords::PlayerView(vec![idle, active]),
        )
        .expect("player-view family");
        let diagnostics = FamilyFrame::try_new(
            FamilyKey::try_new("diagnostics").expect("diagnostics key"),
            FamilyRecords::Diagnostics(vec![
                DiagnosticRecord::try_new(
                    upsert,
                    ProducerIdentity::try_new([0x11; 20], [0x22; 32]).expect("identity"),
                    6,
                    QueueCounters::try_new(1, 2, 3, 4, 5, 6, 7).expect("queues"),
                    ErrorClassCounters::try_new(1, 2, 3, 4, 5, 6).expect("counters"),
                )
                .expect("diagnostics record"),
            ]),
        )
        .expect("diagnostics family");
        PresentationFrame::try_new(
            epoch,
            revision,
            6,
            vec![session, terrain, player_view, diagnostics],
        )
        .expect("candidate")
    }

    /// Reads one family's records out of a rendered frame value.
    fn family_records<'a>(frame: &'a BoundaryValue, name: &str) -> &'a [BoundaryValue] {
        let families = frame
            .field("families")
            .and_then(|value| value.as_list().ok())
            .expect("families list");
        for family in families {
            let key = family.field("key").expect("family key");
            let logical = key
                .field("logical_name")
                .and_then(|value| value.as_text().ok())
                .expect("logical name");
            if logical == name {
                return family
                    .field("records")
                    .and_then(|value| value.as_list().ok())
                    .expect("records list");
            }
        }
        panic!("family {name} is absent");
    }

    // ==================================================================
    // Case: rust mode does not load the Go producer
    // ==================================================================

    #[test]
    fn rust_mode_does_not_load_go() {
        // Before anything, no Go producer is reachable: the loader probe
        // never resolves a packed version in this tree.
        let identity = producer_identity();
        assert_eq!(
            identity.packed_version, None,
            "no go producer may be reachable before rust mode runs"
        );

        // The full rust-mode open path: the checked config decodes, the
        // C1 owner validates it and the arena issues a nonzero slot with
        // generation one. The token round-trips through its own boundary
        // shape.
        let mut arena = CoreArena::new();
        let clock =
            Arc::new(mornlea_client_core::contracts::StdMonotonicClock) as Arc<dyn MonotonicClock>;
        let connectors = Arc::new(ConnectorRegistry::new());
        let opened = arena.open_core_routine(&config_value(), clock, connectors);
        let token = token_of(&opened);
        assert_eq!(token.slot(), 1);
        assert_eq!(token.generation(), 1);

        // The frozen descriptor table answers through the same facade.
        let table = arena.family_table_routine(&token.to_boundary());
        let value = assert_success(&table);
        assert_eq!(
            value
                .field("producer")
                .and_then(|v| v.as_text().ok())
                .expect("producer name"),
            "rust-client-core"
        );
        let descriptors = value
            .field("descriptors")
            .and_then(|v| v.as_list().ok())
            .expect("descriptor list");
        assert_eq!(descriptors.len(), 10);
        for (index, descriptor) in descriptors.iter().enumerate() {
            assert_eq!(
                descriptor
                    .field("numeric_id")
                    .and_then(|v| v.as_int_in(1, 10).ok()),
                Some(index as i64 + 1),
                "numeric id at position {index}"
            );
            assert_eq!(
                descriptor
                    .field("record_limit")
                    .and_then(|v| v.as_text().ok()),
                Some("4096"),
                "the accepted per-family record limit"
            );
        }

        // A repeated close of the issued token succeeds without a second
        // release, and the retired token no longer addresses a core.
        assert!(matches!(
            assert_success(&arena.close_routine(&token.to_boundary())),
            BoundaryValue::Null
        ));
        assert!(matches!(
            assert_success(&arena.close_routine(&token.to_boundary())),
            BoundaryValue::Null
        ));
        assert_eq!(arena.releases(), 1, "exactly one release");
        assert_failure(
            &arena.family_table_routine(&token.to_boundary()),
            "InvalidState",
        );

        // After the whole rust-mode lifecycle the Go producer is still not
        // loaded: rust mode opened, used and released a safe Rust core and
        // the dynamic loader never resolved a producer.
        let identity = producer_identity();
        assert_eq!(
            identity.packed_version, None,
            "the rust-mode lifecycle never loads the go producer"
        );
        assert!(identity.failure.is_some());
    }

    // ==================================================================
    // Case: whole-frame failure is atomic
    // ==================================================================

    #[test]
    fn whole_frame_failure_atomic() {
        let (peer, config) = memory_stack();
        let injection = Rc::new(InjectionCell::default());
        let (mut arena, token) = wrapped_arena(InjectedFrames {
            inner: LoginSession::new(config).expect("login session"),
            injection: Rc::clone(&injection),
        });
        // Connect through the real facade routines so the wrapped session
        // owns a live epoch and a real visible frame.
        connect_and_admit(&mut arena, token, &peer, 4);

        // The real pending frame renders and becomes the visible copy.
        let pulled = arena.pull_frame_routine(&token.to_boundary(), &epoch_text(1));
        let first = assert_success(&pulled).clone();
        assert_eq!(
            arena.visible_frame(token).expect("visible copy"),
            &first,
            "the visible copy is the rendered frame"
        );

        // A frame with no session family fails the whole-frame check and
        // leaves the prior visible copy untouched.
        injection
            .frame
            .borrow_mut()
            .replace(missing_session_frame());
        assert_failure(
            &arena.pull_frame_routine(&token.to_boundary(), &epoch_text(1)),
            "InvalidInput",
        );
        assert_eq!(
            arena.visible_frame(token).expect("visible copy"),
            &first,
            "a failed pull preserves the prior visible frame"
        );

        // A mixed-revision frame fails the same way.
        injection.frame.borrow_mut().replace(mixed_revision_frame());
        assert_failure(
            &arena.pull_frame_routine(&token.to_boundary(), &epoch_text(1)),
            "InvalidInput",
        );
        assert_eq!(arena.visible_frame(token).expect("visible copy"), &first);

        // A valid frame replaces the visible copy again.
        injection.frame.borrow_mut().replace(rich_example_frame());
        let second =
            assert_success(&arena.pull_frame_routine(&token.to_boundary(), &epoch_text(1))).clone();
        assert_ne!(second, first);
        assert_eq!(arena.visible_frame(token).expect("visible copy"), &second);

        // The owned copy stays valid and readable after the core's own
        // frame is released by the close.
        assert!(matches!(
            assert_success(&arena.close_routine(&token.to_boundary())),
            BoundaryValue::Null
        ));
        let families = second
            .field("families")
            .and_then(|value| value.as_list().ok())
            .expect("families survive the release");
        assert_eq!(families.len(), 4);
        assert_eq!(
            second
                .field("frame_index")
                .and_then(|value| value.as_text().ok()),
            Some("6"),
            "the released copy keeps its exact fields"
        );
    }

    // ==================================================================
    // Case: the method table and the owned values it round-trips
    // ==================================================================

    #[test]
    fn method_table_and_owned_values() {
        canonical_u64_text();
        uuid_and_hash_shapes();
        envelope_and_error_shape();
        all_twenty_actions_decode();
        container_regions_and_token_shapes();
        batch_bounds_and_rejections();
        arena_lookup_and_submit_counts();
    }

    fn canonical_u64_text() {
        // u64::MAX is canonical and decodes.
        assert_eq!(
            boundary::u64_from_text(&boundary::u64_text(u64::MAX)),
            Ok(u64::MAX)
        );
        assert_eq!(
            boundary::u64_from_text(&BoundaryValue::Text("0".to_string())),
            Ok(0)
        );
        // MAX + 1, leading zeros, signs, whitespace, junk and wrong types
        // all reject.
        for text in [
            "18446744073709551616",
            "01",
            "00",
            "-1",
            "+1",
            " 1",
            "",
            "1a",
            "1.0",
            "0x10",
            "184467440737095516157",
        ] {
            assert!(
                boundary::u64_from_text(&BoundaryValue::Text(text.to_string())).is_err(),
                "text {text} must reject"
            );
        }
        assert!(boundary::u64_from_text(&BoundaryValue::Int(5)).is_err());
        // The rendering is canonical by construction.
        assert_eq!(
            boundary::u64_text(u64::MAX),
            BoundaryValue::Text("18446744073709551615".to_string())
        );
    }

    fn uuid_and_hash_shapes() {
        // A valid 32-digit lowercase identity decodes to its bytes.
        let bytes = uuid_bytes(5);
        let hex = hex_of(&bytes);
        assert_eq!(hex.len(), 32);
        assert_eq!(
            boundary::uuid_from_hex(&BoundaryValue::Text(hex.clone())),
            Ok(bytes)
        );
        // Wrong lengths, uppercase, non-hex and dashed spellings reject.
        for text in [
            hex[..31].to_string(),
            format!("{hex}0"),
            hex.to_uppercase(),
            hex.replace('a', "g"),
            format!("{}-{}", &hex[..8], &hex[8..]),
        ] {
            assert!(
                boundary::uuid_from_hex(&BoundaryValue::Text(text)).is_err(),
                "uuid spelling must reject"
            );
        }
        // The rendered hashes are fixed-length lowercase hex: a 20-byte
        // source digest renders 40 digits and a 32-byte contract digest 64.
        let frame = rich_example_frame();
        let limits = ClientLimits::try_new().expect("frozen limits");
        let rendered = boundary::render_frame(&frame, &limits).expect("valid example frame");
        let diagnostics = family_records(&rendered, "diagnostics");
        assert_eq!(diagnostics.len(), 1);
        let producer = diagnostics[0].field("producer").expect("producer");
        let source_sha = producer
            .field("source_sha")
            .and_then(|v| v.as_text().ok())
            .expect("source hash");
        let contract_sha = producer
            .field("contract_sha")
            .and_then(|v| v.as_text().ok())
            .expect("contract hash");
        assert_eq!(source_sha.len(), 40);
        assert_eq!(contract_sha.len(), 64);
        assert!(
            source_sha
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
        );
        assert!(
            contract_sha
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
        );
        assert_eq!(source_sha, hex_of(&[0x11; 20]));
        assert_eq!(contract_sha, hex_of(&[0x22; 32]));
        // The session record's player identity renders as the same
        // lowercase-hex UUID, and the mining and geometry examples pin.
        let session = family_records(&rendered, "session");
        assert_eq!(
            session[0].field("player_id").and_then(|v| v.as_text().ok()),
            Some(hex_of(&uuid_bytes(9)).as_str())
        );
        let player_view = family_records(&rendered, "player-view");
        let (idle_tag, idle_value) = player_view[0]
            .field("mining")
            .and_then(|v| v.tag())
            .expect("idle mining tag");
        assert_eq!(idle_tag, "Idle");
        assert!(matches!(idle_value, BoundaryValue::Null));
        let (active_tag, active_value) = player_view[1]
            .field("mining")
            .and_then(|v| v.tag())
            .expect("active mining tag");
        assert_eq!(active_tag, "Active");
        assert_eq!(
            active_value
                .field("progress")
                .and_then(|v| v.as_int_in(0, 9).ok()),
            Some(3)
        );
        assert_eq!(
            active_value
                .field("required")
                .and_then(|v| v.as_int_in(0, 9).ok()),
            Some(9)
        );
        assert_eq!(
            active_value
                .field("harvestable")
                .and_then(|v| v.as_bool().ok()),
            Some(true)
        );
        let terrain = family_records(&rendered, "terrain");
        let (near_tag, near_value) = terrain[0]
            .field("key")
            .and_then(|v| v.tag())
            .expect("near key tag");
        assert_eq!(near_tag, "Section");
        assert_eq!(
            near_value
                .field("section")
                .and_then(|v| v.as_int_in(0, 23).ok()),
            Some(5)
        );
        assert_eq!(
            terrain[0]
                .field("visibility")
                .and_then(|v| v.as_text().ok()),
            Some("Near")
        );
        let (far_tag, far_value) = terrain[1]
            .field("key")
            .and_then(|v| v.tag())
            .expect("far key tag");
        assert_eq!(far_tag, "LodTile");
        assert_eq!(
            far_value.field("x").and_then(|v| v.as_int_in(-9, -9).ok()),
            Some(-9)
        );
        assert_eq!(
            far_value.field("z").and_then(|v| v.as_int_in(8, 8).ok()),
            Some(8)
        );
        assert_eq!(
            terrain[1]
                .field("visibility")
                .and_then(|v| v.as_text().ok()),
            Some("Far")
        );
        // The frame envelope's own fields are the declared shape.
        assert_eq!(
            rendered
                .field("layout_major")
                .and_then(|v| v.as_int_in(1, 1).ok()),
            Some(1)
        );
        assert_eq!(
            rendered
                .field("session_epoch")
                .and_then(|v| v.as_text().ok()),
            Some("2")
        );
        assert_eq!(
            rendered
                .field("confirmed_revision")
                .and_then(|v| v.as_text().ok()),
            Some("1")
        );
    }

    fn envelope_and_error_shape() {
        // The closed envelope and the error value's exact field set.
        let ok = boundary::outcome(Ok(BoundaryValue::Null));
        assert!(matches!(assert_success(&ok), BoundaryValue::Null));
        for (error, class) in [
            (ClientError::InvalidInput, "InvalidInput"),
            (ClientError::IncompatibleVersion, "IncompatibleVersion"),
            (ClientError::InvalidState, "InvalidState"),
            (ClientError::StaleEpoch, "StaleEpoch"),
            (ClientError::Capacity, "Capacity"),
            (ClientError::Timeout, "Timeout"),
            (ClientError::Disconnected, "Disconnected"),
            (ClientError::Io, "Io"),
            (ClientError::Internal, "Internal"),
        ] {
            let failed = boundary::outcome(Err(error));
            assert_failure(&failed, class);
            let error_value = failed.field("error").expect("error key");
            error_value
                .exact_fields(&["class", "resource", "limit", "observed"])
                .expect("the exact error field set");
            assert!(matches!(
                error_value.field("resource").expect("resource"),
                BoundaryValue::Null
            ));
            assert!(matches!(
                error_value.field("limit").expect("limit"),
                BoundaryValue::Null
            ));
            assert!(matches!(
                error_value.field("observed").expect("observed"),
                BoundaryValue::Null
            ));
        }
        // The token value's own shape: a zero slot and a non-canonical
        // generation reject, and the round trip is exact.
        assert!(CoreTokenValue::new(0, 1).is_err());
        let token = CoreTokenValue::new(2, 3).expect("nonzero slot");
        assert_eq!(
            CoreTokenValue::from_boundary(&token.to_boundary()),
            Ok(token)
        );
        let bad_slot = BoundaryValue::fields([
            ("slot", BoundaryValue::Int(0)),
            ("generation", boundary::u64_text(1)),
        ]);
        assert!(CoreTokenValue::from_boundary(&bad_slot).is_err());
        let bad_generation = BoundaryValue::fields([
            ("slot", BoundaryValue::Int(1)),
            ("generation", BoundaryValue::Text("01".to_string())),
        ]);
        assert!(CoreTokenValue::from_boundary(&bad_generation).is_err());
        let extra_field = BoundaryValue::fields([
            ("slot", BoundaryValue::Int(1)),
            ("generation", boundary::u64_text(1)),
            ("extra", BoundaryValue::Null),
        ]);
        assert!(CoreTokenValue::from_boundary(&extra_field).is_err());
    }

    /// One PlayerInput action value.
    fn player_input_value() -> BoundaryValue {
        action_value(
            "PlayerInput",
            BoundaryValue::fields([
                (
                    "movement",
                    BoundaryValue::fields([
                        ("move_x", BoundaryValue::Int(1)),
                        ("move_z", BoundaryValue::Int(-1)),
                        ("jump", BoundaryValue::Bool(true)),
                    ]),
                ),
                ("look", look_value(0.5, -0.25)),
                (
                    "actions",
                    BoundaryValue::fields([
                        ("primary", BoundaryValue::Bool(true)),
                        ("eating", BoundaryValue::Bool(false)),
                        ("sprinting", BoundaryValue::Bool(true)),
                        ("sneaking", BoundaryValue::Bool(false)),
                    ]),
                ),
            ]),
            BoundaryValue::Null,
            BoundaryValue::Null,
        )
    }

    fn expected_player_input() -> ClientIntent {
        ClientIntent::PlayerInput(PlayerControl::new(PlayerControlParts {
            movement: Movement {
                move_x: 1,
                move_z: -1,
                jump: true,
            },
            look: LookAngles::try_new(0.5, -0.25).expect("look"),
            actions: HeldActions {
                primary: true,
                eating: false,
                sprinting: true,
                sneaking: false,
            },
        }))
    }

    fn all_twenty_actions_decode() {
        let container = container_ref_value(3, 1, 2);
        let actions = vec![
            player_input_value(),
            action_value(
                "PlaceBlock",
                BoundaryValue::fields([
                    ("look", look_value(1.0, 0.0)),
                    ("slot", BoundaryValue::Int(3)),
                ]),
                BoundaryValue::Null,
                BoundaryValue::Null,
            ),
            action_value(
                "Resync",
                BoundaryValue::fields([
                    ("dimension", BoundaryValue::Int(1)),
                    (
                        "chunk",
                        BoundaryValue::fields([
                            ("x", BoundaryValue::Int(-5)),
                            ("z", BoundaryValue::Int(7)),
                        ]),
                    ),
                    ("have_revision", boundary::u64_text(u64::MAX)),
                ]),
                BoundaryValue::Null,
                BoundaryValue::Null,
            ),
            action_value(
                "SelectHotbar",
                BoundaryValue::fields([("slot", BoundaryValue::Int(8))]),
                BoundaryValue::Null,
                BoundaryValue::Null,
            ),
            action_value(
                "OpenContainer",
                BoundaryValue::fields([("look", look_value(0.1, 0.2))]),
                BoundaryValue::Null,
                BoundaryValue::Null,
            ),
            action_value(
                "TillSoil",
                BoundaryValue::fields([("look", look_value(0.1, 0.2))]),
                BoundaryValue::Null,
                BoundaryValue::Null,
            ),
            action_value(
                "BoneMeal",
                BoundaryValue::fields([("look", look_value(0.1, 0.2))]),
                BoundaryValue::Null,
                BoundaryValue::Null,
            ),
            action_value(
                "CollectWater",
                BoundaryValue::fields([("look", look_value(0.1, 0.2))]),
                BoundaryValue::Null,
                BoundaryValue::Null,
            ),
            action_value(
                "PlaceWater",
                BoundaryValue::fields([("look", look_value(0.1, 0.2))]),
                BoundaryValue::Null,
                BoundaryValue::Null,
            ),
            action_value(
                "MoveInventory",
                BoundaryValue::fields([
                    ("from", BoundaryValue::Int(0)),
                    ("to", BoundaryValue::Int(35)),
                ]),
                BoundaryValue::Null,
                BoundaryValue::Null,
            ),
            action_value(
                "MoveCrafting",
                BoundaryValue::fields([
                    ("from", BoundaryValue::Int(0)),
                    ("to", BoundaryValue::Int(40)),
                ]),
                BoundaryValue::Null,
                crafting_token_value(1, 1),
            ),
            action_value(
                "MoveContainer",
                BoundaryValue::fields([
                    ("container", container.clone()),
                    ("from", BoundaryValue::Int(36)),
                    ("to", BoundaryValue::Int(37)),
                ]),
                container_token_value(1, 1),
                BoundaryValue::Null,
            ),
            action_value(
                "CloseContainer",
                BoundaryValue::Null,
                container_token_value(1, 1),
                BoundaryValue::Null,
            ),
            action_value(
                "DropSelectedItem",
                BoundaryValue::Null,
                BoundaryValue::Null,
                BoundaryValue::Null,
            ),
            action_value(
                "TakeCraftingOutput",
                BoundaryValue::Null,
                BoundaryValue::Null,
                crafting_token_value(1, 1),
            ),
            action_value(
                "EquipArmor",
                BoundaryValue::Null,
                BoundaryValue::Null,
                BoundaryValue::Null,
            ),
            action_value(
                "MovePartial",
                BoundaryValue::fields([
                    (
                        "view",
                        BoundaryValue::tagged("Inventory", BoundaryValue::Null),
                    ),
                    ("from", BoundaryValue::Int(1)),
                    ("to", BoundaryValue::Int(2)),
                    ("single", BoundaryValue::Bool(true)),
                ]),
                BoundaryValue::Null,
                BoundaryValue::Null,
            ),
            action_value(
                "QuickMove",
                BoundaryValue::fields([
                    (
                        "view",
                        BoundaryValue::tagged("Crafting", BoundaryValue::Null),
                    ),
                    ("slot", BoundaryValue::Int(10)),
                ]),
                BoundaryValue::Null,
                BoundaryValue::Null,
            ),
            action_value(
                "DropStack",
                BoundaryValue::fields([
                    (
                        "view",
                        BoundaryValue::tagged("Container", container.clone()),
                    ),
                    ("slot", BoundaryValue::Int(40)),
                ]),
                container_token_value(1, 1),
                BoundaryValue::Null,
            ),
            action_value(
                "Chat",
                BoundaryValue::fields([("text", BoundaryValue::Text("/stop ALPHA".to_string()))]),
                BoundaryValue::Null,
                BoundaryValue::Null,
            ),
        ];
        assert_eq!(actions.len(), 20, "exactly the twenty closed actions");
        let decoded = boundary::decode_input_batch(&batch_value(1, actions))
            .expect("the whole batch decodes");
        assert_eq!(decoded.epoch().get(), 1);
        assert_eq!(decoded.actions().len(), 20);
        let expected: Vec<ClientIntent> = vec![
            expected_player_input(),
            ClientIntent::PlaceBlock(
                mornlea_domain::PlacementIntent::try_new(
                    LookAngles::try_new(1.0, 0.0).expect("look"),
                    3,
                )
                .expect("placement"),
            ),
            ClientIntent::Resync(
                mornlea_domain::ResyncIntent::try_new(1, ChunkPos::new(-5, 7), u64::MAX)
                    .expect("resync"),
            ),
            ClientIntent::SelectHotbar(HotbarSlot::new(8).expect("hotbar slot")),
            ClientIntent::OpenContainer(LookAngles::try_new(0.1, 0.2).expect("look")),
            ClientIntent::TillSoil(LookAngles::try_new(0.1, 0.2).expect("look")),
            ClientIntent::BoneMeal(LookAngles::try_new(0.1, 0.2).expect("look")),
            ClientIntent::CollectWater(LookAngles::try_new(0.1, 0.2).expect("look")),
            ClientIntent::PlaceWater(LookAngles::try_new(0.1, 0.2).expect("look")),
            ClientIntent::MoveInventory(
                mornlea_domain::InventoryMove::try_new(0, 35).expect("inventory move"),
            ),
            ClientIntent::MoveCrafting(
                mornlea_domain::CraftingMove::try_new(0, 40).expect("crafting move"),
            ),
            ClientIntent::MoveContainer(
                mornlea_domain::ContainerMove::try_new(
                    ChunkPos::new(3, -4),
                    ContainerKind::Chest,
                    1,
                    2,
                    36,
                    37,
                )
                .expect("container move"),
            ),
            ClientIntent::CloseContainer,
            ClientIntent::DropSelectedItem,
            ClientIntent::TakeCraftingOutput,
            ClientIntent::EquipArmor,
            ClientIntent::MovePartial(
                mornlea_domain::PartialMove::try_new(
                    mornlea_domain::StackView::Inventory,
                    1,
                    2,
                    true,
                )
                .expect("partial move"),
            ),
            ClientIntent::QuickMove(
                mornlea_domain::StackSource::try_new(mornlea_domain::StackView::Crafting, 10)
                    .expect("quick move"),
            ),
            ClientIntent::DropStack(
                mornlea_domain::StackSource::try_new(
                    mornlea_domain::StackView::Container(
                        ContainerRef::try_new(ChunkPos::new(3, -4), ContainerKind::Chest, 1, 2)
                            .expect("reference"),
                    ),
                    40,
                )
                .expect("drop stack"),
            ),
            ClientIntent::Chat(mornlea_domain::ChatIntent::new(
                mornlea_domain::CommandText::try_from_canonical("/stop ALPHA".to_string())
                    .expect("command text"),
            )),
        ];
        for (index, (action, intent)) in decoded.actions().iter().zip(expected).enumerate() {
            assert_eq!(action.intent, intent, "action {index} decodes exactly");
        }
        // The tokens decode to the checked C1 values with the exact
        // reference attribution.
        let moved = &decoded.actions()[11];
        let token = moved.container.as_ref().expect("container token");
        assert_eq!(token.epoch().get(), 1);
        assert_eq!(token.confirmed_revision().get(), 1);
        assert_eq!(token.reference().kind(), ContainerKind::Chest);
        let crafted = &decoded.actions()[10];
        let token = crafted.crafting.as_ref().expect("crafting token");
        assert_eq!(token.size(), mornlea_domain::CraftingSize::Workbench);
        assert_eq!(token.confirmed_revision().get(), 1);
    }

    fn container_regions_and_token_shapes() {
        // Every stack-view region for the three region-addressed intents.
        let container = container_ref_value(3, 1, 2);
        let views = [
            BoundaryValue::tagged("Inventory", BoundaryValue::Null),
            BoundaryValue::tagged("Crafting", BoundaryValue::Null),
            BoundaryValue::tagged("Container", container.clone()),
        ];
        let tokens = [
            (BoundaryValue::Null, BoundaryValue::Null),
            (BoundaryValue::Null, crafting_token_value(1, 1)),
            (container_token_value(1, 1), BoundaryValue::Null),
        ];
        for (view, (container_token, crafting_token)) in views.iter().zip(tokens) {
            for (tag, payload) in [
                (
                    "MovePartial",
                    BoundaryValue::fields([
                        ("view", view.clone()),
                        ("from", BoundaryValue::Int(1)),
                        ("to", BoundaryValue::Int(2)),
                        ("single", BoundaryValue::Bool(false)),
                    ]),
                ),
                (
                    "QuickMove",
                    BoundaryValue::fields([
                        ("view", view.clone()),
                        ("slot", BoundaryValue::Int(3)),
                    ]),
                ),
                (
                    "DropStack",
                    BoundaryValue::fields([
                        ("view", view.clone()),
                        ("slot", BoundaryValue::Int(3)),
                    ]),
                ),
            ] {
                let decoded = boundary::decode_input_batch(&batch_value(
                    1,
                    vec![action_value(
                        tag,
                        payload,
                        container_token.clone(),
                        crafting_token.clone(),
                    )],
                ))
                .expect("region action decodes");
                assert_eq!(decoded.actions().len(), 1, "{tag} over the region");
            }
        }
        // An unknown view tag, an unknown intent tag, a nullary payload
        // that is not null and an unknown token size all reject.
        let unknown_view = action_value(
            "QuickMove",
            BoundaryValue::fields([
                (
                    "view",
                    BoundaryValue::tagged("Vending", BoundaryValue::Null),
                ),
                ("slot", BoundaryValue::Int(3)),
            ]),
            BoundaryValue::Null,
            BoundaryValue::Null,
        );
        assert!(boundary::decode_input_batch(&batch_value(1, vec![unknown_view])).is_err());
        let unknown_tag = action_value(
            "EquipArmour",
            BoundaryValue::Null,
            BoundaryValue::Null,
            BoundaryValue::Null,
        );
        assert!(boundary::decode_input_batch(&batch_value(1, vec![unknown_tag])).is_err());
        let non_null_nullary = action_value(
            "EquipArmor",
            BoundaryValue::fields([("slot", BoundaryValue::Int(1))]),
            BoundaryValue::Null,
            BoundaryValue::Null,
        );
        assert!(boundary::decode_input_batch(&batch_value(1, vec![non_null_nullary])).is_err());
        let bad_size = action_value(
            "EquipArmor",
            BoundaryValue::Null,
            BoundaryValue::Null,
            BoundaryValue::fields([
                ("epoch", boundary::u64_text(1)),
                ("confirmed_revision", boundary::u64_text(1)),
                ("size", BoundaryValue::Text("Huge".to_string())),
            ]),
        );
        assert!(boundary::decode_input_batch(&batch_value(1, vec![bad_size])).is_err());
    }

    fn batch_bounds_and_rejections() {
        // The empty batch decodes and admits as a no-op.
        let empty =
            boundary::decode_input_batch(&batch_value(1, Vec::new())).expect("empty batch decodes");
        assert_eq!(empty.actions().len(), 0);
        // The 129th action rejects the complete batch with the typed
        // capacity error before any core call.
        let mut actions = Vec::new();
        for _ in 0..129 {
            actions.push(action_value(
                "EquipArmor",
                BoundaryValue::Null,
                BoundaryValue::Null,
                BoundaryValue::Null,
            ));
        }
        assert_eq!(
            boundary::decode_input_batch(&batch_value(1, actions)).unwrap_err(),
            ClientError::Capacity
        );
        // A bool-as-integer leaf rejects: booleans never coerce.
        let bool_as_integer = BoundaryValue::fields([
            ("epoch", boundary::u64_text(1)),
            (
                "actions",
                BoundaryValue::List(vec![BoundaryValue::fields([
                    (
                        "intent",
                        BoundaryValue::tagged(
                            "PlayerInput",
                            BoundaryValue::fields([
                                (
                                    "movement",
                                    BoundaryValue::fields([
                                        ("move_x", BoundaryValue::Int(1)),
                                        ("move_z", BoundaryValue::Int(0)),
                                        ("jump", BoundaryValue::Int(1)),
                                    ]),
                                ),
                                ("look", look_value(0.0, 0.0)),
                                (
                                    "actions",
                                    BoundaryValue::fields([
                                        ("primary", BoundaryValue::Bool(false)),
                                        ("eating", BoundaryValue::Bool(false)),
                                        ("sprinting", BoundaryValue::Bool(false)),
                                        ("sneaking", BoundaryValue::Bool(false)),
                                    ]),
                                ),
                            ]),
                        ),
                    ),
                    ("container", BoundaryValue::Null),
                    ("crafting", BoundaryValue::Null),
                ])]),
            ),
        ]);
        assert_eq!(
            boundary::decode_input_batch(&bool_as_integer).unwrap_err(),
            ClientError::InvalidInput
        );
        // A NaN float rejects the whole batch.
        let nan_look = action_value(
            "OpenContainer",
            BoundaryValue::fields([
                ("yaw", BoundaryValue::Float(f64::NAN)),
                ("pitch", BoundaryValue::Float(0.0)),
            ]),
            BoundaryValue::Null,
            BoundaryValue::Null,
        );
        assert_eq!(
            boundary::decode_input_batch(&batch_value(1, vec![nan_look])).unwrap_err(),
            ClientError::InvalidInput
        );
        // An extra field and a missing field reject.
        let extra = BoundaryValue::fields([
            ("epoch", boundary::u64_text(1)),
            ("actions", BoundaryValue::List(Vec::new())),
            ("extra", BoundaryValue::Null),
        ]);
        assert_eq!(
            boundary::decode_input_batch(&extra).unwrap_err(),
            ClientError::InvalidInput
        );
        let missing = BoundaryValue::fields([("epoch", boundary::u64_text(1))]);
        assert_eq!(
            boundary::decode_input_batch(&missing).unwrap_err(),
            ClientError::InvalidInput
        );
        // A non-epoch epoch text rejects.
        let bad_epoch = BoundaryValue::fields([
            ("epoch", BoundaryValue::Text("01".to_string())),
            ("actions", BoundaryValue::List(Vec::new())),
        ]);
        assert_eq!(
            boundary::decode_input_batch(&bad_epoch).unwrap_err(),
            ClientError::InvalidInput
        );
        // The endpoint and identity decoders fail closed on shape drift.
        let unknown_endpoint = BoundaryValue::tagged(
            "Unix",
            BoundaryValue::fields([("path", BoundaryValue::Text("/x".to_string()))]),
        );
        assert_eq!(
            boundary::decode_endpoint(&unknown_endpoint).unwrap_err(),
            ClientError::InvalidInput
        );
        let bad_host = BoundaryValue::tagged(
            "Tcp",
            BoundaryValue::fields([
                ("host", BoundaryValue::Text("example.invalid".to_string())),
                ("port", BoundaryValue::Int(9)),
            ]),
        );
        assert_eq!(
            boundary::decode_endpoint(&bad_host).unwrap_err(),
            ClientError::InvalidInput,
            "a DNS name is not a numeric endpoint"
        );
        let numeric_host = BoundaryValue::tagged(
            "Tcp",
            BoundaryValue::fields([
                ("host", BoundaryValue::Text("127.0.0.1".to_string())),
                ("port", BoundaryValue::Int(9)),
            ]),
        );
        assert!(boundary::decode_endpoint(&numeric_host).is_ok());
        let bad_identity = BoundaryValue::fields([(
            "login",
            BoundaryValue::fields([
                ("player_id", BoundaryValue::Text("00".to_string())),
                ("display_name", BoundaryValue::Text("driver".to_string())),
                ("view_distance", BoundaryValue::Int(9)),
            ]),
        )]);
        assert_eq!(
            boundary::decode_identity(&bad_identity).unwrap_err(),
            ClientError::InvalidInput
        );
        let out_of_range_view = BoundaryValue::fields([(
            "login",
            BoundaryValue::fields([
                ("player_id", BoundaryValue::Text(hex_of(&uuid_bytes(2)))),
                ("display_name", BoundaryValue::Text("driver".to_string())),
                ("view_distance", BoundaryValue::Int(65)),
            ]),
        )]);
        assert_eq!(
            boundary::decode_identity(&out_of_range_view).unwrap_err(),
            ClientError::InvalidInput
        );
        // The step work decoder rejects an over-budget demand.
        assert_eq!(
            boundary::decode_work(&work_value(4097, 0)).unwrap_err(),
            ClientError::Capacity
        );
    }

    fn arena_lookup_and_submit_counts() {
        let (mut arena, token, counts, epoch) = counting_admitted();
        assert_eq!(counts.connects.get(), 1);
        assert_eq!(counts.steps.get(), 2);

        // A rejected decoding makes zero core calls.
        let submits_before = counts.submits.get();
        let garbage = BoundaryValue::fields([("epoch", boundary::u64_text(epoch))]);
        assert_failure(
            &arena.submit_routine(&token.to_boundary(), &epoch_text(epoch), &garbage),
            "InvalidInput",
        );
        assert_eq!(counts.submits.get(), submits_before);

        // A stale generation or an unknown slot fails the lookup before
        // any core dereference.
        let stale =
            CoreTokenValue::new(token.slot(), token.generation() + 1).expect("stale token shape");
        assert_failure(
            &arena.submit_routine(
                &stale.to_boundary(),
                &epoch_text(epoch),
                &batch_value(epoch, vec![player_input_value()]),
            ),
            "InvalidState",
        );
        let unknown = CoreTokenValue::new(token.slot() + 10, token.generation())
            .expect("unknown token shape");
        assert_failure(
            &arena.submit_routine(
                &unknown.to_boundary(),
                &epoch_text(epoch),
                &batch_value(epoch, vec![player_input_value()]),
            ),
            "InvalidState",
        );
        let zero_slot = BoundaryValue::fields([
            ("slot", BoundaryValue::Int(0)),
            ("generation", boundary::u64_text(1)),
        ]);
        assert_failure(
            &arena.submit_routine(
                &zero_slot,
                &epoch_text(epoch),
                &batch_value(epoch, vec![player_input_value()]),
            ),
            "InvalidInput",
        );
        assert_eq!(
            counts.submits.get(),
            submits_before,
            "stale and unknown tokens made zero core calls"
        );

        // A wrong epoch text against the batch epoch is stale.
        assert_failure(
            &arena.submit_routine(
                &token.to_boundary(),
                &epoch_text(epoch + 1),
                &batch_value(epoch, vec![player_input_value()]),
            ),
            "StaleEpoch",
        );

        // The empty batch submits exactly once and answers Noop.
        let empty_receipt = arena.submit_routine(
            &token.to_boundary(),
            &epoch_text(epoch),
            &batch_value(epoch, Vec::new()),
        );
        let value = assert_success(&empty_receipt);
        let (tag, _) = value.tag().expect("receipt tag");
        assert_eq!(tag, "Noop");
        assert_eq!(counts.submits.get(), submits_before + 1);

        // A successful semantic submit runs exactly one core call and
        // answers the queued receipt with the first sequence.
        let queued = arena.submit_routine(
            &token.to_boundary(),
            &epoch_text(epoch),
            &batch_value(epoch, vec![player_input_value()]),
        );
        let value = assert_success(&queued);
        let (tag, payload) = value.tag().expect("receipt tag");
        assert_eq!(tag, "Queued");
        assert_eq!(
            payload.field("epoch").and_then(|v| v.as_text().ok()),
            Some(epoch.to_string().as_str())
        );
        assert_eq!(
            payload
                .field("first_sequence")
                .and_then(|v| v.as_text().ok()),
            Some("1"),
            "the first issued sequence is one"
        );
        assert_eq!(
            payload
                .field("sequenced_count")
                .and_then(|v| v.as_int_in(1, 1).ok()),
            Some(1)
        );
        assert_eq!(counts.submits.get(), submits_before + 2);

        // An irrelevant container token rejects the complete batch at the
        // C1 whole-batch gate, and an altered reference is an invalid shape.
        let irrelevant = batch_value(
            epoch,
            vec![action_value(
                "MoveInventory",
                BoundaryValue::fields([
                    ("from", BoundaryValue::Int(0)),
                    ("to", BoundaryValue::Int(1)),
                ]),
                container_token_value(epoch, 1),
                BoundaryValue::Null,
            )],
        );
        assert_failure(
            &arena.submit_routine(&token.to_boundary(), &epoch_text(epoch), &irrelevant),
            "InvalidInput",
        );
        let altered = batch_value(
            epoch,
            vec![action_value(
                "MoveContainer",
                BoundaryValue::fields([
                    ("container", container_ref_value(4, 1, 2)),
                    ("from", BoundaryValue::Int(36)),
                    ("to", BoundaryValue::Int(37)),
                ]),
                container_token_value(epoch, 1),
                BoundaryValue::Null,
            )],
        );
        assert_failure(
            &arena.submit_routine(&token.to_boundary(), &epoch_text(epoch), &altered),
            "InvalidInput",
        );
        // The two C1-level rejections each reached the core exactly once
        // and were refused by the whole-batch gate: the counter advanced
        // for every decoded batch and for none of the shape-rejected ones.
        assert_eq!(counts.submits.get(), submits_before + 4);

        // Reset issues the fresh epoch text through the same routine.
        let reset = arena.reset_routine(&token.to_boundary(), &epoch_text(epoch));
        assert_eq!(
            assert_success(&reset).as_text().ok(),
            Some("2"),
            "the fresh epoch is canonical text"
        );
        assert_eq!(counts.resets.get(), 1);

        // A repeated close releases exactly once and retires the token.
        assert!(matches!(
            assert_success(&arena.close_routine(&token.to_boundary())),
            BoundaryValue::Null
        ));
        assert!(matches!(
            assert_success(&arena.close_routine(&token.to_boundary())),
            BoundaryValue::Null
        ));
        assert_eq!(arena.releases(), 1);
        assert_failure(
            &arena.family_table_routine(&token.to_boundary()),
            "InvalidState",
        );
    }

    /// The caught-panic case: the guard maps the panic to `Internal` and
    /// the prior visible frame survives byte for byte.
    #[test]
    fn caught_panic_is_internal_and_preserves_visible_state() {
        let (peer, config) = memory_stack();
        let mut arena = CoreArena::new();
        let token = arena
            .open_with_endpoint(
                &open_spec(),
                Box::new(Panicking {
                    inner: LoginSession::new(config).expect("login session"),
                }),
            )
            .expect("the checked open issues a token");
        let epoch = connect_and_admit(&mut arena, token, &peer, 6);
        let pulled =
            assert_success(&arena.pull_frame_routine(&token.to_boundary(), &epoch_text(epoch)))
                .clone();
        // Silence the injected panic's default hook so the failure output
        // stays readable, then restore it.
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(|_| {}));
        let failed = arena.submit_routine(
            &token.to_boundary(),
            &epoch_text(epoch),
            &batch_value(epoch, vec![player_input_value()]),
        );
        assert_failure(&failed, "Internal");
        std::panic::set_hook(previous);
        assert_eq!(
            arena.visible_frame(token).expect("visible copy"),
            &pulled,
            "the caught panic preserved the prior visible frame"
        );
        assert!(matches!(
            assert_success(&arena.close_routine(&token.to_boundary())),
            BoundaryValue::Null
        ));
        assert_eq!(arena.releases(), 1);
    }

    /// The bounded-text rendering keeps the target label kind: a support
    /// check that the renderers never invent a second text rule.
    #[test]
    fn bounded_text_rendering_uses_the_checked_boundary() {
        let text = BoundedText::try_new("stone".to_string(), TextKind::Target).expect("label");
        assert_eq!(text.as_str(), "stone");
        assert_eq!(text.kind(), &TextKind::Target);
    }

    /// A late argument's marshalling refusal closes the whole call: the
    /// multi-argument facade dispatch admits only fully marshalled argument
    /// lists, so a hostile leaf in any position — including behind an
    /// already-marshalled first argument — answers the closed
    /// invalid-input envelope instead of dispatching (the original defect
    /// dispatched an `Ok` first argument into a dead closure and panicked
    /// across the engine boundary).
    ///
    /// The admission is pinned through the shared engine-free join the
    /// bridge's multi-argument dispatch calls; constructing the hostile
    /// native leaves themselves (packed arrays, vectors, objects, non-finite
    /// floats) needs the engine, so the native conversion and dictionary
    /// dispatch remain engine-deferred and are not claimed here.
    #[test]
    fn late_argument_marshalling_refusal_closes_the_call() {
        let ok = || Ok(BoundaryValue::Text("marshalled".to_string()));
        let refused = || Err(());

        // Two arguments: every refusal permutation, and especially the
        // first-Ok-second-refused one the defect dispatched on.
        assert!(boundary::join_arguments2(ok(), refused()).is_err());
        assert!(boundary::join_arguments2(refused(), ok()).is_err());
        assert!(boundary::join_arguments2(refused(), refused()).is_err());
        let joined = boundary::join_arguments2(ok(), ok()).expect("two admitted arguments");
        assert_eq!(joined.0, BoundaryValue::Text("marshalled".to_string()));
        assert_eq!(joined.1, BoundaryValue::Text("marshalled".to_string()));

        // Three arguments: a refusal in any of the three positions closes
        // the call; the reviewed defect reached the dead closure whenever
        // the first argument marshalled and a later one refused.
        assert!(boundary::join_arguments3(ok(), ok(), refused()).is_err());
        assert!(boundary::join_arguments3(ok(), refused(), ok()).is_err());
        assert!(boundary::join_arguments3(refused(), ok(), ok()).is_err());
        assert!(boundary::join_arguments3(ok(), refused(), refused()).is_err());
        assert!(boundary::join_arguments3(refused(), refused(), refused()).is_err());
        let joined = boundary::join_arguments3(ok(), ok(), ok()).expect("three admitted arguments");
        assert_eq!(joined.2, BoundaryValue::Text("marshalled".to_string()));

        // The refusal the bridge renders for a joined Err is the closed
        // invalid-input envelope — the `Internal` class stays reserved for
        // genuine routine panics caught by the guard.
        let refusal = boundary::outcome(Err(ClientError::InvalidInput));
        assert_failure(&refusal, "InvalidInput");
    }
}
