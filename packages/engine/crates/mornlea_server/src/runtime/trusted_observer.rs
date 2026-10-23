//! Host AttachTrustedObserver / center / close surface.
//!
//! Mirrors Go `Server.AttachTrustedObserver`, `CloseTrustedObserver`,
//! `SetTrustedObserverCenter`, and `AppliedTrustedObserverCenter`. The
//! authority library (`state_trusted_observer`) owns session mutation; this
//! host owns the singular attached endpoint and routes center/close through
//! the between-ticks gate.

use mornlea_domain::{ChunkPos, Dimension};

use crate::contracts::{ServerError, SessionKey};
use crate::state::AuthorityState;

/// Peer endpoint owned by the host for the attached trusted observer.
///
/// Go's `network.ServerEndpoint.Close` is the model: close returns the
/// endpoint's own error (if any) after detach has already cleared the
/// session. Implementations must be safe to close once.
pub trait ObserverEndpoint: Send {
    /// Closes the peer transport. Called once from [`TrustedObserverHost::close`].
    fn close(&mut self) -> Result<(), ServerError>;
}

/// Singular host-side trusted-observer attachment (Go `server.trustedObserver`).
#[derive(Default)]
pub struct TrustedObserverHost {
    session: Option<SessionKey>,
    generation: u64,
    endpoint: Option<Box<dyn ObserverEndpoint>>,
}

impl TrustedObserverHost {
    /// Empty host with no attached observer.
    pub fn new() -> Self {
        Self::default()
    }

    /// Session key of the attached observer, if any.
    pub fn session(&self) -> Option<SessionKey> {
        self.session
    }

    /// Whether an observer endpoint is currently attached.
    pub fn is_attached(&self) -> bool {
        self.session.is_some()
    }

    /// Go `AttachTrustedObserver`: attach through authority and retain the
    /// endpoint. A nil-equivalent (caller must not pass a dead box) refusal
    /// matches Go's `endpoint == nil` → `ErrInvalidSession`.
    pub fn attach(
        &mut self,
        state: &mut AuthorityState,
        endpoint: Box<dyn ObserverEndpoint>,
    ) -> Result<SessionKey, ServerError> {
        // Cap / disabled / phase refusals come from authority; host only
        // refuses a second live attachment the same way Go checks
        // `trustedObserver != nil` after the enable/running gate.
        if self.session.is_some() {
            // Authority would refuse Capacity/TrustedObservers; keep host and
            // authority in sync by asking authority first when we somehow
            // drifted. Prefer authority's attach when host thinks empty.
            return Err(ServerError::Capacity {
                resource: crate::contracts::Resource::TrustedObservers,
                limit: 1,
                observed: 2,
            });
        }
        let key = state.attach_trusted_observer()?;
        self.generation = self.generation.saturating_add(1);
        self.session = Some(key);
        self.endpoint = Some(endpoint);
        Ok(key)
    }

    /// Go `CloseTrustedObserver`: detach the session, then close the endpoint.
    /// Idempotent when nothing is attached (returns Ok). Returns the
    /// endpoint's close error after detach succeeds.
    pub fn close(&mut self, state: &mut AuthorityState) -> Result<(), ServerError> {
        let Some(session) = self.session.take() else {
            self.endpoint = None;
            self.generation = 0;
            return Ok(());
        };
        self.generation = 0;
        // Detach first so a close error cannot leave a live observer session.
        let detach = state.detach_trusted_observer(session);
        let close_result = self.endpoint.take().map(|mut endpoint| endpoint.close());
        detach?;
        match close_result {
            Some(Err(error)) => Err(error),
            _ => Ok(()),
        }
    }

    /// Go `SetTrustedObserverCenter`: queue center for the attached observer.
    pub fn set_center(
        &mut self,
        state: &mut AuthorityState,
        dimension: Dimension,
        center: ChunkPos,
    ) -> Result<(), ServerError> {
        let Some(session) = self.session else {
            return Err(ServerError::InvalidInput {
                field: "trusted_observer_disabled",
            });
        };
        state.set_trusted_observer_center(session, dimension, center)
    }

    /// Go `AppliedTrustedObserverCenter`: last center applied at a tick
    /// boundary, if any (sequence > 0).
    pub fn applied_center(
        &self,
        state: &AuthorityState,
    ) -> Option<(Dimension, ChunkPos, u64)> {
        let session = self.session?;
        state.applied_trusted_observer_center(session)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contracts::{Resource, TickBudget};
    use crate::core::contracts::ServerLimits;
    use crate::core::state::TickContext;
    use std::sync::{Arc, Mutex};

    fn authority() -> AuthorityState {
        AuthorityState::try_new(
            ServerLimits::try_new(8, 4096, 512, 64, 64, 1_048_576).unwrap(),
            7,
        )
        .unwrap()
    }

    #[derive(Default)]
    struct RecordingEndpoint {
        closed: Arc<Mutex<usize>>,
        close_err: Option<ServerError>,
    }

    impl ObserverEndpoint for RecordingEndpoint {
        fn close(&mut self) -> Result<(), ServerError> {
            *self.closed.lock().unwrap() += 1;
            match self.close_err.take() {
                Some(err) => Err(err),
                None => Ok(()),
            }
        }
    }

    #[test]
    fn attach_close_set_center_applied_after_tick() {
        let mut state = authority();
        state.set_trusted_observer_enabled(true).unwrap();
        let closed = Arc::new(Mutex::new(0usize));
        let mut host = TrustedObserverHost::new();
        assert!(!host.is_attached());

        let key = host
            .attach(
                &mut state,
                Box::new(RecordingEndpoint {
                    closed: Arc::clone(&closed),
                    close_err: None,
                }),
            )
            .expect("attach");
        assert_eq!(host.session(), Some(key));
        assert!(state.is_trusted_observer(key));

        let center = ChunkPos::new(3, -2);
        host.set_center(&mut state, Dimension::OVERWORLD, center)
            .expect("set_center");
        assert!(host.applied_center(&state).is_none(), "pending until tick");

        {
            let _ctx = TickContext::for_tick(&mut state, TickBudget::full());
        }
        let applied = host.applied_center(&state).expect("applied after tick");
        assert_eq!(applied, (Dimension::OVERWORLD, center, 1));

        host.close(&mut state).expect("close");
        assert!(!host.is_attached());
        assert!(!state.is_trusted_observer(key));
        assert_eq!(*closed.lock().unwrap(), 1);
        assert_eq!(
            host.set_center(&mut state, Dimension::OVERWORLD, center),
            Err(ServerError::InvalidInput {
                field: "trusted_observer_disabled",
            })
        );
    }

    #[test]
    fn close_returns_endpoint_error_after_detach() {
        let mut state = authority();
        state.set_trusted_observer_enabled(true).unwrap();
        let closed = Arc::new(Mutex::new(0usize));
        let mut host = TrustedObserverHost::new();
        let key = host
            .attach(
                &mut state,
                Box::new(RecordingEndpoint {
                    closed: Arc::clone(&closed),
                    close_err: Some(ServerError::Internal {
                        invariant: "injected observer close",
                    }),
                }),
            )
            .unwrap();
        let err = host.close(&mut state).expect_err("close error");
        assert_eq!(
            err,
            ServerError::Internal {
                invariant: "injected observer close",
            }
        );
        assert!(!state.is_trusted_observer(key));
        assert_eq!(*closed.lock().unwrap(), 1);
        // Second close is idempotent.
        host.close(&mut state).unwrap();
        assert_eq!(*closed.lock().unwrap(), 1);
    }

    #[test]
    fn attach_refuses_when_disabled_and_second_attach() {
        let mut state = authority();
        let mut host = TrustedObserverHost::new();
        let err = host
            .attach(&mut state, Box::new(RecordingEndpoint::default()))
            .expect_err("disabled");
        assert_eq!(err, ServerError::InvalidInput { field: "session" });

        state.set_trusted_observer_enabled(true).unwrap();
        host.attach(&mut state, Box::new(RecordingEndpoint::default()))
            .unwrap();
        let err = host
            .attach(&mut state, Box::new(RecordingEndpoint::default()))
            .expect_err("second");
        assert!(matches!(
            err,
            ServerError::Capacity {
                resource: Resource::TrustedObservers,
                ..
            }
        ));
    }

    #[test]
    fn reattach_after_close() {
        let mut state = authority();
        state.set_trusted_observer_enabled(true).unwrap();
        let mut host = TrustedObserverHost::new();
        let first = host
            .attach(&mut state, Box::new(RecordingEndpoint::default()))
            .unwrap();
        host.close(&mut state).unwrap();
        let second = host
            .attach(&mut state, Box::new(RecordingEndpoint::default()))
            .unwrap();
        assert_ne!(first, second);
        assert!(state.is_trusted_observer(second));
        assert!(!state.is_trusted_observer(first));
    }

    #[test]
    fn non_overworld_center_refused() {
        let mut state = authority();
        state.set_trusted_observer_enabled(true).unwrap();
        let mut host = TrustedObserverHost::new();
        host.attach(&mut state, Box::new(RecordingEndpoint::default()))
            .unwrap();
        let nether = Dimension::new(1).unwrap();
        assert_eq!(
            host.set_center(&mut state, nether, ChunkPos::new(0, 0)),
            Err(ServerError::InvalidInput {
                field: "trusted_observer_center",
            })
        );
    }
}
