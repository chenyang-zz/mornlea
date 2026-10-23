//! Test-only authority/transport owner shared by engine-free qualification and
//! the disposable TCP example. Source residents use the accepted parity fixtures.
#![allow(dead_code)]
use mornlea_domain::*;
use mornlea_protocol::{ClientPacket, LoginStart};
#[cfg(test)]
use mornlea_protocol::{ProtocolCodec, ServerPacket, State, read_frame_ref};
use mornlea_server::contracts::*;
use mornlea_server::core::world::ReadyChunk;
use mornlea_server::state::{AuthorityState, TickContext};
use mornlea_server::transport::{live::LoginDriver, memory::MemoryTransport, tcp::TcpTransport};
use mornlea_storage::{
    ChestSlot, Chunk, CompanionBody, ContainerSnapshot, FurnaceSlot, HostileMob, Inventory,
    ItemStack as StorageStack, PassiveMob, PlayerId as SavePlayerId, PlayerLocation, StorageKind,
    StoredCompanionTask, StoredPlayer,
};
#[cfg(test)]
use std::collections::VecDeque;
use std::collections::{BTreeMap, BTreeSet};
#[cfg(test)]
use std::num::NonZeroU64;
#[cfg(test)]
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
const WORLD_SEED: i64 = 7;
const GRASS_BLOCK: u16 = 4;
const CHEST_BLOCK: u16 = 11;
const FURNACE_BLOCK: u16 = 9;
const ITEM_COAL: u16 = 5;
pub const DEADLINE: Duration = Duration::from_secs(10);

pub struct RealClock;
impl Clock for RealClock {
    fn monotonic(&self) -> Instant {
        Instant::now()
    }
    fn unix_ms(&self) -> i64 {
        1000
    }
}

/// Only persistence is substituted; admission and installation remain live owners.
#[derive(Default)]
pub struct ImmediateLoad {
    next: u64,
    tickets: BTreeMap<u64, StoredPlayer>,
    spawn_y: Option<f32>,
}
impl PlayerLoadPort for ImmediateLoad {
    fn start(&mut self, player: PlayerId, _: Deadline) -> Result<LoginTicket, ServerError> {
        self.next += 1;
        let ticket = LoginTicket::try_from_raw(self.next)?;
        let stored = stored_player(
            player.bytes()[0],
            "Ada",
            [0.5, self.spawn_y.unwrap_or(65.0), 0.5],
            0.0,
            0.0,
            |stored| {
                stored.player_id = SavePlayerId::from_bytes(player.bytes());
                stored.inventory.backpack[0] = storage(2, 5);
            },
        );
        self.tickets.insert(ticket.get(), stored);
        Ok(ticket)
    }
    fn poll(&mut self, ticket: LoginTicket) -> LoadPoll {
        self.tickets
            .remove(&ticket.get())
            .map_or(LoadPoll::Pending, |stored| LoadPoll::Loaded(Some(stored)))
    }
    fn cancel(&mut self, ticket: LoginTicket) -> Result<(), ServerError> {
        self.tickets.remove(&ticket.get());
        Ok(())
    }
}
pub fn player(tag: u8) -> PlayerId {
    PlayerId::try_from_bytes(uuid(tag)).unwrap()
}

fn uuid(tag: u8) -> [u8; 16] {
    let mut bytes = [0u8; 16];
    bytes[0] = tag.max(1);
    bytes[6] = 0x40;
    bytes[8] = 0x80;
    bytes
}

fn chunk_key(x: i32, z: i32) -> ChunkKey {
    ChunkKey {
        dimension: Dimension::OVERWORLD,
        pos: ChunkPos::new(x, z),
    }
}

fn air_chunk() -> Chunk {
    Chunk {
        sections: vec![
            ContainerSnapshot {
                kind: StorageKind::Single,
                bits: 0,
                single: 0,
                palette: vec![],
                packed: vec![],
            };
            24
        ],
        drops: vec![Default::default(); 32],
        furnaces: vec![FurnaceSlot::default(); 32],
        chests: vec![ChestSlot::default(); 16],
    }
}

fn set_cell(chunk: &mut Chunk, pos: BlockPos, block: u16) {
    let index = mornlea_domain::chunk_block_index(pos) as usize;
    let section = &mut chunk.sections[index / 4096];
    if section.kind == StorageKind::Single {
        *section = ContainerSnapshot {
            kind: StorageKind::Direct,
            bits: 15,
            single: 0,
            palette: vec![],
            packed: vec![0; 1024],
        };
    }
    section.packed[(index % 4096) / 4] |= u64::from(block) << ((index % 4) * 15);
}

fn ground_chunk() -> Chunk {
    let mut chunk = air_chunk();
    for x in 0..16 {
        for z in 0..16 {
            set_cell(&mut chunk, BlockPos::new(x, 64, z), GRASS_BLOCK);
        }
    }
    chunk
}

fn chest_in_chunk(
    chunk: &mut Chunk,
    pos: BlockPos,
    items: [StorageStack; 27],
) -> mornlea_domain::ContainerRef {
    set_cell(chunk, pos, CHEST_BLOCK);
    let index = mornlea_domain::chunk_block_index(pos) as u32;
    chunk.chests[0] = ChestSlot {
        generation: 1,
        active: true,
        block_index: index,
        items,
    };
    mornlea_domain::ContainerRef::try_new(
        ChunkPos::new(pos.x() >> 4, pos.z() >> 4),
        ContainerKind::Chest,
        0,
        1,
    )
    .unwrap()
}

fn furnace_in_chunk(
    chunk: &mut Chunk,
    pos: BlockPos,
    input: StorageStack,
    fuel: StorageStack,
    output: StorageStack,
) -> mornlea_domain::ContainerRef {
    set_cell(chunk, pos, FURNACE_BLOCK);
    let index = mornlea_domain::chunk_block_index(pos) as u32;
    chunk.furnaces[1] = FurnaceSlot {
        generation: 1,
        active: true,
        block_index: index,
        input,
        fuel,
        output,
        burn_ticks: 0,
        progress_ticks: 0,
    };
    mornlea_domain::ContainerRef::try_new(
        ChunkPos::new(pos.x() >> 4, pos.z() >> 4),
        ContainerKind::Furnace,
        1,
        1,
    )
    .unwrap()
}

fn day_environment(world_time: u64) -> EnvironmentState {
    EnvironmentState {
        seed: WORLD_SEED,
        next_tick: 0,
        world_time,
        day_phase_offset: 0,
        season_offset: 0,
        weather: Weather::Clear,
        weather_remaining: 0,
        difficulty: 0,
        tunables: RuleTunables::source_defaults(),
    }
}

fn survival() -> SurvivalState {
    SurvivalState::try_new(SurvivalStateParts {
        health: 20,
        oxygen: 300,
        hunger: 20,
        saturation_zero: false,
        armor_points: 0,
    })
    .unwrap()
}

fn look(yaw: f32, pitch: f32) -> LookAngles {
    LookAngles::try_new(yaw, pitch).unwrap()
}

fn companion_actor(id: CompanionId, position: [f32; 3], yaw: f32) -> ActorRecord {
    ActorRecord::try_new(
        ActorKey::Companion(id),
        ActorLifecycle::Active,
        Dimension::OVERWORLD,
        MotionState::new(MotionStateParts {
            position: FiniteVec3::try_new(position).unwrap(),
            velocity: FiniteVec3::try_new([0.0; 3]).unwrap(),
            on_ground: true,
        }),
        look(yaw, 0.0),
        survival(),
        ActorBody::Companion(CompanionBody {
            id: SavePlayerId::from_bytes(id.bytes()),
            dimension: 0,
            position,
            yaw,
            pitch: 0.0,
            inventory: Inventory::default(),
        }),
    )
    .unwrap()
}

fn hostile_actor(id: u64, position: [f32; 3], kind: u8) -> ActorRecord {
    ActorRecord::try_new(
        ActorKey::Hostile(HostileId::try_new(id).unwrap()),
        ActorLifecycle::Active,
        Dimension::OVERWORLD,
        MotionState::new(MotionStateParts {
            position: FiniteVec3::try_new(position).unwrap(),
            velocity: FiniteVec3::try_new([0.0; 3]).unwrap(),
            on_ground: true,
        }),
        look(0.0, 0.0),
        survival(),
        ActorBody::Hostile(HostileMob {
            id,
            dimension: 0,
            position,
            velocity: [0.0; 3],
            on_ground: true,
            yaw: 0.0,
            health: 20,
            attack_cooldown: 0,
            hurt_cooldown: 0,
            burn_cooldown: 20,
            has_target: false,
            player_id: SavePlayerId::from_bytes([0; 16]),
            next_repath_ticks: 0,
            distant_ticks: 0,
            kind,
        }),
    )
    .unwrap()
}

fn passive_actor(id: u64, position: [f32; 3]) -> ActorRecord {
    ActorRecord::try_new(
        ActorKey::Passive(PassiveId::try_new(id).unwrap()),
        ActorLifecycle::Active,
        Dimension::OVERWORLD,
        MotionState::new(MotionStateParts {
            position: FiniteVec3::try_new(position).unwrap(),
            velocity: FiniteVec3::try_new([0.0; 3]).unwrap(),
            on_ground: true,
        }),
        look(0.0, 0.0),
        survival(),
        ActorBody::Passive(PassiveMob {
            id,
            dimension: 0,
            position,
            velocity: [0.0; 3],
            on_ground: true,
            yaw: 0.0,
            health: 20,
        }),
    )
    .unwrap()
}

fn passive_runtime(id: PassiveId) -> ActorRuntime {
    ActorRuntime {
        key: ActorKey::Passive(id),
        controls: None,
        has_view: false,
        reset: false,
        attack_cooldown: 0,
        hurt_cooldown: 0,
        burn_cooldown: 0,
        oxygen: 0,
        peak_y: 65.0,
        exhaustion_milli: 0,
        saturation_milli: 0,
        since_damage_ticks: 0,
        drown_ticks: 0,
        starvation_ticks: 0,
        eating: None,
        bow: None,
        path: None,
        aux: ActorAux::Passive {
            home: BlockPos::new(8, 65, 8),
            flee_ticks: 0,
            flee_from: None,
            graze_ticks: 0,
            graze_at: None,
            fresh: false,
        },
    }
}

fn drop_record(slot: u8, age: u32, position: [f32; 3]) -> DropRecord {
    DropRecord {
        id: DropId::try_new(0, ChunkPos::new(0, 0), slot, 1).unwrap(),
        position: FiniteVec3::try_new(position).unwrap(),
        stack: StorageStack {
            item: ITEM_COAL,
            count: 2,
            durability: 0,
        },
        pickup_delay: 5,
        age,
    }
}

fn stack(item: u16, count: u8) -> ItemStack {
    ItemStack::try_new(item, count, 0).unwrap()
}

fn storage(item: u16, count: u8) -> StorageStack {
    StorageStack {
        item,
        count,
        durability: 0,
    }
}

fn storage_array<const N: usize>(filled: &[(usize, u16, u8)]) -> [StorageStack; N] {
    let mut slots = [StorageStack::default(); N];
    for (index, item, count) in filled {
        slots[*index] = storage(*item, *count);
    }
    slots
}

fn stored_player(
    tag: u8,
    name: &str,
    position: [f32; 3],
    yaw: f32,
    pitch: f32,
    customize: impl FnOnce(&mut StoredPlayer),
) -> StoredPlayer {
    let mut player = StoredPlayer {
        player_id: SavePlayerId::from_bytes(uuid(tag)),
        revision: 1,
        display_name: name.to_owned(),
        current: PlayerLocation {
            dimension: 0,
            position,
        },
        yaw,
        pitch,
        safe: None,
        inventory: Inventory::default(),
        health: 20,
        hunger: 20,
        saturation_milli: 5_000,
        exhaustion_milli: 0,
        respawn_present: false,
        respawn_position: [0.0; 3],
        respawn_dimension: 0,
        armor: [StorageStack::default(); 4],
        needs_rewrite: false,
    };
    customize(&mut player);
    player
}

/// Observes successful live operations without changing any result or ownership decision.
struct EndpointTap<'a> {
    inner: mornlea_server::transport::live::LiveEndpoint<'a>,
    submissions: &'a mut Vec<(SessionKey, mornlea_protocol::PlayIntent)>,
    activations: &'a mut Vec<SessionKey>,
}
impl mornlea_server::transport::common::TransportSessionPort for EndpointTap<'_> {
    fn submit(
        &mut self,
        session: SessionKey,
        intent: mornlea_protocol::PlayIntent,
    ) -> Result<SubmissionReceipt, ServerError> {
        let receipt = self.inner.submit(session, intent.clone())?;
        self.submissions.push((session, intent));
        Ok(receipt)
    }
    fn close_session(
        &mut self,
        session: SessionKey,
        reason: CloseReason,
    ) -> Result<(), ServerError> {
        self.inner.close_session(session, reason)
    }
}
impl PublicationPort for EndpointTap<'_> {
    fn publish(&mut self, publication: TickPublication) -> Result<(), ServerError> {
        self.inner.publish(publication)
    }
    fn take_outbox(
        &mut self,
        session: SessionKey,
        frames: usize,
        bytes: usize,
    ) -> Result<Vec<Vec<u8>>, ServerError> {
        self.inner.take_outbox(session, frames, bytes)
    }
    fn close_outbox(&mut self, session: SessionKey, reason: CloseReason) {
        self.inner.close_outbox(session, reason)
    }
}
impl mornlea_server::transport::common::TransportAuthority for EndpointTap<'_> {
    fn begin_login(
        &mut self,
        login: mornlea_protocol::AdmittedLogin,
        kind: TransportKind,
        deadline: Deadline,
    ) -> Result<LoginTicket, ServerError> {
        self.inner.begin_login(login, kind, deadline)
    }
    fn poll_login(&mut self, ticket: LoginTicket) -> LoginPoll {
        self.inner.poll_login(ticket)
    }
    fn commit_login(&mut self, ticket: LoginTicket) -> Result<SessionKey, ServerError> {
        let session = self.inner.commit_login(ticket)?;
        self.activations.push(session);
        Ok(session)
    }
    fn cancel_login(&mut self, ticket: LoginTicket) {
        self.inner.cancel_login(ticket)
    }
    fn world_seed(&self) -> i64 {
        self.inner.world_seed()
    }
}

pub struct Server {
    pub authority: AuthorityState,
    driver: LoginDriver,
    loads: ImmediateLoad,
    fixture_transport: MemoryTransport,
    fixture_conn: ConnectionId,
    pub ticks: u64,
    pub packets: u64,
    pub submissions: Vec<(SessionKey, mornlea_protocol::PlayIntent)>,
    activations: Vec<SessionKey>,
}
impl Server {
    pub fn new() -> Self {
        let limits = ServerLimits::try_new(8, 4096, 512, 64, 64, 1_048_576)
            .unwrap()
            .with_view_radius(2);
        let mut authority = AuthorityState::try_new(limits, WORLD_SEED).unwrap();
        let mut context = TickContext::restage_full(&mut authority);
        context
            .stage(RuleEffect::Environment(day_environment(1_000)))
            .unwrap();
        let mut front = ground_chunk();
        chest_in_chunk(
            &mut front,
            BlockPos::new(0, 66, -1),
            [StorageStack::default(); 27],
        );
        context.preload_ready_chunk(ReadyChunk::try_new(chunk_key(0, -1), 1, 1, front).unwrap());
        let mut home = ground_chunk();
        furnace_in_chunk(
            &mut home,
            BlockPos::new(0, 66, 1),
            storage(6, 2),
            storage(5, 2),
            StorageStack::default(),
        );
        context.preload_ready_chunk(ReadyChunk::try_new(chunk_key(0, 0), 1, 1, home).unwrap());
        context
            .stage(RuleEffect::Actor(companion_actor(
                CompanionId::try_from_bytes(uuid(9)).unwrap(),
                [8.5, 65.0, 8.5],
                1.25,
            )))
            .unwrap();
        context
            .stage(RuleEffect::Runtime(contract_runtime(
                CompanionId::try_from_bytes(uuid(9)).unwrap(),
            )))
            .unwrap();
        context
            .stage(RuleEffect::Actor(hostile_actor(31, [8.5, 65.0, 8.5], 0)))
            .unwrap();
        context
            .stage(RuleEffect::Actor(passive_actor(7, [8.5, 65.0, 8.5])))
            .unwrap();
        context
            .stage(RuleEffect::Runtime(passive_runtime(
                PassiveId::try_new(7).unwrap(),
            )))
            .unwrap();
        context.preload_drop(drop_record(3, 40, [0.5, 65.5, 2.5]));
        let residents = context.resident_snapshot();
        drop(context);
        authority.commit_residents(residents);
        authority
            .configure_companion_chat(&[(
                CompanionId::try_from_bytes(uuid(9)).unwrap(),
                CompanionName::try_from_canonical("companion-0900000000004000".to_owned()).unwrap(),
            )])
            .unwrap();
        let mut fixture_transport = MemoryTransport::new();
        let fixture_conn = fixture_transport.connect(Instant::now()).unwrap();
        let mut server = Self {
            authority,
            driver: LoginDriver::new(),
            loads: ImmediateLoad::default(),
            fixture_transport,
            fixture_conn,
            ticks: 0,
            packets: 0,
            submissions: Vec::new(),
            activations: Vec::new(),
        };
        // The remote actor is an independently admitted peer, never a fabricated player.
        let packets = [
            ClientPacket::ClientHello(
                mornlea_protocol::ClientHello::decode_inbound(&mornlea_protocol::encode_uvarint(
                    Identities::current().protocol,
                ))
                .unwrap(),
            ),
            ClientPacket::LoginStart(
                LoginStart::decode_inbound(
                    &LoginStart::new(player(2), "Bea", 8)
                        .unwrap()
                        .encode()
                        .unwrap(),
                )
                .unwrap(),
            ),
        ];
        for packet in packets {
            let mut endpoint = EndpointTap {
                inner: server.driver.bind(&mut server.authority, &mut server.loads),
                submissions: &mut server.submissions,
                activations: &mut server.activations,
            };
            server.fixture_transport.send(
                fixture_conn,
                MemoryTransport::encode_frame(&packet).unwrap(),
                &mut endpoint,
                &RealClock,
            );
            server
                .fixture_transport
                .poll(fixture_conn, &mut endpoint, &RealClock);
            let frames = server.fixture_transport.receive(fixture_conn, 64, 1 << 20);
            server
                .fixture_transport
                .acknowledge(fixture_conn, frames.len(), &mut endpoint);
        }
        server.tick();
        assert!(server.session(player(2)).is_some());
        let remote = server.session(player(2)).unwrap();
        server.stage(move |ctx| {
            let mut actor = ctx.read().actor(ActorKey::Player(remote)).cloned().unwrap();
            actor.motion = MotionState::new(MotionStateParts {
                position: FiniteVec3::try_new([4.5, 65.0, 4.5]).unwrap(),
                velocity: FiniteVec3::try_new([0.0; 3]).unwrap(),
                on_ground: true,
            });
            ctx.stage(RuleEffect::Actor(actor)).unwrap();
            ctx.stage(RuleEffect::Projectile {
                before: None,
                after: Some(ProjectileRecord {
                    id: ProjectileId::try_new(5).unwrap(),
                    owner: ActorKey::Player(remote),
                    dimension: Dimension::OVERWORLD,
                    position: FiniteVec3::try_new([4.5, 100.0, 4.5]).unwrap(),
                    velocity: FiniteVec3::try_new([0.0; 3]).unwrap(),
                    kind: ProjectileKind::Shard,
                    damage: 3,
                    age: 97,
                }),
            })
            .unwrap();
        });
        server
    }
    fn endpoint(&mut self) -> EndpointTap<'_> {
        EndpointTap {
            inner: self.driver.bind(&mut self.authority, &mut self.loads),
            submissions: &mut self.submissions,
            activations: &mut self.activations,
        }
    }
    pub fn session(&self, id: PlayerId) -> Option<SessionKey> {
        self.active_sessions()
            .into_iter()
            .find(|key| self.authority.session(*key).unwrap().player_id == id)
    }
    #[cfg(test)]
    pub fn ground_only(&mut self) {
        self.stage(|ctx| {
            ctx.preload_ready_chunk(
                ReadyChunk::try_new(chunk_key(0, 0), 1, 1, ground_chunk()).unwrap(),
            )
        });
    }
    #[cfg(test)]
    pub fn snow_ground(&mut self) {
        self.loads.spawn_y = Some(65.5);
        self.stage(|ctx| {
            let mut chunk = ground_chunk();
            for x in 0..16 {
                for z in 0..16 {
                    set_cell(&mut chunk, BlockPos::new(x, 65, z), 88);
                }
            }
            ctx.preload_ready_chunk(ReadyChunk::try_new(chunk_key(0, 0), 1, 2, chunk).unwrap());
        });
    }
    pub fn active_sessions(&self) -> Vec<SessionKey> {
        self.authority
            .residents()
            .actors
            .iter()
            .filter_map(|actor| match actor.key {
                ActorKey::Player(session)
                    if self
                        .authority
                        .session(session)
                        .is_some_and(|facts| facts.phase == SessionPhase::Active) =>
                {
                    Some(session)
                }
                _ => None,
            })
            .collect()
    }
    pub fn stage(&mut self, apply: impl FnOnce(&mut TickContext<'_>)) {
        let mut ctx = TickContext::restage_full(&mut self.authority);
        apply(&mut ctx);
        let residents = ctx.resident_snapshot();
        drop(ctx);
        self.authority.commit_residents(residents);
    }
    pub fn tick(&mut self) -> TickPublication {
        self.tick_full()
    }
    pub fn tick_full(&mut self) -> TickPublication {
        self.authority.advance_tick_full().expect("tick")
    }

    pub fn tick_commands(&mut self, commands: usize) -> TickPublication {
        self.authority
            .advance_tick_commands(commands)
            .expect("tick")
    }

    pub fn tick_budget(&mut self, budget: TickBudget) -> TickPublication {
        // Only the external planner completion is immediate; task installation and publication stay authoritative.
        let companion = CompanionId::try_from_bytes(uuid(9)).unwrap();
        if let Some(planning) = self
            .authority
            .take_companion_chat_planning(companion)
            .unwrap()
        {
            let plan = AgentPlan::try_new(
                "follow".to_owned(),
                vec![PlanStep::Follow {
                    player_id: planning.issuer.player_id,
                }],
            )
            .unwrap();
            assert!(
                self.authority
                    .install_companion_chat_plan(companion, planning.generation, plan)
                    .unwrap()
            );
        }
        let publication = self.authority.advance_tick(budget).unwrap();
        self.ticks += 1;
        self.packets += (publication.events.len() + publication.control.len()) as u64;
        // The fixture peer consumes its output too, so it cannot become a slow receiver.
        if let Some(session) = self.session(player(2)) {
            self.authority.take_outbox(session, 512, 8 << 20).unwrap();
        }
        publication
    }
}

/// Nonblocking loopback service used verbatim by the external process fixture.
pub struct TcpServer {
    pub server: Server,
    pub transport: TcpTransport,
    connections: Vec<ConnectionId>,
    owners: BTreeMap<ConnectionId, SessionKey>,
    pub admitted: BTreeSet<SessionKey>,
    pub retired: u64,
    pub errors: Vec<String>,
    #[cfg(test)]
    pub wire: Vec<(SessionKey, ServerPacket)>,
}
impl TcpServer {
    pub fn new() -> Self {
        Self {
            server: Server::new(),
            transport: TcpTransport::bind_loopback().unwrap(),
            connections: Vec::new(),
            owners: BTreeMap::new(),
            admitted: BTreeSet::new(),
            retired: 0,
            errors: Vec::new(),
            #[cfg(test)]
            wire: Vec::new(),
        }
    }
    pub fn address(&self) -> std::net::SocketAddr {
        self.transport.local_addr().unwrap()
    }
    fn observe(&mut self, id: ConnectionId, progress: ConnectionProgress) {
        if let ConnectionProgress::Closed { reason, class } = progress {
            if self.connections.contains(&id) {
                self.retired += 1;
                if !matches!(reason, CloseReason::PeerGone | CloseReason::Shutdown) {
                    self.errors.push(format!("closed: {reason:?}, {class:?}"));
                }
            }
            self.connections.retain(|live| *live != id);
            self.owners.remove(&id);
        }
    }
    fn bind_new_activations(&mut self, id: ConnectionId, start: usize) {
        if !self.connections.contains(&id) {
            return;
        }
        for session in &self.server.activations[start..] {
            assert!(
                self.owners.insert(id, *session).is_none(),
                "one admission belongs to one connection"
            );
            self.admitted.insert(*session);
        }
    }
    pub fn pump(&mut self) {
        for _ in 0..16 {
            let result = {
                let mut endpoint = self.server.endpoint();
                self.transport.accept_one(&mut endpoint, &RealClock)
            };
            match result {
                Ok(Some(id)) => self.connections.push(id),
                Ok(None) => break,
                Err(error) => {
                    self.errors.push(error.to_string());
                    break;
                }
            }
        }
        for id in self.connections.clone() {
            let activation_start = self.server.activations.len();
            let progress = {
                let mut endpoint = self.server.endpoint();
                self.transport.pump_in(id, &mut endpoint, &RealClock)
            };
            self.observe(id, progress);
            self.bind_new_activations(id, activation_start);
            if !self.connections.contains(&id) {
                continue;
            }
            let activation_start = self.server.activations.len();
            let progress = {
                let mut endpoint = self.server.endpoint();
                self.transport.poll(id, &mut endpoint, &RealClock)
            };
            self.observe(id, progress);
            self.bind_new_activations(id, activation_start);
            if !self.connections.contains(&id) {
                continue;
            }
            let activation_start = self.server.activations.len();
            let progress = {
                let mut endpoint = self.server.endpoint();
                self.transport.flush_out(id, &mut endpoint).1
            };
            self.observe(id, progress);
            self.bind_new_activations(id, activation_start);
            if let Some(session) = self.owners.get(&id).copied() {
                let mut endpoint = self.server.endpoint();
                match self
                    .transport
                    .forward_outbox(id, session, &mut endpoint, 512, 8 << 20)
                {
                    Ok(frames) => {
                        #[cfg(test)]
                        for frame in &frames {
                            let parsed = read_frame_ref(frame).unwrap();
                            self.wire.push((
                                session,
                                ProtocolCodec::new()
                                    .unwrap()
                                    .decode_server(State::Play, parsed.packet_id, parsed.payload)
                                    .unwrap(),
                            ));
                        }
                        #[cfg(not(test))]
                        let _ = frames;
                    }
                    Err(error) => self.errors.push(format!("forward: {error:?}")),
                }
                let progress = self.transport.flush_out(id, &mut endpoint).1;
                self.observe(id, progress);
            }
        }
    }
    pub fn close_peers(&mut self) {
        for id in self.connections.clone() {
            let mut endpoint = self.server.endpoint();
            self.transport
                .close(id, CloseReason::Shutdown, &mut endpoint);
            self.retired += 1;
        }
        self.connections.clear();
        self.owners.clear();
    }
    pub fn close_connections(&mut self) {
        self.server.driver.stop_new();
        self.close_peers();
        if let Err(error) = self
            .server
            .driver
            .cancel_pending(&mut self.server.authority, &mut self.server.loads)
        {
            self.errors.push(format!("shutdown: {error:?}"));
        }
    }
}

#[cfg(test)]
pub use client::*;

fn contract_runtime(id: CompanionId) -> ActorRuntime {
    ActorRuntime {
        key: ActorKey::Companion(id),
        controls: None,
        has_view: false,
        reset: false,
        attack_cooldown: 0,
        hurt_cooldown: 0,
        burn_cooldown: 0,
        oxygen: 300,
        peak_y: 65.0,
        exhaustion_milli: 0,
        saturation_milli: 0,
        since_damage_ticks: 0,
        drown_ticks: 0,
        starvation_ticks: 0,
        eating: None,
        bow: None,
        path: None,
        aux: ActorAux::Companion {
            generation: 0,
            attempt: 3,
            task: StoredCompanionTask::default(),
            mining_target: None,
        },
    }
}

// Arena plumbing stays out of the standalone TCP example's dependency surface.
#[cfg(test)]
mod client {
    use super::*;
    use crate::abi::boundary::{BoundaryValue, CoreOpenSpec, CoreTokenValue};
    use crate::client_core::rust_core::CoreArena;
    use mornlea_client_core::session::io::{MemoryConnector, MemoryPeer};
    use mornlea_client_core::{
        ClientConfig, ClientCore, ClientEndpoint, ClientError, ClientIdentity, ClientLimits,
        ClientWorkBudget, Connector, ConnectorRegistry, Endpoint, InputAction, InputBatch,
        InputReceipt, PresentationFrame, SessionEpoch, StdMonotonicClock, StepReport,
        TransportPoll, TransportTicket,
    };
    use std::cell::RefCell;
    use std::rc::Rc;

    /// Physical terminal evidence belongs to the captured generation, never the peer's current slot.
    pub struct F2Memory {
        inner: Arc<MemoryConnector>,
        peer: MemoryPeer,
        current: Mutex<Option<TransportTicket>>,
        terminal: Mutex<BTreeSet<u64>>,
    }
    impl F2Memory {
        fn new() -> Arc<Self> {
            let (inner, peer) = MemoryConnector::pair(
                ClientLimits::try_new().unwrap(),
                NonZeroU64::new(1).unwrap(),
            );
            Arc::new(Self {
                inner,
                peer,
                current: Mutex::new(None),
                terminal: Mutex::new(BTreeSet::new()),
            })
        }
        pub fn ticket(&self) -> TransportTicket {
            self.current.lock().unwrap().unwrap()
        }
        pub fn feed(&self, ticket: TransportTicket, bytes: &[u8]) -> Result<usize, ClientError> {
            if *self.current.lock().unwrap() != Some(ticket)
                || self
                    .terminal
                    .lock()
                    .unwrap()
                    .contains(&ticket.generation().get())
            {
                return Err(ClientError::StaleEpoch);
            }
            self.peer.feed(bytes)
        }
        fn server_closed(&self, ticket: TransportTicket) {
            self.terminal
                .lock()
                .unwrap()
                .insert(ticket.generation().get());
        }
    }
    impl Connector for F2Memory {
        fn try_connect(
            &self,
            endpoint: &Endpoint,
            identity: &ClientIdentity,
        ) -> Result<TransportTicket, ClientError> {
            let ticket = self.inner.try_connect(endpoint, identity)?;
            *self.current.lock().unwrap() = Some(ticket);
            Ok(ticket)
        }
        fn poll(&self, ticket: TransportTicket) -> TransportPoll {
            if self
                .terminal
                .lock()
                .unwrap()
                .contains(&ticket.generation().get())
            {
                TransportPoll::Closed(ClientError::Disconnected)
            } else {
                self.inner.poll(ticket)
            }
        }
        fn try_send(&self, ticket: TransportTicket, frame: &[u8]) -> Result<(), ClientError> {
            self.inner.try_send(ticket, frame)
        }
        fn close(&self, ticket: TransportTicket) -> Result<(), ClientError> {
            self.inner.close(ticket)
        }
    }
    #[derive(Default)]
    pub struct Capture {
        pub frame: Option<Arc<PresentationFrame>>,
        pub inject: Option<PresentationFrame>,
        pub prepared: usize,
        resources: Vec<mornlea_client_core::PreparedResourceKey>,
        pub retired_resources: usize,
    }
    struct ObservedCore {
        core: ClientCore,
        capture: Rc<RefCell<Capture>>,
    }
    impl ClientEndpoint for ObservedCore {
        fn connect(&mut self, e: Endpoint, i: ClientIdentity) -> Result<SessionEpoch, ClientError> {
            self.core.connect(e, i)
        }
        fn submit_input(
            &mut self,
            e: SessionEpoch,
            i: InputBatch,
        ) -> Result<InputReceipt, ClientError> {
            self.core.submit_input(e, i)
        }
        fn step(
            &mut self,
            e: SessionEpoch,
            w: ClientWorkBudget,
        ) -> Result<StepReport, ClientError> {
            self.core.step(e, w)
        }
        fn snapshot(&self, e: SessionEpoch) -> Result<Arc<PresentationFrame>, ClientError> {
            if let Some(frame) = self.capture.borrow_mut().inject.take() {
                return Ok(Arc::new(frame));
            }
            let frame = self.core.snapshot(e)?;
            let mut capture = self.capture.borrow_mut();
            for family in frame.families() {
                if let mornlea_client_core::FamilyRecords::Terrain(records) = family.records() {
                    for record in records {
                        if let Some(key) = record.resource() {
                            assert!(self.core.prepared_resource(key).is_ok());
                            capture.prepared += 1;
                            if !capture.resources.contains(key) {
                                capture.resources.push(*key);
                            }
                        }
                    }
                }
            }
            capture.frame = Some(Arc::clone(&frame));
            Ok(frame)
        }
        fn reset(&mut self, e: SessionEpoch) -> Result<SessionEpoch, ClientError> {
            let epoch = self.core.reset(e)?;
            let mut capture = self.capture.borrow_mut();
            for key in &capture.resources {
                assert_eq!(
                    self.core.prepared_resource(key),
                    Err(ClientError::StaleEpoch)
                );
            }
            capture.retired_resources += capture.resources.len();
            capture.resources.clear();
            Ok(epoch)
        }
        fn close(&mut self) -> Result<(), ClientError> {
            self.core.close()
        }
    }
    enum Link {
        Memory {
            transport: MemoryTransport,
            connector: Arc<F2Memory>,
            conn: ConnectionId,
            ticket: Option<TransportTicket>,
            ingress: State,
            pending: VecDeque<(Vec<u8>, usize, bool)>,
        },
        Tcp(Box<TcpServer>),
    }
    pub struct Rig {
        pub server: Server,
        link: Link,
        pub arena: CoreArena,
        pub token: CoreTokenValue,
        pub epoch: SessionEpoch,
        pub capture: Rc<RefCell<Capture>>,
        pub publications: Vec<TickPublication>,
        pub wire: Vec<ServerPacket>,
        pub outbound: Vec<ClientPacket>,
        pub boundary: Option<BoundaryValue>,
        pub transcript: Vec<Arc<PresentationFrame>>,
        retired_memory: Vec<(ConnectionId, TransportTicket)>,
    }
    impl Rig {
        pub fn memory() -> Self {
            Self::new(false)
        }
        pub fn tcp() -> Self {
            Self::new(true)
        }
        fn new(tcp: bool) -> Self {
            let mut registry = ConnectorRegistry::new();
            let server = Server::new();
            let (link, endpoint) = if tcp {
                let tcp = TcpServer::new();
                let addr = tcp.address();
                (Link::Tcp(Box::new(tcp)), Endpoint::Tcp(addr))
            } else {
                let connector = F2Memory::new();
                registry
                    .register(
                        NonZeroU64::new(1).unwrap(),
                        Arc::clone(&connector) as Arc<dyn Connector>,
                    )
                    .unwrap();
                let mut transport = MemoryTransport::new();
                let conn = transport.connect(Instant::now()).unwrap();
                (
                    Link::Memory {
                        transport,
                        connector,
                        conn,
                        ticket: None,
                        ingress: State::Handshake,
                        pending: VecDeque::new(),
                    },
                    Endpoint::Memory {
                        connector_id: NonZeroU64::new(1).unwrap(),
                    },
                )
            };
            if let Link::Tcp(ref tcp) = link {
                // Rig exposes the same authority owner regardless of adapter; swap without duplicating state.
                let _ = tcp;
            }
            let spec = CoreOpenSpec {
                limits: ClientLimits::try_new().unwrap(),
                hello_ms: 5000,
                login_ms: 10000,
                connector_capability: 1,
            };
            let config = ClientConfig::try_new(
                spec.limits,
                Duration::from_millis(5000),
                Duration::from_millis(10000),
                Arc::new(StdMonotonicClock),
                Arc::new(registry),
            )
            .unwrap();
            let capture = Rc::new(RefCell::new(Capture::default()));
            let mut arena = CoreArena::new();
            let token = arena
                .open_with_endpoint(
                    &spec,
                    Box::new(ObservedCore {
                        core: ClientCore::new(config).unwrap(),
                        capture: Rc::clone(&capture),
                    }),
                )
                .unwrap();
            let epoch = arena
                .connect(
                    token,
                    endpoint,
                    ClientIdentity::try_new(LoginStart::new(player(1), "Ada", 8).unwrap()).unwrap(),
                )
                .unwrap();
            let mut rig = Self {
                server,
                link,
                arena,
                token,
                epoch,
                capture,
                publications: Vec::new(),
                wire: Vec::new(),
                outbound: Vec::new(),
                boundary: None,
                transcript: Vec::new(),
                retired_memory: Vec::new(),
            };
            if let Link::Tcp(tcp) = &mut rig.link {
                std::mem::swap(&mut rig.server, &mut tcp.server);
            }
            rig.frame();
            rig
        }
        pub fn pump(&mut self) {
            match &mut self.link {
                Link::Memory {
                    transport,
                    connector,
                    conn,
                    ticket,
                    ingress,
                    pending,
                } => {
                    let current = connector.ticket();
                    if *ticket != Some(current) {
                        assert!(
                            ticket.is_none(),
                            "reset must replace the server connection before pumping"
                        );
                        *ticket = Some(current);
                    }
                    let mut endpoint = self.server.endpoint();
                    for frame in connector.peer.drain_sent() {
                        let parsed = read_frame_ref(&frame).unwrap();
                        self.outbound.push(
                            mornlea_protocol::decode_client(
                                *ingress,
                                parsed.packet_id,
                                parsed.payload,
                            )
                            .unwrap(),
                        );
                        *ingress = match *ingress {
                            State::Handshake => State::Login,
                            _ => State::Play,
                        };
                        transport.send(*conn, frame, &mut endpoint, &RealClock);
                    }
                    let progress = transport.poll(*conn, &mut endpoint, &RealClock);
                    if matches!(progress, ConnectionProgress::Closed { .. }) {
                        connector.server_closed(current);
                    }
                    for frame in transport.receive(*conn, 512, 8 << 20) {
                        pending.push_back((frame, 0, true));
                    }
                    if let Some(session) = self.server.session(player(1)) {
                        for frame in self
                            .server
                            .authority
                            .take_outbox(session, 512, 8 << 20)
                            .unwrap()
                        {
                            let parsed = read_frame_ref(&frame).unwrap();
                            self.wire.push(
                                ProtocolCodec::new()
                                    .unwrap()
                                    .decode_server(State::Play, parsed.packet_id, parsed.payload)
                                    .unwrap(),
                            );
                            pending.push_back((frame, 0, false));
                        }
                    }
                    while let Some((frame, head, control)) = pending.front_mut() {
                        let mut retained = false;
                        match connector.feed(current, &frame[*head..]) {
                            Ok(consumed) => {
                                *head += consumed;
                                if *head < frame.len() {
                                    break;
                                }
                            }
                            // Capacity retains the complete bytes; polling admits them without re-feeding.
                            Err(ClientError::Capacity) => {
                                *head = frame.len();
                                retained = true;
                            }
                            Err(error) => panic!("complete delivery failed: {error:?}"),
                        }
                        let control = *control;
                        pending.pop_front();
                        if control {
                            let mut endpoint = self.server.endpoint();
                            transport.acknowledge(*conn, 1, &mut endpoint);
                        }
                        if retained {
                            break;
                        }
                    }
                }
                Link::Tcp(tcp) => {
                    std::mem::swap(&mut self.server, &mut tcp.server);
                    tcp.pump();
                    std::mem::swap(&mut self.server, &mut tcp.server);
                    for (session, packet) in tcp.wire.drain(..) {
                        if self
                            .server
                            .authority
                            .session(session)
                            .is_some_and(|facts| facts.player_id == player(1))
                        {
                            self.wire.push(packet);
                        }
                    }
                    assert!(tcp.errors.is_empty(), "{:?}", tcp.errors);
                }
            }
        }
        pub fn step(&mut self) -> StepReport {
            self.pump();
            let report = self
                .arena
                .step(
                    self.token,
                    self.epoch,
                    ClientWorkBudget::try_new(4096, 4096).unwrap(),
                )
                .unwrap();
            self.frame();
            report
        }
        pub fn frame(&mut self) -> Arc<PresentationFrame> {
            self.boundary = Some(self.arena.pull_frame(self.token, self.epoch).unwrap());
            let frame = self.capture.borrow().frame.as_ref().unwrap().clone();
            assert!(
                self.transcript.len() < 4096,
                "bounded qualification transcript exhausted"
            );
            self.transcript.push(Arc::clone(&frame));
            frame
        }
        pub fn login(&mut self) {
            let deadline = Instant::now() + DEADLINE;
            loop {
                let report = self.step();
                assert!(
                    report.terminal().is_none(),
                    "actual login terminated: {:?}",
                    report.terminal()
                );
                let frame = self.frame();
                let admitted=frame.families().iter().any(|family|matches!(family.records(),mornlea_client_core::FamilyRecords::Session(records) if records.iter().any(|record|*record.phase()==mornlea_client_core::SessionPhase::Admitted)));
                if admitted && self.server.driver.pending() == 0 && self.server.loads.next >= 2 {
                    break;
                }
                assert!(Instant::now() < deadline, "actual login timed out");
                std::thread::yield_now();
            }
            self.tick();
        }
        pub fn tick(&mut self) -> Arc<PresentationFrame> {
            self.tick_full()
        }
        pub fn tick_full(&mut self) -> Arc<PresentationFrame> {
            self.pump();
            let wire_start = self.wire.len();
            let publication = self.server.tick_full();
            self.finish_tick_publication(wire_start, publication)
        }
        pub fn tick_commands(&mut self, commands: usize) -> Arc<PresentationFrame> {
            self.pump();
            let wire_start = self.wire.len();
            let publication = self.server.tick_commands(commands);
            self.finish_tick_publication(wire_start, publication)
        }
        pub fn tick_budget(&mut self, budget: TickBudget) -> Arc<PresentationFrame> {
            self.pump();
            let wire_start = self.wire.len();
            let publication = self.server.tick_budget(budget);
            self.finish_tick_publication(wire_start, publication)
        }
        fn finish_tick_publication(
            &mut self,
            wire_start: usize,
            publication: TickPublication,
        ) -> Arc<PresentationFrame> {
            let session = self.server.session(player(1)).unwrap();
            let expected: Vec<_> = publication
                .events
                .iter()
                .filter(|event| {
                    event.recipient() == EventRecipient::Session(session.get())
                        || event.recipient() == EventRecipient::Broadcast
                })
                .map(|event| ServerPacket::try_from(event.event().clone()).unwrap())
                .chain(
                    publication
                        .control
                        .iter()
                        .filter(|reply| reply.session == session)
                        .map(|reply| reply.packet.clone()),
                )
                .collect();
            self.publications.push(publication);
            let old_revision = self.frame().confirmed_revision().get();
            let deadline = Instant::now() + DEADLINE;
            loop {
                self.step();
                let frame = self.frame();
                if frame.confirmed_revision().get() >= old_revision + expected.len() as u64 {
                    assert_eq!(
                        self.wire[wire_start..],
                        expected,
                        "source records encode in exact outbox order"
                    );
                    return frame;
                }
                assert!(
                    Instant::now() < deadline,
                    "output timed out: expected {} from revision {}, now {}",
                    expected.len(),
                    old_revision,
                    frame.confirmed_revision().get()
                );
                std::thread::yield_now();
            }
        }
        pub fn submit(&mut self, actions: Vec<InputAction>) -> Result<InputReceipt, ClientError> {
            let session = self
                .server
                .session(player(1))
                .ok_or(ClientError::InvalidState)?;
            let before = self.server.authority.session(session).unwrap().next_arrival;
            let companion = CompanionId::try_from_bytes(uuid(9)).unwrap();
            let chat_before = self
                .server
                .submissions
                .iter()
                .filter(|(owner, intent)| {
                    *owner == session && matches!(intent, mornlea_protocol::PlayIntent::Chat(_))
                })
                .count();
            let receipt = self
                .arena
                .submit(self.token, InputBatch::try_new(self.epoch, actions)?)?;
            if let InputReceipt::Queued {
                sequenced_count,
                chat_count,
                ..
            } = receipt
            {
                // One semantic step publishes its transient records; transport waiting must not consume another frame.
                self.step();
                let deadline = Instant::now() + DEADLINE;
                loop {
                    self.pump();
                    let arrived = self.server.authority.session(session).unwrap().next_arrival
                        >= before + u64::from(sequenced_count);
                    let chat_arrived = self
                        .server
                        .submissions
                        .iter()
                        .filter(|(owner, intent)| {
                            *owner == session
                                && matches!(intent, mornlea_protocol::PlayIntent::Chat(_))
                        })
                        .count()
                        >= chat_before + usize::from(chat_count);
                    if arrived && chat_arrived {
                        break;
                    }
                    assert!(
                        Instant::now() < deadline,
                        "actual input ingress timed out: {receipt:?}, before {before}, after {:?}, chat {:?}",
                        self.server.authority.session(session),
                        self.server.authority.companion_chat_queue(companion)
                    );
                    std::thread::yield_now();
                }
            }
            Ok(receipt)
        }
        pub fn close_server(&mut self) {
            match &mut self.link {
                Link::Memory {
                    transport,
                    connector,
                    conn,
                    ..
                } => {
                    let mut endpoint = self.server.endpoint();
                    transport.close(*conn, CloseReason::PeerGone, &mut endpoint);
                    let progress = transport.poll(*conn, &mut endpoint, &RealClock);
                    assert!(matches!(progress, ConnectionProgress::Closed { .. }));
                    connector.server_closed(connector.ticket());
                }
                Link::Tcp(tcp) => {
                    std::mem::swap(&mut self.server, &mut tcp.server);
                    tcp.close_peers();
                    std::mem::swap(&mut self.server, &mut tcp.server);
                }
            }
        }
        fn replace_connection(&mut self) {
            if let Link::Memory {
                transport,
                conn,
                ticket,
                ingress,
                pending,
                ..
            } = &mut self.link
            {
                if let Some(old) = *ticket {
                    assert!(self.retired_memory.len() < 64);
                    self.retired_memory.push((*conn, old));
                }
                *conn = transport.connect(Instant::now()).unwrap();
                *ticket = None;
                *ingress = State::Handshake;
                pending.clear();
            }
        }
        pub fn reset(&mut self) {
            // Memory needs its declared peer pairing; TCP retirement must come from client EOF.
            if matches!(self.link, Link::Memory { .. }) {
                self.close_server();
            }
            self.epoch = self.arena.reset(self.token, self.epoch).unwrap();
            self.replace_connection();
        }
        pub fn reconnect(&mut self) {
            self.replace_connection();
            let endpoint = match &self.link {
                Link::Memory { .. } => Endpoint::Memory {
                    connector_id: NonZeroU64::new(1).unwrap(),
                },
                Link::Tcp(tcp) => Endpoint::Tcp(tcp.address()),
            };
            self.epoch = self
                .arena
                .connect(
                    self.token,
                    endpoint,
                    ClientIdentity::try_new(LoginStart::new(player(1), "Ada", 8).unwrap()).unwrap(),
                )
                .unwrap();
        }
        pub fn prove_retired_memory_close(&mut self) {
            if let Link::Memory {
                transport,
                connector,
                ..
            } = &mut self.link
            {
                let (old_conn, old_ticket) = *self.retired_memory.last().unwrap();
                let mut endpoint = self.server.endpoint();
                assert!(matches!(
                    transport.poll(old_conn, &mut endpoint, &RealClock),
                    ConnectionProgress::Closed { .. }
                ));
                connector.server_closed(old_ticket);
                assert_eq!(
                    connector.feed(old_ticket, &[0xff]),
                    Err(ClientError::StaleEpoch)
                );
            }
        }
        pub fn memory_connector(&self) -> Option<Arc<F2Memory>> {
            match &self.link {
                Link::Memory { connector, .. } => Some(Arc::clone(connector)),
                _ => None,
            }
        }
    }
}
