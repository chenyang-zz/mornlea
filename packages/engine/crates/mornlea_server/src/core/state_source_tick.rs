//! Exclusive post-commit source tick ownership.
//!
//! A held continuation prevents external authority and goal mutation until the
//! original reduction completes. Normal abandonment fences committed state;
//! callers must retain this private owner across waits rather than replay a tick.

use super::*;
use crate::core::publication_project::EncodedSourceProjectionWork;
use crate::core::step::ReducedTick;

pub(crate) struct SourceTickContinuation<'a> {
    authority: &'a mut AuthorityState,
    goals: &'a mut SourceGoals,
    reduced: Option<ReducedTick>,
    projection: Option<EncodedSourceProjectionWork>,
}

impl AuthorityState {
    pub(crate) fn begin_source_tick<'a>(
        &'a mut self,
        work: TickBudget,
        goals: &'a mut SourceGoals,
    ) -> Result<SourceTickContinuation<'a>, ServerError> {
        self.check_source_tick(work)?;
        let reduced = crate::core::step::prepare_source_tick(self, work, goals)?;
        Ok(SourceTickContinuation {
            authority: self,
            goals,
            reduced: Some(reduced),
            projection: None,
        })
    }
}

impl SourceTickContinuation<'_> {
    pub(crate) fn state(&self) -> &AuthorityState {
        self.authority
    }
    pub(crate) fn tick(&self) -> u64 {
        self.reduced.as_ref().expect("held reduction").tick
    }

    pub(crate) fn prepare_encoding(&mut self) -> Result<(), ServerError> {
        if self.projection.is_some() {
            return Err(self.fail_encoding(ServerError::Internal {
                invariant: "source projection already prepared",
            }));
        }
        let reduced = self.reduced.as_ref().expect("held reduction");
        self.projection = Some(crate::core::step::tick_fence(self.authority, |state| {
            EncodedSourceProjectionWork::prepare(state, reduced.tick, &reduced.outcome)
        })?);
        Ok(())
    }

    pub(crate) fn poll_encoding(
        &mut self,
        encoding: &mut crate::core::source_encoding::SourceSnapshotEncoding,
    ) -> Result<bool, ServerError> {
        let projection = self.projection.as_mut().expect("prepared projection");
        crate::core::step::tick_fence(self.authority, |state| projection.poll(state, encoding))
    }

    /// Retains the exclusive references after moving the original result to its caller.
    pub(crate) fn complete_encoded_with<T>(
        &mut self,
        then: impl FnOnce(
            &mut AuthorityState,
            &mut SourceGoals,
            TickPublication,
        ) -> Result<T, ServerError>,
    ) -> Result<T, ServerError> {
        if self.state().next_tick() != self.tick() {
            return Err(self.fail_encoding(ServerError::Internal {
                invariant: "source tick continuation identity",
            }));
        }
        let reduced = self.reduced.take().expect("held reduction");
        let work = self.projection.take().expect("prepared projection");
        let publication = crate::core::step::tick_fence(self.authority, |state| {
            let projection = work.finish(state, &reduced.outcome);
            crate::core::step::finish_encoded_source_tick(state, reduced, self.goals, projection)
        })?;
        self.authority.next_tick = self.authority.next_tick.saturating_add(1);
        then(self.authority, self.goals, publication)
    }

    pub(crate) fn fail_encoding(&mut self, error: ServerError) -> ServerError {
        if let Some(work) = self.projection.take() {
            work.restore(self.authority);
        }
        self.reduced.take();
        self.authority.fail_tick(error)
    }

    /// Consumes the original reduction; no second dispatch can occur.
    pub(crate) fn complete(mut self) -> Result<TickPublication, ServerError> {
        if self.state().next_tick() != self.tick() {
            return Err(self.abort(ServerError::Internal {
                invariant: "source tick continuation identity",
            }));
        }
        let reduced = self.reduced.take().expect("held reduction");
        match crate::core::step::finish_source_tick(self.authority, reduced, self.goals) {
            Ok(publication) => {
                self.authority.next_tick = self.authority.next_tick.saturating_add(1);
                Ok(publication)
            }
            Err(error) => Err(self.abort(error)),
        }
    }

    /// A cancelled committed tick is a hard authority failure, not a rollback.
    pub(crate) fn abort(mut self, error: ServerError) -> ServerError {
        self.fail_encoding(error)
    }
}

impl Drop for SourceTickContinuation<'_> {
    fn drop(&mut self) {
        if let Some(work) = self.projection.take() {
            work.restore(self.authority);
        }
        if self.reduced.is_some() {
            self.authority.fail_tick(ServerError::Internal {
                invariant: "source tick continuation abandoned",
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::*;
    use crate::core::step::{AuthoritativeFinalReducer, set_dispatch_hook};
    use mornlea_domain::{CommandRejection, Event};
    use std::cell::Cell;

    thread_local! { static CALLS: Cell<usize> = const { Cell::new(0) }; }
    struct Hook;
    impl Hook {
        fn install(hook: fn(&mut TickContext<'_>) -> Result<(), ServerError>) -> Self {
            CALLS.with(|n| n.set(0));
            set_dispatch_hook(Some(hook));
            Self
        }
    }
    impl Drop for Hook {
        fn drop(&mut self) {
            set_dispatch_hook(None);
        }
    }
    fn budget() -> TickBudget {
        TickBudget::try_new(0, 0, 0, 0, 0).unwrap()
    }
    fn key() -> ChunkKey {
        ChunkKey {
            dimension: Dimension::OVERWORLD,
            pos: ChunkPos::new(0, 0),
        }
    }
    fn pos() -> BlockPos {
        BlockPos::new(15, 200, 15)
    }
    fn fixture() -> (AuthorityState, SessionKey, SourceGoals) {
        let mut a = AuthorityState::try_new(
            ServerLimits::try_new(8, 4096, 512, 64, 64, 1_048_576).unwrap(),
            42,
        )
        .unwrap();
        let id =
            PlayerId::try_from_bytes([1, 0, 0, 0, 0, 0, 64, 0, 128, 0, 0, 0, 0, 0, 0, 1]).unwrap();
        let start = mornlea_protocol::LoginStart::new(id, "Ada", 8).unwrap();
        let session = a
            .prepare(
                mornlea_protocol::admit_login(
                    mornlea_protocol::LoginStart::decode_inbound(&start.encode().unwrap()).unwrap(),
                )
                .unwrap(),
                TransportKind::Memory,
            )
            .unwrap();
        a.install(session, None).unwrap();
        a.activate(session).unwrap();
        // This prepared bodyless session isolates continuation ownership from login seeding.
        a.sessions.get_mut(&session).unwrap().body = None;
        a.enable_live_chunks().unwrap();
        a.advance_tick(budget()).unwrap();
        a.sessions.get_mut(&session).unwrap().outbox.clear();
        let mut ctx = TickContext::for_tick(&mut a, budget());
        ctx.stage(RuleEffect::Environment(EnvironmentState {
            seed: 42,
            next_tick: 1,
            world_time: 100,
            day_phase_offset: 7800,
            season_offset: 0,
            weather: Weather::Clear,
            weather_remaining: 5000,
            difficulty: 0,
            tunables: RuleTunables::source_defaults(),
        }))
        .unwrap();
        ctx.commit_carried();
        drop(ctx);
        a.fluid_schedule.enqueue_fluid(key(), pos(), 100);
        a.farmland_schedule.enqueue_candidate(key(), pos(), 100);
        (a, session, SourceGoals::default())
    }
    fn calls(n: usize) {
        CALLS.with(|c| assert_eq!(c.get(), n));
    }
    fn emit(ctx: &mut TickContext<'_>, to: EventRecipient) -> Result<(), ServerError> {
        CALLS.with(|c| c.set(c.get() + 1));
        ctx.emit(RoutedEvent::new(
            to,
            Event::CommandRejected(CommandRejection::new(1, RejectReason::InvalidRay)),
        ))
    }
    fn success(ctx: &mut TickContext<'_>) -> Result<(), ServerError> {
        emit(ctx, EventRecipient::Broadcast)
    }
    fn stale(ctx: &mut TickContext<'_>) -> Result<(), ServerError> {
        emit(ctx, EventRecipient::Session(999999))
    }
    fn fault(ctx: &mut TickContext<'_>) -> Result<(), ServerError> {
        success(ctx)?;
        Err(ServerError::Internal {
            invariant: "source continuation provider failure",
        })
    }
    fn unwind(ctx: &mut TickContext<'_>) -> Result<(), ServerError> {
        success(ctx)?;
        panic!("intentional continuation dispatch unwind")
    }
    fn owners(a: &AuthorityState) {
        assert_eq!(
            a.fluid_schedule.fluid_due(Dimension::OVERWORLD, pos()),
            Some(100)
        );
        assert_eq!(
            a.farmland_schedule
                .candidate_due(Dimension::OVERWORLD, pos()),
            Some(100)
        );
    }
    fn empty(a: &AuthorityState, s: SessionKey) {
        assert!(a.sessions[&s].outbox.is_empty());
    }
    fn rejected(p: &TickPublication) -> usize {
        p.events.iter().filter(|r|matches!(r.event(),Event::CommandRejected(x) if x.sequence()==1 && x.reason()==RejectReason::InvalidRay)).count()
    }
    fn fenced(a: &mut AuthorityState, g: &mut SourceGoals, s: SessionKey, e: ServerError) {
        assert_eq!(a.phase(), ServerPhase::Closing);
        assert_eq!(a.tick_failure(), Some(e));
        assert_eq!(a.next_tick(), 1);
        empty(a, s);
        owners(a);
        assert_eq!(a.capture_source_snapshot(key()).err(), Some(e));
        assert_eq!(a.try_metadata_snapshot(), Err(e));
        assert_eq!(a.advance_tick(budget()), Err(e));
        assert_eq!(a.advance_source_tick(budget(), g), Err(e));
        assert_eq!(a.run_final(&mut AuthoritativeFinalReducer), Err(e));
        assert_eq!(a.fail_tick(ServerError::InvalidInput { field: "later" }), e);
        assert_eq!(a.next_tick(), 1);
        calls(1);
    }

    #[test]
    fn source_continuation_suspends_counter_and_delivery() {
        let (mut a, s, mut g) = fixture();
        let _hook = Hook::install(success);
        let pending = a.begin_source_tick(budget(), &mut g).unwrap();
        assert_eq!(pending.tick(), 1);
        assert_eq!(pending.state().next_tick(), 1);
        empty(pending.state(), s);
        owners(pending.state());
        calls(1);
        assert_eq!(
            pending
                .state()
                .residents
                .environment
                .as_ref()
                .unwrap()
                .world_time,
            101
        );
        assert_eq!(pending.state().tick_failure(), None);
        for _ in 0..3 {
            assert_eq!(pending.state().next_tick(), 1);
            empty(pending.state(), s);
            calls(1);
        }
        let publication = pending.complete().unwrap();
        assert_eq!(
            (
                publication.tick,
                publication.counters.executed_tick,
                rejected(&publication)
            ),
            (1, 1, 1)
        );
        assert_eq!(a.next_tick(), 2);
        assert!(!a.sessions[&s].outbox.is_empty());
        assert_eq!(a.residents.environment.as_ref().unwrap().world_time, 101);
        owners(&a);
        calls(1);
    }
    #[test]
    fn source_continuation_completes_original_reduction_once() {
        let (mut a, s, mut g) = fixture();
        let _hook = Hook::install(success);
        let pending = a.begin_source_tick(budget(), &mut g).unwrap();
        assert_eq!(pending.state().next_tick(), 1);
        empty(pending.state(), s);
        let first = pending.complete().unwrap();
        assert_eq!((first.tick, rejected(&first)), (1, 1));
        calls(1);
        let frames = a.sessions[&s].outbox.len();
        assert!(frames > 0);
        assert_eq!(a.tick_failure(), None);
        set_dispatch_hook(Some(success));
        let second = a.advance_source_tick(budget(), &mut g).unwrap();
        assert_eq!((second.tick, rejected(&second), a.next_tick()), (2, 1, 3));
        calls(2);
        assert_eq!(a.residents.environment.as_ref().unwrap().world_time, 102);
        assert!(a.sessions[&s].outbox.len() > frames);
        assert_eq!(a.phase(), ServerPhase::Running);
    }
    #[test]
    fn source_continuation_abandonment_fences_without_counter() {
        let (mut a, s, mut g) = fixture();
        let _hook = Hook::install(success);
        let pending = a.begin_source_tick(budget(), &mut g).unwrap();
        drop(pending);
        let error = ServerError::Internal {
            invariant: "source tick continuation abandoned",
        };
        fenced(&mut a, &mut g, s, error);
        assert_eq!(a.residents.environment.as_ref().unwrap().world_time, 101);
    }
    #[test]
    fn source_continuation_explicit_abort_retains_first_error() {
        let (mut a, s, mut g) = fixture();
        let _hook = Hook::install(success);
        let pending = a.begin_source_tick(budget(), &mut g).unwrap();
        assert_eq!(
            pending.abort(ServerError::Disconnected),
            ServerError::Disconnected
        );
        fenced(&mut a, &mut g, s, ServerError::Disconnected);
        assert_eq!(a.residents.environment.as_ref().unwrap().world_time, 101);
    }
    #[test]
    fn source_continuation_late_delivery_failure_reaches_completion() {
        let (mut a, s, mut g) = fixture();
        let _hook = Hook::install(stale);
        let pending = a.begin_source_tick(budget(), &mut g).unwrap();
        assert_eq!(pending.state().next_tick(), 1);
        empty(pending.state(), s);
        assert_eq!(
            pending
                .state()
                .residents
                .environment
                .as_ref()
                .unwrap()
                .world_time,
            101
        );
        let error = ServerError::StaleSession {
            session: SessionKey::from_raw(999999).unwrap(),
        };
        assert_eq!(pending.complete(), Err(error));
        fenced(&mut a, &mut g, s, error);
        assert_eq!(a.residents.environment.as_ref().unwrap().world_time, 101);
    }
    #[test]
    fn source_continuation_dispatch_error_and_unwind_restore_owners() {
        for panic in [false, true] {
            let (mut a, s, mut g) = fixture();
            let _hook = Hook::install(if panic { unwind } else { fault });
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                a.begin_source_tick(budget(), &mut g).err()
            }));
            let error = ServerError::Internal {
                invariant: if panic {
                    "authoritative tick panic"
                } else {
                    "source continuation provider failure"
                },
            };
            assert_eq!(result.unwrap(), Some(error));
            fenced(&mut a, &mut g, s, error);
            assert_eq!(a.residents.environment.as_ref().unwrap().world_time, 100);
        }
    }
    #[test]
    fn source_continuation_validation_precedes_any_reduction() {
        for phase in [
            ServerPhase::Running,
            ServerPhase::Closing,
            ServerPhase::Closed,
        ] {
            let mut a = AuthorityState::try_new(
                ServerLimits::try_new(8, 4096, 512, 64, 64, 1_048_576).unwrap(),
                42,
            )
            .unwrap();
            a.phase = phase;
            let mut g = SourceGoals::default();
            let _hook = Hook::install(success);
            assert_eq!(
                a.begin_source_tick(budget(), &mut g).err(),
                Some(ServerError::InvalidState { phase })
            );
            calls(0);
            assert_eq!(a.next_tick(), 0);
            assert_eq!(a.tick_failure(), None);
            a.fail_tick(ServerError::Disconnected);
            assert_eq!(
                a.begin_source_tick(budget(), &mut g).err(),
                Some(ServerError::Disconnected)
            );
            calls(0);
        }
    }
}
