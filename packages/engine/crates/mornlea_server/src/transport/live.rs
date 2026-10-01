//! Bounded login ownership over borrowed authority and its existing load owner.
//!
//! The runtime retains ticks, shutdown and storage lifecycle. This driver owns
//! only uncommitted login aliases; validated bodies move directly into authority.

use super::common::{MAX_PENDING_LOGINS, TransportAuthority, TransportSessionPort};
use crate::contracts::{
    CloseReason, Deadline, LoadPoll, LoginPoll, LoginTicket, Operation, PlayerLoadPort,
    PublicationPort, Resource, ServerError, ServerPhase, SessionKey, SessionPhase, StorageFailure,
    SubmissionReceipt, TickPublication, TransportKind,
};
use crate::core::publication::{EnqueueOutcome, PreparedFrame, PreparedPublicationPort};
use crate::state::AuthorityState;
use mornlea_domain::PlayerId;
use mornlea_protocol::{
    AdmittedLogin, LOGIN_INTERNAL_ERROR, LOGIN_PLAYER_DATA_CORRUPT, LOGIN_STORE_UNAVAILABLE,
    LoginSuccess, PlayIntent, ServerPacket,
};
use std::collections::BTreeMap;

#[derive(Clone, Copy)]
enum State {
    Loading,
    Ready,
    Cancelled,
}
#[derive(Clone, Copy)]
struct Record {
    session: SessionKey,
    player: PlayerId,
    state: State,
}

/// Retains at most sixteen uncommitted aliases, including refused cancellation.
#[derive(Default)]
pub struct LoginDriver {
    records: BTreeMap<LoginTicket, Record>,
    stopped: bool,
    last_cancellation_error: Option<ServerError>,
}
impl LoginDriver {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn pending(&self) -> usize {
        self.records.len()
    }
    pub fn stop_new(&mut self) {
        self.stopped = true;
    }
    /// Borrows all three owners for one bounded transport operation.
    pub fn bind<'a>(
        &'a mut self,
        authority: &'a mut AuthorityState,
        loads: &'a mut dyn PlayerLoadPort,
    ) -> LiveEndpoint<'a> {
        LiveEndpoint {
            driver: self,
            authority,
            loads,
        }
    }
    /// Stops admission and visits each original alias once in ticket order.
    /// Independent successes remain removed even when an earlier cancel refuses.
    pub fn cancel_pending(
        &mut self,
        authority: &mut AuthorityState,
        loads: &mut dyn PlayerLoadPort,
    ) -> Result<usize, ServerError> {
        self.stop_new();
        let tickets: Vec<_> = self.records.keys().copied().collect();
        let mut removed = 0;
        let mut first_error = None;
        for ticket in tickets {
            match self.cancel(ticket, authority, loads) {
                Ok(true) => removed += 1,
                Ok(false) => {}
                Err(error) => {
                    first_error.get_or_insert(error);
                }
            }
        }
        match first_error {
            Some(error) => Err(error),
            None => Ok(removed),
        }
    }
    fn cancel(
        &mut self,
        ticket: LoginTicket,
        authority: &mut AuthorityState,
        loads: &mut dyn PlayerLoadPort,
    ) -> Result<bool, ServerError> {
        let Some(record) = self.records.get_mut(&ticket) else {
            return Ok(false);
        };
        // Retirement releases player capacity once; a retained alias still fences
        // a late load result until the same owner accepts cancellation.
        if authority
            .session(record.session)
            .is_some_and(|facts| facts.phase == SessionPhase::Prepared)
        {
            let _ = authority.close_session(record.session, CloseReason::PeerGone);
        }
        record.state = State::Cancelled;
        match loads.cancel(ticket) {
            Ok(()) => {
                self.records.remove(&ticket);
                Ok(true)
            }
            Err(error) => {
                self.last_cancellation_error = Some(error);
                Err(error)
            }
        }
    }
}

/// Session/login/publication access without borrowing runtime lifecycle ownership.
pub struct LiveEndpoint<'a> {
    driver: &'a mut LoginDriver,
    authority: &'a mut AuthorityState,
    loads: &'a mut dyn PlayerLoadPort,
}
impl TransportSessionPort for LiveEndpoint<'_> {
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
// Borrowed delivery forwards owners directly; the endpoint retains no queue.
impl PreparedPublicationPort for LiveEndpoint<'_> {
    fn enqueue_prepared(
        &mut self,
        session: SessionKey,
        frame: PreparedFrame,
    ) -> Result<EnqueueOutcome, ServerError> {
        self.authority.enqueue_prepared(session, frame)
    }
    fn take_prepared_outbox(
        &mut self,
        session: SessionKey,
        max_frames: usize,
        max_bytes: usize,
    ) -> Result<Vec<PreparedFrame>, ServerError> {
        self.authority
            .take_prepared_outbox(session, max_frames, max_bytes)
    }
}
impl PublicationPort for LiveEndpoint<'_> {
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
        self.authority.close_outbox(session, reason)
    }
}
impl TransportAuthority for LiveEndpoint<'_> {
    fn begin_login(
        &mut self,
        login: AdmittedLogin,
        kind: TransportKind,
        deadline: Deadline,
    ) -> Result<LoginTicket, ServerError> {
        if self.driver.stopped {
            return Err(ServerError::InvalidState {
                phase: ServerPhase::Closing,
            });
        }
        if self.driver.pending() == MAX_PENDING_LOGINS {
            return Err(ServerError::Capacity {
                resource: Resource::PendingLogins,
                limit: MAX_PENDING_LOGINS,
                observed: MAX_PENDING_LOGINS + 1,
            });
        }
        let player = login.player_id();
        let session = self.authority.prepare(login, kind)?;
        let ticket = match self.loads.start(player, deadline) {
            Ok(ticket) => ticket,
            Err(error) => {
                let _ = self.authority.close_session(session, CloseReason::PeerGone);
                return Err(error);
            }
        };
        if self.driver.records.contains_key(&ticket) {
            // A repeated identity cannot overwrite or cancel the older owner.
            let _ = self.authority.close_session(session, CloseReason::PeerGone);
            return Err(ServerError::Internal {
                invariant: "login load ticket identity",
            });
        }
        self.driver.records.insert(
            ticket,
            Record {
                session,
                player,
                state: State::Loading,
            },
        );
        Ok(ticket)
    }
    fn poll_login(&mut self, ticket: LoginTicket) -> LoginPoll {
        let Some(record) = self.driver.records.get(&ticket).copied() else {
            return LoginPoll::Pending;
        };
        match record.state {
            State::Cancelled => {
                let _ = self.driver.cancel(ticket, self.authority, self.loads);
                return LoginPoll::Pending;
            }
            State::Ready => return ready(record, self.world_seed()),
            State::Loading => {}
        }
        let error = match self.loads.poll(ticket) {
            LoadPoll::Pending => return LoginPoll::Pending,
            LoadPoll::Loaded(stored) => match self.authority.install(record.session, stored) {
                Ok(()) => {
                    self.driver
                        .records
                        .get_mut(&ticket)
                        .expect("owned login alias")
                        .state = State::Ready;
                    return ready(record, self.world_seed());
                }
                Err(error) => error,
            },
            LoadPoll::Failed(error) => error,
        };
        self.driver.records.remove(&ticket);
        let _ = self
            .authority
            .close_session(record.session, CloseReason::PeerGone);
        LoginPoll::Failed {
            error,
            reject: reject(error),
        }
    }
    fn commit_login(&mut self, ticket: LoginTicket) -> Result<SessionKey, ServerError> {
        let record = self
            .driver
            .records
            .get(&ticket)
            .filter(|record| matches!(record.state, State::Ready))
            .ok_or(ServerError::InvalidInput {
                field: "login_ticket",
            })?;
        let session = record.session;
        self.authority.activate(session)?;
        self.driver.records.remove(&ticket);
        Ok(session)
    }
    fn cancel_login(&mut self, ticket: LoginTicket) {
        let _ = self.driver.cancel(ticket, self.authority, self.loads);
    }
    fn world_seed(&self) -> i64 {
        self.authority.world_seed()
    }
}
fn ready(record: Record, seed: i64) -> LoginPoll {
    LoginPoll::Ready {
        session: record.session,
        success: ServerPacket::LoginSuccess(LoginSuccess::new(record.player, seed as u64)),
    }
}
fn reject(error: ServerError) -> u8 {
    match error {
        ServerError::Io {
            operation: Operation::Load,
            kind: std::io::ErrorKind::InvalidData,
        }
        | ServerError::InvalidInput { .. }
        | ServerError::Storage {
            family: "player",
            kind: StorageFailure::Corrupt | StorageFailure::FutureVersion,
        } => LOGIN_PLAYER_DATA_CORRUPT,
        ServerError::Timeout {
            operation: Operation::Load,
        }
        | ServerError::Io { .. } => LOGIN_STORE_UNAVAILABLE,
        _ => LOGIN_INTERNAL_ERROR,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contracts::ServerLimits;
    use mornlea_protocol::{LoginStart, admit_login};
    use std::collections::BTreeSet;
    use std::time::{Duration, Instant};

    fn player(tag: u8) -> PlayerId {
        let mut bytes = [0; 16];
        bytes[0] = tag;
        bytes[6] = 0x40;
        bytes[8] = 0x80;
        PlayerId::try_from_bytes(bytes).unwrap()
    }
    fn login(tag: u8) -> AdmittedLogin {
        admit_login(
            LoginStart::decode_inbound(
                &LoginStart::new(player(tag), "Ada", 8)
                    .unwrap()
                    .encode()
                    .unwrap(),
            )
            .unwrap(),
        )
        .unwrap()
    }
    fn authority() -> AuthorityState {
        AuthorityState::try_new(
            ServerLimits::try_new(8, 4096, 512, 64, 64, 1_048_576).unwrap(),
            -57,
        )
        .unwrap()
    }
    fn deadline() -> Deadline {
        Deadline::after(Instant::now(), Duration::from_secs(5)).unwrap()
    }
    fn cancellation_error() -> ServerError {
        ServerError::Io {
            operation: Operation::Load,
            kind: std::io::ErrorKind::PermissionDenied,
        }
    }
    #[derive(Default)]
    struct Loads {
        next: u64,
        repeat: Option<LoginTicket>,
        start_error: Option<ServerError>,
        outcome: Option<LoadPoll>,
        cancel_errors: BTreeMap<LoginTicket, ServerError>,
        refuse_all: bool,
        live: BTreeSet<LoginTicket>,
        starts: Vec<(PlayerId, Deadline)>,
        polls: usize,
        cancels: Vec<LoginTicket>,
    }
    impl PlayerLoadPort for Loads {
        fn start(
            &mut self,
            player: PlayerId,
            deadline: Deadline,
        ) -> Result<LoginTicket, ServerError> {
            self.starts.push((player, deadline));
            if let Some(error) = self.start_error {
                return Err(error);
            }
            self.next += 1;
            let ticket = self
                .repeat
                .unwrap_or_else(|| LoginTicket::try_from_raw(self.next).unwrap());
            self.live.insert(ticket);
            Ok(ticket)
        }
        fn poll(&mut self, _: LoginTicket) -> LoadPoll {
            self.polls += 1;
            self.outcome.take().unwrap_or(LoadPoll::Pending)
        }
        fn cancel(&mut self, ticket: LoginTicket) -> Result<(), ServerError> {
            self.cancels.push(ticket);
            if self.refuse_all {
                return Err(cancellation_error());
            }
            if let Some(error) = self.cancel_errors.get(&ticket) {
                return Err(*error);
            }
            self.live.remove(&ticket);
            Ok(())
        }
    }
    fn begin(d: &mut LoginDriver, a: &mut AuthorityState, l: &mut Loads, tag: u8) -> LoginTicket {
        d.bind(a, l)
            .begin_login(login(tag), TransportKind::Memory, deadline())
            .unwrap()
    }
    fn record_session(d: &LoginDriver, t: LoginTicket) -> SessionKey {
        d.records[&t].session
    }

    #[test]
    fn start_refusal_retires_reservation_and_returns_original_error() {
        let mut a = authority();
        let mut d = LoginDriver::new();
        let mut l = Loads {
            start_error: Some(cancellation_error()),
            ..Default::default()
        };
        for _ in 0..20 {
            assert_eq!(
                d.bind(&mut a, &mut l)
                    .begin_login(login(1), TransportKind::Memory, deadline()),
                Err(cancellation_error())
            );
        }
        assert_eq!(d.pending(), 0);
        l.start_error = None;
        assert_eq!(begin(&mut d, &mut a, &mut l, 1).get(), 1);
    }
    #[test]
    fn prepare_precedence_and_actual_player_ceiling_are_preserved() {
        let mut a = authority();
        let mut d = LoginDriver::new();
        let mut l = Loads::default();
        begin(&mut d, &mut a, &mut l, 1);
        assert!(
            d.bind(&mut a, &mut l)
                .begin_login(login(1), TransportKind::Memory, deadline())
                .is_err()
        );
        assert_eq!(l.starts.len(), 1);
        for tag in 2..=8 {
            begin(&mut d, &mut a, &mut l, tag);
        }
        assert_eq!(
            d.bind(&mut a, &mut l)
                .begin_login(login(9), TransportKind::Memory, deadline()),
            Err(ServerError::Capacity {
                resource: Resource::Players,
                limit: 8,
                observed: 9
            })
        );
        assert_eq!(l.starts.len(), 8);
        a.begin_close();
        assert_eq!(
            d.bind(&mut a, &mut l)
                .begin_login(login(9), TransportKind::Memory, deadline()),
            Err(ServerError::InvalidState {
                phase: ServerPhase::Closing
            })
        );
    }
    #[test]
    fn duplicate_ticket_preserves_older_alias_and_new_capacity_is_reusable() {
        let mut a = authority();
        let mut d = LoginDriver::new();
        let mut l = Loads::default();
        let ticket = begin(&mut d, &mut a, &mut l, 1);
        let original = record_session(&d, ticket);
        l.repeat = Some(ticket);
        assert_eq!(
            d.bind(&mut a, &mut l)
                .begin_login(login(2), TransportKind::Tcp, deadline()),
            Err(ServerError::Internal {
                invariant: "login load ticket identity"
            })
        );
        assert_eq!(d.pending(), 1);
        assert_eq!(record_session(&d, ticket), original);
        assert!(l.cancels.is_empty());
        l.repeat = None;
        begin(&mut d, &mut a, &mut l, 2);
    }
    #[test]
    fn ready_is_owned_repeatable_and_commits_once_without_history() {
        let mut a = authority();
        let mut d = LoginDriver::new();
        let mut l = Loads {
            outcome: Some(LoadPoll::Loaded(None)),
            ..Default::default()
        };
        let t = begin(&mut d, &mut a, &mut l, 1);
        let session = record_session(&d, t);
        assert_eq!(
            d.bind(&mut a, &mut l).commit_login(t),
            Err(ServerError::InvalidInput {
                field: "login_ticket"
            })
        );
        let expected = LoginPoll::Ready {
            session,
            success: ServerPacket::LoginSuccess(LoginSuccess::new(player(1), -57i64 as u64)),
        };
        assert_eq!(d.bind(&mut a, &mut l).poll_login(t), expected);
        assert_eq!(d.bind(&mut a, &mut l).poll_login(t), expected);
        assert_eq!(l.polls, 1);
        assert_eq!(a.session(session).unwrap().phase, SessionPhase::Prepared);
        assert!(
            d.bind(&mut a, &mut l)
                .submit(
                    session,
                    PlayIntent::try_from(mornlea_protocol::ClientPacket::CloseContainer(
                        mornlea_protocol::CloseContainer::new(1)
                    ))
                    .unwrap()
                )
                .is_err()
        );
        assert_eq!(d.bind(&mut a, &mut l).commit_login(t), Ok(session));
        assert_eq!(d.pending(), 0);
        assert_eq!(a.session(session).unwrap().phase, SessionPhase::Active);
        assert_eq!(
            d.bind(&mut a, &mut l).commit_login(t),
            Err(ServerError::InvalidInput {
                field: "login_ticket"
            })
        );
        assert_eq!(d.bind(&mut a, &mut l).poll_login(t), LoginPoll::Pending);
        d.bind(&mut a, &mut l).cancel_login(t);
        assert!(l.cancels.is_empty());
        assert!(
            d.bind(&mut a, &mut l)
                .submit(
                    session,
                    PlayIntent::try_from(mornlea_protocol::ClientPacket::CloseContainer(
                        mornlea_protocol::CloseContainer::new(1)
                    ))
                    .unwrap()
                )
                .is_ok()
        );
    }
    #[test]
    fn activation_refusal_keeps_ready_alias_for_cancellation() {
        let mut a = authority();
        let mut d = LoginDriver::new();
        let mut l = Loads {
            outcome: Some(LoadPoll::Loaded(None)),
            ..Default::default()
        };
        let t = begin(&mut d, &mut a, &mut l, 1);
        let s = record_session(&d, t);
        assert!(matches!(
            d.bind(&mut a, &mut l).poll_login(t),
            LoginPoll::Ready { .. }
        ));
        a.close_session(s, CloseReason::PeerGone).unwrap();
        assert!(d.bind(&mut a, &mut l).commit_login(t).is_err());
        assert_eq!(d.pending(), 1);
        d.bind(&mut a, &mut l).cancel_login(t);
        assert_eq!(d.pending(), 0);
    }
    #[test]
    fn cancelled_alias_suppresses_late_ready_and_retries_same_owner() {
        let mut a = authority();
        let mut d = LoginDriver::new();
        let mut l = Loads {
            refuse_all: true,
            ..Default::default()
        };
        let t = begin(&mut d, &mut a, &mut l, 1);
        let s = record_session(&d, t);
        d.bind(&mut a, &mut l).cancel_login(t);
        assert_eq!(a.session(s).unwrap().phase, SessionPhase::Retired);
        assert_eq!(d.pending(), 1);
        assert_eq!(d.last_cancellation_error, Some(cancellation_error()));
        l.outcome = Some(LoadPoll::Loaded(None));
        assert_eq!(d.bind(&mut a, &mut l).poll_login(t), LoginPoll::Pending);
        assert_eq!(l.polls, 0);
        assert_eq!(
            d.bind(&mut a, &mut l).commit_login(t),
            Err(ServerError::InvalidInput {
                field: "login_ticket"
            })
        );
        l.refuse_all = false;
        assert_eq!(d.bind(&mut a, &mut l).poll_login(t), LoginPoll::Pending);
        assert_eq!(d.pending(), 0);
        d.bind(&mut a, &mut l).cancel_login(t);
        assert_eq!(l.cancels, vec![t, t, t]);
        assert_eq!(a.session(s).unwrap().phase, SessionPhase::Retired);
    }
    #[test]
    fn sixteen_cancelled_aliases_charge_limit_before_prepare_or_start() {
        let mut a = authority();
        let mut d = LoginDriver::new();
        let mut l = Loads {
            refuse_all: true,
            ..Default::default()
        };
        for count in 1..=16 {
            let t = begin(&mut d, &mut a, &mut l, 1);
            d.bind(&mut a, &mut l).cancel_login(t);
            assert_eq!(d.pending(), count);
        }
        assert_eq!(
            d.bind(&mut a, &mut l)
                .begin_login(login(1), TransportKind::Memory, deadline()),
            Err(ServerError::Capacity {
                resource: Resource::PendingLogins,
                limit: 16,
                observed: 17
            })
        );
        assert_eq!(l.starts.len(), 16);
        l.refuse_all = false;
        assert_eq!(d.cancel_pending(&mut a, &mut l), Ok(16));
        assert_eq!(d.pending(), 0);
        assert_eq!(
            l.cancels[16..],
            (1..=16)
                .map(|id| LoginTicket::try_from_raw(id).unwrap())
                .collect::<Vec<_>>()
        );
    }
    #[test]
    fn cancel_pending_visits_whole_batch_in_order_and_retains_first_exact_error() {
        let mut a = authority();
        let mut d = LoginDriver::new();
        let mut l = Loads::default();
        let tickets = (1..=4)
            .map(|tag| begin(&mut d, &mut a, &mut l, tag))
            .collect::<Vec<_>>();
        let first = cancellation_error();
        let second = ServerError::Internal {
            invariant: "cancel fixture",
        };
        l.cancel_errors.insert(tickets[0], first);
        l.cancel_errors.insert(tickets[2], second);
        assert_eq!(d.cancel_pending(&mut a, &mut l), Err(first));
        assert_eq!(l.cancels, tickets);
        assert_eq!(d.pending(), 2);
        assert_eq!(d.last_cancellation_error, Some(second));
        assert_eq!(
            d.bind(&mut a, &mut l)
                .begin_login(login(5), TransportKind::Memory, deadline()),
            Err(ServerError::InvalidState {
                phase: ServerPhase::Closing
            })
        );
        assert_eq!(l.starts.len(), 4);
        l.cancel_errors.clear();
        assert_eq!(d.cancel_pending(&mut a, &mut l), Ok(2));
        assert_eq!(d.cancel_pending(&mut a, &mut l), Ok(0));
    }
    #[test]
    fn typed_load_failures_preserve_original_error_and_never_install_absence() {
        let errors = [
            (
                ServerError::Storage {
                    family: "player",
                    kind: StorageFailure::Corrupt,
                },
                LOGIN_PLAYER_DATA_CORRUPT,
            ),
            (
                ServerError::Storage {
                    family: "player",
                    kind: StorageFailure::FutureVersion,
                },
                LOGIN_PLAYER_DATA_CORRUPT,
            ),
            (
                ServerError::Storage {
                    family: "chunk",
                    kind: StorageFailure::Corrupt,
                },
                LOGIN_INTERNAL_ERROR,
            ),
            (
                ServerError::Io {
                    operation: Operation::Load,
                    kind: std::io::ErrorKind::InvalidData,
                },
                LOGIN_PLAYER_DATA_CORRUPT,
            ),
            (
                ServerError::InvalidInput {
                    field: "loaded_player",
                },
                LOGIN_PLAYER_DATA_CORRUPT,
            ),
            (
                ServerError::Timeout {
                    operation: Operation::Load,
                },
                LOGIN_STORE_UNAVAILABLE,
            ),
            (cancellation_error(), LOGIN_STORE_UNAVAILABLE),
            (
                ServerError::Internal {
                    invariant: "store load panic",
                },
                LOGIN_INTERNAL_ERROR,
            ),
            (
                ServerError::InvalidState {
                    phase: ServerPhase::Closing,
                },
                LOGIN_INTERNAL_ERROR,
            ),
            (
                ServerError::Capacity {
                    resource: Resource::Players,
                    limit: 8,
                    observed: 9,
                },
                LOGIN_INTERNAL_ERROR,
            ),
            (
                ServerError::Storage {
                    family: "player",
                    kind: StorageFailure::OutputTooSmall,
                },
                LOGIN_INTERNAL_ERROR,
            ),
        ];
        for (error, reject) in errors {
            let mut a = authority();
            let mut d = LoginDriver::new();
            let mut l = Loads {
                outcome: Some(LoadPoll::Failed(error)),
                ..Default::default()
            };
            let t = begin(&mut d, &mut a, &mut l, 1);
            let s = record_session(&d, t);
            assert_eq!(
                d.bind(&mut a, &mut l).poll_login(t),
                LoginPoll::Failed { error, reject }
            );
            assert_eq!(d.pending(), 0);
            assert_eq!(a.session(s).unwrap().phase, SessionPhase::Retired);
            a.advance_tick(crate::contracts::TickBudget::full())
                .unwrap();
            assert!(a.residents().actors.is_empty());
            assert_eq!(d.bind(&mut a, &mut l).poll_login(t), LoginPoll::Pending);
        }
    }
    #[test]
    fn install_failure_retires_and_deadline_is_forwarded_without_clock_policy() {
        let mut a = authority();
        let mut d = LoginDriver::new();
        let mut l = Loads::default();
        // The common core, rather than this borrowed provider, observes its clock.
        let past = Deadline::after(
            Instant::now() - Duration::from_secs(20),
            Duration::from_secs(1),
        )
        .unwrap();
        let t = d
            .bind(&mut a, &mut l)
            .begin_login(login(1), TransportKind::Memory, past)
            .unwrap();
        assert_eq!(l.starts, vec![(player(1), past)]);
        let s = record_session(&d, t);
        a.close_session(s, CloseReason::PeerGone).unwrap();
        l.outcome = Some(LoadPoll::Loaded(None));
        assert_eq!(
            d.bind(&mut a, &mut l).poll_login(t),
            LoginPoll::Failed {
                error: ServerError::StaleSession { session: s },
                reject: LOGIN_INTERNAL_ERROR
            }
        );
        assert_eq!(d.pending(), 0);
        let unknown = LoginTicket::try_from_raw(99).unwrap();
        assert_eq!(
            d.bind(&mut a, &mut l).poll_login(unknown),
            LoginPoll::Pending
        );
        d.bind(&mut a, &mut l).cancel_login(unknown);
        assert!(l.cancels.is_empty());
        d.stop_new();
        assert_eq!(
            d.bind(&mut a, &mut l)
                .begin_login(login(2), TransportKind::Tcp, deadline()),
            Err(ServerError::InvalidState {
                phase: ServerPhase::Closing
            })
        );
    }
}
