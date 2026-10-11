//! Two bounded load ledgers. Values move from the sole owner to their consumer.
use super::background::{Background, internal};
use crate::core::contracts::{
    ChunkKey, ChunkLoadPoll, ChunkRequestId, Deadline, DiskBackend, LoadPoll, LoadedValue,
    LoginTicket, Operation, Resource, SaveKey, ServerError,
};
use crate::core::world::PreparedChunk;
use mornlea_domain::PlayerId;
use std::collections::VecDeque;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::mpsc;
use std::time::Instant;

#[derive(Clone, Copy)]
pub(super) enum LoadKey {
    Player(PlayerId),
    Chunk(ChunkKey, u64),
}
// Whole player/prepared values are bounded by the sixteen/eight entry ledgers.
#[allow(clippy::large_enum_variant)]
pub(super) enum LoadResult {
    Player(LoadPoll),
    Chunk(ChunkLoadPoll),
}
pub(super) fn execute<B: DiskBackend>(
    backend: &mut B,
    key: LoadKey,
    deadline: Deadline,
) -> LoadResult {
    let result = if deadline.expired(Instant::now()) {
        Err(ServerError::Timeout {
            operation: Operation::Load,
        })
    } else {
        catch_unwind(AssertUnwindSafe(|| {
            let value = backend.load(match key {
                LoadKey::Player(p) => SaveKey::Player(p),
                LoadKey::Chunk(k, _) => SaveKey::Chunk(k),
            })?;
            match (key, value) {
                (LoadKey::Player(p), LoadedValue::Player(v)) => {
                    if v.player_id.to_bytes() != p.bytes() {
                        return Err(ServerError::InvalidInput {
                            field: "loaded_player",
                        });
                    }
                    Ok(LoadResult::Player(LoadPoll::Loaded(Some(v))))
                }
                (LoadKey::Chunk(k, g), LoadedValue::Chunk(v)) => Ok(LoadResult::Chunk(
                    ChunkLoadPoll::Loaded(Some(PreparedChunk::try_new(k, g, v)?)),
                )),
                _ => Err(internal("store load family")),
            }
        }))
        .unwrap_or_else(|_| Err(internal("store load panic")))
    };
    match result {
        Ok(v) => v,
        Err(ServerError::Io {
            operation: Operation::Load,
            kind: std::io::ErrorKind::NotFound,
        }) => match key {
            LoadKey::Player(_) => LoadResult::Player(LoadPoll::Loaded(None)),
            LoadKey::Chunk(_, _) => LoadResult::Chunk(ChunkLoadPoll::Loaded(None)),
        },
        Err(e) => failed(key, e),
    }
}
fn failed(key: LoadKey, e: ServerError) -> LoadResult {
    match key {
        LoadKey::Player(_) => LoadResult::Player(LoadPoll::Failed(e)),
        LoadKey::Chunk(_, _) => LoadResult::Chunk(ChunkLoadPoll::Failed(e)),
    }
}
// Each lane holds whole results only within its fixed admission ceiling.
#[allow(clippy::large_enum_variant)]
enum State {
    Queued,
    Started(mpsc::Receiver<LoadResult>),
    Ready(LoadResult),
}
struct Record {
    id: u64,
    key: LoadKey,
    deadline: Deadline,
    state: State,
    cancelled: bool,
}
pub(super) struct Loads {
    players: VecDeque<Record>,
    chunks: VecDeque<Record>,
    next_player: u64,
    next_chunk: u64,
    active: usize,
    player_first: bool,
}
impl Loads {
    pub fn new() -> Self {
        Self {
            players: VecDeque::new(),
            chunks: VecDeque::new(),
            next_player: 0,
            next_chunk: 0,
            active: 0,
            player_first: true,
        }
    }
    pub fn retained(&self) -> bool {
        !self.players.is_empty() || !self.chunks.is_empty()
    }
    /// Free chunk-load admission slots: queued, started and unconsumed
    /// results all occupy the fixed eight-entry ledger.
    pub fn chunk_slots(&self) -> usize {
        8usize.saturating_sub(self.chunks.len())
    }
    pub fn start_player(&mut self, p: PlayerId, d: Deadline) -> Result<LoginTicket, ServerError> {
        let id = admit(
            &mut self.next_player,
            self.players.len(),
            16,
            Resource::PendingLogins,
            "player load ticket space",
        )?;
        self.players.push_back(Record {
            id,
            key: LoadKey::Player(p),
            deadline: d,
            state: State::Queued,
            cancelled: false,
        });
        LoginTicket::try_from_raw(id)
    }
    pub fn start_chunk(
        &mut self,
        k: ChunkKey,
        g: u64,
        d: Deadline,
    ) -> Result<ChunkRequestId, ServerError> {
        if g == 0 {
            return Err(ServerError::InvalidInput {
                field: "chunk_generation",
            });
        }
        let id = admit(
            &mut self.next_chunk,
            self.chunks.len(),
            8,
            Resource::ChunkResults,
            "chunk load ticket space",
        )?;
        self.chunks.push_back(Record {
            id,
            key: LoadKey::Chunk(k, g),
            deadline: d,
            state: State::Queued,
            cancelled: false,
        });
        ChunkRequestId::try_new(id)
    }
    /// One bounded scan gathers replies and queued identities; dispatch uses those
    /// identities without walking history or revisiting completed payloads.
    fn observe(&mut self) -> (VecDeque<usize>, VecDeque<usize>) {
        let mut candidates = (VecDeque::new(), VecDeque::new());
        for (lane, queued) in [
            (&mut self.players, &mut candidates.0),
            (&mut self.chunks, &mut candidates.1),
        ] {
            let mut index = 0;
            while index < lane.len() {
                let r = &mut lane[index];
                if let State::Started(rx) = &r.state {
                    let result = match rx.try_recv() {
                        Ok(v) => Some(v),
                        Err(mpsc::TryRecvError::Empty) => None,
                        Err(mpsc::TryRecvError::Disconnected) => {
                            Some(failed(r.key, internal("store owner disconnected")))
                        }
                    };
                    if let Some(v) = result {
                        self.active -= 1;
                        r.state = State::Ready(v);
                    }
                }
                if r.cancelled && matches!(r.state, State::Ready(_)) {
                    lane.remove(index);
                    continue;
                }
                if matches!(r.state, State::Queued) {
                    queued.push_back(index);
                }
                index += 1;
            }
        }
        candidates
    }
    pub fn collect(&mut self) {
        self.observe();
    }
    pub fn drive(
        &mut self,
        owner: Option<&Background>,
        mut inline: Option<&mut impl DiskBackend>,
        max_active: usize,
    ) -> usize {
        let (mut p, mut c) = self.observe();
        let mut completed = 0;
        while self.active < max_active {
            let is_player = if self.player_first {
                !p.is_empty() || c.is_empty()
            } else {
                c.is_empty() && !p.is_empty()
            };
            let ids = if is_player { &mut p } else { &mut c };
            let Some(index) = ids.pop_front() else {
                break;
            };
            let lane = if is_player {
                &mut self.players
            } else {
                &mut self.chunks
            };
            let r = &mut lane[index];
            if let Some(b) = owner {
                match b.try_load(r.key, r.deadline) {
                    LoadHandoff::Sent(rx) => {
                        r.state = State::Started(rx);
                        self.active += 1;
                    }
                    LoadHandoff::Full => break,
                    LoadHandoff::Disconnected => {
                        r.state = State::Ready(failed(r.key, internal("store owner disconnected")));
                    }
                }
            } else if let Some(b) = inline.as_deref_mut() {
                r.state = State::Ready(execute(b, r.key, r.deadline));
                completed += 1;
            }
            self.player_first = !is_player;
            if completed == max_active {
                break;
            }
        }
        completed
    }
    pub fn poll_player(&mut self, id: LoginTicket) -> LoadPoll {
        match poll_lane(&mut self.players, &mut self.active, id.get()) {
            Some(LoadResult::Player(v)) => v,
            _ => LoadPoll::Pending,
        }
    }
    pub fn poll_chunk(&mut self, id: ChunkRequestId) -> ChunkLoadPoll {
        match poll_lane(&mut self.chunks, &mut self.active, id.get()) {
            Some(LoadResult::Chunk(v)) => v,
            _ => ChunkLoadPoll::Pending,
        }
    }
    pub fn cancel_player(&mut self, id: LoginTicket) {
        cancel(&mut self.players, id.get());
    }
    pub fn cancel_chunk(&mut self, id: ChunkRequestId) {
        cancel(&mut self.chunks, id.get());
    }
}
pub(super) enum LoadHandoff {
    Sent(mpsc::Receiver<LoadResult>),
    Full,
    Disconnected,
}
fn admit(
    next: &mut u64,
    len: usize,
    cap: usize,
    resource: Resource,
    invariant: &'static str,
) -> Result<u64, ServerError> {
    let id = next.checked_add(1).ok_or(internal(invariant))?;
    if len == cap {
        return Err(ServerError::Capacity {
            resource,
            limit: cap,
            observed: cap + 1,
        });
    }
    *next = id;
    Ok(id)
}
fn poll_lane(lane: &mut VecDeque<Record>, active: &mut usize, id: u64) -> Option<LoadResult> {
    let mut index = 0;
    let mut result = None;
    while index < lane.len() {
        let r = &mut lane[index];
        if let State::Started(rx) = &r.state {
            let value = match rx.try_recv() {
                Ok(v) => Some(v),
                Err(mpsc::TryRecvError::Empty) => None,
                Err(mpsc::TryRecvError::Disconnected) => {
                    Some(failed(r.key, internal("store owner disconnected")))
                }
            };
            if let Some(v) = value {
                *active -= 1;
                r.state = State::Ready(v);
            }
        }
        if matches!(r.state, State::Ready(_)) && (r.cancelled || r.id == id) {
            let r = lane.remove(index).expect("observed completed load");
            if !r.cancelled
                && let State::Ready(v) = r.state
            {
                result = Some(v);
            }
        } else {
            index += 1;
        }
    }
    result
}
fn cancel(lane: &mut VecDeque<Record>, id: u64) {
    if let Some(pos) = lane.iter().position(|r| r.id == id) {
        if matches!(lane[pos].state, State::Started(_)) {
            lane[pos].cancelled = true;
        } else {
            lane.remove(pos);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mornlea_domain::{ChunkPos, Dimension};

    fn player() -> PlayerId {
        PlayerId::try_from_bytes([1, 0, 0, 0, 0, 0, 0x40, 0, 0x80, 0, 0, 0, 0, 0, 0, 0]).unwrap()
    }
    fn key() -> ChunkKey {
        ChunkKey {
            dimension: Dimension::OVERWORLD,
            pos: ChunkPos::new(0, 0),
        }
    }
    #[test]
    fn exhausted_player_identity_never_admits_or_wraps() {
        let mut loads = Loads::new();
        loads.next_player = u64::MAX;
        assert_eq!(
            loads.start_player(player(), Deadline::at(Instant::now())),
            Err(internal("player load ticket space"))
        );
        assert_eq!(loads.next_player, u64::MAX);
        assert!(loads.players.is_empty());
        assert_eq!(
            loads
                .start_chunk(key(), 1, Deadline::at(Instant::now()))
                .unwrap()
                .get(),
            1
        );
    }
    #[test]
    fn zero_generation_precedes_exhausted_chunk_identity() {
        let mut loads = Loads::new();
        loads.next_chunk = u64::MAX;
        assert_eq!(
            loads.start_chunk(key(), 0, Deadline::at(Instant::now())),
            Err(ServerError::InvalidInput {
                field: "chunk_generation"
            })
        );
        assert_eq!(
            loads.start_chunk(key(), 1, Deadline::at(Instant::now())),
            Err(internal("chunk load ticket space"))
        );
        assert_eq!(loads.next_chunk, u64::MAX);
        assert!(loads.chunks.is_empty());
        assert_eq!(
            loads
                .start_player(player(), Deadline::at(Instant::now()))
                .unwrap()
                .get(),
            1
        );
    }
    #[test]
    fn full_owner_channel_preserves_queued_identity_and_active_ceiling() {
        use super::super::background::{Handoff, LifecycleKind, SaveExecutor};
        use crate::core::contracts::{SaveCompletion, SaveRequest, SaveTicket};
        use std::sync::Arc;
        use std::time::Duration;
        struct HeldBackend {
            entered: Option<mpsc::SyncSender<()>>,
            opened: mpsc::Receiver<()>,
        }
        impl DiskBackend for HeldBackend {
            fn load(&mut self, _: SaveKey) -> Result<LoadedValue, ServerError> {
                if let Some(tx) = self.entered.take() {
                    tx.send(()).unwrap();
                    self.opened.recv().unwrap();
                }
                Err(ServerError::Io {
                    operation: Operation::Load,
                    kind: std::io::ErrorKind::NotFound,
                })
            }
            fn write(&mut self, ticket: SaveTicket, request: SaveRequest) -> SaveCompletion {
                SaveCompletion {
                    ticket,
                    snapshots: request.snapshots,
                    submitted: vec![],
                    committed: vec![],
                    error: None,
                }
            }
            fn sync(&mut self) -> Result<(), ServerError> {
                Ok(())
            }
            fn close(&mut self) -> Result<(), ServerError> {
                Ok(())
            }
        }
        let (entered, receive) = mpsc::sync_channel(1);
        let (release, opened) = mpsc::sync_channel(1);
        let mut owner = Background::spawn(
            1,
            HeldBackend {
                entered: Some(entered),
                opened,
            },
            SaveExecutor::try_new().unwrap(),
        )
        .unwrap();
        let d = Deadline::after(Instant::now(), Duration::from_secs(5)).unwrap();
        let mut loads = Loads::new();
        let p1 = loads.start_player(player(), d).unwrap();
        let p2 = loads.start_player(player(), d).unwrap();
        let c = loads.start_chunk(key(), 73, d).unwrap();
        loads.drive(Some(&owner), None::<&mut HeldBackend>, 1);
        let entered_result = receive.recv_timeout(Duration::from_secs(5));
        let active_at_one = loads.active;
        let request = Arc::new(SaveRequest { snapshots: vec![] });
        let save = owner.try_save(SaveTicket::try_from_raw(1).unwrap(), &request, &[]);
        drop(request);
        loads.drive(Some(&owner), None::<&mut HeldBackend>, 2);
        let active_after_full = loads.active;
        let preserved = loads.chunks[0].id == c.get()
            && matches!(loads.chunks[0].key, LoadKey::Chunk(k, 73) if k == key())
            && loads.chunks[0].deadline.instant() == d.instant()
            && matches!(loads.chunks[0].state, State::Queued)
            && loads.players[1].id == p2.get()
            && matches!(loads.players[1].state, State::Queued);
        let released = release.send(());
        // Open the fixture gate before assertions or lifecycle join.
        assert!(entered_result.is_ok());
        assert!(released.is_ok());
        assert!(matches!(save, Handoff::Sent(_)));
        assert_eq!((active_at_one, active_after_full), (1, 1));
        assert!(preserved);
        let until = Instant::now() + Duration::from_secs(5);
        while loads.retained() {
            loads.drive(Some(&owner), None::<&mut HeldBackend>, 2);
            assert!(loads.active <= 2);
            let _ = loads.poll_player(p1);
            let _ = loads.poll_player(p2);
            let _ = loads.poll_chunk(c);
            assert!(Instant::now() < until);
            std::thread::yield_now();
        }
        owner
            .lifecycle(
                LifecycleKind::Close,
                Deadline::after(Instant::now(), Duration::from_secs(5)).unwrap(),
            )
            .unwrap();
    }
}
