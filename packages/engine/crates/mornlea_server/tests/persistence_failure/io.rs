use mornlea_server::contracts::*;
use mornlea_server::store::io::{
    DiskIo, IoCancellation, NativeDiskIo, io_error, storage_error, write_all,
};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::sync::atomic::{AtomicU64, Ordering};

static TEMP_ID: AtomicU64 = AtomicU64::new(0);
fn temporary_file() -> (std::path::PathBuf, File) {
    let path = std::env::temp_dir().join(format!(
        "mornlea-io-{}-{}",
        std::process::id(),
        TEMP_ID.fetch_add(1, Ordering::Relaxed)
    ));
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .open(&path)
        .unwrap();
    (path, file)
}

#[test]
fn cancel_clone() {
    let cancellation = IoCancellation::new();
    let worker = cancellation.clone();
    assert_eq!(worker.check(), Ok(()));
    cancellation.cancel();
    assert_eq!(worker.check(), Err(ServerError::Cancelled));
    assert_eq!(IoCancellation::new().check(), Ok(()));
}

#[test]
fn error_classes() {
    for (error, kind) in [
        (
            mornlea_storage::StorageError::Corrupt("bad crc".into()),
            StorageFailure::Corrupt,
        ),
        (
            mornlea_storage::StorageError::FutureVersion("opaque description".into()),
            StorageFailure::FutureVersion,
        ),
        (
            mornlea_storage::StorageError::OutputTooSmall {
                needed: 2,
                available: 1,
            },
            StorageFailure::OutputTooSmall,
        ),
    ] {
        assert_eq!(
            storage_error("player", error),
            ServerError::Storage {
                family: "player",
                kind
            }
        );
    }
    for kind in [
        io::ErrorKind::NotFound,
        io::ErrorKind::PermissionDenied,
        io::ErrorKind::UnexpectedEof,
    ] {
        assert_eq!(
            io_error(Operation::Load, io::Error::from(kind)),
            ServerError::Io {
                operation: Operation::Load,
                kind
            }
        );
    }
}

struct ShortWriter {
    interrupt: bool,
    zero: bool,
}
impl DiskIo for ShortWriter {
    fn write(&mut self, file: &mut File, bytes: &[u8]) -> io::Result<usize> {
        if std::mem::take(&mut self.interrupt) {
            return Err(io::ErrorKind::Interrupted.into());
        }
        if self.zero {
            return Ok(0);
        }
        file.write(&bytes[..bytes.len().min(2)])
    }
}

#[test]
fn partial_interrupted_write() {
    let (path, mut file) = temporary_file();
    write_all(
        &mut ShortWriter {
            interrupt: true,
            zero: false,
        },
        &mut file,
        b"complete",
    )
    .unwrap();
    file.sync_all().unwrap();
    NativeDiskIo.close(file).unwrap();
    assert_eq!(fs::read(&path).unwrap(), b"complete");
    fs::remove_file(path).unwrap();
}

#[test]
fn zero_write_refuses() {
    let (path, mut file) = temporary_file();
    let error = write_all(
        &mut ShortWriter {
            interrupt: false,
            zero: true,
        },
        &mut file,
        b"never",
    )
    .unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::WriteZero);
    assert_eq!(file.metadata().unwrap().len(), 0);
    NativeDiskIo.close(file).unwrap();
    fs::remove_file(path).unwrap();
}

struct LoadDouble {
    value: Option<LoadedValue>,
}
impl DiskBackend for LoadDouble {
    fn load(&mut self, _key: SaveKey) -> Result<LoadedValue, ServerError> {
        Ok(self.value.take().unwrap())
    }
    fn write(&mut self, _ticket: SaveTicket, _request: SaveRequest) -> SaveCompletion {
        unreachable!("load-only double")
    }
    fn sync(&mut self) -> Result<(), ServerError> {
        Ok(())
    }
    fn close(&mut self) -> Result<(), ServerError> {
        Ok(())
    }
}

#[test]
fn decoded_migration_and_partial_commit() {
    let old = mornlea_storage::StoredCompanions {
        source_schema: 4,
        revision: 7,
        ..Default::default()
    };
    let mut backend = LoadDouble {
        value: Some(LoadedValue::Companions(old.clone())),
    };
    assert_eq!(
        backend.load(SaveKey::Companions).unwrap(),
        LoadedValue::Companions(old)
    );
    let mut player_id = [0; 16];
    player_id[0] = 1;
    player_id[6] = 0x40;
    player_id[8] = 0x80;
    let player_key = SaveKey::Player(mornlea_domain::PlayerId::try_from_bytes(player_id).unwrap());
    let body = mornlea_storage::StoredPlayer {
        player_id: mornlea_storage::PlayerId::from_bytes(player_id),
        revision: 7,
        display_name: "Player".into(),
        current: mornlea_storage::PlayerLocation {
            dimension: 0,
            position: [0.0, 64.0, 0.0],
        },
        yaw: 0.0,
        pitch: 0.0,
        safe: None,
        inventory: Default::default(),
        health: 20,
        hunger: 20,
        saturation_milli: 5000,
        exhaustion_milli: 0,
        respawn_present: false,
        respawn_position: [0.0; 3],
        respawn_dimension: 0,
        armor: Default::default(),
        needs_rewrite: true,
    };
    let mut backend = LoadDouble {
        value: Some(LoadedValue::Player(body.clone())),
    };
    assert_eq!(backend.load(player_key).unwrap(), LoadedValue::Player(body));
    let failed = ServerError::Io {
        operation: Operation::WriteBank,
        kind: io::ErrorKind::Other,
    };
    let result = DiskWriteOutcome {
        committed: vec![(SaveKey::Companions, 7)],
        error: Some(failed),
    };
    assert_eq!(result.committed, [(SaveKey::Companions, 7)]);
    assert_eq!(result.error, Some(failed));
}
