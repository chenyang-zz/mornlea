//! The lifecycle provider: the repeatable Reset/Invalidate/local-close
//! projections and the epoch-scoped invalidation and generation rules.
//!
//! This owner is the policy kernel the session wiring drives whenever one
//! connection generation ends. [`LifecycleOwner`] issues strictly monotonic
//! epochs, stages the dying epoch's ordered lifecycle records for the serial
//! publication transaction, invalidates every epoch-scoped consumer owner
//! before the next epoch can open, and releases the dying generation's
//! transport exactly once. The login provider keeps the hello/login exchange
//! and its own Open/terminal publications; the controller wiring composes
//! the two, which is why [`LifecycleOwner::adopt`] registers an epoch the
//! login provider issued while [`LifecycleOwner::open`] issues one itself.
//!
//! Projection shapes: an epoch opens with exactly one `Open` record at
//! revision zero carrying the checked core generation and the core-owned
//! resource order. A reset ends its epoch with the ordered `Reset`,
//! `Invalidate` pair — `Reset` marks the turnover and `Invalidate` names the
//! core-owned resources in consumer-before-provider order — and a local
//! close ends it with the terminal `Close`, `Invalidate` pair the login
//! provider's terminal publication established. Both pairs stage into the
//! pending lifecycle state, so the controller publishes them through the
//! serial publication transaction, which consumes them exactly once, and a
//! new epoch opens only after that pending state is empty: an unconsumed
//! projection of the dying epoch rejects the next `open` typed instead of
//! carrying old records forward.
//!
//! Invalidation rules: while the dying epoch is still the current one, the
//! pending observation queue, the admission owner's journal, outbound queue,
//! local cue sources and view-validity overlay, and the audio owner's
//! committed dedup keys and pending cancellations are all cleared, so no
//! old-epoch sequence, record or attribution can enter an owner the next
//! epoch re-anchors. The preparation port invalidates the dying epoch
//! monotonically and reports what it released; the transport ticket is
//! released exactly once, and every later operation on it is the connector's
//! own typed rejection. Exhaustion of the nonzero epoch space is the typed
//! `Capacity` failure that issues nothing — never a wrapped or reused epoch.

use crate::contracts::{
    ClientError, ClientLimits, CloseReason, ConfirmedRevision, Connector, FamilyOperation,
    PreparationPort, RecordHeader, SessionEpoch, TransportTicket,
};
use crate::input::InputAdmissionState;
use crate::preparation::InvalidationReport;
use crate::presentation::frame::{LifecycleRecord, LifecycleTransition};
use crate::presentation::{
    AcceptedObservation, AudioProjectionState, LifecycleProjectionState, ResourceKey,
};

/// The core-owned resource keys in release order: consumers before
/// providers. The Python feature and native bridge handle owners live
/// outside this core, so their identities are never listed here; a
/// projection that named them would fabricate ownership this core does not
/// hold.
pub fn core_resource_order() -> Vec<ResourceKey> {
    vec![
        ResourceKey::InputJournal,
        ResourceKey::PreparationQueue,
        ResourceKey::PresentationFrames,
    ]
}

/// The `Open` projection of one fresh epoch: exactly one record at revision
/// zero under the checked core generation, naming the core-owned resource
/// order the epoch takes ownership of. The controller rebases it onto the
/// candidate revision when the publication identity requires it.
pub fn open_projection(epoch: SessionEpoch) -> Result<LifecycleRecord, ClientError> {
    let header = RecordHeader::try_new(
        epoch,
        ConfirmedRevision::new(0),
        None,
        FamilyOperation::Upsert,
    )?;
    LifecycleRecord::try_new(
        header,
        LifecycleTransition::Open,
        epoch.get(),
        core_resource_order(),
    )
}

/// The ordered reset projection of one dying epoch: `Reset` marks the epoch
/// turnover itself, then `Invalidate` releases the core-owned resources in
/// consumer-before-provider order. Both records carry the dying epoch's own
/// generation, so the new epoch's records can never be confused with them.
pub fn reset_projection(
    epoch: SessionEpoch,
    revision: ConfirmedRevision,
) -> Result<Vec<LifecycleRecord>, ClientError> {
    let header = RecordHeader::try_new(epoch, revision, None, FamilyOperation::Upsert)?;
    Ok(vec![
        LifecycleRecord::try_new(header, LifecycleTransition::Reset, epoch.get(), Vec::new())?,
        LifecycleRecord::try_new(
            header,
            LifecycleTransition::Invalidate,
            epoch.get(),
            core_resource_order(),
        )?,
    ])
}

/// The ordered terminal projection of one dying epoch under a local close:
/// `Close` ends the epoch, then `Invalidate` releases the core-owned
/// resources in consumer-before-provider order — the same ordered pair the
/// login provider's terminal publication established.
pub fn close_projection(
    epoch: SessionEpoch,
    revision: ConfirmedRevision,
) -> Result<Vec<LifecycleRecord>, ClientError> {
    let header = RecordHeader::try_new(epoch, revision, None, FamilyOperation::Upsert)?;
    Ok(vec![
        LifecycleRecord::try_new(header, LifecycleTransition::Close, epoch.get(), Vec::new())?,
        LifecycleRecord::try_new(
            header,
            LifecycleTransition::Invalidate,
            epoch.get(),
            core_resource_order(),
        )?,
    ])
}

/// The controller-lending invalidation bundle one reset or close projection
/// runs over: every epoch-scoped real owner whose retained work must not
/// cross into the next epoch. The bundle grants no authority over the
/// confirmed mirror; the controller replaces that wholesale on reset.
pub struct LifecycleInvalidation<'a> {
    observations: &'a mut Vec<AcceptedObservation>,
    admission: &'a mut InputAdmissionState,
    audio: &'a mut AudioProjectionState,
    lifecycle: &'a mut LifecycleProjectionState,
}

impl<'a> LifecycleInvalidation<'a> {
    pub fn try_new(
        observations: &'a mut Vec<AcceptedObservation>,
        admission: &'a mut InputAdmissionState,
        audio: &'a mut AudioProjectionState,
        lifecycle: &'a mut LifecycleProjectionState,
    ) -> Result<Self, ClientError> {
        Ok(Self {
            observations,
            admission,
            audio,
            lifecycle,
        })
    }
}

/// What one completed reset projection reports: the dying epoch, its staged
/// ordered record pair for the controller's publication, and the preparation
/// port's measured invalidation. Every count is measured, never invented.
pub struct ResetOutcome {
    dying: SessionEpoch,
    records: Vec<LifecycleRecord>,
    preparation: InvalidationReport,
}

impl ResetOutcome {
    fn new(
        dying: SessionEpoch,
        records: Vec<LifecycleRecord>,
        preparation: InvalidationReport,
    ) -> Self {
        Self {
            dying,
            records,
            preparation,
        }
    }

    pub fn dying(&self) -> SessionEpoch {
        self.dying
    }

    pub fn records(&self) -> &[LifecycleRecord] {
        &self.records
    }

    pub fn preparation(&self) -> &InvalidationReport {
        &self.preparation
    }
}

/// One tracked connection generation: its epoch, its transport ticket while
/// the transport is still held, the one-way release flag and the terminal
/// reason once the generation ended terminally.
struct Generation {
    epoch: SessionEpoch,
    ticket: Option<TransportTicket>,
    released: bool,
    terminal: Option<CloseReason>,
}

/// The repeatable lifecycle owner: the epoch authority and the reset and
/// local-close projections over the real epoch-scoped consumer owners.
///
/// Epochs are issued strictly monotonically from a checked counter, so an
/// old epoch never repeats and exhaustion of the nonzero space is the typed
/// `Capacity` failure. One owner serves any number of connection
/// generations: a reset drops the dying generation after its projection is
/// staged, the next `open` re-anchors the consumer owners on the fresh
/// epoch, and a terminal generation is retired by the `open` that follows
/// it, which is the reconnect-after-terminal path.
pub struct LifecycleOwner {
    limits: ClientLimits,
    counter: u64,
    live: Option<Generation>,
}

impl LifecycleOwner {
    /// Builds the owner with numbering that issues its first epoch as one.
    pub fn try_new(limits: ClientLimits) -> Result<Self, ClientError> {
        Self::try_new_after(0, limits)
    }

    /// Builds the owner naming the last epoch value already issued. A
    /// deployment resuming an interrupted process continues its numbering
    /// instead of repeating a dead epoch, and the exhaustion boundary is
    /// pinnable at the top of the nonzero space.
    pub fn try_new_after(counter: u64, limits: ClientLimits) -> Result<Self, ClientError> {
        Ok(Self {
            limits,
            counter,
            live: None,
        })
    }

    /// The epoch currently live, if one is.
    pub fn epoch(&self) -> Option<SessionEpoch> {
        self.live.as_ref().map(|generation| generation.epoch)
    }

    /// The terminal reason of the live generation, if it ended terminally.
    pub fn terminal(&self) -> Option<&CloseReason> {
        self.live
            .as_ref()
            .and_then(|generation| generation.terminal.as_ref())
    }

    /// Whether the live generation's transport has been released. A
    /// generation without a ticket owns no transport and reports released.
    pub fn transport_released(&self) -> bool {
        self.live
            .as_ref()
            .is_none_or(|generation| generation.ticket.is_none() || generation.released)
    }

    /// Adopts one already-issued epoch as the live generation without a
    /// transport: the wiring the controller uses when the login provider
    /// owns the connect path and its release. The epoch must be strictly
    /// newer than everything this owner issued, so adoption continues a
    /// numbering instead of resurrecting a dead epoch; a provider-terminated
    /// epoch is adopted with its terminal reason, which the next `open`
    /// retires exactly as it retires this owner's own terminal generations.
    pub fn adopt(
        &mut self,
        epoch: SessionEpoch,
        terminal: Option<CloseReason>,
    ) -> Result<(), ClientError> {
        if self.live.is_some() {
            return Err(ClientError::InvalidState);
        }
        if epoch.get() <= self.counter {
            return Err(ClientError::StaleEpoch);
        }
        self.counter = epoch.get();
        self.live = Some(Generation {
            epoch,
            ticket: None,
            released: false,
            terminal,
        });
        Ok(())
    }

    /// Opens the next epoch over an acquired transport ticket: issues the
    /// strictly next epoch, re-anchors the admission owner on it with every
    /// epoch-scoped owner cleared, and stages exactly one `Open` record into
    /// a fresh pending lifecycle projection state. The dying generation's
    /// projection must already be consumed — a nonempty pending state rejects
    /// the new epoch typed — and a terminal generation is retired here, the
    /// reconnect-after-terminal path.
    pub fn open(
        &mut self,
        ticket: TransportTicket,
        admission: &mut InputAdmissionState,
        lifecycle: &mut LifecycleProjectionState,
    ) -> Result<SessionEpoch, ClientError> {
        if self
            .live
            .as_ref()
            .is_some_and(|generation| generation.terminal.is_none())
        {
            return Err(ClientError::InvalidState);
        }
        if !lifecycle.pending().is_empty() {
            // The dying epoch's projection was never published; opening over
            // it would carry old records into the new epoch.
            return Err(ClientError::InvalidState);
        }
        let epoch = self.issue()?;
        let open = open_projection(epoch)?;
        admission.reset_epoch(epoch, self.limits)?;
        *lifecycle = LifecycleProjectionState::try_new(epoch.get())?.with_pending(vec![open]);
        self.live = Some(Generation {
            epoch,
            ticket: Some(ticket),
            released: false,
            terminal: None,
        });
        Ok(epoch)
    }

    /// The reset projection of the dying epoch: stages its ordered `Reset`,
    /// `Invalidate` pair into the pending lifecycle state for the
    /// controller's next publication, invalidates every consumer owner,
    /// releases the transport exactly once, invalidates the dying epoch's
    /// preparation work monotonically and drops the generation. Validation
    /// and every fallible step precede the first owner mutation.
    pub fn reset(
        &mut self,
        epoch: SessionEpoch,
        connector: &dyn Connector,
        owners: LifecycleInvalidation<'_>,
        preparation: &mut dyn PreparationPort,
    ) -> Result<ResetOutcome, ClientError> {
        let (ticket, already_released) = {
            let generation = self.live.as_ref().ok_or(ClientError::InvalidState)?;
            if generation.epoch != epoch {
                return Err(ClientError::StaleEpoch);
            }
            if generation.terminal.is_some() {
                return Err(ClientError::Disconnected);
            }
            (generation.ticket, generation.released)
        };
        // The fallible pieces run before any owner mutates: the record pair
        // construction and the one transport release.
        let records = reset_projection(epoch, ConfirmedRevision::new(0))?;
        if let Some(ticket) = ticket.filter(|_| !already_released) {
            connector.close(ticket)?;
        }
        if let Some(generation) = self.live.as_mut() {
            generation.released = true;
        }
        self.stage_dying_records(owners, epoch, records.clone());
        let report = preparation.invalidate(epoch);
        self.live = None;
        Ok(ResetOutcome::new(epoch, records, report))
    }

    /// The local-close projection of the live epoch: stages its ordered
    /// `Close`, `Invalidate` pair, performs the same consumer invalidation
    /// as a reset, releases the transport exactly once and marks the
    /// generation terminal. A repeated close of the same generation
    /// succeeds and does nothing — no second pair, no second release — and
    /// closing when no generation is live succeeds the same way, because a
    /// generation that already ended has nothing left to close.
    pub fn close(
        &mut self,
        epoch: SessionEpoch,
        connector: &dyn Connector,
        owners: LifecycleInvalidation<'_>,
        preparation: &mut dyn PreparationPort,
    ) -> Result<Vec<LifecycleRecord>, ClientError> {
        let Some(generation) = self.live.as_mut() else {
            return Ok(Vec::new());
        };
        if generation.epoch != epoch {
            return Err(ClientError::StaleEpoch);
        }
        if generation.terminal.is_some() {
            return Ok(Vec::new());
        }
        let records = close_projection(epoch, ConfirmedRevision::new(0))?;
        if let Some(ticket) = generation.ticket.filter(|_| !generation.released) {
            connector.close(ticket)?;
            generation.released = true;
        }
        generation.terminal = Some(CloseReason::LocalClose);
        self.stage_dying_records(owners, epoch, records.clone());
        let _ = preparation.invalidate(epoch);
        Ok(records)
    }

    /// Issues the strictly next epoch from the checked counter. Exhaustion
    /// of the nonzero space is the typed capacity failure that issues
    /// nothing; the caller's state is untouched.
    fn issue(&mut self) -> Result<SessionEpoch, ClientError> {
        let next = self.counter.checked_add(1).ok_or(ClientError::Capacity)?;
        self.counter = next;
        SessionEpoch::try_new(next)
    }

    /// Clears every epoch-scoped consumer owner and stages the dying
    /// epoch's records into the pending lifecycle state. The admission owner
    /// is reset onto the dying epoch's own identity — its journal, outbound
    /// queue, cue sources and overlay are cleared and its sequence space
    /// restarts — because the next `open` re-anchors it on the fresh epoch
    /// before any batch can commit.
    fn stage_dying_records(
        &mut self,
        owners: LifecycleInvalidation<'_>,
        epoch: SessionEpoch,
        records: Vec<LifecycleRecord>,
    ) {
        let LifecycleInvalidation {
            observations,
            admission,
            audio,
            lifecycle,
        } = owners;
        observations.clear();
        let _ = admission.reset_epoch(epoch, self.limits);
        *audio = AudioProjectionState::try_new()
            .expect("the fresh audio owner is the checked empty value");
        let generation = lifecycle.generation();
        let mut pending = lifecycle.pending().to_vec();
        pending.extend(records);
        *lifecycle = LifecycleProjectionState::try_new(generation)
            .expect("the checked lifecycle state")
            .with_pending(pending);
    }
}
