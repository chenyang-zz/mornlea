//! One region's serialized file ownership and durable bank publication.

use crate::core::contracts::{
    ChunkKey as ServerChunkKey, DiskWriteOutcome, IoFaultPoint, Operation, RecoveredChunk, SaveKey,
    ServerError, StorageFailure,
};
use crate::store::io::{
    DiskIo, IoCancellation, IoPhase, NativeDiskIo, io_error, storage_error, write_all,
};
use mornlea_storage::{
    BANK_A_START_SECTOR, BANK_B_START_SECTOR, BANK_SIZE, ChunkKey, ChunkSave, DATA_START_SECTOR,
    RegionBank, RegionEntry, RegionKey, SECTOR_SIZE, crc32c, decode_chunk, decode_region_bank,
    decode_superblock, encode_chunk, encode_region_bank, encode_superblock, region_for,
    select_region_bank,
};
use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);
const HEADER_LEN: usize = DATA_START_SECTOR as usize * SECTOR_SIZE as usize;

/// The sole mutable owner of a region descriptor and its selected bank.
/// A possible bank publication invalidates the cached allocation view until
/// both copies have been reread from the canonical file.
pub struct RegionIo {
    path: PathBuf,
    key: RegionKey,
    io: Box<dyn DiskIo>,
    file: Option<File>,
    bank: Option<RegionBank>,
    active: usize,
    copies: [Option<RegionBank>; 2],
    uncertain: bool,
    closed: bool,
}

impl RegionIo {
    pub fn open(path: &Path, key: RegionKey) -> Result<Self, ServerError> {
        Self::with_io(path, key, Box::new(NativeDiskIo))
    }

    pub fn with_io(path: &Path, key: RegionKey, io: Box<dyn DiskIo>) -> Result<Self, ServerError> {
        let mut owner = Self {
            path: path.to_owned(),
            key,
            io,
            file: None,
            bank: None,
            active: 0,
            copies: [None, None],
            uncertain: false,
            closed: false,
        };
        match OpenOptions::new().read(true).write(true).open(path) {
            Ok(file) => owner.file = Some(file),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                owner.create()?;
                owner.file = Some(
                    OpenOptions::new()
                        .read(true)
                        .write(true)
                        .open(path)
                        .map_err(|e| io_error(Operation::Load, e))?,
                );
            }
            Err(error) => return Err(io_error(Operation::Load, error)),
        }
        owner.refresh()?;
        Ok(owner)
    }

    pub fn load(&mut self, key: ChunkKey) -> Result<RecoveredChunk, ServerError> {
        self.ensure_view()?;
        let (owner, slot) = region_for(key);
        if owner != self.key {
            return Err(corrupt());
        }
        let active = self.bank.as_ref().ok_or(closed())?.entries[slot];
        if active.offset_sector == 0 {
            return Err(io_error(Operation::Load, io::ErrorKind::NotFound.into()));
        }
        match self.load_entry(key, active) {
            Ok(decoded) => Ok(RecoveredChunk {
                chunk: decoded.chunk,
                revision: decoded.revision,
                persisted_revision: active.revision,
                needs_rewrite: decoded.migrated,
                recovered: false,
            }),
            Err(
                error @ ServerError::Storage {
                    kind: StorageFailure::FutureVersion,
                    ..
                },
            ) => Err(error),
            Err(error @ ServerError::Io { .. }) => Err(error),
            Err(_) => {
                let standby = self.copies[1 - self.active]
                    .as_ref()
                    .map(|bank| bank.entries[slot])
                    .ok_or(corrupt())?;
                if standby.offset_sector == 0
                    || standby.revision >= active.revision
                    || overlaps(active, standby)
                    || active.revision == u64::MAX
                {
                    return Err(corrupt());
                }
                let old = self.load_entry(key, standby)?;
                Ok(RecoveredChunk {
                    chunk: old.chunk,
                    revision: active.revision + 1,
                    persisted_revision: standby.revision,
                    needs_rewrite: true,
                    recovered: true,
                })
            }
        }
    }

    pub fn save(&mut self, saves: &[ChunkSave], cancel: &IoCancellation) -> DiskWriteOutcome {
        let mut outcome = DiskWriteOutcome::default();
        if let Err(error) = self.save_into(saves, cancel, &mut outcome.committed) {
            outcome.error = Some(error);
        }
        outcome
    }

    fn save_into(
        &mut self,
        saves: &[ChunkSave],
        cancel: &IoCancellation,
        committed: &mut Vec<(SaveKey, u64)>,
    ) -> Result<(), ServerError> {
        cancel.check()?;
        self.ensure_view()?;
        // Validation and duplicate arbitration complete before the first payload byte.
        let mut pending: BTreeMap<usize, (ChunkSave, Vec<u8>, bool)> = BTreeMap::new();
        for save in saves {
            let (owner, slot) = region_for(save.key);
            if owner != self.key {
                return Err(corrupt());
            }
            save_key(save.key)?;
            let payload = encode_chunk(save).map_err(|e| storage_error("chunk", e))?;
            match pending.get_mut(&slot) {
                Some((prior, old_payload, conflicting)) if prior.revision == save.revision => {
                    if old_payload != &payload {
                        *conflicting = true;
                    }
                }
                Some((prior, _, _)) if prior.revision > save.revision => {}
                _ => {
                    pending.insert(slot, (save.clone(), payload, false));
                }
            }
        }
        // A superseded lower revision cannot veto the selected highest value.
        if pending.values().any(|(_, _, conflicting)| *conflicting) {
            return Err(corrupt());
        }
        let mut writes = Vec::new();
        let mut known = Vec::new();
        for (slot, (save, payload, _)) in pending {
            let entry = self.bank.as_ref().ok_or(closed())?.entries[slot];
            if entry.offset_sector != 0 && save.revision <= entry.revision {
                if save.revision == entry.revision {
                    let loaded = self.load(save.key)?;
                    if loaded.recovered || loaded.chunk != save.chunk {
                        return Err(corrupt());
                    }
                }
                known.push((save_key(save.key)?, entry.revision));
            } else {
                writes.push((slot, save, payload));
            }
        }
        if self.uncertain && !known.is_empty() {
            self.at(IoFaultPoint::BankSync, Operation::SyncBank, |owner| {
                owner
                    .file
                    .as_mut()
                    .ok_or(io::ErrorKind::NotConnected)?
                    .sync_all()
            })?;
            self.uncertain = false;
        }
        committed.extend(known);
        if writes.is_empty() {
            return Ok(());
        }
        let mut next = self.bank.as_ref().ok_or(closed())?.clone();
        next.generation = next.generation.checked_add(1).ok_or(corrupt())?;
        let file_size = self
            .file()?
            .metadata()
            .map_err(|e| io_error(Operation::Load, e))?
            .len();
        let mut free = free_extents(&next, file_size)?;
        let mut append = file_size.div_ceil(SECTOR_SIZE as u64);
        let mut allocated = Vec::new();
        for (slot, save, payload) in writes {
            let acknowledgment = save_key(save.key)?;
            let sectors = (payload.len() as u64).div_ceil(SECTOR_SIZE as u64);
            let start = allocate(&mut free, &mut append, sectors)?;
            next.entries[slot] = RegionEntry {
                offset_sector: start as u32,
                sector_count: sectors as u32,
                payload_length: payload.len() as u32,
                revision: save.revision,
                payload_crc32c: crc32c(&payload),
            };
            let mut padded = vec![0; sectors as usize * SECTOR_SIZE as usize];
            padded[..payload.len()].copy_from_slice(&payload);
            allocated.push((acknowledgment, save.revision, start, padded));
        }
        let encoded =
            encode_region_bank(self.key, &next).map_err(|e| storage_error("region", e))?;
        for (_, _, sector, bytes) in &allocated {
            cancel.check()?;
            self.at(
                IoFaultPoint::PayloadWrite,
                Operation::WritePayload,
                |owner| owner.write_at(*sector * SECTOR_SIZE as u64, bytes),
            )?;
        }
        self.at(IoFaultPoint::PayloadSync, Operation::SyncPayload, |owner| {
            owner
                .file
                .as_mut()
                .ok_or(io::ErrorKind::NotConnected)?
                .sync_all()
        })?;
        cancel.check()?;
        // From this point cancellation cannot unwind a potentially published bank.
        self.bank = None;
        self.uncertain = true;
        let inactive = 1 - self.active;
        self.at(IoFaultPoint::BankWrite, Operation::WriteBank, |owner| {
            owner.write_at(bank_offset(inactive), &encoded)
        })?;
        self.at(IoFaultPoint::BankSync, Operation::SyncBank, |owner| {
            owner
                .file
                .as_mut()
                .ok_or(io::ErrorKind::NotConnected)?
                .sync_all()
        })?;
        self.active = inactive;
        self.copies[inactive] = Some(next.clone());
        self.bank = Some(next);
        self.uncertain = false;
        committed.extend(
            allocated
                .into_iter()
                .map(|(key, revision, _, _)| (key, revision)),
        );
        Ok(())
    }

    pub fn compact(&mut self, cancel: &IoCancellation) -> Result<(), ServerError> {
        cancel.check()?;
        self.ensure_view()?;
        let (temp_path, temporary) =
            new_temp(&self.path).map_err(|e| io_error(Operation::Replace, e))?;
        let result = self.compact_to(temporary, &temp_path, cancel);
        if result.is_err() {
            let _ = fs::remove_file(&temp_path);
        }
        result
    }

    fn compact_to(
        &mut self,
        mut temporary: File,
        temp_path: &Path,
        cancel: &IoCancellation,
    ) -> Result<(), ServerError> {
        let mut next = self.bank.as_ref().ok_or(closed())?.clone();
        let mut sector = DATA_START_SECTOR as u64;
        self.io
            .boundary(IoFaultPoint::TempWrite, IoPhase::Before)
            .map_err(|e| io_error(Operation::Replace, e))?;
        // Copy one bounded payload at a time; a full region must not be
        // retained in worker memory during compaction.
        for (slot, entry) in next.entries.iter_mut().enumerate() {
            if entry.offset_sector == 0 {
                continue;
            }
            cancel.check()?;
            let payload = self.read_payload(*entry)?;
            if crc32c(&payload) != entry.payload_crc32c {
                return Err(corrupt());
            }
            let key = key_for_slot(self.key, slot)?;
            decode_chunk(key, entry.revision, &payload).map_err(|e| storage_error("chunk", e))?;
            let sectors = (payload.len() as u64).div_ceil(SECTOR_SIZE as u64);
            if sector + sectors > u32::MAX as u64 {
                return Err(corrupt());
            }
            entry.offset_sector = sector as u32;
            entry.sector_count = sectors as u32;
            let mut padded = vec![0; sectors as usize * SECTOR_SIZE as usize];
            padded[..payload.len()].copy_from_slice(&payload);
            temporary
                .seek(SeekFrom::Start(sector * SECTOR_SIZE as u64))
                .map_err(|e| io_error(Operation::Replace, e))?;
            write_all(self.io.as_mut(), &mut temporary, &padded)
                .map_err(|e| io_error(Operation::Replace, e))?;
            sector += sectors;
        }
        let header = header(self.key, &next)?;
        temporary
            .seek(SeekFrom::Start(0))
            .map_err(|e| io_error(Operation::Replace, e))?;
        write_all(self.io.as_mut(), &mut temporary, &header)
            .map_err(|e| io_error(Operation::Replace, e))?;
        self.io
            .boundary(IoFaultPoint::TempWrite, IoPhase::After)
            .map_err(|e| io_error(Operation::Replace, e))?;
        self.sync_temp(&mut temporary)?;
        // The temporary descriptor is consumed once; no replacement follows a close failure.
        self.io
            .close(temporary)
            .map_err(|e| io_error(Operation::Close, e))?;
        cancel.check()?;
        self.file()?
            .sync_all()
            .map_err(|e| io_error(Operation::Sync, e))?;
        // The canonical descriptor is about to be consumed. A failed reopen
        // must never pair the replacement file with the old allocation view.
        self.bank = None;
        self.copies = [None, None];
        let canonical = self.file.take().ok_or(closed())?;
        if let Err(error) = self.io.close(canonical) {
            let _ = self.reopen();
            return Err(io_error(Operation::Close, error));
        }
        let canonical_path = self.path.clone();
        let renamed = self.at(IoFaultPoint::Rename, Operation::Replace, |_owner| {
            fs::rename(temp_path, &canonical_path)
        });
        if let Err(error) = renamed {
            let _ = self.reopen();
            return Err(error);
        }
        let synced = self.sync_parent();
        let reopened = self.reopen();
        synced?;
        reopened?;
        if self.active != 0 || self.bank.as_ref() != Some(&next) {
            return Err(corrupt());
        }
        Ok(())
    }

    pub fn sync(&mut self) -> Result<(), ServerError> {
        self.ensure_view()?;
        self.file()?
            .sync_all()
            .map_err(|e| io_error(Operation::Sync, e))
    }

    pub fn close(&mut self) -> Result<(), ServerError> {
        self.closed = true;
        if let Some(file) = self.file.take() {
            self.bank = None;
            self.io
                .close(file)
                .map_err(|e| io_error(Operation::Close, e))?;
        }
        Ok(())
    }

    fn create(&mut self) -> Result<(), ServerError> {
        let (temp_path, mut temp) =
            new_temp(&self.path).map_err(|e| io_error(Operation::Replace, e))?;
        let result = (|| {
            let mut first = RegionBank::empty();
            first.generation = 1;
            let bytes = header(self.key, &first)?;
            self.io
                .boundary(IoFaultPoint::TempWrite, IoPhase::Before)
                .map_err(|e| io_error(Operation::Replace, e))?;
            write_all(self.io.as_mut(), &mut temp, &bytes)
                .map_err(|e| io_error(Operation::Replace, e))?;
            self.io
                .boundary(IoFaultPoint::TempWrite, IoPhase::After)
                .map_err(|e| io_error(Operation::Replace, e))?;
            self.sync_temp(&mut temp)?;
            self.io
                .close(temp)
                .map_err(|e| io_error(Operation::Close, e))?;
            let canonical_path = self.path.clone();
            self.at(IoFaultPoint::Rename, Operation::Replace, |_owner| {
                fs::rename(&temp_path, &canonical_path)
            })?;
            self.sync_parent()
        })();
        if result.is_err() {
            let _ = fs::remove_file(&temp_path);
        }
        result
    }

    fn reopen(&mut self) -> Result<(), ServerError> {
        self.bank = None;
        self.copies = [None, None];
        self.file = Some(
            OpenOptions::new()
                .read(true)
                .write(true)
                .open(&self.path)
                .map_err(|e| io_error(Operation::Load, e))?,
        );
        self.closed = false;
        self.refresh()
    }

    fn ensure_view(&mut self) -> Result<(), ServerError> {
        if self.closed {
            return Err(closed());
        }
        if self.file.is_none() {
            self.reopen()?;
        }
        if self.bank.is_none() {
            self.refresh()?;
        }
        Ok(())
    }

    fn refresh(&mut self) -> Result<(), ServerError> {
        self.bank = None;
        self.copies = [None, None];
        let file_size = self
            .file()?
            .metadata()
            .map_err(|e| io_error(Operation::Load, e))?
            .len();
        if file_size < HEADER_LEN as u64 || file_size > i64::MAX as u64 {
            return Err(corrupt());
        }
        let mut header = vec![0; HEADER_LEN];
        self.read_at(0, &mut header)?;
        decode_superblock(self.key, &header[..SECTOR_SIZE as usize])
            .map_err(|e| storage_error("region", e))?;
        let a = decode_region_bank(
            self.key,
            &header[bank_offset(0) as usize..bank_offset(0) as usize + BANK_SIZE],
            file_size as i64,
        );
        let b = decode_region_bank(
            self.key,
            &header[bank_offset(1) as usize..bank_offset(1) as usize + BANK_SIZE],
            file_size as i64,
        );
        let copies = [a.clone().ok(), b.clone().ok()];
        let (bank, active) = select_region_bank(a, b).map_err(|e| storage_error("region", e))?;
        self.bank = Some(bank);
        self.active = active;
        self.copies = copies;
        Ok(())
    }

    fn load_entry(
        &mut self,
        key: ChunkKey,
        entry: RegionEntry,
    ) -> Result<mornlea_storage::DecodedChunk, ServerError> {
        let payload = self.read_payload(entry).map_err(|error| match error {
            ServerError::Io {
                kind: io::ErrorKind::UnexpectedEof,
                ..
            } => corrupt(),
            other => other,
        })?;
        if crc32c(&payload) != entry.payload_crc32c {
            return Err(corrupt());
        }
        decode_chunk(key, entry.revision, &payload).map_err(|e| storage_error("chunk", e))
    }

    fn read_payload(&mut self, entry: RegionEntry) -> Result<Vec<u8>, ServerError> {
        let mut payload = vec![0; entry.payload_length as usize];
        self.read_at(
            entry.offset_sector as u64 * SECTOR_SIZE as u64,
            &mut payload,
        )?;
        Ok(payload)
    }

    fn read_at(&mut self, offset: u64, bytes: &mut [u8]) -> Result<(), ServerError> {
        let file = self.file()?;
        file.seek(SeekFrom::Start(offset))
            .map_err(|e| io_error(Operation::Load, e))?;
        file.read_exact(bytes)
            .map_err(|e| io_error(Operation::Load, e))
    }

    fn write_at(&mut self, offset: u64, bytes: &[u8]) -> io::Result<()> {
        let file = self.file.as_mut().ok_or(io::ErrorKind::NotConnected)?;
        file.seek(SeekFrom::Start(offset))?;
        write_all(self.io.as_mut(), file, bytes)
    }

    fn file(&mut self) -> Result<&mut File, ServerError> {
        self.file.as_mut().ok_or(closed())
    }

    fn at<T>(
        &mut self,
        point: IoFaultPoint,
        operation: Operation,
        action: impl FnOnce(&mut Self) -> io::Result<T>,
    ) -> Result<T, ServerError> {
        self.io
            .boundary(point, IoPhase::Before)
            .map_err(|e| io_error(operation, e))?;
        let value = action(self).map_err(|e| io_error(operation, e))?;
        self.io
            .boundary(point, IoPhase::After)
            .map_err(|e| io_error(operation, e))?;
        Ok(value)
    }

    fn sync_temp(&mut self, temp: &mut File) -> Result<(), ServerError> {
        self.io
            .boundary(IoFaultPoint::TempSync, IoPhase::Before)
            .map_err(|e| io_error(Operation::Sync, e))?;
        temp.sync_all().map_err(|e| io_error(Operation::Sync, e))?;
        self.io
            .boundary(IoFaultPoint::TempSync, IoPhase::After)
            .map_err(|e| io_error(Operation::Sync, e))
    }

    fn sync_parent(&mut self) -> Result<(), ServerError> {
        let parent = self.path.parent().unwrap_or(Path::new(".")).to_owned();
        self.at(
            IoFaultPoint::DirectorySync,
            Operation::DirectorySync,
            |owner| {
                let directory = File::open(parent)?;
                let synced = directory.sync_all();
                // Close is part of the durability barrier even if sync failed.
                let closed = owner.io.close(directory);
                synced.and(closed)
            },
        )
    }
}

fn corrupt() -> ServerError {
    ServerError::Storage {
        family: "region",
        kind: StorageFailure::Corrupt,
    }
}
fn closed() -> ServerError {
    io_error(Operation::Load, io::ErrorKind::NotConnected.into())
}
fn bank_offset(index: usize) -> u64 {
    u64::from(if index == 0 {
        BANK_A_START_SECTOR
    } else {
        BANK_B_START_SECTOR
    }) * u64::from(SECTOR_SIZE)
}
fn header(key: RegionKey, first: &RegionBank) -> Result<Vec<u8>, ServerError> {
    let a = encode_region_bank(key, first).map_err(|e| storage_error("region", e))?;
    let b =
        encode_region_bank(key, &RegionBank::empty()).map_err(|e| storage_error("region", e))?;
    let mut header = vec![0; HEADER_LEN];
    header[..SECTOR_SIZE as usize].copy_from_slice(&encode_superblock(key));
    header[bank_offset(0) as usize..bank_offset(0) as usize + BANK_SIZE].copy_from_slice(&a);
    header[bank_offset(1) as usize..bank_offset(1) as usize + BANK_SIZE].copy_from_slice(&b);
    Ok(header)
}
fn overlaps(a: RegionEntry, b: RegionEntry) -> bool {
    let a0 = a.offset_sector as u64;
    let b0 = b.offset_sector as u64;
    a0 < b0 + b.sector_count as u64 && b0 < a0 + a.sector_count as u64
}
fn free_extents(bank: &RegionBank, file_size: u64) -> Result<Vec<(u64, u64)>, ServerError> {
    let end = file_size.div_ceil(SECTOR_SIZE as u64);
    if end > u32::MAX as u64 {
        return Err(corrupt());
    }
    let mut used: Vec<_> = bank
        .entries
        .iter()
        .filter(|e| e.offset_sector != 0)
        .map(|e| {
            (
                e.offset_sector as u64,
                e.offset_sector as u64 + e.sector_count as u64,
            )
        })
        .collect();
    used.sort_unstable();
    let mut free = Vec::new();
    let mut cursor = DATA_START_SECTOR as u64;
    for (start, stop) in used {
        if cursor < start {
            free.push((cursor, start - cursor));
        }
        cursor = cursor.max(stop);
    }
    if cursor < end {
        free.push((cursor, end - cursor));
    }
    Ok(free)
}
fn allocate(
    free: &mut Vec<(u64, u64)>,
    append: &mut u64,
    sectors: u64,
) -> Result<u64, ServerError> {
    if sectors == 0 {
        return Err(corrupt());
    }
    if let Some(index) = free.iter().position(|(_, length)| *length >= sectors) {
        let start = free[index].0;
        free[index].0 += sectors;
        free[index].1 -= sectors;
        if free[index].1 == 0 {
            free.remove(index);
        }
        return Ok(start);
    }
    let start = *append;
    *append = append.checked_add(sectors).ok_or(corrupt())?;
    if *append > u32::MAX as u64 {
        return Err(corrupt());
    }
    Ok(start)
}
fn new_temp(path: &Path) -> io::Result<(PathBuf, File)> {
    let parent = path.parent().unwrap_or(Path::new("."));
    let name = path
        .file_name()
        .ok_or(io::ErrorKind::InvalidInput)?
        .to_string_lossy();
    for _ in 0..32 {
        let next = TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let temp = parent.join(format!(".{name}.{}-{next}.tmp", std::process::id()));
        let mut options = OpenOptions::new();
        options.read(true).write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        match options.open(&temp) {
            Ok(file) => return Ok((temp, file)),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        }
    }
    Err(io::ErrorKind::AlreadyExists.into())
}
fn save_key(key: ChunkKey) -> Result<SaveKey, ServerError> {
    let dimension = u8::try_from(key.dimension)
        .ok()
        .and_then(|raw| mornlea_domain::Dimension::new(raw).ok())
        .ok_or(ServerError::InvalidInput { field: "dimension" })?;
    Ok(SaveKey::Chunk(ServerChunkKey {
        dimension,
        pos: mornlea_domain::ChunkPos::new(key.x, key.z),
    }))
}
fn key_for_slot(region: RegionKey, slot: usize) -> Result<ChunkKey, ServerError> {
    Ok(ChunkKey {
        dimension: region.dimension,
        x: region
            .x
            .checked_mul(32)
            .and_then(|x| x.checked_add((slot % 32) as i32))
            .ok_or(corrupt())?,
        z: region
            .z
            .checked_mul(32)
            .and_then(|z| z.checked_add((slot / 32) as i32))
            .ok_or(corrupt())?,
    })
}
