//! Named, source-compatible world backup under the exclusive disk lease.

use crate::core::contracts::{IoFaultPoint, Operation, ServerError};
use crate::store::disk::DiskStore;
use crate::store::io::{DiskIo, IoCancellation, IoPhase, io_error, write_all};
use serde_json::{Value, json};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

const IDENTITY: &str = ".mcgo-world-backup-v1.json";
const IDENTITY_LIMIT: u64 = 4096;
static TEMP_ID: AtomicU64 = AtomicU64::new(0);

pub(crate) fn backup(
    store: &mut DiskStore,
    destination: &Path,
    cancel: &IoCancellation,
) -> Result<(), ServerError> {
    cancel.check()?;
    store.sync_for_backup()?;
    let (root, metadata, factory) = store.backup_context();
    let source = absolute_lexical(root)?;
    let target = absolute_lexical(destination)?;
    let canonical_source =
        fs::canonicalize(&source).map_err(|error| io_error(Operation::Load, error))?;
    let parent = target.parent().ok_or(ServerError::InvalidInput {
        field: "backup_destination",
    })?;
    let canonical_parent =
        fs::canonicalize(parent).map_err(|error| io_error(Operation::Load, error))?;
    let canonical_target =
        canonical_parent.join(target.file_name().ok_or(ServerError::InvalidInput {
            field: "backup_destination",
        })?);
    if canonical_target.starts_with(&canonical_source) {
        return Err(ServerError::InvalidInput {
            field: "backup_destination",
        });
    }
    let want =
        json!({"source": source.to_string_lossy(), "seed": metadata.seed, "migration_version": 1});
    let mut io = factory();
    if target_matches(&target, &want, io.as_mut())? {
        return sync_directory(&canonical_parent, io.as_mut());
    }
    let name = target
        .file_name()
        .ok_or(ServerError::InvalidInput {
            field: "backup_destination",
        })?
        .to_string_lossy();
    let temporary = loop {
        let candidate = canonical_parent.join(format!(
            ".{name}.tmp-{}-{}",
            std::process::id(),
            TEMP_ID.fetch_add(1, Ordering::Relaxed)
        ));
        match fs::create_dir(&candidate) {
            Ok(()) => break candidate,
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(io_error(Operation::Replace, error)),
        }
    };
    let mut published = false;
    let outcome = (|| {
        let mut directories = Vec::new();
        copy_tree(
            &source,
            &temporary,
            &source,
            cancel,
            io.as_mut(),
            &mut directories,
        )?;
        let mut identity_bytes =
            serde_json::to_vec(&want).map_err(|_| ServerError::InvalidInput {
                field: "backup_identity",
            })?;
        identity_bytes.push(b'\n');
        write_file(&temporary.join(IDENTITY), &identity_bytes, io.as_mut())?;
        let root_permissions = fs::metadata(&source)
            .map_err(|error| io_error(Operation::Load, error))?
            .permissions();
        fs::set_permissions(&temporary, root_permissions)
            .map_err(|error| io_error(Operation::Replace, error))?;
        cancel.check()?;
        // Child directories reach stable storage before the temporary root.
        for directory in directories.into_iter().rev() {
            sync_directory(&directory, io.as_mut())?;
        }
        cancel.check()?;
        match fs::symlink_metadata(&target) {
            Ok(_) => {
                return Err(ServerError::InvalidInput {
                    field: "backup_destination",
                });
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(io_error(Operation::Load, error)),
        }
        io.boundary(IoFaultPoint::Rename, IoPhase::Before)
            .map_err(|error| io_error(Operation::Replace, error))?;
        fs::rename(&temporary, &target).map_err(|error| io_error(Operation::Replace, error))?;
        published = true;
        // Publication has happened; report hook and parent failures, but do not
        // let cancellation turn a complete named backup into an unknown result.
        let after = io
            .boundary(IoFaultPoint::Rename, IoPhase::After)
            .map_err(|error| io_error(Operation::Replace, error));
        let barrier = sync_directory(&canonical_parent, io.as_mut());
        after.and(barrier)
    })();
    if !published {
        cleanup_temporary(&temporary);
    }
    outcome
}

fn cleanup_temporary(path: &Path) {
    // Copied directory modes can be read-only. Restore traversal permission
    // only inside this attempt's private sibling before removing it.
    fn permit_removal(path: &Path) -> io::Result<()> {
        let info = fs::symlink_metadata(path)?;
        if !info.is_dir() || info.file_type().is_symlink() {
            return Ok(());
        }
        let mut permissions = info.permissions();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            permissions.set_mode(0o700);
        }
        #[cfg(not(unix))]
        permissions.set_readonly(false);
        fs::set_permissions(path, permissions)?;
        for entry in fs::read_dir(path)? {
            permit_removal(&entry?.path())?;
        }
        Ok(())
    }
    let _ = permit_removal(path).and_then(|()| fs::remove_dir_all(path));
}

fn absolute_lexical(path: &Path) -> Result<PathBuf, ServerError> {
    let absolute = if path.is_absolute() {
        path.to_owned()
    } else {
        std::env::current_dir()
            .map_err(|error| io_error(Operation::Load, error))?
            .join(path)
    };
    let mut result = PathBuf::new();
    for component in absolute.components() {
        match component {
            Component::Prefix(_) | Component::RootDir | Component::Normal(_) => {
                result.push(component.as_os_str())
            }
            Component::CurDir => {}
            Component::ParentDir => {
                if result.file_name().is_some() {
                    result.pop();
                }
            }
        }
    }
    Ok(result)
}

fn target_matches(target: &Path, want: &Value, io: &mut dyn DiskIo) -> Result<bool, ServerError> {
    let info = match fs::symlink_metadata(target) {
        Ok(info) => info,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(io_error(Operation::Load, error)),
    };
    if !info.is_dir() || info.file_type().is_symlink() {
        return Err(ServerError::InvalidInput {
            field: "backup_destination",
        });
    }
    let path = target.join(IDENTITY);
    let info = fs::symlink_metadata(&path).map_err(|error| io_error(Operation::Load, error))?;
    if !info.is_file() || info.file_type().is_symlink() || info.len() > IDENTITY_LIMIT {
        return Err(ServerError::InvalidInput {
            field: "backup_identity",
        });
    }
    let file = File::open(&path).map_err(|error| io_error(Operation::Load, error))?;
    let mut bytes = Vec::new();
    let read = (&file)
        .take(IDENTITY_LIMIT + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| io_error(Operation::Load, error));
    let close = io
        .close(file)
        .map_err(|error| io_error(Operation::Close, error));
    read.and(close)?;
    if bytes.len() as u64 > IDENTITY_LIMIT {
        return Err(ServerError::InvalidInput {
            field: "backup_identity",
        });
    }
    let got: Value = serde_json::from_slice(&bytes).map_err(|_| ServerError::InvalidInput {
        field: "backup_identity",
    })?;
    if got.get("source") != want.get("source")
        || got.get("seed") != want.get("seed")
        || got.get("migration_version") != want.get("migration_version")
    {
        return Err(ServerError::InvalidInput {
            field: "backup_identity",
        });
    }
    Ok(true)
}

fn temporary_name(name: &str) -> bool {
    name.strip_prefix('.').is_some_and(|tail| {
        [".tmp-", ".compact-", ".create-"]
            .iter()
            .any(|pattern| tail.contains(pattern))
    })
}

fn copy_tree(
    source: &Path,
    target: &Path,
    root: &Path,
    cancel: &IoCancellation,
    io: &mut dyn DiskIo,
    directories: &mut Vec<PathBuf>,
) -> Result<(), ServerError> {
    cancel.check()?;
    let source_info =
        fs::symlink_metadata(source).map_err(|error| io_error(Operation::Load, error))?;
    if !source_info.is_dir() || source_info.file_type().is_symlink() {
        return Err(ServerError::InvalidInput {
            field: "backup_source",
        });
    }
    directories.push(target.to_owned());
    let mut entries = fs::read_dir(source)
        .map_err(|error| io_error(Operation::Load, error))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| io_error(Operation::Load, error))?;
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        cancel.check()?;
        let name = entry.file_name();
        let name_text = name.to_string_lossy();
        let source_entry = entry.path();
        let info = fs::symlink_metadata(&source_entry)
            .map_err(|error| io_error(Operation::Load, error))?;
        // Go rejects symlink entries before excluding lock and temp names.
        if info.file_type().is_symlink() {
            return Err(ServerError::InvalidInput {
                field: "backup_source",
            });
        }
        if (source == root && (name_text == "world.lock" || name_text == IDENTITY))
            || temporary_name(&name_text)
        {
            continue;
        }
        let target_entry = target.join(name);
        if info.is_dir() {
            fs::create_dir(&target_entry).map_err(|error| io_error(Operation::Replace, error))?;
            copy_tree(&source_entry, &target_entry, root, cancel, io, directories)?;
            fs::set_permissions(&target_entry, info.permissions())
                .map_err(|error| io_error(Operation::Replace, error))?;
        } else if info.is_file() {
            copy_file(&source_entry, &target_entry, info.permissions(), cancel, io)?;
        } else {
            return Err(ServerError::InvalidInput {
                field: "backup_source",
            });
        }
    }
    if source != root {
        fs::set_permissions(target, source_info.permissions())
            .map_err(|error| io_error(Operation::Replace, error))?;
    }
    Ok(())
}

fn copy_file(
    source: &Path,
    target: &Path,
    permissions: fs::Permissions,
    cancel: &IoCancellation,
    io: &mut dyn DiskIo,
) -> Result<(), ServerError> {
    let mut input = File::open(source).map_err(|error| io_error(Operation::Load, error))?;
    let mut output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(target)
        .map_err(|error| io_error(Operation::Replace, error))?;
    let mut buffer = [0u8; 64 * 1024];
    let copy = (|| {
        loop {
            cancel.check()?;
            let size = input
                .read(&mut buffer)
                .map_err(|error| io_error(Operation::Load, error))?;
            if size == 0 {
                break;
            }
            io.boundary(IoFaultPoint::TempWrite, IoPhase::Before)
                .map_err(|error| io_error(Operation::WritePayload, error))?;
            write_all(io, &mut output, &buffer[..size])
                .map_err(|error| io_error(Operation::WritePayload, error))?;
            io.boundary(IoFaultPoint::TempWrite, IoPhase::After)
                .map_err(|error| io_error(Operation::WritePayload, error))?;
        }
        fs::set_permissions(target, permissions)
            .map_err(|error| io_error(Operation::Replace, error))?;
        io.boundary(IoFaultPoint::TempSync, IoPhase::Before)
            .map_err(|error| io_error(Operation::SyncPayload, error))?;
        output
            .sync_all()
            .map_err(|error| io_error(Operation::SyncPayload, error))?;
        io.boundary(IoFaultPoint::TempSync, IoPhase::After)
            .map_err(|error| io_error(Operation::SyncPayload, error))?;
        Ok(())
    })();
    let close_output = io
        .close(output)
        .map_err(|error| io_error(Operation::Close, error));
    let close_input = io
        .close(input)
        .map_err(|error| io_error(Operation::Close, error));
    copy.and(close_output).and(close_input)
}

fn write_file(path: &Path, bytes: &[u8], io: &mut dyn DiskIo) -> Result<(), ServerError> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options
        .open(path)
        .map_err(|error| io_error(Operation::Replace, error))?;
    let write = (|| {
        io.boundary(IoFaultPoint::TempWrite, IoPhase::Before)
            .and_then(|()| write_all(io, &mut file, bytes))
            .and_then(|()| io.boundary(IoFaultPoint::TempWrite, IoPhase::After))
            .map_err(|error| io_error(Operation::WritePayload, error))?;
        io.boundary(IoFaultPoint::TempSync, IoPhase::Before)
            .and_then(|()| file.sync_all())
            .and_then(|()| io.boundary(IoFaultPoint::TempSync, IoPhase::After))
            .map_err(|error| io_error(Operation::SyncPayload, error))
    })();
    let close = io
        .close(file)
        .map_err(|error| io_error(Operation::Close, error));
    write.and(close)
}

fn sync_directory(path: &Path, io: &mut dyn DiskIo) -> Result<(), ServerError> {
    let directory = File::open(path).map_err(|error| io_error(Operation::DirectorySync, error))?;
    let sync = io
        .boundary(IoFaultPoint::DirectorySync, IoPhase::Before)
        .and_then(|()| directory.sync_all())
        .and_then(|()| io.boundary(IoFaultPoint::DirectorySync, IoPhase::After))
        .map_err(|error| io_error(Operation::DirectorySync, error));
    let close = io
        .close(directory)
        .map_err(|error| io_error(Operation::DirectorySync, error));
    sync.and(close)
}
