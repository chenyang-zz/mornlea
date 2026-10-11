//! Exclusive native world-lock ownership shared with the Go server.

use crate::core::contracts::{Operation, ServerError};
use crate::store::io::io_error;
use std::fs::{self, File, OpenOptions};
use std::io;
use std::path::Path;

/// The lock file remains on disk. A failed unlock keeps its live descriptor so
/// the caller can retry without allowing a second writer to open the world.
pub struct WorldLease {
    file: Option<File>,
}

impl WorldLease {
    pub fn acquire(root: &Path) -> Result<Self, ServerError> {
        let path = root.join("world.lock");
        match fs::symlink_metadata(&path) {
            Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_file() => {
                return Err(ServerError::InvalidInput {
                    field: "world_lock",
                });
            }
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(io_error(Operation::Load, error)),
        }
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&path)
            .map_err(|error| io_error(Operation::Load, error))?;
        if !file
            .metadata()
            .map_err(|error| io_error(Operation::Load, error))?
            .is_file()
        {
            return Err(ServerError::InvalidInput {
                field: "world_lock",
            });
        }
        file.try_lock().map_err(|error| {
            let kind = match error {
                fs::TryLockError::WouldBlock => io::ErrorKind::WouldBlock,
                fs::TryLockError::Error(error) => error.kind(),
            };
            ServerError::Io {
                operation: Operation::Load,
                kind,
            }
        })?;
        Ok(Self { file: Some(file) })
    }

    pub fn release(&mut self) -> Result<(), ServerError> {
        if let Some(file) = self.file.as_ref() {
            File::unlock(file).map_err(|error| io_error(Operation::Close, error))?;
        }
        self.file.take();
        Ok(())
    }
}
