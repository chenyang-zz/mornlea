//! Real standalone files exercise the publication and durability boundaries.

use mornlea_domain::PlayerId;
use mornlea_server::contracts::{
    IoFaultPoint, LoadedValue, Operation, OwnedSnapshot, SaveKey, SaveUrgency, SaveValue,
    ServerError, StorageFailure,
};
use mornlea_server::store::atomic_file::AtomicFiles;
use mornlea_server::store::io::{DiskIo, IoCancellation, IoPhase};
use mornlea_storage::{
    CompanionSave, HostileMobsSave, Inventory, ItemStack, METADATA_CURRENT_VERSION, Metadata,
    MetadataChunkPos, PassiveMobsSave, PlayerLocation, PlayerSave,
};
use std::fs::{self, File};
use std::io::{self, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

static NEXT_DIR: AtomicU64 = AtomicU64::new(0);

struct TestRoot(PathBuf);
impl TestRoot {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "mornlea-atomic-{}-{}",
            std::process::id(),
            NEXT_DIR.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
}
impl Drop for TestRoot {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}

fn uuid(tag: u8) -> [u8; 16] {
    let mut id = [0; 16];
    id[0] = tag;
    id[6] = 0x40;
    id[8] = 0x80;
    id
}
fn metadata(time: u64, weather: u8) -> Metadata {
    Metadata {
        format_version: METADATA_CURRENT_VERSION,
        seed: 7,
        spawn_dimension: 0,
        spawn_anchor: MetadataChunkPos { x: 0, z: 0 },
        world_time_ticks: time,
        day_phase_offset: 0,
        weather_kind: weather,
        weather_ticks_remaining: 11,
        depths_spawn_anchor: MetadataChunkPos { x: 0, z: 0 },
        depths_seed_salt: 0,
        difficulty: 0,
    }
}
fn snapshot(value: SaveValue, revision: u64) -> OwnedSnapshot {
    let key = match &value {
        SaveValue::Player(_) => SaveKey::Player(PlayerId::try_from_bytes(uuid(1)).unwrap()),
        SaveValue::Companions(_) => SaveKey::Companions,
        SaveValue::Hostiles(_) => SaveKey::Hostiles,
        SaveValue::Passives(_) => SaveKey::Passives,
        SaveValue::Metadata(_) => SaveKey::Metadata,
        SaveValue::Chunk(_) => unreachable!(),
    };
    OwnedSnapshot::try_new(key, revision, 1024, SaveUrgency::Autosave, value).unwrap()
}
fn player(revision: u64, health: u8) -> OwnedSnapshot {
    snapshot(
        SaveValue::Player(PlayerSave {
            player_id: mornlea_storage::PlayerId::from_bytes(uuid(1)),
            revision,
            display_name: "Tester".to_owned(),
            current: PlayerLocation {
                dimension: 0,
                position: [0.0, 64.0, 0.0],
            },
            yaw: 0.0,
            pitch: 0.0,
            safe: None,
            inventory: Inventory::default(),
            health,
            hunger: 20,
            saturation_milli: 5000,
            exhaustion_milli: 0,
            respawn_present: false,
            respawn_position: [0.0; 3],
            respawn_dimension: 0,
            armor: [ItemStack::default(); 4],
        }),
        revision,
    )
}
fn all(revision: u64) -> Vec<OwnedSnapshot> {
    vec![
        player(revision, 20),
        snapshot(
            SaveValue::Companions(CompanionSave {
                revision,
                agent_namespace_id: mornlea_storage::PlayerId::from_bytes(uuid(2)),
                records: vec![],
                lifecycles: vec![],
                queues: vec![],
            }),
            revision,
        ),
        snapshot(
            SaveValue::Hostiles(HostileMobsSave {
                revision,
                records: vec![],
            }),
            revision,
        ),
        snapshot(
            SaveValue::Passives(PassiveMobsSave {
                revision,
                records: vec![],
            }),
            revision,
        ),
        snapshot(SaveValue::Metadata(metadata(6993 + revision, 0)), revision),
    ]
}
fn canonical(root: &TestRoot, key: &SaveKey) -> PathBuf {
    root.0.join(match key {
        SaveKey::Player(_) => "players/01000000-0000-4000-8000-000000000000.player",
        SaveKey::Companions => "companions.ai",
        SaveKey::Hostiles => "hostile_mobs.bin",
        SaveKey::Passives => "passive_mobs.bin",
        SaveKey::Metadata => "world.meta",
        SaveKey::Chunk(_) => unreachable!(),
    })
}

fn player_path(root: &TestRoot, id: PlayerId) -> PathBuf {
    let b = id.bytes();
    root.0.join("players").join(format!(
        "{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}.player",
        b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7],
        b[8], b[9], b[10], b[11], b[12], b[13], b[14], b[15]
    ))
}

#[derive(Clone)]
struct Fault(Arc<Mutex<Option<(IoFaultPoint, IoPhase)>>>);
impl DiskIo for Fault {
    fn boundary(&mut self, point: IoFaultPoint, phase: IoPhase) -> io::Result<()> {
        if *self.0.lock().unwrap() == Some((point, phase)) {
            Err(io::ErrorKind::PermissionDenied.into())
        } else {
            Ok(())
        }
    }
}
struct ZeroWrite;
impl DiskIo for ZeroWrite {
    fn write(&mut self, _file: &mut File, _bytes: &[u8]) -> io::Result<usize> {
        Ok(0)
    }
}
struct ShortWrite;
impl DiskIo for ShortWrite {
    fn write(&mut self, file: &mut File, bytes: &[u8]) -> io::Result<usize> {
        file.write(&bytes[..bytes.len().min(3)])
    }
}
struct CloseFault {
    after_temp_sync: bool,
}
impl DiskIo for CloseFault {
    fn boundary(&mut self, point: IoFaultPoint, phase: IoPhase) -> io::Result<()> {
        if point == IoFaultPoint::TempSync && phase == IoPhase::After {
            self.after_temp_sync = true;
        }
        Ok(())
    }
    fn close(&mut self, file: File) -> io::Result<()> {
        drop(file);
        if self.after_temp_sync {
            Err(io::ErrorKind::PermissionDenied.into())
        } else {
            Ok(())
        }
    }
}

struct CancelAt {
    token: IoCancellation,
    point: IoFaultPoint,
    phase: IoPhase,
}
impl DiskIo for CancelAt {
    fn boundary(&mut self, point: IoFaultPoint, phase: IoPhase) -> io::Result<()> {
        if self.point == point && self.phase == phase {
            self.token.cancel();
        }
        Ok(())
    }
}

#[test]
fn all_families_round_trip() {
    let root = TestRoot::new();
    let mut files = AtomicFiles::open(&root.0).unwrap();
    assert!(root.0.join("players").is_dir());
    for item in all(7) {
        assert_eq!(files.save(&item, &IoCancellation::new()), Ok(7));
        assert!(canonical(&root, &item.key).exists());
        let loaded = files.load(item.key.clone()).unwrap();
        match (item.key, loaded) {
            (SaveKey::Player(_), LoadedValue::Player(value)) => assert_eq!(value.revision, 7),
            (SaveKey::Companions, LoadedValue::Companions(value)) => {
                assert_eq!(value.revision, 7);
                assert_eq!(
                    value.source_schema,
                    mornlea_storage::COMPANION_CURRENT_SCHEMA
                );
            }
            (SaveKey::Hostiles, LoadedValue::Hostiles(value)) => assert_eq!(value.revision, 7),
            (SaveKey::Passives, LoadedValue::Passives(value)) => assert_eq!(value.revision, 7),
            (SaveKey::Metadata, LoadedValue::Metadata(value)) => {
                assert_eq!(value.world_time_ticks, 7000)
            }
            other => panic!("wrong loaded family: {other:?}"),
        }
    }
    files.sync().unwrap();
    files.close().unwrap();
}

#[test]
fn each_failure_complete_file() {
    for point in [
        IoFaultPoint::TempWrite,
        IoFaultPoint::TempSync,
        IoFaultPoint::Rename,
        IoFaultPoint::DirectorySync,
    ] {
        for phase in [IoPhase::Before, IoPhase::After] {
            let root = TestRoot::new();
            let state = Arc::new(Mutex::new(None));
            let mut files = AtomicFiles::with_io(&root.0, Box::new(Fault(state.clone()))).unwrap();
            for old in all(7) {
                files.save(&old, &IoCancellation::new()).unwrap();
            }
            *state.lock().unwrap() = Some((point, phase));
            for newer in all(8) {
                let old_bytes = fs::read(canonical(&root, &newer.key)).unwrap();
                assert!(
                    files.save(&newer, &IoCancellation::new()).is_err(),
                    "{point:?} {phase:?}"
                );
                let actual = fs::read(canonical(&root, &newer.key)).unwrap();
                if matches!(point, IoFaultPoint::TempWrite | IoFaultPoint::TempSync)
                    || (point == IoFaultPoint::Rename && phase == IoPhase::Before)
                {
                    assert_eq!(actual, old_bytes);
                } else {
                    assert_ne!(actual, old_bytes);
                }
                *state.lock().unwrap() = None;
                files.save(&newer, &IoCancellation::new()).unwrap();
                *state.lock().unwrap() = Some((point, phase));
            }
        }
    }
}

#[test]
fn future_and_corrupt_preserved() {
    let root = TestRoot::new();
    let mut files = AtomicFiles::open(&root.0).unwrap();
    for item in all(7) {
        let path = canonical(&root, &item.key);
        fs::write(&path, b"corrupt").unwrap();
        assert_eq!(
            files.save(&item, &IoCancellation::new()),
            Err(ServerError::Storage {
                family: match item.key {
                    SaveKey::Player(_) => "player",
                    SaveKey::Companions => "companions",
                    SaveKey::Hostiles => "hostiles",
                    SaveKey::Passives => "passives",
                    SaveKey::Metadata => "metadata",
                    _ => unreachable!(),
                },
                kind: StorageFailure::Corrupt
            })
        );
        assert_eq!(fs::read(&path).unwrap(), b"corrupt");
    }
    let path = canonical(&root, &SaveKey::Metadata);
    let mut future = mornlea_storage::encode_world_metadata(&metadata(1, 0)).unwrap();
    future[4..8].copy_from_slice(&(METADATA_CURRENT_VERSION + 1).to_le_bytes());
    fs::write(&path, &future).unwrap();
    assert_eq!(
        files.load(SaveKey::Metadata),
        Err(ServerError::Storage {
            family: "metadata",
            kind: StorageFailure::FutureVersion
        })
    );
    assert!(
        files
            .save(
                &snapshot(SaveValue::Metadata(metadata(2, 0)), 8),
                &IoCancellation::new()
            )
            .is_err()
    );
    assert_eq!(fs::read(path).unwrap(), future);

    let mut oversized_future = future.clone();
    oversized_future.extend([0; 100]);
    let path = canonical(&root, &SaveKey::Metadata);
    fs::write(&path, &oversized_future).unwrap();
    assert_eq!(
        files.load(SaveKey::Metadata),
        Err(ServerError::Storage {
            family: "metadata",
            kind: StorageFailure::FutureVersion
        })
    );
    assert_eq!(fs::read(path).unwrap(), oversized_future);
}

#[test]
fn legacy_load_keeps_migration_facts_without_write() {
    let root = TestRoot::new();
    let mut files = AtomicFiles::open(&root.0).unwrap();
    let fixtures = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../server/storage");
    let player_bytes = fs::read(fixtures.join("player/testdata/player-v8.bin")).unwrap();
    let id = PlayerId::try_from_bytes(player_bytes[12..28].try_into().unwrap()).unwrap();
    let path = player_path(&root, id);
    fs::write(&path, &player_bytes).unwrap();
    assert!(
        matches!(files.load(SaveKey::Player(id)).unwrap(), LoadedValue::Player(value) if value.needs_rewrite)
    );
    assert_eq!(fs::read(path).unwrap(), player_bytes);

    let companion_bytes = fs::read(fixtures.join("companion/testdata/companions-v4.bin")).unwrap();
    let path = canonical(&root, &SaveKey::Companions);
    fs::write(&path, &companion_bytes).unwrap();
    assert!(
        matches!(files.load(SaveKey::Companions).unwrap(), LoadedValue::Companions(value) if value.source_schema == mornlea_storage::COMPANION_SCHEMA_V4)
    );
    assert_eq!(fs::read(path).unwrap(), companion_bytes);
}

#[test]
fn equal_revision_conflict() {
    let root = TestRoot::new();
    let mut files = AtomicFiles::open(&root.0).unwrap();
    let old = player(7, 20);
    files.save(&old, &IoCancellation::new()).unwrap();
    let before = fs::read(canonical(&root, &old.key)).unwrap();
    assert_eq!(files.save(&old, &IoCancellation::new()), Ok(7));
    assert_eq!(
        files.save(&player(7, 19), &IoCancellation::new()),
        Err(ServerError::InvalidInput { field: "revision" })
    );
    assert_eq!(
        files.save(&player(6, 20), &IoCancellation::new()),
        Err(ServerError::InvalidInput { field: "revision" })
    );
    assert_eq!(fs::read(canonical(&root, &old.key)).unwrap(), before);
}

#[test]
fn directory_sync_error_not_success() {
    let root = TestRoot::new();
    let state = Arc::new(Mutex::new(None));
    let mut files = AtomicFiles::with_io(&root.0, Box::new(Fault(state.clone()))).unwrap();
    let first = player(7, 20);
    files.save(&first, &IoCancellation::new()).unwrap();
    *state.lock().unwrap() = Some((IoFaultPoint::DirectorySync, IoPhase::After));
    let second = player(8, 19);
    assert!(files.save(&second, &IoCancellation::new()).is_err());
    assert_eq!(
        match files.load(second.key.clone()).unwrap() {
            LoadedValue::Player(p) => p.revision,
            _ => unreachable!(),
        },
        8
    );
    assert!(files.save(&second, &IoCancellation::new()).is_err());
    *state.lock().unwrap() = None;
    assert_eq!(files.save(&second, &IoCancellation::new()), Ok(8));
}

#[test]
fn metadata_same_time_new_weather() {
    let root = TestRoot::new();
    let mut files = AtomicFiles::open(&root.0).unwrap();
    let first = snapshot(SaveValue::Metadata(metadata(7000, 0)), 7);
    let second = snapshot(SaveValue::Metadata(metadata(7000, 1)), 8);
    files.save(&first, &IoCancellation::new()).unwrap();
    files.save(&second, &IoCancellation::new()).unwrap();
    assert!(
        matches!(files.load(SaveKey::Metadata).unwrap(), LoadedValue::Metadata(m) if m.world_time_ticks == 7000 && m.weather_kind == 1)
    );
    let mut invalid = second;
    invalid.revision = 0;
    assert_eq!(
        files.save(&invalid, &IoCancellation::new()),
        Err(ServerError::InvalidInput {
            field: "metadata_sequence"
        })
    );
}

#[test]
fn metadata_sequence_follows_physical_publication() {
    let root = TestRoot::new();
    let state = Arc::new(Mutex::new(None));
    let mut files = AtomicFiles::with_io(&root.0, Box::new(Fault(state.clone()))).unwrap();
    let seven = snapshot(SaveValue::Metadata(metadata(7000, 0)), 7);
    let eight = snapshot(SaveValue::Metadata(metadata(7000, 1)), 8);
    let nine = snapshot(SaveValue::Metadata(metadata(7000, 2)), 9);
    files.save(&eight, &IoCancellation::new()).unwrap();
    let eight_bytes = fs::read(canonical(&root, &SaveKey::Metadata)).unwrap();
    assert_eq!(
        files.save(&seven, &IoCancellation::new()),
        Err(ServerError::InvalidInput { field: "revision" })
    );
    assert_eq!(
        files.save(
            &snapshot(SaveValue::Metadata(metadata(7001, 0)), 8),
            &IoCancellation::new()
        ),
        Err(ServerError::InvalidInput { field: "revision" })
    );
    assert_eq!(
        fs::read(canonical(&root, &SaveKey::Metadata)).unwrap(),
        eight_bytes
    );

    *state.lock().unwrap() = Some((IoFaultPoint::Rename, IoPhase::Before));
    assert!(files.save(&nine, &IoCancellation::new()).is_err());
    assert_eq!(files.save(&eight, &IoCancellation::new()), Ok(8));
    assert_eq!(
        fs::read(canonical(&root, &SaveKey::Metadata)).unwrap(),
        eight_bytes
    );

    *state.lock().unwrap() = Some((IoFaultPoint::Rename, IoPhase::After));
    assert!(files.save(&nine, &IoCancellation::new()).is_err());
    let nine_bytes = fs::read(canonical(&root, &SaveKey::Metadata)).unwrap();
    assert_ne!(nine_bytes, eight_bytes);
    assert_eq!(
        files.save(&eight, &IoCancellation::new()),
        Err(ServerError::InvalidInput { field: "revision" })
    );
    assert_eq!(
        files.save(
            &snapshot(SaveValue::Metadata(metadata(7001, 0)), 9),
            &IoCancellation::new()
        ),
        Err(ServerError::InvalidInput { field: "revision" })
    );
    *state.lock().unwrap() = Some((IoFaultPoint::DirectorySync, IoPhase::Before));
    assert!(files.save(&nine, &IoCancellation::new()).is_err());
    *state.lock().unwrap() = Some((IoFaultPoint::Rename, IoPhase::Before));
    assert_eq!(files.save(&nine, &IoCancellation::new()), Ok(9));
    assert_eq!(
        fs::read(canonical(&root, &SaveKey::Metadata)).unwrap(),
        nine_bytes
    );
}

#[test]
fn open_requires_existing_root_and_retries_root_barrier() {
    let root = TestRoot::new();
    let missing = root.0.join("missing");
    assert!(matches!(
        AtomicFiles::open(&missing),
        Err(ServerError::Io {
            operation: Operation::Load,
            kind: io::ErrorKind::NotFound
        })
    ));
    assert!(!missing.exists());

    let state = Arc::new(Mutex::new(Some((
        IoFaultPoint::DirectorySync,
        IoPhase::Before,
    ))));
    assert!(AtomicFiles::with_io(&root.0, Box::new(Fault(state.clone()))).is_err());
    assert!(root.0.join("players").is_dir());
    *state.lock().unwrap() = None;
    AtomicFiles::with_io(&root.0, Box::new(Fault(state.clone()))).unwrap();
    *state.lock().unwrap() = Some((IoFaultPoint::DirectorySync, IoPhase::After));
    assert!(AtomicFiles::with_io(&root.0, Box::new(Fault(state))).is_err());
    assert!(
        AtomicFiles::with_io(
            &root.0,
            Box::new(CloseFault {
                after_temp_sync: true
            })
        )
        .is_err()
    );
}

#[test]
fn partial_zero_close_and_cancellation_refuse() {
    let root = TestRoot::new();
    for injected in [
        Box::new(ZeroWrite) as Box<dyn DiskIo>,
        Box::new(CloseFault {
            after_temp_sync: false,
        }),
    ] {
        let mut files = AtomicFiles::with_io(&root.0, injected).unwrap();
        assert!(files.save(&player(7, 20), &IoCancellation::new()).is_err());
        assert!(!canonical(&root, &player(7, 20).key).exists());
    }
    let mut files = AtomicFiles::with_io(&root.0, Box::new(ShortWrite)).unwrap();
    let cancelled = IoCancellation::new();
    cancelled.cancel();
    assert_eq!(
        files.save(&player(7, 20), &cancelled),
        Err(ServerError::Cancelled)
    );
    assert_eq!(files.save(&player(7, 20), &IoCancellation::new()), Ok(7));
    assert_eq!(
        files.load(SaveKey::Chunk(mornlea_server::contracts::ChunkKey {
            dimension: mornlea_domain::Dimension::OVERWORLD,
            pos: mornlea_domain::ChunkPos::new(0, 0)
        })),
        Err(ServerError::InvalidInput { field: "save_key" })
    );
    assert_eq!(files.sync(), Ok(()));
    assert_eq!(files.close(), Ok(()));
    assert_eq!(
        files.load(SaveKey::Metadata),
        Err(ServerError::Io {
            operation: Operation::Load,
            kind: io::ErrorKind::NotConnected
        })
    );
}

#[test]
fn cancellation_straddles_rename() {
    for (point, phase, outcome) in [
        (
            IoFaultPoint::TempSync,
            IoPhase::After,
            Err(ServerError::Cancelled),
        ),
        (IoFaultPoint::Rename, IoPhase::After, Ok(7)),
    ] {
        let root = TestRoot::new();
        let token = IoCancellation::new();
        let mut files = AtomicFiles::with_io(
            &root.0,
            Box::new(CancelAt {
                token: token.clone(),
                point,
                phase,
            }),
        )
        .unwrap();
        let item = player(7, 20);
        assert_eq!(files.save(&item, &token), outcome);
        assert_eq!(canonical(&root, &item.key).exists(), outcome.is_ok());
    }
}
