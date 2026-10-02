//! The bridge release-order and panic-containment contract target: the
//! code-level ownership that releases every consumer of a rust core before
//! the single native release, ignores late callbacks after close, and
//! contains native panics as the closed `Internal` envelope.
//!
//! The crate root keeps the ownership module private, so this registered
//! target includes the production sources by path (the established
//! convention of the rust-producer target). Everything exercised here is
//! the real release ownership over the real adapter arena and the real C1
//! login endpoint; the only doubles are thin wrappers around that real
//! endpoint for deterministic tracing and panic injection, plus consumer
//! doubles standing in for host features. No engine is loaded and no
//! native bridge harness is run: those qualifications stay deferred.

#![allow(dead_code)]

#[path = "../src/abi.rs"]
mod abi;

#[path = "../src/client_core.rs"]
mod client_core;

#[path = "../src/feature_negotiation.rs"]
mod feature_negotiation;

#[path = "../src/lifecycle.rs"]
mod lifecycle;

mod lifecycle_contract {
    use std::cell::RefCell;
    use std::num::NonZeroU64;
    use std::rc::Rc;
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant};

    use mornlea_client_core::contracts::{
        ClientConfig, ClientEndpoint, ClientError, ClientIdentity, ClientLimits, ClientWorkBudget,
        Connector, ConnectorRegistry, Endpoint, InputReceipt, MonotonicClock, SessionEpoch,
        StepReport,
    };
    use mornlea_client_core::input::InputBatch;
    use mornlea_client_core::presentation::frame::PresentationFrame;
    use mornlea_client_core::session::io::{MemoryConnector, MemoryPeer};
    use mornlea_client_core::session::login::LoginSession;
    use mornlea_domain::{Identities, PlayerId};
    use mornlea_protocol::{LoginSuccess, ServerHello, write_frame};

    use super::abi::boundary::{self, BoundaryValue, CoreTokenValue};
    use super::client_core::rust_core::CoreArena;
    use super::lifecycle::ownership::{CoreConsumer, CoreOwnership};

    // ------------------------------------------------------------------
    // Deterministic fixtures (the rust-producer target's conventions)
    // ------------------------------------------------------------------

    /// The shared deterministic trace: endpoint calls, consumer callbacks,
    /// and ownership actions append one line each, in order.
    type Trace = Rc<RefCell<Vec<String>>>;

    fn record(trace: &Trace, line: &str) {
        trace.borrow_mut().push(line.to_string());
    }

    fn count(trace: &Trace, line: &str) -> usize {
        trace
            .borrow()
            .iter()
            .filter(|item| item.as_str() == line)
            .count()
    }

    fn position(trace: &Trace, line: &str) -> usize {
        trace
            .borrow()
            .iter()
            .position(|item| item.as_str() == line)
            .expect(line)
    }

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

    /// One PlayerInput action value.
    fn player_input_value() -> BoundaryValue {
        BoundaryValue::fields([
            (
                "intent",
                BoundaryValue::tagged(
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
                ),
            ),
            ("container", BoundaryValue::Null),
            ("crafting", BoundaryValue::Null),
        ])
    }

    fn batch_value(epoch: u64, actions: Vec<BoundaryValue>) -> BoundaryValue {
        BoundaryValue::fields([
            ("epoch", boundary::u64_text(epoch)),
            ("actions", BoundaryValue::List(actions)),
        ])
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

    /// A tracing wrapper around the real login session: every endpoint call
    /// appends one deterministic trace line and delegates unchanged, so the
    /// tests read the exact moment the adapter touched the native core.
    struct Traced {
        inner: LoginSession,
        trace: Trace,
    }

    impl ClientEndpoint for Traced {
        fn connect(
            &mut self,
            endpoint: Endpoint,
            identity: ClientIdentity,
        ) -> Result<SessionEpoch, ClientError> {
            record(&self.trace, "endpoint:connect");
            self.inner.connect(endpoint, identity)
        }

        fn submit_input(
            &mut self,
            epoch: SessionEpoch,
            input: InputBatch,
        ) -> Result<InputReceipt, ClientError> {
            record(&self.trace, "endpoint:submit");
            self.inner.submit_input(epoch, input)
        }

        fn step(
            &mut self,
            epoch: SessionEpoch,
            work: ClientWorkBudget,
        ) -> Result<StepReport, ClientError> {
            record(&self.trace, "endpoint:step");
            self.inner.step(epoch, work)
        }

        fn snapshot(&self, epoch: SessionEpoch) -> Result<Arc<PresentationFrame>, ClientError> {
            record(&self.trace, "endpoint:snapshot");
            self.inner.snapshot(epoch)
        }

        fn reset(&mut self, epoch: SessionEpoch) -> Result<SessionEpoch, ClientError> {
            record(&self.trace, "endpoint:reset");
            self.inner.reset(epoch)
        }

        fn close(&mut self) -> Result<(), ClientError> {
            record(&self.trace, "endpoint:close");
            self.inner.close()
        }
    }

    /// A submit-panicking wrapper: `submit_input` records its trace line and
    /// panics, every other call records and delegates. The ownership must
    /// route input through the adapter's guarded facade routine so the
    /// panic is contained as the closed `Internal` envelope.
    struct PanickingSubmit {
        inner: LoginSession,
        trace: Trace,
    }

    impl ClientEndpoint for PanickingSubmit {
        fn connect(
            &mut self,
            endpoint: Endpoint,
            identity: ClientIdentity,
        ) -> Result<SessionEpoch, ClientError> {
            record(&self.trace, "endpoint:connect");
            self.inner.connect(endpoint, identity)
        }

        fn submit_input(
            &mut self,
            _epoch: SessionEpoch,
            _input: InputBatch,
        ) -> Result<InputReceipt, ClientError> {
            record(&self.trace, "endpoint:submit");
            panic!("injected native submit panic");
        }

        fn step(
            &mut self,
            epoch: SessionEpoch,
            work: ClientWorkBudget,
        ) -> Result<StepReport, ClientError> {
            record(&self.trace, "endpoint:step");
            self.inner.step(epoch, work)
        }

        fn snapshot(&self, epoch: SessionEpoch) -> Result<Arc<PresentationFrame>, ClientError> {
            record(&self.trace, "endpoint:snapshot");
            self.inner.snapshot(epoch)
        }

        fn reset(&mut self, epoch: SessionEpoch) -> Result<SessionEpoch, ClientError> {
            record(&self.trace, "endpoint:reset");
            self.inner.reset(epoch)
        }

        fn close(&mut self) -> Result<(), ClientError> {
            record(&self.trace, "endpoint:close");
            self.inner.close()
        }
    }

    /// A close-panicking wrapper: the native release itself records its
    /// trace line and panics, every other call records and delegates. The
    /// ownership must still have released every consumer first, contain the
    /// panic as `Internal`, and drop the released token so no second
    /// release attempt can exist.
    struct PanickingClose {
        inner: LoginSession,
        trace: Trace,
    }

    impl ClientEndpoint for PanickingClose {
        fn connect(
            &mut self,
            endpoint: Endpoint,
            identity: ClientIdentity,
        ) -> Result<SessionEpoch, ClientError> {
            record(&self.trace, "endpoint:connect");
            self.inner.connect(endpoint, identity)
        }

        fn submit_input(
            &mut self,
            epoch: SessionEpoch,
            input: InputBatch,
        ) -> Result<InputReceipt, ClientError> {
            record(&self.trace, "endpoint:submit");
            self.inner.submit_input(epoch, input)
        }

        fn step(
            &mut self,
            epoch: SessionEpoch,
            work: ClientWorkBudget,
        ) -> Result<StepReport, ClientError> {
            record(&self.trace, "endpoint:step");
            self.inner.step(epoch, work)
        }

        fn snapshot(&self, epoch: SessionEpoch) -> Result<Arc<PresentationFrame>, ClientError> {
            record(&self.trace, "endpoint:snapshot");
            self.inner.snapshot(epoch)
        }

        fn reset(&mut self, epoch: SessionEpoch) -> Result<SessionEpoch, ClientError> {
            record(&self.trace, "endpoint:reset");
            self.inner.reset(epoch)
        }

        fn close(&mut self) -> Result<(), ClientError> {
            record(&self.trace, "endpoint:close");
            panic!("injected native release panic");
        }
    }

    // ------------------------------------------------------------------
    // Consumer doubles and ownership plumbing
    // ------------------------------------------------------------------

    /// The shared state cell of one consumer double: the double lives
    /// inside the ownership, so the test reads and stages it through this
    /// handle while the ownership holds its own reference.
    #[derive(Default)]
    struct ConsumerCell {
        preparation: Option<&'static str>,
        frames: usize,
        released: bool,
    }

    type ConsumerState = Rc<RefCell<ConsumerCell>>;

    /// One host-feature stand-in: a consumer of the live core whose apply,
    /// invalidate, and release callbacks append deterministic trace lines.
    struct FeatureConsumer {
        name: &'static str,
        trace: Trace,
        state: ConsumerState,
    }

    impl CoreConsumer for FeatureConsumer {
        fn apply(&mut self, _frame: &BoundaryValue) {
            self.state.borrow_mut().frames += 1;
            record(&self.trace, &format!("consumer[{}]:apply", self.name));
        }

        fn invalidate(&mut self) {
            self.state.borrow_mut().preparation = None;
            record(&self.trace, &format!("consumer[{}]:reset", self.name));
        }

        fn release(&mut self) {
            self.state.borrow_mut().released = true;
            record(&self.trace, &format!("consumer[{}]:release", self.name));
        }
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

    fn server_hello_frame() -> Vec<u8> {
        let hello = ServerHello::new(Identities::current().protocol).expect("current hello");
        write_frame(ServerHello::PACKET_ID, &hello.encode().expect("payload")).expect("hello frame")
    }

    fn login_success_frame(for_player: PlayerId) -> Vec<u8> {
        let payload = LoginSuccess::new(for_player, 7).encode().expect("payload");
        write_frame(LoginSuccess::PACKET_ID, &payload).expect("login frame")
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

    /// One ownership over the real arena and a wrapped real login session,
    /// beside the issued token and the fixture peer.
    fn opened_ownership<T: ClientEndpoint + 'static>(
        wrap: impl FnOnce(LoginSession) -> T,
    ) -> (CoreOwnership, CoreTokenValue, MemoryPeer) {
        let (peer, config) = memory_stack();
        let mut ownership = CoreOwnership::new(CoreArena::new());
        let token = ownership
            .open_with_endpoint(
                &open_spec(),
                Box::new(wrap(LoginSession::new(config).expect("login session"))),
            )
            .expect("the checked open issues a token");
        (ownership, token, peer)
    }

    /// Registers one consumer double whose shared state the test keeps.
    fn register_feature(
        ownership: &mut CoreOwnership,
        name: &'static str,
        trace: &Trace,
    ) -> (ConsumerState, super::lifecycle::ownership::ConsumerHandle) {
        let state: ConsumerState = Rc::new(RefCell::new(ConsumerCell::default()));
        let handle = ownership
            .register_consumer(Box::new(FeatureConsumer {
                name,
                trace: Rc::clone(trace),
                state: Rc::clone(&state),
            }))
            .expect("a live core accepts consumers");
        (state, handle)
    }

    /// Connects and admits through the ownership's own dispatch, so the
    /// wrapped session owns a live epoch and a real visible frame.
    fn connect_and_admit(ownership: &mut CoreOwnership, peer: &MemoryPeer, seed: u8) {
        let connected = ownership.connect(&memory_endpoint_value(), &identity_value(seed));
        assert_success(&connected);
        peer.feed(&server_hello_frame()).expect("hello fed");
        assert!(ownership.tick(&work_value(4, 0)));
        peer.feed(&login_success_frame(player(seed)))
            .expect("login fed");
        assert!(ownership.tick(&work_value(4, 0)));
    }

    // ==================================================================
    // Case: consumers are released before the native core release
    // ==================================================================

    #[test]
    fn consumers_before_core() {
        let trace: Trace = Rc::new(RefCell::new(Vec::new()));
        let (mut ownership, _token, peer) = opened_ownership(|inner| Traced {
            inner,
            trace: Rc::clone(&trace),
        });
        let (provider_state, _provider) = register_feature(&mut ownership, "provider", &trace);
        let (consumer_state, _consumer) = register_feature(&mut ownership, "consumer", &trace);
        connect_and_admit(&mut ownership, &peer, 3);

        // The admitted session applies frames in registration order:
        // providers before consumers, one frame per consumer per tick.
        assert!(
            position(&trace, "consumer[provider]:apply")
                < position(&trace, "consumer[consumer]:apply"),
            "providers apply before consumers"
        );
        let applied = provider_state.borrow().frames;
        assert!(applied >= 1);
        assert_eq!(consumer_state.borrow().frames, applied);

        // The close releases every consumer first — consumers before
        // providers, in the exact reverse of registration order — and only
        // then performs the one native release.
        assert!(matches!(
            assert_success(&ownership.close()),
            BoundaryValue::Null
        ));
        let consumer_release = position(&trace, "consumer[consumer]:release");
        let provider_release = position(&trace, "consumer[provider]:release");
        let native_release = position(&trace, "endpoint:close");
        assert!(consumer_release < provider_release, "reverse registration");
        assert!(
            provider_release < native_release,
            "consumers are released before the native release"
        );
        assert_eq!(count(&trace, "endpoint:close"), 1, "one native release");
        assert_eq!(ownership.releases(), 1, "the adapter released exactly once");

        // Zero retained test-owned handles: the registry is empty, both
        // consumer identities are retired, and the owned token, epoch, and
        // retained frame are gone.
        assert_eq!(ownership.retained_consumers(), 0);
        assert!(!ownership.is_registered(_provider));
        assert!(!ownership.is_registered(_consumer));
        assert!(provider_state.borrow().released && consumer_state.borrow().released);
        assert_eq!(ownership.epoch(), "");
        assert!(ownership.retained_frame().is_none());

        // A repeated close performs no second release wave.
        assert!(matches!(
            assert_success(&ownership.close()),
            BoundaryValue::Null
        ));
        assert_eq!(count(&trace, "endpoint:close"), 1);
        assert_eq!(count(&trace, "consumer[provider]:release"), 1);
        assert_eq!(count(&trace, "consumer[consumer]:release"), 1);
        assert_eq!(ownership.releases(), 1);
    }

    // ==================================================================
    // Case: late callbacks after close are ignored
    // ==================================================================

    #[test]
    fn late_callback_ignored() {
        let trace: Trace = Rc::new(RefCell::new(Vec::new()));
        let (mut ownership, token, peer) = opened_ownership(|inner| Traced {
            inner,
            trace: Rc::clone(&trace),
        });
        let (_solo_state, solo) = register_feature(&mut ownership, "solo", &trace);
        connect_and_admit(&mut ownership, &peer, 5);
        assert!(matches!(
            assert_success(&ownership.close()),
            BoundaryValue::Null
        ));
        let epoch = boundary::u64_text(1);

        // A late tick is a stale callback: no dispatch happens at all, so
        // no step, no pull, and no consumer apply can occur.
        let steps = count(&trace, "endpoint:step");
        let snapshots = count(&trace, "endpoint:snapshot");
        let applies = count(&trace, "consumer[solo]:apply");
        assert!(!ownership.tick(&work_value(4, 0)));
        assert_eq!(count(&trace, "endpoint:step"), steps);
        assert_eq!(count(&trace, "endpoint:snapshot"), snapshots);
        assert_eq!(count(&trace, "consumer[solo]:apply"), applies);

        // The retired token earns the adapter's typed rejection before any
        // core dereference: zero native submits behind the refusal.
        let submits = count(&trace, "endpoint:submit");
        assert_failure(
            &ownership.submit_input(
                &token.to_boundary(),
                &epoch,
                &batch_value(1, vec![player_input_value()]),
            ),
            "InvalidState",
        );
        assert_eq!(count(&trace, "endpoint:submit"), submits);

        // A stale generation and an unknown slot reject the same way.
        let stale =
            CoreTokenValue::new(token.slot(), token.generation() + 1).expect("stale token shape");
        assert_failure(
            &ownership.submit_input(
                &stale.to_boundary(),
                &epoch,
                &batch_value(1, vec![player_input_value()]),
            ),
            "InvalidState",
        );
        let unknown = CoreTokenValue::new(token.slot() + 10, token.generation())
            .expect("unknown token shape");
        assert_failure(
            &ownership.submit_input(
                &unknown.to_boundary(),
                &epoch,
                &batch_value(1, vec![player_input_value()]),
            ),
            "InvalidState",
        );
        assert_eq!(count(&trace, "endpoint:submit"), submits);

        // A late reset dispatches nothing: the native core is untouched and
        // the retired ownership answers the typed refusal.
        let resets = count(&trace, "endpoint:reset");
        assert_failure(&ownership.reset(), "InvalidState");
        assert_eq!(count(&trace, "endpoint:reset"), resets);

        // The released consumer's registration is gone for good.
        assert!(!ownership.is_registered(solo));
        assert_eq!(ownership.retained_consumers(), 0);
        assert_eq!(ownership.releases(), 1);
    }

    // ==================================================================
    // Case: a native panic is contained as the Internal envelope
    // ==================================================================

    #[test]
    fn native_panic_is_internal() {
        // Phase one: a native panic inside a routed input routine answers
        // the closed Internal envelope while the prior visible state
        // survives on both the ownership and the adapter's own slot.
        let trace: Trace = Rc::new(RefCell::new(Vec::new()));
        let (mut ownership, token, peer) = opened_ownership(|inner| PanickingSubmit {
            inner,
            trace: Rc::clone(&trace),
        });
        let (solo_state, _solo) = register_feature(&mut ownership, "solo", &trace);
        connect_and_admit(&mut ownership, &peer, 6);
        assert!(ownership.tick(&work_value(4, 0)));
        let retained = ownership.retained_frame().expect("retained copy").clone();
        let visible = ownership
            .visible_frame()
            .expect("slot visible copy")
            .clone();
        let epoch = boundary::u64_text(1);

        // Silence the injected panic's default hook so the failure output
        // stays readable, then restore it.
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(|_| {}));
        let failed = ownership.submit_input(
            &token.to_boundary(),
            &epoch,
            &batch_value(1, vec![player_input_value()]),
        );
        std::panic::set_hook(previous);
        assert_failure(&failed, "Internal");
        let error = failed.field("error").expect("error key");
        error
            .exact_fields(&["class", "resource", "limit", "observed"])
            .expect("the closed error field set");
        assert_eq!(
            ownership.retained_frame(),
            Some(&retained),
            "the ownership's visible state survived the panic"
        );
        assert_eq!(
            ownership.visible_frame(),
            Some(&visible),
            "the adapter's slot-visible copy survived the panic"
        );

        // The panic is not a release: the ownership stays live and its
        // release path still runs consumers first with one native release.
        assert_eq!(ownership.epoch(), "1");
        assert_eq!(ownership.retained_consumers(), 1);
        assert!(matches!(
            assert_success(&ownership.close()),
            BoundaryValue::Null
        ));
        assert!(
            position(&trace, "consumer[solo]:release") < position(&trace, "endpoint:close"),
            "consumers are released before the native release"
        );
        assert_eq!(ownership.releases(), 1);
        assert!(solo_state.borrow().released);

        // Phase two: a native panic inside the release itself is contained
        // the same way. The consumers were already released first, the
        // panicked token is dropped, and no second attempt exists.
        let trace: Trace = Rc::new(RefCell::new(Vec::new()));
        let (mut ownership, _token, peer) = opened_ownership(|inner| PanickingClose {
            inner,
            trace: Rc::clone(&trace),
        });
        let (solo_state, _solo) = register_feature(&mut ownership, "solo", &trace);
        connect_and_admit(&mut ownership, &peer, 7);
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(|_| {}));
        let failed_close = ownership.close();
        std::panic::set_hook(previous);
        assert_failure(&failed_close, "Internal");
        assert!(
            position(&trace, "consumer[solo]:release") < position(&trace, "endpoint:close"),
            "consumers were released before the panicked native release"
        );
        assert_eq!(count(&trace, "endpoint:close"), 1, "one release attempt");
        assert_eq!(ownership.retained_consumers(), 0);
        assert!(solo_state.borrow().released);

        // The dropped token means a repeated close dispatches nothing: no
        // second native release attempt and no panic replay.
        assert!(matches!(
            assert_success(&ownership.close()),
            BoundaryValue::Null
        ));
        assert_eq!(count(&trace, "endpoint:close"), 1);
    }

    // ==================================================================
    // Case: reset with queued input and staged preparation
    // ==================================================================

    #[test]
    fn reset_with_queued_input_and_preparation() {
        let trace: Trace = Rc::new(RefCell::new(Vec::new()));
        let (mut ownership, token, peer) = opened_ownership(|inner| Traced {
            inner,
            trace: Rc::clone(&trace),
        });
        let (provider_state, _provider) = register_feature(&mut ownership, "provider", &trace);
        let (consumer_state, _consumer) = register_feature(&mut ownership, "consumer", &trace);
        connect_and_admit(&mut ownership, &peer, 4);
        assert!(ownership.tick(&work_value(4, 0)));
        assert!(ownership.retained_frame().is_some());
        let live_epoch = ownership.epoch().to_string();
        let live_epoch_value = boundary::u64_text(1);

        // Queue real input on the live epoch through the actual routine.
        let queued = ownership.submit_input(
            &token.to_boundary(),
            &live_epoch_value,
            &batch_value(1, vec![player_input_value()]),
        );
        let value = assert_success(&queued);
        let (tag, payload) = value.tag().expect("receipt tag");
        assert_eq!(tag, "Queued");
        assert_eq!(
            payload
                .field("first_sequence")
                .and_then(|v| v.as_text().ok()),
            Some("1"),
            "the queued input holds the first sequence"
        );

        // Stage preparation on both consumers of the live epoch.
        provider_state.borrow_mut().preparation = Some("staged provider work");
        consumer_state.borrow_mut().preparation = Some("staged consumer work");

        // Reset: the native reset runs first, then every consumer's queued
        // state is invalidated in registration order (providers before
        // consumers), before anything can be applied onto the fresh epoch.
        let reset = ownership.reset();
        assert_eq!(
            assert_success(&reset).as_text().ok(),
            Some("2"),
            "the fresh epoch is canonical text"
        );
        let native_reset = position(&trace, "endpoint:reset");
        let provider_reset = position(&trace, "consumer[provider]:reset");
        let consumer_reset = position(&trace, "consumer[consumer]:reset");
        assert_eq!(count(&trace, "endpoint:reset"), 1);
        assert!(
            native_reset < provider_reset,
            "the native reset answers first"
        );
        assert!(
            provider_reset < consumer_reset,
            "invalidation follows registration order"
        );

        // The staged preparation is gone and the retained frame copy of the
        // retired epoch was dropped, while the fresh epoch was adopted.
        assert!(provider_state.borrow().preparation.is_none());
        assert!(consumer_state.borrow().preparation.is_none());
        assert!(ownership.retained_frame().is_none());
        assert_eq!(ownership.epoch(), "2");

        // The retired epoch's queued input cannot cross onto the fresh
        // epoch: the adapter answers the typed stale-epoch refusal.
        assert_failure(
            &ownership.submit_input(
                &token.to_boundary(),
                &BoundaryValue::Text(live_epoch.clone()),
                &batch_value(1, vec![player_input_value()]),
            ),
            "StaleEpoch",
        );

        // The ownership still closes exactly once, consumers first.
        assert!(matches!(
            assert_success(&ownership.close()),
            BoundaryValue::Null
        ));
        assert!(
            position(&trace, "consumer[consumer]:release")
                < position(&trace, "consumer[provider]:release")
                && position(&trace, "consumer[provider]:release")
                    < position(&trace, "endpoint:close"),
            "consumers are released before the one native release"
        );
        assert_eq!(count(&trace, "endpoint:close"), 1);
        assert_eq!(ownership.releases(), 1);
        assert_eq!(ownership.retained_consumers(), 0);
    }
}
