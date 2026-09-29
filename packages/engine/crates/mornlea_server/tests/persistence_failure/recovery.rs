//! Real filesystem and lock behavior for the exclusive disk owner.

use mornlea_domain::{ChunkPos, Dimension, PlayerId};
use mornlea_server::contracts::{
    DiskBackend, IoFaultPoint, LoadedValue, Operation, OwnedSnapshot, SaveKey, SaveRequest,
    SaveTicket, SaveUrgency, SaveValue, ServerError,
};
use mornlea_server::store::disk::{DiskOptions, DiskStore};
use mornlea_server::store::io::{DiskIo, IoCancellation, IoPhase, NativeDiskIo};
use mornlea_server::store::region_io::RegionIo;
use mornlea_storage::{
    BANK_A_START_SECTOR, BANK_SIZE, ChestSlot, Chunk, ChunkSave, CompanionSave, ContainerSnapshot,
    DropSlot, FurnaceSlot, HostileMobsSave, Inventory, ItemStack, METADATA_CURRENT_VERSION,
    Metadata, MetadataChunkPos, PassiveMobsSave, PlayerLocation, PlayerSave, RegionKey,
    SECTOR_SIZE, StorageKind, decode_region_bank,
};
use std::fs;
use std::io::{self, BufRead};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

static NEXT_ROOT: AtomicU64 = AtomicU64::new(0);

struct Root(PathBuf);
impl Root {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "mornlea-disk-{}-{}",
            std::process::id(),
            NEXT_ROOT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
}
impl Drop for Root {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}

fn metadata(seed: i64) -> Metadata {
    Metadata {
        format_version: METADATA_CURRENT_VERSION,
        seed,
        spawn_dimension: 0,
        spawn_anchor: MetadataChunkPos { x: 0, z: 0 },
        world_time_ticks: 0,
        day_phase_offset: 0,
        weather_kind: 0,
        weather_ticks_remaining: 0,
        depths_spawn_anchor: MetadataChunkPos { x: 0, z: 0 },
        depths_seed_salt: 0,
        difficulty: 0,
    }
}
fn options(seed: i64) -> DiskOptions {
    DiskOptions {
        create: metadata(seed),
        region_handle_cap: 1,
    }
}

fn chunk(x: i32, revision: u64, block: u16) -> OwnedSnapshot {
    let key = SaveKey::Chunk(mornlea_server::contracts::ChunkKey {
        dimension: Dimension::OVERWORLD,
        pos: ChunkPos::new(x, 0),
    });
    OwnedSnapshot::try_new(
        key,
        revision,
        1024,
        SaveUrgency::Autosave,
        SaveValue::Chunk(ChunkSave {
            key: mornlea_storage::ChunkKey {
                dimension: 0,
                x,
                z: 0,
            },
            revision,
            chunk: Chunk {
                sections: vec![
                    ContainerSnapshot {
                        kind: StorageKind::Single,
                        bits: 0,
                        single: block,
                        palette: Vec::new(),
                        packed: Vec::new(),
                    };
                    24
                ],
                drops: vec![DropSlot::default(); 32],
                furnaces: vec![FurnaceSlot::default(); 32],
                chests: vec![ChestSlot::default(); 16],
            },
        }),
    )
    .unwrap()
}

fn uuid(tag: u8) -> [u8; 16] {
    let mut bytes = [0; 16];
    bytes[0] = tag;
    bytes[6] = 0x40;
    bytes[8] = 0x80;
    bytes
}

fn all_families(revision: u64) -> Vec<OwnedSnapshot> {
    let player_id = PlayerId::try_from_bytes(uuid(1)).unwrap();
    let values = [
        (
            SaveKey::Player(player_id),
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
                health: 20,
                hunger: 20,
                saturation_milli: 5000,
                exhaustion_milli: 0,
                respawn_present: false,
                respawn_position: [0.0; 3],
                respawn_dimension: 0,
                armor: [ItemStack::default(); 4],
            }),
        ),
        (
            SaveKey::Companions,
            SaveValue::Companions(CompanionSave {
                revision,
                agent_namespace_id: mornlea_storage::PlayerId::from_bytes(uuid(2)),
                records: vec![],
                lifecycles: vec![],
                queues: vec![],
            }),
        ),
        (
            SaveKey::Hostiles,
            SaveValue::Hostiles(HostileMobsSave {
                revision,
                records: vec![],
            }),
        ),
        (
            SaveKey::Passives,
            SaveValue::Passives(PassiveMobsSave {
                revision,
                records: vec![],
            }),
        ),
        (SaveKey::Metadata, SaveValue::Metadata(metadata(13))),
    ];
    let mut snapshots = vec![chunk(0, revision, 1)];
    snapshots.extend(values.into_iter().map(|(key, value)| {
        OwnedSnapshot::try_new(key, revision, 1024, SaveUrgency::Autosave, value).unwrap()
    }));
    snapshots
}

fn write(
    disk: &mut DiskStore,
    snapshots: Vec<OwnedSnapshot>,
) -> mornlea_server::contracts::SaveCompletion {
    disk.write(
        SaveTicket::try_from_raw(1).unwrap(),
        SaveRequest { snapshots },
    )
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

#[test]
fn absence_creates_metadata_once_and_existing_seed_wins() {
    let root = Root::new();
    let mut disk = DiskStore::open(&root.0, options(13)).unwrap();
    assert_eq!(disk.metadata().seed, 13);
    assert_eq!(disk.initial_metadata_sequence(), 2);
    assert!(root.0.join("world.meta").is_file());
    disk.close().unwrap();
    let mut reopened = DiskStore::open(&root.0, options(99)).unwrap();
    assert_eq!(reopened.metadata().seed, 13);
    assert_eq!(reopened.initial_metadata_sequence(), 2);
    assert_eq!(
        reopened.load(SaveKey::Metadata),
        Ok(LoadedValue::Metadata(metadata(13)))
    );
    reopened.close().unwrap();
}

#[test]
fn corrupt_metadata_is_preserved_and_lock_refuses_concurrent_owner() {
    let root = Root::new();
    let mut first = DiskStore::open(&root.0, options(13)).unwrap();
    assert_eq!(
        DiskStore::open(&root.0, options(14)).err(),
        Some(ServerError::Io {
            operation: Operation::Load,
            kind: io::ErrorKind::WouldBlock,
        })
    );
    first.close().unwrap();
    let original = b"corrupt metadata";
    fs::write(root.0.join("world.meta"), original).unwrap();
    assert!(DiskStore::open(&root.0, options(14)).is_err());
    assert_eq!(fs::read(root.0.join("world.meta")).unwrap(), original);
}

#[test]
fn backup_copies_extra_files_and_skips_lock_and_temporary_files() {
    let root = Root::new();
    let mut disk = DiskStore::open(&root.0, options(13)).unwrap();
    fs::write(root.0.join("extra.dat"), b"source-owned").unwrap();
    fs::write(root.0.join(".extra.dat.tmp-1"), b"partial").unwrap();
    let target = root.0.parent().unwrap().join(format!(
        "mornlea-backup-{}-{}",
        std::process::id(),
        NEXT_ROOT.fetch_add(1, Ordering::Relaxed)
    ));
    disk.backup(&target, &IoCancellation::new()).unwrap();
    assert_eq!(fs::read(target.join("extra.dat")).unwrap(), b"source-owned");
    assert!(target.join("world.meta").is_file());
    assert!(!target.join("world.lock").exists());
    assert!(!target.join(".extra.dat.tmp-1").exists());
    disk.backup(&target, &IoCancellation::new()).unwrap();
    fs::remove_dir_all(target).unwrap();
    disk.close().unwrap();
}

#[test]
fn backup_copies_hidden_canonical_names_but_skips_go_temporary_patterns() {
    let root = Root::new();
    let mut disk = DiskStore::open(&root.0, options(13)).unwrap();
    for name in [".tmp-note", ".compact-note", ".create-note"] {
        fs::write(root.0.join(name), b"canonical").unwrap();
    }
    for name in [
        ".a.tmp-incomplete",
        ".a.compact-incomplete",
        ".a.create-incomplete",
    ] {
        fs::write(root.0.join(name), b"candidate").unwrap();
    }
    let target = root.0.parent().unwrap().join(format!(
        "mornlea-hidden-backup-{}-{}",
        std::process::id(),
        NEXT_ROOT.fetch_add(1, Ordering::Relaxed)
    ));
    disk.backup(&target, &IoCancellation::new()).unwrap();
    let copied: Vec<_> = [".tmp-note", ".compact-note", ".create-note"]
        .into_iter()
        .map(|name| fs::read(target.join(name)).ok())
        .collect();
    let skipped: Vec<_> = [
        ".a.tmp-incomplete",
        ".a.compact-incomplete",
        ".a.create-incomplete",
    ]
    .into_iter()
    .map(|name| target.join(name).exists())
    .collect();
    fs::remove_dir_all(&target).unwrap();
    assert_eq!(copied, vec![Some(b"canonical".to_vec()); 3]);
    assert_eq!(skipped, vec![false, false, false]);
    disk.close().unwrap();
}

#[cfg(unix)]
#[test]
fn backup_rejects_temporary_named_symlink_before_filtering() {
    use std::os::unix::fs::symlink;
    let root = Root::new();
    let mut disk = DiskStore::open(&root.0, options(13)).unwrap();
    symlink(
        root.0.join("world.meta"),
        root.0.join(".world.meta.tmp-link"),
    )
    .unwrap();
    let target = root.0.parent().unwrap().join(format!(
        "mornlea-temp-link-backup-{}-{}",
        std::process::id(),
        NEXT_ROOT.fetch_add(1, Ordering::Relaxed)
    ));
    let result = disk.backup(&target, &IoCancellation::new());
    let published = target.exists();
    if published {
        fs::remove_dir_all(&target).unwrap();
    }
    assert!(result.is_err());
    assert!(!published);
    disk.close().unwrap();
}

#[test]
fn backup_rejects_self_and_cancellation_without_publishing() {
    let root = Root::new();
    let mut disk = DiskStore::open(&root.0, options(13)).unwrap();
    assert!(disk.backup(&root.0, &IoCancellation::new()).is_err());
    let child = root.0.join("child");
    assert!(disk.backup(&child, &IoCancellation::new()).is_err());
    let target = root.0.parent().unwrap().join(format!(
        "mornlea-cancelled-backup-{}-{}",
        std::process::id(),
        NEXT_ROOT.fetch_add(1, Ordering::Relaxed)
    ));
    let cancel = IoCancellation::new();
    cancel.cancel();
    assert_eq!(disk.backup(&target, &cancel), Err(ServerError::Cancelled));
    assert!(!target.exists());
    disk.close().unwrap();
}

#[test]
fn six_families_roundtrip_and_missing_chunk_does_not_create_region() {
    let root = Root::new();
    let mut disk = DiskStore::open(&root.0, options(13)).unwrap();
    let snapshots = all_families(7);
    let missing = disk.load(chunk(0, 1, 1).key.clone());
    assert_eq!(
        missing,
        Err(ServerError::Io {
            operation: Operation::Load,
            kind: io::ErrorKind::NotFound
        })
    );
    assert!(!root.0.join("dimensions").exists());
    let completion = write(&mut disk, snapshots.clone());
    assert_eq!(completion.error, None);
    assert_eq!(completion.snapshots, snapshots);
    assert_eq!(completion.committed.len(), 6);
    disk.close().unwrap();
    let mut disk = DiskStore::open(&root.0, options(99)).unwrap();
    for snapshot in snapshots {
        let loaded = disk.load(snapshot.key.clone()).unwrap();
        match (loaded, snapshot.value) {
            (LoadedValue::Chunk(found), SaveValue::Chunk(want)) => {
                assert_eq!(found.chunk, want.chunk);
                assert_eq!(found.revision, want.revision);
            }
            (LoadedValue::Player(found), SaveValue::Player(want)) => {
                assert_eq!(found.revision, want.revision);
                assert_eq!(found.player_id, want.player_id);
            }
            (LoadedValue::Companions(found), SaveValue::Companions(want)) => {
                assert_eq!(found.revision, want.revision)
            }
            (LoadedValue::Hostiles(found), SaveValue::Hostiles(want)) => {
                assert_eq!(found.revision, want.revision)
            }
            (LoadedValue::Passives(found), SaveValue::Passives(want)) => {
                assert_eq!(found.revision, want.revision)
            }
            (LoadedValue::Metadata(found), SaveValue::Metadata(want)) => assert_eq!(found, want),
            other => panic!("wrong family: {other:?}"),
        }
    }
    disk.close().unwrap();
}

#[test]
fn bad_identity_and_revision_refuse_whole_batch() {
    let root = Root::new();
    let mut disk = DiskStore::open(&root.0, options(13)).unwrap();
    let mut bad = chunk(1, 7, 2);
    if let SaveValue::Chunk(save) = &mut bad.value {
        save.key.x = 9;
    }
    let original = vec![chunk(0, 7, 1), bad.clone()];
    let result = write(&mut disk, original.clone());
    assert!(result.error.is_some());
    assert!(result.committed.is_empty());
    assert_eq!(result.snapshots, original);
    assert!(!root.0.join("dimensions").exists());
    let mut bad = chunk(1, 7, 2);
    if let SaveValue::Chunk(save) = &mut bad.value {
        save.revision = 8;
    }
    let result = write(&mut disk, vec![chunk(0, 7, 1), bad]);
    assert!(result.error.is_some());
    assert!(result.committed.is_empty());
    disk.close().unwrap();
}

#[test]
fn highest_duplicate_wins_and_equal_highest_conflict_refuses() {
    let root = Root::new();
    let mut disk = DiskStore::open(&root.0, options(13)).unwrap();
    let result = write(
        &mut disk,
        vec![chunk(0, 5, 1), chunk(0, 7, 3), chunk(0, 5, 2)],
    );
    assert_eq!(result.error, None);
    assert_eq!(result.committed, vec![(chunk(0, 7, 3).key, 7)]);
    let loaded = disk.load(chunk(0, 7, 3).key).unwrap();
    let LoadedValue::Chunk(found) = loaded else {
        panic!("expected chunk")
    };
    assert_eq!(
        found.chunk,
        match chunk(0, 7, 3).value {
            SaveValue::Chunk(save) => save.chunk,
            _ => unreachable!(),
        }
    );
    let result = write(&mut disk, vec![chunk(0, 9, 4), chunk(0, 9, 5)]);
    assert!(result.error.is_some());
    assert!(result.committed.is_empty());
    disk.close().unwrap();
}

#[test]
fn later_family_fault_preserves_earlier_commit_and_snapshot_ownership() {
    let root = Root::new();
    let fault = Arc::new(Mutex::new(None));
    let state = fault.clone();
    let mut disk = DiskStore::with_io(
        &root.0,
        options(13),
        Box::new(move || Box::new(Fault(state.clone()))),
    )
    .unwrap();
    assert_eq!(write(&mut disk, vec![chunk(0, 6, 1)]).error, None);
    *fault.lock().unwrap() = Some((IoFaultPoint::TempWrite, IoPhase::Before));
    let snapshots = vec![chunk(0, 7, 1), all_families(7)[1].clone()];
    let result = write(&mut disk, snapshots.clone());
    assert!(result.error.is_some());
    assert_eq!(result.committed, vec![(snapshots[0].key.clone(), 7)]);
    assert_eq!(result.snapshots, snapshots);
    *fault.lock().unwrap() = None;
    disk.close().unwrap();
}

#[test]
fn later_region_fault_preserves_earlier_region_commit() {
    let root = Root::new();
    let fault = Arc::new(Mutex::new(None));
    let state = fault.clone();
    let mut disk = DiskStore::with_io(
        &root.0,
        options(13),
        Box::new(move || Box::new(Fault(state.clone()))),
    )
    .unwrap();
    assert_eq!(write(&mut disk, vec![chunk(0, 6, 1)]).error, None);
    *fault.lock().unwrap() = Some((IoFaultPoint::TempWrite, IoPhase::Before));
    let snapshots = vec![chunk(0, 7, 1), chunk(32, 7, 2)];
    let result = write(&mut disk, snapshots.clone());
    assert!(result.error.is_some());
    assert_eq!(result.committed, vec![(snapshots[0].key.clone(), 7)]);
    assert_eq!(result.snapshots, snapshots);
    *fault.lock().unwrap() = None;
    disk.close().unwrap();
}

#[test]
fn retry_rechecks_parent_barrier_after_partial_region_directory_create() {
    let root = Root::new();
    let fault = Arc::new(Mutex::new(None));
    let for_io = fault.clone();
    let mut disk = DiskStore::with_io(
        &root.0,
        options(13),
        Box::new(move || Box::new(Fault(for_io.clone()))),
    )
    .unwrap();
    *fault.lock().unwrap() = Some((IoFaultPoint::DirectorySync, IoPhase::Before));
    let failed = write(&mut disk, vec![chunk(0, 7, 1)]);
    assert!(failed.error.is_some());
    assert!(failed.committed.is_empty());
    assert!(root.0.join("dimensions").is_dir());
    assert!(!root.0.join("dimensions/0/regions/r.0.0.region").exists());
    *fault.lock().unwrap() = None;
    assert_eq!(write(&mut disk, vec![chunk(0, 7, 1)]).error, None);
    assert!(root.0.join("dimensions/0/regions/r.0.0.region").is_file());
    disk.close().unwrap();
}

#[test]
fn one_region_handle_eviction_reopens_previous_region() {
    let root = Root::new();
    let mut disk = DiskStore::open(&root.0, options(13)).unwrap();
    assert_eq!(write(&mut disk, vec![chunk(0, 7, 1)]).error, None);
    assert_eq!(disk.open_region_count(), 1);
    assert_eq!(write(&mut disk, vec![chunk(32, 7, 2)]).error, None);
    assert_eq!(disk.open_region_count(), 1);
    assert!(matches!(
        disk.load(chunk(0, 7, 1).key),
        Ok(LoadedValue::Chunk(_))
    ));
    assert_eq!(disk.open_region_count(), 1);
    disk.close().unwrap();
}

#[test]
fn failed_eviction_close_can_reopen_region_and_finish_close() {
    struct EvictionCloseFault(Arc<std::sync::atomic::AtomicBool>);
    impl DiskIo for EvictionCloseFault {
        fn close(&mut self, file: fs::File) -> io::Result<()> {
            let is_region_file = file.metadata()?.is_file();
            let mut native = NativeDiskIo;
            native.close(file)?;
            if is_region_file && self.0.swap(false, Ordering::SeqCst) {
                Err(io::ErrorKind::PermissionDenied.into())
            } else {
                Ok(())
            }
        }
    }
    let root = Root::new();
    let fail = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let for_io = fail.clone();
    let mut disk = DiskStore::with_io(
        &root.0,
        options(13),
        Box::new(move || Box::new(EvictionCloseFault(for_io.clone()))),
    )
    .unwrap();
    assert_eq!(write(&mut disk, vec![chunk(0, 7, 1)]).error, None);
    fail.store(true, Ordering::SeqCst);
    let failed = write(&mut disk, vec![chunk(32, 7, 2)]);
    assert_eq!(
        failed.error,
        Some(ServerError::Io {
            operation: Operation::Close,
            kind: io::ErrorKind::PermissionDenied,
        })
    );
    assert!(failed.committed.is_empty());
    assert_eq!(disk.open_region_count(), 0);
    assert!(matches!(
        disk.load(chunk(0, 7, 1).key),
        Ok(LoadedValue::Chunk(_))
    ));
    fail.store(true, Ordering::SeqCst);
    assert!(write(&mut disk, vec![chunk(32, 7, 2)]).error.is_some());
    assert_eq!(disk.open_region_count(), 0);
    assert_eq!(
        DiskStore::open(&root.0, options(14)).err(),
        Some(ServerError::Io {
            operation: Operation::Load,
            kind: io::ErrorKind::WouldBlock,
        })
    );
    disk.close().unwrap();
    let mut reopened = DiskStore::open(&root.0, options(14)).unwrap();
    assert!(matches!(
        reopened.load(chunk(0, 7, 1).key),
        Ok(LoadedValue::Chunk(_))
    ));
    assert_eq!(
        reopened.load(chunk(32, 7, 2).key),
        Err(ServerError::Io {
            operation: Operation::Load,
            kind: io::ErrorKind::NotFound,
        })
    );
    reopened.close().unwrap();
}

#[test]
fn closed_disk_refuses_new_operations() {
    let root = Root::new();
    let mut disk = DiskStore::open(&root.0, options(13)).unwrap();
    disk.close().unwrap();
    assert_eq!(
        disk.sync(),
        Err(ServerError::Io {
            operation: Operation::Sync,
            kind: io::ErrorKind::NotConnected,
        })
    );
    assert_eq!(
        disk.load(SaveKey::Metadata),
        Err(ServerError::Io {
            operation: Operation::Load,
            kind: io::ErrorKind::NotConnected,
        })
    );
    assert!(write(&mut disk, vec![chunk(0, 2, 1)]).error.is_some());
    disk.close().unwrap();
}

#[test]
fn future_metadata_is_not_replaced() {
    let root = Root::new();
    let mut disk = DiskStore::open(&root.0, options(13)).unwrap();
    disk.close().unwrap();
    let path = root.0.join("world.meta");
    let mut bytes = fs::read(&path).unwrap();
    bytes[4..8].copy_from_slice(&(METADATA_CURRENT_VERSION + 1).to_le_bytes());
    fs::write(&path, &bytes).unwrap();
    assert!(DiskStore::open(&root.0, options(99)).is_err());
    assert_eq!(fs::read(path).unwrap(), bytes);
}

#[test]
fn failed_region_close_retains_world_lock_until_retry() {
    struct CloseFault(Arc<std::sync::atomic::AtomicBool>);
    impl DiskIo for CloseFault {
        fn close(&mut self, file: fs::File) -> io::Result<()> {
            let mut native = NativeDiskIo;
            let result = native.close(file);
            if self.0.swap(false, Ordering::SeqCst) {
                result?;
                Err(io::ErrorKind::PermissionDenied.into())
            } else {
                result
            }
        }
    }
    let root = Root::new();
    let fault = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let state = fault.clone();
    let mut disk = DiskStore::with_io(
        &root.0,
        options(13),
        Box::new(move || Box::new(CloseFault(state.clone()))),
    )
    .unwrap();
    assert_eq!(write(&mut disk, vec![chunk(0, 7, 1)]).error, None);
    fault.store(true, Ordering::SeqCst);
    assert!(disk.close().is_err());
    assert_eq!(
        DiskStore::open(&root.0, options(14)).err(),
        Some(ServerError::Io {
            operation: Operation::Load,
            kind: io::ErrorKind::WouldBlock,
        })
    );
    disk.close().unwrap();
    DiskStore::open(&root.0, options(14))
        .unwrap()
        .close()
        .unwrap();
}

#[test]
fn failed_close_sync_retains_world_lock_until_retry() {
    let root = Root::new();
    let fault = Arc::new(Mutex::new(None));
    let for_io = fault.clone();
    let mut disk = DiskStore::with_io(
        &root.0,
        options(13),
        Box::new(move || Box::new(Fault(for_io.clone()))),
    )
    .unwrap();
    *fault.lock().unwrap() = Some((IoFaultPoint::DirectorySync, IoPhase::Before));
    assert!(disk.close().is_err());
    assert_eq!(
        DiskStore::open(&root.0, options(14)).err(),
        Some(ServerError::Io {
            operation: Operation::Load,
            kind: io::ErrorKind::WouldBlock,
        })
    );
    *fault.lock().unwrap() = None;
    disk.close().unwrap();
    DiskStore::open(&root.0, options(14))
        .unwrap()
        .close()
        .unwrap();
}

#[test]
fn recovered_standby_chunk_is_read_only_until_authority_resaves() {
    let root = Root::new();
    let mut disk = DiskStore::open(&root.0, options(13)).unwrap();
    assert_eq!(write(&mut disk, vec![chunk(0, 7, 1)]).error, None);
    assert_eq!(write(&mut disk, vec![chunk(0, 8, 2)]).error, None);
    disk.close().unwrap();
    let path = root.0.join("dimensions/0/regions/r.0.0.region");
    let mut bytes = fs::read(&path).unwrap();
    let bank = decode_region_bank(
        RegionKey {
            dimension: 0,
            x: 0,
            z: 0,
        },
        &bytes[BANK_A_START_SECTOR as usize * SECTOR_SIZE as usize
            ..BANK_A_START_SECTOR as usize * SECTOR_SIZE as usize + BANK_SIZE],
        bytes.len() as i64,
    )
    .unwrap();
    bytes[bank.entries[0].offset_sector as usize * SECTOR_SIZE as usize] ^= 0xff;
    fs::write(&path, &bytes).unwrap();
    let mut disk = DiskStore::open(&root.0, options(13)).unwrap();
    let LoadedValue::Chunk(loaded) = disk.load(chunk(0, 1, 1).key).unwrap() else {
        panic!("expected chunk")
    };
    assert_eq!(
        (
            loaded.revision,
            loaded.persisted_revision,
            loaded.needs_rewrite,
            loaded.recovered
        ),
        (9, 7, true, true)
    );
    assert_eq!(fs::read(&path).unwrap(), bytes);
    disk.close().unwrap();
}

#[test]
fn compaction_policy_requires_both_waste_and_ratio() {
    let root = Root::new();
    let path = root.0.join("r.0.0.region");
    let key = RegionKey {
        dimension: 0,
        x: 0,
        z: 0,
    };
    let mut region = RegionIo::open(&path, key).unwrap();
    assert!(!region.should_compact(1, 0.25).unwrap());
    for revision in 1..=3 {
        let SaveValue::Chunk(save) = chunk(0, revision, revision as u16).value else {
            unreachable!()
        };
        assert!(region.save(&[save], &IoCancellation::new()).error.is_none());
    }
    assert!(!region.should_compact(8 * 1024 * 1024, 0.25).unwrap());
    assert!(!region.should_compact(0, 1.01).unwrap());
    assert!(region.should_compact(1, 0.01).unwrap());
    region.close().unwrap();
}

#[cfg(unix)]
#[test]
fn go_flock_and_rust_lease_exclude_each_other_across_processes() {
    let root = Root::new();
    let source = root.0.join("flock.go");
    let binary = root.0.join("flock-helper");
    fs::write(
        &source,
        r#"package main
import ("bufio"; "fmt"; "os"; "syscall")
func main() {
 f, e := os.OpenFile(os.Args[1], os.O_CREATE|os.O_RDWR, 0600); if e != nil { os.Exit(4) }
 if syscall.Flock(int(f.Fd()), syscall.LOCK_EX|syscall.LOCK_NB) != nil { os.Exit(3) }
 fmt.Println("READY")
 bufio.NewReader(os.Stdin).ReadByte()
 syscall.Flock(int(f.Fd()), syscall.LOCK_UN)
}


"#,
    )
    .unwrap();
    let built = Command::new("go")
        .arg("build")
        .arg("-o")
        .arg(&binary)
        .arg(&source)
        .output()
        .unwrap();
    assert!(
        built.status.success(),
        "{}",
        String::from_utf8_lossy(&built.stderr)
    );
    let mut go = Command::new(&binary)
        .arg(root.0.join("world.lock"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut ready = String::new();
    io::BufReader::new(go.stdout.take().unwrap())
        .read_line(&mut ready)
        .unwrap();
    assert_eq!(ready, "READY\n");
    assert_eq!(
        DiskStore::open(&root.0, options(13)).err(),
        Some(ServerError::Io {
            operation: Operation::Load,
            kind: io::ErrorKind::WouldBlock,
        })
    );
    drop(go.stdin.take());
    assert!(go.wait().unwrap().success());
    let mut disk = DiskStore::open(&root.0, options(13)).unwrap();
    let attempt = Command::new(&binary)
        .arg(root.0.join("world.lock"))
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert_eq!(attempt.status.code(), Some(3));
    disk.close().unwrap();
}

#[test]
fn failed_backup_parent_sync_leaves_reusable_complete_target() {
    struct ParentFault {
        state: Arc<Mutex<(bool, bool)>>,
    }
    impl DiskIo for ParentFault {
        fn boundary(&mut self, point: IoFaultPoint, phase: IoPhase) -> io::Result<()> {
            let mut state = self.state.lock().unwrap();
            if point == IoFaultPoint::Rename && phase == IoPhase::After && state.0 {
                state.1 = true;
            }
            if point == IoFaultPoint::DirectorySync && phase == IoPhase::Before && state.1 {
                state.0 = false;
                state.1 = false;
                return Err(io::ErrorKind::PermissionDenied.into());
            }
            Ok(())
        }
    }
    let root = Root::new();
    let state = Arc::new(Mutex::new((false, false)));
    let owner = state.clone();
    let mut disk = DiskStore::with_io(
        &root.0,
        options(13),
        Box::new(move || {
            Box::new(ParentFault {
                state: owner.clone(),
            })
        }),
    )
    .unwrap();
    fs::write(root.0.join("extra.dat"), b"complete").unwrap();
    let target = root.0.parent().unwrap().join(format!(
        "mornlea-retry-backup-{}-{}",
        std::process::id(),
        NEXT_ROOT.fetch_add(1, Ordering::Relaxed)
    ));
    state.lock().unwrap().0 = true;
    assert!(disk.backup(&target, &IoCancellation::new()).is_err());
    assert_eq!(fs::read(target.join("extra.dat")).unwrap(), b"complete");
    assert!(target.join(".mcgo-world-backup-v1.json").exists());
    disk.backup(&target, &IoCancellation::new()).unwrap();
    fs::remove_dir_all(target).unwrap();
    disk.close().unwrap();
}

#[test]
fn partial_backup_write_never_publishes_target() {
    struct Partial(Arc<std::sync::atomic::AtomicBool>);
    impl DiskIo for Partial {
        fn write(&mut self, file: &mut fs::File, bytes: &[u8]) -> io::Result<usize> {
            if self.0.load(Ordering::SeqCst) {
                use std::io::Write;
                let _ = file.write(&bytes[..1])?;
                return Err(io::ErrorKind::WriteZero.into());
            }
            use std::io::Write;
            file.write(bytes)
        }
    }
    let root = Root::new();
    let fail = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let arm = fail.clone();
    let mut disk = DiskStore::with_io(
        &root.0,
        options(13),
        Box::new(move || Box::new(Partial(arm.clone()))),
    )
    .unwrap();
    fs::write(root.0.join("extra.dat"), b"copy this").unwrap();
    let target = root.0.parent().unwrap().join(format!(
        "mornlea-partial-backup-{}-{}",
        std::process::id(),
        NEXT_ROOT.fetch_add(1, Ordering::Relaxed)
    ));
    fail.store(true, Ordering::SeqCst);
    assert!(disk.backup(&target, &IoCancellation::new()).is_err());
    assert!(!target.exists());
    fail.store(false, Ordering::SeqCst);
    disk.close().unwrap();
}

#[test]
fn identity_write_fault_reports_payload_operation_and_no_target() {
    struct IdentityFault(Arc<std::sync::atomic::AtomicU64>);
    impl DiskIo for IdentityFault {
        fn boundary(&mut self, point: IoFaultPoint, phase: IoPhase) -> io::Result<()> {
            if point == IoFaultPoint::TempWrite
                && phase == IoPhase::Before
                && self.0.fetch_add(1, Ordering::SeqCst) == 1
            {
                return Err(io::ErrorKind::PermissionDenied.into());
            }
            Ok(())
        }
    }
    let root = Root::new();
    let counter = Arc::new(AtomicU64::new(0));
    let for_io = counter.clone();
    let mut disk = DiskStore::with_io(
        &root.0,
        options(13),
        Box::new(move || Box::new(IdentityFault(for_io.clone()))),
    )
    .unwrap();
    counter.store(0, Ordering::SeqCst);
    let target = root.0.parent().unwrap().join(format!(
        "mornlea-identity-fault-{}-{}",
        std::process::id(),
        NEXT_ROOT.fetch_add(1, Ordering::Relaxed)
    ));
    assert_eq!(
        disk.backup(&target, &IoCancellation::new()),
        Err(ServerError::Io {
            operation: Operation::WritePayload,
            kind: io::ErrorKind::PermissionDenied
        })
    );
    assert!(!target.exists());
    disk.close().unwrap();
}

#[cfg(unix)]
#[test]
fn failed_backup_cleans_read_only_temporary_tree() {
    use std::os::unix::fs::PermissionsExt;
    struct RootSyncFault(Arc<AtomicU64>);
    impl DiskIo for RootSyncFault {
        fn boundary(&mut self, point: IoFaultPoint, phase: IoPhase) -> io::Result<()> {
            if point == IoFaultPoint::DirectorySync
                && phase == IoPhase::Before
                && self.0.fetch_add(1, Ordering::SeqCst) == 3
            {
                return Err(io::ErrorKind::PermissionDenied.into());
            }
            Ok(())
        }
    }
    let root = Root::new();
    let calls = Arc::new(AtomicU64::new(0));
    let for_io = calls.clone();
    let mut disk = DiskStore::with_io(
        &root.0,
        options(13),
        Box::new(move || Box::new(RootSyncFault(for_io.clone()))),
    )
    .unwrap();
    let target = root.0.parent().unwrap().join(format!(
        "mornlea-readonly-backup-{}-{}",
        std::process::id(),
        NEXT_ROOT.fetch_add(1, Ordering::Relaxed)
    ));
    fs::set_permissions(&root.0, fs::Permissions::from_mode(0o555)).unwrap();
    calls.store(0, Ordering::SeqCst);
    let outcome = disk.backup(&target, &IoCancellation::new());
    fs::set_permissions(&root.0, fs::Permissions::from_mode(0o700)).unwrap();
    let prefix = format!(".{}.tmp-", target.file_name().unwrap().to_string_lossy());
    let leaked: Vec<_> = fs::read_dir(root.0.parent().unwrap())
        .unwrap()
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with(&prefix)
        })
        .collect();
    for path in &leaked {
        fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
        fs::remove_dir_all(path).unwrap();
    }
    assert!(outcome.is_err());
    assert!(leaked.is_empty());
    assert!(!target.exists());
    disk.close().unwrap();
}

#[test]
fn backup_source_close_failure_never_publishes() {
    struct SourceCloseFault(Arc<AtomicU64>);
    impl DiskIo for SourceCloseFault {
        fn close(&mut self, file: fs::File) -> io::Result<()> {
            let index = self.0.fetch_add(1, Ordering::SeqCst);
            let mut native = NativeDiskIo;
            native.close(file)?;
            if index == 3 {
                Err(io::ErrorKind::PermissionDenied.into())
            } else {
                Ok(())
            }
        }
    }
    let root = Root::new();
    let closes = Arc::new(AtomicU64::new(100));
    let for_io = closes.clone();
    let mut disk = DiskStore::with_io(
        &root.0,
        options(13),
        Box::new(move || Box::new(SourceCloseFault(for_io.clone()))),
    )
    .unwrap();
    closes.store(0, Ordering::SeqCst);
    let target = root.0.parent().unwrap().join(format!(
        "mornlea-source-close-{}-{}",
        std::process::id(),
        NEXT_ROOT.fetch_add(1, Ordering::Relaxed)
    ));
    assert_eq!(
        disk.backup(&target, &IoCancellation::new()),
        Err(ServerError::Io {
            operation: Operation::Close,
            kind: io::ErrorKind::PermissionDenied
        })
    );
    assert!(!target.exists());
    disk.close().unwrap();
}

#[cfg(unix)]
#[test]
fn backup_source_read_refusal_never_publishes() {
    use std::os::unix::fs::PermissionsExt;
    let root = Root::new();
    let mut disk = DiskStore::open(&root.0, options(13)).unwrap();
    let source = root.0.join("extra.dat");
    fs::write(&source, b"unreadable").unwrap();
    fs::set_permissions(&source, fs::Permissions::from_mode(0o000)).unwrap();
    let target = root.0.parent().unwrap().join(format!(
        "mornlea-read-refusal-{}-{}",
        std::process::id(),
        NEXT_ROOT.fetch_add(1, Ordering::Relaxed)
    ));
    let outcome = disk.backup(&target, &IoCancellation::new());
    fs::set_permissions(&source, fs::Permissions::from_mode(0o600)).unwrap();
    assert!(outcome.is_err());
    assert!(!target.exists());
    disk.close().unwrap();
}

#[test]
fn backup_wrong_identity_is_unchanged() {
    let root = Root::new();
    let mut disk = DiskStore::open(&root.0, options(13)).unwrap();
    let target = root.0.parent().unwrap().join(format!(
        "mornlea-wrong-backup-{}-{}",
        std::process::id(),
        NEXT_ROOT.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir(&target).unwrap();
    let identity = target.join(".mcgo-world-backup-v1.json");
    let wrong = b"{\"source\":\"wrong\",\"seed\":13,\"migration_version\":1}\n";
    fs::write(&identity, wrong).unwrap();
    assert!(disk.backup(&target, &IoCancellation::new()).is_err());
    assert_eq!(fs::read(&identity).unwrap(), wrong);
    fs::remove_dir_all(target).unwrap();
    disk.close().unwrap();
}

#[cfg(unix)]
#[test]
fn backup_rejects_symlinks_and_fifo_source_entries() {
    use std::os::unix::fs::symlink;
    let root = Root::new();
    let mut disk = DiskStore::open(&root.0, options(13)).unwrap();
    let target = root.0.parent().unwrap().join(format!(
        "mornlea-refuse-backup-{}-{}",
        std::process::id(),
        NEXT_ROOT.fetch_add(1, Ordering::Relaxed)
    ));
    symlink(root.0.join("world.meta"), root.0.join("alias")).unwrap();
    assert!(disk.backup(&target, &IoCancellation::new()).is_err());
    assert!(!target.exists());
    fs::remove_file(root.0.join("alias")).unwrap();
    let fifo = root.0.join("fifo");
    let result = Command::new("mkfifo").arg(&fifo).status().unwrap();
    assert!(result.success());
    assert!(disk.backup(&target, &IoCancellation::new()).is_err());
    assert!(!target.exists());
    fs::remove_file(fifo).unwrap();
    symlink(&root.0, &target).unwrap();
    assert!(disk.backup(&target, &IoCancellation::new()).is_err());
    fs::remove_file(target).unwrap();
    let alias = root.0.parent().unwrap().join(format!(
        "mornlea-world-alias-{}-{}",
        std::process::id(),
        NEXT_ROOT.fetch_add(1, Ordering::Relaxed)
    ));
    symlink(&root.0, &alias).unwrap();
    assert!(
        disk.backup(&alias.join("child"), &IoCancellation::new())
            .is_err()
    );
    assert!(!root.0.join("child").exists());
    fs::remove_file(alias).unwrap();
    disk.close().unwrap();
}

#[cfg(unix)]
#[test]
fn symlink_world_lock_is_refused_without_following_target() {
    use std::os::unix::fs::symlink;
    let root = Root::new();
    let target = root.0.join("real-lock-target");
    fs::write(&target, b"sentinel").unwrap();
    symlink(&target, root.0.join("world.lock")).unwrap();
    assert!(DiskStore::open(&root.0, options(13)).is_err());
    assert_eq!(fs::read(target).unwrap(), b"sentinel");
    assert!(!root.0.join("world.meta").exists());
}

#[test]
fn cancellation_during_backup_copy_leaves_no_named_target() {
    struct CancelAfterWrite {
        token: IoCancellation,
    }
    impl DiskIo for CancelAfterWrite {
        fn boundary(&mut self, point: IoFaultPoint, phase: IoPhase) -> io::Result<()> {
            if point == IoFaultPoint::TempWrite && phase == IoPhase::After {
                self.token.cancel();
            }
            Ok(())
        }
    }
    let root = Root::new();
    let token = IoCancellation::new();
    let for_io = token.clone();
    let mut disk = DiskStore::with_io(
        &root.0,
        options(13),
        Box::new(move || {
            Box::new(CancelAfterWrite {
                token: for_io.clone(),
            })
        }),
    )
    .unwrap();
    let target = root.0.parent().unwrap().join(format!(
        "mornlea-cancel-copy-{}-{}",
        std::process::id(),
        NEXT_ROOT.fetch_add(1, Ordering::Relaxed)
    ));
    fs::write(root.0.join("extra.dat"), b"copy this").unwrap();
    assert_eq!(disk.backup(&target, &token), Err(ServerError::Cancelled));
    assert!(!target.exists());
    disk.close().unwrap();
}

#[test]
fn backup_preserves_persisted_codecs_and_directory_contents() {
    let root = Root::new();
    let mut disk = DiskStore::open(&root.0, options(13)).unwrap();
    assert_eq!(write(&mut disk, all_families(7)).error, None);
    fs::create_dir(root.0.join("owned-extra")).unwrap();
    fs::write(root.0.join("owned-extra/note.txt"), b"world attachment").unwrap();
    let target = root.0.parent().unwrap().join(format!(
        "mornlea-codec-backup-{}-{}",
        std::process::id(),
        NEXT_ROOT.fetch_add(1, Ordering::Relaxed)
    ));
    disk.backup(&target, &IoCancellation::new()).unwrap();
    for relative in [
        "world.meta",
        "players/01000000-0000-4000-8000-000000000000.player",
        "companions.ai",
        "hostile_mobs.bin",
        "passive_mobs.bin",
        "dimensions/0/regions/r.0.0.region",
        "owned-extra/note.txt",
    ] {
        assert_eq!(
            fs::read(root.0.join(relative)).unwrap(),
            fs::read(target.join(relative)).unwrap()
        );
    }
    fs::remove_dir_all(target).unwrap();
    disk.close().unwrap();
}

#[test]
fn go_and_rust_reuse_each_others_named_backup_identity() {
    let root = Root::new();
    let rust_target = root.0.parent().unwrap().join(format!(
        "mornlea-rust-identity-{}-{}",
        std::process::id(),
        NEXT_ROOT.fetch_add(1, Ordering::Relaxed)
    ));
    let go_target = root.0.parent().unwrap().join(format!(
        "mornlea-go-identity-{}-{}",
        std::process::id(),
        NEXT_ROOT.fetch_add(1, Ordering::Relaxed)
    ));
    let mut disk = DiskStore::open(&root.0, options(13)).unwrap();
    disk.backup(&rust_target, &IoCancellation::new()).unwrap();
    disk.close().unwrap();
    let program = root.0.join("backup_identity.go");
    fs::write(
        &program,
        r#"package main
import (
 "context"
 "os"
 "github.com/channing771/mornlea/packages/server/storage"
)
func main() {
 owner, err := storage.OpenDisk(context.Background(), os.Args[1], storage.OpenOptions{})
 if err != nil { panic(err) }
 if err := owner.Backup(context.Background(), os.Args[2]); err != nil { panic(err) }
 if err := owner.Backup(context.Background(), os.Args[3]); err != nil { panic(err) }
 if err := owner.Close(); err != nil { panic(err) }
}
"#,
    )
    .unwrap();
    let repository = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(4)
        .unwrap()
        .to_owned();
    let go = Command::new("go")
        .arg("run")
        .arg(&program)
        .arg(&root.0)
        .arg(&rust_target)
        .arg(&go_target)
        .current_dir(repository)
        .output()
        .unwrap();
    assert!(
        go.status.success(),
        "{}",
        String::from_utf8_lossy(&go.stderr)
    );
    let mut disk = DiskStore::open(&root.0, options(99)).unwrap();
    disk.backup(&go_target, &IoCancellation::new()).unwrap();
    disk.close().unwrap();
    fs::remove_dir_all(rust_target).unwrap();
    fs::remove_dir_all(go_target).unwrap();
}
