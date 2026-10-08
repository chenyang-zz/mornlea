//! Configured companion startup over the actual `DiskStore`, authority, and
//! Agent service constructors.
//!
//! Expected values mirror `packages/server/server/companion_bootstrap_test.go`:
//! the bootstrap merge saves before any runtime starts, inactive records keep
//! their metadata, a retired configuration tombstones once, corrupt or future
//! aggregates refuse before any write, and a failed save stops startup. The
//! namespace lease runs on its own control worker, so a refused acquire never
//! blocks startup or a tick.

use std::collections::BTreeSet;
use std::fs;
use std::net::TcpListener;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use mornlea_domain::{ChunkPos, CompanionId, Dimension};
use mornlea_protocol::{LoginStart, admit_login};
use mornlea_server::agent::lease::ControlPhase;
use mornlea_server::contracts::*;
use mornlea_server::core::{chunk_driver::ChunkDriver, generation_worker::GenerationPool};
use mornlea_server::runtime::RuntimeConfig;
use mornlea_server::runtime::companion::{
    CompanionRuntime, CompanionStartError, CompanionStartupPorts, SystemClock, start_companions,
};
use mornlea_server::state::AuthorityState;
use mornlea_server::store::disk::{DiskOptions, DiskStore};
use mornlea_server::store::io::{DiskIo, IoPhase};
use mornlea_server::store::mailbox::StoreMailbox;
use mornlea_server::store::scheduler::{AutosaveScheduler, SchedulerConfig};
use mornlea_storage::{CompanionBody, Inventory, PlayerId, StoredCompanions};

static ROOTS: AtomicU64 = AtomicU64::new(0);

struct Root(PathBuf);

impl Root {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "runtime-companion-{}-{}",
            std::process::id(),
            ROOTS.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }

    fn companions_file(&self) -> PathBuf {
        self.0.join("companions.ai")
    }
}

impl Drop for Root {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn raw_id(tag: u8) -> [u8; 16] {
    let mut bytes = [0; 16];
    bytes[0] = tag;
    bytes[6] = 0x40;
    bytes[8] = 0x80;
    bytes
}

fn companion(tag: u8) -> CompanionId {
    CompanionId::try_from_bytes(raw_id(tag)).unwrap()
}

fn save_id(tag: u8) -> PlayerId {
    PlayerId::from_bytes(raw_id(tag))
}

fn uuid_text(tag: u8) -> String {
    let hex: String = raw_id(tag).iter().map(|b| format!("{b:02x}")).collect();
    format!(
        "{}-{}-{}-{}-{}",
        &hex[0..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..32]
    )
}

/// A loopback port with nothing listening: every Agent RPC is refused.
fn dead_endpoint() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    format!("http://127.0.0.1:{port}")
}

/// Decodes a real config file. `apiKeyEnv` names `PATH`, which the test
/// process always has, so Go's non-empty check passes; the credential value
/// itself comes from the injected lookup, never from the environment.
fn config(tags: &[(u8, &str)], endpoint: &str) -> RuntimeConfig {
    let companions: Vec<String> = tags
        .iter()
        .map(|(tag, name)| format!(r#"{{"id":"{}","name":"{name}"}}"#, uuid_text(*tag)))
        .collect();
    let text = format!(
        r#"{{"ai":{{"agentService":{{"endpoint":"{endpoint}","apiKeyEnv":"PATH"}},"companions":[{}],"taskTimeoutMinutes":7}}}}"#,
        companions.join(",")
    );
    RuntimeConfig::decode(text.as_bytes()).unwrap()
}

fn options() -> DiskOptions {
    DiskOptions {
        region_handle_cap: 1,
        create: mornlea_storage::Metadata {
            format_version: mornlea_storage::METADATA_CURRENT_VERSION,
            seed: 42,
            spawn_dimension: 0,
            spawn_anchor: mornlea_storage::MetadataChunkPos { x: 0, z: 0 },
            world_time_ticks: 1000,
            day_phase_offset: 0,
            weather_kind: 0,
            weather_ticks_remaining: 0,
            depths_spawn_anchor: mornlea_storage::MetadataChunkPos { x: 0, z: 0 },
            depths_seed_salt: 0,
            difficulty: 0,
        },
    }
}

fn chunk_key() -> ChunkKey {
    ChunkKey {
        dimension: Dimension::OVERWORLD,
        pos: ChunkPos::new(0, 0),
    }
}

fn snapshot(value: SaveValue) -> OwnedSnapshot {
    let (key, revision, bytes) = match &value {
        SaveValue::Companions(body) => (
            SaveKey::Companions,
            body.revision,
            mornlea_storage::companions_encoded_len(body).unwrap(),
        ),
        SaveValue::Chunk(body) => (SaveKey::Chunk(chunk_key()), body.revision, 1),
        _ => panic!("fixture target"),
    };
    OwnedSnapshot::try_new(key, revision, bytes, SaveUrgency::Autosave, value).unwrap()
}

fn as_save(aggregate: StoredCompanions) -> mornlea_storage::CompanionSave {
    mornlea_storage::CompanionSave {
        revision: aggregate.revision,
        agent_namespace_id: aggregate.agent_namespace_id,
        records: aggregate.records,
        lifecycles: aggregate.lifecycles,
        queues: aggregate.queues,
    }
}

fn write(disk: &mut DiskStore, value: SaveValue) {
    let completion = disk.write(
        SaveTicket::try_from_raw(1).unwrap(),
        SaveRequest {
            snapshots: vec![snapshot(value)],
        },
    );
    assert_eq!(completion.error, None);
    assert_eq!(completion.committed.len(), 1);
}

/// Opens a world whose spawn chunk is a flat stone floor.
fn open_world(root: &Root) -> DiskStore {
    let mut disk = DiskStore::open(&root.0, options()).unwrap();
    seed_floor(&mut disk);
    disk
}

fn seed_floor(disk: &mut DiskStore) {
    let flat = mornlea_storage::Chunk {
        sections: (0..24)
            .map(|index| mornlea_storage::ContainerSnapshot {
                kind: mornlea_storage::StorageKind::Single,
                bits: 0,
                single: if index < 9 { 2 } else { 0 },
                palette: Vec::new(),
                packed: Vec::new(),
            })
            .collect(),
        drops: vec![Default::default(); 32],
        furnaces: vec![Default::default(); 32],
        chests: vec![Default::default(); 16],
    };
    write(
        disk,
        SaveValue::Chunk(mornlea_storage::ChunkSave {
            key: mornlea_storage::ChunkKey {
                dimension: 0,
                x: 0,
                z: 0,
            },
            revision: 9,
            chunk: flat,
        }),
    );
}

fn body(tag: u8, x: f32) -> CompanionBody {
    CompanionBody {
        id: save_id(tag),
        dimension: 0,
        position: [x, 65.0, 8.5],
        yaw: 0.0,
        pitch: 0.0,
        inventory: Inventory::default(),
    }
}

/// Deterministic UUIDv4 identities `0x80, 0x81, ...` with a call counter.
struct Identities {
    next: u8,
    calls: usize,
}

impl Identities {
    fn new() -> Self {
        Self {
            next: 0x80,
            calls: 0,
        }
    }

    fn mint(&mut self) -> Result<[u8; 16], ServerError> {
        self.calls += 1;
        let id = raw_id(self.next);
        self.next = self.next.wrapping_add(1);
        Ok(id)
    }
}

/// Builds a canonical v5 aggregate through the production merge: `active`
/// bodies are configured, `retired` bodies were configured earlier (at most
/// four at a time) and are now inactive tombstones.
fn seeded(active: &[CompanionBody], retired: &[CompanionBody]) -> StoredCompanions {
    let mut ids = Identities::new();
    ids.next = 0xa0;
    let mut generate = || ids.mint().map(PlayerId::from_bytes);
    let mut aggregate = StoredCompanions::default();
    for configured in retired.chunks(4).chain(std::iter::once(active)) {
        aggregate =
            mornlea_storage::merge_companions_v5(&aggregate, configured, Some(&mut generate))
                .unwrap()
                .0;
    }
    aggregate
}

fn authority(disk: &DiskStore) -> AuthorityState {
    let mut state = AuthorityState::try_new_with_metadata(
        ServerLimits::try_new(8, 4096, 512, 64, 64, 1_048_576).unwrap(),
        disk.metadata().clone(),
    )
    .unwrap();
    state.enable_source_player_restoration(1).unwrap();
    state.enable_live_chunks().unwrap();
    state.enable_actor_saves().unwrap();
    state.enable_player_persistence().unwrap();
    state
}

fn credential(_: &str) -> Option<String> {
    Some("runtime-companion-token".to_owned())
}

fn start(
    config: &RuntimeConfig,
    disk: &mut DiskStore,
    state: &mut AuthorityState,
    ids: &mut Identities,
) -> Result<Option<CompanionRuntime>, CompanionStartError> {
    let mut mint = || ids.mint();
    start_companions(
        config.ai(),
        disk,
        state,
        CompanionStartupPorts {
            clock: Arc::new(SystemClock),
            credential: &credential,
            identity: &mut mint,
        },
    )
}

fn deadline() -> Deadline {
    Deadline::after(Instant::now(), Duration::from_secs(10)).unwrap()
}

fn load(disk: &mut DiskStore) -> StoredCompanions {
    let LoadedValue::Companions(aggregate) = disk.load(SaveKey::Companions).unwrap() else {
        panic!("companions")
    };
    aggregate
}

fn companion_lifecycles(state: &AuthorityState) -> Vec<(ActorKey, ActorLifecycle)> {
    state
        .residents()
        .actors
        .iter()
        .filter(|actor| matches!(actor.key, ActorKey::Companion(_)))
        .map(|actor| (actor.key, actor.lifecycle))
        .collect()
}

/// Joins one new player through the persistent login path after startup.
fn join_player(state: &mut AuthorityState) -> SessionKey {
    let mut bytes = [0x31; 16];
    bytes[6] = 0x40;
    bytes[8] = 0x80;
    let id = mornlea_domain::PlayerId::try_from_bytes(bytes).unwrap();
    let start = LoginStart::new(id, "Ada", 8).unwrap().encode().unwrap();
    let login = admit_login(LoginStart::decode_inbound(&start).unwrap()).unwrap();
    let session = state.prepare(login, TransportKind::Memory).unwrap();
    assert!(!state.prepare_player_cache(session).unwrap());
    state.install(session, None).unwrap();
    state.activate(session).unwrap();
    session
}

/// Hands the world store to the background scheduler, loads the spawn
/// chunk through the actual chunk driver, and runs the first tick.
fn first_tick(state: &mut AuthorityState, disk: DiskStore) -> AutosaveScheduler<DiskStore> {
    let mut store = AutosaveScheduler::try_new(
        SchedulerConfig::default(),
        StoreMailbox::try_new_background(
            StoreLimits::try_new(2, 16, 3, 3, 3, 1, 8, 4_194_304).unwrap(),
            disk,
        )
        .unwrap(),
    )
    .unwrap();
    state
        .replace_chunk_wants(BTreeSet::from([chunk_key()]))
        .unwrap();
    let mut driver = ChunkDriver::new();
    let mut pool = GenerationPool::try_new(42, false, 1).unwrap();
    driver
        .start_load(state, &mut store, chunk_key(), deadline())
        .unwrap();
    let until = deadline();
    loop {
        store.drive_workers();
        let report = driver.poll(state, &mut store, &mut pool);
        assert_eq!(report.first_error, None);
        if report.retained == 0 {
            break;
        }
        assert!(!until.expired(Instant::now()));
        thread::yield_now();
    }
    state.advance_tick(TickBudget::full()).unwrap();
    pool.close(deadline()).unwrap();
    store
}

fn close(mut runtime: CompanionRuntime) {
    runtime.agent_mut().close_until(deadline()).unwrap();
    assert_eq!(runtime.agent().lease_phase(), ControlPhase::Closed);
}

/// Go `TestNewHostSkipsCompanionStoreWhenAIDisabled`: no configuration and
/// no aggregate start nothing and write nothing.
#[test]
fn unconfigured_world_without_aggregate_starts_nothing() {
    let root = Root::new();
    let mut disk = open_world(&root);
    let mut state = authority(&disk);
    let mut ids = Identities::new();
    let runtime = start(&RuntimeConfig::defaults(), &mut disk, &mut state, &mut ids).unwrap();
    assert!(runtime.is_none());
    assert!(!root.companions_file().exists());
    assert!(!state.companion_persistence_enabled());
    assert_eq!(ids.calls, 0);
    disk.close().unwrap();
}

/// Go `TestNewHostRestoresConfiguredBodiesAndPreservesInactiveRecords`, with
/// two configured companions: the canonical v5 aggregate is not rewritten,
/// the inactive record keeps its metadata, and the first tick activates both
/// configured bodies.
#[test]
fn two_configured_companions_restore_from_v5_save_and_activate_on_first_tick() {
    let root = Root::new();
    let mut disk = open_world(&root);
    let aggregate = seeded(&[body(1, 8.5), body(2, 9.5)], &[body(3, 20.5)]);
    write(&mut disk, SaveValue::Companions(as_save(aggregate.clone())));
    let before = fs::read(root.companions_file()).unwrap();
    let mut state = authority(&disk);
    let mut ids = Identities::new();
    let runtime = start(
        &config(&[(1, "Mira"), (2, "Tove")], &dead_endpoint()),
        &mut disk,
        &mut state,
        &mut ids,
    )
    .unwrap()
    .expect("configured runtime");

    assert_eq!(fs::read(root.companions_file()).unwrap(), before);
    assert_eq!(load(&mut disk), aggregate);
    assert!(state.companion_persistence_enabled());
    assert_eq!(
        state.actor_save_current(&SaveKey::Companions).unwrap().0,
        aggregate.revision
    );
    assert_eq!(
        runtime
            .definitions()
            .iter()
            .map(|(id, name)| (*id, name.as_str().to_owned()))
            .collect::<Vec<_>>(),
        vec![(companion(1), "Mira".into()), (companion(2), "Tove".into())]
    );
    assert_eq!(runtime.task_timeout_minutes(), 7);
    assert_eq!(
        runtime.agent().namespace().bytes(),
        aggregate.agent_namespace_id.to_bytes()
    );
    // Only the client instance identity is minted; the save needed none.
    assert_eq!(ids.calls, 1);
    assert_eq!(runtime.agent().client().bytes(), raw_id(0x80));
    let pending = vec![
        (ActorKey::Companion(companion(1)), ActorLifecycle::Pending),
        (ActorKey::Companion(companion(2)), ActorLifecycle::Pending),
    ];
    assert_eq!(companion_lifecycles(&state), pending);

    let mut store = first_tick(&mut state, disk);
    assert_eq!(
        companion_lifecycles(&state),
        vec![
            (ActorKey::Companion(companion(1)), ActorLifecycle::Active),
            (ActorKey::Companion(companion(2)), ActorLifecycle::Active),
        ]
    );
    close(runtime);
    store.close(deadline()).unwrap();
}

/// Go `TestNewHostAddsConfiguredIDWithoutDeletingInactiveRecords`: a stored
/// companion with no definition is retired to metadata only, the configured
/// one is added at the spawn anchor, the merge saves once, and startup
/// proceeds to activate only the configured companion.
#[test]
fn missing_definition_keeps_metadata_and_does_not_block_startup() {
    let root = Root::new();
    let mut disk = open_world(&root);
    let stored = body(4, 20.5);
    let aggregate = seeded(std::slice::from_ref(&stored), &[]);
    write(&mut disk, SaveValue::Companions(as_save(aggregate.clone())));
    let mut state = authority(&disk);
    let mut ids = Identities::new();
    let runtime = start(
        &config(&[(3, "Mira")], &dead_endpoint()),
        &mut disk,
        &mut state,
        &mut ids,
    )
    .unwrap()
    .expect("configured runtime");

    let saved = load(&mut disk);
    assert_eq!(saved.revision, aggregate.revision + 1);
    assert_eq!(saved.agent_namespace_id, aggregate.agent_namespace_id);
    assert_eq!(saved.records.len(), 2);
    assert!(saved.records.contains(&stored));
    let mut anchored = body(3, 0.5);
    anchored.position = [0.5, 321.0, 0.5];
    assert!(saved.records.contains(&anchored));
    let retired = saved
        .lifecycles
        .iter()
        .find(|lifecycle| lifecycle.id == save_id(4))
        .unwrap();
    assert!(!retired.active);
    assert!(retired.tombstone_operation_id.is_valid());
    assert!(
        saved
            .lifecycles
            .iter()
            .any(|lifecycle| lifecycle.id == save_id(3) && lifecycle.active)
    );
    assert_eq!(
        state.actor_save_current(&SaveKey::Companions).unwrap().0,
        saved.revision
    );

    let mut store = first_tick(&mut state, disk);
    assert_eq!(
        companion_lifecycles(&state),
        vec![(ActorKey::Companion(companion(3)), ActorLifecycle::Active)]
    );
    close(runtime);
    store.close(deadline()).unwrap();
}

/// Go `TestNewHostPersistsCompanionIdentityBeforeRuntimeConstruction`: a new
/// world saves the generated namespace and anchor body before the Agent
/// client identity is minted and before the authority sees the aggregate.
#[test]
fn new_world_persists_identity_before_runtime_construction() {
    let root = Root::new();
    let mut disk = open_world(&root);
    let mut state = authority(&disk);
    let mut ids = Identities::new();
    let runtime = start(
        &config(&[(1, "Mira")], &dead_endpoint()),
        &mut disk,
        &mut state,
        &mut ids,
    )
    .unwrap()
    .expect("configured runtime");
    let saved = load(&mut disk);
    let mut anchored = body(1, 0.5);
    anchored.position = [0.5, 321.0, 0.5];
    assert_eq!(saved.revision, 1);
    assert_eq!(saved.agent_namespace_id, save_id(0x80));
    assert_eq!(saved.records, vec![anchored]);
    assert_eq!(saved.lifecycles.len(), 1);
    assert!(saved.lifecycles[0].active);
    assert_eq!(saved.lifecycles[0].memory_epoch, 1);
    assert_eq!(saved.lifecycles[0].memory_revision, 0);
    assert_eq!(runtime.agent().namespace().bytes(), raw_id(0x80));
    assert_eq!(runtime.agent().client().bytes(), raw_id(0x81));
    assert!(state.companion_persistence_enabled());
    close(runtime);
    disk.close().unwrap();
}

/// Fails the companion payload sync, never replacing the disk backend.
struct FailCompanionSync {
    armed: Arc<AtomicBool>,
    companion_payload: bool,
}

impl DiskIo for FailCompanionSync {
    fn write(&mut self, file: &mut fs::File, bytes: &[u8]) -> std::io::Result<usize> {
        self.companion_payload = bytes.starts_with(b"MCAI");
        std::io::Write::write(file, bytes)
    }

    fn boundary(&mut self, point: IoFaultPoint, phase: IoPhase) -> std::io::Result<()> {
        if point == IoFaultPoint::TempSync
            && phase == IoPhase::Before
            && self.companion_payload
            && self.armed.swap(false, Ordering::AcqRel)
        {
            return Err(std::io::ErrorKind::PermissionDenied.into());
        }
        Ok(())
    }
}

/// Go `TestNewHostBootstrapSaveFailureStopsBeforeRuntimeConstruction`.
#[test]
fn bootstrap_save_failure_stops_startup_before_agent_and_authority() {
    let root = Root::new();
    drop(open_world(&root));
    let armed = Arc::new(AtomicBool::new(true));
    let hook = armed.clone();
    let mut disk = DiskStore::with_io(
        &root.0,
        options(),
        Box::new(move || {
            Box::new(FailCompanionSync {
                armed: hook.clone(),
                companion_payload: false,
            })
        }),
    )
    .unwrap();
    let mut state = authority(&disk);
    let mut ids = Identities::new();
    let error = start(
        &config(&[(1, "Mira")], &dead_endpoint()),
        &mut disk,
        &mut state,
        &mut ids,
    )
    .err()
    .expect("save failure refuses startup");
    assert_eq!(
        error,
        CompanionStartError::Save(ServerError::Io {
            operation: Operation::SyncPayload,
            kind: std::io::ErrorKind::PermissionDenied,
        })
    );
    assert!(!armed.load(Ordering::Acquire));
    // The namespace was minted for the merge; no Agent client identity was.
    assert_eq!(ids.calls, 1);
    assert!(!state.companion_persistence_enabled());
    assert!(companion_lifecycles(&state).is_empty());
    assert!(disk.load(SaveKey::Companions).is_err());
    disk.close().unwrap();
}

/// Go `TestNewHostRejectsCorruptOrFutureCompanionStoreBeforeWorkersStart`.
#[test]
fn corrupt_or_future_aggregate_refuses_before_any_write() {
    for future in [false, true] {
        let root = Root::new();
        let mut disk = open_world(&root);
        write(
            &mut disk,
            SaveValue::Companions(as_save(seeded(&[body(1, 8.5)], &[]))),
        );
        let mut bytes = fs::read(root.companions_file()).unwrap();
        if future {
            bytes[4..8].copy_from_slice(&[0xff; 4]);
        } else {
            *bytes.last_mut().unwrap() ^= 0xff;
        }
        fs::write(root.companions_file(), &bytes).unwrap();
        let mut state = authority(&disk);
        let mut ids = Identities::new();
        let error = start(
            &config(&[(1, "Mira")], &dead_endpoint()),
            &mut disk,
            &mut state,
            &mut ids,
        )
        .err()
        .expect("unreadable aggregate refuses startup");
        let kind = if future {
            StorageFailure::FutureVersion
        } else {
            StorageFailure::Corrupt
        };
        assert_eq!(
            error,
            CompanionStartError::Load(ServerError::Storage {
                family: "companions",
                kind,
            })
        );
        assert_eq!(fs::read(root.companions_file()).unwrap(), bytes);
        assert_eq!(ids.calls, 0);
        assert!(!state.companion_persistence_enabled());
        disk.close().unwrap();
    }
}

/// Go `TestNewHostRejectsSixtyFifthDistinctStoredOrNewCompanion`.
#[test]
fn sixty_fifth_distinct_companion_refuses_without_write() {
    let root = Root::new();
    let mut disk = open_world(&root);
    let stored: Vec<CompanionBody> = (1..=64).map(|tag| body(tag, 8.5)).collect();
    let aggregate = seeded(&stored[..4], &stored[4..]);
    write(&mut disk, SaveValue::Companions(as_save(aggregate)));
    let before = fs::read(root.companions_file()).unwrap();
    let mut state = authority(&disk);
    let mut ids = Identities::new();
    let error = start(
        &config(&[(65, "Mira")], &dead_endpoint()),
        &mut disk,
        &mut state,
        &mut ids,
    )
    .err()
    .expect("65th companion refuses");
    assert!(matches!(error, CompanionStartError::Merge(_)), "{error}");
    assert_eq!(fs::read(root.companions_file()).unwrap(), before);
    assert!(!state.companion_persistence_enabled());
    disk.close().unwrap();
}

/// Go `TestNewHostRetiresExistingCompanionsWhenConfigEmpty` and
/// `TestNewHostDoesNotRepeatInactiveRetirement`.
#[test]
fn unconfigured_world_retires_active_companions_exactly_once() {
    let root = Root::new();
    let mut disk = open_world(&root);
    let stored = body(1, 4.5);
    let aggregate = seeded(std::slice::from_ref(&stored), &[]);
    write(&mut disk, SaveValue::Companions(as_save(aggregate.clone())));
    let mut state = authority(&disk);
    let mut ids = Identities::new();
    assert!(
        start(&RuntimeConfig::defaults(), &mut disk, &mut state, &mut ids)
            .unwrap()
            .is_none()
    );
    let retired = load(&mut disk);
    assert_eq!(retired.revision, aggregate.revision + 1);
    assert_eq!(retired.records, vec![stored]);
    assert_eq!(retired.lifecycles.len(), 1);
    assert!(!retired.lifecycles[0].active);
    assert!(retired.lifecycles[0].tombstone_operation_id.is_valid());
    assert_eq!(ids.calls, 1);
    assert!(!state.companion_persistence_enabled());

    let before = fs::read(root.companions_file()).unwrap();
    let mut again = authority(&disk);
    assert!(
        start(&RuntimeConfig::defaults(), &mut disk, &mut again, &mut ids)
            .unwrap()
            .is_none()
    );
    assert_eq!(fs::read(root.companions_file()).unwrap(), before);
    assert_eq!(ids.calls, 1);
    disk.close().unwrap();
}

/// Go `bootstrapCompanionPersistence`: an empty credential refuses before
/// any companion save I/O.
#[test]
fn missing_credential_refuses_before_save_io() {
    let root = Root::new();
    let mut disk = open_world(&root);
    let mut state = authority(&disk);
    let mut mint = || -> Result<[u8; 16], ServerError> { panic!("no identity before credential") };
    let error = start_companions(
        config(&[(1, "Mira")], &dead_endpoint()).ai(),
        &mut disk,
        &mut state,
        CompanionStartupPorts {
            clock: Arc::new(SystemClock),
            credential: &|_| None,
            identity: &mut mint,
        },
    )
    .err()
    .expect("empty credential refuses");
    assert_eq!(error, CompanionStartError::MissingCredential);
    assert!(!root.companions_file().exists());
    assert!(!state.companion_persistence_enabled());
    disk.close().unwrap();
}

/// A refused namespace lease acquire stays on the control worker: startup
/// succeeds, ticks keep running, every configured companion still activates,
/// a player joining afterwards goes Active, and the worker keeps retrying
/// without holding a lease.
#[test]
fn refused_lease_acquire_never_blocks_startup_or_ticks() {
    let root = Root::new();
    let mut disk = open_world(&root);
    let mut state = authority(&disk);
    let mut ids = Identities::new();
    let runtime = start(
        &config(&[(1, "Mira"), (2, "Tove")], &dead_endpoint()),
        &mut disk,
        &mut state,
        &mut ids,
    )
    .unwrap()
    .expect("configured runtime");
    let until = Instant::now() + Duration::from_secs(10);
    while runtime.agent().lease_phase() != ControlPhase::Absent
        || !runtime.agent().lease_worker_running()
    {
        assert!(Instant::now() < until, "first acquire never settled");
        thread::sleep(Duration::from_millis(5));
    }
    let mut store = first_tick(&mut state, disk);
    let player = join_player(&mut state);
    for _ in 0..3 {
        state.advance_tick(TickBudget::full()).unwrap();
    }
    assert_eq!(
        companion_lifecycles(&state),
        vec![
            (ActorKey::Companion(companion(1)), ActorLifecycle::Active),
            (ActorKey::Companion(companion(2)), ActorLifecycle::Active),
        ]
    );
    assert_eq!(
        state
            .residents()
            .actors
            .iter()
            .filter(|actor| matches!(actor.key, ActorKey::Player(_)))
            .map(|actor| (actor.key, actor.lifecycle))
            .collect::<Vec<_>>(),
        vec![(ActorKey::Player(player), ActorLifecycle::Active)]
    );
    assert_eq!(runtime.agent().current_lease(), None);
    assert!(runtime.agent().lease_worker_running());
    close(runtime);
    store.close(deadline()).unwrap();
}

/// An endpoint Go's config accepts but the loopback wire cannot dial refuses
/// startup typed, before any companion save I/O or authority handoff.
#[test]
fn undialable_endpoint_refuses_before_authority_handoff() {
    let root = Root::new();
    let mut disk = open_world(&root);
    let mut state = authority(&disk);
    let mut ids = Identities::new();
    let error = start(
        &config(&[(1, "Mira")], "http://x[::1]:80"),
        &mut disk,
        &mut state,
        &mut ids,
    )
    .err()
    .expect("undialable endpoint refuses");
    assert!(matches!(error, CompanionStartError::Agent(_)), "{error}");
    assert!(!root.companions_file().exists());
    assert_eq!(ids.calls, 0);
    assert!(!state.companion_persistence_enabled());
    assert!(companion_lifecycles(&state).is_empty());
    disk.close().unwrap();
}
