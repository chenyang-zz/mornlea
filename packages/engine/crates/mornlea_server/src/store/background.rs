//! Private, serialized filesystem owner and its bounded immutable handoff.

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::{Arc, mpsc};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use mornlea_storage::{ChunkCodec, StorageError};

use super::loads::{self, LoadHandoff, LoadKey, LoadResult};
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
            let mut normalized = request.clone();
            for snapshot in &mut normalized.snapshots {
                if let SaveValue::ChunkView(view) = &snapshot.value {
                    snapshot.value = SaveValue::Chunk(view.materialize());
                }
            }
            for (index, snapshot) in normalized.snapshots.iter().enumerate() {
                if let SaveValue::Chunk(save) = &snapshot.value {
                    reservations[index] = self
                        .codec
                        .encode_into(save, &mut self.scratch)
                        .map_err(store_io_error)?;
                }
            }
            // Only the backend owner clones large bodies. Its echoed snapshots
            // never replace the immutable request retained by the authority.
            let completion = backend.write(ticket, normalized);
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
    Load {
        key: LoadKey,
        deadline: Deadline,
        reply: mpsc::SyncSender<LoadResult>,
    },
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
    outcome: Option<Result<(), ServerError>>,
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
                        Command::Load {
                            key,
                            deadline,
                            reply,
                        } => {
                            let result = loads::execute(&mut backend, key, deadline);
                            let _ = reply.send(result);
                        }
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

    pub fn try_load(&self, key: LoadKey, deadline: Deadline) -> LoadHandoff {
        let (reply, receive) = mpsc::sync_channel(1);
        match self.commands.try_send(Command::Load {
            key,
            deadline,
            reply,
        }) {
            Ok(()) => LoadHandoff::Sent(receive),
            Err(mpsc::TrySendError::Full(_)) => LoadHandoff::Full,
            Err(mpsc::TrySendError::Disconnected(_)) => LoadHandoff::Disconnected,
        }
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
                            outcome: None,
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
                            outcome: None,
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
            if pending.outcome.is_none() {
                match pending.reply.try_recv() {
                    // Retain the result before waiting for owner destruction.
                    Ok(result) => pending.outcome = Some(result),
                    Err(mpsc::TryRecvError::Empty) => {
                        wait(deadline, kind.operation())?;
                        continue;
                    }
                    Err(mpsc::TryRecvError::Disconnected) => {
                        pending.disconnected = true;
                        return Err(internal("store owner disconnected"));
                    }
                }
            }
            let result = pending.outcome.expect("retained lifecycle outcome");
            if result.is_ok() && kind == LifecycleKind::Close {
                let Some(thread) = self.thread.as_ref() else {
                    pending.disconnected = true;
                    return Err(internal("store owner join"));
                };
                if !thread.is_finished() {
                    // Close may have succeeded while backend destruction still
                    // runs. Timeout keeps both that success and its owner handle.
                    wait(deadline, kind.operation())?;
                    continue;
                }
                // Joining a finished owner cannot wait on backend destruction.
                // An unexpected exit retains the failed close boundary forever.
                let joined = self
                    .thread
                    .take()
                    .expect("finished store owner")
                    .join()
                    .map_err(|_| internal("store owner join"));
                if let Err(error) = joined {
                    pending.disconnected = true;
                    return Err(error);
                }
            }
            self.lifecycle = None;
            return result;
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

#[cfg(test)]
mod chunk_view_tests {
    use super::*;
    use crate::core::contracts::{
        LoadedValue, OwnedSnapshot, SaveCompletion, SavePoll, SaveUrgency, StoreHandle, StoreLimits,
    };
    use crate::core::world::{ReadyChunk, materializations, reset_materializations};
    use crate::store::mailbox::StoreMailbox;
    use mornlea_domain::{ChunkPos, Dimension};
    use mornlea_storage::{Chunk, ContainerSnapshot, StorageKind};

    struct MeasuredBackend(mpsc::SyncSender<(usize, thread::ThreadId)>);
    impl DiskBackend for MeasuredBackend {
        fn write(&mut self, ticket: SaveTicket, request: SaveRequest) -> SaveCompletion {
            assert!(matches!(request.snapshots[0].value, SaveValue::Chunk(_)));
            self.0
                .send((materializations(), thread::current().id()))
                .unwrap();
            let submitted = request
                .snapshots
                .iter()
                .map(|s| (s.key.clone(), s.revision))
                .collect::<Vec<_>>();
            SaveCompletion {
                ticket,
                snapshots: request.snapshots,
                committed: submitted.clone(),
                submitted,
                error: None,
            }
        }
        fn load(&mut self, _: SaveKey) -> Result<LoadedValue, ServerError> {
            unreachable!()
        }
        fn sync(&mut self) -> Result<(), ServerError> {
            Ok(())
        }
        fn close(&mut self) -> Result<(), ServerError> {
            Ok(())
        }
    }
    #[test]
    fn actual_materialization_runs_only_on_reusable_backend_owner() {
        let key = crate::core::contracts::ChunkKey {
            dimension: Dimension::OVERWORLD,
            pos: ChunkPos::new(0, 0),
        };
        let ready = ReadyChunk::try_new(
            key,
            7,
            5,
            Chunk {
                sections: vec![
                    ContainerSnapshot {
                        kind: StorageKind::Single,
                        bits: 0,
                        single: 2,
                        palette: vec![],
                        packed: vec![]
                    };
                    24
                ],
                drops: vec![Default::default(); 32],
                furnaces: vec![Default::default(); 32],
                chests: vec![Default::default(); 16],
            },
        )
        .unwrap();
        let snapshot = OwnedSnapshot::try_new(
            SaveKey::Chunk(key),
            5,
            CHUNK_MAX_RESERVATION,
            SaveUrgency::Autosave,
            SaveValue::ChunkView(ready.capture(None, None)),
        )
        .unwrap();
        let (send, receive) = mpsc::sync_channel(2);
        let mut store = StoreMailbox::try_new_background(
            StoreLimits::try_new(2, 16, 3, 3, 3, 1, 8, 4_194_304).unwrap(),
            MeasuredBackend(send),
        )
        .unwrap();
        reset_materializations();
        let mut owners = Vec::new();
        for expected in [1, 2] {
            let ticket = store
                .submit(crate::core::contracts::SaveRequest {
                    snapshots: vec![snapshot.clone()],
                })
                .unwrap();
            let until = Instant::now() + Duration::from_secs(5);
            let done = loop {
                store.drive_workers();
                if let SavePoll::Completed(done) = store.poll(ticket) {
                    break done;
                }
                assert!(Instant::now() < until);
                thread::yield_now();
            };
            assert!(
                done.error.is_none(),
                "owner must receive materialized chunk values"
            );
            assert_eq!(done.snapshots, vec![snapshot.clone()]);
            assert_eq!(
                materializations(),
                0,
                "clone, submit and poll cannot expand authority data"
            );
            let (count, owner) = receive.recv_timeout(Duration::from_secs(5)).unwrap();
            assert_eq!(count, expected);
            owners.push(owner);
        }
        store
            .close(Deadline::after(Instant::now(), Duration::from_secs(5)).unwrap())
            .unwrap();
        assert_eq!(owners[0], owners[1]);
        assert_ne!(owners[0], thread::current().id());
    }
}
