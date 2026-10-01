//! The login provider cases: state-checked hello/login before play, the
//! accepted monotonic deadlines, one terminal publication and one transport
//! release per session, and the real lifecycle Open/terminal records.
//!
//! The cases drive the session under test through the deterministic replay
//! harness (shared clock, shared memory connector) so the sent frames, the
//! release counter and the published session/lifecycle records are observed
//! through the landed checked surfaces only. The wrong-behavior artifact at
//! the bottom keeps the contract double's rejected login behavior executable:
//! it performs no hello/login exchange, enforces no deadline and republishes
//! its terminal on every step.

use std::num::NonZeroU64;
use std::time::Duration;

use crate::support::{DoubleMode, ReplayHarness, frame_server_packet};
use mornlea_client_core::contracts::{
    ClientError, ClientIdentity, ClientWorkBudget, CloseReason, Endpoint, FAMILY_LIFECYCLE,
    FAMILY_SESSION, InputReceipt, SessionEpoch, SessionPhase, StepReport,
};
use mornlea_client_core::input::{ClientIntent, InputAction, InputBatch};
use mornlea_client_core::presentation::frame::LifecycleTransition;
use mornlea_client_core::presentation::frame::PresentationFrame;
use mornlea_client_core::presentation::{BoundedText, FamilyRecords, ResourceKey, TextKind};
use mornlea_client_core::session::login::LoginSession;
use mornlea_client_core::{ClientEndpoint, Connector};
use mornlea_domain::{
    HeldActions, Identities, LookAngles, Movement, PlayerControl, PlayerControlParts, PlayerId,
};
use mornlea_protocol::{
    ClientHello, LoginReject, LoginStart, LoginSuccess, ServerPacket, write_frame,
};

/// Which session implementation the table drives.
enum Driver {
    Double,
    Provider,
}

/// The implementation the named cases run against. The red run of this table
/// drove `Double` — the only session behavior that existed at the contract
/// landing — and its recorded wrong behavior is kept executable by the
/// artifact case below.
const DRIVER: Driver = Driver::Provider;

/// The session under test beside its deterministic clock and transport.
struct UnderTest {
    harness: ReplayHarness,
    identity: ClientIdentity,
    epoch: Option<SessionEpoch>,
    provider: Option<LoginSession>,
}

impl UnderTest {
    fn new() -> Self {
        match DRIVER {
            Driver::Double => Self::double(),
            Driver::Provider => Self::provider(),
        }
    }

    /// The contract landing's deterministic double, whose login path is the
    /// recorded wrong behavior the real provider replaces.
    fn double() -> Self {
        let harness = ReplayHarness::new(DoubleMode::Contract).expect("harness");
        Self {
            harness,
            identity: identity("driver", 9),
            epoch: None,
            provider: None,
        }
    }

    /// The real login provider over the same deterministic clock, memory
    /// connector and accepted deadline policy.
    fn provider() -> Self {
        let harness = ReplayHarness::new(DoubleMode::Contract).expect("harness");
        let config = harness.config().expect("checked config");
        let provider = LoginSession::new(config).expect("login session");
        Self {
            harness,
            identity: identity("driver", 9),
            epoch: None,
            provider: Some(provider),
        }
    }

    fn connect(&mut self, name: &str, byte: u8) -> SessionEpoch {
        self.identity = identity(name, byte);
        let epoch = match self.provider.as_mut() {
            Some(provider) => provider
                .connect(memory_endpoint(), self.identity.clone())
                .expect("pending epoch"),
            None => self
                .harness
                .connect(self.identity.clone())
                .expect("pending epoch"),
        };
        self.epoch = Some(epoch);
        epoch
    }

    fn epoch(&self) -> SessionEpoch {
        self.epoch.expect("connected")
    }

    /// Delivers one complete server frame to the session.
    fn feed(&mut self, frame: Vec<u8>) {
        match self.provider.as_ref() {
            Some(_) => self.harness.connector.feed_frame(frame),
            None => {
                let _ = self.harness.double.receive_bytes(&frame);
            }
        }
    }

    fn advance(&mut self, by: Duration) {
        self.harness.clock.advance(by);
    }

    fn step(&mut self) -> Result<StepReport, ClientError> {
        let epoch = self.epoch();
        let budget = ClientWorkBudget::try_new(4, 0).expect("budget");
        match self.provider.as_mut() {
            Some(provider) => provider.step(epoch, budget),
            None => self.harness.step(epoch, budget),
        }
    }

    fn submit(&mut self, batch: InputBatch) -> Result<InputReceipt, ClientError> {
        let epoch = self.epoch();
        match self.provider.as_mut() {
            Some(provider) => provider.submit_input(epoch, batch),
            None => self.harness.submit_input(epoch, batch),
        }
    }

    /// The visible frame of the session under test.
    fn visible(&self) -> std::sync::Arc<PresentationFrame> {
        match self.provider.as_ref() {
            Some(provider) => provider.snapshot(self.epoch()).expect("visible frame"),
            None => self.harness.snapshot(self.epoch()).expect("visible frame"),
        }
    }

    /// Simulates the peer vanishing: the shared transport reports `Closed`
    /// afterwards. The release it counts belongs to the peer, not the session.
    fn peer_closes_transport(&mut self) {
        let ticket = mornlea_client_core::contracts::TransportTicket::try_new(
            NonZeroU64::new(1).expect("one"),
            NonZeroU64::new(u64::MAX).expect("max generation"),
        )
        .expect("ticket shape");
        self.harness.connector.close(ticket).expect("peer close");
    }

    fn double_close(&mut self) {
        self.harness
            .double_close_checked(self.epoch())
            .expect("double close");
    }

    fn sent(&self) -> Vec<Vec<u8>> {
        self.harness.connector.sent()
    }

    fn releases(&self) -> u32 {
        self.harness.connector.releases()
    }

    /// The published session record's phase, confirmed player identity and
    /// terminal reason, read from the visible frame.
    fn session_parts(&self) -> (SessionPhase, Option<PlayerId>, Option<CloseReason>) {
        let frame = self.visible();
        let family = frame
            .families()
            .iter()
            .find(|family| family.key().logical_name == FAMILY_SESSION)
            .expect("session family");
        let FamilyRecords::Session(records) = family.records() else {
            panic!("session records");
        };
        assert_eq!(records.len(), 1, "one session record per publication");
        let record = &records[0];
        (
            *record.phase(),
            record.player_id(),
            record.terminal().cloned(),
        )
    }

    fn session_phase(&self) -> SessionPhase {
        self.session_parts().0
    }

    fn session_terminal(&self) -> Option<CloseReason> {
        self.session_parts().2
    }

    /// The published lifecycle records as transition, generation, resource
    /// order and header revision tuples, in publication order.
    fn lifecycle_records(&self) -> Vec<(LifecycleTransition, u64, Vec<ResourceKey>, u64)> {
        let frame = self.visible();
        let Some(family) = frame
            .families()
            .iter()
            .find(|family| family.key().logical_name == FAMILY_LIFECYCLE)
        else {
            return Vec::new();
        };
        let FamilyRecords::Lifecycle(records) = family.records() else {
            panic!("lifecycle records");
        };
        records
            .iter()
            .map(|record| {
                (
                    record.transition(),
                    record.generation(),
                    record.resource_order().to_vec(),
                    record.header().revision().get(),
                )
            })
            .collect()
    }
}

fn identity(name: &str, byte: u8) -> ClientIdentity {
    let login = LoginStart::new(player(byte), name, 8).expect("login");
    ClientIdentity::try_new(login).expect("identity")
}

/// The registered memory connector endpoint the provider connects through.
fn memory_endpoint() -> Endpoint {
    Endpoint::Memory {
        connector_id: NonZeroU64::new(1).expect("one"),
    }
}

fn player(byte: u8) -> PlayerId {
    PlayerId::try_from_bytes([0, 0, 0, 0, 0, 0, 0x40, 0, 0x80, 0, 0, 0, 0, 0, 0, byte])
        .expect("uuid v4")
}

fn control() -> PlayerControl {
    PlayerControl::new(PlayerControlParts {
        movement: Movement {
            move_x: 1,
            move_z: 0,
            jump: false,
        },
        look: LookAngles::try_new(0.0, 0.0).expect("finite look"),
        actions: HeldActions {
            primary: false,
            eating: false,
            sprinting: false,
            sneaking: false,
        },
    })
}

fn batch(epoch: SessionEpoch) -> InputBatch {
    InputBatch::try_new(
        epoch,
        vec![InputAction {
            intent: ClientIntent::PlayerInput(control()),
            container: None,
            crafting: None,
        }],
    )
    .expect("batch shape")
}

fn control_text(text: &str) -> BoundedText {
    BoundedText::try_new(text.to_string(), TextKind::Control).expect("control text")
}

/// The client's own current-version hello frame, built through the landed
/// encoder so the expected bytes are never a second encoder.
fn hello_frame() -> Vec<u8> {
    let hello = ClientHello::new(Identities::current().protocol).expect("current hello");
    write_frame(
        ClientHello::PACKET_ID,
        &hello.encode().expect("hello payload"),
    )
    .expect("hello frame")
}

/// The client's login start frame for one checked identity.
fn login_frame(for_identity: &ClientIdentity) -> Vec<u8> {
    write_frame(
        LoginStart::PACKET_ID,
        &for_identity.login().encode().expect("login payload"),
    )
    .expect("login frame")
}

fn login_success_frame(for_player: PlayerId) -> Vec<u8> {
    frame_server_packet(&ServerPacket::LoginSuccess(LoginSuccess::new(
        for_player, 7,
    )))
}

fn login_reject_frame(code: u8, message: &str) -> Vec<u8> {
    frame_server_packet(&ServerPacket::LoginReject(
        LoginReject::new(code, message.to_string()).expect("login reject"),
    ))
}

/// A frame whose declared body is exactly one byte over the accepted 2 MiB
/// frame body cap (`mornlea_protocol::MAX_FRAME_BYTES`): the canonical uvarint
/// of 2 MiB + 1 followed by a stand-in body byte. The landed framer refuses to
/// build it, which is the point.
fn oversized_frame() -> Vec<u8> {
    vec![0x81, 0x80, 0x80, 0x01, 0x00]
}

/// The resource keys the client core actually owns, in release order:
/// no Python feature or native bridge handle identity is listed.
fn core_resources() -> Vec<ResourceKey> {
    vec![
        ResourceKey::InputJournal,
        ResourceKey::PreparationQueue,
        ResourceKey::PresentationFrames,
    ]
}

/// `login::memory_success_admits_after_complete_login`: the memory connector
/// admits only after the complete hello/login exchange — Connecting, then
/// Handshaking with the login start sent, then Admitted with the confirmed
/// identity — and input before the exchange rejects with the typed state
/// error while input after it admits locally.
#[test]
fn memory_success_admits_after_complete_login() {
    let mut test = UnderTest::new();
    let epoch = test.connect("admits", 3);
    assert_ne!(epoch.get(), 0, "the pending epoch is nonzero");
    assert_eq!(test.session_phase(), SessionPhase::Connecting);

    // Input before the complete exchange rejects: nothing is admitted while
    // the session is still working through hello/login.
    let error = test
        .submit(batch(epoch))
        .expect_err("input before complete login rejects");
    assert_eq!(error, ClientError::InvalidState);

    // The server hello moves the session to Handshaking and sends the login
    // start; the exact frames are the landed encoder's bytes.
    test.feed(hello_frame());
    let report = test.step().expect("hello step");
    assert!(report.terminal().is_none());
    assert_eq!(test.session_phase(), SessionPhase::Handshaking);
    assert_eq!(
        test.sent(),
        vec![hello_frame(), login_frame(&test.identity)],
        "hello then login start, byte for byte"
    );
    assert_eq!(test.releases(), 0, "no release while logging in");

    // The login success admits the session with the confirmed identity.
    test.feed(login_success_frame(player(3)));
    test.step().expect("login step");
    assert_eq!(test.session_phase(), SessionPhase::Admitted);
    let (phase, confirmed, terminal) = test.session_parts();
    assert_eq!(phase, SessionPhase::Admitted);
    assert_eq!(confirmed, Some(player(3)), "the confirmed player identity");
    assert_eq!(terminal, None);
    assert_eq!(
        test.releases(),
        0,
        "an admitted session holds its transport"
    );

    // Input after the complete exchange admits locally through the real
    // landed admission path.
    let receipt = test.submit(batch(epoch)).expect("admitted input");
    assert!(matches!(receipt, InputReceipt::Queued { .. }));
}

/// `login::bad_login_exchange_frames_reject`: a wrong-version hello, a login
/// packet before the hello, a truncated hello, a frame one byte over the
/// accepted 2 MiB body cap, a duplicate login success and a login success for
/// another identity each reject — no phase transition, no terminal, no extra
/// frame sent, no release — and none of them terminates the session.
#[test]
fn bad_login_exchange_frames_reject() {
    // Every bad frame at the Connecting phase: still Connecting afterwards.
    let bad_frames: [(&str, Vec<u8>); 4] = [
        (
            "wrong v45",
            write_frame(0, &[44]).expect("wrong-version frame"),
        ),
        (
            "login packet before the hello",
            login_success_frame(player(3)),
        ),
        (
            "truncated hello",
            write_frame(0, &[]).expect("truncated frame"),
        ),
        ("2 MiB body plus one", oversized_frame()),
    ];
    for (name, frame) in bad_frames {
        let mut test = UnderTest::new();
        test.connect("reject", 4);
        test.feed(frame);
        let report = test.step().expect("a rejected frame is not a failed step");
        assert!(report.terminal().is_none(), "{name}: no terminal");
        assert_eq!(
            test.session_phase(),
            SessionPhase::Connecting,
            "{name}: no transition"
        );
        assert_eq!(
            test.sent(),
            vec![hello_frame()],
            "{name}: the login start is not sent"
        );
        assert_eq!(test.releases(), 0, "{name}: no release");
    }

    // A duplicate login success after admission rejects the same way.
    let mut test = UnderTest::new();
    let epoch = test.connect("duplicate", 5);
    test.feed(hello_frame());
    test.step().expect("hello step");
    test.feed(login_success_frame(player(5)));
    test.step().expect("login step");
    assert_eq!(test.session_phase(), SessionPhase::Admitted);
    test.feed(login_success_frame(player(5)));
    let report = test
        .step()
        .expect("duplicate login rejects, not terminates");
    assert!(report.terminal().is_none());
    assert_eq!(test.session_phase(), SessionPhase::Admitted);
    assert_eq!(test.releases(), 0);
    // The admitted session still works after the duplicate was rejected.
    assert!(matches!(
        test.submit(batch(epoch)).expect("still admitted"),
        InputReceipt::Queued { .. }
    ));

    // A login success for another identity rejects without admitting; the
    // correct one afterwards still completes the exchange.
    let mut test = UnderTest::new();
    test.connect("mismatch", 6);
    test.feed(hello_frame());
    test.step().expect("hello step");
    test.feed(login_success_frame(player(60)));
    let report = test.step().expect("identity mismatch rejects");
    assert!(report.terminal().is_none());
    assert_eq!(
        test.session_phase(),
        SessionPhase::Handshaking,
        "not admitted"
    );
    test.feed(login_success_frame(player(6)));
    test.step().expect("login step");
    assert_eq!(test.session_phase(), SessionPhase::Admitted);
}

/// `login::disconnect_during_login_terminates_once`: a transport that reports
/// closed during the login exchange terminates the session exactly once — one
/// terminal publication, one session release beyond the peer's own — and a
/// later step cannot publish or release a second time.
#[test]
fn disconnect_during_login_terminates_once() {
    let mut test = UnderTest::new();
    test.connect("disconnect", 7);
    test.feed(hello_frame());
    test.step().expect("hello step");
    assert_eq!(test.session_phase(), SessionPhase::Handshaking);
    let before = test.releases();

    test.peer_closes_transport();
    let report = test.step().expect("the terminal step publishes once");
    assert_eq!(
        report.terminal(),
        Some(&CloseReason::RemoteDisconnect(control_text(""))),
        "the transport carried no disconnect text"
    );
    assert_eq!(
        test.releases(),
        before + 2,
        "the peer's close plus exactly one session release"
    );
    let (phase, _, terminal) = test.session_parts();
    assert_eq!(phase, SessionPhase::Closing);
    assert_eq!(
        terminal,
        Some(CloseReason::RemoteDisconnect(control_text(""))),
        "the terminal session record is published"
    );

    // No second publication and no second release.
    assert_eq!(test.step(), Err(ClientError::Disconnected));
    assert_eq!(test.releases(), before + 2);
    assert_eq!(
        test.session_terminal(),
        Some(CloseReason::RemoteDisconnect(control_text("")))
    );
}

/// `login::hello_and_login_deadlines_time_out_late_login_cannot_revive`: the
/// accepted 5 s hello and 10 s login policy (the landed S2 handshake and login
/// timeouts mirrored by the harness configuration) leaves the session pending
/// just before each deadline, times out at the deadline, and a late hello or
/// login cannot revive the timed-out session.
#[test]
fn hello_and_login_deadlines_time_out_late_login_cannot_revive() {
    // Hello deadline: 5 s from the pending epoch.
    let mut test = UnderTest::new();
    test.connect("hello-deadline", 1);
    test.advance(Duration::from_secs(5) - Duration::from_millis(1));
    let report = test.step().expect("just before the hello deadline");
    assert!(report.terminal().is_none(), "just before remains pending");
    assert_eq!(test.session_phase(), SessionPhase::Connecting);
    // The late hello is already queued when the deadline step runs: the
    // deadline is checked before any frame is processed, so a queued answer
    // cannot extend the phase.
    test.feed(hello_frame());
    test.advance(Duration::from_millis(2));
    let report = test.step().expect("the deadline step");
    assert_eq!(report.terminal(), Some(&CloseReason::Timeout));
    assert_eq!(test.releases(), 1, "one release at the timeout");
    assert_eq!(test.session_phase(), SessionPhase::Closing);
    // A late login cannot revive the session either.
    test.feed(login_success_frame(player(1)));
    assert_eq!(test.step(), Err(ClientError::Disconnected));
    assert_eq!(test.releases(), 1, "no second release");
    assert_eq!(test.session_phase(), SessionPhase::Closing);

    // Login deadline: 10 s from the accepted hello.
    let mut test = UnderTest::new();
    test.connect("login-deadline", 2);
    test.feed(hello_frame());
    test.step().expect("hello step");
    test.advance(Duration::from_secs(10) - Duration::from_millis(1));
    let report = test.step().expect("just before the login deadline");
    assert!(report.terminal().is_none(), "just before remains pending");
    assert_eq!(test.session_phase(), SessionPhase::Handshaking);
    // The late login success is queued before the deadline step for the same
    // reason: arriving late cannot admit the session.
    test.feed(login_success_frame(player(2)));
    test.advance(Duration::from_millis(2));
    let report = test.step().expect("the deadline step");
    assert_eq!(report.terminal(), Some(&CloseReason::Timeout));
    assert_eq!(test.releases(), 1);
    // A late login success cannot revive the timed-out session.
    test.feed(login_success_frame(player(2)));
    assert_eq!(test.step(), Err(ClientError::Disconnected));
    assert_eq!(test.session_phase(), SessionPhase::Closing);
    assert_eq!(test.releases(), 1);
}

/// `login::lifecycle_open_terminal`: one lifecycle Open at revision 0 under
/// the checked local core generation listing exactly the core-owned resource
/// keys, then one ordered terminal Close/Invalidate pair with real nonempty
/// lifecycle records, published before any frame assembler exists.
#[test]
fn lifecycle_open_terminal() {
    let mut test = UnderTest::new();
    let epoch = test.connect("lifecycle", 8);

    // The pending epoch opens exactly one lifecycle record at revision 0.
    let open = test.lifecycle_records();
    assert_eq!(open.len(), 1, "one Open");
    assert_eq!(
        open[0],
        (LifecycleTransition::Open, epoch.get(), core_resources(), 0),
        "Open at revision 0 under the checked core generation"
    );

    // A login rejection terminates with the server text preserved
    // byte-for-byte and publishes the ordered terminal pair.
    test.feed(hello_frame());
    test.step().expect("hello step");
    test.feed(login_reject_frame(
        mornlea_protocol::LOGIN_SERVER_FULL,
        "server full",
    ));
    let report = test.step().expect("terminal step");
    assert_eq!(
        report.terminal(),
        Some(&CloseReason::LoginRejected(control_text("server full"))),
        "login-reject text preserved byte for byte"
    );
    let terminal = test.lifecycle_records();
    assert_eq!(terminal.len(), 2, "one ordered terminal pair");
    assert_eq!(terminal[0].0, LifecycleTransition::Close, "Close first");
    assert_eq!(terminal[0].1, epoch.get());
    assert_eq!(terminal[0].3, 0, "the terminal pair is at revision 0");
    assert_eq!(
        terminal[1],
        (
            LifecycleTransition::Invalidate,
            epoch.get(),
            core_resources(),
            0
        ),
        "Invalidate lists the actual core-owned resource order"
    );
    assert_eq!(test.releases(), 1, "one transport release");
    assert_eq!(
        test.session_terminal(),
        Some(CloseReason::LoginRejected(control_text("server full")))
    );
}

/// The wrong-behavior artifact: the contract double's login path — the
/// behavior the real provider replaces — cannot process the hello/login
/// exchange at all, sends nothing, never transitions past Connecting, enforces
/// no deadline, and republishes its terminal on every step after a close.
#[test]
fn wrong_double_login_rejected() {
    // Missing transition: the double's step cannot even apply the server
    // hello, and no hello/login frame is ever sent.
    let mut test = UnderTest::double();
    test.connect("wrong-double", 9);
    test.feed(hello_frame());
    test.feed(login_success_frame(player(9)));
    let error = test
        .step()
        .expect_err("the double has no hello/login transition");
    assert_eq!(error, ClientError::InvalidInput);
    assert_eq!(
        test.sent(),
        Vec::<Vec<u8>>::new(),
        "the double sends nothing"
    );
    assert_eq!(test.session_phase(), SessionPhase::Connecting);

    // Wrong deadline: eleven seconds past the pending epoch is still pending.
    let mut test = UnderTest::double();
    test.connect("wrong-deadline", 9);
    test.advance(Duration::from_secs(11));
    let report = test.step().expect("an idle double step");
    assert!(report.terminal().is_none(), "no timeout exists to enforce");

    // Duplicate terminal: every step after the close republishes it.
    let mut test = UnderTest::double();
    test.connect("wrong-terminal", 9);
    test.double_close();
    let first = test.step().expect("first step after close");
    let second = test.step().expect("second step after close");
    assert!(first.terminal().is_some() && second.terminal().is_some());
}
