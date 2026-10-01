//! Private, serialized filesystem owner and its bounded immutable handoff.

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::{Arc, mpsc};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use mornlea_storage::{ChunkCodec, StorageError};

use super::mailbox::CHUNK_MAX_RESERVATION;
use crate::core::contracts::{
    Deadline, DiskBackend, Operation, SaveKey, SaveRequest, SaveTicket, SaveValue, ServerError,
    ServerPhase,
};

pub(super) struct SaveResult {
    pub ticket: SaveTicket,
    pub committed: Vec<(SaveKey, u64)>,
    pub error: Option<ServerError>,
    pub reservations: Vec<usize>,
}

/// Codec scratch travels with the backend; a background mailbox has no idle codec.
pub(super) struct SaveExecutor {
    codec: ChunkCodec,
    scratch: Vec<u8>,
}
impl SaveExecutor {
    pub fn try_new() -> Result<Self, ServerError> {
        let codec = ChunkCodec::try_new().map_err(|_| internal("store chunk codec"))?;
        Ok(Self {
            codec,
            scratch: vec![0; CHUNK_MAX_RESERVATION],
        })
    }

    pub fn save<B: DiskBackend>(
        &mut self,
        backend: &mut B,
        ticket: SaveTicket,
        request: &SaveRequest,
        mut reservations: Vec<usize>,
    ) -> SaveResult {
        let result = catch_unwind(AssertUnwindSafe(|| {
            for (index, snapshot) in request.snapshots.iter().enumerate() {
                if let SaveValue::Chunk(save) = &snapshot.value {
                    reservations[index] = self
                        .codec
                        .encode_into(save, &mut self.scratch)
                        .map_err(store_io_error)?;
                }
            }
            // Only the backend owner clones large bodies. Its echoed snapshots
            // never replace the immutable request retained by the authority.
            let completion = backend.write(ticket, request.clone());
            if completion.ticket != ticket {
                return Err(internal("store completion ticket"));
            }
            Ok((completion.committed, completion.error))
        }));
        let (committed, error) = match result {
            Ok(Ok(result)) => result,
            Ok(Err(error)) => (Vec::new(), Some(error)),
            Err(_) => (Vec::new(), Some(internal("store owner panic"))),
        };
        SaveResult {
            ticket,
            committed,
            error,
            reservations,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum LifecycleKind {
    Sync,
    Close,
}
impl LifecycleKind {
    fn operation(self) -> Operation {
        match self {
            Self::Sync => Operation::Sync,
            Self::Close => Operation::Close,
        }
    }
}

enum Command {
    Save {
        ticket: SaveTicket,
        request: Arc<SaveRequest>,
        reservations: Vec<usize>,
        reply: mpsc::SyncSender<SaveResult>,
    },
    Lifecycle {
        kind: LifecycleKind,
        deadline: Deadline,
        reply: mpsc::SyncSender<Result<(), ServerError>>,
    },
}

struct PendingLifecycle {
    kind: LifecycleKind,
    reply: mpsc::Receiver<Result<(), ServerError>>,
    disconnected: bool,
}

pub(super) struct Background {
    commands: mpsc::SyncSender<Command>,
    thread: Option<JoinHandle<()>>,
    lifecycle: Option<PendingLifecycle>,
}

pub(super) enum Handoff {
    Sent(mpsc::Receiver<SaveResult>),
    Full,
    Disconnected,
}

impl Background {
    pub fn spawn<B: DiskBackend + Send + 'static>(
        capacity: usize,
        mut backend: B,
        mut executor: SaveExecutor,
    ) -> Result<Self, ServerError> {
        let (commands, receive) = mpsc::sync_channel(capacity);
        let thread = thread::Builder::new()
            .name("mornlea-store".to_owned())
            .spawn(move || {
                while let Ok(command) = receive.recv() {
                    match command {
                        Command::Save {
                            ticket,
                            request,
                            reservations,
                            reply,
                        } => {
                            let result =
                                executor.save(&mut backend, ticket, &request, reservations);
                            // The authority can unwrap its Arc as soon as the facts arrive.
                            drop(request);
                            let _ = reply.send(result);
                        }
                        Command::Lifecycle {
                            kind,
                            deadline,
                            reply,
                        } => {
                            let result = if deadline.expired(Instant::now()) {
                                Err(ServerError::Timeout {
                                    operation: kind.operation(),
                                })
                            } else {
                                catch_unwind(AssertUnwindSafe(|| match kind {
                                    LifecycleKind::Sync => backend.sync(),
                                    LifecycleKind::Close => backend.close(),
                                }))
                                .unwrap_or_else(|_| Err(internal("store owner panic")))
                            };
                            let closed = kind == LifecycleKind::Close && result.is_ok();
                            let _ = reply.send(result);
                            if closed {
                                break;
                            }
                        }
                    }
                }
            })
            .map_err(|error| ServerError::Io {
                operation: Operation::Load,
                kind: error.kind(),
            })?;
        Ok(Self {
            commands,
            thread: Some(thread),
            lifecycle: None,
        })
    }

    pub fn try_save(
        &self,
        ticket: SaveTicket,
        request: &Arc<SaveRequest>,
        reservations: &[usize],
    ) -> Handoff {
        let (reply, receive) = mpsc::sync_channel(1);
        let command = Command::Save {
            ticket,
            request: Arc::clone(request),
            reservations: reservations.to_vec(),
            reply,
        };
        match self.commands.try_send(command) {
            Ok(()) => Handoff::Sent(receive),
            Err(mpsc::TrySendError::Full(_)) => Handoff::Full,
            Err(mpsc::TrySendError::Disconnected(_)) => Handoff::Disconnected,
        }
    }

    /// A timed-out call retains this receiver. A same-kind retry consumes its
    /// original result; a different barrier cannot overtake unresolved ownership.
    pub fn lifecycle(
        &mut self,
        kind: LifecycleKind,
        deadline: Deadline,
    ) -> Result<(), ServerError> {
        if let Some(pending) = &self.lifecycle {
            if pending.kind != kind {
                return Err(ServerError::InvalidState {
                    phase: ServerPhase::Closing,
                });
            }
        } else {
            let (reply, receive) = mpsc::sync_channel(1);
            let mut command = Command::Lifecycle {
                kind,
                deadline,
                reply,
            };
            loop {
                if deadline.expired(Instant::now()) {
                    return Err(ServerError::Timeout {
                        operation: kind.operation(),
                    });
                }
                match self.commands.try_send(command) {
                    Ok(()) => {
                        self.lifecycle = Some(PendingLifecycle {
                            kind,
                            reply: receive,
                            disconnected: false,
                        });
                        break;
                    }
                    Err(mpsc::TrySendError::Full(returned)) => {
                        command = returned;
                        wait(deadline, kind.operation())?;
                    }
                    Err(mpsc::TrySendError::Disconnected(_)) => {
                        self.lifecycle = Some(PendingLifecycle {
                            kind,
                            reply: receive,
                            disconnected: true,
                        });
                        return Err(internal("store owner disconnected"));
                    }
                }
            }
        }
        loop {
            let pending = self.lifecycle.as_mut().expect("retained lifecycle");
            if pending.disconnected {
                return Err(internal("store owner disconnected"));
            }
            match pending.reply.try_recv() {
                Ok(result) => {
                    let mut pending = self.lifecycle.take().expect("retained lifecycle");
                    if result.is_ok() && kind == LifecycleKind::Close {
                        // A successful close joins through backend destruction. An
                        // unexpected exit retains the failed close boundary forever.
                        let joined = self
                            .thread
                            .take()
                            .ok_or_else(|| internal("store owner join"))
                            .and_then(|thread| {
                                thread.join().map_err(|_| internal("store owner join"))
                            });
                        if let Err(error) = joined {
                            pending.disconnected = true;
                            self.lifecycle = Some(pending);
                            return Err(error);
                        }
                    }
                    return result;
                }
                Err(mpsc::TryRecvError::Empty) => wait(deadline, kind.operation())?,
                Err(mpsc::TryRecvError::Disconnected) => {
                    pending.disconnected = true;
                    return Err(internal("store owner disconnected"));
                }
            }
        }
    }
}

/// Off-tick polling is bounded by host time, even with a stationary caller clock.
pub(super) fn wait(deadline: Deadline, operation: Operation) -> Result<(), ServerError> {
    let remaining = deadline.instant().saturating_duration_since(Instant::now());
    if remaining.is_zero() {
        return Err(ServerError::Timeout { operation });
    }
    thread::sleep(remaining.min(Duration::from_millis(1)));
    Ok(())
}
pub(super) fn internal(invariant: &'static str) -> ServerError {
    ServerError::Internal { invariant }
}
fn store_io_error(error: StorageError) -> ServerError {
    match error {
        StorageError::OutputTooSmall { .. } => internal("store chunk scratch"),
        StorageError::Corrupt(_) | StorageError::FutureVersion(_) => ServerError::Io {
            operation: Operation::WritePayload,
            kind: std::io::ErrorKind::InvalidData,
        },
    }
}
