//! Source login registration, exclusive scans, keyed Safe, retained trample/Snow capture and late action scalars.
use super::actor_placement::{RestoreCandidate, TRAMPLE_CELLS_PER_PLAYER};
use super::contracts::{
    ActorAux, ActorKey, ActorLifecycle, ActorRuntime, Resource, RuleEffect, ServerError, SessionKey,
};
use super::login_seed::{SeededPlayer, seed_player};
use super::pending_restore::{PendingRestore, RestoreKind};
use super::state::{AuthorityReadView, TickContext};
use crate::rules::crops::{SOURCE_SNOW_CAPACITY, SOURCE_TRAMPLE_CAPACITY};
use mornlea_domain::{BlockPos, ChunkPos, Dimension, FiniteVec3, MotionState, MotionStateParts};
use mornlea_storage::PlayerSave;
use std::collections::BTreeMap;

/// Exclusive live-session owner; scans are never cloned or reconstructed per tick.
#[derive(Default)]
pub(crate) struct SourcePlayerBook {
    pub(crate) entries: BTreeMap<SessionKey, SourcePlayerEntry>,
    tramples: SourceTrampleBatch,
    snow_pending: SourceSnowBatch,
}
/// Fixed copied coordinates survive actor reset and abandoned context loans.
/// Each live source player contributes at most one landing; inactive tail stays retained.
struct SourceTrampleBatch {
    cells: [crate::rules::crops::FootprintCell; SOURCE_TRAMPLE_CAPACITY],
    len: usize,
}
impl Default for SourceTrampleBatch {
    fn default() -> Self {
        Self {
            cells: [crate::rules::crops::FootprintCell {
                dimension: Dimension::OVERWORLD,
                pos: BlockPos::ORIGIN,
            }; SOURCE_TRAMPLE_CAPACITY],
            len: 0,
        }
    }
}
impl SourceTrampleBatch {
    fn append(&mut self, dimension: Dimension, positions: &[BlockPos]) -> Result<(), ServerError> {
        // One landing covering more than the box footprint is a geometry fault.
        if positions.len() > TRAMPLE_CELLS_PER_PLAYER {
            return Err(ServerError::Internal {
                invariant: "source player trample cells",
            });
        }
        // Overflow means more landing players than the player plane admits;
        // refuse with the typed capacity error and keep the prefix intact.
        let observed = self.len.saturating_add(positions.len());
        if observed > SOURCE_TRAMPLE_CAPACITY {
            return Err(ServerError::Capacity {
                resource: Resource::Players,
                limit: SOURCE_TRAMPLE_CAPACITY,
                observed,
            });
        }
        let len = observed;
        for (cell, pos) in self.cells[self.len..len].iter_mut().zip(positions) {
            *cell = crate::rules::crops::FootprintCell {
                dimension,
                pos: *pos,
            };
        }
        self.len = len;
        Ok(())
    }
}
impl SourcePlayerBook {
    #[cfg(test)]
    pub(crate) fn trample_test_snapshot(
        &self,
    ) -> (
        [crate::rules::crops::FootprintCell; SOURCE_TRAMPLE_CAPACITY],
        usize,
    ) {
        (self.tramples.cells, self.tramples.len)
    }
}
/// Inline travel and dimensionless cell memory belongs to one live player entry.
#[derive(Copy, Clone, Debug, PartialEq)]
struct SourceSnowTracker {
    travel: f32,
    cell: BlockPos,
    cell_valid: bool,
}
impl Default for SourceSnowTracker {
    fn default() -> Self {
        Self {
            travel: 0.,
            cell: BlockPos::ORIGIN,
            cell_valid: false,
        }
    }
}
impl SourceSnowTracker {
    fn preview(
        &self,
        before: [f32; 3],
        after: [f32; 3],
    ) -> Result<(Self, Option<BlockPos>), ServerError> {
        let mut next = *self;
        let dx = after[0] - before[0];
        let dz = after[2] - before[2];
        // Preserve source float squares and accumulation, with its widened square root.
        let distance = (f64::from(dx * dx + dz * dz).sqrt()) as f32;
        next.travel += distance;
        if next.travel < 0.6 {
            return Ok((next, None));
        }
        next.travel = 0.;
        let cell = super::actor_placement::snow_cell(after)?;
        if next.cell_valid && next.cell == cell {
            return Ok((next, None));
        }
        next.cell = cell;
        next.cell_valid = true;
        Ok((next, Some(cell)))
    }
}
/// Copied candidates retain original coordinates through reset and abandoned loans.
struct SourceSnowBatch {
    cells: [crate::rules::crops::FootprintCell; SOURCE_SNOW_CAPACITY],
    len: usize,
}
impl Default for SourceSnowBatch {
    fn default() -> Self {
        Self {
            cells: [crate::rules::crops::FootprintCell {
                dimension: Dimension::OVERWORLD,
                pos: BlockPos::ORIGIN,
            }; SOURCE_SNOW_CAPACITY],
            len: 0,
        }
    }
}
impl SourceSnowBatch {
    fn append(&mut self, dimension: Dimension, pos: BlockPos) -> Result<(), ServerError> {
        // One candidate per live player per tick; overflow is a typed refusal.
        if self.len >= SOURCE_SNOW_CAPACITY {
            return Err(ServerError::Capacity {
                resource: Resource::Players,
                limit: SOURCE_SNOW_CAPACITY,
                observed: self.len + 1,
            });
        }
        self.cells[self.len] = crate::rules::crops::FootprintCell { dimension, pos };
        self.len += 1;
        Ok(())
    }
}
impl SourcePlayerBook {
    #[cfg(test)]
    pub(crate) fn snow_test_snapshot(
        &self,
    ) -> (
        [crate::rules::crops::FootprintCell; SOURCE_SNOW_CAPACITY],
        usize,
    ) {
        (self.snow_pending.cells, self.snow_pending.len)
    }
    #[cfg(test)]
    pub(crate) fn snow_test_state(&self, session: SessionKey) -> Option<(f32, BlockPos, bool)> {
        self.entries
            .get(&session)
            .map(|e| (e.snow.travel, e.snow.cell, e.snow.cell_valid))
    }
    #[cfg(test)]
    pub(crate) fn snow_test_set_state(
        &mut self,
        session: SessionKey,
        travel: f32,
        cell: BlockPos,
        cell_valid: bool,
    ) {
        self.entries.get_mut(&session).unwrap().snow = SourceSnowTracker {
            travel,
            cell,
            cell_valid,
        };
    }
}
/// Preview and candidate acceptance precede committing retained travel.
pub(crate) fn capture_snow(
    book: &mut SourcePlayerBook,
    context: &TickContext<'_>,
    session: SessionKey,
) -> Result<(), ServerError> {
    let Some(entry) = book
        .entries
        .get_mut(&session)
        .filter(|entry| entry.ever_spawned)
    else {
        return Ok(());
    };
    let Some((dimension, before, after)) = context.capture_source_player_snow(session)? else {
        return Ok(());
    };
    let (next, candidate) = entry.snow.preview(before, after)?;
    if let Some(pos) = candidate {
        book.snow_pending.append(dimension, pos)?;
    }
    entry.snow = next;
    Ok(())
}
/// Successful fresh-cell settlement drains only the active candidate prefix.
pub(crate) fn settle_snow(
    book: &mut SourcePlayerBook,
    context: &mut TickContext<'_>,
) -> Result<crate::core::contracts::PhaseReport, ServerError> {
    if book.snow_pending.len > SOURCE_SNOW_CAPACITY {
        return Err(ServerError::Capacity {
            resource: Resource::Players,
            limit: SOURCE_SNOW_CAPACITY,
            observed: book.snow_pending.len,
        });
    }
    let report = crate::rules::crops::settle_captured_source_snow(
        &book.snow_pending.cells[..book.snow_pending.len],
        context,
    )?;
    book.snow_pending.len = 0;
    Ok(report)
}

pub(crate) struct SourcePlayerEntry {
    pub(crate) restore: PendingRestore,
    pub(crate) ever_spawned: bool,
    snow: SourceSnowTracker,
}
pub(crate) struct PreparedSourcePlayer {
    pub(crate) seeded: SeededPlayer,
    pub(crate) runtime: ActorRuntime,
    pub(crate) entry: SourcePlayerEntry,
}

/// Prepares every player owner before the handoff mutates any authority collection.
pub(crate) fn prepare(
    session: SessionKey,
    save: &PlayerSave,
    loaded_current: bool,
    spawn_dimension: Dimension,
    anchor: ChunkPos,
    radius: u8,
    view: &AuthorityReadView<'_>,
) -> Result<PreparedSourcePlayer, ServerError> {
    let mut seeded = seed_player(session, save)?;
    seeded.actor.lifecycle = ActorLifecycle::Pending;
    seeded.actor.dimension = spawn_dimension;
    seeded.actor.motion = MotionState::new(MotionStateParts {
        position: FiniteVec3::try_new([
            (anchor.x() as f32) * 16.0 + 0.5,
            321.0,
            (anchor.z() as f32) * 16.0 + 0.5,
        ])
        .map_err(|_| ServerError::InvalidInput { field: "position" })?,
        velocity: FiniteVec3::try_new([0.0; 3])
            .map_err(|_| ServerError::InvalidInput { field: "velocity" })?,
        on_ground: false,
    });
    // Missing canonical pose belongs to storage defaults and is never a restore candidate.
    let mut candidates = Vec::new();
    if loaded_current {
        candidates.push(candidate(&save.current, false)?);
    }
    if let Some(safe) = &save.safe {
        candidates.push(candidate(safe, true)?);
    }
    // Shared initialization preserves hunger and all transient defaults; only bed rounding differs.
    let mut runtime = crate::rules::player_survival::merged_runtime(view, &seeded.actor)?;
    // Source registration establishes the subscription independently of loaded geometry.
    runtime.has_view = true;
    runtime.aux = ActorAux::Player {
        respawn: source_bed(save),
        workbench: None,
    };
    Ok(PreparedSourcePlayer {
        seeded,
        runtime,
        entry: SourcePlayerEntry {
            restore: PendingRestore::try_new(
                RestoreKind::Player,
                spawn_dimension,
                anchor,
                radius,
                candidates,
            )?,
            ever_spawned: false,
            snow: SourceSnowTracker::default(),
        },
    })
}
fn candidate(
    location: &mornlea_storage::PlayerLocation,
    require_support: bool,
) -> Result<RestoreCandidate, ServerError> {
    let dimension = Dimension::new(
        u8::try_from(location.dimension)
            .map_err(|_| ServerError::InvalidInput { field: "dimension" })?,
    )
    .map_err(|_| ServerError::InvalidInput { field: "dimension" })?;
    Ok(RestoreCandidate {
        dimension,
        position: location.position,
        require_support,
    })
}
fn source_bed(save: &PlayerSave) -> Option<(Dimension, BlockPos)> {
    if !save.respawn_present {
        return None;
    }
    let dimension = Dimension::new(u8::try_from(save.respawn_dimension).ok()?).ok()?;
    let [x, y, z] = save.respawn_position;
    Some((
        dimension,
        BlockPos::new(round_block(x)?, round_block(y)?, round_block(z)?),
    ))
}
fn round_block(value: f32) -> Option<i32> {
    let value = f64::from(value).round();
    // Check in the wider representation before any narrowing conversion.
    if !value.is_finite() || !(f64::from(i32::MIN)..=f64::from(i32::MAX)).contains(&value) {
        return None;
    }
    Some(value as i32)
}
/// The complete scan remains in its bounded session owner after activation.
pub(crate) fn advance(
    book: &mut SourcePlayerBook,
    context: &mut TickContext<'_>,
) -> Result<(), ServerError> {
    for (session, entry) in &mut book.entries {
        if !context.source_player_session_active(*session) {
            continue;
        }
        let key = ActorKey::Player(*session);
        let effects = {
            let view = context.read();
            let actor = view.actor(key).ok_or(ServerError::Internal {
                invariant: "source player registration",
            })?;
            if actor.lifecycle == ActorLifecycle::Active {
                if !entry.ever_spawned {
                    return Err(ServerError::Internal {
                        invariant: "source player registration",
                    });
                }
                continue;
            }
            if actor.lifecycle != ActorLifecycle::Pending {
                continue;
            }
            let runtime = view.runtime(key).ok_or(ServerError::Internal {
                invariant: "source player runtime",
            })?;
            let environment = view.environment().ok_or(ServerError::Internal {
                invariant: "source player environment",
            })?;
            let super::pending_restore::RestoreProgress::Activated(chosen) = entry
                .restore
                .advance(&view, environment.tunables.eye_height())?
            else {
                continue;
            };
            let mut actor = actor.clone();
            actor.dimension = chosen.dimension;
            actor.lifecycle = ActorLifecycle::Active;
            actor.motion = MotionState::new(MotionStateParts {
                position: FiniteVec3::try_new(chosen.position).map_err(|_| {
                    ServerError::Internal {
                        invariant: "source player activation",
                    }
                })?,
                velocity: FiniteVec3::try_new([0.0; 3]).map_err(|_| ServerError::Internal {
                    invariant: "source player activation",
                })?,
                on_ground: chosen.on_ground,
            });
            let mut runtime = runtime.clone();
            runtime.controls = None;
            runtime.peak_y = chosen.position[1];
            runtime.reset = true;
            vec![RuleEffect::Actor(actor), RuleEffect::Runtime(runtime)]
        };
        // One constant-size compound publishes both prepared owners or neither.
        context
            .stage(RuleEffect::Compound(effects))
            .map_err(|_| ServerError::Internal {
                invariant: "source player activation",
            })?;
        entry.ever_spawned = true;
    }
    Ok(())
}

/// Restarts the same captured scan only after the indexed pair enters Pending.
/// Advancement already ran this tick; recovery never activates immediately.
pub(crate) fn recover(
    book: &mut SourcePlayerBook,
    context: &mut TickContext<'_>,
    session: SessionKey,
) -> Result<bool, ServerError> {
    let Some(entry) = book.entries.get_mut(&session) else {
        return Ok(false);
    };
    if !context.source_player_session_active(session) {
        return Ok(false);
    }
    if !entry.ever_spawned {
        return Err(ServerError::Internal {
            invariant: "source player registration",
        });
    }
    let Some(dimension) = context.recover_source_player(session, &entry.restore)? else {
        return Ok(false);
    };
    entry.snow = SourceSnowTracker::default();
    entry.restore.restart_player(dimension, Vec::new())?;
    Ok(true)
}

/// Settles live source deaths in session order and restarts each same captured scan.
/// Advancement already ran; accepted prefix mutations survive the first typed failure.
pub(crate) fn settle_deaths(
    book: &mut SourcePlayerBook,
    context: &mut TickContext<'_>,
) -> Result<(), ServerError> {
    for (session, entry) in &mut book.entries {
        if !entry.ever_spawned || !context.source_player_death_deferred(*session) {
            continue;
        }
        if let Some(reset) = context.settle_source_player_death(*session, &entry.restore)? {
            entry.snow = SourceSnowTracker::default();
            entry
                .restore
                .restart_player(reset.dimension, reset.candidate.into_iter().collect())?;
        }
    }
    Ok(())
}

/// Qualifies the existing per-player Safe sample without moving its retained scan owner.
pub(crate) fn checkpoint_safe(
    book: &SourcePlayerBook,
    context: &mut TickContext<'_>,
    session: SessionKey,
) -> Result<(), ServerError> {
    if !book
        .entries
        .get(&session)
        .is_some_and(|entry| entry.ever_spawned)
    {
        return Ok(());
    }
    context.checkpoint_source_player_safe(session)
}

/// A keyed registration check precedes borrowing one player's landing certificate.
pub(crate) fn capture_trample(
    book: &mut SourcePlayerBook,
    context: &TickContext<'_>,
    session: SessionKey,
) -> Result<(), ServerError> {
    if !book
        .entries
        .get(&session)
        .is_some_and(|entry| entry.ever_spawned)
    {
        return Ok(());
    }
    if let Some((dimension, positions, len)) = context.capture_source_player_trample(session)? {
        book.tramples.append(dimension, &positions[..len])?;
    }
    Ok(())
}
/// Only successful fresh-cell settlement drains the prefix; missing environment retains it.
pub(crate) fn settle_tramples(
    book: &mut SourcePlayerBook,
    context: &mut TickContext<'_>,
) -> Result<crate::core::contracts::PhaseReport, ServerError> {
    if book.tramples.len > SOURCE_TRAMPLE_CAPACITY {
        return Err(ServerError::Capacity {
            resource: Resource::Players,
            limit: SOURCE_TRAMPLE_CAPACITY,
            observed: book.tramples.len,
        });
    }
    let report = crate::rules::crops::settle_captured_tramples(
        &book.tramples.cells[..book.tramples.len],
        context,
    )?;
    book.tramples.len = 0;
    Ok(report)
}

/// Sorted live registrations qualify late costs without moving any scan owner.
/// Earlier scalar acceptance survives a later refusal; receipts remain tick-local.
pub(crate) fn settle_action_costs(
    book: &SourcePlayerBook,
    context: &mut TickContext<'_>,
) -> Result<crate::core::contracts::PhaseReport, ServerError> {
    let mut report = crate::core::contracts::PhaseReport {
        examined: 0,
        applied: 0,
        carried: 0,
        rejected: 0,
    };
    for (session, entry) in &book.entries {
        if !entry.ever_spawned {
            continue;
        }
        let next = context.settle_source_player_action_costs(*session)?;
        report.examined += next.examined;
        report.applied += next.applied;
        report.carried += next.carried;
        report.rejected += next.rejected;
    }
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::contracts::MAX_PLAYERS;
    #[test]
    fn bed_rounding_checks_half_away_edges_and_drops_invalid_records() {
        for (input, expected) in [
            (-1.5, -2),
            (-0.5, -1),
            (-0.49, 0),
            (0.49, 0),
            (0.5, 1),
            (1.5, 2),
            (-2147483648., i32::MIN),
            (2147483520., 2147483520),
        ] {
            assert_eq!(round_block(input), Some(expected));
        }
        for input in [
            f32::NAN,
            f32::INFINITY,
            f32::NEG_INFINITY,
            2147483648.,
            -2147483904.,
        ] {
            assert_eq!(round_block(input), None);
        }
        let mut save = mornlea_storage::PlayerSave {
            player_id: mornlea_storage::PlayerId::from_bytes([1; 16]),
            revision: 1,
            display_name: "Ada".into(),
            current: mornlea_storage::PlayerLocation {
                dimension: 0,
                position: [0., 64., 0.],
            },
            yaw: 0.,
            pitch: 0.,
            safe: None,
            inventory: Default::default(),
            health: 20,
            hunger: 20,
            saturation_milli: 5000,
            exhaustion_milli: 0,
            respawn_present: true,
            respawn_position: [0.5, -0.5, 1.5],
            respawn_dimension: 1,
            armor: Default::default(),
        };
        assert_eq!(
            source_bed(&save),
            Some((Dimension::DEPTHS, BlockPos::new(1, -1, 2)))
        );
        save.respawn_dimension = 256;
        assert_eq!(source_bed(&save), None);
        save.respawn_dimension = 0;
        save.respawn_position[0] = f32::INFINITY;
        assert_eq!(source_bed(&save), None);
        save.respawn_position = [0.; 3];
        save.respawn_present = false;
        assert_eq!(source_bed(&save), None);
    }

    #[test]
    fn trample_batch_preserves_order_and_duplicates() {
        let mut batch = SourceTrampleBatch::default();
        let first = [
            BlockPos::new(2, 3, 4),
            BlockPos::ORIGIN,
            BlockPos::new(2, 3, 4),
            BlockPos::new(-1, 8, 9),
        ];
        let second = [BlockPos::new(5, 6, 7); 4];
        batch.append(Dimension::OVERWORLD, &first).unwrap();
        batch.append(Dimension::DEPTHS, &second).unwrap();
        assert_eq!(batch.len, 8);
        for (i, pos) in first.iter().chain(second.iter()).enumerate() {
            assert_eq!(
                batch.cells[i],
                crate::rules::crops::FootprintCell {
                    dimension: if i < 4 {
                        Dimension::OVERWORLD
                    } else {
                        Dimension::DEPTHS
                    },
                    pos: *pos
                }
            );
        }
        assert_eq!(batch.cells[8..], SourceTrampleBatch::default().cells[8..]);
        assert!(
            std::mem::size_of::<SourceTrampleBatch>()
                <= SOURCE_TRAMPLE_CAPACITY
                    * std::mem::size_of::<crate::rules::crops::FootprintCell>()
                    + std::mem::size_of::<usize>()
        );
    }
    #[test]
    fn trample_batch_capacity_is_atomic() {
        let mut batch = SourceTrampleBatch::default();
        for i in 0..i32::from(MAX_PLAYERS) {
            batch
                .append(
                    Dimension::DEPTHS,
                    &[BlockPos::new(i, 3, 4); TRAMPLE_CELLS_PER_PLAYER],
                )
                .unwrap();
        }
        assert_eq!(batch.len, SOURCE_TRAMPLE_CAPACITY);
        assert_eq!(SOURCE_TRAMPLE_CAPACITY, 32);
        let before = (batch.cells, batch.len);
        // Overflow is a typed player-plane capacity refusal, never an internal fault.
        assert_eq!(
            batch.append(Dimension::OVERWORLD, &[BlockPos::ORIGIN]),
            Err(ServerError::Capacity {
                resource: Resource::Players,
                limit: SOURCE_TRAMPLE_CAPACITY,
                observed: SOURCE_TRAMPLE_CAPACITY + 1,
            })
        );
        assert_eq!((batch.cells, batch.len), before);
        assert_eq!(
            batch.append(Dimension::OVERWORLD, &[BlockPos::ORIGIN; 3]),
            Err(ServerError::Capacity {
                resource: Resource::Players,
                limit: SOURCE_TRAMPLE_CAPACITY,
                observed: SOURCE_TRAMPLE_CAPACITY + 3,
            })
        );
        assert_eq!((batch.cells, batch.len), before);
        assert_eq!(batch.append(Dimension::OVERWORLD, &[]), Ok(()));
        assert_eq!((batch.cells, batch.len), before);
        assert_eq!(
            batch.append(Dimension::OVERWORLD, &[BlockPos::ORIGIN; 5]),
            Err(ServerError::Internal {
                invariant: "source player trample cells"
            })
        );
        assert_eq!((batch.cells, batch.len), before);
    }
    #[test]
    fn snow_tracker_source_stride_and_memory() {
        let mut tracker = SourceSnowTracker::default();
        let mut x = 8.1f32;
        for tick in 1..=18 {
            let next = x + f32::from_bits(0x3d999980);
            let (updated, cell) = tracker.preview([x, 64., 8.5], [next, 64., 8.5]).unwrap();
            tracker = updated;
            x = next;
            if tick == 8 {
                assert_eq!(tracker.travel.to_bits(), 0x3f199980);
            }
            if tick == 9 || tick == 18 {
                assert_eq!(
                    cell,
                    Some(BlockPos::new(if tick == 9 { 8 } else { 9 }, 64, 8))
                );
                assert_eq!(tracker.travel, 0.);
            } else {
                assert_eq!(cell, None);
            }
        }
        let (next, cell) = tracker.preview([9., 64., 8.5], [9.7, 64., 8.5]).unwrap();
        assert_eq!(cell, None);
        assert_eq!(next.travel, 0.);
        let (next, cell) = next.preview([9.7, 64., 8.5], [10.4, 64., 8.5]).unwrap();
        assert_eq!(cell, Some(BlockPos::new(10, 64, 8)));
        assert_eq!(next.cell, cell.unwrap());
    }
    #[test]
    fn snow_tracker_checked_preview_is_atomic() {
        let tracker = SourceSnowTracker {
            travel: 0.55,
            cell: BlockPos::new(4, 64, 8),
            cell_valid: true,
        };
        let before = tracker;
        assert_eq!(
            tracker.preview([8., 64., 8.], [f32::MAX, 64., 8.]),
            Err(ServerError::InvalidInput {
                field: "actor_geometry"
            })
        );
        assert_eq!(tracker, before);
        let (next, cell) = tracker.preview([8., 64., 8.], [8.1, 64., 8.]).unwrap();
        assert_eq!(cell, Some(BlockPos::new(8, 64, 8)));
        assert_eq!(next.travel, 0.);
        assert_eq!(next.cell, cell.unwrap());
        assert_eq!(tracker, before);
    }
    #[test]
    fn snow_batch_order_and_capacity() {
        let mut batch = SourceSnowBatch::default();
        let cells: Vec<_> = (0..8)
            .map(|i| crate::rules::crops::FootprintCell {
                dimension: if i % 2 == 0 {
                    Dimension::OVERWORLD
                } else {
                    Dimension::DEPTHS
                },
                pos: BlockPos::new(i / 2, 64, 8),
            })
            .collect();
        for cell in &cells {
            batch.append(cell.dimension, cell.pos).unwrap();
        }
        assert_eq!(batch.len, 8);
        assert_eq!(batch.cells.as_slice(), cells.as_slice());
        assert!(
            std::mem::size_of::<SourceSnowBatch>()
                <= SOURCE_SNOW_CAPACITY * std::mem::size_of::<crate::rules::crops::FootprintCell>()
                    + std::mem::size_of::<usize>()
        );
        let before = (batch.cells, batch.len);
        assert_eq!(SOURCE_SNOW_CAPACITY, usize::from(MAX_PLAYERS));
        assert_eq!(
            batch.append(Dimension::OVERWORLD, BlockPos::ORIGIN),
            Err(ServerError::Capacity {
                resource: Resource::Players,
                limit: SOURCE_SNOW_CAPACITY,
                observed: SOURCE_SNOW_CAPACITY + 1,
            })
        );
        assert_eq!((batch.cells, batch.len), before);
    }
}
