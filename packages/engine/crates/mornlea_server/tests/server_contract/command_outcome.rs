//! Executing interface doubles and bounded refusal publication contracts.
//! These consumers do not qualify any production command provider.
use mornlea_domain::PlayerId;
use mornlea_domain::{
    Command, CommandEnvelope, CommandEnvelopeParts, CommandRejection, Event, EventRecipient,
    RejectReason, RoutedEvent,
};
use mornlea_protocol::{LoginStart, admit_login};
use mornlea_server::contracts::{
    PhaseReport, Resource, ServerError, ServerLimits, SessionKey, TickBudget,
};
use mornlea_server::core::command_outcome::{
    CommandDisposition, CommandProvider, CommandRejections, CommandResult, RejectionStage,
};
use mornlea_server::state::{AuthorityState, TickContext};

fn session(raw: u8) -> SessionKey {
    let mut authority = state();
    let mut last = None;
    for tag in 1..=raw {
        let mut id = [0; 16];
        id[0] = tag;
        id[6] = 0x40;
        id[8] = 0x80;
        let login = LoginStart::new(
            PlayerId::try_from_bytes(id).unwrap(),
            format!("peer{tag}"),
            8,
        )
        .unwrap();
        let admitted =
            admit_login(LoginStart::decode_inbound(&login.encode().unwrap()).unwrap()).unwrap();
        last = Some(
            authority
                .admit(admitted, mornlea_server::contracts::TransportKind::Memory)
                .unwrap(),
        );
    }
    let key = last.unwrap();
    assert_eq!(key.get(), u64::from(raw));
    key
}

fn envelope(session: u64, sequence: u64) -> CommandEnvelope {
    CommandEnvelope::try_new(CommandEnvelopeParts {
        tick: 7,
        session,
        sequence,
        arrival_index: 1,
        command: Command::CloseContainer,
    })
    .unwrap()
}

fn rejected(session: u64, sequence: u64, reason: RejectReason) -> RoutedEvent {
    RoutedEvent::new(
        EventRecipient::Session(session),
        Event::CommandRejected(CommandRejection::new(sequence, reason)),
    )
}

#[test]
fn phase_order_keeps_original_owner_sequence_and_reason() {
    let mut log = CommandRejections::try_new(3, 1).unwrap();
    log.record_command(
        &envelope(2, 9),
        RejectReason::InvalidSlot,
        RejectionStage::Settlement,
    )
    .unwrap();
    log.record_command(
        &envelope(1, 8),
        RejectReason::PlayerNotReady,
        RejectionStage::Admission,
    )
    .unwrap();
    log.record_held_input(session(1), 7, RejectReason::ProtectedBlock)
        .unwrap();
    log.record_command(
        &envelope(2, 6),
        RejectReason::NotArmor,
        RejectionStage::Admission,
    )
    .unwrap();
    assert_eq!(
        log.into_events(),
        vec![
            rejected(1, 8, RejectReason::PlayerNotReady),
            rejected(2, 6, RejectReason::NotArmor),
            rejected(2, 9, RejectReason::InvalidSlot),
            rejected(1, 7, RejectReason::ProtectedBlock),
        ]
    );
}

#[test]
fn command_and_held_capacity_are_independent_and_atomic() {
    let mut log = CommandRejections::try_new(2, 1).unwrap();
    log.record_command(
        &envelope(2, 9),
        RejectReason::InvalidSlot,
        RejectionStage::Settlement,
    )
    .unwrap();
    log.record_command(
        &envelope(1, 8),
        RejectReason::PlayerNotReady,
        RejectionStage::Admission,
    )
    .unwrap();
    assert_eq!(
        log.record_command(
            &envelope(1, 99),
            RejectReason::InvalidInput,
            RejectionStage::Admission
        ),
        Err(ServerError::Capacity {
            resource: Resource::Commands,
            limit: 2,
            observed: 3
        })
    );
    log.record_held_input(session(1), 7, RejectReason::DropCapacity)
        .unwrap();
    assert_eq!(
        log.record_held_input(session(2), 99, RejectReason::NoTarget),
        Err(ServerError::Capacity {
            resource: Resource::Players,
            limit: 1,
            observed: 2
        })
    );
    assert_eq!(
        log.into_events(),
        vec![
            rejected(1, 8, RejectReason::PlayerNotReady),
            rejected(2, 9, RejectReason::InvalidSlot),
            rejected(1, 7, RejectReason::DropCapacity)
        ]
    );
}

#[test]
fn idle_limit_keeps_zero_input_sequence_and_rejects_command() {
    let mut log = CommandRejections::try_new(0, 1).unwrap();
    log.record_held_input(session(1), 0, RejectReason::NoTarget)
        .unwrap();
    assert_eq!(
        log.record_command(
            &envelope(1, 1),
            RejectReason::InvalidInput,
            RejectionStage::Admission
        ),
        Err(ServerError::Capacity {
            resource: Resource::Commands,
            limit: 0,
            observed: 1
        })
    );
    assert_eq!(
        log.into_events(),
        vec![rejected(1, 0, RejectReason::NoTarget)]
    );
    assert_eq!(
        CommandRejections::try_new(4097, 1).err(),
        Some(ServerError::Capacity {
            resource: Resource::Commands,
            limit: 4096,
            observed: 4097
        })
    );
    assert_eq!(
        CommandRejections::try_new(0, 9).err(),
        Some(ServerError::Capacity {
            resource: Resource::Players,
            limit: 8,
            observed: 9
        })
    );
    assert_eq!(
        CommandRejections::try_new(0, 0).err(),
        Some(ServerError::InvalidInput {
            field: "rejection_players"
        })
    );
}

fn unowned(_: &mut TickContext<'_>, _: &CommandEnvelope) -> CommandResult {
    Ok(CommandDisposition::Unowned)
}
fn deferred(_: &mut TickContext<'_>, _: &CommandEnvelope) -> CommandResult {
    Ok(CommandDisposition::Settled(PhaseReport {
        examined: 1,
        applied: 0,
        carried: 1,
        rejected: 0,
    }))
}
fn refusal(_: &mut TickContext<'_>, _: &CommandEnvelope) -> CommandResult {
    Ok(CommandDisposition::Refused(RejectReason::NotArmor))
}
fn hard(_: &mut TickContext<'_>, _: &CommandEnvelope) -> CommandResult {
    Err(ServerError::Internal {
        invariant: "command outcome double",
    })
}
fn must_not_run(_: &mut TickContext<'_>, _: &CommandEnvelope) -> CommandResult {
    panic!("consumed outcome must stop provider fallthrough")
}

fn consume(
    context: &mut TickContext<'_>,
    envelope: &CommandEnvelope,
    providers: &[CommandProvider],
    log: &mut CommandRejections,
) -> Result<Option<PhaseReport>, ServerError> {
    for provider in providers {
        match provider(context, envelope)? {
            CommandDisposition::Unowned => {}
            CommandDisposition::Settled(report) => return Ok(Some(report)),
            CommandDisposition::Refused(reason) => {
                log.record_command(envelope, reason, RejectionStage::Admission)?;
                return Ok(None);
            }
        }
    }
    Ok(None)
}

fn state() -> AuthorityState {
    AuthorityState::try_new(ServerLimits::try_new(8, 4, 2, 1, 4, 1024).unwrap(), 7).unwrap()
}

#[test]
fn consumer_double_distinguishes_unowned_settled_and_refused() {
    let mut state = state();
    let mut context = TickContext::harness(&mut state, TickBudget::full());
    let mut log = CommandRejections::try_new(2, 1).unwrap();
    assert_eq!(
        consume(
            &mut context,
            &envelope(1, 4),
            &[unowned, deferred, must_not_run],
            &mut log
        ),
        Ok(Some(PhaseReport {
            examined: 1,
            applied: 0,
            carried: 1,
            rejected: 0
        }))
    );
    assert_eq!(
        consume(
            &mut context,
            &envelope(2, 5),
            &[unowned, refusal, must_not_run],
            &mut log
        ),
        Ok(None)
    );
    assert_eq!(
        log.into_events(),
        vec![rejected(2, 5, RejectReason::NotArmor)]
    );
    assert!(context.events().is_empty());
}

#[test]
fn consumer_double_propagates_hard_failure_without_wire() {
    let mut state = state();
    let mut context = TickContext::harness(&mut state, TickBudget::full());
    let mut log = CommandRejections::try_new(1, 1).unwrap();
    assert_eq!(
        consume(
            &mut context,
            &envelope(2, 5),
            &[unowned, hard, must_not_run],
            &mut log
        ),
        Err(ServerError::Internal {
            invariant: "command outcome double"
        })
    );
    assert!(log.into_events().is_empty());
    assert!(context.events().is_empty());
}
