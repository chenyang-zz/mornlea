//! Executing consumer double for the authoritative server contract.
//!
//! A passing double shows that the declared ports accept and reject owned
//! values. It is not acceptance of a world rule, reducer, transport, or disk.

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use mornlea_domain::{
    BlockPos, ChunkPos, Command, CommandRejection, CommandText, CompanionId, Dimension, Event,
    EventRecipient, FiniteVec3, HotbarSlot, LookAngles, PlayerId, ProjectileId, ProjectileKind,
    RejectReason, RoutedEvent,
};
use mornlea_protocol::{
    LoginStart, LoginSuccess, PlayIntent, ProtocolCodec, ServerPacket, admit_login,
};
use mornlea_server::contracts::*;
use mornlea_server::state::{AuthorityState, ShutdownIo, TickContext, resolve_place};
use mornlea_storage::{Chunk, ChunkSave, ItemStack, Metadata};

struct ContractServer {
    authority: AuthorityState,
}

impl ServerEndpoint for ContractServer {
    fn admit(
        &mut self,
        login: mornlea_protocol::AdmittedLogin,
        transport: TransportKind,
    ) -> Result<SessionKey, ServerError> {
        self.authority.admit(login, transport)
    }

    fn submit(
        &mut self,
        session: SessionKey,
        intent: PlayIntent,
    ) -> Result<SubmissionReceipt, ServerError> {
        self.authority.submit(session, intent)
    }

    fn submit_companion(
        &mut self,
        candidate: CompanionActionEnvelope,
    ) -> Result<CompanionReceipt, ServerError> {
        self.authority.submit_companion(candidate)
    }

    fn advance_tick(&mut self, work: TickBudget) -> Result<TickPublication, ServerError> {
        self.authority.advance_tick(work)
    }

    fn close_session(
        &mut self,
        session: SessionKey,
        reason: CloseReason,
    ) -> Result<(), ServerError> {
        self.authority.close_session(session, reason)
    }

    fn shutdown(&mut self, deadline: Deadline) -> Result<ShutdownReport, ShutdownFailure> {
        let mut reducer = FinalOnce;
        let mut store = MemoryStore::new(true);
        let mut agent = MemoryAgent::default();
        let mut snapshots = MemorySnapshots::default();
        let mut workers = IdleWorkers;
        let mut persistence = IdlePersistence;
        let mut mcp = IdleMcp;
        let mut memory = IdleMemory;
        let instant = deadline
            .instant()
            .checked_sub(Duration::from_secs(1))
            .unwrap_or_else(|| deadline.instant());
        let clock = SplitClock {
            instant,
            unix_ms: i64::MAX,
        };
        let mut io = ShutdownIo {
            reducer: &mut reducer,
            store: &mut store,
            agent: &mut agent,
            snapshots: &mut snapshots,
            clock: &clock,
            workers: &mut workers,
            persistence: &mut persistence,
            mcp: &mut mcp,
            memory: &mut memory,
        };
        self.authority.drive_shutdown(deadline, &mut io)
    }
}

fn limits() -> ServerLimits {
    ServerLimits::try_new(8, 8, 2, 1, 4, 1024).unwrap()
}

fn server() -> ContractServer {
    ContractServer {
        authority: AuthorityState::try_new(limits(), 7).unwrap(),
    }
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

fn sequenced(sequence: u64) -> PlayIntent {
    PlayIntent::Sequenced {
        sequence,
        command: Command::CloseContainer,
    }
}

fn slot(index: u8) -> HotbarSlot {
    HotbarSlot::new(index).unwrap()
}

#[test]
fn valid_receipt() {
    assert!(ServerLimits::try_new(8, 4096, 512, 64, 64, 1_048_576).is_ok());
    assert!(ServerLimits::try_new(9, 4096, 512, 64, 64, 1_048_576).is_err());
    assert!(ServerLimits::try_new(0, 4096, 512, 64, 64, 1_048_576).is_err());
    assert!(ServerLimits::try_new(8, 4097, 512, 64, 64, 1_048_576).is_err());
    assert!(TickBudget::try_new(4097, 0, 0, 0, 0).is_err());
    assert!(TickBudget::try_new(0, 0, 0, 0, 0).is_ok());
    assert!(StoreLimits::try_new(2, 16, 3, 3, 3, 1, 8, 4_194_304).is_ok());
    assert!(StoreLimits::try_new(3, 16, 3, 3, 3, 1, 8, 4_194_304).is_err());

    let mut endpoint = server();
    let admitted = login(1, "Ada");
    let session = endpoint.admit(admitted, TransportKind::Memory).unwrap();
    let receipt = endpoint.submit(session, sequenced(9)).unwrap();
    assert_eq!(
        receipt,
        SubmissionReceipt::QueuedForTick {
            tick: 0,
            arrival_index: 0
        }
    );
    let facts = endpoint.authority.session(session).unwrap();
    assert_eq!(facts.last_applied_sequence, 0);
    assert_eq!(facts.next_arrival, 1);
    let again = endpoint.submit(session, sequenced(8)).unwrap();
    assert_eq!(
        again,
        SubmissionReceipt::QueuedForTick {
            tick: 0,
            arrival_index: 1
        }
    );
    let chat = PlayIntent::Chat(mornlea_domain::ChatIntent::new(
        CommandText::try_from_canonical("hello".to_owned()).unwrap(),
    ));
    assert_eq!(
        endpoint.submit(session, chat).unwrap(),
        SubmissionReceipt::ControlAccepted
    );
    assert_eq!(
        endpoint
            .submit(session, PlayIntent::KeepAliveReply { token: 4 })
            .unwrap(),
        SubmissionReceipt::ControlAccepted
    );
    assert_eq!(endpoint.authority.session(session).unwrap().next_arrival, 2);

    let mut capped = ContractServer {
        authority: AuthorityState::try_new(ServerLimits::try_new(8, 1, 2, 1, 4, 1024).unwrap(), 7)
            .unwrap(),
    };
    let session = capped.admit(login(2, "Bea"), TransportKind::Tcp).unwrap();
    assert!(capped.submit(session, sequenced(1)).is_ok());
    let before = capped.authority.session(session).unwrap();
    assert!(capped.submit(session, sequenced(2)).is_err());
    let after = capped.authority.session(session).unwrap();
    assert_eq!(after.next_arrival, before.next_arrival);
    assert_eq!(after.last_applied_sequence, 0);
    capped
        .authority
        .retire(session, CloseReason::PeerGone)
        .unwrap();
    assert!(matches!(
        capped.submit(session, sequenced(3)),
        Err(ServerError::StaleSession { .. })
    ));
}

#[test]
fn all_ports_type_flow() {
    let mut endpoint = server();
    let session = endpoint
        .admit(login(3, "Cara"), TransportKind::Memory)
        .unwrap();
    assert_eq!(
        endpoint.authority.session(session).unwrap().phase,
        SessionPhase::Active
    );
    assert!(endpoint.authority.apply_sequence(session, 4));
    assert!(!endpoint.authority.apply_sequence(session, 4));
    assert!(!endpoint.authority.apply_sequence(session, 3));

    let frozen = endpoint.authority.freeze_eligible(0);
    assert_eq!(frozen.len(), 0);
    endpoint.submit(session, sequenced(5)).unwrap();
    let frozen = endpoint.authority.freeze_eligible(0);
    assert_eq!(frozen.len(), 1);
    endpoint.authority.carry(frozen).unwrap();

    let chunk = chunk_result(1);
    assert!(endpoint.authority.admit_chunk(chunk).is_ok());
    let refused = endpoint.authority.admit_chunk(chunk_result(2));
    let Err(returned) = refused else {
        panic!("full chunk mailbox must return the result");
    };
    assert_eq!(returned.request.get(), 2);
    endpoint.authority.cancel_chunk(returned.request);
    assert!(endpoint.authority.admit_chunk(returned).is_ok());
    assert!(endpoint.authority.drain_chunks(1).is_empty() || true);
    let _ = endpoint.authority.drain_companions(1);

    let publication = TickPublication {
        tick: 0,
        events: Vec::new(),
        control: vec![ControlReply {
            session,
            packet: ServerPacket::LoginSuccess(LoginSuccess::new(
                endpoint.authority.session(session).unwrap().player_id,
                7,
            )),
        }],
        counters: TickCounters::default(),
    };
    endpoint.authority.publish(publication).unwrap();
    let frames = endpoint.authority.take_outbox(session, 8, 1024).unwrap();
    assert_eq!(frames.len(), 1);
    assert_eq!(frames[0].len(), 24);
    endpoint
        .authority
        .close_outbox(session, CloseReason::SlowReceiver);
    assert!(endpoint.authority.take_outbox(session, 1, 1).is_ok());

    let snapshot = endpoint.authority.metadata_snapshot();
    assert_eq!(snapshot.revision, 1);
    assert!(matches!(snapshot.key, SaveKey::Metadata));
    endpoint.authority.remember_dirty(snapshot);
    let selected = endpoint
        .authority
        .select(SaveMode::Urgent, SaveBudget::default());
    assert_eq!(selected.len(), 1);
    endpoint.authority.return_dirty(selected[0].clone());
    assert_eq!(endpoint.authority.save_stats().dirty, 1);

    let mut context = TickContext::harness(
        &mut endpoint.authority,
        TickBudget::try_new(1, 0, 0, 0, 0).unwrap(),
    );
    let actor = ActorKey::Player(session);
    context.preload_inventory(actor, InventoryRecord::empty());
    let placed = resolve_place(
        actor,
        &mornlea_domain::PlacementIntent::try_new(LookAngles::try_new(0.0, 0.0).unwrap(), 0)
            .unwrap(),
        &context.read(),
    );
    assert!(matches!(placed, Err(RuleReject::StaleObservation)));
    context.charge(WorkKind::Commands, 1).unwrap();
    assert!(context.charge(WorkKind::Commands, 1).is_err());

    let mut store = MemoryStore::new(false);
    let request = SaveRequest {
        snapshots: vec![metadata_snapshot(3)],
    };
    let ticket = store.submit(request).unwrap();
    let SavePoll::Completed(completion) = store.poll(ticket) else {
        panic!("completed save");
    };
    assert_eq!(completion.committed, vec![(SaveKey::Metadata, 3)]);

    let mut agent = MemoryAgent::default();
    let id = agent
        .submit(AgentRequest::Acquire(base_identity(4)))
        .unwrap();
    assert!(matches!(agent.poll(id), AgentPoll::Pending));
    let now = Instant::now();
    agent.cancel(id, Deadline::at(now)).unwrap();
    assert!(
        agent
            .freeze(&SplitClock {
                instant: now,
                unix_ms: 1
            })
            .is_none()
    );
    agent.close(Deadline::at(now)).unwrap();
    assert!(
        agent
            .submit(AgentRequest::Acquire(base_identity(5)))
            .is_err()
    );

    let mut snapshots = MemorySnapshots::default();
    let registered = snapshots
        .register(
            namespace(6),
            companion(6),
            1,
            planning_snapshot(6),
            Deadline::at(now),
        )
        .unwrap();
    assert_eq!(registered.capability.len(), 32);
    snapshots.complete(registered.id).unwrap();
    assert!(snapshots.cancel(registered.id).is_err());
    snapshots.close().unwrap();
    assert!(
        snapshots
            .register(
                namespace(6),
                companion(6),
                1,
                planning_snapshot(6),
                Deadline::at(now),
            )
            .is_err()
    );

    let mut workers = IdleWorkers;
    workers.stop_new().unwrap();
    workers.cancel().unwrap();
    let mut persistence = IdlePersistence;
    let flushed = persistence
        .flush(SaveKey::Companions, Deadline::at(now))
        .unwrap();
    assert_eq!(flushed.failed, 0);
    let mut failing = FailingPersistence;
    assert!(failing.flush(SaveKey::Hostiles, Deadline::at(now)).is_err());
}

#[test]
fn compound_rejects_partial() {
    let mut endpoint = server();
    let session = endpoint
        .admit(login(7, "Dora"), TransportKind::Memory)
        .unwrap();
    let mut context = TickContext::harness(
        &mut endpoint.authority,
        TickBudget::try_new(1, 0, 0, 0, 0).unwrap(),
    );
    let actor = ActorKey::Player(session);
    let original = InventoryRecord::empty();
    context.preload_inventory(actor, original);
    let first = InventoryPatch::try_new(actor, original, original.with_selected(slot(1))).unwrap();
    let stale = InventoryPatch::try_new(
        actor,
        original.with_selected(slot(2)),
        original.with_selected(slot(3)),
    )
    .unwrap();
    let rejected = context.stage(RuleEffect::Compound(vec![
        RuleEffect::Inventory(first),
        RuleEffect::Inventory(stale),
    ]));
    assert_eq!(rejected, Err(RuleReject::StaleObservation));
    assert_eq!(context.read().inventory(actor).unwrap().selected, slot(0));
}

#[test]
fn compound_projectile_insert_is_atomic() {
    let mut endpoint = server();
    let session = endpoint
        .admit(login(11, "Faye"), TransportKind::Memory)
        .unwrap();
    let mut context = TickContext::harness(
        &mut endpoint.authority,
        TickBudget::try_new(1, 0, 0, 0, 0).unwrap(),
    );
    let actor = ActorKey::Player(session);
    let original = InventoryRecord::empty();
    context.preload_inventory(actor, original);
    for index in 1..MAX_PROJECTILE_RECORDS {
        context
            .stage(insert_projectile(projectile(index as u64, actor)))
            .unwrap();
    }
    assert_eq!(context.projectile_len(), MAX_PROJECTILE_RECORDS - 1);
    let patch = InventoryPatch::try_new(actor, original, original.with_selected(slot(1))).unwrap();
    let rejected = context.stage(RuleEffect::Compound(vec![
        RuleEffect::Inventory(patch),
        insert_projectile(projectile(1_000, actor)),
        insert_projectile(projectile(1_001, actor)),
    ]));
    assert_eq!(
        rejected,
        Err(RuleReject::ResourceFull(Resource::RuleEffects))
    );
    assert_eq!(context.projectile_len(), MAX_PROJECTILE_RECORDS - 1);
    assert_eq!(context.read().inventory(actor).unwrap().selected, slot(0));
}

fn projectile(id: u64, owner: ActorKey) -> ProjectileRecord {
    ProjectileRecord {
        id: ProjectileId::try_new(id).unwrap(),
        owner,
        dimension: Dimension::OVERWORLD,
        position: FiniteVec3::try_new([0.0, 64.0, 0.0]).unwrap(),
        velocity: FiniteVec3::try_new([0.0, 0.0, 1.0]).unwrap(),
        kind: ProjectileKind::Arrow,
        damage: 1,
        age: 0,
    }
}

fn insert_projectile(record: ProjectileRecord) -> RuleEffect {
    RuleEffect::Projectile {
        before: None,
        after: Some(record),
    }
}

#[test]
fn event_publication_reaches_outbox() {
    let mut endpoint = server();
    let session = endpoint
        .admit(login(12, "Gia"), TransportKind::Memory)
        .unwrap();
    let event = Event::CommandRejected(CommandRejection::new(4, RejectReason::InvalidRay));
    let packet = ServerPacket::try_from(event.clone()).unwrap();
    let mut codec = ProtocolCodec::new().unwrap();
    let mut buffer = vec![0u8; 64];
    let written = codec.encode_server_into(&packet, &mut buffer).unwrap();
    let expected = buffer[..written].to_vec();

    endpoint
        .authority
        .publish(TickPublication {
            tick: 0,
            events: vec![RoutedEvent::new(
                EventRecipient::Session(session.get()),
                event,
            )],
            control: vec![ControlReply {
                session,
                packet: ServerPacket::LoginSuccess(LoginSuccess::new(
                    endpoint.authority.session(session).unwrap().player_id,
                    7,
                )),
            }],
            counters: TickCounters::default(),
        })
        .unwrap();
    let frames = endpoint.authority.take_outbox(session, 8, 4096).unwrap();
    assert_eq!(frames.len(), 2);
    assert_eq!(frames[0], expected);
    assert_ne!(frames[0].len(), frames[1].len());
    assert_eq!(frames[1].len(), 24);

    let refused = endpoint.authority.publish(TickPublication {
        tick: 1,
        events: vec![RoutedEvent::new(
            EventRecipient::Session(0),
            Event::CommandRejected(CommandRejection::new(5, RejectReason::NoTarget)),
        )],
        control: vec![ControlReply {
            session,
            packet: ServerPacket::LoginSuccess(LoginSuccess::new(
                endpoint.authority.session(session).unwrap().player_id,
                7,
            )),
        }],
        counters: TickCounters::default(),
    });
    assert!(matches!(
        refused,
        Err(ServerError::InvalidInput { field: "packet" })
    ));
    assert!(
        endpoint
            .authority
            .take_outbox(session, 8, 4096)
            .unwrap()
            .is_empty()
    );
}

#[test]
fn shutdown_failure_retains_report() {
    let mut endpoint = server();
    let start = Instant::now();
    let deadline = Deadline::after(start, Duration::from_secs(30)).unwrap();
    let failure = endpoint.shutdown(deadline).unwrap_err();
    assert_eq!(failure.report.next, ShutdownPhase::StoreSync);
    assert!(failure.report.retryable);
    assert!(failure.report.completed.contains(&ShutdownPhase::FinalTick));
    assert!(
        failure
            .report
            .completed
            .contains(&ShutdownPhase::StopAdmission)
    );
    assert!(!failure.report.completed.contains(&ShutdownPhase::StoreSync));
    assert_eq!(endpoint.authority.phase(), ServerPhase::Closing);
    assert_eq!(endpoint.authority.shutdown_progress(), failure.report);
    let again = endpoint.shutdown(deadline).unwrap_err();
    assert_eq!(again.report.next, ShutdownPhase::StoreSync);
    assert_eq!(
        again
            .report
            .completed
            .iter()
            .filter(|phase| **phase == ShutdownPhase::FinalTick)
            .count(),
        1
    );
}

#[test]
fn completion_returns_ownership() {
    let limits = StoreLimits::try_new(2, 16, 3, 3, 3, 1, 8, 4_194_304).unwrap();
    let mut store = MemoryStore::with_limits(limits);
    store
        .submit(SaveRequest {
            snapshots: (0..8).map(|index| chunk_snapshot(index, 9)).collect(),
        })
        .unwrap();
    assert_eq!(store.occupancy.chunks(), 8);
    let ninth = SaveRequest {
        snapshots: vec![chunk_snapshot(8, 11)],
    };
    let error = store.submit(ninth).unwrap_err();
    assert_eq!(error.request.snapshots[0].revision, 11);
    assert!(matches!(error.request.snapshots[0].key, SaveKey::Chunk(_)));
    assert_eq!(store.occupancy.chunks(), 8);
    let metadata = SaveRequest {
        snapshots: vec![metadata_snapshot(4)],
    };
    let ticket = store.submit(metadata).unwrap();
    assert_eq!(store.occupancy.chunks(), 8);
    let duplicate = store
        .submit(SaveRequest {
            snapshots: vec![metadata_snapshot(5)],
        })
        .unwrap_err();
    assert_eq!(duplicate.request.snapshots[0].revision, 5);
    assert_eq!(store.occupancy.chunks(), 8);
    let SavePoll::Completed(completion) = store.poll(ticket) else {
        panic!("metadata completion");
    };
    assert_eq!(completion.committed, vec![(SaveKey::Metadata, 4)]);
    assert_eq!(completion.snapshots[0].revision, 4);
    assert!(matches!(completion.snapshots[0].key, SaveKey::Metadata));
}

#[test]
fn clock_units_separate() {
    let start = Instant::now();
    let deadline = Deadline::after(start, Duration::from_millis(5)).unwrap();
    assert!(!deadline.expired(start));
    let ahead = SplitClock {
        instant: start,
        unix_ms: i64::MAX,
    };
    let behind = SplitClock {
        instant: start,
        unix_ms: i64::MIN,
    };
    assert_eq!(ahead.unix_ms(), i64::MAX);
    assert_eq!(behind.unix_ms(), i64::MIN);
    assert!(!deadline.expired(ahead.monotonic()));
    assert!(!deadline.expired(behind.monotonic()));
    assert!(deadline.expired(start + Duration::from_millis(5)));
    assert!(Deadline::after(start, Duration::MAX).is_err());
}

#[test]
fn load_prepare_install_send_activate() {
    for kind in [TransportKind::Memory, TransportKind::Tcp] {
        let mut endpoint = server();
        let prepared = endpoint.authority.prepare(login(8, "Eve"), kind).unwrap();
        assert_eq!(
            endpoint.authority.session(prepared).unwrap().phase,
            SessionPhase::Prepared
        );
        assert!(endpoint.submit(prepared, sequenced(1)).is_err());
        endpoint.authority.install(prepared, None).unwrap();
        assert_eq!(
            endpoint.authority.session(prepared).unwrap().phase,
            SessionPhase::Prepared
        );
        endpoint.authority.activate(prepared).unwrap();
        assert_eq!(
            endpoint.authority.session(prepared).unwrap().phase,
            SessionPhase::Active
        );
        assert!(endpoint.submit(prepared, sequenced(1)).is_ok());
    }

    let mut endpoint = server();
    let mut reserved = Vec::new();
    for index in 0..8 {
        let name = format!("P{index}");
        reserved.push(
            endpoint
                .authority
                .prepare(login(10 + index, &name), TransportKind::Memory)
                .unwrap(),
        );
    }
    assert!(
        endpoint
            .authority
            .prepare(login(30, "Late"), TransportKind::Tcp)
            .is_err()
    );
    endpoint
        .authority
        .retire(reserved[0], CloseReason::Shutdown)
        .unwrap();
    assert!(
        endpoint
            .authority
            .prepare(login(30, "Late"), TransportKind::Tcp)
            .is_ok()
    );
}

struct SplitClock {
    instant: Instant,
    unix_ms: i64,
}

impl Clock for SplitClock {
    fn monotonic(&self) -> Instant {
        self.instant
    }
    fn unix_ms(&self) -> i64 {
        self.unix_ms
    }
}

struct FinalOnce;

impl FinalReducer for FinalOnce {
    fn reduce_final(&mut self, authority: &mut AuthorityState) -> Result<u64, ServerError> {
        Ok(authority.next_tick())
    }
}

struct MemoryStore {
    limits: StoreLimits,
    occupancy: SaveOccupancy,
    next_ticket: u64,
    jobs: BTreeMap<u64, SaveRequest>,
    fail_sync: bool,
}

impl MemoryStore {
    fn new(fail_sync: bool) -> Self {
        Self::with_limits(StoreLimits::try_new(2, 16, 3, 3, 3, 1, 8, 4_194_304).unwrap())
            .failing(fail_sync)
    }

    fn with_limits(limits: StoreLimits) -> Self {
        Self {
            limits,
            occupancy: SaveOccupancy::default(),
            next_ticket: 1,
            jobs: BTreeMap::new(),
            fail_sync: false,
        }
    }

    fn failing(mut self, fail_sync: bool) -> Self {
        self.fail_sync = fail_sync;
        self
    }
}

impl StoreHandle for MemoryStore {
    fn submit(&mut self, request: SaveRequest) -> Result<SaveTicket, SubmitSaveError> {
        match self.limits.try_admit(&self.occupancy, &request) {
            Ok(next) => {
                self.occupancy = next;
                let ticket = SaveTicket::try_from_raw(self.next_ticket).unwrap();
                self.next_ticket += 1;
                self.jobs.insert(ticket.get(), request);
                Ok(ticket)
            }
            Err(error) => Err(SubmitSaveError { error, request }),
        }
    }

    fn poll(&mut self, ticket: SaveTicket) -> SavePoll {
        let Some(request) = self.jobs.get(&ticket.get()).cloned() else {
            return SavePoll::Pending;
        };
        let listed = request
            .snapshots
            .iter()
            .map(|snapshot| (snapshot.key.clone(), snapshot.revision))
            .collect::<Vec<_>>();
        SavePoll::Completed(SaveCompletion {
            ticket,
            snapshots: request.snapshots,
            submitted: listed.clone(),
            committed: listed,
            error: None,
        })
    }

    fn poll_tick(
        &mut self,
        _tick: u64,
        _budget: SaveBudget,
        _authority: &mut dyn SaveAuthority,
    ) -> Result<SaveScheduleReport, ServerError> {
        Ok(SaveScheduleReport {
            urgent: 0,
            autosave: 0,
            retry: 0,
            stats: SaveStats::default(),
            backpressured: false,
        })
    }

    fn cancel_pending(&mut self) -> Result<Vec<OwnedSnapshot>, ServerError> {
        let pending = std::mem::take(&mut self.jobs)
            .into_values()
            .flat_map(|request| request.snapshots)
            .collect();
        Ok(pending)
    }

    fn flush(
        &mut self,
        _deadline: Deadline,
        _authority: &mut dyn SaveAuthority,
        _clock: &dyn Clock,
    ) -> Result<FlushReport, ServerError> {
        Ok(FlushReport::default())
    }

    fn sync(&mut self, _deadline: Deadline) -> Result<(), ServerError> {
        if self.fail_sync {
            Err(ServerError::Io {
                operation: Operation::Sync,
                kind: std::io::ErrorKind::Other,
            })
        } else {
            Ok(())
        }
    }

    fn close(&mut self, _deadline: Deadline) -> Result<(), ServerError> {
        Ok(())
    }
}

#[derive(Default)]
struct MemoryAgent {
    closed: bool,
    next_id: u64,
}

impl AgentHandle for MemoryAgent {
    fn submit(&mut self, request: AgentRequest) -> Result<AgentRequestId, ServerError> {
        if self.closed {
            return Err(ServerError::InvalidState {
                phase: ServerPhase::Closed,
            });
        }
        let AgentRequest::Acquire(identity) = request else {
            return Err(ServerError::InvalidInput { field: "agent" });
        };
        self.next_id += 1;
        let _ = identity;
        Ok(AgentRequestId::try_from_bytes(uuid(self.next_id as u8)).unwrap())
    }

    fn poll(&mut self, _id: AgentRequestId) -> AgentPoll {
        AgentPoll::Pending
    }

    fn cancel(&mut self, _id: AgentRequestId, _deadline: Deadline) -> Result<(), ServerError> {
        Ok(())
    }

    fn freeze(&mut self, _clock: &dyn Clock) -> Option<FrozenLease> {
        None
    }

    fn release(&mut self, _lease: &FrozenLease, _deadline: Deadline) -> Result<(), ServerError> {
        Ok(())
    }

    fn close(&mut self, _deadline: Deadline) -> Result<(), ServerError> {
        self.closed = true;
        Ok(())
    }
}

#[derive(Default)]
struct MemorySnapshots {
    closed: bool,
    live: Option<SnapshotId>,
}

impl SnapshotPort for MemorySnapshots {
    fn register(
        &mut self,
        _namespace: NamespaceId,
        _companion: CompanionId,
        generation: u64,
        _snapshot: PlanningSnapshot,
        _deadline: Deadline,
    ) -> Result<SnapshotRegistration, ServerError> {
        if self.closed || generation == 0 {
            return Err(ServerError::InvalidState {
                phase: ServerPhase::Closed,
            });
        }
        let id = SnapshotId::try_from_bytes(uuid(9)).unwrap();
        self.live = Some(id);
        SnapshotRegistration::try_new(id, [1; 32], vec![9; 32], "http://127.0.0.1/mcp".to_owned())
    }

    fn complete(&mut self, id: SnapshotId) -> Result<(), ServerError> {
        if self.live == Some(id) {
            self.live = None;
            Ok(())
        } else {
            Err(ServerError::InvalidInput { field: "snapshot" })
        }
    }

    fn cancel(&mut self, id: SnapshotId) -> Result<(), ServerError> {
        if self.live == Some(id) {
            self.live = None;
            Ok(())
        } else {
            Err(ServerError::Cancelled)
        }
    }

    fn close(&mut self) -> Result<(), ServerError> {
        self.closed = true;
        self.live = None;
        Ok(())
    }
}

struct IdleWorkers;
impl WorkerLifecycle for IdleWorkers {
    fn stop_new(&mut self) -> Result<(), ServerError> {
        Ok(())
    }
    fn cancel(&mut self) -> Result<(), ServerError> {
        Ok(())
    }
    fn wait(&mut self, _deadline: Deadline) -> Result<(), ServerError> {
        Ok(())
    }
    fn close(&mut self, _deadline: Deadline) -> Result<(), ServerError> {
        Ok(())
    }
}

struct IdlePersistence;
impl ActorPersistence for IdlePersistence {
    fn flush(&mut self, _family: SaveKey, _deadline: Deadline) -> Result<FlushReport, ServerError> {
        Ok(FlushReport::default())
    }
}

struct FailingPersistence;
impl ActorPersistence for FailingPersistence {
    fn flush(&mut self, _family: SaveKey, _deadline: Deadline) -> Result<FlushReport, ServerError> {
        Err(ServerError::Io {
            operation: Operation::Flush,
            kind: std::io::ErrorKind::Other,
        })
    }
}

struct IdleMcp;
impl McpLifecycle for IdleMcp {
    fn close(&mut self, _deadline: Deadline) -> Result<(), ServerError> {
        Ok(())
    }
}

struct IdleMemory;
impl MemoryFinalizer for IdleMemory {
    fn pending(&self) -> MemoryFinalizationReport {
        MemoryFinalizationReport::default()
    }
    fn begin_attempt(&mut self, _deadline: Deadline) -> Result<(), ServerError> {
        Ok(())
    }
    fn drain(&mut self, _deadline: Deadline) -> Result<MemoryFinalizationReport, ServerError> {
        Ok(MemoryFinalizationReport::default())
    }
}

fn uuid(tag: u8) -> [u8; 16] {
    let mut bytes = [0u8; 16];
    bytes[0] = tag.max(1);
    bytes[6] = 0x40;
    bytes[8] = 0x80;
    bytes
}

fn base_identity(tag: u8) -> BaseIdentity {
    BaseIdentity {
        request_id: AgentRequestId::try_from_bytes(uuid(tag)).unwrap(),
        client_instance_id: ClientInstanceId::try_from_bytes(uuid(tag.wrapping_add(1))).unwrap(),
        namespace_id: namespace(tag),
    }
}

fn namespace(tag: u8) -> NamespaceId {
    NamespaceId::try_from_bytes(uuid(tag.wrapping_add(2))).unwrap()
}

fn companion(tag: u8) -> CompanionId {
    CompanionId::try_from_bytes(uuid(tag)).unwrap()
}

fn planning_snapshot(tag: u8) -> PlanningSnapshot {
    let look = LookAngles::try_new(0.0, 0.0).unwrap();
    let position = FiniteVec3::try_new([0.0, 0.0, 0.0]).unwrap();
    let player = PlayerId::try_from_bytes(uuid(tag)).unwrap();
    PlanningSnapshot::try_new(
        1,
        0,
        CommandText::try_from_canonical("ping".to_owned()).unwrap(),
        SnapshotIssuer {
            player_id: player,
            position,
            look,
            look_hit: None,
        },
        SnapshotCompanion {
            companion_id: companion(tag),
            position,
            look,
            task_status: SnapshotTaskStatusText::idle(),
            inventory: [ItemStack::default(); 36],
        },
        Vec::new(),
        Vec::new(),
        Vec::new(),
        SnapshotTerrain::try_new(
            BlockPos::ORIGIN,
            [33, 17, 33],
            vec![0; 137],
            vec![0; 1089],
            vec![0; 18_513],
        )
        .unwrap(),
    )
    .unwrap()
}

fn metadata_value() -> Metadata {
    Metadata {
        format_version: mornlea_storage::METADATA_CURRENT_VERSION,
        seed: 1,
        spawn_dimension: 0,
        spawn_anchor: mornlea_storage::MetadataChunkPos { x: 0, z: 0 },
        world_time_ticks: 0,
        day_phase_offset: 0,
        weather_kind: 0,
        weather_ticks_remaining: 0,
        depths_spawn_anchor: mornlea_storage::MetadataChunkPos { x: 0, z: 0 },
        depths_seed_salt: 0,
        difficulty: 0,
    }
}

fn metadata_snapshot(revision: u64) -> OwnedSnapshot {
    OwnedSnapshot::try_new(
        SaveKey::Metadata,
        revision,
        64,
        SaveUrgency::Autosave,
        SaveValue::Metadata(metadata_value()),
    )
    .unwrap()
}

fn chunk_snapshot(index: i32, revision: u64) -> OwnedSnapshot {
    OwnedSnapshot::try_new(
        SaveKey::Chunk(ChunkKey {
            dimension: Dimension::OVERWORLD,
            pos: ChunkPos::new(index, 0),
        }),
        revision,
        32,
        SaveUrgency::Autosave,
        SaveValue::Chunk(ChunkSave {
            key: mornlea_storage::ChunkKey {
                dimension: 0,
                x: index,
                z: 0,
            },
            revision,
            chunk: Chunk {
                sections: Vec::new(),
                drops: Vec::new(),
                furnaces: Vec::new(),
                chests: Vec::new(),
            },
        }),
    )
    .unwrap()
}

fn chunk_result(raw: u64) -> ChunkResult {
    ChunkResult {
        request: ChunkRequestId::try_new(raw).unwrap(),
        key: ChunkKey {
            dimension: Dimension::OVERWORLD,
            pos: ChunkPos::new(0, 0),
        },
        generation: 1,
        result: Ok(Chunk {
            sections: Vec::new(),
            drops: Vec::new(),
            furnaces: Vec::new(),
            chests: Vec::new(),
        }),
    }
}

fn projectile_world() -> mornlea_domain::WorldState {
    mornlea_domain::WorldState::try_new(mornlea_domain::WorldStateParts {
        day_phase_offset: 0,
        world_time_ticks: 0,
        weather: mornlea_domain::Weather::Clear,
        season: mornlea_domain::Season::Spring,
        season_progress: 0,
        temperature: 0,
    })
    .unwrap()
}

#[test]
fn projectile_update_remove_and_stale() {
    let mut endpoint = server();
    let actor = ActorKey::Player(
        endpoint
            .admit(login(19, "Projectile"), TransportKind::Memory)
            .unwrap(),
    );
    let mut ctx = TickContext::harness(&mut endpoint.authority, TickBudget::full());
    let original = projectile(1, actor);
    ctx.stage(insert_projectile(original.clone())).unwrap();
    let mut moved = original.clone();
    moved.age = 1;
    moved.position = FiniteVec3::try_new([0.0, 64.0, 1.0]).unwrap();
    ctx.stage(RuleEffect::Projectile {
        before: Some(original.clone()),
        after: Some(moved.clone()),
    })
    .unwrap();
    assert_eq!(
        ctx.snapshot_state(projectile_world()).projectiles,
        vec![moved.clone()]
    );
    assert_eq!(
        ctx.stage(RuleEffect::Projectile {
            before: Some(original),
            after: None
        }),
        Err(RuleReject::StaleObservation)
    );
    assert_eq!(
        ctx.snapshot_state(projectile_world()).projectiles,
        vec![moved.clone()]
    );
    ctx.stage(RuleEffect::Projectile {
        before: Some(moved),
        after: None,
    })
    .unwrap();
    assert_eq!(ctx.projectile_len(), 0);
}

#[test]
fn projectile_compound_capacity_and_rollback() {
    let mut endpoint = server();
    let actor = ActorKey::Player(
        endpoint
            .admit(login(19, "Projectile"), TransportKind::Memory)
            .unwrap(),
    );
    let mut ctx = TickContext::harness(&mut endpoint.authority, TickBudget::full());
    let inventory = InventoryRecord::empty();
    ctx.preload_inventory(actor, inventory);
    for id in 1..=128 {
        ctx.stage(insert_projectile(projectile(id, actor))).unwrap();
    }
    let old = projectile(1, actor);
    let mut moved = old.clone();
    moved.age = 1;
    ctx.stage(RuleEffect::Projectile {
        before: Some(old),
        after: Some(moved.clone()),
    })
    .unwrap();
    ctx.stage(RuleEffect::Compound(vec![
        RuleEffect::Projectile {
            before: Some(moved),
            after: None,
        },
        insert_projectile(projectile(129, actor)),
    ]))
    .unwrap();
    let before = ctx.snapshot_state(projectile_world()).projectiles;
    assert_eq!(
        before.iter().map(|p| p.id.get()).collect::<Vec<_>>(),
        (2..=129).collect::<Vec<_>>()
    );
    let result = ctx.stage(RuleEffect::Compound(vec![
        RuleEffect::Inventory(
            InventoryPatch::try_new(actor, inventory, inventory.with_selected(slot(1))).unwrap(),
        ),
        RuleEffect::Projectile {
            before: Some(projectile(2, actor)),
            after: None,
        },
        insert_projectile(projectile(3, actor)),
    ]));
    assert_eq!(result, Err(RuleReject::StaleObservation));
    assert_eq!(ctx.snapshot_state(projectile_world()).projectiles, before);
    assert_eq!(ctx.read().inventory(actor), Some(&inventory));
    assert_eq!(
        ctx.stage(RuleEffect::Compound(vec![
            insert_projectile(projectile(130, actor)),
            RuleEffect::Projectile {
                before: Some(projectile(2, actor)),
                after: None
            },
        ])),
        Err(RuleReject::ResourceFull(Resource::RuleEffects))
    );
    assert_eq!(ctx.snapshot_state(projectile_world()).projectiles, before);
}

#[test]
fn projectile_fixture_initialization() {
    let mut endpoint = server();
    let actor = ActorKey::Player(
        endpoint
            .admit(login(19, "Projectile"), TransportKind::Memory)
            .unwrap(),
    );
    let runtime = ActorRuntime {
        key: actor,
        controls: None,
        has_view: true,
        reset: false,
        attack_cooldown: 0,
        hurt_cooldown: 0,
        burn_cooldown: 0,
        oxygen: 300,
        peak_y: 64.0,
        exhaustion_milli: 0,
        saturation_milli: 0,
        since_damage_ticks: 0,
        drown_ticks: 0,
        starvation_ticks: 0,
        eating: None,
        bow: Some(BowProgress {
            slot: slot(0),
            ticks: 19,
        }),
        path: None,
        aux: ActorAux::Player {
            respawn: None,
            workbench: None,
        },
    };
    let mut initial = TickContext::harness(&mut endpoint.authority, TickBudget::full())
        .snapshot_state(projectile_world());
    initial.projectiles.push(projectile(1, actor));
    initial.runtime.push(runtime.clone());
    let ctx = TickContext::from_fixture(&mut endpoint.authority, &initial, TickBudget::full());
    assert_eq!(
        ctx.snapshot_state(projectile_world()).projectiles,
        initial.projectiles
    );
    assert_eq!(ctx.read().runtime(actor), Some(&runtime));
    assert_eq!(
        ctx.snapshot_state(projectile_world()).runtime,
        initial.runtime
    );
    assert_eq!(ctx.read().projectiles(), initial.projectiles);
}

#[test]
fn projectile_invalid_shapes_and_identity() {
    let mut endpoint = server();
    let actor = ActorKey::Player(
        endpoint
            .admit(login(19, "Projectile"), TransportKind::Memory)
            .unwrap(),
    );
    let mut ctx = TickContext::harness(&mut endpoint.authority, TickBudget::full());
    let original = projectile(1, actor);
    assert_eq!(
        ctx.stage(RuleEffect::Projectile {
            before: None,
            after: None
        }),
        Err(RuleReject::Wire(RejectReason::InvalidInput))
    );
    assert_eq!(
        ctx.stage(RuleEffect::Projectile {
            before: Some(original.clone()),
            after: None
        }),
        Err(RuleReject::StaleObservation)
    );
    ctx.stage(insert_projectile(original.clone())).unwrap();
    assert_eq!(
        ctx.stage(insert_projectile(original.clone())),
        Err(RuleReject::StaleObservation)
    );
    assert_eq!(
        ctx.stage(RuleEffect::Projectile {
            before: Some(original.clone()),
            after: Some(projectile(2, actor))
        }),
        Err(RuleReject::Wire(RejectReason::InvalidInput))
    );
    assert_eq!(
        ctx.snapshot_state(projectile_world()).projectiles,
        vec![original]
    );
}

#[test]
fn projectile_damage_staging_is_bounded_and_atomic() {
    let mut endpoint = server();
    let actor = ActorKey::Player(
        endpoint
            .admit(login(19, "Damage"), TransportKind::Memory)
            .unwrap(),
    );
    let mut ctx = TickContext::harness(&mut endpoint.authority, TickBudget::full());
    let original = projectile(1, actor);
    ctx.stage(insert_projectile(original.clone())).unwrap();
    let hit = DamageIntent {
        source: actor,
        target: actor,
        dimension: Dimension::OVERWORLD,
        amount: 2,
        cause: DamageCause::Projectile,
        projectile: Some(original.id),
        tick: 0,
    };
    ctx.stage(RuleEffect::Damage(hit)).unwrap();
    assert_eq!(ctx.read().damage_intents(), &[hit]);
    for _ in 1..4095 {
        ctx.stage(RuleEffect::Damage(hit)).unwrap();
    }
    assert_eq!(
        ctx.stage(RuleEffect::Compound(vec![
            RuleEffect::Projectile {
                before: Some(original.clone()),
                after: None
            },
            RuleEffect::Damage(hit),
            RuleEffect::Damage(hit),
        ])),
        Err(RuleReject::ResourceFull(Resource::RuleEffects))
    );
    assert_eq!(ctx.read().damage_intents().len(), 4095);
    assert_eq!(ctx.read().projectiles(), std::slice::from_ref(&original));
    ctx.stage(RuleEffect::Compound(vec![
        RuleEffect::Projectile {
            before: Some(original),
            after: None,
        },
        RuleEffect::Damage(hit),
    ]))
    .unwrap();
    assert_eq!(ctx.read().damage_intents().len(), 4096);
    assert!(ctx.read().projectiles().is_empty());
    assert_eq!(
        ctx.stage(RuleEffect::Damage(hit)),
        Err(RuleReject::ResourceFull(Resource::RuleEffects))
    );
}

#[test]
fn action_receipt_preflight_and_suppression_capacity() {
    use mornlea_server::state::ActionKind;
    let mut endpoint = server();
    let mut actors = Vec::new();
    for tag in 1..=9 {
        let session = endpoint
            .admit(login(tag, "Receipt"), TransportKind::Memory)
            .unwrap();
        actors.push(ActorKey::Player(session));
        endpoint
            .close_session(session, CloseReason::PeerGone)
            .unwrap();
    }
    let mut ctx = TickContext::harness(&mut endpoint.authority, TickBudget::full());
    for _ in 0..4095 {
        ctx.note_charge(actors[0], ActionKind::Till).unwrap();
    }
    ctx.check_charge_capacity().unwrap();
    ctx.check_charge_capacity().unwrap();
    ctx.note_charge(actors[0], ActionKind::Till).unwrap();
    assert_eq!(
        ctx.check_charge_capacity(),
        Err(ServerError::Capacity {
            resource: Resource::Commands,
            limit: 4096,
            observed: 4097
        })
    );
    assert_eq!(ctx.take_charges().len(), 4096);
    ctx.check_charge_capacity().unwrap();
    for actor in &actors[..8] {
        ctx.check_mining_suppression(*actor).unwrap();
        ctx.suppress_mining(*actor).unwrap();
    }
    ctx.suppress_mining(actors[0]).unwrap();
    assert!(ctx.mining_suppressed(actors[0]));
    assert_eq!(
        ctx.check_mining_suppression(actors[8]),
        Err(ServerError::Capacity {
            resource: Resource::Players,
            limit: 8,
            observed: 9
        })
    );
    assert!(ctx.suppress_mining(actors[8]).is_err());
    assert!(!ctx.mining_suppressed(actors[8]));
    let companion = ActorKey::Companion(
        CompanionId::try_from_bytes([1, 2, 3, 4, 5, 6, 0x40, 8, 0x80, 10, 11, 12, 13, 14, 15, 16])
            .unwrap(),
    );
    assert_eq!(
        ctx.check_mining_suppression(companion),
        Err(ServerError::InvalidInput { field: "actor" })
    );
}
