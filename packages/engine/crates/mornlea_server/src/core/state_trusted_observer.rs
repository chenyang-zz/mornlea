//! Trusted observer attach/detach/center freeze.
//!
//! Mirrors Go `session_registry.go` attach/detach/setTrustedObserverCenter:
//! observers subscribe only, never occupy a `MAX_PLAYERS` slot, and never
//! appear in `online_players()`. Lifecycle mutations require the between-ticks
//! gate. Cap is `MAX_TRUSTED_OBSERVERS` (= 1), matching Go's singular slot.

use super::*;
use mornlea_domain::{ChunkPos, Dimension, PlayerId};

pub(crate) const TRUSTED_OBSERVER_DISABLED: &str = "trusted_observer_disabled";

/// Sentinel v4 UUID bytes for observer session records (not a playable
/// identity): version nibble 4 at byte 6, RFC 4122 variant at byte 8. Fixed so
/// attach never depends on entropy. Excluded from player duplicate checks
/// because observers skip `current_sessions`.
const OBSERVER_PLACEHOLDER_BYTES: [u8; 16] = [
    0x7e, 0x52, 0x75, 0x53, 0x74, 0x45, 0x40, 0x62, 0x80, 0x73, 0x65, 0x72, 0x76, 0x65, 0x72, 0x01,
];

/// Builds the placeholder identity; a refusal is a typed internal error.
fn observer_placeholder_player_id() -> Result<PlayerId, ServerError> {
    PlayerId::try_from_bytes(OBSERVER_PLACEHOLDER_BYTES).map_err(|_| ServerError::Internal {
        invariant: "observer placeholder id",
    })
}

/// Per-observer frozen subscription state.
#[derive(Clone, Debug)]
pub(super) struct TrustedObserverRecord {
    #[allow(dead_code)] // retained for Go generation parity / future CAS
    pub(super) session: SessionKey,
    #[allow(dead_code)] // retained for Go generation parity / future CAS
    pub(super) generation: u64,
    /// Center written between ticks; applied at the next tick boundary.
    pub(crate) pending_center: Option<(Dimension, ChunkPos)>,
    /// Last center applied at a tick boundary (sequence > 0 when set).
    pub(crate) applied_center: Option<(Dimension, ChunkPos)>,
    pub(crate) applied_sequence: u64,
}

impl AuthorityState {
    /// Plain enable flag (gap 3 wires config; this PR takes a bool only).
    pub fn set_trusted_observer_enabled(&mut self, enabled: bool) -> Result<(), ServerError> {
        self.require_between_ticks()?;
        self.trusted_observer_enabled = enabled;
        if !enabled {
            // Detach any live observer when the flag turns off.
            let keys: Vec<SessionKey> = self.trusted_observers.keys().copied().collect();
            for key in keys {
                let _ = self.detach_trusted_observer(key);
            }
        }
        Ok(())
    }

    pub fn trusted_observer_enabled(&self) -> bool {
        self.trusted_observer_enabled
    }

    pub fn trusted_observer_count(&self) -> usize {
        self.trusted_observers.len()
    }

    pub fn is_trusted_observer(&self, session: SessionKey) -> bool {
        self.trusted_observers.contains_key(&session)
    }

    /// Attach one observer. Does not increment `occupied` / take a player slot.
    ///
    /// Refusal order follows Go `attachTrustedObserverLocked`: a non-running
    /// server or a disabled flag returns `ErrInvalidSession` (here the invalid
    /// `session` input) before the occupied slot (`ErrSessionExists`, here
    /// Capacity) is considered.
    pub fn attach_trusted_observer(&mut self) -> Result<SessionKey, ServerError> {
        self.require_between_ticks()?;
        if self.phase != ServerPhase::Running || !self.trusted_observer_enabled {
            return Err(ServerError::InvalidInput { field: "session" });
        }
        if self.trusted_observers.len() >= MAX_TRUSTED_OBSERVERS {
            return Err(ServerError::Capacity {
                resource: Resource::TrustedObservers,
                limit: MAX_TRUSTED_OBSERVERS,
                observed: self.trusted_observers.len() + 1,
            });
        }
        // Session-id exhaustion refuses exactly like player admission.
        if self.ids_exhausted {
            return Err(ServerError::Capacity {
                resource: Resource::Players,
                limit: usize::from(self.limits.max_players()),
                observed: self.occupied + 1,
            });
        }
        let placeholder = observer_placeholder_player_id()?;
        let raw = self.next_session;
        let key = SessionKey::from_raw(raw).ok_or(ServerError::Internal {
            invariant: "session id",
        })?;
        if raw == u64::MAX {
            self.ids_exhausted = true;
        } else {
            self.next_session = raw + 1;
        }
        self.trusted_observer_generation =
            self.trusted_observer_generation
                .checked_add(1)
                .ok_or(ServerError::Internal {
                    invariant: "trusted observer generation",
                })?;
        let generation = self.trusted_observer_generation;
        // Engine upper-bound subscription: view_distance such that
        // view_distance+1 clamps to limits.view_radius() (Go RegisterObserverSession).
        let view_distance =
            u8::try_from(self.limits.view_radius().saturating_sub(1)).unwrap_or(u8::MAX);
        self.insert_observer_session(key, placeholder, view_distance);
        self.trusted_observers.insert(
            key,
            TrustedObserverRecord {
                session: key,
                generation,
                pending_center: None,
                applied_center: None,
                applied_sequence: 0,
            },
        );
        self.session_views.entry(key).or_default();
        Ok(key)
    }

    /// Detach one observer between ticks. Player `occupied` is untouched.
    pub fn detach_trusted_observer(&mut self, session: SessionKey) -> Result<(), ServerError> {
        self.require_between_ticks()?;
        self.detach_trusted_observer_unchecked(session)
    }

    /// Internal detach used by publication saturation (may run while cleaning up).
    pub(crate) fn detach_trusted_observer_unchecked(
        &mut self,
        session: SessionKey,
    ) -> Result<(), ServerError> {
        let Some(record) = self.trusted_observers.remove(&session) else {
            return Err(ServerError::StaleSession { session });
        };
        let _ = record;
        if let Some(entry) = self.sessions.get_mut(&session) {
            entry.phase = SessionPhase::Retired;
            entry.outbox_closed = true;
        }
        self.sessions.remove(&session);
        self.session_views.remove(&session);
        self.views.remove(&session);
        Ok(())
    }

    /// Queue a center update; applied at the next tick boundary.
    pub fn set_trusted_observer_center(
        &mut self,
        session: SessionKey,
        dimension: Dimension,
        center: ChunkPos,
    ) -> Result<(), ServerError> {
        self.require_between_ticks()?;
        // Go `setTrustedObserverCenterLocked` folds a non-running server, a
        // disabled flag and a missing observer into `ErrTrustedObserverDisabled`.
        if self.phase != ServerPhase::Running || !self.trusted_observer_enabled {
            return Err(ServerError::InvalidInput {
                field: TRUSTED_OBSERVER_DISABLED,
            });
        }
        let Some(observer) = self.trusted_observers.get_mut(&session) else {
            return Err(ServerError::InvalidInput {
                field: TRUSTED_OBSERVER_DISABLED,
            });
        };
        if dimension != Dimension::OVERWORLD {
            return Err(ServerError::InvalidInput {
                field: "trusted_observer_center",
            });
        }
        observer.pending_center = Some((dimension, center));
        Ok(())
    }

    /// Applied center after the last tick boundary, if any.
    pub fn applied_trusted_observer_center(
        &self,
        session: SessionKey,
    ) -> Option<(Dimension, ChunkPos, u64)> {
        let observer = self.trusted_observers.get(&session)?;
        let (dimension, center) = observer.applied_center?;
        if observer.applied_sequence == 0 {
            return None;
        }
        Some((dimension, center, observer.applied_sequence))
    }

    /// Promote pending centers at the tick boundary (called from `for_tick`).
    pub(crate) fn apply_trusted_observer_centers(&mut self) {
        for observer in self.trusted_observers.values_mut() {
            if let Some((dimension, center)) = observer.pending_center.take() {
                observer.applied_sequence = observer.applied_sequence.saturating_add(1);
                observer.applied_center = Some((dimension, center));
            }
        }
    }

    /// Applied center facts for publication (session, dimension, center, radius).
    pub(crate) fn trusted_observer_view_facts(&self) -> Vec<(SessionKey, Dimension, ChunkPos, u8)> {
        self.trusted_observers
            .values()
            .filter_map(|observer| {
                let (dimension, center) = observer.applied_center?;
                let radius = self.session_view_radius(observer.session)?;
                Some((observer.session, dimension, center, radius))
            })
            .collect()
    }

    fn insert_observer_session(&mut self, key: SessionKey, player_id: PlayerId, view_distance: u8) {
        self.sessions.insert(
            key,
            SessionRecord {
                player_id,
                display_name: String::new(),
                view_distance,
                phase: SessionPhase::Active,
                last_applied_sequence: 0,
                last_input_sequence: 0,
                next_arrival: 0,
                body: None,
                loaded_current: false,
                outbox: VecDeque::new(),
                outbox_closed: false,
            },
        );
        // Intentionally NOT inserted into current_sessions and NOT counted in occupied.
    }
}

#[cfg(test)]
mod trusted_observer_tests {
    use super::*;
    use mornlea_domain::{ChunkPos, Dimension, PlayerId};
    use mornlea_protocol::{LoginStart, admit_login};

    fn authority() -> AuthorityState {
        AuthorityState::try_new(
            ServerLimits::try_new(8, 4096, 512, 64, 64, 1_048_576).unwrap(),
            7,
        )
        .unwrap()
    }

    fn login(tag: u8, name: &str) -> mornlea_protocol::AdmittedLogin {
        let mut bytes = [0u8; 16];
        bytes[0] = tag;
        bytes[6] = 0x40;
        bytes[8] = 0x80;
        let id = PlayerId::try_from_bytes(bytes).unwrap();
        let start = LoginStart::new(id, name, 8).unwrap();
        let inbound = LoginStart::decode_inbound(&start.encode().unwrap()).unwrap();
        admit_login(inbound).unwrap()
    }

    /// Go `attachTrustedObserverLocked` refuses a disabled observer with
    /// `ErrInvalidSession`; the Rust equivalent is the invalid `session` input.
    #[test]
    fn disabled_attach_is_invalid_session() {
        let mut state = authority();
        let err = state.attach_trusted_observer().expect_err("disabled");
        assert_eq!(err, ServerError::InvalidInput { field: "session" });
        assert_eq!(state.trusted_observer_count(), 0);
    }

    /// Go `attachTrustedObserverLocked` returns `ErrInvalidSession` for a
    /// non-running server, the same error as the disabled branch.
    #[test]
    fn attach_on_closing_server_is_invalid_session() {
        let mut state = authority();
        state.set_trusted_observer_enabled(true).unwrap();
        state.phase = ServerPhase::Closing;
        let err = state.attach_trusted_observer().expect_err("closing");
        assert_eq!(err, ServerError::InvalidInput { field: "session" });
        assert_eq!(state.trusted_observer_count(), 0);
    }

    /// Go evaluates `lifecycle != serverRunning || !TrustedObserver` before
    /// `trustedObserver != nil`, so the first check decides the error:
    /// closing and disabled together, or closing with an observer already
    /// attached, both return `ErrInvalidSession` (never `ErrSessionExists`).
    #[test]
    fn attach_check_order_follows_go() {
        let mut state = authority();
        state.phase = ServerPhase::Closing;
        assert!(!state.trusted_observer_enabled());
        assert_eq!(
            state.attach_trusted_observer(),
            Err(ServerError::InvalidInput { field: "session" }),
            "closing and disabled"
        );
        let mut state = authority();
        state.set_trusted_observer_enabled(true).unwrap();
        state.attach_trusted_observer().unwrap();
        state.phase = ServerPhase::Closing;
        assert_eq!(
            state.attach_trusted_observer(),
            Err(ServerError::InvalidInput { field: "session" }),
            "closing with an attached observer"
        );
        state.phase = ServerPhase::Running;
        assert!(matches!(
            state.attach_trusted_observer(),
            Err(ServerError::Capacity {
                resource: Resource::TrustedObservers,
                ..
            })
        ));
    }

    /// Session-id exhaustion uses the same refusal as player admission.
    #[test]
    fn attach_with_exhausted_ids_matches_player_refusal() {
        let mut state = authority();
        state.set_trusted_observer_enabled(true).unwrap();
        state.ids_exhausted = true;
        let observer_err = state.attach_trusted_observer().expect_err("exhausted");
        let player_err = state
            .admit(login(1, "Ada"), TransportKind::Memory)
            .expect_err("exhausted");
        assert_eq!(observer_err, player_err);
        assert!(matches!(
            observer_err,
            ServerError::Capacity {
                resource: Resource::Players,
                ..
            }
        ));
        assert_eq!(state.trusted_observer_count(), 0);
    }

    #[test]
    fn observer_placeholder_id_constructs_without_panic() {
        let id = observer_placeholder_player_id().expect("valid v4 placeholder");
        assert_eq!(id.bytes(), OBSERVER_PLACEHOLDER_BYTES);
    }

    #[test]
    fn attach_does_not_consume_player_slot() {
        let mut state = authority();
        state.set_trusted_observer_enabled(true).unwrap();
        let observer = state.attach_trusted_observer().unwrap();
        assert!(state.is_trusted_observer(observer));
        assert_eq!(state.trusted_observer_count(), 1);
        assert_eq!(state.occupied, 0, "observer must not increment occupied");
        for tag in 1..=8u8 {
            state
                .admit(login(tag, &format!("P{tag}")), TransportKind::Memory)
                .expect("eight players still admit beside the observer");
        }
        assert_eq!(state.occupied, 8);
        let ninth = state.admit(login(9, "Ivy"), TransportKind::Memory);
        assert!(matches!(
            ninth,
            Err(ServerError::Capacity {
                resource: Resource::Players,
                ..
            })
        ));
        // No resident player actor for the observer session.
        assert!(
            !state
                .residents
                .actors
                .iter()
                .any(|actor| actor.key == ActorKey::Player(observer))
        );
    }

    #[test]
    fn second_attach_is_capacity() {
        let mut state = authority();
        state.set_trusted_observer_enabled(true).unwrap();
        state.attach_trusted_observer().unwrap();
        let err = state.attach_trusted_observer().expect_err("cap");
        assert!(matches!(
            err,
            ServerError::Capacity {
                resource: Resource::TrustedObservers,
                limit: MAX_TRUSTED_OBSERVERS,
                observed: 2,
            }
        ));
    }

    #[test]
    fn center_applies_only_at_tick_boundary() {
        let mut state = authority();
        state.set_trusted_observer_enabled(true).unwrap();
        let observer = state.attach_trusted_observer().unwrap();
        let center = ChunkPos::new(4, -2);
        state
            .set_trusted_observer_center(observer, Dimension::OVERWORLD, center)
            .unwrap();
        assert!(
            state.applied_trusted_observer_center(observer).is_none(),
            "pending center is not applied before a tick"
        );
        {
            let _ctx = TickContext::for_tick(&mut state, TickBudget::full());
        }
        let applied = state
            .applied_trusted_observer_center(observer)
            .expect("applied after tick boundary");
        assert_eq!(applied.0, Dimension::OVERWORLD);
        assert_eq!(applied.1, center);
        assert_eq!(applied.2, 1);
    }

    #[test]
    fn attach_detach_center_refuse_during_tick() {
        let mut state = authority();
        state.set_trusted_observer_enabled(true).unwrap();
        let observer = state.attach_trusted_observer().unwrap();
        let ctx = TickContext::for_tick(&mut state, TickBudget::full());
        assert!(matches!(
            ctx.authority.attach_trusted_observer(),
            Err(ServerError::InvalidState { .. })
        ));
        assert!(matches!(
            ctx.authority.detach_trusted_observer(observer),
            Err(ServerError::InvalidState { .. })
        ));
        assert!(matches!(
            ctx.authority.set_trusted_observer_center(
                observer,
                Dimension::OVERWORLD,
                ChunkPos::new(0, 0)
            ),
            Err(ServerError::InvalidState { .. })
        ));
        drop(ctx);
        state.detach_trusted_observer(observer).unwrap();
        assert_eq!(state.trusted_observer_count(), 0);
        assert_eq!(state.occupied, 0);
    }

    #[test]
    fn set_center_when_disabled_is_typed_refusal() {
        let mut state = authority();
        let err = state.set_trusted_observer_center(
            SessionKey::from_raw(1).unwrap(),
            Dimension::OVERWORLD,
            ChunkPos::new(0, 0),
        );
        assert!(matches!(
            err,
            Err(ServerError::InvalidInput {
                field: TRUSTED_OBSERVER_DISABLED
            })
        ));
    }

    /// Go `setTrustedObserverCenterLocked` folds a non-running server into
    /// `ErrTrustedObserverDisabled`.
    #[test]
    fn set_center_on_closing_server_is_disabled_refusal() {
        let mut state = authority();
        state.set_trusted_observer_enabled(true).unwrap();
        let observer = state.attach_trusted_observer().unwrap();
        state.phase = ServerPhase::Closing;
        let err =
            state.set_trusted_observer_center(observer, Dimension::OVERWORLD, ChunkPos::new(0, 0));
        assert_eq!(
            err,
            Err(ServerError::InvalidInput {
                field: TRUSTED_OBSERVER_DISABLED
            })
        );
    }

    /// Go `setTrustedObserverCenterLocked` checks lifecycle before the
    /// dimension, so a closing server refuses a non-overworld center as
    /// disabled rather than as an invalid center.
    #[test]
    fn set_center_check_order_follows_go() {
        let mut state = authority();
        state.set_trusted_observer_enabled(true).unwrap();
        let observer = state.attach_trusted_observer().unwrap();
        let nether = Dimension::new(1).unwrap();
        assert_eq!(
            state.set_trusted_observer_center(observer, nether, ChunkPos::new(0, 0)),
            Err(ServerError::InvalidInput {
                field: "trusted_observer_center"
            })
        );
        state.phase = ServerPhase::Closing;
        assert_eq!(
            state.set_trusted_observer_center(observer, nether, ChunkPos::new(0, 0)),
            Err(ServerError::InvalidInput {
                field: TRUSTED_OBSERVER_DISABLED
            })
        );
    }

    #[test]
    fn retire_player_does_not_detach_observer() {
        let mut state = authority();
        state.set_trusted_observer_enabled(true).unwrap();
        let observer = state.attach_trusted_observer().unwrap();
        let player = state.admit(login(1, "Ada"), TransportKind::Memory).unwrap();
        state.retire(player, CloseReason::PeerGone).unwrap();
        assert!(state.is_trusted_observer(observer));
        assert_eq!(state.occupied, 0);
    }

    #[test]
    fn publish_saturation_detaches_only_the_observer() {
        use crate::core::publication::PreparedFrame;
        use mornlea_protocol::{CommandRejected, ProtocolCodec, ServerPacket};

        let mut state = authority();
        state.set_trusted_observer_enabled(true).unwrap();
        let observer = state.attach_trusted_observer().unwrap();
        let player = state.admit(login(1, "Ada"), TransportKind::Memory).unwrap();
        let limit = state.limits().session_outbox();
        let mut codec = ProtocolCodec::new().unwrap();
        let packet = ServerPacket::CommandRejected(CommandRejected::new(1, 1).unwrap());
        for _ in 0..limit {
            let frame = PreparedFrame::encode(&mut codec, &packet).unwrap();
            let outcome = state.enqueue_prepared(observer, frame).unwrap();
            assert_eq!(outcome, crate::core::publication::EnqueueOutcome::Queued);
        }
        let overflow = PreparedFrame::encode(&mut codec, &packet).unwrap();
        let outcome = state.enqueue_prepared(observer, overflow).unwrap();
        assert_eq!(outcome, crate::core::publication::EnqueueOutcome::Closed);
        assert!(
            !state.is_trusted_observer(observer),
            "saturated observer must detach"
        );
        assert!(
            state.session(player).is_some(),
            "player session must survive observer saturation"
        );
        assert_eq!(state.occupied, 1);
    }
}
