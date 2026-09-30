//! Serial disk owner joining the accepted region and standalone providers.

use crate::core::contracts::{
    DiskBackend, IoFaultPoint, LoadedValue, Operation, OwnedSnapshot, SaveCompletion, SaveKey,
    SaveRequest, SaveTicket, SaveValue, ServerError,
};
use crate::store::atomic_file::AtomicFiles;
use crate::store::io::{DiskIo, IoCancellation, IoPhase, NativeDiskIo, io_error, storage_error};
use crate::store::lease::WorldLease;
use crate::store::region_io::RegionIo;
use mornlea_storage::{self as storage, ChunkKey, ChunkSave, Metadata, RegionKey, region_for};
use std::cmp::Ordering;
use std::fs::{self, File};
use std::io;
use std::path::{Path, PathBuf};

pub struct DiskOptions {
    pub create: Metadata,
    pub region_handle_cap: usize,
}

struct CachedRegion {
    key: RegionKey,
    used: u64,
    owner: RegionIo,
}

/// This owner is serialized by the caller's worker. Its lease outlives every
/// incomplete close and every region descriptor, including failed barriers.
pub struct DiskStore {
    root: PathBuf,
    metadata: Metadata,
    lease: WorldLease,
    files: AtomicFiles,
    regions: Vec<CachedRegion>,
    cap: usize,
    clock: u64,
    factory: Box<dyn Fn() -> Box<dyn DiskIo> + Send>,
    frozen: bool,
    synced: bool,
    files_closed: bool,
    lease_released: bool,
}

impl DiskStore {
    pub fn open(root: &Path, options: DiskOptions) -> Result<Self, ServerError> {
        Self::with_io(root, options, Box::new(|| Box::new(NativeDiskIo)))
    }

    pub fn with_io(
        root: &Path,
        options: DiskOptions,
        factory: Box<dyn Fn() -> Box<dyn DiskIo> + Send>,
    ) -> Result<Self, ServerError> {
        fs::create_dir_all(root).map_err(|error| io_error(Operation::Replace, error))?;
        let lease = WorldLease::acquire(root)?;
        let mut files = AtomicFiles::with_io(root, factory())?;
        let metadata = match files.load(SaveKey::Metadata) {
            Ok(LoadedValue::Metadata(value)) => value,
            Err(ServerError::Io {
                operation: Operation::Load,
                kind: io::ErrorKind::NotFound,
            }) => {
                let snapshot = OwnedSnapshot {
                    key: SaveKey::Metadata,
                    revision: 1,
                    estimated_bytes: 78,
                    urgency: crate::core::contracts::SaveUrgency::Autosave,
                    value: SaveValue::Metadata(options.create.clone()),
                };
                files.save(&snapshot, &IoCancellation::new())?;
                options.create
            }
            Ok(_) => unreachable!("metadata key decodes to metadata"),
            Err(error) => return Err(error),
        };
        Ok(Self {
            root: root.to_owned(),
            metadata,
            lease,
            files,
            regions: Vec::new(),
            cap: if options.region_handle_cap == 0 {
                256
            } else {
                options.region_handle_cap
            },
            clock: 0,
            factory,
            frozen: false,
            synced: false,
            files_closed: false,
            lease_released: false,
        })
    }

    pub fn metadata(&self) -> &Metadata {
        &self.metadata
    }

    pub fn initial_metadata_sequence(&self) -> u64 {
        2
    }

    pub fn open_region_count(&self) -> usize {
        self.regions.len()
    }

    fn ensure_open(&self, operation: Operation) -> Result<(), ServerError> {
        if self.frozen {
            Err(ServerError::Io {
                operation,
                kind: io::ErrorKind::NotConnected,
            })
        } else {
            Ok(())
        }
    }

    fn region_path(&self, key: RegionKey) -> PathBuf {
        self.root
            .join("dimensions")
            .join(key.dimension.to_string())
            .join("regions")
            .join(format!("r.{}.{}.region", key.x, key.z))
    }

    fn sync_directory(&self, path: &Path) -> Result<(), ServerError> {
        let mut io = (self.factory)();
        let directory =
            File::open(path).map_err(|error| io_error(Operation::DirectorySync, error))?;
        let result = io
            .boundary(IoFaultPoint::DirectorySync, IoPhase::Before)
            .and_then(|()| directory.sync_all())
            .and_then(|()| io.boundary(IoFaultPoint::DirectorySync, IoPhase::After))
            .map_err(|error| io_error(Operation::DirectorySync, error));
        let closed = io
            .close(directory)
            .map_err(|error| io_error(Operation::DirectorySync, error));
        result.and(closed)
    }

    fn ensure_region_directory(&self, key: RegionKey) -> Result<(), ServerError> {
        let dimension = self.root.join("dimensions");
        let child = dimension.join(key.dimension.to_string());
        let regions = child.join("regions");
        for (parent, path) in [
            (&self.root, &dimension),
            (&dimension, &child),
            (&child, &regions),
        ] {
            match fs::symlink_metadata(path) {
                Ok(info) if info.is_dir() && !info.file_type().is_symlink() => {}
                Ok(_) => {
                    return Err(ServerError::InvalidInput {
                        field: "region_directory",
                    });
                }
                Err(error) if error.kind() == io::ErrorKind::NotFound => {
                    fs::create_dir(path).map_err(|error| io_error(Operation::Replace, error))?;
                }
                Err(error) => return Err(io_error(Operation::Load, error)),
            }
            // Repeating the parent barrier also repairs an earlier uncertain create.
            self.sync_directory(parent)?;
        }
        Ok(())
    }

    fn touch(&mut self, index: usize) {
        if self.clock == u64::MAX {
            let mut order: Vec<usize> = (0..self.regions.len()).collect();
            order.sort_by(|&a, &b| {
                self.regions[a]
                    .used
                    .cmp(&self.regions[b].used)
                    .then_with(|| region_order(self.regions[a].key, self.regions[b].key))
            });
            for (rank, index) in order.into_iter().enumerate() {
                self.regions[index].used = rank as u64 + 1;
            }
            self.clock = self.regions.len() as u64;
        }
        self.clock += 1;
        self.regions[index].used = self.clock;
    }

    fn region(&mut self, key: RegionKey, create: bool) -> Result<&mut RegionIo, ServerError> {
        if let Some(index) = self.regions.iter().position(|entry| entry.key == key) {
            self.touch(index);
            return Ok(&mut self.regions[index].owner);
        }
        let path = self.region_path(key);
        if !create {
            fs::symlink_metadata(&path).map_err(|error| io_error(Operation::Load, error))?;
        } else {
            self.ensure_region_directory(key)?;
        }
        if self.regions.len() == self.cap {
            let victim = (0..self.regions.len())
                .min_by(|&a, &b| {
                    self.regions[a]
                        .used
                        .cmp(&self.regions[b].used)
                        .then_with(|| region_order(self.regions[a].key, self.regions[b].key))
                })
                .expect("nonzero cap");
            self.regions[victim].owner.sync()?;
            // RegionIo consumes its descriptor even when native close reports
            // failure. Keep the lease, report that failure, and reopen on retry.
            let closed = self.regions[victim].owner.close();
            self.regions.remove(victim);
            closed?;
        }
        let owner = RegionIo::with_io(&path, key, (self.factory)())?;
        self.regions.push(CachedRegion {
            key,
            used: 0,
            owner,
        });
        let index = self.regions.len() - 1;
        self.touch(index);
        Ok(&mut self.regions[index].owner)
    }

    fn write_selected(
        &mut self,
        selected: &[OwnedSnapshot],
        committed: &mut Vec<(SaveKey, u64)>,
    ) -> Result<(), ServerError> {
        let cancel = IoCancellation::new();
        let mut offset = 0;
        while offset < selected.len() {
            match &selected[offset].key {
                SaveKey::Chunk(key) => {
                    let region_key = region_for(storage_key(*key)?).0;
                    let mut end = offset + 1;
                    while end < selected.len() {
                        if let SaveKey::Chunk(next) = selected[end].key
                            && region_for(storage_key(next)?).0 == region_key
                        {
                            end += 1;
                            continue;
                        }
                        break;
                    }
                    let saves: Vec<ChunkSave> = selected[offset..end]
                        .iter()
                        .map(|snapshot| {
                            let SaveValue::Chunk(save) = &snapshot.value else {
                                unreachable!()
                            };
                            save.clone()
                        })
                        .collect();
                    let owner = self.region(region_key, true)?;
                    let outcome = owner.save(&saves, &cancel);
                    committed.extend(outcome.committed);
                    if let Some(error) = outcome.error {
                        return Err(error);
                    }
                    if owner.should_compact(8 * 1024 * 1024, 0.25)? {
                        owner.compact(&cancel)?;
                    }
                    offset = end;
                }
                _ => {
                    let snapshot = &selected[offset];
                    let revision = self.files.save(snapshot, &cancel)?;
                    committed.push((snapshot.key.clone(), revision));
                    if let SaveValue::Metadata(value) = &snapshot.value {
                        self.metadata = value.clone();
                    }
                    offset += 1;
                }
            }
        }
        Ok(())
    }

    pub fn backup(
        &mut self,
        destination: &Path,
        cancel: &IoCancellation,
    ) -> Result<(), ServerError> {
        self.ensure_open(Operation::Load)?;
        crate::store::recovery::backup(self, destination, cancel)
    }

    pub(crate) fn backup_context(&self) -> (&Path, &Metadata, &dyn Fn() -> Box<dyn DiskIo>) {
        (&self.root, &self.metadata, self.factory.as_ref())
    }

    pub(crate) fn sync_for_backup(&mut self) -> Result<(), ServerError> {
        self.sync()
    }

    fn sync_inner(&mut self) -> Result<(), ServerError> {
        for region in &mut self.regions {
            region.owner.sync()?;
        }
        self.files.sync()
    }
}

impl DiskBackend for DiskStore {
    fn write(&mut self, ticket: SaveTicket, request: SaveRequest) -> SaveCompletion {
        let submitted = request
            .snapshots
            .iter()
            .map(|snapshot| (snapshot.key.clone(), snapshot.revision))
            .collect();
        let mut completion = SaveCompletion {
            ticket,
            snapshots: request.snapshots,
            submitted,
            committed: Vec::new(),
            error: None,
        };
        if let Err(error) = self.ensure_open(Operation::Replace) {
            completion.error = Some(error);
            return completion;
        }
        let selected = match select_and_validate(&completion.snapshots) {
            Ok(selected) => selected,
            Err(error) => {
                completion.error = Some(error);
                return completion;
            }
        };
        if let Err(error) = self.write_selected(&selected, &mut completion.committed) {
            completion.error = Some(error);
        }
        completion
    }

    fn load(&mut self, key: SaveKey) -> Result<LoadedValue, ServerError> {
        self.ensure_open(Operation::Load)?;
        match key {
            SaveKey::Chunk(key) => {
                let storage_key = storage_key(key)?;
                let region_key = region_for(storage_key).0;
                self.region(region_key, false)?
                    .load(storage_key)
                    .map(LoadedValue::Chunk)
            }
            other => self.files.load(other),
        }
    }

    fn sync(&mut self) -> Result<(), ServerError> {
        self.ensure_open(Operation::Sync)?;
        self.sync_inner()
    }

    fn close(&mut self) -> Result<(), ServerError> {
        if self.lease_released {
            return Ok(());
        }
        self.frozen = true;
        if !self.synced {
            self.sync_inner()?;
            self.synced = true;
        }
        self.regions.sort_by(|a, b| region_order(a.key, b.key));
        while !self.regions.is_empty() {
            let closed = self.regions[0].owner.close();
            self.regions.remove(0);
            closed?;
        }
        if !self.files_closed {
            self.files.close()?;
            self.files_closed = true;
        }
        self.lease.release()?;
        self.lease_released = true;
        Ok(())
    }
}

fn storage_key(key: crate::core::contracts::ChunkKey) -> Result<ChunkKey, ServerError> {
    Ok(ChunkKey {
        dimension: i32::from(key.dimension.get()),
        x: key.pos.x(),
        z: key.pos.z(),
    })
}

fn region_order(a: RegionKey, b: RegionKey) -> Ordering {
    (a.dimension, a.x, a.z).cmp(&(b.dimension, b.x, b.z))
}

fn save_order(a: &SaveKey, b: &SaveKey) -> Ordering {
    match (a, b) {
        (SaveKey::Chunk(a), SaveKey::Chunk(b)) => {
            let ar = region_for(storage_key(*a).expect("validated dimension")).0;
            let br = region_for(storage_key(*b).expect("validated dimension")).0;
            region_order(ar, br).then_with(|| {
                (a.dimension.get(), a.pos.x(), a.pos.z()).cmp(&(
                    b.dimension.get(),
                    b.pos.x(),
                    b.pos.z(),
                ))
            })
        }
        (SaveKey::Player(a), SaveKey::Player(b)) => a.bytes().cmp(&b.bytes()),
        _ => save_rank(a).cmp(&save_rank(b)),
    }
}

fn save_rank(key: &SaveKey) -> u8 {
    match key {
        SaveKey::Chunk(_) => 0,
        SaveKey::Player(_) => 1,
        SaveKey::Companions => 2,
        SaveKey::Hostiles => 3,
        SaveKey::Passives => 4,
        SaveKey::Metadata => 5,
    }
}

fn select_and_validate(snapshots: &[OwnedSnapshot]) -> Result<Vec<OwnedSnapshot>, ServerError> {
    for snapshot in snapshots {
        validate_snapshot(snapshot)?;
    }
    let mut selected = snapshots.to_vec();
    selected.sort_by(|a, b| save_order(&a.key, &b.key).then_with(|| b.revision.cmp(&a.revision)));
    let mut result: Vec<OwnedSnapshot> = Vec::new();
    for snapshot in selected {
        if let Some(last) = result.last()
            && last.key == snapshot.key
        {
            if last.revision == snapshot.revision && last.value != snapshot.value {
                return Err(ServerError::InvalidInput {
                    field: "save_value",
                });
            }
            continue;
        }
        result.push(snapshot);
    }
    Ok(result)
}

fn validate_snapshot(snapshot: &OwnedSnapshot) -> Result<(), ServerError> {
    let revision = match (&snapshot.key, &snapshot.value) {
        (SaveKey::Chunk(key), SaveValue::Chunk(save)) => {
            if storage_key(*key)? != save.key {
                return Err(ServerError::InvalidInput {
                    field: "save_value",
                });
            }
            storage::encode_chunk(save).map_err(|error| storage_error("chunk", error))?;
            Some(save.revision)
        }
        (SaveKey::Player(id), SaveValue::Player(save))
            if id.bytes() == save.player_id.to_bytes() =>
        {
            storage::encode_player(save).map_err(|error| storage_error("player", error))?;
            Some(save.revision)
        }
        (SaveKey::Companions, SaveValue::Companions(save)) => {
            storage::encode_companions(save).map_err(|error| storage_error("companions", error))?;
            Some(save.revision)
        }
        (SaveKey::Hostiles, SaveValue::Hostiles(save)) => {
            storage::encode_hostile_mobs(save).map_err(|error| storage_error("hostiles", error))?;
            Some(save.revision)
        }
        (SaveKey::Passives, SaveValue::Passives(save)) => {
            storage::encode_passive_mobs(save).map_err(|error| storage_error("passives", error))?;
            Some(save.revision)
        }
        (SaveKey::Metadata, SaveValue::Metadata(save)) => {
            if snapshot.revision == 0 {
                return Err(ServerError::InvalidInput {
                    field: "metadata_sequence",
                });
            }
            storage::encode_world_metadata(save)
                .map_err(|error| storage_error("metadata", error))?;
            None
        }
        _ => {
            return Err(ServerError::InvalidInput {
                field: "save_value",
            });
        }
    };
    if let Some(revision) = revision
        && revision != snapshot.revision
    {
        return Err(ServerError::InvalidInput { field: "revision" });
    }
    Ok(())
}
