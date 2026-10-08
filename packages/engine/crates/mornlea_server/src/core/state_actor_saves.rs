//! Actor persistence is a private child of the opaque authority, with no sibling field access.
use super::*;
use crate::core::actor_save::ActorSaveLedger;

const ACTOR_TARGETS: usize = 19;
/// Chunk keys one actor-routed completion may name.
const COMPLETION_CHUNK_KEYS: usize = 8;
/// Player keys one completion may name: the actor ledger's retained player
/// records, online or departed, not the online player cap. The overflow keeps
/// the ledger's existing `Resource::Players` refusal.
const COMPLETION_PLAYER_KEYS: usize = crate::core::actor_save::PLAYER_KEYS;
const COMPLETION_TARGETS: usize = ACTOR_TARGETS + COMPLETION_CHUNK_KEYS + 1;
const SAVE_IDENTITY: ServerError = ServerError::Internal {
    invariant: "save completion identity",
};

impl AuthorityState {
    /// Enables retained actor ownership before sessions or ticks can produce state.
    /// Live chunks share completion routing; this creates no producer or loader.
    pub fn enable_actor_saves(&mut self) -> Result<(), ServerError> {
        self.require_live_chunks(true)?;
        if self.actor_saves.is_some() {
            return Ok(());
        }
        if self.next_tick != 0
            || !self.current_sessions.is_empty()
            || !self.dirty.is_empty()
            || !self.in_flight.is_empty()
        {
            return Err(ServerError::InvalidState { phase: self.phase });
        }
        self.actor_saves = Some(ActorSaveLedger::default());
        Ok(())
    }

    /// Admits validated source facts; only the trusted lifecycle producer chooses eligibility.
    /// Closing fences new facts while already retained targets remain flushable.
    pub fn retain_actor_save(
        &mut self,
        value: SaveValue,
        persisted_revision: u64,
        needs_rewrite: bool,
        eligible: bool,
        pinned: bool,
    ) -> Result<(), ServerError> {
        self.require_live_chunks(true)?;
        self.actor_saves
            .as_mut()
            .ok_or(ServerError::InvalidState { phase: self.phase })?
            .retain(value, persisted_revision, needs_rewrite, eligible, pinned)
    }

    /// Observes latest source content without replacing an immutable selected target.
    /// This port carries no unvalidated session or world action.
    pub fn observe_actor_save(
        &mut self,
        value: SaveValue,
        persistable: bool,
        force: bool,
    ) -> Result<bool, ServerError> {
        self.require_live_chunks(true)?;
        self.actor_saves
            .as_mut()
            .ok_or(ServerError::InvalidState { phase: self.phase })?
            .observe(value, persistable, force)
    }

    /// Borrows latest facts and their acknowledged revision, without capture or I/O.
    pub fn actor_save_current(&self, key: &SaveKey) -> Option<(u64, &SaveValue)> {
        self.actor_saves.as_ref()?.current(key)
    }

    pub(super) fn select_live(&mut self, mode: SaveMode, budget: SaveBudget) -> Vec<OwnedSnapshot> {
        if self.actor_saves.is_none() {
            return self.select_live_chunks(mode, budget);
        }
        let preferred_actor = self.actor_saves_next;
        for actor in [preferred_actor, !preferred_actor] {
            let selected = if actor {
                match self
                    .actor_saves
                    .as_mut()
                    .expect("enabled actor saves")
                    .select(
                        mode,
                        SaveBudget {
                            chunks: ACTOR_TARGETS,
                            estimated_bytes: budget.estimated_bytes,
                        },
                    ) {
                    Ok(selected) => selected,
                    Err(error) => {
                        self.fail_tick(error);
                        return Vec::new();
                    }
                }
            } else {
                self.select_live_chunks(mode, budget)
            };
            if !selected.is_empty() {
                // Separate tickets avoid a large chunk reservation starving later actor families.
                self.actor_saves_next = !actor;
                return selected;
            }
        }
        Vec::new()
    }

    pub(super) fn return_actor_dirty(&mut self, snapshot: &OwnedSnapshot) {
        let ledger = self.actor_saves.as_mut().expect("enabled actor saves");
        if let Err(error) = ledger.validate_saved(snapshot) {
            self.fail_tick(error);
            return;
        }
        ledger.return_dirty(snapshot);
    }

    pub(super) fn apply_actor_completion(&mut self, completion: SaveCompletion) -> AckReport {
        let mut errors: Vec<_> = completion.error.into_iter().collect();
        // All shape and immutable-preimage checks precede every owner's durability mutation.
        let validation = self.validate_actor_completion(&completion);
        if let Err(error) = validation {
            errors.push(error);
            return AckReport {
                acked: 0,
                released: 0,
                retry: Vec::new(),
                errors,
            };
        }
        let mut acked = 0;
        let mut released = 0;
        let mut retry = Vec::new();
        for snapshot in completion.snapshots {
            let chunk = matches!(snapshot.key, SaveKey::Chunk(_));
            let held = if chunk {
                self.acquisition.matches(&snapshot)
            } else {
                self.actor_saves
                    .as_ref()
                    .expect("enabled actor saves")
                    .matches(&snapshot)
            };
            if !held && !matches!(snapshot.key, SaveKey::Metadata) {
                continue;
            }
            if completion
                .committed
                .contains(&(snapshot.key.clone(), snapshot.revision))
            {
                if held {
                    if chunk {
                        self.acquisition
                            .saved(&snapshot)
                            .expect("validated chunk save preimage");
                    } else {
                        self.actor_saves
                            .as_mut()
                            .expect("enabled actor saves")
                            .saved(&snapshot)
                            .expect("validated actor save preimage");
                    }
                }
                acked += 1;
            } else {
                // Admitted failures keep their exact flight charged through scheduler backoff.
                released += 1;
                retry.push(snapshot);
            }
        }
        if !retry.is_empty() && errors.is_empty() {
            errors.push(ServerError::Internal {
                invariant: "incomplete save completion",
            });
        }
        AckReport {
            acked,
            released,
            retry,
            errors,
        }
    }

    fn validate_actor_completion(&self, completion: &SaveCompletion) -> Result<(), ServerError> {
        let observed = completion
            .snapshots
            .len()
            .max(completion.submitted.len())
            .max(completion.committed.len());
        if observed > COMPLETION_TARGETS {
            return Err(ServerError::Capacity {
                resource: Resource::SaveChunks,
                limit: COMPLETION_TARGETS,
                observed,
            });
        }
        for keys in [
            completion
                .snapshots
                .iter()
                .map(|s| &s.key)
                .collect::<Vec<_>>(),
            completion.submitted.iter().map(|(k, _)| k).collect(),
            completion.committed.iter().map(|(k, _)| k).collect(),
        ] {
            let chunks = keys
                .iter()
                .filter(|k| matches!(k, SaveKey::Chunk(_)))
                .count();
            let players = keys
                .iter()
                .filter(|k| matches!(k, SaveKey::Player(_)))
                .count();
            if chunks > COMPLETION_CHUNK_KEYS {
                return Err(ServerError::Capacity {
                    resource: Resource::SaveChunks,
                    limit: COMPLETION_CHUNK_KEYS,
                    observed: chunks,
                });
            }
            if players > COMPLETION_PLAYER_KEYS {
                return Err(ServerError::Capacity {
                    resource: Resource::Players,
                    limit: COMPLETION_PLAYER_KEYS,
                    observed: players,
                });
            }
            let mut unique = Vec::new();
            for key in keys {
                if unique.contains(&key) {
                    return Err(SAVE_IDENTITY);
                }
                unique.push(key);
            }
        }
        if completion.submitted.len() != completion.snapshots.len()
            || !completion
                .submitted
                .iter()
                .zip(&completion.snapshots)
                .all(|((key, revision), s)| *key == s.key && *revision == s.revision)
            || !completion
                .committed
                .iter()
                .all(|id| completion.submitted.contains(id))
        {
            return Err(SAVE_IDENTITY);
        }
        for snapshot in &completion.snapshots {
            match snapshot.key {
                SaveKey::Chunk(key) => {
                    // Acquisition validates matching targets only; reject altered held identities first.
                    if self.acquisition.has_target(key, snapshot.revision)
                        && !self.acquisition.matches(snapshot)
                    {
                        return Err(SAVE_IDENTITY);
                    }
                    self.acquisition.validate_saved(snapshot)?;
                }
                _ => self
                    .actor_saves
                    .as_ref()
                    .expect("enabled actor saves")
                    .validate_saved(snapshot)?,
            }
        }
        Ok(())
    }
}
