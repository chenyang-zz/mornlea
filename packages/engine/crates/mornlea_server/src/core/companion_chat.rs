//! Configured companion chat admission and task ownership.
//!
//! This module is the single serial owner for configured companion chat
//! addressing, bounded task FIFOs, captured issuer facts, the current
//! generation and phase, and the decided chat facts the publication family
//! drains. The existing external model networking and composition stay
//! caller-owned; this module performs no I/O and never infers task state
//! from position, path, or broadcast output.

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use mornlea_domain::{
    BlockPos, ChatBody, CommandText, CompanionId, CompanionName, CompanionSpeaker, DisplayName,
    FiniteVec3, LookAngles, PlayerId, TaskState,
};

use super::contracts::{AgentPlan, PlanStep, ServerError, SessionKey};

/// Maximum immutable configured companions.
pub const MAX_CONFIGURED_COMPANIONS: usize = 4;
/// Maximum pending commands per companion besides the current task.
pub const MAX_PENDING_COMMANDS: usize = 16;
/// Transport intake ceiling for staged chat between two ticks.
pub const MAX_CHAT_INGRESS: usize = 256;
/// Decided fact ceiling: ingress facts plus external lifecycle facts.
pub const MAX_DECIDED_FACTS: usize = MAX_CHAT_INGRESS + 4;
/// External lifecycle facts allowed between two publication drains.
///
/// Install-started and terminal facts reserve this quota; tick-boundary
/// ingress facts (including successful stop facts) never consume it.
pub const MAX_EXTERNAL_LIFECYCLE: usize = 4;
/// Exact trimmed stop instruction.
pub const STOP_COMMAND: &str = "停止";

/// Issuer facts captured at the authoritative chat ingress boundary.
#[derive(Clone, Debug, PartialEq)]
pub struct CompanionChatIssuer {
    /// Session that issued the command.
    pub session: SessionKey,
    /// Player identity of the issuer.
    pub player_id: PlayerId,
    /// Display name of the issuer.
    pub player_name: DisplayName,
    /// Actor position at ingress, or the source fallback.
    pub position: FiniteVec3,
    /// Actor look at ingress, or zero look.
    pub look: LookAngles,
    /// Ray hit at ingress, if any.
    pub look_hit: Option<BlockPos>,
}

/// Current task phase owned by the chat book.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CompanionChatPhase {
    /// Waiting for a planning take.
    Queued,
    /// Taken for planning, awaiting a validated plan install.
    Planning,
    /// Running with an installed checked plan.
    Running,
}

/// One current task with its captured ingress facts.
#[derive(Clone, Debug, PartialEq)]
pub struct CompanionChatTask {
    /// Checked generation naming this task.
    pub generation: u64,
    /// Exact trimmed instruction.
    pub command: CommandText,
    /// Original issuer captured at ingress.
    pub issuer: CompanionChatIssuer,
    /// Tick that staged the instruction.
    pub source_tick: u64,
    /// Current phase.
    pub phase: CompanionChatPhase,
    /// Installed checked plan while running.
    pub plan: Option<AgentPlan>,
}

/// Bounded queue view cloned for one companion.
#[derive(Clone, Debug, PartialEq)]
pub struct CompanionChatQueueView {
    /// Current task, if any.
    pub current: Option<CompanionChatTask>,
    /// Pending commands with captured issuers and source ticks.
    pub pending: Vec<(CommandText, CompanionChatIssuer, u64)>,
}

/// One slot: the generation counter and the bounded FIFO.
#[derive(Clone, Debug, PartialEq)]
struct ChatSlot {
    generation: u64,
    current: Option<CompanionChatTask>,
    pending: VecDeque<(CommandText, CompanionChatIssuer, u64)>,
}

/// One decided chat fact awaiting the publication drain.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct DecidedChatFact {
    pub(crate) player_id: PlayerId,
    pub(crate) player_name: DisplayName,
    /// `None` broadcasts, `Some` rejects to the sender only.
    pub(crate) recipient: Option<SessionKey>,
    pub(crate) body: ChatBody,
}

impl DecidedChatFact {
    pub(crate) fn broadcast(player_id: PlayerId, player_name: DisplayName, body: ChatBody) -> Self {
        Self {
            player_id,
            player_name,
            recipient: None,
            body,
        }
    }

    pub(crate) fn sender_only(
        player_id: PlayerId,
        player_name: DisplayName,
        session: SessionKey,
        body: ChatBody,
    ) -> Self {
        Self {
            player_id,
            player_name,
            recipient: Some(session),
            body,
        }
    }
}

/// Serial owner for configured chat addressing and task queues.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CompanionChatBook {
    pub(crate) configured: BTreeMap<CompanionId, CompanionName>,
    pub(crate) by_name: BTreeMap<String, CompanionId>,
    slots: BTreeMap<CompanionId, ChatSlot>,
    pub(crate) decided: Vec<DecidedChatFact>,
    pub(crate) ever_configured: bool,
    external_lifecycle: usize,
}

impl CompanionChatBook {
    /// Empty book with no addressed companion.
    pub fn new() -> Self {
        Self::default()
    }

    /// Atomically validates and installs the immutable configuration.
    ///
    /// At most four entries, no duplicate ids or names; the map stays
    /// sorted by identity. Identical repeated configuration is idempotent.
    pub(crate) fn apply_configuration(
        &mut self,
        definitions: &[(CompanionId, CompanionName)],
    ) -> Result<(), ServerError> {
        if definitions.len() > MAX_CONFIGURED_COMPANIONS {
            return Err(ServerError::InvalidInput {
                field: "companion_chat_config",
            });
        }
        let mut configured = BTreeMap::new();
        let mut by_name: BTreeMap<String, CompanionId> = BTreeMap::new();
        for (id, name) in definitions {
            if configured.contains_key(id) || by_name.contains_key(name.as_str()) {
                return Err(ServerError::InvalidInput {
                    field: "companion_chat_config",
                });
            }
            configured.insert(*id, name.clone());
            if by_name.insert(name.as_str().to_owned(), *id).is_some() {
                return Err(ServerError::InvalidInput {
                    field: "companion_chat_config",
                });
            }
        }
        if configured.len() != definitions.len() {
            return Err(ServerError::InvalidInput {
                field: "companion_chat_config",
            });
        }
        self.configured = configured;
        self.by_name = by_name;
        self.slots.retain(|id, _| self.configured.contains_key(id));
        for id in self.configured.keys() {
            self.slots.entry(*id).or_insert(ChatSlot {
                generation: 0,
                current: None,
                pending: VecDeque::new(),
            });
        }
        self.ever_configured = true;
        Ok(())
    }

    /// Whether the exact configuration is already installed.
    ///
    /// Length alone never proves identity: the candidate pairs are checked
    /// for unique ids and names first, so a duplicate-heavy request with a
    /// matching length (for example existing `[A, B]` against `[A, A]`) is
    /// not reported identical. Only the complete checked map comparison
    /// decides.
    pub(crate) fn same_configuration(&self, definitions: &[(CompanionId, CompanionName)]) -> bool {
        if definitions.len() != self.configured.len() {
            return false;
        }
        let mut seen_ids = BTreeSet::new();
        let mut seen_names = BTreeSet::new();
        for (id, name) in definitions {
            if !seen_ids.insert(*id) || !seen_names.insert(name.as_str()) {
                return false;
            }
        }
        definitions.iter().all(|(id, name)| {
            self.configured
                .get(id)
                .is_some_and(|existing| existing.as_str() == name.as_str())
        })
    }

    /// Whether any task or decided fact exists.
    pub(crate) fn has_activity(&self) -> bool {
        !self.decided.is_empty()
            || self.slots.values().any(|slot| {
                slot.current.is_some() || !slot.pending.is_empty() || slot.generation != 0
            })
    }

    /// Whether one companion id is configured.
    pub(crate) fn is_configured(&self, id: CompanionId) -> bool {
        self.configured.contains_key(&id)
    }

    /// Bounded queue view for one companion.
    pub(crate) fn queue_view(&self, id: CompanionId) -> Option<CompanionChatQueueView> {
        let slot = self.slots.get(&id)?;
        Some(CompanionChatQueueView {
            current: slot.current.clone(),
            pending: slot.pending.iter().cloned().collect(),
        })
    }

    /// Speaker for one configured companion.
    pub(crate) fn speaker(&self, id: CompanionId) -> Option<CompanionSpeaker> {
        Some(CompanionSpeaker::new(id, self.configured.get(&id)?.clone()))
    }

    /// Pushes one decided fact within the bounded buffer.
    pub(crate) fn push_decided(&mut self, fact: DecidedChatFact) -> Result<(), ServerError> {
        if self.decided.len() >= MAX_DECIDED_FACTS {
            return Err(ServerError::Capacity {
                resource: super::contracts::Resource::Commands,
                limit: MAX_DECIDED_FACTS,
                observed: self.decided.len() + 1,
            });
        }
        self.decided.push(fact);
        Ok(())
    }

    /// Drains decided facts for one publication pass.
    ///
    /// Draining also resets the external lifecycle quota: a new budget of
    /// install and terminal facts opens after every publication.
    pub(crate) fn take_decided(&mut self) -> Vec<DecidedChatFact> {
        self.external_lifecycle = 0;
        std::mem::take(&mut self.decided)
    }

    /// External lifecycle facts reserved since the last drain.
    pub(crate) fn external_lifecycle_used(&self) -> usize {
        self.external_lifecycle
    }

    /// Reserves one external lifecycle fact without mutating any task.
    ///
    /// Refuses once four external facts are already reserved. Decided-buffer
    /// and event-id headroom stay the caller's atomic preflight, which covers
    /// the already staged chat plus the new fact before any mutation.
    pub(crate) fn reserve_external_lifecycle(&mut self) -> Result<(), ServerError> {
        if self.external_lifecycle >= MAX_EXTERNAL_LIFECYCLE {
            return Err(ServerError::Capacity {
                resource: super::contracts::Resource::Commands,
                limit: MAX_EXTERNAL_LIFECYCLE,
                observed: self.external_lifecycle + 1,
            });
        }
        self.external_lifecycle += 1;
        Ok(())
    }

    /// Moves the current queued task into planning, returning it exactly once.
    ///
    /// Returns `None` when the companion is unconfigured, has no current
    /// task, or the current task is not queued. The caller owns liveness
    /// gating; the book owns the phase transition and the retained
    /// generation, command, issuer, source tick, and plan.
    pub(crate) fn take_queued_for_planning(
        &mut self,
        id: CompanionId,
    ) -> Option<CompanionChatTask> {
        let slot = self.slots.get_mut(&id)?;
        let current = slot.current.as_mut()?;
        if current.phase != CompanionChatPhase::Queued {
            return None;
        }
        current.phase = CompanionChatPhase::Planning;
        Some(current.clone())
    }

    /// Whether one slot has room for another pending command.
    pub(crate) fn pending_has_capacity(&self, id: CompanionId) -> bool {
        self.slots
            .get(&id)
            .is_some_and(|slot| slot.pending.len() < MAX_PENDING_COMMANDS)
    }

    /// Reserves one pending FIFO entry behind the current task.
    ///
    /// Returns `false` without mutation when the slot already holds
    /// sixteen pending commands. A missing configured slot is a caller
    /// error; FIFO-full is an ordinary refusal.
    pub(crate) fn try_admit(
        &mut self,
        id: CompanionId,
        command: CommandText,
        issuer: CompanionChatIssuer,
        source_tick: u64,
    ) -> Result<bool, ServerError> {
        let slot = self.slots.get_mut(&id).ok_or(ServerError::InvalidInput {
            field: "companion_chat_slot",
        })?;
        if slot.pending.len() >= MAX_PENDING_COMMANDS {
            return Ok(false);
        }
        slot.pending.push_back((command, issuer, source_tick));
        Ok(true)
    }

    /// Promotes one pending head per companion that has no current task.
    ///
    /// The generation counter advances under a checked addition before the
    /// head is popped, so exhaustion refuses before any FIFO or current
    /// mutation. The new current task waits in `Queued` for a real planning
    /// take and never pretends to run.
    pub(crate) fn promote_heads(&mut self) -> Result<(), ServerError> {
        for slot in self.slots.values_mut() {
            if slot.current.is_some() || slot.pending.is_empty() {
                continue;
            }
            let next = slot
                .generation
                .checked_add(1)
                .ok_or(ServerError::Capacity {
                    resource: super::contracts::Resource::Commands,
                    limit: usize::MAX,
                    observed: usize::MAX,
                })?;
            let (command, issuer, source_tick) =
                slot.pending.pop_front().expect("checked pending head");
            slot.generation = next;
            slot.current = Some(CompanionChatTask {
                generation: next,
                command,
                issuer,
                source_tick,
                phase: CompanionChatPhase::Queued,
                plan: None,
            });
        }
        Ok(())
    }

    /// Returns the current task when it can be stopped, without mutation.
    ///
    /// Only a `Running` task carrying a checked plan whose last step is a
    /// terminal `Follow` is stoppable; earlier finite steps before that
    /// terminal follow are permitted. Idle, queued, planning, plan-less, or
    /// non-follow running tasks all yield `None`.
    pub(crate) fn peek_stoppable(&self, id: CompanionId) -> Option<CompanionChatTask> {
        let current = self.slots.get(&id)?.current.as_ref()?;
        if current.phase != CompanionChatPhase::Running {
            return None;
        }
        if !matches!(
            current.plan.as_ref()?.steps.last(),
            Some(PlanStep::Follow { .. })
        ) {
            return None;
        }
        Some(current.clone())
    }

    /// Clears a stoppable current task, returning the original task facts.
    ///
    /// Returns `None` under the same conditions as [`Self::peek_stoppable`];
    /// the pending FIFO, the generation counter, and every other companion
    /// are untouched.
    pub(crate) fn stop_current(&mut self, id: CompanionId) -> Option<CompanionChatTask> {
        self.peek_stoppable(id)?;
        self.slots.get_mut(&id)?.current.take()
    }

    /// Whether one companion holds a running task of this generation.
    pub(crate) fn is_running_generation(&self, id: CompanionId, generation: u64) -> bool {
        self.slots.get(&id).is_some_and(|slot| {
            slot.current.as_ref().is_some_and(|current| {
                current.phase == CompanionChatPhase::Running && current.generation == generation
            })
        })
    }

    /// Returns the installable current task without mutation.
    ///
    /// Only a current `Planning` task with a matching generation is
    /// installable; anything else (including unconfigured ids) yields `None`.
    pub(crate) fn peek_installable(
        &self,
        id: CompanionId,
        generation: u64,
    ) -> Option<CompanionChatTask> {
        let current = self.slots.get(&id)?.current.as_ref()?;
        if current.phase != CompanionChatPhase::Planning || current.generation != generation {
            return None;
        }
        Some(current.clone())
    }

    /// Marks an installable task running with its checked plan.
    ///
    /// Revalidates the same conditions as [`Self::peek_installable`]; the
    /// pending FIFO, the generation counter, and every other companion are
    /// untouched.
    pub(crate) fn commit_install(
        &mut self,
        id: CompanionId,
        generation: u64,
        plan: AgentPlan,
    ) -> Option<CompanionChatTask> {
        let current = self.slots.get_mut(&id)?.current.as_mut()?;
        if current.phase != CompanionChatPhase::Planning || current.generation != generation {
            return None;
        }
        current.phase = CompanionChatPhase::Running;
        current.plan = Some(plan);
        Some(current.clone())
    }

    /// Whether one terminal state may finish the current task.
    ///
    /// Completed and timed-out finish only a `Running` task; failed finishes
    /// a `Planning` or `Running` task. Queued tasks never complete or fail
    /// here, and the generation must match.
    fn finishable_phase(phase: CompanionChatPhase, state: TaskState) -> bool {
        match state {
            TaskState::Completed | TaskState::TimedOut => phase == CompanionChatPhase::Running,
            TaskState::Failed(_) => {
                phase == CompanionChatPhase::Planning || phase == CompanionChatPhase::Running
            }
            TaskState::Started | TaskState::Progress | TaskState::Stopped => false,
        }
    }

    /// Returns the finishable current task without mutation.
    pub(crate) fn peek_finishable(
        &self,
        id: CompanionId,
        generation: u64,
        state: TaskState,
    ) -> Option<CompanionChatTask> {
        let current = self.slots.get(&id)?.current.as_ref()?;
        if current.generation != generation || !Self::finishable_phase(current.phase, state) {
            return None;
        }
        Some(current.clone())
    }

    /// Clears a finishable current task, returning the original task facts.
    ///
    /// Revalidates the same conditions as [`Self::peek_finishable`]; the
    /// pending FIFO, the generation counter, and every other companion are
    /// untouched, so the next tick can promote the pending head.
    pub(crate) fn commit_finish(
        &mut self,
        id: CompanionId,
        generation: u64,
        state: TaskState,
    ) -> Option<CompanionChatTask> {
        self.peek_finishable(id, generation, state)?;
        self.slots.get_mut(&id)?.current.take()
    }
}

/// One staged chat text's addressing outcome before the queue-full branch.
#[derive(Debug)]
pub(crate) enum Addressed {
    Invalid,
    Unknown {
        name: CompanionName,
    },
    Matched {
        id: CompanionId,
        name: CompanionName,
        command: CommandText,
    },
}

/// Parses one staged chat text against the configured companion names.
///
/// The shape is `@<name><separator><command>`: the name runs to the first
/// whitespace separator and the remainder is Go-trimmed before the checked
/// `CommandText` validation. Every malformed shape rejects as invalid
/// format; a well-formed name matching no configured companion rejects as
/// unknown.
pub(crate) fn parse_chat_address(text: &str, names: &BTreeMap<String, CompanionId>) -> Addressed {
    let Some(rest) = text.strip_prefix('@') else {
        return Addressed::Invalid;
    };
    let Some(separator) = rest.find(char::is_whitespace) else {
        return Addressed::Invalid;
    };
    let (name, remainder) = rest.split_at(separator);
    // Go TrimSpace on the command: both leading and trailing whitespace.
    let trimmed = remainder.trim_matches(char::is_whitespace);
    if trimmed.is_empty() {
        return Addressed::Invalid;
    }
    let Ok(name) = CompanionName::try_from_canonical(name.to_owned()) else {
        return Addressed::Invalid;
    };
    let Ok(command) = CommandText::try_from_canonical(trimmed.to_owned()) else {
        return Addressed::Invalid;
    };
    match names.get(name.as_str()) {
        Some(&id) => Addressed::Matched { id, name, command },
        None => Addressed::Unknown { name },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_uuid(tag: u8) -> [u8; 16] {
        let mut bytes = [0u8; 16];
        bytes[0] = tag.max(1);
        bytes[6] = 0x40;
        bytes[8] = 0x80;
        bytes
    }

    fn test_issuer() -> CompanionChatIssuer {
        CompanionChatIssuer {
            session: SessionKey::from_raw(1).unwrap(),
            player_id: PlayerId::try_from_bytes(test_uuid(1)).unwrap(),
            player_name: DisplayName::try_from_canonical("Ada".to_owned()).unwrap(),
            position: FiniteVec3::try_new([0.0, 1.0, 0.0]).unwrap(),
            look: LookAngles::try_new(0.0, 0.0).unwrap(),
            look_hit: None,
        }
    }

    fn configured_book() -> (CompanionChatBook, CompanionId) {
        let mut book = CompanionChatBook::new();
        let id = CompanionId::try_from_bytes(test_uuid(9)).unwrap();
        let name = CompanionName::try_from_canonical("阿木".to_owned()).unwrap();
        book.apply_configuration(&[(id, name)]).unwrap();
        (book, id)
    }

    fn test_command(text: &str) -> CommandText {
        CommandText::try_from_canonical(text.to_owned()).unwrap()
    }

    /// Generation exhaustion refuses before any FIFO or current mutation:
    /// the pending head stays queued with the counter and current intact.
    #[test]
    fn generation_exhaustion_refuses_before_promotion_mutation() {
        let (mut book, id) = configured_book();
        assert!(book
            .try_admit(id, test_command("dig"), test_issuer(), 7)
            .unwrap());
        book.slots.get_mut(&id).expect("slot").generation = u64::MAX;
        let snapshot = book.clone();
        assert_eq!(
            book.promote_heads(),
            Err(ServerError::Capacity {
                resource: super::contracts::Resource::Commands,
                limit: usize::MAX,
                observed: usize::MAX,
            })
        );
        assert_eq!(book, snapshot);
        assert!(book.slots.get(&id).expect("slot").current.is_none());
        assert_eq!(book.slots.get(&id).expect("slot").pending.len(), 1);
    }

    /// The last usable generation promotes exactly once to `u64::MAX`, and
    /// the promoted task keeps its queued command, issuer, and source tick
    /// for the once-only planning take.
    #[test]
    fn last_usable_generation_promotes_once_with_receipt() {
        let (mut book, id) = configured_book();
        assert!(book
            .try_admit(id, test_command("dig"), test_issuer(), 7)
            .unwrap());
        book.slots.get_mut(&id).expect("slot").generation = u64::MAX - 1;
        book.promote_heads().unwrap();
        let slot = book.slots.get(&id).expect("slot");
        assert_eq!(slot.generation, u64::MAX);
        let current = slot.current.as_ref().expect("promoted current");
        assert_eq!(current.generation, u64::MAX);
        assert_eq!(current.command, test_command("dig"));
        assert_eq!(current.issuer, test_issuer());
        assert_eq!(current.source_tick, 7);
        assert_eq!(current.phase, CompanionChatPhase::Queued);
        assert!(slot.pending.is_empty());
        let taken = book.take_queued_for_planning(id).expect("receipt");
        assert_eq!(taken.generation, u64::MAX);
        assert!(book.take_queued_for_planning(id).is_none());
    }
}
