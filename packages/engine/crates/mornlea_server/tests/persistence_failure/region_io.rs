use mornlea_server::contracts::{IoFaultPoint, Operation, SaveKey, ServerError, StorageFailure};
use mornlea_server::store::io::{DiskIo, IoCancellation, IoPhase, NativeDiskIo};
use mornlea_server::store::region_io::RegionIo;
use mornlea_storage::{
    BANK_A_START_SECTOR, BANK_B_START_SECTOR, BANK_SIZE, ChestSlot, Chunk, ChunkKey, ChunkSave,
    ContainerSnapshot, DropSlot, FurnaceSlot, RegionKey, SECTOR_SIZE, StorageKind,
    decode_region_bank, encode_region_bank,
};
use std::fs::{self, File};
use std::io::{self, Write};
use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);

struct TempRegion {
    dir: PathBuf,
    path: PathBuf,
}
impl TempRegion {
    fn new() -> Self {
        let dir = std::env::temp_dir().join(format!(
            "mornlea-region-{}-{}",
            std::process::id(),
            NEXT_TEMP.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&dir).unwrap();
        let path = dir.join("r.0.0.region");
        Self { dir, path }
    }
}
impl Drop for TempRegion {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.dir).unwrap();
    }
}
fn region_key() -> RegionKey {
    RegionKey {
        dimension: 0,
        x: 0,
        z: 0,
    }
}
fn chunk_key(x: i32) -> ChunkKey {
    ChunkKey {
        dimension: 0,
        x,
        z: 0,
    }
}
fn save(x: i32, revision: u64, block: u16) -> ChunkSave {
    ChunkSave {
        key: chunk_key(x),
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
    }
}
fn committed(x: i32, revision: u64) -> Vec<(SaveKey, u64)> {
    vec![(
        SaveKey::Chunk(mornlea_server::contracts::ChunkKey {
            dimension: mornlea_domain::Dimension::new(0).unwrap(),
            pos: mornlea_domain::ChunkPos::new(x, 0),
        }),
        revision,
    )]
}

struct Fault {
    point: IoFaultPoint,
    phase: IoPhase,
    crash: bool,
}
impl DiskIo for Fault {
    fn boundary(&mut self, point: IoFaultPoint, phase: IoPhase) -> io::Result<()> {
        if point == self.point && phase == self.phase {
            if self.crash {
                std::process::exit(86);
            }
            return Err(io::ErrorKind::Other.into());
        }
        Ok(())
    }
}

#[test]
fn roundtrip_and_revision_conflict() {
    let temp = TempRegion::new();
    let mut region = RegionIo::open(&temp.path, region_key()).unwrap();
    assert_eq!(
        region
            .save(&[save(0, 7, 1)], &IoCancellation::new())
            .committed,
        committed(0, 7)
    );
    let loaded = region.load(chunk_key(0)).unwrap();
    assert_eq!(
        (loaded.revision, loaded.persisted_revision, loaded.recovered),
        (7, 7, false)
    );
    assert_eq!(loaded.chunk, save(0, 7, 1).chunk);
    assert_eq!(
        region.save(&[save(0, 7, 2)], &IoCancellation::new()).error,
        Some(ServerError::Storage {
            family: "region",
            kind: StorageFailure::Corrupt
        })
    );
    assert_eq!(
        region
            .save(&[save(0, 6, 2)], &IoCancellation::new())
            .committed,
        committed(0, 7)
    );
    region.close().unwrap();
    assert_eq!(
        RegionIo::open(&temp.path, region_key())
            .unwrap()
            .load(chunk_key(0))
            .unwrap()
            .chunk,
        save(0, 7, 1).chunk
    );
}

#[test]
fn highest_revision_wins_irrespective_of_lower_conflicts() {
    for requests in [
        vec![save(0, 7, 1), save(0, 7, 2), save(0, 8, 3)],
        vec![save(0, 8, 3), save(0, 7, 1), save(0, 7, 2)],
    ] {
        let temp = TempRegion::new();
        let mut region = RegionIo::open(&temp.path, region_key()).unwrap();
        let result = region.save(&requests, &IoCancellation::new());
        assert_eq!(result.error, None);
        assert_eq!(result.committed, committed(0, 8));
        assert_eq!(
            region.load(chunk_key(0)).unwrap().chunk,
            save(0, 8, 3).chunk
        );
    }
}

#[test]
fn failed_reopen_never_uses_stale_bank_view() {
    struct CorruptCompactionHeader;
    impl DiskIo for CorruptCompactionHeader {
        fn write(&mut self, file: &mut File, bytes: &[u8]) -> io::Result<usize> {
            if bytes.starts_with(b"MCGR") {
                let mut corrupt_header = bytes.to_vec();
                corrupt_header[0] = b'X';
                file.write(&corrupt_header)
            } else {
                file.write(bytes)
            }
        }
    }
    let temp = TempRegion::new();
    let mut region = RegionIo::open(&temp.path, region_key()).unwrap();
    region.save(&[save(0, 7, 1)], &IoCancellation::new());
    region.close().unwrap();
    let valid_before = fs::read(&temp.path).unwrap();
    let mut region =
        RegionIo::with_io(&temp.path, region_key(), Box::new(CorruptCompactionHeader)).unwrap();
    assert_eq!(
        region.compact(&IoCancellation::new()),
        Err(ServerError::Storage {
            family: "region",
            kind: StorageFailure::Corrupt
        })
    );
    let bytes = fs::read(&temp.path).unwrap();
    assert_eq!(
        region.load(chunk_key(0)),
        Err(ServerError::Storage {
            family: "region",
            kind: StorageFailure::Corrupt
        })
    );
    let retry = region.save(&[save(0, 8, 2)], &IoCancellation::new());
    assert!(retry.committed.is_empty());
    assert_eq!(
        retry.error,
        Some(ServerError::Storage {
            family: "region",
            kind: StorageFailure::Corrupt
        })
    );
    assert_eq!(fs::read(&temp.path).unwrap(), bytes);
    fs::write(&temp.path, valid_before).unwrap();
    assert_eq!(region.load(chunk_key(0)).unwrap().revision, 7);
    let retry = region.save(&[save(0, 8, 2)], &IoCancellation::new());
    assert_eq!(retry.error, None);
    assert_eq!(retry.committed, committed(0, 8));
}

#[test]
fn directory_close_failure_is_not_durable_success() {
    struct FailDirectoryClose;
    impl DiskIo for FailDirectoryClose {
        fn close(&mut self, file: File) -> io::Result<()> {
            let is_directory = file.metadata()?.is_dir();
            NativeDiskIo.close(file)?;
            if is_directory {
                Err(io::ErrorKind::Other.into())
            } else {
                Ok(())
            }
        }
    }
    let temp = TempRegion::new();
    assert!(matches!(
        RegionIo::with_io(&temp.path, region_key(), Box::new(FailDirectoryClose)),
        Err(ServerError::Io {
            operation: Operation::DirectorySync,
            kind: io::ErrorKind::Other
        })
    ));
    assert!(temp.path.exists());
    let mut region = RegionIo::open(&temp.path, region_key()).unwrap();
    region.save(&[save(0, 7, 1)], &IoCancellation::new());
    region.close().unwrap();
    let mut region =
        RegionIo::with_io(&temp.path, region_key(), Box::new(FailDirectoryClose)).unwrap();
    assert_eq!(
        region.compact(&IoCancellation::new()),
        Err(ServerError::Io {
            operation: Operation::DirectorySync,
            kind: io::ErrorKind::Other
        })
    );
    assert_eq!(region.load(chunk_key(0)).unwrap().revision, 7);
}

#[test]
fn active_extent_not_overwritten() {
    let temp = TempRegion::new();
    let mut region = RegionIo::open(&temp.path, region_key()).unwrap();
    region.save(&[save(0, 7, 1)], &IoCancellation::new());
    let before = fs::read(&temp.path).unwrap();
    region.save(&[save(0, 8, 2)], &IoCancellation::new());
    let after = fs::read(&temp.path).unwrap();
    assert_eq!(&after[15 * 4096..before.len()], &before[15 * 4096..]);
    assert_eq!(
        region.load(chunk_key(0)).unwrap().chunk,
        save(0, 8, 2).chunk
    );
}

#[test]
fn uncertain_bank_retry_refreshes_and_syncs() {
    struct FailFirstBankSync(bool);
    impl DiskIo for FailFirstBankSync {
        fn boundary(&mut self, point: IoFaultPoint, phase: IoPhase) -> io::Result<()> {
            if self.0 && point == IoFaultPoint::BankSync && phase == IoPhase::After {
                self.0 = false;
                return Err(io::ErrorKind::Other.into());
            }
            Ok(())
        }
    }
    let temp = TempRegion::new();
    let mut region = RegionIo::open(&temp.path, region_key()).unwrap();
    region.save(&[save(0, 7, 1)], &IoCancellation::new());
    region.close().unwrap();
    let mut region =
        RegionIo::with_io(&temp.path, region_key(), Box::new(FailFirstBankSync(true))).unwrap();
    let failed = region.save(&[save(0, 8, 2)], &IoCancellation::new());
    assert!(failed.committed.is_empty());
    assert_eq!(
        failed.error,
        Some(ServerError::Io {
            operation: Operation::SyncBank,
            kind: io::ErrorKind::Other
        })
    );
    assert_eq!(region.load(chunk_key(0)).unwrap().revision, 8);
    let retry = region.save(&[save(0, 8, 2)], &IoCancellation::new());
    assert_eq!(retry.committed, committed(0, 8));
    assert_eq!(retry.error, None);
    assert_eq!(
        region.load(chunk_key(0)).unwrap().chunk,
        save(0, 8, 2).chunk
    );
}

#[test]
fn uncertain_bank_barrier_failure_never_acknowledges() {
    let temp = TempRegion::new();
    let mut region = RegionIo::open(&temp.path, region_key()).unwrap();
    region.save(&[save(0, 7, 1)], &IoCancellation::new());
    region.close().unwrap();
    let mut region = RegionIo::with_io(
        &temp.path,
        region_key(),
        Box::new(Fault {
            point: IoFaultPoint::BankSync,
            phase: IoPhase::After,
            crash: false,
        }),
    )
    .unwrap();
    for _ in 0..2 {
        let result = region.save(&[save(0, 8, 2)], &IoCancellation::new());
        assert!(result.committed.is_empty());
        assert_eq!(
            result.error,
            Some(ServerError::Io {
                operation: Operation::SyncBank,
                kind: io::ErrorKind::Other,
            })
        );
    }
    assert_eq!(region.load(chunk_key(0)).unwrap().revision, 8);
}

#[test]
fn compact_atomic() {
    let temp = TempRegion::new();
    let mut region = RegionIo::open(&temp.path, region_key()).unwrap();
    for revision in 1..=8 {
        assert_eq!(
            region
                .save(
                    &[save(0, revision, revision as u16)],
                    &IoCancellation::new()
                )
                .error,
            None
        );
    }
    let before = fs::metadata(&temp.path).unwrap().len();
    region.compact(&IoCancellation::new()).unwrap();
    let after = fs::metadata(&temp.path).unwrap().len();
    assert!(after < before);
    assert_eq!(region.load(chunk_key(0)).unwrap().revision, 8);
    region.close().unwrap();
    assert_eq!(
        RegionIo::open(&temp.path, region_key())
            .unwrap()
            .load(chunk_key(0))
            .unwrap()
            .chunk,
        save(0, 8, 8).chunk
    );
}

#[test]
fn partial_regions() {
    let first = TempRegion::new();
    let second = TempRegion::new();
    let mut a = RegionIo::open(&first.path, region_key()).unwrap();
    let mut b = RegionIo::with_io(
        &second.path,
        region_key(),
        Box::new(Fault {
            point: IoFaultPoint::PayloadWrite,
            phase: IoPhase::Before,
            crash: false,
        }),
    )
    .unwrap();
    let committed = a.save(&[save(0, 7, 1)], &IoCancellation::new()).committed;
    let failed = b.save(&[save(0, 7, 2)], &IoCancellation::new());
    assert_eq!(committed.len(), 1);
    assert!(failed.committed.is_empty());
    assert!(failed.error.is_some());
    assert_eq!(a.load(chunk_key(0)).unwrap().revision, 7);
}

#[test]
fn corrupt_active_recovers_older_payload_without_rewrite() {
    let temp = TempRegion::new();
    let mut region = RegionIo::open(&temp.path, region_key()).unwrap();
    region.save(&[save(0, 7, 1)], &IoCancellation::new());
    region.save(&[save(0, 8, 2)], &IoCancellation::new());
    region.close().unwrap();
    let mut bytes = fs::read(&temp.path).unwrap();
    let active = decode_region_bank(
        region_key(),
        &bytes[BANK_A_START_SECTOR as usize * SECTOR_SIZE as usize
            ..BANK_A_START_SECTOR as usize * SECTOR_SIZE as usize + BANK_SIZE],
        bytes.len() as i64,
    )
    .unwrap();
    let payload_at = active.entries[0].offset_sector as usize * SECTOR_SIZE as usize;
    bytes[payload_at] ^= 0xff;
    fs::write(&temp.path, &bytes).unwrap();
    let before = fs::read(&temp.path).unwrap();
    let loaded = RegionIo::open(&temp.path, region_key())
        .unwrap()
        .load(chunk_key(0))
        .unwrap();
    assert_eq!(
        (
            loaded.revision,
            loaded.persisted_revision,
            loaded.needs_rewrite,
            loaded.recovered
        ),
        (9, 7, true, true)
    );
    assert_eq!(loaded.chunk, save(0, 7, 1).chunk);
    assert_eq!(fs::read(&temp.path).unwrap(), before);
}

#[test]
fn generation_ties_and_overflow() {
    let temp = TempRegion::new();
    RegionIo::open(&temp.path, region_key())
        .unwrap()
        .close()
        .unwrap();
    let mut bytes = fs::read(&temp.path).unwrap();
    let a_start = BANK_A_START_SECTOR as usize * SECTOR_SIZE as usize;
    let b_start = BANK_B_START_SECTOR as usize * SECTOR_SIZE as usize;
    let first = bytes[a_start..a_start + BANK_SIZE].to_vec();
    bytes[b_start..b_start + BANK_SIZE].copy_from_slice(&first);
    fs::write(&temp.path, &bytes).unwrap();
    let mut region = RegionIo::open(&temp.path, region_key()).unwrap();
    region.save(&[save(0, 7, 1)], &IoCancellation::new());
    region.close().unwrap();
    let bytes = fs::read(&temp.path).unwrap();
    let bank_b = decode_region_bank(
        region_key(),
        &bytes[b_start..b_start + BANK_SIZE],
        bytes.len() as i64,
    )
    .unwrap();
    assert_eq!(bank_b.generation, 2);

    let divergent = TempRegion::new();
    RegionIo::open(&divergent.path, region_key())
        .unwrap()
        .close()
        .unwrap();
    let mut region = RegionIo::open(&divergent.path, region_key()).unwrap();
    region.save(&[save(0, 7, 1)], &IoCancellation::new());
    region.save(&[save(0, 8, 2)], &IoCancellation::new());
    region.close().unwrap();
    let mut bytes = fs::read(&divergent.path).unwrap();
    let b = decode_region_bank(
        region_key(),
        &bytes[b_start..b_start + BANK_SIZE],
        bytes.len() as i64,
    )
    .unwrap();
    let mut tied = b.clone();
    tied.generation = 3;
    bytes[b_start..b_start + BANK_SIZE]
        .copy_from_slice(&encode_region_bank(region_key(), &tied).unwrap());
    fs::write(&divergent.path, &bytes).unwrap();
    assert!(matches!(
        RegionIo::open(&divergent.path, region_key()),
        Err(ServerError::Storage {
            family: "region",
            kind: StorageFailure::Corrupt
        })
    ));

    let overflow = TempRegion::new();
    RegionIo::open(&overflow.path, region_key())
        .unwrap()
        .close()
        .unwrap();
    let mut bytes = fs::read(&overflow.path).unwrap();
    let mut full = mornlea_storage::RegionBank::empty();
    full.generation = u64::MAX;
    bytes[a_start..a_start + BANK_SIZE]
        .copy_from_slice(&encode_region_bank(region_key(), &full).unwrap());
    fs::write(&overflow.path, &bytes).unwrap();
    let mut region = RegionIo::open(&overflow.path, region_key()).unwrap();
    assert!(
        region
            .save(&[save(0, 1, 1)], &IoCancellation::new())
            .error
            .is_some()
    );
    assert_eq!(fs::read(&overflow.path).unwrap(), bytes);
}

#[test]
fn unsupported_dimension_refuses_before_publication() {
    let temp = TempRegion::new();
    let key = RegionKey {
        dimension: -1,
        x: 0,
        z: 0,
    };
    let mut region = RegionIo::open(&temp.path, key).unwrap();
    let before = fs::read(&temp.path).unwrap();
    let mut request = save(0, 1, 1);
    request.key.dimension = -1;
    let result = region.save(&[request], &IoCancellation::new());
    assert_eq!(
        result.error,
        Some(ServerError::InvalidInput { field: "dimension" })
    );
    assert!(result.committed.is_empty());
    assert_eq!(fs::read(&temp.path).unwrap(), before);
}

#[test]
fn compact_atomic_fault_boundaries() {
    for (point, phase, operation) in [
        (IoFaultPoint::TempWrite, IoPhase::Before, Operation::Replace),
        (IoFaultPoint::TempSync, IoPhase::Before, Operation::Sync),
        (IoFaultPoint::Rename, IoPhase::Before, Operation::Replace),
        (IoFaultPoint::Rename, IoPhase::After, Operation::Replace),
        (
            IoFaultPoint::DirectorySync,
            IoPhase::After,
            Operation::DirectorySync,
        ),
    ] {
        let temp = TempRegion::new();
        let mut region = RegionIo::open(&temp.path, region_key()).unwrap();
        for revision in 1..=4 {
            region.save(
                &[save(0, revision, revision as u16)],
                &IoCancellation::new(),
            );
        }
        region.close().unwrap();
        let mut region = RegionIo::with_io(
            &temp.path,
            region_key(),
            Box::new(Fault {
                point,
                phase,
                crash: false,
            }),
        )
        .unwrap();
        assert_eq!(
            region.compact(&IoCancellation::new()),
            Err(ServerError::Io {
                operation,
                kind: io::ErrorKind::Other
            })
        );
        assert_eq!(
            region.load(chunk_key(0)).unwrap().chunk,
            save(0, 4, 4).chunk
        );
        region.close().unwrap();
        assert_eq!(
            RegionIo::open(&temp.path, region_key())
                .unwrap()
                .load(chunk_key(0))
                .unwrap()
                .revision,
            4
        );
    }
}

#[test]
fn cancellation_before_bank_retains_old_generation() {
    struct CancelOnPayloadSync(IoCancellation);
    impl DiskIo for CancelOnPayloadSync {
        fn boundary(&mut self, point: IoFaultPoint, phase: IoPhase) -> io::Result<()> {
            if point == IoFaultPoint::PayloadSync && phase == IoPhase::After {
                self.0.cancel();
            }
            Ok(())
        }
    }
    let temp = TempRegion::new();
    let mut region = RegionIo::open(&temp.path, region_key()).unwrap();
    region.save(&[save(0, 7, 1)], &IoCancellation::new());
    region.close().unwrap();
    let token = IoCancellation::new();
    let mut region = RegionIo::with_io(
        &temp.path,
        region_key(),
        Box::new(CancelOnPayloadSync(token.clone())),
    )
    .unwrap();
    let result = region.save(&[save(0, 8, 2)], &token);
    assert_eq!(result.error, Some(ServerError::Cancelled));
    assert!(result.committed.is_empty());
    assert_eq!(region.load(chunk_key(0)).unwrap().revision, 7);
}

#[test]
fn zero_payload_write_preserves_old_bank() {
    struct ZeroWriter;
    impl DiskIo for ZeroWriter {
        fn write(&mut self, _file: &mut File, _bytes: &[u8]) -> io::Result<usize> {
            Ok(0)
        }
    }
    let temp = TempRegion::new();
    let mut region = RegionIo::open(&temp.path, region_key()).unwrap();
    region.save(&[save(0, 7, 1)], &IoCancellation::new());
    region.close().unwrap();
    let mut region = RegionIo::with_io(&temp.path, region_key(), Box::new(ZeroWriter)).unwrap();
    let result = region.save(&[save(0, 8, 2)], &IoCancellation::new());
    assert!(result.committed.is_empty());
    assert_eq!(
        result.error,
        Some(ServerError::Io {
            operation: Operation::WritePayload,
            kind: io::ErrorKind::WriteZero,
        })
    );
    assert_eq!(
        region.load(chunk_key(0)).unwrap().chunk,
        save(0, 7, 1).chunk
    );
}

#[test]
fn four_crash_points_old_or_new() {
    for (point, new_required) in [
        (IoFaultPoint::PayloadWrite, false),
        (IoFaultPoint::PayloadSync, false),
        (IoFaultPoint::BankWrite, false),
        (IoFaultPoint::BankSync, true),
    ] {
        let temp = TempRegion::new();
        let mut region = RegionIo::open(&temp.path, region_key()).unwrap();
        region.save(&[save(0, 7, 1)], &IoCancellation::new());
        region.close().unwrap();
        let child = Command::new(std::env::current_exe().unwrap())
            .arg("--exact")
            .arg("region_io::crash_child")
            .env("MORNLEA_REGION_TEST_PATH", &temp.path)
            .env("MORNLEA_REGION_TEST_POINT", format!("{point:?}"))
            .output()
            .unwrap();
        assert_eq!(child.status.code(), Some(86), "{child:?}");
        let mut reopened = RegionIo::open(&temp.path, region_key()).unwrap();
        let loaded = reopened.load(chunk_key(0)).unwrap();
        assert!(loaded.revision == 7 || loaded.revision == 8);
        if new_required {
            assert_eq!(loaded.revision, 8);
        }
        assert_eq!(
            loaded.chunk,
            save(0, loaded.revision, if loaded.revision == 7 { 1 } else { 2 }).chunk
        );
    }
}

#[test]
fn crash_child() {
    let Ok(path) = std::env::var("MORNLEA_REGION_TEST_PATH") else {
        return;
    };
    let point = match std::env::var("MORNLEA_REGION_TEST_POINT").unwrap().as_str() {
        "PayloadWrite" => IoFaultPoint::PayloadWrite,
        "PayloadSync" => IoFaultPoint::PayloadSync,
        "BankWrite" => IoFaultPoint::BankWrite,
        "BankSync" => IoFaultPoint::BankSync,
        _ => panic!("unknown point"),
    };
    let mut region = RegionIo::with_io(
        PathBuf::from(path).as_path(),
        region_key(),
        Box::new(Fault {
            point,
            phase: IoPhase::After,
            crash: true,
        }),
    )
    .unwrap();
    region.save(&[save(0, 8, 2)], &IoCancellation::new());
    panic!("fault not reached");
}
