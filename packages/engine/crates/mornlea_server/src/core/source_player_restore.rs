//! Source login registration, exclusive restore scans, keyed Safe qualification and fixed trample capture.
use super::actor_placement::RestoreCandidate;
use super::contracts::{
    ActorAux, ActorKey, ActorLifecycle, ActorRuntime, RuleEffect, ServerError, SessionKey,
};
use super::login_seed::{SeededPlayer, seed_player};
use super::pending_restore::{PendingRestore, RestoreKind};
use super::state::{AuthorityReadView, TickContext};
use mornlea_domain::{BlockPos, ChunkPos, Dimension, FiniteVec3, MotionState, MotionStateParts};
use mornlea_storage::PlayerSave;
use std::collections::BTreeMap;

/// Exclusive live-session owner; scans are never cloned or reconstructed per tick.
#[derive(Default)]
pub(crate) struct SourcePlayerBook {
    pub(crate) entries: BTreeMap<SessionKey, SourcePlayerEntry>,
    tramples: SourceTrampleBatch,
}
/// Fixed copied coordinates survive actor reset and abandoned context loans.
/// Eight live source players contribute at most four cells each; inactive tail stays retained.
struct SourceTrampleBatch {
    cells: [crate::rules::crops::FootprintCell; 32],
    len: usize,
}
impl Default for SourceTrampleBatch {
    fn default() -> Self {
        Self {
            cells: [crate::rules::crops::FootprintCell {
                dimension: Dimension::OVERWORLD,
                pos: BlockPos::ORIGIN,
            }; 32],
            len: 0,
        }
    }
}
impl SourceTrampleBatch {
    fn append(&mut self, dimension: Dimension, positions: &[BlockPos]) -> Result<(), ServerError> {
        if positions.len() > 4 {
            return Err(ServerError::Internal {
                invariant: "source player trample cells",
            });
        }
        let len = self
            .len
            .checked_add(positions.len())
            .filter(|len| *len <= 32)
            .ok_or(ServerError::Internal {
                invariant: "source player trample capacity",
            })?;
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
    ) -> ([crate::rules::crops::FootprintCell; 32], usize) {
        (self.tramples.cells, self.tramples.len)
    }
}
pub(crate) struct SourcePlayerEntry {
    pub(crate) restore: PendingRestore,
    pub(crate) ever_spawned: bool,
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
    if book.tramples.len > 32 {
        return Err(ServerError::Internal {
            invariant: "source player trample capacity",
        });
    }
    let report = crate::rules::crops::settle_captured_tramples(
        &book.tramples.cells[..book.tramples.len],
        context,
    )?;
    book.tramples.len = 0;
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
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
                <= 32 * std::mem::size_of::<crate::rules::crops::FootprintCell>()
                    + std::mem::size_of::<usize>()
        );
    }
    #[test]
    fn trample_batch_capacity_is_atomic() {
        let mut batch = SourceTrampleBatch::default();
        for i in 0..8 {
            batch
                .append(Dimension::DEPTHS, &[BlockPos::new(i, 3, 4); 4])
                .unwrap();
        }
        assert_eq!(batch.len, 32);
        let before = (batch.cells, batch.len);
        assert_eq!(
            batch.append(Dimension::OVERWORLD, &[BlockPos::ORIGIN]),
            Err(ServerError::Internal {
                invariant: "source player trample capacity"
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
}
