//! Whole companion startup is an atomic handoff into the existing authority and save owners.
use super::*;
use crate::core::actor_projection::project_companion;
use crate::core::companion_chat::CompanionTaskObservation;
use mornlea_storage::{CompanionSave, StoredCompanions};

pub(super) struct CompanionPersistence {
    last_tasks: Vec<CompanionTaskObservation>,
}

const INVALID: ServerError = ServerError::InvalidInput {
    field: "companion_startup",
};
const OWNERSHIP: ServerError = ServerError::Internal {
    invariant: "complete companion ownership",
};

impl AuthorityState {
    /// Installs a complete aggregate already merged and durably saved by the startup caller.
    /// Preparation cannot publish tasks or mutate any resident before ledger admission succeeds.
    pub fn enable_companion_persistence(
        &mut self,
        definitions: &[(CompanionId, CompanionName)],
        loaded: StoredCompanions,
    ) -> Result<(), ServerError> {
        self.require_live_chunks(true)?;
        if self.actor_saves.is_none()
            || !self.player_persistence_enabled()
            || self.companion_persistence.is_some()
            || self.next_tick != 0
            || !self.sessions.is_empty()
            || !self.source_companions.entries.is_empty()
            || self.companion_chat.ever_configured
            || definitions.is_empty()
            || definitions.len() > 4
            || loaded.source_schema != 5
            || loaded.revision == 0
        {
            return Err(INVALID);
        }
        // Size checks precede scans, including overlays that a malformed trusted caller could add.
        if self.residents.actors.len() > 110 - definitions.len()
            || self.residents.runtimes.len() > 110 - definitions.len()
            || self.residents.inventories.len() > 12 - definitions.len()
            || self
                .residents
                .actors
                .iter()
                .any(|actor| matches!(actor.key, ActorKey::Player(_) | ActorKey::Companion(_)))
            || self
                .residents
                .actors
                .iter()
                .any(|actor| matches!(actor.body, ActorBody::Player(_) | ActorBody::Companion(_)))
            || self
                .residents
                .runtimes
                .keys()
                .any(|key| matches!(key, ActorKey::Player(_) | ActorKey::Companion(_)))
            || self.residents.runtimes.values().any(|runtime| {
                matches!(runtime.key, ActorKey::Player(_) | ActorKey::Companion(_))
                    || matches!(
                        runtime.aux,
                        ActorAux::Player { .. } | ActorAux::Companion { .. }
                    )
            })
            || self
                .residents
                .inventories
                .keys()
                .any(|key| matches!(key, ActorKey::Player(_) | ActorKey::Companion(_)))
        {
            return Err(INVALID);
        }
        let (book, mut restored_tasks) =
            CompanionChatBook::from_persisted(definitions, &loaded).map_err(|_| INVALID)?;
        let anchor = ChunkPos::new(self.metadata.spawn_anchor.x, self.metadata.spawn_anchor.z);
        let mut prepared = Vec::with_capacity(definitions.len());
        for &id in book.configured.keys() {
            let body = loaded
                .records
                .iter()
                .find(|body| body.id.to_bytes() == id.bytes())
                .ok_or(INVALID)?;
            let mut owner =
                super::super::source_companion_restore::prepare(id, anchor, Some(body.clone()))
                    .map_err(|_| INVALID)?;
            if let Some((restored_generation, restored_task)) = restored_tasks.remove(&id) {
                let ActorAux::Companion {
                    generation, task, ..
                } = &mut owner.runtime.aux
                else {
                    return Err(INVALID);
                };
                *generation = restored_generation;
                *task = restored_task;
            }
            prepared.push((id, owner));
        }
        let persisted_revision = loaded.revision;
        let save = CompanionSave {
            revision: persisted_revision,
            agent_namespace_id: loaded.agent_namespace_id,
            records: loaded.records,
            lifecycles: loaded.lifecycles,
            queues: loaded.queues,
        };
        // This is the final fallible operation; all prepared resident transfers below are owned.
        self.actor_saves
            .as_mut()
            .unwrap()
            .retain(
                SaveValue::Companions(save),
                persisted_revision,
                false,
                true,
                true,
            )
            .map_err(|_| INVALID)?;
        for (id, owner) in prepared {
            let key = ActorKey::Companion(id);
            self.residents.actors.push(owner.actor);
            self.residents.runtimes.insert(key, owner.runtime);
            self.residents.inventories.insert(key, owner.inventory);
            self.source_companions.entries.insert(id, owner.restore);
        }
        self.companion_chat = book;
        self.companion_persistence = Some(CompanionPersistence {
            last_tasks: Vec::new(),
        });
        Ok(())
    }

    pub fn companion_persistence_enabled(&self) -> bool {
        self.companion_persistence.is_some()
    }

    /// Refuses incoherent trusted owners before a tick moves any book or resident collection.
    pub(crate) fn preflight_companion_persistence(&self) -> Result<(), ServerError> {
        if self.companion_persistence.is_none() {
            return Ok(());
        }
        if let Some(error) = self.tick_failure {
            return Err(error);
        }
        let count = self.companion_chat.configured.len();
        if count == 0
            || count > 4
            || self.source_companions.entries.len() != count
            || self.residents.actors.len() > 110
            || self.residents.runtimes.len() > 110
            || self.residents.inventories.len() > 12
        {
            return Err(OWNERSHIP);
        }
        let mut physical = BTreeSet::new();
        for actor in &self.residents.actors {
            let ActorKey::Companion(id) = actor.key else {
                if matches!(actor.body, ActorBody::Companion(_)) {
                    return Err(OWNERSHIP);
                }
                continue;
            };
            let ActorBody::Companion(body) = &actor.body else {
                return Err(OWNERSHIP);
            };
            if !self.companion_chat.configured.contains_key(&id)
                || !self.source_companions.entries.contains_key(&id)
                || !physical.insert(id)
                || !matches!(
                    actor.lifecycle,
                    ActorLifecycle::Pending | ActorLifecycle::Active
                )
                || actor.dimension != Dimension::OVERWORLD
                || body.dimension != 0
                || body.id.to_bytes() != id.bytes()
            {
                return Err(OWNERSHIP);
            }
            let runtime = self.residents.runtimes.get(&actor.key).ok_or(OWNERSHIP)?;
            if runtime.key != actor.key
                || !matches!(runtime.aux, ActorAux::Companion { .. })
                || !self.residents.inventories.contains_key(&actor.key)
            {
                return Err(OWNERSHIP);
            }
        }
        if physical.len() != count
            || self
                .residents
                .inventories
                .keys()
                .any(|key| matches!(key, ActorKey::Companion(id) if !physical.contains(id)))
        {
            return Err(OWNERSHIP);
        }
        for (key, runtime) in &self.residents.runtimes {
            if matches!(key, ActorKey::Companion(_))
                || matches!(runtime.key, ActorKey::Companion(_))
                || matches!(runtime.aux, ActorAux::Companion { .. })
            {
                let ActorKey::Companion(id) = key else {
                    return Err(OWNERSHIP);
                };
                if !physical.contains(id)
                    || runtime.key != *key
                    || !matches!(runtime.aux, ActorAux::Companion { .. })
                {
                    return Err(OWNERSHIP);
                }
            }
        }
        let aggregate = self.current_companion_aggregate()?;
        self.companion_chat
            .snapshot_persisted(&aggregate, &self.residents.runtimes)
            .map_err(|_| OWNERSHIP)?;
        Ok(())
    }

    /// Observes complete settled bodies and raw task facts, including the unpublished final tick.
    pub(crate) fn capture_companion_saves(&mut self) -> Result<(), ServerError> {
        if self.companion_persistence.is_none() {
            return Ok(());
        }
        self.preflight_companion_persistence()?;
        let mut aggregate = self.current_companion_aggregate()?;
        for actor in &self.residents.actors {
            let ActorKey::Companion(id) = actor.key else {
                continue;
            };
            if actor.lifecycle != ActorLifecycle::Active {
                continue;
            }
            let body = project_companion(actor, self.residents.inventories.get(&actor.key))
                .map_err(|_| OWNERSHIP)?;
            let retained = aggregate
                .records
                .iter_mut()
                .find(|body| body.id.to_bytes() == id.bytes())
                .ok_or(OWNERSHIP)?;
            *retained = body;
        }
        aggregate.records.sort_by_key(|body| body.id.to_bytes());
        let queues = self
            .companion_chat
            .snapshot_persisted(&aggregate, &self.residents.runtimes)
            .map_err(|_| OWNERSHIP)?;
        let tasks = self
            .companion_chat
            .persistence_observation(&queues)
            .map_err(|_| OWNERSHIP)?;
        let changed = tasks != self.companion_persistence.as_ref().unwrap().last_tasks;
        let value = SaveValue::Companions(CompanionSave {
            revision: 1,
            agent_namespace_id: aggregate.agent_namespace_id,
            records: aggregate.records,
            lifecycles: aggregate.lifecycles,
            queues,
        });
        let ledger = self.actor_saves.as_mut().ok_or(OWNERSHIP)?;
        ledger.observe(value, true, false)?;
        // Observe has already proved open, existing and eligible ownership for the raw dirty fact.
        if changed {
            ledger.mark_observation_changed(&SaveKey::Companions)?;
        }
        self.companion_persistence.as_mut().unwrap().last_tasks = tasks;
        Ok(())
    }

    fn current_companion_aggregate(&self) -> Result<StoredCompanions, ServerError> {
        let (_, SaveValue::Companions(save)) = self
            .actor_saves
            .as_ref()
            .and_then(|ledger| ledger.current(&SaveKey::Companions))
            .ok_or(OWNERSHIP)?
        else {
            return Err(OWNERSHIP);
        };
        // Ledger admission already validates variable sizes; repeat fixed counts before cloning.
        if save.records.len() > 64 || save.lifecycles.len() > 64 || save.queues.len() > 4 {
            return Err(OWNERSHIP);
        }
        Ok(StoredCompanions {
            source_schema: 5,
            revision: 1,
            agent_namespace_id: save.agent_namespace_id,
            records: save.records.clone(),
            lifecycles: save.lifecycles.clone(),
            queues: save.queues.clone(),
        })
    }
}
