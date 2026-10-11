//! The chat event observation.
//!
//! This is the chat fact an authoritative session confirms for one player,
//! closed as a semantic union: every accepted wire combination of the Go
//! `protocol.ChatEvent` validator has exactly one `ChatBody` variant, and
//! rejected command restatements cannot enter the immutable `ChatEvent`.
//! The Go record reuses one wire slot for the command and the speech text,
//! one reason slot for the reject and failure reasons, and one companion
//! identity that a rejected event may carry as zero; the domain closes all
//! three seams by construction instead of runtime cross-field checks.
//!
//! The sixteen legal branch shapes are the accepted command, the four
//! rejections (malformed format, unknown companion, queue full, not
//! following), the five plain task facts (started, progress, completed,
//! timed out, stopped), the five failure reasons inside the failed task
//! fact, and the companion speech line. A malformed command never addressed
//! a companion, so its branch carries nothing; an unknown companion keeps
//! only the target name so the issuing player can check the spelling; the
//! queue-full and not-following rejections keep the full speaker and the
//! command so the player can match the rejection to the exact instruction;
//! the task facts restate the command and never carry model-generated
//! speech; and the speech line is the only branch that carries
//! `SpeechText` and the only one that does not restate the command.
//!
//! The raw kind and reason bytes belong to the evidence and protocol
//! adapters: the domain stores the closed enums and no type here exposes
//! an enum number or an `Unknown` member, so an unclassifiable wire value
//! fails classification before any constructor runs. The event carries no
//! routing recipient, no publish tick and no command sequence: those belong
//! to the publication envelope and the wire, and its event id names the
//! chat acknowledgment itself rather than a `CommandEnvelope` sequence,
//! because a chat command travels through its own FIFO with no sequence
//! at all.

use crate::identity::{CompanionId, DomainError, PlayerId};
use crate::text::{CommandText, CompanionName, DisplayName, SpeechText};

/// The companion a chat event speaks about.
///
/// The pair is total from its parts because both are already checked domain
/// values: the identity is a nonzero UUIDv4 `CompanionId` and the name a
/// canonical `CompanionName` that carries no embedded whitespace, which
/// together are exactly the Go `companion.ID.Valid` and
/// `companion.ValidateName` rules the chat validator applies to every
/// companion-bearing branch.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CompanionSpeaker {
    id: CompanionId,
    name: CompanionName,
}

impl CompanionSpeaker {
    /// Wraps one companion speaker.
    ///
    /// Construction is total: the checked parts are the whole rule, so
    /// there is no relation left to verify.
    pub fn new(id: CompanionId, name: CompanionName) -> Self {
        Self { id, name }
    }

    pub fn id(&self) -> CompanionId {
        self.id
    }

    pub fn name(&self) -> &CompanionName {
        &self.name
    }
}

/// Closed task failure reason set, mirroring the Go `TaskFailReason`
/// constants `16..=20`.
///
/// The reasons ride only inside [`TaskState::Failed`], because the wire
/// reason slot carries a reject reason on every other branch and a failure
/// reason on no plain task fact. The mapping from the wire number to the
/// variant belongs to the evidence and protocol adapters; the domain never
/// exposes the number.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TaskFailure {
    PlannerUnavailable,
    InvalidPlan,
    PathUnreachable,
    WorldChanged,
    InventoryFull,
}

/// Closed task lifecycle state set, mirroring the Go task fact kinds and
/// the failed fact's reason payload.
///
/// Every state restates the player's original command in the enclosing
/// [`ChatBody::Task`] branch; a failed task adds exactly one of the five
/// closed failure reasons.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TaskState {
    Started,
    Progress,
    Completed,
    TimedOut,
    Stopped,
    /// The failed task fact, whose wire reason slot carries one of the five
    /// closed `TaskFailure` reasons.
    Failed(TaskFailure),
}

/// The closed chat semantic union.
///
/// Each variant's field list is the cross-field rule the Go validator
/// checks at runtime: which branches carry a companion, which carry the
/// command, and which carry speech are decided by the variant shapes, so an
/// illegal combination — speech on a non-speech branch, a command on speech
/// or malformed format, a leaked companion identity on malformed format or
/// unknown companion — has no constructible state at all. There is no
/// `Unknown` member: a raw kind or reason the adapters cannot classify
/// fails classification before any constructor runs.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ChatBody {
    /// Addressing succeeded and the command entered the companion's queue.
    Accepted {
        companion: CompanionSpeaker,
        command: CommandText,
    },
    /// The command was malformed before any addressing happened, so the
    /// branch carries nothing: no companion, no name and no restated
    /// command, because a malformed command never addressed anyone.
    InvalidFormat,
    /// The named companion does not exist. Only the target name stays, so
    /// the issuing player can check the spelling; the identity and the
    /// command are absent rather than zero or empty.
    UnknownCompanion { name: CompanionName },
    /// The companion's task queue is full. The full speaker and the
    /// command stay so the player can match the rejection to the exact
    /// instruction that was refused.
    QueueFull {
        companion: CompanionSpeaker,
        command: CommandText,
    },
    /// The companion has no ongoing task to stop. The payload matches the
    /// queue-full rejection because both rejections target one companion's
    /// current task state.
    NotFollowing {
        companion: CompanionSpeaker,
        command: CommandText,
    },
    /// A task lifecycle fact for an accepted command: the task restates
    /// the player's original instruction and reports one lifecycle state,
    /// and never carries model-generated speech.
    Task {
        companion: CompanionSpeaker,
        command: CommandText,
        state: TaskState,
    },
    /// A model-generated companion speech line. This is the only branch
    /// carrying `SpeechText` and the only one that does not restate the
    /// command; the two text slots are different types, so they cannot be
    /// swapped by construction.
    Speech {
        companion: CompanionSpeaker,
        text: SpeechText,
    },
}

/// Parts of one chat event.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ChatEventParts {
    pub event_id: u64,
    pub player_id: PlayerId,
    pub player_name: DisplayName,
    pub body: ChatBody,
}

/// The chat fact an authoritative session confirms for one player.
///
/// The event owns the issuing player's checked identity and name plus the
/// closed body union, and nothing else: no routing recipient, no publish
/// tick, no raw reason byte and no command sequence belongs to the value,
/// because those are publication-envelope and wire quantities. The event id
/// names the chat acknowledgment itself rather than a `CommandEnvelope`
/// sequence, since a chat command travels through its own FIFO with no
/// sequence at all.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ChatEvent {
    event_id: u64,
    player_id: PlayerId,
    player_name: DisplayName,
    body: ChatBody,
}

impl ChatEvent {
    /// Admits a nonzero event identity and canonical command restatements.
    /// Saved task commands may preserve surrounding whitespace, but the source
    /// wire event rule still rejects those bytes. No field is trimmed or rewritten.
    pub fn try_new(parts: ChatEventParts) -> Result<Self, DomainError> {
        if parts.event_id == 0 {
            return Err(DomainError::InvalidIdentity);
        }
        let command = match &parts.body {
            ChatBody::Accepted { command, .. }
            | ChatBody::QueueFull { command, .. }
            | ChatBody::NotFollowing { command, .. }
            | ChatBody::Task { command, .. } => Some(command),
            _ => None,
        };
        if command.is_some_and(|command| !command.is_canonical()) {
            return Err(DomainError::InvalidText);
        }
        Ok(Self {
            event_id: parts.event_id,
            player_id: parts.player_id,
            player_name: parts.player_name,
            body: parts.body,
        })
    }

    pub fn event_id(&self) -> u64 {
        self.event_id
    }

    pub fn player_id(&self) -> PlayerId {
        self.player_id
    }

    pub fn player_name(&self) -> &DisplayName {
        &self.player_name
    }

    pub fn body(&self) -> &ChatBody {
        &self.body
    }
}
