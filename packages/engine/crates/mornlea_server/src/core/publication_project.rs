//! The per-tick publication projection.
//!
//! This module owns the tick-end pass that constructs the visibility and
//! record event families the private player observation does not cover: chunk
//! interest (forget, snapshot, deltas), remote players, companions, mobs,
//! projectiles, item drops, chat addressing, and the owner-only record
//! states. It runs after the viewer and resident commits and before the
//! existing [`Event::PlayerState`] projection, so a session's tick output
//! reads world families first and its private observation last.

use std::collections::{BTreeMap, BTreeSet};

use mornlea_domain::{
    self, BlockChange, BlockChanges, BlockChangesParts, BlockPos, ChatBody, ChatEvent,
    ChatEventParts, ChunkPos, CommandText, CompanionDespawn, CompanionId, CompanionName,
    CompanionSpawn, CompanionSpawnParts, CompanionSpeaker, CompanionState, CompanionStateParts,
    CompanionStates, CompanionStatesParts, ContainerClosed, CraftingState, CraftingStateParts,
    Dimension, Event, EventRecipient, ForgetChunks, ForgetChunksParts, HostileDespawn,
    HostileDespawnParts, HostileId, HostileKind, HostileSpawn, HostileSpawnParts,
    HostileSpawnRecord, HostileSpawnRecordParts, HostileState, HostileStateParts,
    HostileStateRecord, HostileStateRecordParts, ItemDrop, ItemDropParts, ItemDropRemoves,
    ItemDropRemovesParts, ItemDropUpserts, ItemDropUpsertsParts, ItemStack, PassiveDespawn,
    PassiveDespawnParts, PassiveDespawnReason, PassiveDespawnRecord, PassiveId, PassiveSpawn,
    PassiveSpawnParts, PassiveSpawnRecord, PassiveSpawnRecordParts, PassiveState,
    PassiveStateParts, PassiveStateRecord, PassiveStateRecordParts, PlayerId, ProjectileDespawn,
    ProjectileDespawnParts, ProjectileId, ProjectileSpawn, ProjectileSpawnParts,
    ProjectileSpawnRecord, ProjectileSpawnRecordParts, ProjectileState, ProjectileStateParts,
    ProjectileStateRecord, ProjectileStateRecordParts, RemotePlayerDespawn, RemotePlayerSpawn,
    RemotePlayerSpawnParts, RemotePlayerState, RemotePlayerStateParts, RemotePlayerStates,
    RemotePlayerStatesParts, RoutedEvent,
};

use crate::contracts::{
    ActorBody, ActorKey, ActorLifecycle, ActorRecord, ChunkKey, ContainerSlots, DropRecord,
    InventoryRecord, ProjectileRecord, SessionKey, ViewLease,
};
use crate::core::session_view::SessionView;
use crate::state::{AuthorityState, Speaker};

/// Wire record caps the outbound protocol conversion enforces per packet.
/// The projection pre-splits every batch at these bounds so a publication can
/// never fail at encode time.
const REMOTE_STATES_CAP: usize = 7;
const COMPANION_STATES_CAP: usize = 4;
const MOB_BATCH_CAP: usize = 64;
const PROJECTILE_BATCH_CAP: usize = 128;
const DROP_BATCH_CAP: usize = 32;
const BLOCK_CHANGES_CAP: usize = 4_096;
const FORGET_CHUNKS_CAP: usize = 4_096;

/// Interest radius around each observer's foot chunk, matching the shared
/// active-key derivation the world providers use.
const INTEREST_RADIUS: i32 = 2;

/// The per-tick captured facts the projection consumes beside the committed
/// residents: block batches grouped per chunk and the provider resync lane.
///
/// Block batches carry `(chunk, base_revision, new_revision, changes)` with
/// the changes ordered by strictly increasing chunk block index; only chunks
/// whose revision advances this tick enter the list.
pub(crate) struct TickOutcome {
    pub(crate) block_batches: Vec<(ChunkKey, u64, u64, Vec<BlockChange>)>,
    pub(crate) resyncs: Vec<(SessionKey, Dimension, ChunkPos)>,
    /// Tick-local quiet passive removals drained from the tick context:
    /// identities the movement rule terminated below the world floor this
    /// tick. Death settlement never appears here.
    pub(crate) quiet_passive_removals: BTreeSet<PassiveId>,
}

/// The shared read-only world facts the per-family emitters consume.
struct WorldInputs<'a> {
    actors: &'a [ActorRecord],
    entities: &'a Entities,
    speakers: &'a [Speaker],
    tick: u64,
    /// Resident death-settlement passive identities for this tick, derived
    /// once per tick with quiet fall-out removals excluded. Death identity
    /// is observer-independent: a death wins over a simultaneous view exit.
    passive_deaths: &'a BTreeSet<PassiveId>,
}

/// One observer's derived projection inputs for this tick.
struct Observer {
    session: SessionKey,
    /// Radius-two interest around the actor's foot chunk, empty without an
    /// Active player actor.
    wanted: BTreeSet<ChunkKey>,
    /// Whether the session has an Active player actor this tick.
    has_actor: bool,
}

/// The classified resident entity sets the visibility families diff against.
struct Entities {
    /// Active player actor indices.
    players: Vec<usize>,
    /// Active overworld companions by ascending identity.
    companions: Vec<(CompanionId, usize)>,
    /// Active overworld hostiles by ascending identity.
    hostiles: Vec<(HostileId, HostileKind, usize)>,
    /// Active overworld passives by ascending identity.
    passives: Vec<(PassiveId, usize)>,
}

/// One observer's new visible identity sets for this tick.
struct Visibility {
    remotes: BTreeSet<PlayerId>,
    companions: BTreeSet<CompanionId>,
    hostiles: BTreeSet<HostileId>,
    passives: BTreeSet<PassiveId>,
    projectiles: BTreeSet<ProjectileId>,
    drops: BTreeSet<mornlea_domain::DropId>,
}

/// Derives the deterministic published name of one companion identity.
///
/// The authority owns no persisted companion name, so every publication
/// address derives one: `companion-` plus the lowercase hex of the identity's
/// first eight bytes. The form is fixed-length, canonical (no whitespace, no
/// control character, well inside the thirty-two scalar cap) and stable for
/// one identity across ticks.
pub(crate) fn companion_display_name(id: CompanionId) -> Option<CompanionName> {
    const PREFIX: &str = "companion-";
    let mut text = String::with_capacity(PREFIX.len() + 16);
    text.push_str(PREFIX);
    for byte in &id.bytes()[..8] {
        text.push_str(&format!("{byte:02x}"));
    }
    CompanionName::try_from_canonical(text).ok()
}

impl AuthorityState {
    /// Projects one tick's publication families for every admitted session.
    ///
    /// The family order inside one tick mirrors the Go authority's per-session
    /// publication order and is frozen: remote and companion despawns, forget
    /// batches, snapshots (a session's resync-flagged columns first, then its
    /// first sends), block deltas, companion spawns and states, remote spawns
    /// and states, the hostile, projectile and passive families, item drops,
    /// chat, then the owner-only record states. Families are grouped per
    /// session in ascending session order, and within one family a session's
    /// events follow the sub-order above. Provider-acknowledged events keep
    /// their existing positions instead: `PlaceBlockSucceeded` and
    /// `CommandRejected` are staged by their providers during dispatch, and
    /// `PlayerState` and `CombatHit` follow this projection unchanged.
    ///
    /// Companion despawns are interest-exit diffs alone: this slice has no
    /// companion death path, so a companion leaves a mirror only by leaving
    /// the observer's wanted columns. Passive despawns publish the died
    /// reason for resident death-settlement removals (even under a
    /// simultaneous interest exit) and the vanished reason for live view
    /// exits, missing actors and quiet movement terminations. An
    /// entity whose checked record cannot be constructed is not published and
    /// stays out of the session's visible set, so the next tick retries its
    /// spawn instead of failing the tick. With no active sessions the
    /// projection publishes nothing.
    pub(crate) fn project_tick_publication(
        &mut self,
        tick: u64,
        outcome: &TickOutcome,
    ) -> Vec<RoutedEvent> {
        let speakers = self.active_speakers();
        if speakers.is_empty() {
            return Vec::new();
        }
        let actors = self.resident_actors().to_vec();
        let entities = classify_entities(&actors);
        let inventories = self.resident_inventories().clone();
        let projectiles = self.resident_projectiles().to_vec();
        let drops = self.resident_drop_records();
        let mut views = self.take_session_views();
        let mut observers = Vec::with_capacity(speakers.len());
        let mut view_list: Vec<SessionView> = Vec::with_capacity(speakers.len());
        for speaker in &speakers {
            observers.push(observer_of(&actors, speaker, &entities));
            view_list.push(views.remove(&speaker.session).unwrap_or_default());
        }
        let visibilities: Vec<Visibility> = observers
            .iter()
            .map(|observer| {
                visibility_of(
                    observer,
                    &actors,
                    &entities,
                    &speakers,
                    &projectiles,
                    &drops,
                )
            })
            .collect();
        apply_resync_requests(&mut view_list, &observers, self, outcome);
        // Resident Dead passive identities once per tick, minus the quiet
        // fall-out removals: death is observer-independent, so Died wins a
        // simultaneous death plus interest or dimension exit, while live
        // view exits, missing actors and quiet movement terminations stay
        // Vanished.
        let mut passive_deaths = BTreeSet::new();
        for actor in &actors {
            if actor.lifecycle != ActorLifecycle::Dead {
                continue;
            }
            if let ActorKey::Passive(id) = actor.key
                && !outcome.quiet_passive_removals.contains(&id)
            {
                passive_deaths.insert(id);
            }
        }
        let inputs = WorldInputs {
            actors: &actors,
            entities: &entities,
            speakers: &speakers,
            tick,
            passive_deaths: &passive_deaths,
        };
        let mut events = Vec::new();
        emit_despawns(&mut view_list, &observers, &visibilities, &mut events);
        emit_forgets(&mut view_list, &observers, &mut events);
        emit_snapshots(&mut view_list, &observers, self, &mut events);
        emit_block_batches(&mut view_list, &observers, self, outcome, &mut events);
        emit_companions(
            &mut view_list,
            &observers,
            &visibilities,
            &inputs,
            &mut events,
        );
        emit_remotes(
            &mut view_list,
            &observers,
            &visibilities,
            &inputs,
            &mut events,
        );
        emit_hostiles(
            &mut view_list,
            &observers,
            &visibilities,
            &inputs,
            &mut events,
        );
        emit_projectiles(
            &mut view_list,
            &observers,
            &visibilities,
            &projectiles,
            tick,
            &mut events,
        );
        emit_passives(
            &mut view_list,
            &observers,
            &visibilities,
            &inputs,
            self,
            &mut events,
        );
        emit_drops(
            &mut view_list,
            &observers,
            &visibilities,
            &drops,
            tick,
            &mut events,
        );
        self.emit_chat(&entities, &mut events);
        self.emit_records(&observers, &inventories, &mut view_list, &mut events);
        for (observer, mut view) in observers.iter().zip(view_list) {
            view.wanted.clone_from(&observer.wanted);
            view.chunks.retain(|key, _| observer.wanted.contains(key));
            for key in &observer.wanted {
                view.chunks.entry(*key).or_default();
            }
            views.insert(observer.session, view);
        }
        self.restore_session_views(views);
        events
    }

    /// Chat: drains the staged chat FIFO and emits one chat event per entry.
    ///
    /// Malformed addressing and an unknown derived name reject to the sender
    /// only, a staged overflow marker rejects queue-full to the sender only,
    /// and valid addressing broadcasts the accepted command. Every published
    /// event consumes one strictly increasing event id; entries for retired
    /// senders are dropped without an id.
    fn emit_chat(&mut self, entities: &Entities, events: &mut Vec<RoutedEvent>) {
        let staged = self.take_chat_queue();
        if staged.is_empty() {
            return;
        }
        let mut names: BTreeMap<String, CompanionId> = BTreeMap::new();
        for (id, _) in &entities.companions {
            if let Some(name) = companion_display_name(*id) {
                names.entry(name.as_str().to_owned()).or_insert(*id);
            }
        }
        for chat in staged {
            if !self.session_active(chat.session) {
                continue;
            }
            // One parse per entry decides both the body and the destination:
            // only a normally staged match broadcasts, while the queue-full
            // marker rejects to the sender even though it addressed fine.
            let (body, broadcast) = match parse_chat_address(chat.text.as_str(), &names) {
                Addressed::Invalid => (ChatBody::InvalidFormat, false),
                Addressed::Unknown { name } => (ChatBody::UnknownCompanion { name }, false),
                Addressed::Matched { id, name, command } => {
                    let companion = CompanionSpeaker::new(id, name);
                    if chat.overflow {
                        (ChatBody::QueueFull { companion, command }, false)
                    } else {
                        (ChatBody::Accepted { companion, command }, true)
                    }
                }
            };
            let Some(event_id) = self.allocate_chat_event_id() else {
                break;
            };
            let Ok(event) = ChatEvent::try_new(ChatEventParts {
                event_id,
                player_id: chat.player_id,
                player_name: chat.display_name,
                body,
            })
            .map(Event::Chat) else {
                continue;
            };
            let recipient = if broadcast {
                EventRecipient::Broadcast
            } else {
                EventRecipient::Session(chat.session.get())
            };
            events.push(RoutedEvent::new(recipient, event));
        }
    }

    /// The owner-only record families: inventory state, the viewed chest
    /// state, the container-closed notice, the crafting state and the viewed
    /// furnace state, in exactly that per-session order. Every family diffs
    /// against the last published snapshot stored on the session view, so an
    /// unchanged record and a refused command publish nothing. A session
    /// holds at most one container lease, so the chest and furnace slots of
    /// the order never compete: the chest publishes before the close notice
    /// and the grid, and the furnace after the grid.
    fn emit_records(
        &mut self,
        observers: &[Observer],
        inventories: &BTreeMap<ActorKey, InventoryRecord>,
        view_list: &mut [SessionView],
        events: &mut Vec<RoutedEvent>,
    ) {
        for (observer, view) in observers.iter().zip(view_list.iter_mut()) {
            let actor = ActorKey::Player(observer.session);
            let owner = EventRecipient::Session(observer.session.get());
            let record = inventories.get(&actor).copied();
            // The inventory state comes first.
            if let Some(record) = record
                && view.last_inventory.as_ref() != Some(&record)
            {
                events.push(RoutedEvent::new(
                    owner,
                    Event::InventoryState(inventory_event(&record)),
                ));
                view.last_inventory = Some(record);
            }
            // The viewed chest publishes next; a furnace defers until after
            // the crafting grid so the frozen order holds for both kinds.
            let lease = self.committed_lease(observer.session);
            let mut deferred_furnace = None;
            if let Some(reference) = lease.map(ViewLease::reference) {
                if let Some(held) = self.resident_container(reference)
                    && view.leased_containers.get(&reference) != Some(&held)
                    && let Some(event) = container_event(reference, &held.slots)
                {
                    match event {
                        Event::FurnaceState(_) => deferred_furnace = Some(event),
                        chest => events.push(RoutedEvent::new(owner, chest)),
                    }
                    view.leased_containers.insert(reference, held);
                }
            } else {
                view.leased_containers.clear();
            }
            // The close notice follows the record states of the same tick.
            if let Some(released) = view.last_lease.filter(|_| lease.is_none()) {
                events.push(RoutedEvent::new(
                    owner,
                    Event::ContainerClosed(ContainerClosed::new(released.reference())),
                ));
            }
            view.last_lease = lease;
            // The crafting grid follows the close notice, with the matched
            // output from the frozen recipe matcher the take provider owns.
            if let Some(record) = record {
                let crafting = (record.crafting, record.crafting_size);
                if view.last_crafting.as_ref() != Some(&crafting) {
                    let extent = crate::rules::crafting::grid_extent(record.crafting_size);
                    let matched = crate::rules::crafting::match_grid(extent, &record.crafting)
                        .map(|(_, output)| domain_stack(output))
                        .unwrap_or(ItemStack::EMPTY);
                    if let Ok(state) = CraftingState::try_new(CraftingStateParts {
                        size: record.crafting_size,
                        slots: domain_stacks(&record.crafting),
                        output: matched,
                    }) {
                        events.push(RoutedEvent::new(owner, Event::CraftingState(state)));
                        view.last_crafting = Some(crafting);
                    }
                }
            }
            // The viewed furnace closes the record families.
            if let Some(event) = deferred_furnace {
                events.push(RoutedEvent::new(owner, event));
            }
        }
    }
}

/// One staged chat text's addressing outcome before the queue-full branch.
#[derive(Debug)]
enum Addressed {
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

/// Parses one staged chat text against the live companion names.
///
/// The format is `@<name><separator><command>`: the name runs to the first
/// whitespace separator, the remainder must re-validate as a `CommandText`,
/// and every malformed shape (no `@`, no separator, an empty or invalid
/// remainder, or a name no canonical companion name accepts) rejects as
/// invalid format. A well-formed name that matches no live companion rejects
/// as unknown. This projection owns the addressing outcome only: feeding the
/// companion task queue is the separately-owned sessionless
/// `CompanionIngress` path, which this slice deliberately does not drive
/// from chat.
fn parse_chat_address(text: &str, names: &BTreeMap<String, CompanionId>) -> Addressed {
    let Some(rest) = text.strip_prefix('@') else {
        return Addressed::Invalid;
    };
    let Some(separator) = rest.find(char::is_whitespace) else {
        return Addressed::Invalid;
    };
    let (name, remainder) = rest.split_at(separator);
    let remainder = remainder.trim_start_matches(char::is_whitespace);
    if CommandText::try_from_canonical(remainder.to_owned()).is_err() {
        return Addressed::Invalid;
    }
    let Ok(name) = CompanionName::try_from_canonical(name.to_owned()) else {
        return Addressed::Invalid;
    };
    let Ok(command) = CommandText::try_from_canonical(remainder.to_owned()) else {
        return Addressed::Invalid;
    };
    match names.get(name.as_str()) {
        Some(&id) => Addressed::Matched { id, name, command },
        None => Addressed::Unknown { name },
    }
}

/// One observer's inputs: identity plus interest derived from its player
/// actor. The session view keeps the previous tick's wanted set until the
/// closing commit, so the forget diff can read it.
fn observer_of(actors: &[ActorRecord], speaker: &Speaker, entities: &Entities) -> Observer {
    let has_actor = entities.players.iter().any(|&index| {
        actors[index].key == ActorKey::Player(speaker.session)
            && actors[index].lifecycle == ActorLifecycle::Active
    });
    let wanted = entities
        .players
        .iter()
        .copied()
        .find(|&index| actors[index].key == ActorKey::Player(speaker.session))
        .filter(|&index| actors[index].lifecycle == ActorLifecycle::Active)
        .map(|index| wanted_columns(&actors[index]))
        .unwrap_or_default();
    Observer {
        session: speaker.session,
        wanted,
        has_actor,
    }
}

/// The radius-two chunk columns around one actor's foot position in its own
/// dimension, reusing the derivation the shared active-key set applies.
fn wanted_columns(actor: &ActorRecord) -> BTreeSet<ChunkKey> {
    let position = actor.motion.position().get();
    let center_x = (position[0].floor() as i32) >> 4;
    let center_z = (position[2].floor() as i32) >> 4;
    let mut keys = BTreeSet::new();
    for dz in -INTEREST_RADIUS..=INTEREST_RADIUS {
        for dx in -INTEREST_RADIUS..=INTEREST_RADIUS {
            keys.insert(ChunkKey {
                dimension: actor.dimension,
                pos: ChunkPos::new(center_x.saturating_add(dx), center_z.saturating_add(dz)),
            });
        }
    }
    keys
}

/// The chunk column one world position sits in.
fn position_chunk(dimension: Dimension, position: [f32; 3]) -> ChunkKey {
    ChunkKey {
        dimension,
        pos: ChunkPos::new(
            (position[0].floor() as i32) >> 4,
            (position[2].floor() as i32) >> 4,
        ),
    }
}

/// Classifies the resident actors into the entity families the visibility
/// diff consumes. Every non-player family is ascending by identity and
/// restricted to the dimensions its wire records admit.
fn classify_entities(actors: &[ActorRecord]) -> Entities {
    let mut entities = Entities {
        players: Vec::new(),
        companions: Vec::new(),
        hostiles: Vec::new(),
        passives: Vec::new(),
    };
    for (index, actor) in actors.iter().enumerate() {
        if actor.lifecycle != ActorLifecycle::Active {
            continue;
        }
        match actor.key {
            ActorKey::Player(_) => entities.players.push(index),
            ActorKey::Companion(id) if actor.dimension == Dimension::OVERWORLD => {
                entities.companions.push((id, index));
            }
            ActorKey::Hostile(id) if actor.dimension == Dimension::OVERWORLD => {
                if let ActorBody::Hostile(body) = &actor.body
                    && let Some(kind) = hostile_kind_of(body.kind)
                {
                    entities.hostiles.push((id, kind, index));
                }
            }
            ActorKey::Passive(id) if actor.dimension == Dimension::OVERWORLD => {
                entities.passives.push((id, index));
            }
            ActorKey::Companion(_) | ActorKey::Hostile(_) | ActorKey::Passive(_) => {}
        }
    }
    entities.companions.sort_by_key(|(id, _)| *id);
    entities.hostiles.sort_by_key(|(id, _, _)| *id);
    entities.passives.sort_by_key(|(id, _)| *id);
    entities
}

/// Maps the storage hostile kind byte onto the closed domain kind through
/// the shared kind constants the hostile provider owns.
fn hostile_kind_of(kind: u8) -> Option<HostileKind> {
    match kind {
        crate::rules::hostile_actors::NIGHTWALKER => Some(HostileKind::Nightwalker),
        crate::rules::hostile_actors::HURLER => Some(HostileKind::BoneThrower),
        _ => None,
    }
}

/// One observer's new visible sets: every entity whose foot column is inside
/// the observer's interest, and every projectile or drop inside it.
#[allow(clippy::too_many_arguments)]
fn visibility_of(
    observer: &Observer,
    actors: &[ActorRecord],
    entities: &Entities,
    speakers: &[Speaker],
    projectiles: &[ProjectileRecord],
    drops: &[DropRecord],
) -> Visibility {
    let mut visibility = Visibility {
        remotes: BTreeSet::new(),
        companions: BTreeSet::new(),
        hostiles: BTreeSet::new(),
        passives: BTreeSet::new(),
        projectiles: BTreeSet::new(),
        drops: BTreeSet::new(),
    };
    if !observer.has_actor {
        return visibility;
    }
    for &index in &entities.players {
        let actor = &actors[index];
        let ActorKey::Player(target) = actor.key else {
            continue;
        };
        if target == observer.session {
            continue;
        }
        if actor.dimension == observer_dimension(actors, observer)
            && observer.wanted.contains(&position_chunk(
                actor.dimension,
                actor.motion.position().get(),
            ))
            && let Some(speaker) = speakers.iter().find(|speaker| speaker.session == target)
        {
            visibility.remotes.insert(speaker.player_id);
        }
    }
    for (id, index) in &entities.companions {
        if observer.wanted.contains(&position_chunk(
            actors[*index].dimension,
            actors[*index].motion.position().get(),
        )) {
            visibility.companions.insert(*id);
        }
    }
    for (id, _, index) in &entities.hostiles {
        if observer.wanted.contains(&position_chunk(
            actors[*index].dimension,
            actors[*index].motion.position().get(),
        )) {
            visibility.hostiles.insert(*id);
        }
    }
    for (id, index) in &entities.passives {
        if observer.wanted.contains(&position_chunk(
            actors[*index].dimension,
            actors[*index].motion.position().get(),
        )) {
            visibility.passives.insert(*id);
        }
    }
    for record in projectiles {
        if observer
            .wanted
            .contains(&position_chunk(record.dimension, record.position.get()))
        {
            visibility.projectiles.insert(record.id);
        }
    }
    for record in drops {
        if let Some(dimension) = u8::try_from(record.id.dimension())
            .ok()
            .and_then(|raw| Dimension::new(raw).ok())
            && observer.wanted.contains(&ChunkKey {
                dimension,
                pos: record.id.chunk(),
            })
        {
            visibility.drops.insert(record.id);
        }
    }
    visibility
}

/// The observer's own actor dimension, overworld when the actor is missing.
fn observer_dimension(actors: &[ActorRecord], observer: &Observer) -> Dimension {
    actors
        .iter()
        .find(|actor| actor.key == ActorKey::Player(observer.session))
        .map_or(Dimension::OVERWORLD, |actor| actor.dimension)
}

/// Records the provider resync requests that name a wanted, Ready column.
/// `HaveRevision` is deliberately ignored, mirroring the provider leg: the
/// reply is always the full current snapshot. Other requests are silently
/// dropped.
fn apply_resync_requests(
    view_list: &mut [SessionView],
    observers: &[Observer],
    state: &AuthorityState,
    outcome: &TickOutcome,
) {
    for (session, dimension, chunk) in &outcome.resyncs {
        let Some(position) = observers
            .iter()
            .position(|observer| observer.session == *session)
        else {
            continue;
        };
        let key = ChunkKey {
            dimension: *dimension,
            pos: *chunk,
        };
        if !observers[position].wanted.contains(&key)
            || state.ready_chunk_current_revision(key).is_none()
        {
            continue;
        }
        view_list[position]
            .chunks
            .entry(key)
            .or_default()
            .resync_queued = true;
    }
}

/// Remote-player and companion despawn diffs. Both families
/// are pure interest exits, so an entity that merely failed to publish is not
/// despawned. Every family emitter stores this tick's visible set back on the
/// session view, so a departure publishes its despawn or removal exactly
/// once, until the entity becomes visible again and correctly re-spawns.
fn emit_despawns(
    view_list: &mut [SessionView],
    observers: &[Observer],
    visibilities: &[Visibility],
    events: &mut Vec<RoutedEvent>,
) {
    for ((observer, view), visibility) in
        observers.iter().zip(view_list.iter_mut()).zip(visibilities)
    {
        for id in view.visible_remotes.difference(&visibility.remotes) {
            events.push(RoutedEvent::new(
                EventRecipient::Session(observer.session.get()),
                Event::RemotePlayerDespawn(RemotePlayerDespawn::new(*id)),
            ));
        }
        for id in view.visible_companions.difference(&visibility.companions) {
            events.push(RoutedEvent::new(
                EventRecipient::Session(observer.session.get()),
                Event::CompanionDespawn(CompanionDespawn::new(*id)),
            ));
        }
    }
}

/// One forget batch per dimension per session for the columns that left
/// the interest set, chunks sorted x-then-z and split at the wire cap.
fn emit_forgets(
    view_list: &mut [SessionView],
    observers: &[Observer],
    events: &mut Vec<RoutedEvent>,
) {
    for (observer, view) in observers.iter().zip(view_list.iter_mut()) {
        let mut by_dimension: BTreeMap<Dimension, Vec<ChunkPos>> = BTreeMap::new();
        for key in &view.wanted {
            if !observer.wanted.contains(key) {
                by_dimension.entry(key.dimension).or_default().push(key.pos);
            }
        }
        for (dimension, mut columns) in by_dimension {
            columns.sort();
            for group in columns.chunks(FORGET_CHUNKS_CAP) {
                if let Ok(batch) = ForgetChunks::try_new(ForgetChunksParts {
                    dimension,
                    chunks: group.to_vec().into_boxed_slice(),
                }) {
                    events.push(RoutedEvent::new(
                        EventRecipient::Session(observer.session.get()),
                        Event::ForgetChunks(batch),
                    ));
                }
            }
        }
    }
}

/// This session's resync-flagged snapshots first, then its first sends,
/// both ascending by chunk key. Columns that are not Ready stay unsent.
fn emit_snapshots(
    view_list: &mut [SessionView],
    observers: &[Observer],
    state: &AuthorityState,
    events: &mut Vec<RoutedEvent>,
) {
    for (observer, view) in observers.iter().zip(view_list.iter_mut()) {
        let flagged: Vec<ChunkKey> = view
            .chunks
            .iter()
            .filter(|(key, entry)| entry.resync_queued && observer.wanted.contains(key))
            .map(|(key, _)| *key)
            .collect();
        for key in flagged {
            if let Some(entry) = view.chunks.get_mut(&key) {
                entry.resync_queued = false;
                if let Some(snapshot) = state.chunk_snapshot_event(key) {
                    entry.snapshot_sent = true;
                    entry.last_revision = snapshot.revision();
                    events.push(RoutedEvent::new(
                        EventRecipient::Session(observer.session.get()),
                        Event::ChunkSnapshot(snapshot),
                    ));
                }
            }
        }
        // One ascending pass over the wanted columns whose entry is missing
        // or still unsent, so retained pending columns and newly wanted ones
        // publish together in chunk-key order.
        let pending: Vec<ChunkKey> = observer
            .wanted
            .iter()
            .copied()
            .filter(|key| {
                !view
                    .chunks
                    .get(key)
                    .is_some_and(|entry| entry.snapshot_sent)
            })
            .collect();
        for key in pending {
            let Some(snapshot) = state.chunk_snapshot_event(key) else {
                continue;
            };
            let entry = view.chunks.entry(key).or_default();
            entry.snapshot_sent = true;
            entry.last_revision = snapshot.revision();
            events.push(RoutedEvent::new(
                EventRecipient::Session(observer.session.get()),
                Event::ChunkSnapshot(snapshot),
            ));
        }
    }
}

/// Per-session contiguous block deltas in ascending chunk order. A
/// revision gap re-sends the full snapshot at the new revision instead of a
/// delta the mirror cannot apply.
fn emit_block_batches(
    view_list: &mut [SessionView],
    observers: &[Observer],
    state: &AuthorityState,
    outcome: &TickOutcome,
    events: &mut Vec<RoutedEvent>,
) {
    for (observer, view) in observers.iter().zip(view_list.iter_mut()) {
        for (key, base, new, changes) in &outcome.block_batches {
            if !observer.wanted.contains(key) {
                continue;
            }
            let Some(entry) = view.chunks.get_mut(key) else {
                continue;
            };
            if !entry.snapshot_sent || entry.last_revision == *new {
                continue;
            }
            if entry.last_revision == *base {
                entry.last_revision = *new;
                for group in changes.chunks(BLOCK_CHANGES_CAP) {
                    if let Ok(batch) = BlockChanges::try_new(BlockChangesParts {
                        dimension: key.dimension,
                        chunk: key.pos,
                        base_revision: *base,
                        new_revision: *new,
                        changes: group.to_vec().into_boxed_slice(),
                    }) {
                        events.push(RoutedEvent::new(
                            EventRecipient::Session(observer.session.get()),
                            Event::BlockChanges(batch),
                        ));
                    }
                }
            } else if let Some(snapshot) = state.chunk_snapshot_event(*key) {
                entry.last_revision = snapshot.revision();
                events.push(RoutedEvent::new(
                    EventRecipient::Session(observer.session.get()),
                    Event::ChunkSnapshot(snapshot),
                ));
            }
        }
    }
}

/// Companion spawns, then the persisting state batch split at the
/// companion wire cap. Newly spawned identities are excluded from the state
/// batch of their spawn tick.
fn emit_companions(
    view_list: &mut [SessionView],
    observers: &[Observer],
    visibilities: &[Visibility],
    inputs: &WorldInputs<'_>,
    events: &mut Vec<RoutedEvent>,
) {
    for ((observer, view), visibility) in
        observers.iter().zip(view_list.iter_mut()).zip(visibilities)
    {
        let mut published = visibility.companions.clone();
        for id in visibility.companions.difference(&view.visible_companions) {
            let Some((_, index)) = inputs
                .entities
                .companions
                .iter()
                .find(|(candidate, _)| candidate == id)
            else {
                published.remove(id);
                continue;
            };
            let actor = &inputs.actors[*index];
            let Some(name) = companion_display_name(*id) else {
                published.remove(id);
                continue;
            };
            match CompanionSpawn::try_new(CompanionSpawnParts {
                id: *id,
                name,
                server_tick: inputs.tick,
                dimension: actor.dimension,
                position: actor.motion.position(),
                look: actor.look,
            }) {
                Ok(spawn) => {
                    events.push(RoutedEvent::new(
                        EventRecipient::Session(observer.session.get()),
                        Event::CompanionSpawn(spawn),
                    ));
                }
                Err(_) => {
                    published.remove(id);
                }
            }
        }
        let mut states = Vec::new();
        for id in visibility
            .companions
            .intersection(&view.visible_companions)
            .copied()
            .collect::<Vec<_>>()
        {
            let Some((_, index)) = inputs
                .entities
                .companions
                .iter()
                .find(|(candidate, _)| candidate == &id)
            else {
                continue;
            };
            let actor = &inputs.actors[*index];
            if let Ok(record) = CompanionState::try_new(CompanionStateParts {
                id,
                dimension: actor.dimension,
                position: actor.motion.position(),
                look: actor.look,
                reset: false,
            }) {
                states.push(record);
            }
        }
        for group in states.chunks(COMPANION_STATES_CAP) {
            if let Ok(batch) = CompanionStates::try_new(CompanionStatesParts {
                server_tick: inputs.tick,
                states: group.to_vec().into_boxed_slice(),
            }) {
                events.push(RoutedEvent::new(
                    EventRecipient::Session(observer.session.get()),
                    Event::CompanionStates(batch),
                ));
            }
        }
        view.visible_companions = published;
    }
}

/// Remote-player spawns, then one persisting state batch per
/// observer split at the remote wire cap.
fn emit_remotes(
    view_list: &mut [SessionView],
    observers: &[Observer],
    visibilities: &[Visibility],
    inputs: &WorldInputs<'_>,
    events: &mut Vec<RoutedEvent>,
) {
    for ((observer, view), visibility) in
        observers.iter().zip(view_list.iter_mut()).zip(visibilities)
    {
        let mut published = visibility.remotes.clone();
        for id in visibility.remotes.difference(&view.visible_remotes) {
            let Some(speaker) = inputs
                .speakers
                .iter()
                .find(|speaker| speaker.player_id == *id)
            else {
                published.remove(id);
                continue;
            };
            let Some(index) = inputs
                .entities
                .players
                .iter()
                .copied()
                .find(|&index| inputs.actors[index].key == ActorKey::Player(speaker.session))
            else {
                published.remove(id);
                continue;
            };
            let actor = &inputs.actors[index];
            events.push(RoutedEvent::new(
                EventRecipient::Session(observer.session.get()),
                Event::RemotePlayerSpawn(RemotePlayerSpawn::new(RemotePlayerSpawnParts {
                    player_id: *id,
                    display_name: speaker.display_name.clone(),
                    server_tick: inputs.tick,
                    dimension: actor.dimension,
                    position: actor.motion.position(),
                    look: actor.look,
                })),
            ));
        }
        let mut states = Vec::new();
        for id in visibility
            .remotes
            .intersection(&view.visible_remotes)
            .copied()
            .collect::<Vec<_>>()
        {
            let Some(speaker) = inputs
                .speakers
                .iter()
                .find(|speaker| speaker.player_id == id)
            else {
                continue;
            };
            let Some(index) = inputs
                .entities
                .players
                .iter()
                .copied()
                .find(|&index| inputs.actors[index].key == ActorKey::Player(speaker.session))
            else {
                continue;
            };
            let actor = &inputs.actors[index];
            states.push(RemotePlayerState::new(RemotePlayerStateParts {
                player_id: id,
                dimension: actor.dimension,
                position: actor.motion.position(),
                look: actor.look,
                reset: false,
            }));
        }
        for group in states.chunks(REMOTE_STATES_CAP) {
            if let Ok(batch) = RemotePlayerStates::try_new(RemotePlayerStatesParts {
                server_tick: inputs.tick,
                states: group.to_vec().into_boxed_slice(),
            }) {
                events.push(RoutedEvent::new(
                    EventRecipient::Session(observer.session.get()),
                    Event::RemotePlayerStates(batch),
                ));
            }
        }
        view.visible_remotes = published;
    }
}

/// Hostile spawn, state and despawn batches split at the
/// mob wire cap, with the exact body, kind and health of each record.
fn emit_hostiles(
    view_list: &mut [SessionView],
    observers: &[Observer],
    visibilities: &[Visibility],
    inputs: &WorldInputs<'_>,
    events: &mut Vec<RoutedEvent>,
) {
    for ((observer, view), visibility) in
        observers.iter().zip(view_list.iter_mut()).zip(visibilities)
    {
        let mut published = visibility.hostiles.clone();
        let mut spawns = Vec::new();
        for (id, kind, index) in &inputs.entities.hostiles {
            if !visibility.hostiles.contains(id) || view.visible_hostiles.contains(id) {
                continue;
            }
            let actor = &inputs.actors[*index];
            match HostileSpawnRecord::try_new(HostileSpawnRecordParts {
                id: *id,
                dimension: actor.dimension,
                position: actor.motion.position(),
                yaw: actor.look.yaw(),
                health: actor.survival.health(),
                kind: *kind,
            }) {
                Ok(record) => spawns.push(record),
                Err(_) => {
                    published.remove(id);
                }
            }
        }
        for group in spawns.chunks(MOB_BATCH_CAP) {
            if let Ok(batch) = HostileSpawn::try_new(HostileSpawnParts {
                server_tick: inputs.tick,
                spawns: group.to_vec().into_boxed_slice(),
            }) {
                events.push(RoutedEvent::new(
                    EventRecipient::Session(observer.session.get()),
                    Event::HostileSpawn(batch),
                ));
            }
        }
        let mut states = Vec::new();
        for (id, kind, index) in &inputs.entities.hostiles {
            if !view.visible_hostiles.contains(id) || !visibility.hostiles.contains(id) {
                continue;
            }
            let actor = &inputs.actors[*index];
            if let Ok(record) = HostileStateRecord::try_new(HostileStateRecordParts {
                id: *id,
                position: actor.motion.position(),
                velocity: actor.motion.velocity(),
                yaw: actor.look.yaw(),
                health: actor.survival.health(),
                kind: *kind,
            }) {
                states.push(record);
            }
        }
        for group in states.chunks(MOB_BATCH_CAP) {
            if let Ok(batch) = HostileState::try_new(HostileStateParts {
                server_tick: inputs.tick,
                states: group.to_vec().into_boxed_slice(),
            }) {
                events.push(RoutedEvent::new(
                    EventRecipient::Session(observer.session.get()),
                    Event::HostileState(batch),
                ));
            }
        }
        let despawns: Vec<HostileId> = view
            .visible_hostiles
            .difference(&visibility.hostiles)
            .copied()
            .collect();
        for group in despawns.chunks(MOB_BATCH_CAP) {
            if let Ok(batch) = HostileDespawn::try_new(HostileDespawnParts {
                server_tick: inputs.tick,
                ids: group.to_vec().into_boxed_slice(),
            }) {
                events.push(RoutedEvent::new(
                    EventRecipient::Session(observer.session.get()),
                    Event::HostileDespawn(batch),
                ));
            }
        }
        view.visible_hostiles = published;
    }
}

/// Passive spawn, state and despawn batches. Grazing is
/// the transient observation the actor runtime carries; despawns publish the
/// died reason for resident death-settlement removals and the vanished reason
/// otherwise (live view exit, missing actor, or quiet movement termination).
fn emit_passives(
    view_list: &mut [SessionView],
    observers: &[Observer],
    visibilities: &[Visibility],
    inputs: &WorldInputs<'_>,
    state: &AuthorityState,
    events: &mut Vec<RoutedEvent>,
) {
    for ((observer, view), visibility) in
        observers.iter().zip(view_list.iter_mut()).zip(visibilities)
    {
        let mut published = visibility.passives.clone();
        let mut spawns = Vec::new();
        for (id, index) in &inputs.entities.passives {
            if !visibility.passives.contains(id) || view.visible_passives.contains(id) {
                continue;
            }
            let actor = &inputs.actors[*index];
            match PassiveSpawnRecord::try_new(PassiveSpawnRecordParts {
                id: *id,
                dimension: actor.dimension,
                position: actor.motion.position(),
                yaw: actor.look.yaw(),
                health: actor.survival.health(),
            }) {
                Ok(record) => spawns.push(record),
                Err(_) => {
                    published.remove(id);
                }
            }
        }
        for group in spawns.chunks(MOB_BATCH_CAP) {
            if let Ok(batch) = PassiveSpawn::try_new(PassiveSpawnParts {
                server_tick: inputs.tick,
                spawns: group.to_vec().into_boxed_slice(),
            }) {
                events.push(RoutedEvent::new(
                    EventRecipient::Session(observer.session.get()),
                    Event::PassiveSpawn(batch),
                ));
            }
        }
        let mut states = Vec::new();
        for (id, index) in &inputs.entities.passives {
            if !view.visible_passives.contains(id) || !visibility.passives.contains(id) {
                continue;
            }
            let actor = &inputs.actors[*index];
            if let Ok(record) = PassiveStateRecord::try_new(PassiveStateRecordParts {
                id: *id,
                position: actor.motion.position(),
                velocity: actor.motion.velocity(),
                yaw: actor.look.yaw(),
                health: actor.survival.health(),
                grazing: state.resident_grazing(*id),
            }) {
                states.push(record);
            }
        }
        for group in states.chunks(MOB_BATCH_CAP) {
            if let Ok(batch) = PassiveState::try_new(PassiveStateParts {
                server_tick: inputs.tick,
                states: group.to_vec().into_boxed_slice(),
            }) {
                events.push(RoutedEvent::new(
                    EventRecipient::Session(observer.session.get()),
                    Event::PassiveState(batch),
                ));
            }
        }
        let despawns: Vec<PassiveDespawnRecord> = view
            .visible_passives
            .difference(&visibility.passives)
            .copied()
            .map(|id| {
                let reason = if inputs.passive_deaths.contains(&id) {
                    PassiveDespawnReason::Died
                } else {
                    PassiveDespawnReason::Vanished
                };
                PassiveDespawnRecord::new(id, reason)
            })
            .collect();
        for group in despawns.chunks(MOB_BATCH_CAP) {
            if let Ok(batch) = PassiveDespawn::try_new(PassiveDespawnParts {
                server_tick: inputs.tick,
                despawns: group.to_vec().into_boxed_slice(),
            }) {
                events.push(RoutedEvent::new(
                    EventRecipient::Session(observer.session.get()),
                    Event::PassiveDespawn(batch),
                ));
            }
        }
        view.visible_passives = published;
    }
}

/// Projectile spawn, state and despawn batches split at
/// the projectile wire cap, carrying the exact flight values.
fn emit_projectiles(
    view_list: &mut [SessionView],
    observers: &[Observer],
    visibilities: &[Visibility],
    projectiles: &[ProjectileRecord],
    tick: u64,
    events: &mut Vec<RoutedEvent>,
) {
    for ((observer, view), visibility) in
        observers.iter().zip(view_list.iter_mut()).zip(visibilities)
    {
        let mut spawns = Vec::new();
        for record in projectiles {
            if !visibility.projectiles.contains(&record.id)
                || view.visible_projectiles.contains(&record.id)
            {
                continue;
            }
            spawns.push(ProjectileSpawnRecord::new(ProjectileSpawnRecordParts {
                id: record.id,
                kind: record.kind,
                dimension: record.dimension,
                position: record.position,
                velocity: record.velocity,
            }));
        }
        spawns.sort_by_key(|record| record.id());
        for group in spawns.chunks(PROJECTILE_BATCH_CAP) {
            if let Ok(batch) = ProjectileSpawn::try_new(ProjectileSpawnParts {
                server_tick: tick,
                spawns: group.to_vec().into_boxed_slice(),
            }) {
                events.push(RoutedEvent::new(
                    EventRecipient::Session(observer.session.get()),
                    Event::ProjectileSpawn(batch),
                ));
            }
        }
        let mut states = Vec::new();
        for record in projectiles {
            if !view.visible_projectiles.contains(&record.id)
                || !visibility.projectiles.contains(&record.id)
            {
                continue;
            }
            states.push(ProjectileStateRecord::new(ProjectileStateRecordParts {
                id: record.id,
                position: record.position,
            }));
        }
        states.sort_by_key(|record| record.id());
        for group in states.chunks(PROJECTILE_BATCH_CAP) {
            if let Ok(batch) = ProjectileState::try_new(ProjectileStateParts {
                server_tick: tick,
                states: group.to_vec().into_boxed_slice(),
            }) {
                events.push(RoutedEvent::new(
                    EventRecipient::Session(observer.session.get()),
                    Event::ProjectileState(batch),
                ));
            }
        }
        let despawns: Vec<ProjectileId> = view
            .visible_projectiles
            .difference(&visibility.projectiles)
            .copied()
            .collect();
        for group in despawns.chunks(PROJECTILE_BATCH_CAP) {
            if let Ok(batch) = ProjectileDespawn::try_new(ProjectileDespawnParts {
                server_tick: tick,
                ids: group.to_vec().into_boxed_slice(),
            }) {
                events.push(RoutedEvent::new(
                    EventRecipient::Session(observer.session.get()),
                    Event::ProjectileDespawn(batch),
                ));
            }
        }
        // The projectile spawn record is total, so this tick's visible set
        // stores as-is: a departure publishes exactly once.
        view.visible_projectiles.clone_from(&visibility.projectiles);
    }
}

/// Item-drop upserts for newly visible stacks and the
/// removes batch for the ones that left, both split at the drop wire cap.
fn emit_drops(
    view_list: &mut [SessionView],
    observers: &[Observer],
    visibilities: &[Visibility],
    drops: &[DropRecord],
    tick: u64,
    events: &mut Vec<RoutedEvent>,
) {
    for ((observer, view), visibility) in
        observers.iter().zip(view_list.iter_mut()).zip(visibilities)
    {
        let mut published = visibility.drops.clone();
        let mut upserts = Vec::new();
        for record in drops {
            if !visibility.drops.contains(&record.id) || view.visible_drops.contains(&record.id) {
                continue;
            }
            let position = record.position.get();
            let cell = BlockPos::new(
                position[0].floor() as i32,
                position[1].floor() as i32,
                position[2].floor() as i32,
            );
            match ItemDrop::try_new(ItemDropParts {
                id: record.id,
                block_index: mornlea_domain::chunk_block_index(cell),
                stack: domain_stack(record.stack),
            }) {
                Ok(drop) => upserts.push(drop),
                Err(_) => {
                    published.remove(&record.id);
                }
            }
        }
        upserts.sort_by_key(|drop| drop.id());
        for group in upserts.chunks(DROP_BATCH_CAP) {
            if let Ok(batch) = ItemDropUpserts::try_new(ItemDropUpsertsParts {
                server_tick: tick,
                drops: group.to_vec().into_boxed_slice(),
            }) {
                events.push(RoutedEvent::new(
                    EventRecipient::Session(observer.session.get()),
                    Event::ItemDropUpserts(batch),
                ));
            }
        }
        let removes: Vec<mornlea_domain::DropId> = view
            .visible_drops
            .difference(&visibility.drops)
            .copied()
            .collect();
        for group in removes.chunks(DROP_BATCH_CAP) {
            if let Ok(batch) = ItemDropRemoves::try_new(ItemDropRemovesParts {
                server_tick: tick,
                ids: group.to_vec().into_boxed_slice(),
            }) {
                events.push(RoutedEvent::new(
                    EventRecipient::Session(observer.session.get()),
                    Event::ItemDropRemoves(batch),
                ));
            }
        }
        view.visible_drops = published;
    }
}

/// The complete inventory publication of one record.
fn inventory_event(record: &InventoryRecord) -> mornlea_domain::InventoryState {
    let mut hotbar = [ItemStack::EMPTY; 9];
    let mut backpack = [ItemStack::EMPTY; 27];
    for (index, held) in record.slots.iter().take(9).enumerate() {
        hotbar[index] = domain_stack(*held);
    }
    for (index, held) in record.slots.iter().skip(9).enumerate() {
        backpack[index] = domain_stack(*held);
    }
    mornlea_domain::InventoryState::new(mornlea_domain::InventoryStateParts {
        selected: record.selected,
        hotbar,
        backpack,
    })
}

/// One viewed container's record family event, chest or furnace by the
/// record's own kind. The furnace timers narrow to the wire bounds the
/// provider maintains; an out-of-bound record is skipped rather than failing
/// the tick.
fn container_event(
    reference: mornlea_domain::ContainerRef,
    slots: &ContainerSlots,
) -> Option<Event> {
    match slots {
        ContainerSlots::Chest(cells) => {
            let mut items = [ItemStack::EMPTY; 27];
            for (index, held) in cells.iter().enumerate() {
                items[index] = domain_stack(*held);
            }
            mornlea_domain::ChestState::try_new(mornlea_domain::ChestStateParts {
                container: reference,
                items,
            })
            .ok()
            .map(Event::ChestState)
        }
        ContainerSlots::Furnace {
            slots,
            fuel,
            progress,
        } => {
            let progress_ticks = u8::try_from(*progress).ok().filter(|value| *value < 200)?;
            let burn_ticks = u16::try_from(*fuel).ok()?;
            mornlea_domain::FurnaceState::try_new(mornlea_domain::FurnaceStateParts {
                container: reference,
                input: domain_stack(slots[0]),
                fuel: domain_stack(slots[1]),
                output: domain_stack(slots[2]),
                progress_ticks,
                burn_ticks,
            })
            .ok()
            .map(Event::FurnaceState)
        }
    }
}

/// Converts one stored slot through the checked domain stack rule; an invalid
/// stored value collapses to the canonical empty stack so a publication can
/// never fail at the domain boundary.
fn domain_stack(stack: mornlea_storage::ItemStack) -> ItemStack {
    ItemStack::try_new(stack.item, stack.count, stack.durability).unwrap_or(ItemStack::EMPTY)
}

/// Converts a fixed stored slot array through the domain stack rule.
fn domain_stacks<const N: usize>(stacks: &[mornlea_storage::ItemStack; N]) -> [ItemStack; N] {
    let mut converted = [ItemStack::EMPTY; N];
    for (index, held) in stacks.iter().enumerate() {
        converted[index] = domain_stack(*held);
    }
    converted
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The derivation pins its exact form: the `companion-` prefix plus the
    /// lowercase hex of the first eight identity bytes, canonical and far
    /// inside the thirty-two scalar cap, and stable across calls.
    #[test]
    fn companion_name_derivation_is_canonical_and_bounded() {
        let mut bytes = [0u8; 16];
        bytes[0] = 0x0a;
        bytes[6] = 0x40;
        bytes[8] = 0x80;
        let id = CompanionId::try_from_bytes(bytes).unwrap();
        let name = companion_display_name(id).unwrap();
        assert_eq!(name.as_str(), "companion-0a00000000004000");
        assert_eq!(name.as_str().len(), 26);
        assert!(name.as_str().chars().count() < 32);
        assert_eq!(companion_display_name(id).unwrap(), name);
    }

    /// The chat parser accepts the exact addressing shape and rejects every
    /// malformed or unknown form.
    #[test]
    fn chat_addressing_parses_the_frozen_shape() {
        let mut names = BTreeMap::new();
        let mut bytes = [0u8; 16];
        bytes[0] = 1;
        bytes[6] = 0x40;
        bytes[8] = 0x80;
        let id = CompanionId::try_from_bytes(bytes).unwrap();
        let name = companion_display_name(id).unwrap();
        names.insert(name.as_str().to_owned(), id);
        let matched = parse_chat_address(&format!("@{} mine stone", name.as_str()), &names);
        match matched {
            Addressed::Matched {
                id: found, command, ..
            } => {
                assert_eq!(found, id);
                assert_eq!(command.as_str(), "mine stone");
            }
            other => panic!("unexpected parse: {other:?}"),
        }
        assert!(matches!(
            parse_chat_address("hello", &names),
            Addressed::Invalid
        ));
        assert!(matches!(
            parse_chat_address("@nobodyhere", &names),
            Addressed::Invalid
        ));
        assert!(matches!(
            parse_chat_address(&format!("@{} ", name.as_str()), &names),
            Addressed::Invalid
        ));
        match parse_chat_address("@nobody dig", &names) {
            Addressed::Unknown { name } => assert_eq!(name.as_str(), "nobody"),
            other => panic!("unexpected parse: {other:?}"),
        }
    }
}
