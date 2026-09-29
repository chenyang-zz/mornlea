//! Standalone save files under an exclusively owned world root.

use crate::core::contracts::{
    IoFaultPoint, LoadedValue, Operation, OwnedSnapshot, SaveKey, SaveValue, ServerError,
    StorageFailure,
};
use crate::store::io::{
    DiskIo, IoCancellation, IoPhase, NativeDiskIo, io_error, storage_error, write_all,
};
use mornlea_storage as storage;
use std::collections::HashSet;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

static TEMP_ID: AtomicU64 = AtomicU64::new(0);

/// The caller owns the world lease; this owner serializes standalone files.
/// A failed publication leaves the canonical bytes intact or complete, but
/// never publishes a durable revision before its parent directory sync.
pub struct AtomicFiles {
    root: PathBuf,
    io: Box<dyn DiskIo>,
    durable: HashSet<PathBuf>,
    metadata_candidate: Option<MetadataCandidate>,
    closed: bool,
}

struct MetadataCandidate {
    sequence: u64,
    bytes: Vec<u8>,
    durable: bool,
}

impl AtomicFiles {
    pub fn open(root: &Path) -> Result<Self, ServerError> {
        Self::with_io(root, Box::<NativeDiskIo>::default())
    }

    pub fn with_io(root: &Path, io: Box<dyn DiskIo>) -> Result<Self, ServerError> {
        let root_metadata = fs::metadata(root).map_err(|error| io_error(Operation::Load, error))?;
        if !root_metadata.is_dir() {
            return Err(ServerError::InvalidInput {
                field: "world_root",
            });
        }
        let players = root.join("players");
        match fs::symlink_metadata(&players) {
            Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {}
            Ok(_) => {
                return Err(ServerError::InvalidInput {
                    field: "players_directory",
                });
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                fs::create_dir(&players).map_err(|error| io_error(Operation::Replace, error))?;
            }
            Err(error) => return Err(io_error(Operation::Load, error)),
        }
        let mut files = Self {
            root: root.to_owned(),
            io,
            durable: HashSet::new(),
            metadata_candidate: None,
            closed: false,
        };
        // The world owner creates the root; this provider makes the players
        // entry durable on both first open and a retry after a failed barrier.
        files.sync_directory(root)?;
        Ok(files)
    }

    pub fn load(&mut self, key: SaveKey) -> Result<LoadedValue, ServerError> {
        self.ensure_open(Operation::Load)?;
        let (path, family, limit) = self.path_and_limit(&key)?;
        let bytes = self.read_bytes(&path, family, limit)?;
        decode(&key, &bytes)
    }

    pub fn save(
        &mut self,
        snapshot: &OwnedSnapshot,
        cancel: &IoCancellation,
    ) -> Result<u64, ServerError> {
        self.ensure_open(Operation::Replace)?;
        cancel.check()?;
        if matches!(snapshot.key, SaveKey::Metadata) && snapshot.revision == 0 {
            return Err(ServerError::InvalidInput {
                field: "metadata_sequence",
            });
        }
        let (path, family, limit) = self.path_and_limit(&snapshot.key)?;
        let (bytes, embedded_revision) = encode(snapshot, family)?;
        if bytes.len() > limit {
            return Err(ServerError::Storage {
                family,
                kind: StorageFailure::Corrupt,
            });
        }
        if let Some(revision) = embedded_revision
            && revision != snapshot.revision
        {
            return Err(ServerError::InvalidInput { field: "revision" });
        }
        if matches!(snapshot.key, SaveKey::Metadata)
            && let Some(candidate) = &self.metadata_candidate
        {
            if snapshot.revision < candidate.sequence
                || (snapshot.revision == candidate.sequence && bytes != candidate.bytes)
            {
                return Err(ServerError::InvalidInput { field: "revision" });
            }
            if snapshot.revision == candidate.sequence {
                cancel.check()?;
                if !candidate.durable {
                    self.sync_directory(path.parent().expect("owned file has parent"))?;
                    self.metadata_candidate
                        .as_mut()
                        .expect("candidate retained across barrier")
                        .durable = true;
                }
                return Ok(snapshot.revision);
            }
        }
        let old = match self.read_bytes(&path, family, limit) {
            Ok(bytes) => Some(bytes),
            Err(ServerError::Io {
                operation: Operation::Load,
                kind: io::ErrorKind::NotFound,
            }) => None,
            Err(error) => return Err(error),
        };
        if let Some(old) = &old {
            let loaded = decode(&snapshot.key, old)?;
            if let Some(old_revision) = loaded_revision(&loaded) {
                if snapshot.revision < old_revision
                    || (snapshot.revision == old_revision && bytes != *old)
                {
                    return Err(ServerError::InvalidInput { field: "revision" });
                }
                if snapshot.revision == old_revision {
                    cancel.check()?;
                    return self.acknowledge_existing(&path, snapshot.revision);
                }
            }
        }
        cancel.check()?;
        let metadata_sequence =
            matches!(snapshot.key, SaveKey::Metadata).then_some(snapshot.revision);
        self.replace(&path, &bytes, cancel, metadata_sequence)?;
        self.durable.insert(path);
        Ok(snapshot.revision)
    }

    pub fn sync(&mut self) -> Result<(), ServerError> {
        self.ensure_open(Operation::Sync)?;
        self.sync_directory(&self.root.clone())?;
        self.sync_directory(&self.root.join("players"))
    }

    pub fn close(&mut self) -> Result<(), ServerError> {
        self.closed = true;
        Ok(())
    }

    fn ensure_open(&self, operation: Operation) -> Result<(), ServerError> {
        if self.closed {
            Err(ServerError::Io {
                operation,
                kind: io::ErrorKind::NotConnected,
            })
        } else {
            Ok(())
        }
    }

    fn path_and_limit(&self, key: &SaveKey) -> Result<(PathBuf, &'static str, usize), ServerError> {
        let result = match key {
            SaveKey::Chunk(_) => return Err(ServerError::InvalidInput { field: "save_key" }),
            SaveKey::Player(id) => {
                let raw = id.bytes();
                let name = format!(
                    "{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}.player",
                    raw[0],
                    raw[1],
                    raw[2],
                    raw[3],
                    raw[4],
                    raw[5],
                    raw[6],
                    raw[7],
                    raw[8],
                    raw[9],
                    raw[10],
                    raw[11],
                    raw[12],
                    raw[13],
                    raw[14],
                    raw[15]
                );
                (
                    self.root.join("players").join(name),
                    "player",
                    storage::PLAYER_ENVELOPE_LENGTH + storage::PLAYER_MAX_PAYLOAD,
                )
            }
            SaveKey::Companions => (
                self.root.join("companions.ai"),
                "companions",
                storage::COMPANION_MAX_FILE_LENGTH,
            ),
            SaveKey::Hostiles => (
                self.root.join("hostile_mobs.bin"),
                "hostiles",
                storage::HOSTILE_MAX_FILE_LENGTH,
            ),
            SaveKey::Passives => (
                self.root.join("passive_mobs.bin"),
                "passives",
                storage::PASSIVE_MAX_FILE_LENGTH,
            ),
            SaveKey::Metadata => (self.root.join("world.meta"), "metadata", 78),
        };
        Ok(result)
    }

    fn read_bytes(
        &mut self,
        path: &Path,
        family: &'static str,
        limit: usize,
    ) -> Result<Vec<u8>, ServerError> {
        let file = File::open(path).map_err(|error| io_error(Operation::Load, error))?;
        let mut bytes = Vec::new();
        let read_result = (&file).take((limit + 1) as u64).read_to_end(&mut bytes);
        let close_result = self.io.close(file);
        read_result.map_err(|error| io_error(Operation::Load, error))?;
        close_result.map_err(|error| io_error(Operation::Load, error))?;
        if bytes.len() > limit {
            // The prefix is enough to keep a future schema distinguishable
            // from an oversized corrupt current file without reading it all.
            let kind = if future_header(family, &bytes) {
                StorageFailure::FutureVersion
            } else {
                StorageFailure::Corrupt
            };
            return Err(ServerError::Storage { family, kind });
        }
        Ok(bytes)
    }

    fn acknowledge_existing(&mut self, path: &Path, revision: u64) -> Result<u64, ServerError> {
        if !self.durable.contains(path) {
            self.sync_directory(path.parent().expect("owned file has parent"))?;
            self.durable.insert(path.to_owned());
        }
        Ok(revision)
    }

    fn replace(
        &mut self,
        path: &Path,
        bytes: &[u8],
        cancel: &IoCancellation,
        metadata_sequence: Option<u64>,
    ) -> Result<(), ServerError> {
        let parent = path.parent().expect("owned file has parent");
        let name = path
            .file_name()
            .expect("owned file has name")
            .to_string_lossy();
        let temporary = parent.join(format!(
            ".{name}.tmp-{}-{}",
            std::process::id(),
            TEMP_ID.fetch_add(1, Ordering::Relaxed)
        ));
        let mut file =
            create_temp(&temporary).map_err(|error| io_error(Operation::Replace, error))?;
        let staged = self.stage_temp(&mut file, bytes);
        let closed = self
            .io
            .close(file)
            .map_err(|error| io_error(Operation::Close, error));
        if let Err(error) = staged.and(closed) {
            let _ = fs::remove_file(&temporary);
            return Err(error);
        }
        if let Err(error) = cancel.check() {
            let _ = fs::remove_file(&temporary);
            return Err(error);
        }
        if let Err(error) = self.io.boundary(IoFaultPoint::Rename, IoPhase::Before) {
            let _ = fs::remove_file(&temporary);
            return Err(io_error(Operation::Replace, error));
        }
        if let Err(error) = fs::rename(&temporary, path) {
            let _ = fs::remove_file(&temporary);
            return Err(io_error(Operation::Replace, error));
        }
        self.durable.remove(path);
        if let Some(sequence) = metadata_sequence {
            self.metadata_candidate = Some(MetadataCandidate {
                sequence,
                bytes: bytes.to_vec(),
                durable: false,
            });
        }
        // Publication has happened. Complete the parent barrier even if the
        // after hook reports a fault; that fault still forbids acknowledgment.
        let after = self
            .io
            .boundary(IoFaultPoint::Rename, IoPhase::After)
            .map_err(|error| io_error(Operation::Replace, error));
        let barrier = self.sync_directory(parent);
        let outcome = after.and(barrier);
        if outcome.is_ok()
            && metadata_sequence.is_some()
            && let Some(candidate) = self.metadata_candidate.as_mut()
        {
            candidate.durable = true;
        }
        outcome
    }

    fn stage_temp(&mut self, file: &mut File, bytes: &[u8]) -> Result<(), ServerError> {
        self.io
            .boundary(IoFaultPoint::TempWrite, IoPhase::Before)
            .map_err(|error| io_error(Operation::WritePayload, error))?;
        write_all(self.io.as_mut(), file, bytes)
            .map_err(|error| io_error(Operation::WritePayload, error))?;
        self.io
            .boundary(IoFaultPoint::TempWrite, IoPhase::After)
            .map_err(|error| io_error(Operation::WritePayload, error))?;
        self.io
            .boundary(IoFaultPoint::TempSync, IoPhase::Before)
            .map_err(|error| io_error(Operation::SyncPayload, error))?;
        file.sync_all()
            .map_err(|error| io_error(Operation::SyncPayload, error))?;
        self.io
            .boundary(IoFaultPoint::TempSync, IoPhase::After)
            .map_err(|error| io_error(Operation::SyncPayload, error))
    }

    fn sync_directory(&mut self, path: &Path) -> Result<(), ServerError> {
        let file = File::open(path).map_err(|error| io_error(Operation::DirectorySync, error))?;
        let barrier = self
            .io
            .boundary(IoFaultPoint::DirectorySync, IoPhase::Before)
            .and_then(|()| file.sync_all())
            .and_then(|()| {
                self.io
                    .boundary(IoFaultPoint::DirectorySync, IoPhase::After)
            })
            .map_err(|error| io_error(Operation::DirectorySync, error));
        let closed = self
            .io
            .close(file)
            .map_err(|error| io_error(Operation::DirectorySync, error));
        barrier.and(closed)
    }
}

fn future_header(family: &str, bytes: &[u8]) -> bool {
    let field = |offset: usize| {
        bytes
            .get(offset..offset + 4)
            .and_then(|value| value.try_into().ok())
            .map(u32::from_le_bytes)
    };
    match family {
        "metadata" => {
            bytes.starts_with(b"MCGM")
                && field(4).is_some_and(|v| v > storage::METADATA_CURRENT_VERSION)
        }
        "player" => {
            bytes.starts_with(b"MCPL")
                && (field(4).is_some_and(|v| v > 1)
                    || (field(4) == Some(1)
                        && field(8).is_some_and(|v| v > storage::PLAYER_CURRENT_SCHEMA)))
        }
        "companions" => {
            bytes.starts_with(b"MCAI")
                && (field(4).is_some_and(|v| v > storage::COMPANION_ENVELOPE_VERSION)
                    || (field(4) == Some(storage::COMPANION_ENVELOPE_VERSION)
                        && field(8).is_some_and(|v| v > storage::COMPANION_CURRENT_SCHEMA)))
        }
        "hostiles" => {
            bytes.starts_with(b"MHST")
                && (field(4).is_some_and(|v| v > storage::HOSTILE_ENVELOPE_VERSION)
                    || (field(4) == Some(storage::HOSTILE_ENVELOPE_VERSION)
                        && field(8).is_some_and(|v| v > storage::HOSTILE_CURRENT_SCHEMA)))
        }
        "passives" => {
            bytes.starts_with(b"PMST")
                && (field(4).is_some_and(|v| v > storage::PASSIVE_ENVELOPE_VERSION)
                    || (field(4) == Some(storage::PASSIVE_ENVELOPE_VERSION)
                        && field(8).is_some_and(|v| v > storage::PASSIVE_CURRENT_SCHEMA)))
        }
        _ => false,
    }
}

fn create_temp(path: &Path) -> io::Result<File> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(path)
}

fn encode(
    snapshot: &OwnedSnapshot,
    family: &'static str,
) -> Result<(Vec<u8>, Option<u64>), ServerError> {
    let result = match (&snapshot.key, &snapshot.value) {
        (SaveKey::Player(id), SaveValue::Player(save))
            if id.bytes() == save.player_id.to_bytes() =>
        {
            (
                storage::encode_player(save).map_err(|error| storage_error(family, error))?,
                Some(save.revision),
            )
        }
        (SaveKey::Companions, SaveValue::Companions(save)) => (
            storage::encode_companions(save).map_err(|error| storage_error(family, error))?,
            Some(save.revision),
        ),
        (SaveKey::Hostiles, SaveValue::Hostiles(save)) => (
            storage::encode_hostile_mobs(save).map_err(|error| storage_error(family, error))?,
            Some(save.revision),
        ),
        (SaveKey::Passives, SaveValue::Passives(save)) => (
            storage::encode_passive_mobs(save).map_err(|error| storage_error(family, error))?,
            Some(save.revision),
        ),
        (SaveKey::Metadata, SaveValue::Metadata(save)) => (
            storage::encode_world_metadata(save).map_err(|error| storage_error(family, error))?,
            None,
        ),
        _ => {
            return Err(ServerError::InvalidInput {
                field: "save_value",
            });
        }
    };
    Ok(result)
}

fn decode(key: &SaveKey, bytes: &[u8]) -> Result<LoadedValue, ServerError> {
    match key {
        SaveKey::Player(id) => {
            storage::decode_player(storage::PlayerId::from_bytes(id.bytes()), bytes)
                .map(LoadedValue::Player)
                .map_err(|error| storage_error("player", error))
        }
        SaveKey::Companions => storage::decode_companions(bytes)
            .map(LoadedValue::Companions)
            .map_err(|error| storage_error("companions", error)),
        SaveKey::Hostiles => storage::decode_hostile_mobs(bytes)
            .map(LoadedValue::Hostiles)
            .map_err(|error| storage_error("hostiles", error)),
        SaveKey::Passives => storage::decode_passive_mobs(bytes)
            .map(LoadedValue::Passives)
            .map_err(|error| storage_error("passives", error)),
        SaveKey::Metadata => storage::decode_world_metadata(bytes)
            .map(LoadedValue::Metadata)
            .map_err(|error| storage_error("metadata", error)),
        SaveKey::Chunk(_) => Err(ServerError::InvalidInput { field: "save_key" }),
    }
}

fn loaded_revision(value: &LoadedValue) -> Option<u64> {
    match value {
        LoadedValue::Player(value) => Some(value.revision),
        LoadedValue::Companions(value) => Some(value.revision),
        LoadedValue::Hostiles(value) => Some(value.revision),
        LoadedValue::Passives(value) => Some(value.revision),
        LoadedValue::Metadata(_) | LoadedValue::Chunk(_) => None,
    }
}
