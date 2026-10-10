//! Complete saved-task transfers belong to the serial chat owner, with no disk or actor mutation.
use super::*;
use crate::core::contracts::{ActorAux, ActorKey, ActorRuntime};
use mornlea_storage::{
    COMPANION_MAX_PLAN_STEPS, COMPANION_PLAN_STEP_FOLLOW, COMPANION_TASK_PLANNING,
    COMPANION_TASK_QUEUED, COMPANION_TASK_RUNNING, COMPANION_TASK_VALIDATING, CompanionSave,
    StoredCompanionQueue, StoredCompanionTask, StoredCompanions, companions_encoded_len,
    merge_companions_v5,
};
use std::convert::Infallible;

/// Fresh task generations and saved runtime payloads, independent of actor placement.
pub type RestoredCompanionTasks = BTreeMap<CompanionId, (u64, StoredCompanionTask)>;

/// Source dirty comparison includes volatile task facts omitted by the durable wire format.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct CompanionTaskObservation {
    queue: StoredCompanionQueue,
    generation: Option<u64>,
    phase: Option<CompanionChatPhase>,
    summary: String,
}

const INVALID: ServerError = ServerError::InvalidInput {
    field: "companion_task_persistence",
};
const INVARIANT: ServerError = ServerError::Internal {
    invariant: "companion task persistence",
};

impl CompanionChatBook {
    /// Captures bounded raw facts only after the complete normalized queue join is validated.
    /// Idle slots are absent, matching the source owner's nil-to-empty quiet observation.
    pub(crate) fn persistence_observation(
        &self,
        queues: &[StoredCompanionQueue],
    ) -> Result<Vec<CompanionTaskObservation>, ServerError> {
        if queues.len() > MAX_CONFIGURED_COMPANIONS {
            return Err(INVARIANT);
        }
        let mut observations = Vec::with_capacity(queues.len());
        for queue in queues {
            let id = CompanionId::try_from_bytes(queue.id.to_bytes()).map_err(|_| INVARIANT)?;
            let slot = self.slots.get(&id).ok_or(INVARIANT)?;
            let current = slot.current.as_ref();
            let summary = current
                .and_then(|task| task.plan.as_ref())
                .map_or("", |plan| plan.summary.as_str());
            if queue.pending.len() > MAX_PENDING_COMMANDS
                || queue.current.command.len() > mornlea_storage::COMPANION_MAX_TASK_COMMAND_BYTES
                || queue.current.plan_steps.len() > COMPANION_MAX_PLAN_STEPS
                || queue.pending.iter().any(|command| {
                    command.len() > mornlea_storage::COMPANION_MAX_TASK_COMMAND_BYTES
                })
                || summary.len() > 512
                || current
                    .and_then(|task| task.plan.as_ref())
                    .is_some_and(|plan| plan.steps.len() > COMPANION_MAX_PLAN_STEPS)
                || queue.has_current != current.is_some()
            {
                return Err(INVARIANT);
            }
            observations.push(CompanionTaskObservation {
                queue: queue.clone(),
                generation: current.map(|task| task.generation),
                phase: current.map(|task| task.phase),
                summary: summary.to_owned(),
            });
        }
        Ok(observations)
    }

    /// Prepares the whole configured owner and runtime handoff without events or partial mutation.
    /// The caller must first durably accept the v5 merge, then install neutral actor runtimes.
    pub fn from_persisted(
        definitions: &[(CompanionId, CompanionName)],
        loaded: &StoredCompanions,
    ) -> Result<(Self, RestoredCompanionTasks), ServerError> {
        let mut book = Self::new();
        book.apply_configuration(definitions).map_err(|_| INVALID)?;
        let loaded = book.checked_persisted_aggregate(loaded)?;
        let issuer = restored_issuer();
        let mut tasks = BTreeMap::new();
        for queue in loaded.queues {
            let id = CompanionId::try_from_bytes(queue.id.to_bytes()).map_err(|_| INVALID)?;
            let slot = book.slots.get_mut(&id).ok_or(INVALID)?;
            if queue.has_current {
                let mut task = queue.current;
                if matches!(
                    task.state,
                    COMPANION_TASK_QUEUED
                        | COMPANION_TASK_PLANNING
                        | COMPANION_TASK_VALIDATING
                        | COMPANION_TASK_RUNNING
                ) {
                    let running = task.state == COMPANION_TASK_RUNNING;
                    if !running {
                        task.state = COMPANION_TASK_QUEUED;
                        task.plan_steps.clear();
                        task.step_index = 0;
                        task.start_tick = 0;
                        task.deadline_ticks = 0;
                    }
                    slot.generation = 1;
                    slot.restored_follow = running
                        && task
                            .plan_steps
                            .last()
                            .is_some_and(|step| step.kind == COMPANION_PLAN_STEP_FOLLOW);
                    slot.current = Some(CompanionChatTask {
                        generation: 1,
                        command: CommandText::try_from_persisted(task.command.clone())
                            .map_err(|_| INVALID)?,
                        issuer: issuer.clone(),
                        source_tick: 0,
                        phase: if running {
                            CompanionChatPhase::Running
                        } else {
                            CompanionChatPhase::Queued
                        },
                        // Saves carry no model summary; the checked steps stay in runtime ownership.
                        plan: None,
                    });
                    tasks.insert(id, (1, task));
                }
            }
            for command in queue.pending {
                slot.pending.push_back((
                    CommandText::try_from_persisted(command).map_err(|_| INVALID)?,
                    issuer.clone(),
                    0,
                ));
            }
        }
        Ok((book, tasks))
    }

    /// Captures complete task queues while leaving body, lifecycle and memory ownership untouched.
    /// Planning saves as queued; running progress comes only from its matching authoritative runtime.
    pub fn snapshot_persisted(
        &self,
        aggregate: &StoredCompanions,
        runtimes: &BTreeMap<ActorKey, ActorRuntime>,
    ) -> Result<Vec<StoredCompanionQueue>, ServerError> {
        let mut candidate = self.checked_persisted_aggregate(aggregate)?;
        if self.slots.len() != self.configured.len() {
            return Err(INVARIANT);
        }
        let mut queues = Vec::with_capacity(self.configured.len());
        for &id in self.configured.keys() {
            let slot = self.slots.get(&id).ok_or(INVARIANT)?;
            if slot.pending.len() > MAX_PENDING_COMMANDS {
                return Err(INVARIANT);
            }
            let mut queue = StoredCompanionQueue {
                id: mornlea_storage::PlayerId::from_bytes(id.bytes()),
                ..Default::default()
            };
            if let Some(current) = &slot.current {
                if current.generation == 0 || current.generation != slot.generation {
                    return Err(INVARIANT);
                }
                queue.has_current = true;
                queue.current = match current.phase {
                    CompanionChatPhase::Queued | CompanionChatPhase::Planning => {
                        StoredCompanionTask {
                            command: current.command.as_str().to_owned(),
                            state: COMPANION_TASK_QUEUED,
                            ..Default::default()
                        }
                    }
                    CompanionChatPhase::Running => {
                        let key = ActorKey::Companion(id);
                        let runtime = runtimes.get(&key).ok_or(INVARIANT)?;
                        let ActorAux::Companion {
                            generation, task, ..
                        } = &runtime.aux
                        else {
                            return Err(INVARIANT);
                        };
                        // Reject oversized untrusted runtime payloads before cloning variable data.
                        if task.command.len() > mornlea_storage::COMPANION_MAX_TASK_COMMAND_BYTES
                            || task.plan_steps.len() > COMPANION_MAX_PLAN_STEPS
                        {
                            return Err(INVALID);
                        }
                        if runtime.key != key
                            || *generation != current.generation
                            || task.state != COMPANION_TASK_RUNNING
                            || task.command != current.command.as_str()
                        {
                            return Err(INVARIANT);
                        }
                        task.clone()
                    }
                };
            }
            queue.pending = slot
                .pending
                .iter()
                .map(|(command, _, _)| command.as_str().to_owned())
                .collect();
            if queue.has_current || !queue.pending.is_empty() {
                queues.push(queue);
            }
        }
        candidate.queues = queues;
        let save = CompanionSave {
            revision: candidate.revision,
            agent_namespace_id: candidate.agent_namespace_id,
            records: candidate.records,
            lifecycles: candidate.lifecycles,
            queues: candidate.queues,
        };
        companions_encoded_len(&save).map_err(|_| INVALID)?;
        Ok(save.queues)
    }

    fn checked_persisted_aggregate(
        &self,
        loaded: &StoredCompanions,
    ) -> Result<StoredCompanions, ServerError> {
        // The fixed-size body union can be prepared only after every collection count is bounded.
        if self.configured.len() > MAX_CONFIGURED_COMPANIONS
            || loaded.source_schema != 5
            || loaded.records.len() > mornlea_storage::COMPANION_MAX_STORED
            || loaded.lifecycles.len() > mornlea_storage::COMPANION_MAX_STORED
            || loaded.queues.len() > MAX_CONFIGURED_COMPANIONS
        {
            return Err(INVALID);
        }
        let mut active = Vec::with_capacity(self.configured.len());
        for id in self.configured.keys() {
            let body = loaded
                .records
                .iter()
                .find(|body| body.id.to_bytes() == id.bytes())
                .ok_or(INVALID)?;
            active.push(body.clone());
        }
        let (value, changed) =
            merge_companions_v5::<Infallible>(loaded, &active, None).map_err(|_| INVALID)?;
        if changed {
            return Err(INVALID);
        }
        Ok(value)
    }
}

fn restored_issuer() -> CompanionChatIssuer {
    let mut bytes = [0; 16];
    bytes[6] = 0x40;
    bytes[8] = 0x80;
    CompanionChatIssuer {
        session: None,
        player_id: PlayerId::try_from_bytes(bytes).expect("source restored identity is canonical"),
        player_name: DisplayName::try_from_canonical("未知发令者".into())
            .expect("source restored name is valid"),
        position: FiniteVec3::try_new([0.0, 1.0, 0.0]).expect("source restored pose is finite"),
        look: LookAngles::try_new(0.0, 0.0).expect("source restored look is finite"),
        look_hit: None,
    }
}

#[cfg(test)]
#[path = "companion_chat_observation_tests.rs"]
mod observation_tests;
