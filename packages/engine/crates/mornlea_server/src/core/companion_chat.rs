//! Configured companion chat admission and task ownership.
//!
//! This module is the single serial owner for configured companion chat
//! addressing, bounded task FIFOs, captured issuer facts, the current
//! generation and phase, and the decided chat facts the publication family
//! drains. The existing external model networking and composition stay
//! caller-owned; this module performs no I/O and never infers task state
//! from position, path, or broadcast output.

use std::collections::{BTreeMap, VecDeque};

use mornlea_domain::{
    BlockPos, ChatBody, CommandText, CompanionId, CompanionName, CompanionSpeaker, DisplayName,
    FiniteVec3, LookAngles, PlayerId,
};

use super::contracts::{AgentPlan, ServerError, SessionKey};

/// Maximum immutable configured companions.
pub const MAX_CONFIGURED_COMPANIONS: usize = 4;
/// Maximum pending commands per companion besides the current task.
pub const MAX_PENDING_COMMANDS: usize = 16;
/// Transport intake ceiling for staged chat between two ticks.
pub const MAX_CHAT_INGRESS: usize = 256;
/// Decided fact ceiling: ingress facts plus external lifecycle facts.
pub const MAX_DECIDED_FACTS: usize = MAX_CHAT_INGRESS + 4;
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
    pub(crate) fn broadcast(
        player_id: PlayerId,
        player_name: DisplayName,
        body: ChatBody,
    ) -> Self {
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
    pub(crate) slots: BTreeMap<CompanionId, ChatSlot>,
    pub(crate) decided: Vec<DecidedChatFact>,
    pub(crate) ever_configured: bool,
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
    pub(crate) fn same_configuration(&self, definitions: &[(CompanionId, CompanionName)]) -> bool {
        if self.configured.len() != definitions.len() {
            return false;
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

    /// Configured name for one companion, if configured.
    pub(crate) fn configured_name(&self, id: CompanionId) -> Option<CompanionName> {
        self.configured.get(&id).cloned()
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
        Some(CompanionSpeaker::new(
            id,
            self.configured.get(&id)?.clone(),
        ))
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
    pub(crate) fn take_decided(&mut self) -> Vec<DecidedChatFact> {
        std::mem::take(&mut self.decided)
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
}

/// One staged chat text's addressing outcome before the queue-full branch.
#[derive(Debug)]
pub(crate) enum Addressed {
    Invalid,
    Unknown { name: CompanionName },
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
pub(crate) fn parse_chat_address(
    text: &str,
    names: &BTreeMap<String, CompanionId>,
) -> Addressed {
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
