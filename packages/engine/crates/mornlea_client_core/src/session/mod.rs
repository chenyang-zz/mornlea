//! The C1 session seam: the confirmed mirror contract and the provider
//! module roots.
//!
//! This module owns the shared session types every session provider consumes.
//! The confirmed mirror is a checked, read-only-by-contract value: it
//! privately owns the epoch, revision, session phase and the separate world,
//! actor, inventory and world-UI confirmed stores. Only the mirror provider
//! commits a validated complete observation and increments the revision; the
//! checked-parts constructor below is the only creation path, so the staging
//! policy stays inside that owner and no projection can mutate confirmed
//! state.

pub mod io;
pub mod lifecycle;
pub mod login;
pub mod mirror;

use std::collections::BTreeMap;

use mornlea_domain::{ChatEvent, ChunkPos, ContainerRef, CraftingSize, Dimension};

use crate::contracts::{ClientError, ConfirmedRevision, SessionEpoch, SessionPhase};

/// The confirmed world store: the dimensions and chunk columns the mirror
/// currently holds, with their latest content revisions.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct WorldConfirmed {
    chunks: BTreeMap<(u8, i64, i64), u64>,
}

impl WorldConfirmed {
    pub fn try_new(chunks: BTreeMap<(u8, i64, i64), u64>) -> Result<Self, ClientError> {
        Ok(Self { chunks })
    }

    pub fn chunk_count(&self) -> usize {
        self.chunks.len()
    }

    pub fn chunks(&self) -> &BTreeMap<(u8, i64, i64), u64> {
        &self.chunks
    }

    /// The checked constructor surface for staged world entries.
    pub fn with_chunk(mut self, dimension: Dimension, chunk: ChunkPos, revision: u64) -> Self {
        self.chunks.insert(
            (dimension.get(), i64::from(chunk.x()), i64::from(chunk.z())),
            revision,
        );
        self
    }

    /// The checked constructor surface for one confirmed chunk removal.
    pub fn remove_chunk(&mut self, dimension: Dimension, chunk: ChunkPos) {
        self.chunks
            .remove(&(dimension.get(), i64::from(chunk.x()), i64::from(chunk.z())));
    }
}

/// The confirmed actor store: the live typed identities per kind and their
/// resolved dimensions. A state or despawn packet without a dimension is
/// matched against exactly one live identity here; an orphan, ambiguous or
/// old-generation packet never enters.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ActorConfirmed {
    remote_players: Vec<(mornlea_domain::PlayerId, Dimension)>,
    hostiles: Vec<(mornlea_domain::HostileId, Dimension)>,
    passives: Vec<(mornlea_domain::PassiveId, Dimension)>,
    projectiles: Vec<(mornlea_domain::ProjectileId, Dimension)>,
    companions: Vec<(mornlea_domain::CompanionId, Dimension)>,
    drops: Vec<mornlea_domain::DropId>,
}

impl ActorConfirmed {
    pub fn try_new() -> Result<Self, ClientError> {
        Ok(Self::default())
    }

    pub fn remote_players(&self) -> &[(mornlea_domain::PlayerId, Dimension)] {
        &self.remote_players
    }

    pub fn hostiles(&self) -> &[(mornlea_domain::HostileId, Dimension)] {
        &self.hostiles
    }

    pub fn passives(&self) -> &[(mornlea_domain::PassiveId, Dimension)] {
        &self.passives
    }

    pub fn projectiles(&self) -> &[(mornlea_domain::ProjectileId, Dimension)] {
        &self.projectiles
    }

    pub fn companions(&self) -> &[(mornlea_domain::CompanionId, Dimension)] {
        &self.companions
    }

    pub fn drops(&self) -> &[mornlea_domain::DropId] {
        &self.drops
    }

    pub fn with_remote_player(
        mut self,
        id: mornlea_domain::PlayerId,
        dimension: Dimension,
    ) -> Self {
        self.remote_players.push((id, dimension));
        self
    }

    pub fn with_hostile(mut self, id: mornlea_domain::HostileId, dimension: Dimension) -> Self {
        self.hostiles.push((id, dimension));
        self
    }

    pub fn with_passive(mut self, id: mornlea_domain::PassiveId, dimension: Dimension) -> Self {
        self.passives.push((id, dimension));
        self
    }

    pub fn with_projectile(
        mut self,
        id: mornlea_domain::ProjectileId,
        dimension: Dimension,
    ) -> Self {
        self.projectiles.push((id, dimension));
        self
    }

    pub fn with_companion(mut self, id: mornlea_domain::CompanionId, dimension: Dimension) -> Self {
        self.companions.push((id, dimension));
        self
    }

    pub fn with_drop(mut self, id: mornlea_domain::DropId) -> Self {
        self.drops.push(id);
        self
    }
}

/// The confirmed inventory store: the current external container views and
/// crafting views with their local mirror attributions. These revisions are
/// client mirror attribution, not wire fields.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct InventoryConfirmed {
    containers: BTreeMap<ContainerRef, ConfirmedRevision>,
    crafting: Vec<(CraftingSize, ConfirmedRevision)>,
}

impl InventoryConfirmed {
    pub fn try_new() -> Result<Self, ClientError> {
        Ok(Self::default())
    }

    pub fn container_revision(&self, reference: &ContainerRef) -> Option<ConfirmedRevision> {
        self.containers.get(reference).copied()
    }

    pub fn crafting_revision(&self, size: CraftingSize) -> Option<ConfirmedRevision> {
        self.crafting
            .iter()
            .find(|(staged, _)| *staged == size)
            .map(|(_, revision)| *revision)
    }

    pub fn container_count(&self) -> usize {
        self.containers.len()
    }

    pub fn containers(&self) -> impl Iterator<Item = (&ContainerRef, ConfirmedRevision)> {
        self.containers
            .iter()
            .map(|(reference, revision)| (reference, *revision))
    }

    /// The checked constructor surface for one staged container view.
    pub fn with_container_view(
        mut self,
        reference: ContainerRef,
        revision: ConfirmedRevision,
    ) -> Self {
        self.containers.insert(reference, revision);
        self
    }

    /// The checked constructor surface for one confirmed container removal:
    /// the accepted `ContainerClosed` observation removes the view, which is
    /// what permits a local close tombstone to retire.
    pub fn retire_container(&mut self, reference: &ContainerRef) {
        self.containers.remove(reference);
    }

    /// The checked constructor surface for one staged crafting view. A size
    /// already staged is replaced, mirroring one confirmed view per size.
    pub fn with_crafting_view(mut self, size: CraftingSize, revision: ConfirmedRevision) -> Self {
        self.crafting.retain(|(staged, _)| *staged != size);
        self.crafting.push((size, revision));
        self
    }
}

/// The confirmed world-UI store: the accepted chat events in arrival order.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct WorldUiConfirmed {
    chat: Vec<ChatEvent>,
}

impl WorldUiConfirmed {
    pub fn try_new() -> Result<Self, ClientError> {
        Ok(Self::default())
    }

    pub fn chat(&self) -> &[ChatEvent] {
        &self.chat
    }

    pub fn with_chat(mut self, event: ChatEvent) -> Self {
        self.chat.push(event);
        self
    }
}

/// The validated-parts constructor input of one confirmed mirror.
#[derive(Clone, Debug, PartialEq)]
pub struct ConfirmedMirrorParts {
    pub epoch: SessionEpoch,
    pub revision: ConfirmedRevision,
    pub phase: SessionPhase,
    pub world: Option<WorldConfirmed>,
    pub actors: Option<ActorConfirmed>,
    pub inventory: Option<InventoryConfirmed>,
    pub world_ui: Option<WorldUiConfirmed>,
}

impl ConfirmedMirrorParts {
    /// The pending-connection parts: a fresh epoch at revision zero with no
    /// confirmed stores.
    pub fn pending(epoch: SessionEpoch) -> Self {
        Self {
            epoch,
            revision: ConfirmedRevision::new(0),
            phase: SessionPhase::Disconnected,
            world: None,
            actors: None,
            inventory: None,
            world_ui: None,
        }
    }
}

/// The confirmed client mirror: one epoch and revision beside the separate
/// world, actor, inventory and world-UI confirmed values. Projections read
/// immutable borrows; no method here mutates anything, and only the mirror
/// provider turns a complete accepted observation into the next revision.
#[derive(Clone, Debug, PartialEq)]
pub struct ConfirmedMirror {
    epoch: SessionEpoch,
    revision: ConfirmedRevision,
    phase: SessionPhase,
    world: WorldConfirmed,
    actors: ActorConfirmed,
    inventory: InventoryConfirmed,
    world_ui: WorldUiConfirmed,
}

impl ConfirmedMirror {
    /// Publishes a mirror from validated parts. The revision must agree with
    /// the phase: a pending connection is at revision zero, and only
    /// session, lifecycle, diagnostics and local input records may exist
    /// there, so a world store staged at revision zero is invalid.
    pub fn try_new(parts: ConfirmedMirrorParts) -> Result<Self, ClientError> {
        if parts.revision.get() == 0
            && parts
                .world
                .as_ref()
                .is_some_and(|world| world.chunk_count() > 0)
        {
            return Err(ClientError::InvalidInput);
        }
        Ok(Self {
            epoch: parts.epoch,
            revision: parts.revision,
            phase: parts.phase,
            world: parts.world.unwrap_or_default(),
            actors: parts.actors.unwrap_or_default(),
            inventory: parts.inventory.unwrap_or_default(),
            world_ui: parts.world_ui.unwrap_or_default(),
        })
    }

    pub fn epoch(&self) -> SessionEpoch {
        self.epoch
    }

    pub fn revision(&self) -> ConfirmedRevision {
        self.revision
    }

    pub fn phase(&self) -> &SessionPhase {
        &self.phase
    }

    pub fn world(&self) -> &WorldConfirmed {
        &self.world
    }

    pub fn actors(&self) -> &ActorConfirmed {
        &self.actors
    }

    pub fn inventory(&self) -> &InventoryConfirmed {
        &self.inventory
    }

    pub fn world_ui(&self) -> &WorldUiConfirmed {
        &self.world_ui
    }

    pub fn container_revision(&self, reference: &ContainerRef) -> Option<ConfirmedRevision> {
        self.inventory.container_revision(reference)
    }

    pub fn crafting_revision(&self, size: CraftingSize) -> Option<ConfirmedRevision> {
        self.inventory.crafting_revision(size)
    }
}
