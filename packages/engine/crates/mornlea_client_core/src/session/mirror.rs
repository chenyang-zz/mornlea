//! The confirmed mirror provider: the sole owner of confirmed-revision
//! increments and actor identity resolution.
//!
//! One complete accepted authoritative observation is committed whole or not
//! at all. `commit` first validates the observation's identity — the staging
//! epoch must be the mirror's own epoch and the staged key must name exactly
//! the next confirmed revision — then validates every packet identity and
//! every required prior dimension against the current stores on staged
//! copies, and only the final infallible swap replaces the mirror, increments
//! the revision exactly once and appends the enriched observation. A
//! duplicate, backward, future, malformed, control-family, old-epoch or
//! old-generation observation therefore preserves the prior accepted state
//! in full: no partial mirror and no partial revision are ever published.
//!
//! Identity resolution: a dimensionless state or despawn record (hostile,
//! passive and projectile bodies, remote-player and companion despawns) is
//! matched against exactly one live typed identity; an orphan (no live
//! identity), an ambiguous match (more than one) and an old generation (a
//! chunk history that does not continue, a snapshot at or below the stored
//! content revision, or a container generation a newer generation already
//! replaced) all reject. Removal precedes reuse: a spawn naming an
//! already-live identity is a duplicate, while the same identity may spawn
//! again after its despawn or removal. The legacy Go mirrors silently ignore
//! several of these classes; this checked core rejects them instead, because
//! a silently dropped record inside an accepted observation would publish a
//! frame whose revision attests content the mirror never confirmed.
//!
//! Compatibility boundaries: the optional source tick is preserved exactly
//! from the packet — every semantic event that carries a server tick keeps
//! it and `ForgetChunks`, which carries none, never invents one — and the
//! staged key is adopted verbatim, so the login provider's FIFO staging
//! order is the retained source order. A close naming an unheld container
//! and a forget naming an already-retired chunk commit without changing any
//! store, matching the accepted close/forget tolerance the Go oracles pin.
//! The committed observation queue is bounded by the frozen inbound
//! observation limit and shrinks only through the publication consumption
//! cursor, so no unbounded packet history is retained.

use mornlea_domain::{
    ChunkPos, ContainerRef, Dimension, DropId, Event, HostileId, PassiveId, PlayerId, ProjectileId,
};

use crate::contracts::{
    ClientError, ClientLimits, ConfirmedRevision, ObservationKey, SessionEpoch, SessionPhase,
};
use crate::presentation::frame::{ActorId, ActorKind};
use crate::presentation::{AcceptedObservation, PublicationConsumption, ResolvedActor};
use crate::session::{
    ActorConfirmed, ConfirmedMirror, ConfirmedMirrorParts, InventoryConfirmed, WorldConfirmed,
    WorldUiConfirmed,
};

/// The confirmed mirror state machine: the mirror beside the bounded queue
/// of committed, not-yet-consumed observations that together form the
/// immutable projection context.
///
/// The provider is built pending for one epoch, admitted once the login
/// exchange completes, and replaced wholesale by the controller on reset.
/// It is the only place the confirmed revision advances: every other owner
/// reads `mirror()` and `observations()` through immutable borrows.
pub struct MirrorProvider {
    mirror: ConfirmedMirror,
    observations: Vec<AcceptedObservation>,
    limits: ClientLimits,
}

impl MirrorProvider {
    /// Builds the pending mirror owner for one epoch: revision zero, no
    /// confirmed stores, disconnected until the login provider admits the
    /// session.
    pub fn new(epoch: SessionEpoch, limits: ClientLimits) -> Result<Self, ClientError> {
        Self::from_parts(ConfirmedMirrorParts::pending(epoch), limits)
    }

    /// Adopts an already-validated mirror. The controller uses this to carry
    /// confirmed state across an owner handover; the next commit continues
    /// from the adopted revision exactly.
    pub fn from_parts(
        parts: ConfirmedMirrorParts,
        limits: ClientLimits,
    ) -> Result<Self, ClientError> {
        Ok(Self {
            mirror: ConfirmedMirror::try_new(parts)?,
            observations: Vec::new(),
            limits,
        })
    }

    /// Admits the mirror: the session phase a pending mirror carries until
    /// the login exchange completes. Admission precedes every observation,
    /// so it may only leave the disconnected phase.
    pub fn admit(&mut self) -> Result<(), ClientError> {
        if *self.mirror.phase() != SessionPhase::Disconnected {
            return Err(ClientError::InvalidState);
        }
        let epoch = self.mirror.epoch();
        let revision = self.mirror.revision();
        let world = self.mirror.world().clone();
        let actors = self.mirror.actors().clone();
        let inventory = self.mirror.inventory().clone();
        let world_ui = self.mirror.world_ui().clone();
        self.mirror = ConfirmedMirror::try_new(ConfirmedMirrorParts {
            epoch,
            revision,
            phase: SessionPhase::Admitted,
            world: Some(world),
            actors: Some(actors),
            inventory: Some(inventory),
            world_ui: Some(world_ui),
        })?;
        Ok(())
    }

    /// The current confirmed mirror: the read-only projection input every
    /// family provider borrows.
    pub fn mirror(&self) -> &ConfirmedMirror {
        &self.mirror
    }

    /// The committed observations no publication has consumed yet, in actual
    /// source order.
    pub fn observations(&self) -> &[AcceptedObservation] {
        &self.observations
    }

    /// Consumes the observation prefix one successful publication has
    /// projected. The other consumption counts belong to their own owners;
    /// only the observation cursor is read here, and an over-long cursor is
    /// an invalid input that consumes nothing.
    pub fn consume(&mut self, consumed: &PublicationConsumption) -> Result<(), ClientError> {
        let count = consumed.observations();
        if count > self.observations.len() {
            return Err(ClientError::InvalidInput);
        }
        self.observations.drain(..count);
        Ok(())
    }

    /// Commits one complete accepted observation and returns the new
    /// confirmed revision it issued.
    ///
    /// The staged key supplies the epoch, the expected next revision and the
    /// retained ordinal; the staged tick and resolution entries are
    /// re-derived here, because this owner — not the structural stager — is
    /// responsible for them. Every failure class leaves the mirror, the
    /// revision and the observation queue exactly as they were.
    pub fn commit(
        &mut self,
        staged: &AcceptedObservation,
    ) -> Result<ConfirmedRevision, ClientError> {
        let key: ObservationKey = *staged.key();
        if key.epoch() != self.mirror.epoch() {
            // An observation from another epoch never enters this mirror,
            // however valid its packet is; old epochs never resurrect.
            return Err(ClientError::StaleEpoch);
        }
        if *self.mirror.phase() != SessionPhase::Admitted {
            // Nothing is confirmed before the login exchange completes.
            return Err(ClientError::InvalidState);
        }
        let next = self
            .mirror
            .revision()
            .get()
            .checked_add(1)
            .ok_or(ClientError::Capacity)?;
        if key.confirmed_revision().get() != next {
            // A duplicate, backward or ahead-of-sequence staged key is a
            // malformed observation identity, never a partial commit.
            return Err(ClientError::InvalidInput);
        }
        let event =
            Event::try_from(staged.packet().clone()).map_err(|_| ClientError::InvalidInput)?;
        if self.observations.len() + 1 > self.limits.inbound_observations() {
            // The queue bound is checked before any staging allocation, so a
            // refused observation stays owned by the caller for retry.
            return Err(ClientError::Capacity);
        }
        let revision = ConfirmedRevision::new(next);
        let mut world = self.mirror.world().clone();
        let mut actors = self.mirror.actors().clone();
        let mut inventory = self.mirror.inventory().clone();
        let mut world_ui = self.mirror.world_ui().clone();
        let mut resolved: Vec<ResolvedActor> = Vec::new();
        stage_event(
            &event,
            revision,
            &mut world,
            &mut actors,
            &mut inventory,
            &mut world_ui,
            &mut resolved,
        )?;
        let committed = AcceptedObservation::try_new(
            key,
            source_tick(&event),
            staged.packet().clone(),
            resolved,
        )?;
        self.mirror = ConfirmedMirror::try_new(ConfirmedMirrorParts {
            epoch: self.mirror.epoch(),
            revision,
            phase: *self.mirror.phase(),
            world: Some(world),
            actors: Some(actors),
            inventory: Some(inventory),
            world_ui: Some(world_ui),
        })?;
        self.observations.push(committed);
        Ok(revision)
    }
}

/// Stages one semantic event onto the staged store copies.
///
/// Every rule below is checked before the matching store changes, and a
/// failure anywhere rejects the whole observation: the caller drops all
/// staged copies, so no partial mirror exists.
fn stage_event(
    event: &Event,
    revision: ConfirmedRevision,
    world: &mut WorldConfirmed,
    actors: &mut ActorConfirmed,
    inventory: &mut InventoryConfirmed,
    world_ui: &mut WorldUiConfirmed,
    resolved: &mut Vec<ResolvedActor>,
) -> Result<(), ClientError> {
    match event {
        Event::ChunkSnapshot(snapshot) => {
            let dimension = snapshot.dimension();
            let chunk = snapshot.chunk();
            if let Some(stored) = held_revision(world, dimension, chunk)
                && snapshot.revision() <= *stored
            {
                // An at-or-below snapshot is an old chunk generation: a
                // replaced history cannot come back through the snapshot
                // path either.
                return Err(ClientError::InvalidInput);
            }
            *world = world
                .clone()
                .with_chunk(dimension, chunk, snapshot.revision());
        }
        Event::BlockChanges(batch) => {
            let dimension = batch.dimension();
            let chunk = batch.chunk();
            // The required prior dimension: a delta must continue a chunk the
            // mirror already holds, so it can never invent world state in a
            // dimension no snapshot established.
            let stored = held_revision(world, dimension, chunk).ok_or(ClientError::InvalidInput)?;
            if batch.base_revision() != *stored {
                // Behind the stored revision is an old generation; ahead of
                // it is not a continuation. Both reject.
                return Err(ClientError::InvalidInput);
            }
            *world = world
                .clone()
                .with_chunk(dimension, chunk, batch.new_revision());
        }
        Event::ForgetChunks(forget) => {
            let mut staged = world.clone();
            for chunk in forget.chunks() {
                // A forget names retirement, not a lookup: a chunk the
                // mirror does not hold is already retired.
                staged.remove_chunk(forget.dimension(), *chunk);
            }
            *world = staged;
        }
        Event::RemotePlayerSpawn(spawn) => {
            reject_live_identity(actors.remote_players(), &spawn.player_id())?;
            *actors = actors
                .clone()
                .with_remote_player(spawn.player_id(), spawn.dimension());
        }
        Event::RemotePlayerDespawn(despawn) => {
            let dimension = resolve_dimensioned(actors.remote_players(), &despawn.player_id())?;
            resolved.push(resolution(
                ActorKind::RemotePlayer,
                ActorId::RemotePlayer(despawn.player_id()),
                dimension,
            )?);
            *actors = without(actors, ActorRemoval::RemotePlayer(&despawn.player_id()))?;
        }
        Event::RemotePlayerStates(states) => {
            for state in states.states() {
                let dimension = resolve_dimensioned(actors.remote_players(), &state.player_id())?;
                if state.dimension() != dimension {
                    // The record names a dimension the confirmed identity
                    // does not live in; a dimension change is a despawn and
                    // spawn pair, never a state record.
                    return Err(ClientError::InvalidInput);
                }
            }
        }
        Event::CompanionSpawn(spawn) => {
            reject_live_identity(actors.companions(), &spawn.id())?;
            *actors = actors.clone().with_companion(spawn.id(), spawn.dimension());
        }
        Event::CompanionStates(states) => {
            for state in states.states() {
                let dimension = resolve_dimensioned(actors.companions(), &state.id())?;
                if state.dimension() != dimension {
                    return Err(ClientError::InvalidInput);
                }
            }
        }
        Event::CompanionDespawn(despawn) => {
            let dimension = resolve_dimensioned(actors.companions(), &despawn.id())?;
            resolved.push(resolution(
                ActorKind::Companion,
                ActorId::Companion(despawn.id()),
                dimension,
            )?);
            *actors = without(actors, ActorRemoval::Companion(&despawn.id()))?;
        }
        Event::HostileSpawn(spawn) => {
            for record in spawn.spawns() {
                reject_live_identity(actors.hostiles(), &record.id())?;
                *actors = actors.clone().with_hostile(record.id(), record.dimension());
            }
        }
        Event::HostileState(states) => {
            for record in states.states() {
                let dimension = resolve_dimensioned(actors.hostiles(), &record.id())?;
                resolved.push(resolution(
                    ActorKind::Hostile,
                    ActorId::Hostile(record.id()),
                    dimension,
                )?);
            }
        }
        Event::HostileDespawn(despawn) => {
            for id in despawn.ids() {
                let dimension = resolve_dimensioned(actors.hostiles(), id)?;
                resolved.push(resolution(
                    ActorKind::Hostile,
                    ActorId::Hostile(*id),
                    dimension,
                )?);
            }
            let mut staged = actors.clone();
            for id in despawn.ids() {
                staged = without(&staged, ActorRemoval::Hostile(id))?;
            }
            *actors = staged;
        }
        Event::PassiveSpawn(spawn) => {
            for record in spawn.spawns() {
                reject_live_identity(actors.passives(), &record.id())?;
                *actors = actors.clone().with_passive(record.id(), record.dimension());
            }
        }
        Event::PassiveState(states) => {
            for record in states.states() {
                let dimension = resolve_dimensioned(actors.passives(), &record.id())?;
                resolved.push(resolution(
                    ActorKind::Passive,
                    ActorId::Passive(record.id()),
                    dimension,
                )?);
            }
        }
        Event::PassiveDespawn(despawn) => {
            for record in despawn.despawns() {
                let dimension = resolve_dimensioned(actors.passives(), &record.id())?;
                resolved.push(resolution(
                    ActorKind::Passive,
                    ActorId::Passive(record.id()),
                    dimension,
                )?);
            }
            let mut staged = actors.clone();
            for record in despawn.despawns() {
                staged = without(&staged, ActorRemoval::Passive(&record.id()))?;
            }
            *actors = staged;
        }
        Event::ProjectileSpawn(spawn) => {
            for record in spawn.spawns() {
                reject_live_identity(actors.projectiles(), &record.id())?;
                *actors = actors
                    .clone()
                    .with_projectile(record.id(), record.dimension());
            }
        }
        Event::ProjectileState(states) => {
            for record in states.states() {
                let dimension = resolve_dimensioned(actors.projectiles(), &record.id())?;
                resolved.push(resolution(
                    ActorKind::Projectile,
                    ActorId::Projectile(record.id()),
                    dimension,
                )?);
            }
        }
        Event::ProjectileDespawn(despawn) => {
            for id in despawn.ids() {
                let dimension = resolve_dimensioned(actors.projectiles(), id)?;
                resolved.push(resolution(
                    ActorKind::Projectile,
                    ActorId::Projectile(*id),
                    dimension,
                )?);
            }
            let mut staged = actors.clone();
            for id in despawn.ids() {
                staged = without(&staged, ActorRemoval::Projectile(id))?;
            }
            *actors = staged;
        }
        Event::ItemDropUpserts(upserts) => {
            // An upsert adds or wholly replaces: a live identity stays exactly
            // once and its stack value belongs to the observation, not the
            // identity store.
            let mut staged = actors.clone();
            for drop in upserts.drops() {
                if !staged.drops().contains(&drop.id()) {
                    staged = staged.with_drop(drop.id());
                }
            }
            *actors = staged;
        }
        Event::ItemDropRemoves(removes) => {
            for id in removes.ids() {
                if !actors.drops().contains(id) {
                    // An unknown drop identity is an orphan removal.
                    return Err(ClientError::InvalidInput);
                }
            }
            let mut staged = actors.clone();
            for id in removes.ids() {
                staged = without(&staged, ActorRemoval::Drop(id))?;
            }
            *actors = staged;
        }
        Event::ChestState(state) => {
            stage_container_view(inventory, state.container(), revision)?;
        }
        Event::FurnaceState(state) => {
            stage_container_view(inventory, state.container(), revision)?;
        }
        Event::ContainerClosed(closed) => {
            // A close naming an unheld container changes nothing: the close
            // tolerance the accepted oracles pin, so a mismatched or stale
            // close never disturbs another kind's live view.
            inventory.retire_container(&closed.container());
        }
        Event::CraftingState(state) => {
            *inventory = inventory.clone().with_crafting_view(state.size(), revision);
        }
        Event::Chat(event) => {
            *world_ui = world_ui.clone().with_chat(event.clone());
        }
        // The private body, command, placement, combat and personal
        // inventory observations carry no mirror-store identity: they are
        // complete accepted observations their family providers project from
        // the retained observation queue.
        Event::PlayerState(_)
        | Event::CommandRejected(_)
        | Event::PlaceBlockSucceeded(_)
        | Event::CombatHit(_)
        | Event::InventoryState(_) => {}
    }
    Ok(())
}

/// The held content revision of one chunk, if the mirror holds it.
fn held_revision(world: &WorldConfirmed, dimension: Dimension, chunk: ChunkPos) -> Option<&u64> {
    world
        .chunks()
        .get(&(dimension.get(), i64::from(chunk.x()), i64::from(chunk.z())))
}

/// Rejects a spawn naming an identity that is already live: removal precedes
/// reuse, so a live identity can only be replaced through despawn first.
fn reject_live_identity<T: PartialEq>(live: &[(T, Dimension)], id: &T) -> Result<(), ClientError> {
    if live.iter().any(|(held, _)| held == id) {
        return Err(ClientError::InvalidInput);
    }
    Ok(())
}

/// Resolves one dimensionless record against exactly one live typed
/// identity: zero matches is an orphan and more than one is ambiguous.
fn resolve_dimensioned<T: PartialEq>(
    live: &[(T, Dimension)],
    id: &T,
) -> Result<Dimension, ClientError> {
    let mut matches = live.iter().filter(|(held, _)| held == id);
    let (_, dimension) = matches.next().ok_or(ClientError::InvalidInput)?;
    if matches.next().is_some() {
        return Err(ClientError::InvalidInput);
    }
    Ok(*dimension)
}

/// Builds one prevalidated resolution entry; the kind and identity tags agree
/// by construction.
fn resolution(
    kind: ActorKind,
    id: ActorId,
    dimension: Dimension,
) -> Result<ResolvedActor, ClientError> {
    ResolvedActor::try_new(kind, id, dimension)
}

/// One identity a staged actor store rebuild removes.
enum ActorRemoval<'a> {
    RemotePlayer(&'a PlayerId),
    Companion(&'a mornlea_domain::CompanionId),
    Hostile(&'a HostileId),
    Passive(&'a PassiveId),
    Projectile(&'a ProjectileId),
    Drop(&'a DropId),
}

/// Rebuilds the actor store without one identity, preserving every live
/// identity of every other kind.
///
/// The frozen store exposes per-kind builders rather than removals, so a
/// removal rebuilds each kind's list; rebuilding from the whole store — not
/// from the one kind — is what keeps the other kinds' identities live.
fn without(
    actors: &ActorConfirmed,
    removal: ActorRemoval<'_>,
) -> Result<ActorConfirmed, ClientError> {
    let mut staged = ActorConfirmed::try_new()?;
    for (id, dimension) in actors.remote_players() {
        if !matches!(removal, ActorRemoval::RemotePlayer(removed) if removed == id) {
            staged = staged.with_remote_player(*id, *dimension);
        }
    }
    for (id, dimension) in actors.companions() {
        if !matches!(removal, ActorRemoval::Companion(removed) if removed == id) {
            staged = staged.with_companion(*id, *dimension);
        }
    }
    for (id, dimension) in actors.hostiles() {
        if !matches!(removal, ActorRemoval::Hostile(removed) if removed == id) {
            staged = staged.with_hostile(*id, *dimension);
        }
    }
    for (id, dimension) in actors.passives() {
        if !matches!(removal, ActorRemoval::Passive(removed) if removed == id) {
            staged = staged.with_passive(*id, *dimension);
        }
    }
    for (id, dimension) in actors.projectiles() {
        if !matches!(removal, ActorRemoval::Projectile(removed) if removed == id) {
            staged = staged.with_projectile(*id, *dimension);
        }
    }
    for id in actors.drops() {
        if !matches!(removal, ActorRemoval::Drop(removed) if removed == id) {
            staged = staged.with_drop(*id);
        }
    }
    Ok(staged)
}

/// Stages one container view at the confirmed revision it arrives with.
///
/// A newer generation at the same per-chunk position replaces every view the
/// position held — a re-placed container never inherits the old view — and a
/// generation a newer one already replaced is an old-generation observation
/// that rejects.
fn stage_container_view(
    inventory: &mut InventoryConfirmed,
    reference: ContainerRef,
    revision: ConfirmedRevision,
) -> Result<(), ClientError> {
    for (held, _) in inventory.containers() {
        if held.kind() == reference.kind()
            && held.chunk() == reference.chunk()
            && held.slot() == reference.slot()
            && held.generation() > reference.generation()
        {
            return Err(ClientError::InvalidInput);
        }
    }
    let mut staged = inventory.clone();
    for (held, _) in inventory.containers() {
        if held.kind() == reference.kind()
            && held.chunk() == reference.chunk()
            && held.slot() == reference.slot()
        {
            staged.retire_container(held);
        }
    }
    *inventory = staged.with_container_view(reference, revision);
    Ok(())
}

/// The optional server tick one semantic event carries, preserved exactly:
/// the tickless families — the world observations, the container and
/// crafting views, chat and the despawn records that never carried one —
/// stay `None` and no tick is ever invented for them.
fn source_tick(event: &Event) -> Option<u64> {
    match event {
        Event::RemotePlayerSpawn(event) => Some(event.server_tick()),
        Event::RemotePlayerStates(event) => Some(event.server_tick()),
        Event::CompanionSpawn(event) => Some(event.server_tick()),
        Event::CompanionStates(event) => Some(event.server_tick()),
        Event::HostileSpawn(event) => Some(event.server_tick()),
        Event::HostileState(event) => Some(event.server_tick()),
        Event::HostileDespawn(event) => Some(event.server_tick()),
        Event::PassiveSpawn(event) => Some(event.server_tick()),
        Event::PassiveState(event) => Some(event.server_tick()),
        Event::PassiveDespawn(event) => Some(event.server_tick()),
        Event::ProjectileSpawn(event) => Some(event.server_tick()),
        Event::ProjectileState(event) => Some(event.server_tick()),
        Event::ProjectileDespawn(event) => Some(event.server_tick()),
        Event::ItemDropUpserts(event) => Some(event.server_tick()),
        Event::ItemDropRemoves(event) => Some(event.server_tick()),
        Event::PlayerState(event) => Some(event.server_tick()),
        Event::CombatHit(event) => Some(event.server_tick()),
        Event::ChunkSnapshot(_)
        | Event::BlockChanges(_)
        | Event::ForgetChunks(_)
        | Event::CommandRejected(_)
        | Event::RemotePlayerDespawn(_)
        | Event::InventoryState(_)
        | Event::FurnaceState(_)
        | Event::ContainerClosed(_)
        | Event::ChestState(_)
        | Event::Chat(_)
        | Event::CompanionDespawn(_)
        | Event::PlaceBlockSucceeded(_)
        | Event::CraftingState(_) => None,
    }
}
