//! Chat projection contract tests.
//!
//! The table pins the accepted chat semantics against the landed mirror
//! provider and the mapped Go `ChatEvents` oracle: a confirmed chat fact
//! projects exactly one record carrying the closed `ChatEvent` value the
//! authority published — the actual sender identity and name and the closed
//! body union verbatim, valid UTF-8 end to end — under the chat topic tag
//! with the actual observation identity the mirror retained for it. The
//! text bounds admit exactly and refuse plus one typed at the checked
//! domain constructors (name at most 32 scalars and 128 bytes, command 1024
//! bytes, speech 256 bytes), and invalid UTF-8 is refused typed at the
//! checked wire decoder before any domain value can exist. The window is the
//! pilot client's chat event ring: only a strictly newer event identity is
//! admitted, so a duplicate or stale resend does not double-emit and leaves
//! the projected output unchanged, and the projection exposes at most the
//! most recent 32 accepted events in source order — below the capacity
//! nothing is dropped, above it the oldest evict in order. A command
//! rejection is a sequence-bound outcome, not a chat fact, and projects
//! nowhere here; a reset epoch starts empty; an old-epoch observation or a
//! packet that is not a checked event publication rejects the whole
//! projection without partial output.

use mornlea_client_core::contracts::{
    ClientError, ClientLimits, ConfirmedRevision, FamilyOperation, ObservationKey, SessionEpoch,
};
use mornlea_client_core::input::InputProjectionState;
use mornlea_client_core::prediction::PlayerProjectionState;
use mornlea_client_core::presentation::frame::WorldUiRecord;
use mornlea_client_core::presentation::world_ui::chat::project_chat;
use mornlea_client_core::presentation::{
    AcceptedObservation, AudioProjectionState, DiagnosticProjectionState, ErrorClassCounters,
    LifecycleProjectionState, MovementIntent, OrderedRecord, Pose, ProducerIdentity,
    ProjectionOrder, ProjectionView, QueueCounters, StableRecordKey, WorldTopic, WorldUiView,
};
use mornlea_client_core::session::ConfirmedMirror;
use mornlea_client_core::session::mirror::MirrorProvider;
use mornlea_domain::{
    ChatBody, ChatEvent, ChatEventParts, CommandRejection, CommandText, CompanionId, CompanionName,
    CompanionSpeaker, DisplayName, DomainError, Event, PlayerId, RejectReason, SpeechText,
};
use mornlea_protocol::{KeepAlive, ProtocolError, ServerPacket};

/// The epoch every fixture mirror and observation key carries.
const EPOCH: u64 = 21;

/// The chat window capacity this projection must never exceed: the pilot
/// client's chat event ring keeps the most recent 32 confirmed events, the
/// value the measured client inventory freezes for the family.
const CHAT_RING_CAPACITY: usize = 32;

/// A checked UUIDv4 player identity whose raw byte order follows `first`, so
/// two fixture senders are distinct actual identities.
fn player_id(first: u8) -> PlayerId {
    let mut bytes = [0u8; 16];
    bytes[0] = first;
    bytes[6] = 0x40;
    bytes[8] = 0x80;
    PlayerId::try_from_bytes(bytes).expect("checked uuid v4 player identity")
}

/// A checked UUIDv4 companion identity for the body fixtures that name one.
fn companion_id(last: u8) -> CompanionId {
    let mut bytes = [0u8; 16];
    bytes[0] = 0x12;
    bytes[6] = 0x40;
    bytes[8] = 0x80;
    bytes[15] = last;
    CompanionId::try_from_bytes(bytes).expect("checked uuid v4 companion identity")
}

fn companion_name(text: &str) -> CompanionName {
    CompanionName::try_from_canonical(text.to_owned()).expect("canonical companion name")
}

fn command_text(text: &str) -> CommandText {
    CommandText::try_from_canonical(text.to_owned()).expect("canonical command text")
}

fn speech_text(text: &str) -> SpeechText {
    SpeechText::try_from_canonical(text.to_owned()).expect("canonical speech text")
}

/// One companion speaker, the shape every addressing-bearing body carries.
fn speaker(last: u8) -> CompanionSpeaker {
    CompanionSpeaker::new(companion_id(last), companion_name("阿木"))
}

/// One confirmed chat fact at the caller's identity, sender, canonical name
/// and closed body: the checked domain union member itself.
fn chat_fact(event_id: u64, sender: PlayerId, name: &str, body: ChatBody) -> ChatEvent {
    ChatEvent::try_new(ChatEventParts {
        event_id,
        player_id: sender,
        player_name: DisplayName::try_from_canonical(name.to_owned())
            .expect("canonical player name"),
        body,
    })
    .expect("checked chat event")
}

/// One accepted-addressing fact at the caller's identity, sender and command,
/// the Go oracle's accepted-chat shape.
fn accepted_fact(event_id: u64, sender: PlayerId, name: &str, command: &str) -> ChatEvent {
    chat_fact(
        event_id,
        sender,
        name,
        ChatBody::Accepted {
            companion: speaker(1),
            command: command_text(command),
        },
    )
}

/// One malformed-command rejection branch, the outcome that addresses no
/// companion and carries no restated command.
fn invalid_format_fact(event_id: u64, sender: PlayerId) -> ChatEvent {
    chat_fact(event_id, sender, "Chen", ChatBody::InvalidFormat)
}

/// The chat publication carrying one fact.
fn chat_event(fact: ChatEvent) -> Event {
    Event::Chat(fact)
}

/// One sequence-bound command rejection, the outcome that is not a chat fact
/// and belongs to its own crafting-owned family record.
fn command_rejected(sequence: u64) -> Event {
    Event::CommandRejected(CommandRejection::new(sequence, RejectReason::InvalidInput))
}

fn packet(event: Event) -> ServerPacket {
    ServerPacket::try_from(event).expect("publication converts to its packet")
}

/// An admitted real mirror provider with no observations yet.
fn admitted_mirror(epoch: u64) -> MirrorProvider {
    let epoch = SessionEpoch::try_new(epoch).expect("epoch");
    let mut provider =
        MirrorProvider::new(epoch, ClientLimits::try_new().expect("limits")).expect("mirror");
    provider.admit().expect("admitted session");
    provider
}

/// Commits one publication through the real mirror provider, so the retained
/// observation queue carries the actual source key of the chat fact.
fn commit(provider: &mut MirrorProvider, event: Event) -> ConfirmedRevision {
    let next = provider.mirror().revision().get() + 1;
    let key = ObservationKey::try_new(provider.mirror().epoch(), ConfirmedRevision::new(next), 0)
        .expect("staged key");
    let staged =
        AcceptedObservation::try_new(key, None, packet(event), Vec::new()).expect("staged");
    provider.commit(&staged).expect("committed observation")
}

/// The projection-view state owners a fixture keeps alive beside the
/// borrowed mirror and observation queue.
struct ViewFixture {
    input: InputProjectionState,
    player: PlayerProjectionState,
    audio: AudioProjectionState,
    lifecycle: LifecycleProjectionState,
    diagnostics: DiagnosticProjectionState,
    limits: ClientLimits,
}

impl ViewFixture {
    fn new(epoch: SessionEpoch, revision: ConfirmedRevision) -> Self {
        Self {
            input: InputProjectionState::try_new(1).expect("input projection state"),
            player: PlayerProjectionState::try_new(
                epoch,
                revision,
                Pose::try_new([0.0, 64.0, 0.0], 0.0, 0.0).expect("finite pose"),
                None,
                Vec::new(),
                None,
                None,
                MovementIntent::try_new(None, false).expect("movement intent"),
            )
            .expect("player projection state"),
            audio: AudioProjectionState::try_new().expect("audio projection state"),
            lifecycle: LifecycleProjectionState::try_new(1).expect("lifecycle projection state"),
            diagnostics: DiagnosticProjectionState::try_new(
                ProducerIdentity::try_new([0u8; 20], [0u8; 32]).expect("producer identity"),
                QueueCounters::default(),
                ErrorClassCounters::default(),
            )
            .expect("diagnostics projection state"),
            limits: ClientLimits::try_new().expect("limits"),
        }
    }

    /// Builds one immutable projection view over the supplied mirror and
    /// observation queue.
    fn view<'a>(
        &'a self,
        mirror: &'a ConfirmedMirror,
        observations: &'a [AcceptedObservation],
        epoch: SessionEpoch,
        revision: ConfirmedRevision,
    ) -> ProjectionView<'a> {
        ProjectionView::try_new(
            mirror,
            observations,
            &self.input,
            &self.player,
            &self.audio,
            &self.lifecycle,
            &self.diagnostics,
            epoch,
            revision,
            5,
            &self.limits,
        )
        .expect("projection view")
    }

    /// The candidate view of one provider's committed state: the next
    /// revision the coherent frame would carry.
    fn view_of<'a>(&'a self, provider: &'a MirrorProvider) -> ProjectionView<'a> {
        let revision = ConfirmedRevision::new(provider.mirror().revision().get() + 1);
        self.view(
            provider.mirror(),
            provider.observations(),
            provider.mirror().epoch(),
            revision,
        )
    }
}

/// The shared fixture owners; the per-view epoch and revision are supplied
/// when the view itself is built.
fn view_fixture() -> ViewFixture {
    ViewFixture::new(
        SessionEpoch::try_new(EPOCH).expect("epoch"),
        ConfirmedRevision::new(1),
    )
}

/// The chat payload of one record, refusing a foreign view tag: this
/// projection publishes the chat view alone.
fn chat_view(entry: &OrderedRecord<WorldUiRecord>) -> &ChatEvent {
    match entry.record().view() {
        WorldUiView::Chat(event) => event,
        other => panic!("chat record carries a foreign view tag: {other:?}"),
    }
}

/// The shared stable-key assertion: the chat topic tag beside the actual
/// observation identity the mirror retained for the carrying publication —
/// never a fabricated event id, generation or tick identity.
fn assert_stable_key(entry: &OrderedRecord<WorldUiRecord>, key: ObservationKey) {
    assert_eq!(
        *entry.stable_key(),
        StableRecordKey::World {
            topic: WorldTopic::Chat,
            identity: Some(key),
        }
    );
}

/// Asserts one entry is server-sourced with the exact observation identity
/// and packet record ordinal — one chat fact per publication, so the ordinal
/// is zero — never a reconstructed or local order.
fn assert_confirmed_order(entry: &OrderedRecord<WorldUiRecord>, revision: u64, ordinal: u32) {
    match entry.order() {
        ProjectionOrder::Confirmed {
            observation,
            record_ordinal,
        } => {
            assert_eq!(observation.epoch().get(), EPOCH);
            assert_eq!(observation.confirmed_revision().get(), revision);
            assert_eq!(*record_ordinal, ordinal);
        }
        other => panic!("chat record is server-sourced: {other:?}"),
    }
}

/// `valid UTF-8/actual sender ID`: two confirmed facts from two distinct
/// actual senders — an accepted addressing with the multibyte command the Go
/// oracle pins and a companion speech line — project exactly one verbatim
/// record each: the actual sender identity and canonical name and the closed
/// body union byte for byte, no source tick invented for the tickless family,
/// the rebased header revision, and the chat topic tag beside each fact's own
/// actual observation identity in actual source order.
#[test]
fn valid_utf8_and_actual_sender_id_project_verbatim() {
    let first_sender = player_id(0x90);
    let second_sender = player_id(0x91);
    let first = accepted_fact(1, first_sender, "Chen", "挖石头");
    let second = chat_fact(
        2,
        second_sender,
        "阿木",
        ChatBody::Speech {
            companion: speaker(2),
            text: speech_text("你好，旅行者"),
        },
    );
    let mut provider = admitted_mirror(EPOCH);
    commit(&mut provider, chat_event(first.clone()));
    commit(&mut provider, chat_event(second.clone()));

    let fixture = view_fixture();
    let view = fixture.view_of(&provider);
    let records = project_chat(&view).expect("two chat records project");
    assert_eq!(records.len(), 2);

    let projected_first = chat_view(&records[0]);
    assert_eq!(projected_first, &first, "the first fact verbatim");
    assert_eq!(projected_first.event_id(), 1);
    assert_eq!(
        projected_first.player_id(),
        first_sender,
        "the actual sender identity carried verbatim"
    );
    assert_eq!(projected_first.player_name().as_str(), "Chen");
    assert_eq!(
        projected_first.body(),
        &ChatBody::Accepted {
            companion: speaker(1),
            command: command_text("挖石头"),
        },
        "the closed body union verbatim, valid UTF-8 end to end"
    );

    let projected_second = chat_view(&records[1]);
    assert_eq!(projected_second, &second, "the second fact verbatim");
    assert_eq!(
        projected_second.player_id(),
        second_sender,
        "each record carries its own actual sender"
    );

    let first_key = *view.observations()[0].key();
    let second_key = *view.observations()[1].key();
    assert_ne!(first_key, second_key, "distinct facts, distinct identities");
    assert_stable_key(&records[0], first_key);
    assert_stable_key(&records[1], second_key);
    assert_confirmed_order(&records[0], 1, 0);
    assert_confirmed_order(&records[1], 2, 0);
    assert!(
        records[0].order() < records[1].order(),
        "actual source order"
    );

    for entry in &records {
        assert_eq!(entry.record().header().operation(), FamilyOperation::Upsert);
        assert_eq!(
            entry.record().header().source_tick(),
            None,
            "chat carries no server tick and none is invented"
        );
        assert_eq!(entry.record().header().epoch().get(), EPOCH);
        assert_eq!(
            entry.record().header().revision(),
            view.frame_revision(),
            "headers are rebased onto the coherent candidate revision"
        );
    }
}

/// `each accepted text cap plus one`: the player name admits exactly 32
/// scalars and the 128-byte corner those scalars bound, the speech line
/// admits exactly its 256-byte bound and the command its 1024-byte bound —
/// each at-cap value commits through the real mirror and projects verbatim —
/// while each plus-one refuses typed at the checked domain constructor
/// before any observation could exist, never clamped back into range.
#[test]
fn accepted_text_caps_admit_and_plus_one_rejects_typed() {
    let name_cap = "a".repeat(32);
    let name_plus_one = "a".repeat(33);
    let name_byte_cap = "𐀀".repeat(32);
    assert_eq!(name_byte_cap.len(), 128, "32 four-byte scalars");
    let speech_cap = "c".repeat(256);
    let speech_plus_one = "c".repeat(257);
    let command_cap = "b".repeat(1024);
    let command_plus_one = "b".repeat(1025);

    assert!(
        DisplayName::try_from_canonical(name_cap.clone()).is_ok(),
        "the 32-scalar name admits exactly"
    );
    assert!(
        DisplayName::try_from_canonical(name_byte_cap).is_ok(),
        "the 128-byte name corner admits exactly"
    );
    assert_eq!(
        DisplayName::try_from_canonical(name_plus_one),
        Err(DomainError::InvalidText),
        "one scalar above the name cap refuses typed"
    );
    assert!(
        SpeechText::try_from_canonical(speech_cap.clone()).is_ok(),
        "the speech cap admits exactly"
    );
    assert_eq!(
        SpeechText::try_from_canonical(speech_plus_one),
        Err(DomainError::InvalidText),
        "one byte above the speech cap refuses typed"
    );
    assert!(
        CommandText::try_from_canonical(command_cap.clone()).is_ok(),
        "the command cap admits exactly"
    );
    assert_eq!(
        CommandText::try_from_canonical(command_plus_one),
        Err(DomainError::InvalidText),
        "one byte above the command cap refuses typed"
    );

    let capped_name_sender = player_id(0x92);
    let at_cap = chat_fact(
        3,
        capped_name_sender,
        &name_cap,
        ChatBody::Accepted {
            companion: speaker(3),
            command: CommandText::try_from_canonical(command_cap).expect("canonical command"),
        },
    );
    let capped_speech = chat_fact(
        4,
        capped_name_sender,
        "Chen",
        ChatBody::Speech {
            companion: speaker(4),
            text: SpeechText::try_from_canonical(speech_cap).expect("canonical speech"),
        },
    );
    let mut provider = admitted_mirror(EPOCH);
    commit(&mut provider, chat_event(at_cap.clone()));
    commit(&mut provider, chat_event(capped_speech.clone()));

    let fixture = view_fixture();
    let records = project_chat(&fixture.view_of(&provider)).expect("at-cap facts project");
    assert_eq!(records.len(), 2);
    assert_eq!(
        chat_view(&records[0]),
        &at_cap,
        "the capped name and command verbatim"
    );
    assert_eq!(
        chat_view(&records[1]),
        &capped_speech,
        "the capped speech verbatim"
    );
}

/// Invalid UTF-8 is refused typed at the checked wire decoder: an encoded
/// chat fact whose name bytes are corrupted decodes to `InvalidString`, so no
/// wire record — and therefore no checked domain value, whose text slots take
/// only valid UTF-8 — can ever carry one, and the projection can never
/// observe an invalid fact upstream.
#[test]
fn invalid_utf8_is_refused_typed_at_the_checked_boundary() {
    let fact = accepted_fact(9, player_id(0x93), "Chen", "挖石头");
    let wire = mornlea_protocol::ChatEvent::try_from(fact).expect("wire record");
    let mut payload = wire.encode().expect("encoded chat event");
    let name = b"Chen";
    let position = payload
        .windows(name.len())
        .position(|window| window == name)
        .expect("encoded name bytes located");
    payload[position] = 0xFF;
    match mornlea_protocol::ChatEvent::decode(&payload) {
        Err(ProtocolError::InvalidString) => {}
        other => panic!("invalid UTF-8 must refuse typed at the wire boundary: {other:?}"),
    }
}

/// `duplicate event`: a resend of an already-accepted event identity and a
/// stale identity that is not newer than the last accepted one are committed
/// by the arrival-order mirror as real observations, yet the projection does
/// not double-emit either — the projected output is exactly the baseline —
/// and a strictly newer identity after both is admitted, the pilot ring's
/// last-accepted-id rule.
#[test]
fn duplicate_and_stale_events_do_not_double_emit() {
    let sender = player_id(0x94);
    let mut provider = admitted_mirror(EPOCH);
    commit(
        &mut provider,
        chat_event(accepted_fact(5, sender, "Chen", "挖石头")),
    );
    commit(
        &mut provider,
        chat_event(accepted_fact(8, sender, "Chen", "挖石头")),
    );

    let fixture = view_fixture();
    let baseline = project_chat(&fixture.view_of(&provider)).expect("baseline projects");
    assert_eq!(baseline.len(), 2);
    assert_eq!(chat_view(&baseline[0]).event_id(), 5);
    assert_eq!(chat_view(&baseline[1]).event_id(), 8);

    // The resends ride the malformed-format branch, the closed union member
    // that addresses no companion: the identity rule is body-independent.
    commit(&mut provider, chat_event(invalid_format_fact(8, sender)));
    commit(&mut provider, chat_event(invalid_format_fact(7, sender)));
    assert_eq!(
        provider.observations().len(),
        4,
        "the mirror accepted both resends; suppression is the projection's window rule"
    );
    let after = project_chat(&fixture.view_of(&provider)).expect("projection still succeeds");
    // The unchanged state is the windowed fact sequence with its actual
    // source identities: the duplicate and stale arrivals add no record.
    // Whole-record equality would also compare the rebased header revision,
    // which is each frame's coherent candidate revision by design and
    // advances because the mirror accepted the resends as real observations.
    let window = |records: &[OrderedRecord<WorldUiRecord>]| {
        records
            .iter()
            .map(|entry| {
                (
                    chat_view(entry).event_id(),
                    *entry.order(),
                    *entry.stable_key(),
                )
            })
            .collect::<Vec<_>>()
    };
    assert_eq!(
        window(&after),
        window(&baseline),
        "the duplicate and stale identities leave the projection unchanged"
    );

    commit(
        &mut provider,
        chat_event(accepted_fact(9, sender, "Chen", "挖石头")),
    );
    let resumed = project_chat(&fixture.view_of(&provider)).expect("newer identity projects");
    let ids: Vec<u64> = resumed
        .iter()
        .map(|entry| chat_view(entry).event_id())
        .collect();
    assert_eq!(ids, vec![5, 8, 9], "a strictly newer identity is admitted");
}

/// The bounded window: committing more facts than the pilot ring capacity
/// exposes exactly the most recent 32 in source order — the oldest evict in
/// order and no output exceeds the capacity — and a later fact slides the
/// window forward the same way.
#[test]
fn ring_keeps_the_latest_thirty_two_in_source_order() {
    let sender = player_id(0x95);
    let mut provider = admitted_mirror(EPOCH);
    for event_id in 1..=40u64 {
        commit(
            &mut provider,
            chat_event(accepted_fact(event_id, sender, "Chen", "挖石头")),
        );
    }

    let fixture = view_fixture();
    let view = fixture.view_of(&provider);
    let records = project_chat(&view).expect("the window projects");
    assert_eq!(records.len(), CHAT_RING_CAPACITY);
    let ids: Vec<u64> = records
        .iter()
        .map(|entry| chat_view(entry).event_id())
        .collect();
    let expected: Vec<u64> = (9..=40).collect();
    assert_eq!(ids, expected, "the most recent 32 in source order");

    for (index, entry) in records.iter().enumerate() {
        assert_stable_key(entry, *view.observations()[index + 8].key());
        if index > 0 {
            assert!(
                records[index - 1].order() < entry.order(),
                "actual source order inside the window"
            );
        }
    }
    let mut keys: Vec<StableRecordKey> = Vec::new();
    for entry in &records {
        if keys.contains(entry.stable_key()) {
            panic!("two windowed records share one stable identity");
        }
        keys.push(*entry.stable_key());
    }

    commit(
        &mut provider,
        chat_event(accepted_fact(41, sender, "Chen", "挖石头")),
    );
    let slid = project_chat(&fixture.view_of(&provider)).expect("the slid window projects");
    assert_eq!(slid.len(), CHAT_RING_CAPACITY);
    let slid_ids: Vec<u64> = slid
        .iter()
        .map(|entry| chat_view(entry).event_id())
        .collect();
    assert_eq!(
        slid_ids,
        (10..=41).collect::<Vec<u64>>(),
        "eviction in order"
    );
}

/// `rejection separate`: a sequence-bound command rejection between two chat
/// facts projects nowhere here — the crafting provider owns the family-wide
/// rejected record — and the chat facts themselves keep their actual source
/// order and identities.
#[test]
fn rejected_command_is_not_a_chat_event() {
    let sender = player_id(0x96);
    let mut provider = admitted_mirror(EPOCH);
    commit(
        &mut provider,
        chat_event(accepted_fact(1, sender, "Chen", "挖石头")),
    );
    commit(&mut provider, command_rejected(7));
    commit(
        &mut provider,
        chat_event(accepted_fact(2, sender, "Chen", "挖石头")),
    );

    let fixture = view_fixture();
    let view = fixture.view_of(&provider);
    let records = project_chat(&view).expect("the chat facts project");
    assert_eq!(
        records.len(),
        2,
        "the rejection is not a chat record and invents none"
    );
    assert_eq!(chat_view(&records[0]).event_id(), 1);
    assert_eq!(chat_view(&records[1]).event_id(), 2);
    assert_stable_key(&records[0], *view.observations()[0].key());
    assert_stable_key(&records[1], *view.observations()[2].key());
    assert_confirmed_order(&records[0], 1, 0);
    assert_confirmed_order(&records[1], 3, 0);
    assert!(records[0].order() < records[1].order());
}

/// `reset`: a fresh epoch's mirror carries no previous-session chat and
/// projects nothing, an observation from the old epoch rejects the whole
/// projection under the new frame epoch, a queue that mixes one valid
/// observation with an old-epoch one rejects without partial output, and a
/// packet that is not a checked event publication rejects whole the same way.
#[test]
fn reset_clears_projection_and_stale_inputs_reject_whole() {
    let sender = player_id(0x97);
    let mut provider = admitted_mirror(EPOCH);
    commit(
        &mut provider,
        chat_event(accepted_fact(1, sender, "Chen", "挖石头")),
    );

    let next_epoch_value = EPOCH + 1;
    let reset_provider = admitted_mirror(next_epoch_value);
    let fixture = view_fixture();
    assert!(
        project_chat(&fixture.view_of(&reset_provider))
            .expect("empty queue projects")
            .is_empty(),
        "a reset epoch carries no previous-session chat"
    );

    let next_epoch = SessionEpoch::try_new(next_epoch_value).expect("next epoch");
    let old_epoch_observation = provider.observations()[0].clone();
    assert_eq!(
        project_chat(&fixture.view(
            reset_provider.mirror(),
            &[old_epoch_observation.clone()],
            next_epoch,
            ConfirmedRevision::new(1)
        )),
        Err(ClientError::StaleEpoch),
        "an old-epoch observation never enters the new epoch's frame"
    );

    let mut fresh = admitted_mirror(next_epoch_value);
    commit(
        &mut fresh,
        chat_event(accepted_fact(2, sender, "Chen", "挖石头")),
    );
    let mixed = vec![fresh.observations()[0].clone(), old_epoch_observation];
    assert_eq!(
        project_chat(&fixture.view(
            fresh.mirror(),
            &mixed,
            next_epoch,
            ConfirmedRevision::new(2)
        )),
        Err(ClientError::StaleEpoch),
        "a mixed queue rejects whole, never a partial prefix"
    );

    let non_event = AcceptedObservation::try_new(
        ObservationKey::try_new(next_epoch, ConfirmedRevision::new(2), 0).expect("key"),
        None,
        ServerPacket::KeepAlive(KeepAlive::new(1).expect("checked keep-alive token")),
        Vec::new(),
    )
    .expect("staged with a non-publication packet");
    let with_non_event = vec![fresh.observations()[0].clone(), non_event];
    assert_eq!(
        project_chat(&fixture.view(
            fresh.mirror(),
            &with_non_event,
            next_epoch,
            ConfirmedRevision::new(2)
        )),
        Err(ClientError::InvalidInput),
        "a packet that is not an event publication rejects without a partial prefix"
    );
}
