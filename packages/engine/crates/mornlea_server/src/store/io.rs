//! Shared cancellation and native file-operation boundaries for save workers.

use crate::core::contracts::{IoFaultPoint, Operation, ServerError, StorageFailure};
use std::fs::File;
use std::io::{self, Write};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

/// A cancellation request belongs to one operation, never the whole disk owner.
#[derive(Clone, Default)]
pub struct IoCancellation(Arc<AtomicBool>);
impl IoCancellation {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn cancel(&self) {
        self.0.store(true, Ordering::Release);
    }
    pub fn check(&self) -> Result<(), ServerError> {
        if self.0.load(Ordering::Acquire) {
            Err(ServerError::Cancelled)
        } else {
            Ok(())
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum IoPhase {
    Before,
    After,
}

/// Hooks surround real provider I/O. An after-hook failure does not undo bytes.
#[doc(hidden)]
pub trait DiskIo: Send {
    fn boundary(&mut self, _point: IoFaultPoint, _phase: IoPhase) -> io::Result<()> {
        Ok(())
    }
    fn write(&mut self, file: &mut File, bytes: &[u8]) -> io::Result<usize> {
        file.write(bytes)
    }
    fn close(&mut self, file: File) -> io::Result<()> {
        close_file(file)
    }
}
#[derive(Default)]
pub struct NativeDiskIo;
impl DiskIo for NativeDiskIo {}

/// A successful write must consume the complete payload, including short writes.
pub fn write_all(io: &mut dyn DiskIo, file: &mut File, mut bytes: &[u8]) -> io::Result<()> {
    while !bytes.is_empty() {
        match io.write(file, bytes) {
            Ok(0) => return Err(io::ErrorKind::WriteZero.into()),
            Ok(written) => bytes = bytes.get(written..).ok_or(io::ErrorKind::InvalidData)?,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(error),
        }
    }
    Ok(())
}
pub fn io_error(operation: Operation, error: io::Error) -> ServerError {
    ServerError::Io {
        operation,
        kind: error.kind(),
    }
}
pub fn storage_error(family: &'static str, error: mornlea_storage::StorageError) -> ServerError {
    let kind = match error {
        mornlea_storage::StorageError::Corrupt(_) => StorageFailure::Corrupt,
        mornlea_storage::StorageError::FutureVersion(_) => StorageFailure::FutureVersion,
        mornlea_storage::StorageError::OutputTooSmall { .. } => StorageFailure::OutputTooSmall,
    };
    ServerError::Storage { family, kind }
}
/// Consumes the unique file owner and reports close errors hidden by File::drop.
/// Never retry the raw handle after failure: its native state may be uncertain.
#[cfg(unix)]
#[allow(unsafe_code)]
pub fn close_file(file: File) -> io::Result<()> {
    use std::os::fd::IntoRawFd;
    let fd = file.into_raw_fd();
    // SAFETY: into_raw_fd transfers this unique File's ownership. This is
    // its one native close; no File destructor can subsequently close it.
    if unsafe { libc::close(fd) } == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

#[cfg(windows)]
#[allow(unsafe_code)]
pub fn close_file(file: File) -> io::Result<()> {
    use std::os::windows::io::IntoRawHandle;
    let handle = file.into_raw_handle();
    // SAFETY: ownership moved out of File and this adapter closes it once.
    if unsafe { windows_sys::Win32::Foundation::CloseHandle(handle) } != 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

#[cfg(not(any(unix, windows)))]
pub fn close_file(file: File) -> io::Result<()> {
    drop(file);
    Err(io::ErrorKind::Unsupported.into())
}
