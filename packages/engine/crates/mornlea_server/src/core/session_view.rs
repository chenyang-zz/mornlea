//! Per-session publication view state for the tick-end projection.
//!
//! One [`SessionView`] exists per admitted session inside the authority. It
//! carries the previous tick's derived interest and published snapshots, so
//! the projection can diff chunk interest, entity visibility and record
//! families without touching the resident state it observes. Entries are
//! created lazily by the projection and removed when a session retires.

use std::collections::{BTreeMap, BTreeSet};

use mornlea_domain::{
    CompanionId, CraftingSize, DropId, HostileId, InventoryState, ItemDrop, PassiveId, PlayerId,
    ProjectileId,
};

use crate::contracts::{ChunkKey, SessionKey};

/// One chunk column's publication state on a single session's mirror.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct ChunkPublication {
    /// Whether the session ever received a full snapshot for this column.
    pub(crate) snapshot_sent: bool,
    /// The revision the session's mirror holds for this column.
    pub(crate) last_revision: u64,
    /// A provider resync is waiting to re-send the full snapshot this tick.
    pub(crate) resync_queued: bool,
}

/// The previous tick's derived interest and published snapshots of one
/// session. All fields are owned copies. Inventory and crafting mirrors advance
/// only after the actual frame enters the FIFO; other families retain their
/// current projection-owned migration behavior.
#[derive(Clone, Debug, Default)]
pub(crate) struct SessionView {
    /// This session's chunk interest as derived from its player actor.
    pub(crate) wanted: BTreeSet<ChunkKey>,
    /// Publication state per wanted chunk column.
    pub(crate) chunks: BTreeMap<ChunkKey, ChunkPublication>,
    /// Last tick's visible companion identities.
    pub(crate) visible_companions: BTreeSet<CompanionId>,
    /// Last tick's visible remote identities and authority incarnations.
    /// Session keys never repeat within the authority that owns this view.
    pub(crate) visible_remotes: BTreeMap<PlayerId, SessionKey>,
    /// Last tick's visible hostile identities.
    pub(crate) visible_hostiles: BTreeSet<HostileId>,
    /// Last tick's visible passive identities.
    pub(crate) visible_passives: BTreeSet<PassiveId>,
    /// Last tick's visible projectile identities.
    pub(crate) visible_projectiles: BTreeSet<ProjectileId>,
    /// Last emitted dropped-stack wire values. Authority-only age and pickup
    /// timers never participate in this diff.
    pub(crate) visible_drops: BTreeMap<DropId, ItemDrop>,
    /// Last queue-admitted inventory wire value; grid and armor have separate intent.
    pub(crate) last_inventory: Option<InventoryState>,
    /// Last queue-admitted crafting grid and size.
    pub(crate) last_crafting: Option<([mornlea_storage::ItemStack; 9], CraftingSize)>,
}
