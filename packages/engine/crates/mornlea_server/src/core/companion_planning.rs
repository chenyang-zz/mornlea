//! Read-only companion planning projections over settled authority state.
//!
//! Both projections port the Go companion snapshot path one to one: the dense
//! 33x17x33 terrain window around the companion's floor cell, the exposed
//! block summary derived from that frozen window, the 3x3 ready chunk
//! revisions, the bounded online player set, the companion inventory, the
//! current task status and the world time. The planning snapshot feeds the
//! Agent request; the current world is the same projection rebuilt when a
//! planner outcome arrives, so plan install compares like with like.

use super::*;
use mornlea_storage::ItemStack;

use crate::core::companion_chat::CompanionChatPhase;

/// Window extent in blocks along x, y and z; the snapshot contract checks it.
const WINDOW: [u8; 3] = [33, 17, 33];
const WINDOW_X: usize = WINDOW[0] as usize;
const WINDOW_Y: usize = WINDOW[1] as usize;
const WINDOW_Z: usize = WINDOW[2] as usize;
const WINDOW_COLUMNS: usize = WINDOW_X * WINDOW_Z;
const RADIUS_XZ: i32 = (WINDOW_X as i32 - 1) / 2;
const RADIUS_Y: i32 = (WINDOW_Y as i32 - 1) / 2;
const WORLD_MIN_Y: i32 = -64;
const WORLD_MAX_Y: i32 = 320;
/// Ready-but-empty column height sentinel.
const EMPTY_COLUMN_HEIGHT: i32 = WORLD_MIN_Y - 1;
/// Exposed block summary cap; traversal stops once it is reached.
const MAX_EXPOSED_BLOCKS: usize = 256;

/// Frozen terrain window and the facts derived from it.
struct PlanningProjection {
    origin: BlockPos,
    ready_columns: Vec<u8>,
    heights: Vec<i16>,
    blocks: Vec<u16>,
    exposed: Vec<SnapshotBlock>,
    revisions: Vec<SnapshotChunkRevision>,
}

impl PlanningProjection {
    fn column_index(&self, x: i32, z: i32) -> Option<usize> {
        let dx = i64::from(x) - i64::from(self.origin.x());
        let dz = i64::from(z) - i64::from(self.origin.z());
        if !(0..WINDOW_X as i64).contains(&dx) || !(0..WINDOW_Z as i64).contains(&dz) {
            return None;
        }
        Some(dx as usize * WINDOW_Z + dz as usize)
    }

    fn column_ready(&self, column: usize) -> bool {
        self.ready_columns[column / 8] & (1 << (column % 8)) != 0
    }

    /// Frozen block at a ready, world-valid window cell; anything else fails
    /// closed rather than exposing normalized air.
    fn lookup(&self, pos: BlockPos) -> Option<u16> {
        if !(WORLD_MIN_Y..WORLD_MAX_Y).contains(&pos.y()) {
            return None;
        }
        let column = self.column_index(pos.x(), pos.z())?;
        let dy = i64::from(pos.y()) - i64::from(self.origin.y());
        if !(0..WINDOW_Y as i64).contains(&dy) || !self.column_ready(column) {
            return None;
        }
        let (dx, dz) = (column / WINDOW_Z, column % WINDOW_Z);
        Some(self.blocks[(dx * WINDOW_Y + dy as usize) * WINDOW_Z + dz])
    }

    /// Neighbors outside the world height count as air; neighbors outside
    /// the window or in unready columns are unknown and count as solid.
    fn has_air_neighbor(&self, pos: BlockPos) -> bool {
        let (x, y, z) = (pos.x(), pos.y(), pos.z());
        [
            (x - 1, y, z),
            (x + 1, y, z),
            (x, y - 1, z),
            (x, y + 1, z),
            (x, y, z - 1),
            (x, y, z + 1),
        ]
        .into_iter()
        .any(|(x, y, z)| {
            !(WORLD_MIN_Y..WORLD_MAX_Y).contains(&y)
                || self.lookup(BlockPos::new(x, y, z)) == Some(0)
        })
    }

    /// Exposed non-air cells in `(x, y, z)` order, capped at the summary bound.
    fn derive_exposed(&mut self) {
        let mut exposed = Vec::with_capacity(MAX_EXPOSED_BLOCKS);
        'scan: for dx in 0..WINDOW_X as i32 {
            for dy in 0..WINDOW_Y as i32 {
                for dz in 0..WINDOW_Z as i32 {
                    let pos = BlockPos::new(
                        self.origin.x() + dx,
                        self.origin.y() + dy,
                        self.origin.z() + dz,
                    );
                    let Some(block) = self.lookup(pos) else {
                        continue;
                    };
                    if block == 0 || !self.has_air_neighbor(pos) {
                        continue;
                    }
                    exposed.push(SnapshotBlock {
                        position: pos,
                        block_id: block,
                    });
                    if exposed.len() == MAX_EXPOSED_BLOCKS {
                        break 'scan;
                    }
                }
            }
        }
        self.exposed = exposed;
    }
}

/// Live companion facts the projections share.
struct PlanningCompanion<'a> {
    actor: &'a ActorRecord,
    inventory: [ItemStack; 36],
}

fn terrain_error() -> ServerError {
    ServerError::InvalidInput {
        field: "planning_terrain",
    }
}

fn block_y_valid(pos: Option<BlockPos>) -> bool {
    pos.is_none_or(|pos| (WORLD_MIN_Y..WORLD_MAX_Y).contains(&pos.y()))
}

impl AuthorityState {
    /// Planning snapshot for one companion's current task.
    ///
    /// Instruction and issuer come from the task frozen at ingress; terrain,
    /// revisions, players, inventory, status and time are read from settled
    /// authority now. Work is bounded by the fixed window and player cap.
    ///
    /// The source tick is `next_tick`, the number of completed ticks. Go's
    /// `PlanSnapshot` has no tick field; Go builds the snapshot in
    /// `dispatchPlanning` before `engine.StepWithTunables`, where the authority
    /// tick is `engine.TickCount()`, the same completed-step count.
    pub fn companion_planning_snapshot(
        &self,
        companion: CompanionId,
    ) -> Result<PlanningSnapshot, ServerError> {
        let view = self.settled_read()?;
        let task =
            self.companion_chat
                .current_task(companion)
                .ok_or(ServerError::InvalidInput {
                    field: "planning_task",
                })?;
        let live = self.planning_companion(&view, companion)?;
        let projection = planning_projection(&view, live.actor)?;
        let issuer = &task.issuer;
        if !block_y_valid(issuer.look_hit) {
            return Err(ServerError::InvalidInput {
                field: "planning_issuer",
            });
        }
        let task_status = match task.phase {
            CompanionChatPhase::Queued => SnapshotTaskStatusText::queued(),
            CompanionChatPhase::Planning => SnapshotTaskStatusText::planning(),
            CompanionChatPhase::Running => SnapshotTaskStatusText::running(),
        };
        let terrain = SnapshotTerrain::try_new(
            projection.origin,
            WINDOW,
            projection.ready_columns,
            projection.heights,
            projection.blocks,
        )?;
        PlanningSnapshot::try_new(
            view.tick(),
            view.world_time(),
            task.command.clone(),
            SnapshotIssuer {
                player_id: issuer.player_id,
                position: issuer.position,
                look: issuer.look,
                look_hit: issuer.look_hit,
            },
            SnapshotCompanion {
                companion_id: companion,
                position: live.actor.motion.position(),
                look: live.actor.look,
                task_status,
                inventory: live.inventory,
            },
            self.planning_players(&view),
            projection.revisions,
            projection.exposed,
            terrain,
        )
    }

    /// Current-world view for revalidating one companion's planner outcome.
    ///
    /// Rebuilds the same projection as the planning snapshot from current
    /// authority. Every ready, world-valid window cell is present; absent
    /// cells and chunks fail closed in plan install. `tick` is the same
    /// completed-tick count as the planning snapshot's source tick.
    ///
    /// An `InvalidInput` refusal (inactive companion, missing or invalid
    /// inventory, a window past the coordinate range, or an unreadable ready
    /// cell) means only that this companion's world changed: the caller
    /// discards the plan as world changed, like Go
    /// `plannerOutcomeMatchesCurrentAuthority` returning false. It concerns
    /// this companion only and is never an authority error; the caller must
    /// not fence the tick or fail the server on it. Only the settled read
    /// itself can return an authority state error (an already fenced or
    /// closed authority), which this projection passes through unchanged.
    pub fn companion_current_world(
        &self,
        companion: CompanionId,
    ) -> Result<CurrentWorld, ServerError> {
        let view = self.settled_read()?;
        let live = self.planning_companion(&view, companion)?;
        let projection = planning_projection(&view, live.actor)?;
        let mut blocks = BTreeMap::new();
        for dx in 0..WINDOW_X as i32 {
            for dy in 0..WINDOW_Y as i32 {
                for dz in 0..WINDOW_Z as i32 {
                    let pos = BlockPos::new(
                        projection.origin.x() + dx,
                        projection.origin.y() + dy,
                        projection.origin.z() + dz,
                    );
                    if let Some(block) = projection.lookup(pos) {
                        blocks.insert(pos, block);
                    }
                }
            }
        }
        Ok(CurrentWorld {
            tick: view.tick(),
            blocks,
            chunk_revisions: projection
                .revisions
                .iter()
                .map(|revision| (revision.pos, revision.revision))
                .collect(),
            inventory: live.inventory,
            online_players: self
                .planning_players(&view)
                .into_iter()
                .map(|player| (player.player_id, player.position.get()))
                .collect(),
        })
    }

    /// Active companion actor with a canonical inventory.
    fn planning_companion<'a>(
        &self,
        view: &AuthorityReadView<'a>,
        companion: CompanionId,
    ) -> Result<PlanningCompanion<'a>, ServerError> {
        let refused = ServerError::InvalidInput {
            field: "planning_companion",
        };
        let key = ActorKey::Companion(companion);
        let actor = view
            .actor(key)
            .filter(|actor| actor.lifecycle == ActorLifecycle::Active)
            .ok_or(refused)?;
        let inventory = view.inventory(key).ok_or(refused)?.slots;
        if !inventory.iter().all(ItemStack::is_valid) {
            return Err(refused);
        }
        Ok(PlanningCompanion { actor, inventory })
    }

    /// Online players sorted by id bytes, deduplicated and capped. Look hits
    /// are not sampled: follow validation reads only id and position.
    fn planning_players(&self, view: &AuthorityReadView<'_>) -> Vec<SnapshotPlayer> {
        let mut players: Vec<SnapshotPlayer> = view
            .online_players()
            .filter_map(|actor| {
                let ActorKey::Player(session) = actor.key else {
                    return None;
                };
                let record = self.sessions.get(&session)?;
                Some(SnapshotPlayer {
                    player_id: record.player_id,
                    position: actor.motion.position(),
                    look: actor.look,
                    look_hit: None,
                })
            })
            .collect();
        players.sort_by_key(|player| player.player_id.bytes());
        players.dedup_by_key(|player| player.player_id);
        players.truncate(MAX_PLAYERS as usize);
        players
    }
}

/// Dense window around the companion floor cell plus the derived summaries.
fn planning_projection(
    view: &AuthorityReadView<'_>,
    actor: &ActorRecord,
) -> Result<PlanningProjection, ServerError> {
    let [x, y, z] = actor
        .motion
        .position()
        .get()
        .map(|axis| f64::from(axis).floor());
    let horizontal = f64::from(i32::MIN + RADIUS_XZ)..=f64::from(i32::MAX - RADIUS_XZ);
    let vertical = f64::from(i32::MIN + RADIUS_Y)..=f64::from(i32::MAX - RADIUS_Y);
    if !horizontal.contains(&x) || !vertical.contains(&y) || !horizontal.contains(&z) {
        return Err(terrain_error());
    }
    let (center_x, center_y, center_z) = (x as i32, y as i32, z as i32);
    let dimension = actor.dimension;
    let mut projection = PlanningProjection {
        origin: BlockPos::new(
            center_x - RADIUS_XZ,
            center_y - RADIUS_Y,
            center_z - RADIUS_XZ,
        ),
        ready_columns: vec![0; WINDOW_COLUMNS.div_ceil(8)],
        heights: vec![EMPTY_COLUMN_HEIGHT as i16; WINDOW_COLUMNS],
        blocks: vec![0; WINDOW_COLUMNS * WINDOW_Y],
        exposed: Vec::new(),
        revisions: Vec::with_capacity(9),
    };
    let origin = projection.origin;
    for dx in 0..WINDOW_X {
        let x = origin.x() + dx as i32;
        for dz in 0..WINDOW_Z {
            let z = origin.z() + dz as i32;
            let key = ChunkKey {
                dimension,
                pos: ChunkPos::new(x >> 4, z >> 4),
            };
            if !view.ready_chunk(key) {
                continue;
            }
            let height = view
                .highest_non_air(dimension, x, z)
                .ok_or_else(terrain_error)?;
            if height != EMPTY_COLUMN_HEIGHT && !(WORLD_MIN_Y..WORLD_MAX_Y).contains(&height) {
                return Err(terrain_error());
            }
            let column = dx * WINDOW_Z + dz;
            projection.ready_columns[column / 8] |= 1 << (column % 8);
            projection.heights[column] = height as i16;
            for dy in 0..WINDOW_Y {
                let y = origin.y() + dy as i32;
                if !(WORLD_MIN_Y..WORLD_MAX_Y).contains(&y) {
                    continue;
                }
                let block = view
                    .block(dimension, BlockPos::new(x, y, z))
                    .ok_or_else(terrain_error)?;
                if !mornlea_domain::registered_block(block) {
                    return Err(terrain_error());
                }
                projection.blocks[(dx * WINDOW_Y + dy) * WINDOW_Z + dz] = block;
            }
        }
    }
    projection.derive_exposed();
    let (center_chunk_x, center_chunk_z) = (center_x >> 4, center_z >> 4);
    for dx in -1..=1 {
        for dz in -1..=1 {
            let pos = ChunkPos::new(center_chunk_x + dx, center_chunk_z + dz);
            let key = ChunkKey { dimension, pos };
            if !view.ready_chunk(key) {
                continue;
            }
            let revision = view.ready_chunk_revision(key).ok_or_else(terrain_error)?;
            projection
                .revisions
                .push(SnapshotChunkRevision { pos, revision });
        }
    }
    Ok(projection)
}

#[cfg(test)]
#[path = "companion_planning_tests.rs"]
pub(crate) mod tests;
