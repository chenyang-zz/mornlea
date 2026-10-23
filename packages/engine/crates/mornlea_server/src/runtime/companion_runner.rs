//! Runtime plumbing for companion path searches.
//!
//! The core runner decides at a tick boundary and returns path requests
//! as effects. This module is the only runtime piece of that loop: it
//! runs each search over the frozen tick-boundary grid and holds the
//! outcomes until the caller delivers them to the authority at the
//! next tick boundary, so a search can never move a companion in the
//! tick that issued it (the Go asynchronous worker shape). Searches
//! are bounded by the grid the core built and by the per-companion
//! single-outstanding rule; an over-send is a hard error, never a
//! silent queue growth.

use std::collections::BTreeSet;

use mornlea_domain::{BlockPos, CompanionId};
use mornlea_engine::native::contracts::PathfindOp;
use mornlea_engine::native::contracts::pathfind::{PathCell, PathScratch};
use mornlea_engine::native::pathfind::NativePathfind;

use crate::core::companion_runner::{CompanionPathOutcome, CompanionPathRequest, PathOutcome};
use crate::core::contracts::{Resource, ServerError};
use crate::state::AuthorityState;

/// Maximum searches held at once: one per configured companion.
pub const MAX_PENDING_COMPANION_PATHS: usize = 4;

/// Held path outcomes plus the companions they belong to, so a second
/// request for the same companion is refused instead of queued.
#[derive(Debug, Default)]
pub struct PendingCompanionPaths {
    outcomes: Vec<CompanionPathOutcome>,
    companions: BTreeSet<CompanionId>,
}

impl PendingCompanionPaths {
    /// An empty holder.
    pub fn new() -> Self {
        Self::default()
    }

    /// Outcomes currently held for the next tick boundary.
    pub fn len(&self) -> usize {
        self.outcomes.len()
    }

    /// Whether nothing is held.
    pub fn is_empty(&self) -> bool {
        self.outcomes.is_empty()
    }

    /// Runs every request over its frozen grid and holds the outcomes.
    ///
    /// Refuses the whole batch before running any search when a
    /// companion already holds an outcome, a batch repeats a companion,
    /// or the batch would exceed the pending bound.
    pub fn issue(&mut self, requests: Vec<CompanionPathRequest>) -> Result<(), ServerError> {
        let mut seen = self.companions.clone();
        for request in &requests {
            if !seen.insert(request.companion) {
                return Err(ServerError::InvalidInput {
                    field: "companion_path_request",
                });
            }
        }
        if self.outcomes.len() + requests.len() > MAX_PENDING_COMPANION_PATHS {
            return Err(ServerError::Capacity {
                resource: Resource::Commands,
                limit: MAX_PENDING_COMPANION_PATHS,
                observed: self.outcomes.len() + requests.len(),
            });
        }
        for request in requests {
            let CompanionPathRequest {
                companion,
                generation,
                start,
                goal,
                grid,
            } = request;
            let cells = grid.size().iter().map(|size| *size as usize).product();
            let mut scratch =
                PathScratch::try_with_capacity(cells).map_err(|_| ServerError::Internal {
                    invariant: "companion path scratch",
                })?;
            let outcome = match NativePathfind.find(
                &grid,
                PathCell {
                    x: start.x(),
                    y: start.y(),
                    z: start.z(),
                },
                PathCell {
                    x: goal.x(),
                    y: goal.y(),
                    z: goal.z(),
                },
                &mut scratch,
            ) {
                Ok(result) => PathOutcome::Found {
                    waypoints: result
                        .waypoints()
                        .iter()
                        .map(|cell| BlockPos::new(cell.x, cell.y, cell.z))
                        .collect(),
                    revisions: result
                        .revisions()
                        .iter()
                        .map(|revision| {
                            (
                                mornlea_domain::ChunkPos::new(revision.chunk[0], revision.chunk[1]),
                                revision.revision,
                            )
                        })
                        .collect(),
                },
                Err(_) => PathOutcome::Failed,
            };
            self.companions.insert(companion);
            self.outcomes.push(CompanionPathOutcome {
                companion,
                generation,
                outcome,
            });
        }
        Ok(())
    }

    /// Drains the held outcomes for delivery at the next tick boundary.
    pub fn take_outcomes(&mut self) -> Vec<CompanionPathOutcome> {
        self.companions.clear();
        std::mem::take(&mut self.outcomes)
    }
}

/// One assembly pass a later tick loop can call: deliver the outcomes
/// held from the previous pass to the authority, then run the searches
/// the authority asks for and hold their outcomes for the next pass.
/// Movement and interaction actions from the authority pass are returned
/// untouched;
/// submitting them to the tick stays with the caller.
pub fn drive_companion_runners(
    state: &mut AuthorityState,
    pending: &mut PendingCompanionPaths,
) -> Result<Vec<(CompanionId, crate::core::contracts::CompanionAction)>, ServerError> {
    let delivered = pending.take_outcomes();
    let output = state.advance_companion_runners(delivered)?;
    pending.issue(output.path_requests)?;
    Ok(output.actions)
}

#[cfg(test)]
mod tests {
    use super::*;
    use mornlea_engine::native::contracts::pathfind::{PathBlockTable, PathGrid};

    fn companion(tag: u8) -> CompanionId {
        let mut bytes = [0u8; 16];
        bytes[0] = tag;
        bytes[6] = 0x40;
        bytes[8] = 0x80;
        CompanionId::try_from_bytes(bytes).unwrap()
    }

    fn flat_request(companion: CompanionId, goal_x: i32) -> CompanionPathRequest {
        // Three layers over a 5x1 footprint: solid floor at y=64, air
        // at y=65 and y=66, so every y=65 cell stands with headroom.
        let size = [5u32, 3, 1];
        // Kernel order is x-outer, z-middle, y-inner: floor then air
        // per column.
        let blocks: Vec<u16> = (0..5).flat_map(|_| [1u16, 0, 0]).collect();
        let table = PathBlockTable::from_passable_ids(&[0]).unwrap();
        let grid = PathGrid::try_new(
            PathCell { x: 0, y: 64, z: 0 },
            size,
            blocks.into_boxed_slice(),
            table,
            Vec::new(),
        )
        .unwrap();
        CompanionPathRequest {
            companion,
            generation: 1,
            start: BlockPos::new(0, 65, 0),
            goal: BlockPos::new(goal_x, 65, 0),
            grid,
        }
    }

    #[test]
    fn search_outcome_is_held_for_next_tick_delivery() {
        let mut pending = PendingCompanionPaths::new();
        pending.issue(vec![flat_request(companion(1), 4)]).unwrap();
        assert_eq!(pending.len(), 1);
        let outcomes = pending.take_outcomes();
        assert_eq!(outcomes.len(), 1);
        assert_eq!(outcomes[0].generation, 1);
        match &outcomes[0].outcome {
            PathOutcome::Found { waypoints, .. } => {
                assert_eq!(waypoints.first(), Some(&BlockPos::new(0, 65, 0)));
                assert_eq!(waypoints.last(), Some(&BlockPos::new(4, 65, 0)));
            }
            other => panic!("expected a found path, got {other:?}"),
        }
        assert!(pending.take_outcomes().is_empty());
    }

    #[test]
    fn unreachable_goal_holds_a_failure_outcome() {
        let mut pending = PendingCompanionPaths::new();
        // Goal outside the grid cannot stand, so the search fails.
        pending.issue(vec![flat_request(companion(1), 9)]).unwrap();
        let outcomes = pending.take_outcomes();
        assert_eq!(outcomes[0].outcome, PathOutcome::Failed);
    }

    #[test]
    fn duplicate_companion_request_is_refused_before_any_search() {
        let mut pending = PendingCompanionPaths::new();
        pending.issue(vec![flat_request(companion(1), 4)]).unwrap();
        assert!(matches!(
            pending.issue(vec![flat_request(companion(1), 2)]),
            Err(ServerError::InvalidInput {
                field: "companion_path_request"
            })
        ));
        assert_eq!(pending.len(), 1, "the held outcome is untouched");
    }
}
