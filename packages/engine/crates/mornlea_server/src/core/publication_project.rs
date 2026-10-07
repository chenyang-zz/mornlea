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
    self, BlockChange, BlockChanges, BlockChangesParts, BlockPos, ChatEvent, ChatEventParts,
    ChunkPos, CompanionDespawn, CompanionId, CompanionName, CompanionSpawn, CompanionSpawnParts,
    CompanionState, CompanionStateParts, CompanionStates, CompanionStatesParts, ContainerClosed,
    ContainerRef, CraftingState, CraftingStateParts, Dimension, Event, EventRecipient,
    ForgetChunks, ForgetChunksParts, HostileDespawn, HostileDespawnParts, HostileId, HostileKind,
    HostileSpawn, HostileSpawnParts, HostileSpawnRecord, HostileSpawnRecordParts, HostileState,
    HostileStateParts, HostileStateRecord, HostileStateRecordParts, ItemDrop, ItemDropParts,
    ItemDropRemoves, ItemDropRemovesParts, ItemDropUpserts, ItemDropUpsertsParts, ItemStack,
    PassiveDespawn, PassiveDespawnParts, PassiveDespawnReason, PassiveDespawnRecord, PassiveId,
    PassiveSpawn, PassiveSpawnParts, PassiveSpawnRecord, PassiveSpawnRecordParts, PassiveState,
    PassiveStateParts, PassiveStateRecord, PassiveStateRecordParts, PlayerId, ProjectileDespawn,
    ProjectileDespawnParts, ProjectileId, ProjectileSpawn, ProjectileSpawnParts,
    ProjectileSpawnRecord, ProjectileSpawnRecordParts, ProjectileState, ProjectileStateParts,
    ProjectileStateRecord, ProjectileStateRecordParts, RemotePlayerDespawn, RemotePlayerSpawn,
    RemotePlayerSpawnParts, RemotePlayerState, RemotePlayerStateParts, RemotePlayerStates,
    RemotePlayerStatesParts, RoutedEvent,
};

use crate::contracts::{
    ActorBody, ActorKey, ActorLifecycle, ActorRecord, ActorRuntime, ChunkKey, ContainerSlots,
    DropRecord, InventoryRecord, ProjectileRecord, ServerError, SessionKey,
};
use crate::core::session_view::SessionView;
use crate::state::{AuthorityState, Speaker};

/// Wire record caps the outbound protocol conversion enforces per packet.
/// Visibility families split at these bounds; chunk deltas validate as one
/// whole revision transition before any snapshot can be admitted.
const REMOTE_STATES_CAP: usize = 7;
const COMPANION_STATES_CAP: usize = 4;
const MOB_BATCH_CAP: usize = 64;
const PROJECTILE_BATCH_CAP: usize = 128;
const DROP_BATCH_CAP: usize = 32;
const BLOCK_CHANGES_CAP: usize = 4_096;
const FORGET_CHUNKS_CAP: usize = 4_096;

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
    /// Tick-local inventory publication intent drained from the tick context:
    /// sessions whose accepted inventory, crafting or container commands
    /// marked their owner state dirty this tick, beside the plain record
    /// diff.
    pub(crate) inventory_dirty: BTreeSet<SessionKey>,
    /// Tick-local crafting identity publication intent drained from the tick
    /// context: sessions whose accepted crafting commands marked their private
    /// grid dirty this tick, beside the plain record diff.
    pub(crate) crafting_dirty: BTreeSet<SessionKey>,
}

/// Recipient-local refusal is ordered before snapshots without changing wire DTOs.
pub(crate) struct SourceProjection {
    pub(crate) events: Vec<RoutedEvent>,
    pub(crate) refused_sessions: Vec<SessionKey>,
    pub(crate) before_snapshots: usize,
}

/// The shared read-only world facts the per-family emitters consume.
struct WorldInputs<'a> {
    actors: &'a [ActorRecord],
    /// Reset is captured before source clearers run, independently of delivery.
    /// Actor-only fixtures have no runtime and retain the neutral false flag.
    runtimes: &'a BTreeMap<ActorKey, ActorRuntime>,
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
    /// Per-session wanted around the actor's foot chunk, empty without an
    /// Active player actor or without an admitted session radius.
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
    remotes: BTreeMap<PlayerId, SessionKey>,
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
/// one identity across ticks. Configured companions publish their configured
/// chat name instead; this derived form stays the fallback for unconfigured
/// entity fixtures only.
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
    pub(crate) fn project_source_publication(
        &mut self,
        tick: u64,
        outcome: &TickOutcome,
    ) -> SourceProjection {
        let invalidated_containers = self.invalidate_container_views();
        let speakers = self.active_speakers();
        if speakers.is_empty() {
            return SourceProjection {
                events: Vec::new(),
                refused_sessions: Vec::new(),
                before_snapshots: 0,
            };
        }
        let actors = self.resident_actors().to_vec();
        let entities = classify_entities(&actors);
        let inventories = self.resident_inventories().clone();
        let projectiles = self.resident_projectiles().to_vec();
        let drops = source_drop_records(self, &actors, &entities, &speakers);
        let mut views = self.take_session_views();
        let mut observers = Vec::with_capacity(speakers.len());
        let mut view_list: Vec<SessionView> = Vec::with_capacity(speakers.len());
        for speaker in &speakers {
            let radius = self.session_view_radius(speaker.session);
            observers.push(observer_of(&actors, speaker, &entities, radius));
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
            runtimes: self.resident_runtimes(),
            entities: &entities,
            speakers: &speakers,
            tick,
            passive_deaths: &passive_deaths,
        };
        let mut events = Vec::new();
        emit_despawns(&mut view_list, &observers, &visibilities, &mut events);
        emit_forgets(&mut view_list, &observers, &mut events);
        let before_snapshots = events.len();
        let (deltas, refused_sessions) =
            classify_block_batches(&mut view_list, &observers, outcome);
        let snapshots = emit_snapshots(&mut view_list, &observers, self, &mut events);
        emit_block_batches(&observers, deltas, &snapshots, &mut events);
        let configured_names = self.configured_chat_names();
        emit_companions(
            &mut view_list,
            &observers,
            &visibilities,
            &inputs,
            &configured_names,
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
        self.emit_records(
            &observers,
            &inventories,
            &invalidated_containers,
            outcome,
            &mut view_list,
            &mut events,
        );
        for (observer, mut view) in observers.iter().zip(view_list) {
            view.wanted.clone_from(&observer.wanted);
            view.chunks.retain(|key, _| observer.wanted.contains(key));
            for key in &observer.wanted {
                view.chunks.entry(*key).or_default();
            }
            views.insert(observer.session, view);
        }
        self.restore_session_views(views);
        SourceProjection {
            events,
            refused_sessions,
            before_snapshots,
        }
    }

    #[cfg(test)]
    pub(crate) fn project_tick_publication(
        &mut self,
        tick: u64,
        outcome: &TickOutcome,
    ) -> Vec<RoutedEvent> {
        let projection = self.project_source_publication(tick, outcome);
        assert!(
            projection.refused_sessions.is_empty(),
            "normal fixtures must not discard source refusals"
        );
        projection.events
    }

    /// Chat: drains the decided chat facts and emits one chat event per fact.
    ///
    /// Admission, queue-full, stop, and lifecycle outcomes were decided at
    /// the tick boundary before provider dispatch; this family only assigns
    /// the monotonic event ids in decided order at the existing family slot.
    /// Accepted and lifecycle facts broadcast while sender-only rejections
    /// address the issuing session. The pre-tick budget proves id headroom,
    /// so allocation cannot run out. Source-invalid restatements consume their
    /// allocated id but emit nothing; later valid facts and task mutations survive.
    fn emit_chat(&mut self, _entities: &Entities, events: &mut Vec<RoutedEvent>) {
        for fact in self.take_decided_chat_facts() {
            let Some(event_id) = self.allocate_chat_event_id() else {
                break;
            };
            let Ok(chat) = ChatEvent::try_new(ChatEventParts {
                event_id,
                player_id: fact.player_id,
                player_name: fact.player_name,
                body: fact.body,
            }) else {
                // Saved task text is broader than wire chat text; Go skips invalid facts too.
                continue;
            };
            let event = Event::Chat(chat);
            let recipient = match fact.recipient {
                None => EventRecipient::Broadcast,
                Some(session) => EventRecipient::Session(session.get()),
            };
            events.push(RoutedEvent::new(recipient, event));
        }
    }

    /// Owner records follow inventory, crafting, furnace, chest and close.
    /// Valid physical views publish complete state every tick. The ephemeral
    /// close facts name only final invalidations, never explicit releases.
    ///
    /// The inventory state and the crafting state are the two families with
    /// an explicit command intent beside their record diffs: a session whose
    /// accepted inventory, crafting or container commands marked the
    /// tick-local dirty lane still publishes exactly one final owner state
    /// even when the
    /// settled wire record equals the last published snapshot (the select
    /// round trip, accepted armor swap, crafting round trip, or a bite followed
    /// by an identical pickup). Pending actors preserve both owner mirrors.
    fn emit_records(
        &mut self,
        observers: &[Observer],
        inventories: &BTreeMap<ActorKey, InventoryRecord>,
        invalidated_containers: &BTreeMap<SessionKey, ContainerRef>,
        outcome: &TickOutcome,
        view_list: &mut [SessionView],
        events: &mut Vec<RoutedEvent>,
    ) {
        for (observer, view) in observers.iter().zip(view_list.iter_mut()) {
            let actor = ActorKey::Player(observer.session);
            let owner = EventRecipient::Session(observer.session.get());
            let record = inventories.get(&actor).copied();
            let inventory_dirty =
                observer.has_actor && outcome.inventory_dirty.contains(&observer.session);
            let crafting_dirty =
                observer.has_actor && outcome.crafting_dirty.contains(&observer.session);
            // Record selection observes admitted mirrors; the FIFO owns their updates.
            // The inventory state comes first.
            if observer.has_actor
                && let Some(record) = record
            {
                let inventory = inventory_event(&record);
                if view.last_inventory.as_ref() != Some(&inventory) || inventory_dirty {
                    events.push(RoutedEvent::new(owner, Event::InventoryState(inventory)));
                }
            }
            // Crafting precedes both container kinds in the source wire order.
            if observer.has_actor
                && let Some(record) = record
            {
                let crafting = (record.crafting, record.crafting_size);
                if view.last_crafting.as_ref() != Some(&crafting) || crafting_dirty {
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
                    }
                }
            }
            if let Some(lease) = self.committed_lease(observer.session) {
                let reference = lease.reference();
                if let Some(held) = self.resident_container(reference)
                    && let Some(event) = container_event(reference, &held.slots)
                {
                    events.push(RoutedEvent::new(owner, event));
                }
            }
            if let Some(&reference) = invalidated_containers.get(&observer.session) {
                events.push(RoutedEvent::new(
                    owner,
                    Event::ContainerClosed(ContainerClosed::new(reference)),
                ));
            }
        }
    }
}

/// One observer's inputs: identity plus per-session wanted derived from its
/// player actor and the authoritative session radius. The session view keeps
/// the previous tick's wanted set until the closing commit, so the forget
/// diff can read it.
fn observer_of(
    actors: &[ActorRecord],
    speaker: &Speaker,
    entities: &Entities,
    radius: Option<u8>,
) -> Observer {
    let has_actor = entities.players.iter().any(|&index| {
        actors[index].key == ActorKey::Player(speaker.session)
            && actors[index].lifecycle == ActorLifecycle::Active
    });
    let wanted = radius
        .and_then(|radius| {
            entities
                .players
                .iter()
                .copied()
                .find(|&index| actors[index].key == ActorKey::Player(speaker.session))
                .filter(|&index| actors[index].lifecycle == ActorLifecycle::Active)
                .map(|index| wanted_columns(&actors[index], radius))
        })
        .unwrap_or_default();
    Observer {
        session: speaker.session,
        wanted,
        has_actor,
    }
}

/// Copies only physical Ready drop slots in the Active players' source interest.
/// Eight players contribute at most two hundred keys and thirty-two slots per key.
fn source_drop_records(
    state: &AuthorityState,
    actors: &[ActorRecord],
    entities: &Entities,
    speakers: &[Speaker],
) -> Vec<DropRecord> {
    let mut keys = BTreeSet::new();
    for speaker in speakers {
        if let Some(&index) = entities
            .players
            .iter()
            .find(|&&index| actors[index].key == ActorKey::Player(speaker.session))
        {
            keys.extend(wanted_columns(&actors[index], 2));
        }
    }
    // Projection runs only after the healthy reducer has committed its exclusive loan.
    let view = state.settled_read().expect("healthy publication authority");
    let mut records = Vec::new();
    for key in keys {
        if view.ready_chunk(key) {
            records.extend_from_slice(view.drops(key));
        }
    }
    records
}

/// The per-session wanted chunk columns around one actor's foot position in
/// its own dimension: the inclusive square centered on floor(X/Z) >> 4 with
/// the given radius.
fn wanted_columns(actor: &ActorRecord, radius: u8) -> BTreeSet<ChunkKey> {
    let position = actor.motion.position().get();
    let center = ChunkPos::new(
        (position[0].floor() as i32) >> 4,
        (position[2].floor() as i32) >> 4,
    );
    // The foot center stays inside the physical chunk range (+/- 134217728)
    // and the session radius ceiling (33) keeps every offset far from the
    // i32 limits, so the shared checked square cannot refuse here.
    wanted_square(actor.dimension, center, radius).expect("physical chunk square")
}

/// The inclusive square of chunk columns around one center in one dimension,
/// shared by publication projection and automatic source acquisition. Every
/// offset is a checked add: a center no physical chunk can move away from
/// refuses instead of saturating into a wrapped square.
pub(crate) fn wanted_square(
    dimension: Dimension,
    center: ChunkPos,
    radius: u8,
) -> Result<BTreeSet<ChunkKey>, ServerError> {
    let bound = i32::from(radius);
    let mut keys = BTreeSet::new();
    for dz in -bound..=bound {
        for dx in -bound..=bound {
            let x = center.x().checked_add(dx).ok_or_else(square_refusal)?;
            let z = center.z().checked_add(dz).ok_or_else(square_refusal)?;
            keys.insert(ChunkKey {
                dimension,
                pos: ChunkPos::new(x, z),
            });
        }
    }
    Ok(keys)
}

/// The one refusal the shared checked square produces.
fn square_refusal() -> ServerError {
    ServerError::InvalidInput {
        field: "source_acquisition_geometry",
    }
}

/// The chunk column one world position sits in.
pub(crate) fn position_chunk(dimension: Dimension, position: [f32; 3]) -> ChunkKey {
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
        remotes: BTreeMap::new(),
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
            visibility.remotes.insert(speaker.player_id, target);
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
    // Drops use physical radius two even when snapshot interest is narrower or wider.
    let drop_wanted = entities
        .players
        .iter()
        .copied()
        .find(|&index| actors[index].key == ActorKey::Player(observer.session))
        .map(|index| wanted_columns(&actors[index], 2))
        .unwrap_or_default();
    for record in drops {
        if let Some(dimension) = u8::try_from(record.id.dimension())
            .ok()
            .and_then(|raw| Dimension::new(raw).ok())
            && drop_wanted.contains(&ChunkKey {
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

/// Remote-player and companion despawn diffs. A remote authority incarnation
/// change despawns the old identity even when interest remains unchanged;
/// companions retain their interest-exit boundary. Both memberships advance
/// only after queue admission, so refused departures remain retryable. Other
/// visibility families retain their projection-owned migration state.
fn emit_despawns(
    view_list: &mut [SessionView],
    observers: &[Observer],
    visibilities: &[Visibility],
    events: &mut Vec<RoutedEvent>,
) {
    for ((observer, view), visibility) in
        observers.iter().zip(view_list.iter_mut()).zip(visibilities)
    {
        for (id, previous) in &view.visible_remotes {
            if visibility.remotes.get(id) == Some(previous) {
                continue;
            }
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
) -> BTreeSet<(SessionKey, ChunkKey)> {
    let mut emitted = BTreeSet::new();
    for (observer, view) in observers.iter().zip(view_list.iter_mut()) {
        let flagged: Vec<ChunkKey> = view
            .chunks
            .iter()
            .filter(|(key, entry)| entry.resync_queued && observer.wanted.contains(key))
            .map(|(key, _)| *key)
            .collect();
        for key in flagged {
            if let Some(entry) = view.chunks.get_mut(&key) {
                if let Some(snapshot) = state.chunk_snapshot_event(key) {
                    emitted.insert((observer.session, key));
                    events.push(RoutedEvent::new(
                        EventRecipient::Session(observer.session.get()),
                        Event::ChunkSnapshot(snapshot),
                    ));
                } else {
                    // An unavailable desired request is discarded as in the source owner.
                    entry.resync_queued = false;
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
                !emitted.contains(&(observer.session, *key))
                    && !view
                        .chunks
                        .get(key)
                        .is_some_and(|entry| entry.snapshot_sent)
            })
            .collect();
        for key in pending {
            let Some(snapshot) = state.chunk_snapshot_event(key) else {
                continue;
            };
            emitted.insert((observer.session, key));
            events.push(RoutedEvent::new(
                EventRecipient::Session(observer.session.get()),
                Event::ChunkSnapshot(snapshot),
            ));
        }
    }
    emitted
}

/// Validate eligible whole revisions before snapshots. One invalid batch
/// discards all classified deltas for its recipient, preserving earlier families.
fn classify_block_batches(
    view_list: &mut [SessionView],
    observers: &[Observer],
    outcome: &TickOutcome,
) -> (Vec<Vec<BlockChanges>>, Vec<SessionKey>) {
    let mut classified = Vec::with_capacity(observers.len());
    let mut refused = Vec::new();
    for (observer, view) in observers.iter().zip(view_list.iter_mut()) {
        let mut deltas = Vec::new();
        for (key, base, new, changes) in &outcome.block_batches {
            if !observer.wanted.contains(key) {
                continue;
            }
            let Some(entry) = view.chunks.get_mut(key) else {
                continue;
            };
            if !entry.snapshot_sent {
                continue;
            }
            if entry.last_revision == *new && entry.last_revision != *base {
                continue;
            }
            if entry.resync_queued || entry.last_revision != *base {
                entry.resync_queued = true;
                continue;
            }
            // Count-first refusal bounds the allocation and never splits one revision.
            let batch = (changes.len() <= BLOCK_CHANGES_CAP).then(|| {
                BlockChanges::try_new(BlockChangesParts {
                    dimension: key.dimension,
                    chunk: key.pos,
                    base_revision: *base,
                    new_revision: *new,
                    changes: changes.clone().into_boxed_slice(),
                })
            });
            let Some(Ok(batch)) = batch else {
                deltas.clear();
                refused.push(observer.session);
                break;
            };
            deltas.push(batch);
        }
        classified.push(deltas);
    }
    (classified, refused)
}

/// A same-pass settled full capture covers every classified delta for that key.
fn emit_block_batches(
    observers: &[Observer],
    classified: Vec<Vec<BlockChanges>>,
    snapshots: &BTreeSet<(SessionKey, ChunkKey)>,
    events: &mut Vec<RoutedEvent>,
) {
    for (observer, deltas) in observers.iter().zip(classified) {
        for batch in deltas {
            let key = ChunkKey {
                dimension: batch.dimension(),
                pos: batch.chunk(),
            };
            if !snapshots.contains(&(observer.session, key)) {
                events.push(RoutedEvent::new(
                    EventRecipient::Session(observer.session.get()),
                    Event::BlockChanges(batch),
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
    configured: &BTreeMap<CompanionId, CompanionName>,
    events: &mut Vec<RoutedEvent>,
) {
    for ((observer, view), visibility) in
        observers.iter().zip(view_list.iter_mut()).zip(visibilities)
    {
        for id in visibility.companions.difference(&view.visible_companions) {
            let Some((_, index)) = inputs
                .entities
                .companions
                .iter()
                .find(|(candidate, _)| candidate == id)
            else {
                continue;
            };
            let actor = &inputs.actors[*index];
            // Configured companions publish their configured chat name; the
            // derived form stays the fallback for unconfigured fixtures.
            let Some(name) = configured
                .get(id)
                .cloned()
                .or_else(|| companion_display_name(*id))
            else {
                continue;
            };
            if let Ok(spawn) = CompanionSpawn::try_new(CompanionSpawnParts {
                id: *id,
                name,
                server_tick: inputs.tick,
                dimension: actor.dimension,
                position: actor.motion.position(),
                look: actor.look,
            }) {
                events.push(RoutedEvent::new(
                    EventRecipient::Session(observer.session.get()),
                    Event::CompanionSpawn(spawn),
                ));
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
                reset: inputs
                    .runtimes
                    .get(&actor.key)
                    .is_some_and(|runtime| runtime.reset),
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
        for (id, session) in &visibility.remotes {
            if view.visible_remotes.get(id) == Some(session) {
                continue;
            }
            let Some(speaker) = inputs
                .speakers
                .iter()
                .find(|speaker| speaker.player_id == *id && speaker.session == *session)
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
        for (&id, &session) in &visibility.remotes {
            if view.visible_remotes.get(&id) != Some(&session) {
                continue;
            }
            let Some(speaker) = inputs
                .speakers
                .iter()
                .find(|speaker| speaker.player_id == id && speaker.session == session)
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
                reset: inputs
                    .runtimes
                    .get(&actor.key)
                    .is_some_and(|runtime| runtime.reset),
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
    }
}

/// Hostile despawn, spawn and survivor state batches split at the
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
        let mut spawns = Vec::new();
        for (id, kind, index) in &inputs.entities.hostiles {
            if !visibility.hostiles.contains(id) || view.visible_hostiles.contains(id) {
                continue;
            }
            let actor = &inputs.actors[*index];
            if let Ok(record) = HostileSpawnRecord::try_new(HostileSpawnRecordParts {
                id: *id,
                dimension: actor.dimension,
                position: actor.motion.position(),
                yaw: actor.look.yaw(),
                health: actor.survival.health(),
                kind: *kind,
            }) {
                spawns.push(record);
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
    }
}

/// Passive despawn, spawn and survivor state batches. Grazing is
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
        view.visible_passives = published;
    }
}

/// Projectile despawn, spawn and survivor state batches split at
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
        // The projectile spawn record is total, so this tick's visible set
        // stores as-is: a departure publishes exactly once.
        view.visible_projectiles.clone_from(&visibility.projectiles);
    }
}

/// Item-drop upserts for newly visible or changed stacks and the
/// removes batch for the ones that left, both split at the drop wire cap.
/// Removes precede changed/new upserts as in the source server. The mirror
/// compares only the complete wire value, so lifecycle counters stay quiet.
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
        let mut published = BTreeMap::new();
        let mut upserts = Vec::new();
        for record in drops {
            if !visibility.drops.contains(&record.id) {
                continue;
            }
            let position = record.position.get();
            let cell = BlockPos::new(
                position[0].floor() as i32,
                position[1].floor() as i32,
                position[2].floor() as i32,
            );
            if let Ok(drop) = ItemDrop::try_new(ItemDropParts {
                id: record.id,
                block_index: mornlea_domain::chunk_block_index(cell),
                stack: domain_stack(record.stack),
            }) {
                if view.visible_drops.get(&record.id) != Some(&drop) {
                    upserts.push(drop);
                }
                published.insert(record.id, drop);
            }
        }
        upserts.sort_by_key(|drop| drop.id());
        let removes: Vec<mornlea_domain::DropId> = view
            .visible_drops
            .keys()
            .filter(|id| !visibility.drops.contains(id))
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
    use crate::core::companion_chat::{Addressed, parse_chat_address};

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
