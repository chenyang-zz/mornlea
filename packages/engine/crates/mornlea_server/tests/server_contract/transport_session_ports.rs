//! Executing consumer double for the transport's session-only ownership seam.

use mornlea_domain::PlayerId;
use mornlea_protocol::{AdmittedLogin, LoginStart, LoginSuccess, PlayIntent, ServerPacket};
use mornlea_server::contracts::{
    CloseReason, Deadline, LoginPoll, LoginTicket, PublicationPort, ServerError, ServerLimits,
    SessionKey, SubmissionReceipt, TickPublication, TransportKind,
};
use mornlea_server::state::AuthorityState;
use mornlea_server::transport::common::{TransportAuthority, TransportSessionPort};

// This declaration intentionally has no full endpoint or shutdown implementation.
// A deterministic immediate installer qualifies only the narrow consumer seam.
struct NarrowDouble {
    authority: AuthorityState,
    pending: Option<(LoginTicket, SessionKey)>,
}

impl NarrowDouble {
    fn new() -> Self {
        Self {
            authority: AuthorityState::try_new(
                ServerLimits::try_new(8, 4096, 512, 64, 64, 1_048_576).unwrap(),
                42,
            )
            .unwrap(),
            pending: None,
        }
    }
}

impl TransportSessionPort for NarrowDouble {
    fn submit(
        &mut self,
        session: SessionKey,
        intent: PlayIntent,
    ) -> Result<SubmissionReceipt, ServerError> {
        self.authority.submit(session, intent)
    }

    fn close_session(
        &mut self,
        session: SessionKey,
        reason: CloseReason,
    ) -> Result<(), ServerError> {
        self.authority.close_session(session, reason)
    }
}

impl PublicationPort for NarrowDouble {
    fn publish(&mut self, publication: TickPublication) -> Result<(), ServerError> {
        self.authority.publish(publication)
    }

    fn take_outbox(
        &mut self,
        session: SessionKey,
        max_frames: usize,
        max_bytes: usize,
    ) -> Result<Vec<Vec<u8>>, ServerError> {
        self.authority.take_outbox(session, max_frames, max_bytes)
    }

    fn close_outbox(&mut self, session: SessionKey, reason: CloseReason) {
        self.authority.close_outbox(session, reason);
    }
}

impl TransportAuthority for NarrowDouble {
    fn begin_login(
        &mut self,
        login: AdmittedLogin,
        kind: TransportKind,
        _deadline: Deadline,
    ) -> Result<LoginTicket, ServerError> {
        let session = self.authority.prepare(login, kind)?;
        self.authority.install(session, None)?;
        let ticket = LoginTicket::try_from_raw(session.get())?;
        self.pending = Some((ticket, session));
        Ok(ticket)
    }

    fn poll_login(&mut self, ticket: LoginTicket) -> LoginPoll {
        match self.pending.filter(|(owned, _)| *owned == ticket) {
            Some((_, session)) => LoginPoll::Ready {
                session,
                success: ServerPacket::LoginSuccess(LoginSuccess::new(
                    self.authority.session(session).unwrap().player_id,
                    42,
                )),
            },
            None => LoginPoll::Pending,
        }
    }

    fn commit_login(&mut self, ticket: LoginTicket) -> Result<SessionKey, ServerError> {
        let (_, session) = self.pending.filter(|(owned, _)| *owned == ticket).ok_or(
            ServerError::InvalidInput {
                field: "login_ticket",
            },
        )?;
        self.authority.activate(session)?;
        Ok(session)
    }

    fn cancel_login(&mut self, ticket: LoginTicket) {
        if let Some((_, session)) = self.pending.filter(|(owned, _)| *owned == ticket) {
            let _ = self.authority.retire(session, CloseReason::PeerGone);
            self.pending = None;
        }
    }

    fn world_seed(&self) -> i64 {
        self.authority.world_seed()
    }
}

fn login() -> AdmittedLogin {
    let player =
        PlayerId::try_from_bytes([1, 0, 0, 0, 0, 0, 64, 0, 128, 0, 0, 0, 0, 0, 0, 1]).unwrap();
    let start = LoginStart::new(player, "Ada", 8).unwrap();
    let inbound = LoginStart::decode_inbound(&start.encode().unwrap()).unwrap();
    mornlea_protocol::admit_login(inbound).unwrap()
}

#[test]
fn narrow_only_authority_accepts_and_closes_a_real_session() {
    let mut owner = NarrowDouble::new();
    let endpoint: &mut dyn TransportAuthority = &mut owner;
    let deadline =
        Deadline::after(std::time::Instant::now(), std::time::Duration::from_secs(5)).unwrap();
    let ticket = endpoint
        .begin_login(login(), TransportKind::Memory, deadline)
        .unwrap();
    assert!(matches!(
        endpoint.poll_login(ticket),
        LoginPoll::Ready { .. }
    ));
    let session = endpoint.commit_login(ticket).unwrap();
    assert_eq!(
        endpoint
            .submit(session, PlayIntent::KeepAliveReply { token: 7 })
            .unwrap(),
        SubmissionReceipt::ControlAccepted
    );
    endpoint
        .close_session(session, CloseReason::PeerGone)
        .unwrap();
    assert_eq!(
        endpoint.submit(session, PlayIntent::KeepAliveReply { token: 7 }),
        Err(ServerError::StaleSession { session })
    );
}

#[test]
fn narrow_session_port_retains_exact_unknown_session_refusal() {
    let mut owner = NarrowDouble::new();
    let unknown = NarrowDouble::new()
        .authority
        .admit(login(), TransportKind::Memory)
        .unwrap();
    let endpoint: &mut dyn TransportSessionPort = &mut owner;
    assert_eq!(
        endpoint.close_session(unknown, CloseReason::Shutdown),
        Err(ServerError::StaleSession { session: unknown })
    );
}
