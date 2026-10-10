//! Checked command ownership and bounded, source-ordered refusal publication.
//! Providers own semantic reasons; hard failures never become wire refusals.
use super::contracts::{MAX_PLAYERS, PhaseReport, Resource, ServerError, SessionKey};
use super::state::TickContext;
use mornlea_domain::{
    CommandEnvelope, CommandRejection, Event, EventRecipient, RejectReason, RoutedEvent,
};

/// Only an unowned command may fall through to another provider. A settled
/// report also consumes deferred or quiet work; refusal consumes the identity.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CommandDisposition {
    Unowned,
    Settled(PhaseReport),
    Refused(RejectReason),
}

/// Ordinary gameplay refusal is a value; trusted hard failures stay errors.
pub type CommandResult = Result<CommandDisposition, ServerError>;
/// Shared callable shape for checked providers and independently tested consumers.
pub type CommandProvider = fn(&mut TickContext<'_>, &CommandEnvelope) -> CommandResult;

/// Source admission refusals precede all later gameplay refusals. Insertion
/// order within each stage retains the serial provider's settlement order.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RejectionStage {
    Admission,
    Settlement,
}

const COMMAND_CEILING: usize = 4096;
/// The rejection log holds one held-input owner per online player.
const PLAYER_CEILING: u8 = MAX_PLAYERS;

struct OwnerRejection {
    session: SessionKey,
    rejection: CommandRejection,
}

/// One tick's compact refusal ownership, independent of success events or
/// transport queues. Separate command and held-input quotas allow idle mining
/// failures without borrowing command capacity. Consuming the log publishes
/// original owners and sequences once, with no sorting or deduplication.
pub struct CommandRejections {
    command_limit: usize,
    player_limit: u8,
    command_count: usize,
    held_count: usize,
    admission: Vec<OwnerRejection>,
    settlement: Vec<OwnerRejection>,
}

impl CommandRejections {
    /// Bounds must describe an already bounded dispatched prefix and player
    /// roster. An empty prefix remains valid for retained held-input failures.
    pub fn try_new(command_limit: usize, player_limit: u8) -> Result<Self, ServerError> {
        if command_limit > COMMAND_CEILING {
            return Err(ServerError::Capacity {
                resource: Resource::Commands,
                limit: COMMAND_CEILING,
                observed: command_limit,
            });
        }
        if player_limit == 0 {
            return Err(ServerError::InvalidInput {
                field: "rejection_players",
            });
        }
        if player_limit > PLAYER_CEILING {
            return Err(ServerError::Capacity {
                resource: Resource::Players,
                limit: usize::from(PLAYER_CEILING),
                observed: usize::from(player_limit),
            });
        }
        Ok(Self {
            command_limit,
            player_limit,
            command_count: 0,
            held_count: 0,
            admission: Vec::new(),
            settlement: Vec::new(),
        })
    }

    /// Preserve the checked envelope's identity. Capacity and allocation
    /// failures occur before either record ownership or counters change.
    pub fn record_command(
        &mut self,
        envelope: &CommandEnvelope,
        reason: RejectReason,
        stage: RejectionStage,
    ) -> Result<(), ServerError> {
        if self.command_count >= self.command_limit {
            return Err(ServerError::Capacity {
                resource: Resource::Commands,
                limit: self.command_limit,
                observed: self.command_count + 1,
            });
        }
        let session = SessionKey::from_raw(envelope.session()).ok_or(ServerError::Internal {
            invariant: "command rejection session",
        })?;
        self.push(session, envelope.sequence(), reason, stage)?;
        self.command_count += 1;
        Ok(())
    }

    /// The caller supplies the player's last admitted input sequence, including
    /// zero when retained by the source. Each human actor has one completion
    /// opportunity per tick; repeated sequences across ticks remain distinct.
    pub fn record_held_input(
        &mut self,
        session: SessionKey,
        sequence: u64,
        reason: RejectReason,
    ) -> Result<(), ServerError> {
        if self.held_count >= usize::from(self.player_limit) {
            return Err(ServerError::Capacity {
                resource: Resource::Players,
                limit: usize::from(self.player_limit),
                observed: self.held_count + 1,
            });
        }
        self.push(session, sequence, reason, RejectionStage::Settlement)?;
        self.held_count += 1;
        Ok(())
    }

    fn push(
        &mut self,
        session: SessionKey,
        sequence: u64,
        reason: RejectReason,
        stage: RejectionStage,
    ) -> Result<(), ServerError> {
        let limit = self.command_limit + usize::from(self.player_limit);
        let observed = self.command_count + self.held_count + 1;
        let lane = match stage {
            RejectionStage::Admission => &mut self.admission,
            RejectionStage::Settlement => &mut self.settlement,
        };
        // Reserve compact records only; a full event can carry large state families.
        if lane.len() == lane.capacity() {
            let next = lane.capacity().saturating_mul(2).max(8).min(limit);
            lane.try_reserve_exact(next - lane.len())
                .map_err(|_| ServerError::Capacity {
                    resource: Resource::Commands,
                    limit,
                    observed,
                })?;
        }
        lane.push(OwnerRejection {
            session,
            rejection: CommandRejection::new(sequence, reason),
        });
        Ok(())
    }

    /// Materialize owner-only wire values after all providers settle. The tick
    /// consumer must place these before success and record publications;
    /// recipient admission and mirror acknowledgment remain transport-owned.
    pub fn into_events(self) -> Vec<RoutedEvent> {
        self.admission
            .into_iter()
            .chain(self.settlement)
            .map(|record| {
                RoutedEvent::new(
                    EventRecipient::Session(record.session.get()),
                    Event::CommandRejected(record.rejection),
                )
            })
            .collect()
    }
}
