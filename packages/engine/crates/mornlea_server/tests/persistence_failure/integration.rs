//! Real save/restart integration through the exclusive disk owner.
//!
//! One case runs the full durable path: snapshots staged through the real
//! authority dirty lane, selected under a checked budget, committed with the
//! real `DiskStore`, synced, acked, closed, reopened, and loaded back. Every
//! comparison names logical fields (chunk revision and block, player pose and
//! health, world seed), never a private Rust layout. The second case induces
//! a failed read at startup by corrupting the metadata file: the reopen
//! reports the failure and preserves the corrupt bytes instead of minting a
//! blank world.

use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use mornlea_domain::{ChunkPos, Dimension, PlayerId};
use mornlea_server::contracts::{
    ChunkKey, DiskBackend, LoadedValue, OwnedSnapshot, SaveBudget, SaveKey, SaveMode, SaveRequest,
    SaveTicket, SaveUrgency, SaveValue, ServerLimits,
};
use mornlea_server::state::AuthorityState;
use mornlea_server::store::disk::{DiskOptions, DiskStore};
use mornlea_storage::{
    ChestSlot, Chunk, ChunkSave, CompanionSave, ContainerSnapshot, DropSlot, FurnaceSlot,
    HostileMobsSave, Inventory, ItemStack, METADATA_CURRENT_VERSION, Metadata, MetadataChunkPos,
    PassiveMobsSave, PlayerLocation, PlayerSave, StorageKind,
};

static NEXT_ROOT: AtomicU64 = AtomicU64::new(0);

/// Disposable world directory removed on drop.
struct Root(PathBuf);

impl Root {
    fn new(tag: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "mornlea-integration-{}-{}-{}",
            std::process::id(),
            NEXT_ROOT.fetch_add(1, Ordering::Relaxed),
            tag
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
}

impl Drop for Root {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
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

fn uuid(tag: u8) -> [u8; 16] {
    let mut bytes = [0; 16];
    bytes[0] = tag;
    bytes[6] = 0x40;
    bytes[8] = 0x80;
    bytes
}

/// One chunk snapshot at revision 9 whose every section carries block 2, the
/// logical payload the restart comparison checks.
fn chunk_snapshot() -> OwnedSnapshot {
    OwnedSnapshot::try_new(
        SaveKey::Chunk(ChunkKey {
            dimension: Dimension::OVERWORLD,
            pos: ChunkPos::new(3, -1),
        }),
        9,
        1024,
        SaveUrgency::Autosave,
        SaveValue::Chunk(ChunkSave {
            key: mornlea_storage::ChunkKey {
                dimension: 0,
                x: 3,
                z: -1,
            },
            revision: 9,
            chunk: Chunk {
                sections: vec![
                    ContainerSnapshot {
                        kind: StorageKind::Single,
                        bits: 0,
                        single: 2,
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

/// One player snapshot with a distinctive pose and health triple, the logical
/// payload the restart comparison checks.
fn player_snapshot() -> OwnedSnapshot {
    OwnedSnapshot::try_new(
        SaveKey::Player(PlayerId::try_from_bytes(uuid(1)).unwrap()),
        9,
        1024,
        SaveUrgency::Autosave,
        SaveValue::Player(PlayerSave {
            player_id: mornlea_storage::PlayerId::from_bytes(uuid(1)),
            revision: 9,
            display_name: "Ada".to_owned(),
            current: PlayerLocation {
                dimension: 0,
                position: [10.5, 65.0, -3.25],
            },
            yaw: 1.2,
            pitch: 0.3,
            safe: None,
            inventory: Inventory::default(),
            health: 15,
            hunger: 17,
            saturation_milli: 9_000,
            exhaustion_milli: 250,
            respawn_present: false,
            respawn_position: [0.0; 3],
            respawn_dimension: 0,
            armor: [ItemStack::default(); 4],
        }),
    )
    .unwrap()
}

/// The remaining save families at the same revision, so the round trip covers
/// the whole durable surface instead of one lane.
fn family_snapshots() -> Vec<OwnedSnapshot> {
    let values = [
        (
            SaveKey::Companions,
            SaveValue::Companions(CompanionSave {
                revision: 9,
                agent_namespace_id: mornlea_storage::PlayerId::from_bytes(uuid(2)),
                records: vec![],
                lifecycles: vec![],
                queues: vec![],
            }),
        ),
        (
            SaveKey::Hostiles,
            SaveValue::Hostiles(HostileMobsSave {
                revision: 9,
                records: vec![],
            }),
        ),
        (
            SaveKey::Passives,
            SaveValue::Passives(PassiveMobsSave {
                revision: 9,
                records: vec![],
            }),
        ),
        (SaveKey::Metadata, SaveValue::Metadata(metadata(13))),
    ];
    values
        .into_iter()
        .map(|(key, value)| {
            OwnedSnapshot::try_new(key, 9, 1024, SaveUrgency::Autosave, value).unwrap()
        })
        .collect()
}

fn authority() -> AuthorityState {
    AuthorityState::try_new(
        ServerLimits::try_new(8, 4096, 512, 64, 64, 1_048_576).unwrap(),
        13,
    )
    .unwrap()
}

/// Stages snapshots through the real authority dirty lane, commits them with
/// the real disk owner, restarts the store, and proves every logical payload
/// survives: chunk revision and block, player pose and health, world seed.
#[test]
fn real_save_restart_round_trip_through_store() {
    let root = Root::new("roundtrip");
    let mut state = authority();
    state.remember_dirty(chunk_snapshot());
    state.remember_dirty(player_snapshot());
    for snapshot in family_snapshots() {
        state.remember_dirty(snapshot);
    }
    let selected = state.select(SaveMode::All, SaveBudget::default());
    assert_eq!(selected.len(), 6, "every family is pending exactly once");
    assert_eq!(state.save_stats().in_flight, 6);

    let mut disk = DiskStore::open(&root.0, options(13)).unwrap();
    let ticket = SaveTicket::try_from_raw(1).unwrap();
    let completion = disk.write(
        ticket,
        SaveRequest {
            snapshots: selected,
        },
    );
    assert!(completion.error.is_none(), "real commit reports no error");
    assert_eq!(completion.committed.len(), 6);
    disk.sync().unwrap();
    let report = state.apply_completion(completion);
    assert_eq!(report.acked, 6, "every pending save acks");
    assert!(report.retry.is_empty());
    assert_eq!(state.save_stats().in_flight, 0);
    disk.close().unwrap();

    // Restart: a fresh owner over the same directory reads every logical
    // payload back unchanged.
    let mut reopened = DiskStore::open(&root.0, options(99)).unwrap();
    assert_eq!(
        reopened.metadata().seed,
        13,
        "existing seed wins over create"
    );
    match reopened
        .load(SaveKey::Chunk(ChunkKey {
            dimension: Dimension::OVERWORLD,
            pos: ChunkPos::new(3, -1),
        }))
        .expect("chunk loads after restart")
    {
        LoadedValue::Chunk(recovered) => {
            assert_eq!(recovered.revision, 9);
            assert_eq!(recovered.persisted_revision, 9);
            assert_eq!(recovered.chunk.sections.len(), 24);
            assert!(
                recovered
                    .chunk
                    .sections
                    .iter()
                    .all(|section| section.single == 2),
                "every section still carries block 2"
            );
        }
        other => panic!("chunk key loaded another family: {other:?}"),
    }
    match reopened
        .load(SaveKey::Player(PlayerId::try_from_bytes(uuid(1)).unwrap()))
        .expect("player loads after restart")
    {
        LoadedValue::Player(stored) => {
            assert_eq!(stored.display_name, "Ada");
            assert_eq!(stored.current.position, [10.5, 65.0, -3.25]);
            assert_eq!(stored.health, 15);
            assert_eq!(stored.hunger, 17);
            assert_eq!(stored.revision, 9);
        }
        other => panic!("player key loaded another family: {other:?}"),
    }
    assert_eq!(
        reopened.load(SaveKey::Metadata),
        Ok(LoadedValue::Metadata(metadata(13))),
        "metadata round-trips exactly"
    );
    reopened.close().unwrap();
}

/// A failed read at startup reports its outcome and preserves the evidence:
/// corrupting the metadata file makes the reopen fail, and the corrupt bytes
/// stay on disk instead of being replaced by a blank world.
#[test]
fn failed_read_at_startup_reports_without_blank_world() {
    let root = Root::new("failed-read");
    let mut disk = DiskStore::open(&root.0, options(13)).unwrap();
    disk.close().unwrap();

    let original = b"corrupt metadata";
    fs::write(root.0.join("world.meta"), original).unwrap();
    assert!(
        DiskStore::open(&root.0, options(13)).is_err(),
        "corrupt metadata fails the startup read"
    );
    assert_eq!(
        fs::read(root.0.join("world.meta")).unwrap(),
        original,
        "the failed read preserves the corrupt bytes"
    );
}
