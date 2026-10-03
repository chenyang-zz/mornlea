//! Bounded borrowed source geometry for restoration, Safe checkpoints, trample capture and spawn reads.
//! Actor lifecycle, subscriptions and scan cadence remain caller-owned.

use super::contracts::{ChunkKey, ServerError};
use super::state::AuthorityReadView;
use crate::rules::player_motion::collision_cell;
use mornlea_domain::{BlockPos, ChunkPos, Dimension};
use mornlea_engine::native::contracts::collision::Aabb;

/// Borrowed current world observations; no lifecycle or retained body ownership.
pub trait PlacementWorld {
    fn ready_revision(&self, key: ChunkKey) -> Option<u64>;
    fn block_at(&self, dimension: Dimension, pos: BlockPos) -> Option<u16>;
    /// A complete Ready column's exact current highest non-air cell, or -65 for air.
    /// Sparse observations and revision equality cannot establish this certificate.
    fn ready_column_height(&self, _dimension: Dimension, _x: i32, _z: i32) -> Option<i32> {
        None
    }
}
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RestoreCandidate {
    pub dimension: Dimension,
    pub position: [f32; 3],
    pub require_support: bool,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RestoreCheck {
    pub valid: bool,
    pub ready: bool,
    pub on_ground: bool,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BodySpace {
    pub free: bool,
    pub ready: bool,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SupportContact {
    pub complete: bool,
    pub any: bool,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct SpawnColumn {
    pub x: i32,
    pub z: i32,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SpawnTier {
    Dry,
    EyeDry,
    Submerged,
}
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SpawnSite {
    pub position: [f32; 3],
    pub tier: SpawnTier,
}
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ColumnSpawn {
    pub ready: bool,
    pub site: Option<SpawnSite>,
}

impl PlacementWorld for AuthorityReadView<'_> {
    fn ready_revision(&self, key: ChunkKey) -> Option<u64> {
        self.ready_chunk_revision(key)
    }
    fn block_at(&self, dimension: Dimension, pos: BlockPos) -> Option<u16> {
        // Only placement reads normalize source out-of-height air, before readiness.
        if !(-64..320).contains(&pos.y()) {
            return Some(0);
        }
        self.observation(dimension, pos).map(|v| v.block)
    }
    fn ready_column_height(&self, dimension: Dimension, x: i32, z: i32) -> Option<i32> {
        self.highest_non_air(dimension, x, z)
    }
}

/// Horizontal retention precedes pose validation, so Y is deliberately ignored.
pub fn candidate_chunks(candidate: RestoreCandidate) -> Result<Vec<ChunkKey>, ServerError> {
    let x = span(
        candidate.position[0] - HALF_WIDTH,
        candidate.position[0] + HALF_WIDTH,
        2,
    )?;
    let z = span(
        candidate.position[2] - HALF_WIDTH,
        candidate.position[2] + HALF_WIDTH,
        2,
    )?;
    let mut keys = Vec::with_capacity(4);
    for x in x.lower..=x.upper {
        for z in z.lower..=z.upper {
            keys.push(ChunkKey {
                dimension: candidate.dimension,
                pos: ChunkPos::new((x as i32) >> 4, (z as i32) >> 4),
            });
        }
    }
    keys.sort_unstable();
    keys.dedup();
    Ok(keys)
}

/// Readiness covers the whole footprint before collision can select a refusal.
pub fn validate_restore(
    world: &impl PlacementWorld,
    candidate: RestoreCandidate,
) -> Result<RestoreCheck, ServerError> {
    let ordinary_invalid = RestoreCheck {
        valid: false,
        ready: true,
        on_ground: false,
    };
    let Ok(shape) = geometry(candidate.position) else {
        return Ok(ordinary_invalid);
    };
    if shape.bounds.minimum[1] < -64.0 || shape.bounds.maximum[1] > 320.0 {
        return Ok(ordinary_invalid);
    }
    for key in candidate_chunks(candidate)? {
        if world.ready_revision(key).is_none() {
            return Ok(RestoreCheck {
                valid: false,
                ready: false,
                on_ground: false,
            });
        }
    }
    let space = body_space(world, candidate.dimension, candidate.position)?;
    if !space.ready {
        return Ok(RestoreCheck {
            valid: false,
            ready: false,
            on_ground: false,
        });
    }
    let contact = support_contact(world, candidate.dimension, candidate.position)?;
    Ok(RestoreCheck {
        valid: space.free && (!candidate.require_support || contact.complete),
        ready: true,
        on_ground: contact.any,
    })
}

/// Unknown collision cells wait; loaded strict face contact remains free.
pub fn body_space(
    world: &impl PlacementWorld,
    dimension: Dimension,
    position: [f32; 3],
) -> Result<BodySpace, ServerError> {
    let shape = geometry(position)?;
    for y in shape.y.lower..=shape.y.upper {
        for x in shape.x.lower..=shape.x.upper {
            for z in shape.z.lower..=shape.z.upper {
                let Some(block) =
                    world.block_at(dimension, BlockPos::new(x as i32, y as i32, z as i32))
                else {
                    return Ok(BodySpace {
                        free: false,
                        ready: false,
                    });
                };
                for local in collision_cell(block)?.boxes() {
                    if overlaps(shape.bounds, translated(*local, x, y, z)) {
                        return Ok(BodySpace {
                            free: false,
                            ready: true,
                        });
                    }
                }
            }
        }
    }
    Ok(BodySpace {
        free: true,
        ready: true,
    })
}

/// Complete support needs one covering box per cell, independently of any contact.
pub fn support_contact(
    world: &impl PlacementWorld,
    dimension: Dimension,
    position: [f32; 3],
) -> Result<SupportContact, ServerError> {
    let shape = geometry(position)?;
    let y = checked_floor(position[1] - GROUND_PROBE)? as i64;
    let mut contact = SupportContact {
        complete: true,
        any: false,
    };
    for x in shape.x.lower..=shape.x.upper {
        for z in shape.z.lower..=shape.z.upper {
            let mut supported = false;
            if let Some(block) =
                world.block_at(dimension, BlockPos::new(x as i32, y as i32, z as i32))
            {
                for local in collision_cell(block)?.boxes() {
                    let b = translated(*local, x, y, z);
                    if b.maximum[1] < position[1] - GROUND_PROBE - EPSILON
                        || b.maximum[1] > position[1] + EPSILON
                    {
                        continue;
                    }
                    if horizontal_overlap(shape.bounds, b) {
                        contact.any = true;
                    }
                    if b.minimum[0] <= shape.bounds.minimum[0].max(x as f32)
                        && b.maximum[0] >= shape.bounds.maximum[0].min((x + 1) as f32)
                        && b.minimum[2] <= shape.bounds.minimum[2].max(z as f32)
                        && b.maximum[2] >= shape.bounds.maximum[2].min((z + 1) as f32)
                    {
                        supported = true;
                        break;
                    }
                }
            }
            contact.complete &= supported;
        }
    }
    Ok(contact)
}

/// Safe writing checks source geometry independently of restore height eligibility.
/// Whole-footprint readiness precedes every cell read; complete support may be fluid-covered.
pub(crate) fn safe_location(
    world: &impl PlacementWorld,
    dimension: Dimension,
    position: [f32; 3],
) -> Result<bool, ServerError> {
    let shape = geometry(position)?;
    for x in shape.x.lower..=shape.x.upper {
        for z in shape.z.lower..=shape.z.upper {
            if world
                .ready_revision(ChunkKey {
                    dimension,
                    pos: ChunkPos::new((x as i32) >> 4, (z as i32) >> 4),
                })
                .is_none()
            {
                return Ok(false);
            }
        }
    }
    let space = body_space(world, dimension, position)?;
    let contact = support_contact(world, dimension, position)?;
    Ok(space.ready && space.free && contact.complete)
}

/// Copies source-f32 support-layer coverage without world reads or retained allocation.
/// Checked endpoints refuse before narrowing, including collapsed spans at the integer limits.
pub(crate) fn trample_cells(position: [f32; 3]) -> Result<([BlockPos; 4], usize), ServerError> {
    let y = checked_floor(position[1] - GROUND_PROBE)?;
    let x = span(position[0] - HALF_WIDTH, position[0] + HALF_WIDTH, 2)?;
    let z = span(position[2] - HALF_WIDTH, position[2] + HALF_WIDTH, 2)?;
    let mut cells = [BlockPos::ORIGIN; 4];
    let mut len = 0;
    for x in x.lower..=x.upper {
        for z in z.lower..=z.upper {
            cells[len] = BlockPos::new(x as i32, y, z as i32);
            len += 1;
        }
    }
    Ok((cells, len))
}

/// Checked source enumeration is bounded before allocation and nearest-first.
pub fn spawn_columns(anchor: ChunkPos, radius: u8) -> Result<Vec<SpawnColumn>, ServerError> {
    if !(1..=64).contains(&radius) {
        return Err(ServerError::InvalidInput {
            field: "spawn_radius",
        });
    }
    let ax = i64::from(anchor.x()) * 16;
    let az = i64::from(anchor.z()) * 16;
    let radius = i64::from(radius);
    for endpoint in [ax, az, ax - radius, ax + radius, az - radius, az + radius] {
        if i32::try_from(endpoint).is_err() {
            return Err(ServerError::InvalidInput {
                field: "spawn_anchor",
            });
        }
    }
    let side = (radius * 2 + 1) as usize;
    let mut columns = Vec::with_capacity(side * side);
    for x in ax - radius..=ax + radius {
        for z in az - radius..=az + radius {
            columns.push(SpawnColumn {
                x: x as i32,
                z: z as i32,
            });
        }
    }
    columns.sort_unstable_by_key(|c| {
        let dx = i64::from(c.x) - ax;
        let dz = i64::from(c.z) - az;
        (dx * dx + dz * dz, c.x, c.z)
    });
    Ok(columns)
}

/// Arbitrary supplied columns retain the input cap, rather than a square's key cap.
pub fn spawn_chunk_keys(
    dimension: Dimension,
    columns: &[SpawnColumn],
) -> Result<Vec<ChunkKey>, ServerError> {
    if columns.len() > MAX_COLUMNS {
        return Err(ServerError::InvalidInput {
            field: "spawn_columns",
        });
    }
    let mut keys: Vec<_> = columns
        .iter()
        .map(|c| ChunkKey {
            dimension,
            pos: ChunkPos::new(c.x >> 4, c.z >> 4),
        })
        .collect();
    keys.sort_unstable();
    keys.dedup();
    Ok(keys)
}

/// A complete Ready non-air certificate skips only source loaded-air row branches.
/// Readers without it execute every source row; unknown rows discard downgrades.
pub fn scan_spawn_column(
    world: &impl PlacementWorld,
    dimension: Dimension,
    column: SpawnColumn,
    eye_height: f32,
) -> Result<ColumnSpawn, ServerError> {
    if !eye_height.is_finite() {
        return Err(ServerError::InvalidInput {
            field: "eye_height",
        });
    }
    let start = match world.ready_column_height(dimension, column.x, column.z) {
        None => 319,
        Some(-65) => {
            return Ok(ColumnSpawn {
                ready: true,
                site: None,
            });
        }
        Some(height @ -64..=319) => height,
        Some(_) => {
            return Err(ServerError::Internal {
                invariant: "actor placement height",
            });
        }
    };
    let mut best: Option<SpawnSite> = None;
    for y in (-64..=start).rev() {
        let Some(block) = world.block_at(dimension, BlockPos::new(column.x, y, column.z)) else {
            return Ok(ColumnSpawn {
                ready: false,
                site: None,
            });
        };
        let cell = collision_cell(block)?;
        let mut tops = [0.0; 8];
        let count = cell.boxes().len();
        for (index, b) in cell.boxes().iter().enumerate() {
            let mut insert = index;
            while insert > 0 && tops[insert - 1] < b.maximum[1] {
                tops[insert] = tops[insert - 1];
                insert -= 1;
            }
            tops[insert] = b.maximum[1];
        }
        for top in &tops[..count] {
            let position = [column.x as f32 + 0.5, y as f32 + top, column.z as f32 + 0.5];
            let space = body_space(world, dimension, position)?;
            if !space.ready {
                return Ok(ColumnSpawn {
                    ready: false,
                    site: None,
                });
            }
            if !space.free || !support_contact(world, dimension, position)?.complete {
                continue;
            }
            let tier = spawn_tier(world, dimension, position, eye_height)?;
            let site = SpawnSite { position, tier };
            if tier == SpawnTier::Dry {
                return Ok(ColumnSpawn {
                    ready: true,
                    site: Some(site),
                });
            }
            if best.is_none_or(|previous| tier_rank(tier) < tier_rank(previous.tier)) {
                best = Some(site);
            }
        }
    }
    Ok(ColumnSpawn {
        ready: true,
        site: best,
    })
}

const HALF_WIDTH: f32 = 0.3;
const HEIGHT: f32 = 1.8;
const EPSILON: f32 = 1e-5;
const GROUND_PROBE: f32 = 1e-4;
const MAX_COLUMNS: usize = 16_641;

#[derive(Clone, Copy)]
struct Span {
    lower: i64,
    upper: i64,
}
struct Geometry {
    bounds: Aabb,
    x: Span,
    y: Span,
    z: Span,
}

fn invalid_geometry() -> ServerError {
    ServerError::InvalidInput {
        field: "actor_geometry",
    }
}

// Bounds round in source f32 order; only endpoint qualification uses f64.
fn bounds(position: [f32; 3]) -> Aabb {
    Aabb {
        minimum: [
            position[0] - HALF_WIDTH,
            position[1],
            position[2] - HALF_WIDTH,
        ],
        maximum: [
            position[0] + HALF_WIDTH,
            position[1] + HEIGHT,
            position[2] + HALF_WIDTH,
        ],
    }
}
fn span(minimum: f32, maximum: f32, limit: i64) -> Result<Span, ServerError> {
    let lower = f64::from(minimum).floor();
    let upper = f64::from(maximum).ceil() - 1.0;
    if !lower.is_finite()
        || !upper.is_finite()
        || lower < f64::from(i32::MIN)
        || lower > f64::from(i32::MAX)
        || upper < f64::from(i32::MIN)
        || upper > f64::from(i32::MAX)
    {
        return Err(invalid_geometry());
    }
    let result = Span {
        lower: lower as i64,
        upper: upper as i64,
    };
    if result.upper >= result.lower && result.upper - result.lower + 1 > limit {
        return Err(invalid_geometry());
    }
    Ok(result)
}
fn geometry(position: [f32; 3]) -> Result<Geometry, ServerError> {
    let bounds = bounds(position);
    Ok(Geometry {
        x: span(bounds.minimum[0], bounds.maximum[0], 2)?,
        y: span(bounds.minimum[1], bounds.maximum[1], 3)?,
        z: span(bounds.minimum[2], bounds.maximum[2], 2)?,
        bounds,
    })
}
fn checked_floor(value: f32) -> Result<i32, ServerError> {
    let value = f64::from(value).floor();
    if !value.is_finite() || value < f64::from(i32::MIN) || value > f64::from(i32::MAX) {
        return Err(invalid_geometry());
    }
    Ok(value as i32)
}
fn translated(local: Aabb, x: i64, y: i64, z: i64) -> Aabb {
    let offset = [x as f32, y as f32, z as f32];
    Aabb {
        minimum: std::array::from_fn(|axis| local.minimum[axis] + offset[axis]),
        maximum: std::array::from_fn(|axis| local.maximum[axis] + offset[axis]),
    }
}
fn horizontal_overlap(a: Aabb, b: Aabb) -> bool {
    a.minimum[0] < b.maximum[0]
        && a.maximum[0] > b.minimum[0]
        && a.minimum[2] < b.maximum[2]
        && a.maximum[2] > b.minimum[2]
}
fn overlaps(a: Aabb, b: Aabb) -> bool {
    horizontal_overlap(a, b) && a.minimum[1] < b.maximum[1] && a.maximum[1] > b.minimum[1]
}
fn is_fluid(block: Option<u16>) -> bool {
    block.is_some_and(|b| (27..=34).contains(&b))
}
fn tier_rank(tier: SpawnTier) -> u8 {
    match tier {
        SpawnTier::Dry => 0,
        SpawnTier::EyeDry => 1,
        SpawnTier::Submerged => 2,
    }
}
fn spawn_tier(
    world: &impl PlacementWorld,
    dimension: Dimension,
    position: [f32; 3],
    eye_height: f32,
) -> Result<SpawnTier, ServerError> {
    let eye = BlockPos::new(
        checked_floor(position[0])?,
        checked_floor(position[1] + eye_height)?,
        checked_floor(position[2])?,
    );
    let eye_fluid = is_fluid(world.block_at(dimension, eye));
    let shape = geometry(position)?;
    // Source fluid sampling alone clamps empty upper spans to their lower cell.
    let xs = shape.x.lower..=shape.x.upper.max(shape.x.lower);
    let ys = shape.y.lower..=shape.y.upper.max(shape.y.lower);
    let zs = shape.z.lower..=shape.z.upper.max(shape.z.lower);
    let mut body_fluid = false;
    'body: for y in ys {
        for x in xs.clone() {
            for z in zs.clone() {
                if is_fluid(world.block_at(dimension, BlockPos::new(x as i32, y as i32, z as i32)))
                {
                    body_fluid = true;
                    break 'body;
                }
            }
        }
    }
    Ok(if eye_fluid {
        SpawnTier::Submerged
    } else if body_fluid {
        SpawnTier::EyeDry
    } else {
        SpawnTier::Dry
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::contracts::{BlockWrite, ServerLimits, SystemRule, TickBudget};
    use crate::core::state::{AuthorityState, TickContext};
    use crate::core::world::{self, ReadyChunk};
    use mornlea_storage::{Chunk, ContainerSnapshot, StorageKind};
    use std::cell::{Cell, RefCell};
    use std::collections::{BTreeMap, BTreeSet};

    const D: Dimension = Dimension::OVERWORLD;
    const COL: SpawnColumn = SpawnColumn { x: 8, z: 8 };
    const INVALID: ServerError = ServerError::InvalidInput {
        field: "actor_geometry",
    };

    #[derive(Default)]
    struct CountingWorld {
        ready: BTreeMap<ChunkKey, u64>,
        blocks: BTreeMap<(Dimension, BlockPos), u16>,
        // Deliberately inconsistent reader injection, never an actual Ready condition.
        missing_cells: BTreeSet<(Dimension, BlockPos)>,
        readiness: Cell<usize>,
        block_calls: Cell<usize>,
        height_calls: Cell<usize>,
        trace: RefCell<Vec<BlockPos>>,
    }
    fn key(dimension: Dimension, x: i32, z: i32) -> ChunkKey {
        ChunkKey {
            dimension,
            pos: ChunkPos::new(x, z),
        }
    }
    impl CountingWorld {
        fn known() -> Self {
            let mut w = Self::default();
            w.ready.insert(key(D, 0, 0), 9);
            w
        }
        fn put(&mut self, x: i32, y: i32, z: i32, block: u16) {
            self.blocks.insert((D, BlockPos::new(x, y, z)), block);
        }
        fn reset(&self) {
            self.readiness.set(0);
            self.block_calls.set(0);
            self.height_calls.set(0);
            self.trace.borrow_mut().clear();
        }
        fn counts(&self) -> (usize, usize, usize) {
            (
                self.readiness.get(),
                self.block_calls.get(),
                self.height_calls.get(),
            )
        }
    }
    impl PlacementWorld for CountingWorld {
        fn ready_revision(&self, k: ChunkKey) -> Option<u64> {
            self.readiness.set(self.readiness.get() + 1);
            self.ready.get(&k).copied()
        }
        fn block_at(&self, d: Dimension, p: BlockPos) -> Option<u16> {
            self.block_calls.set(self.block_calls.get() + 1);
            self.trace.borrow_mut().push(p);
            if !(-64..320).contains(&p.y()) {
                return Some(0);
            }
            if self.missing_cells.contains(&(d, p)) {
                return None;
            }
            self.ready
                .contains_key(&key(d, p.x() >> 4, p.z() >> 4))
                .then(|| self.blocks.get(&(d, p)).copied().unwrap_or(0))
        }
        fn ready_column_height(&self, _d: Dimension, _x: i32, _z: i32) -> Option<i32> {
            self.height_calls.set(self.height_calls.get() + 1);
            None
        }
    }
    fn candidate(position: [f32; 3], require_support: bool) -> RestoreCandidate {
        RestoreCandidate {
            dimension: D,
            position,
            require_support,
        }
    }
    fn check(w: &impl PlacementWorld, p: [f32; 3], support: bool) -> RestoreCheck {
        validate_restore(w, candidate(p, support)).unwrap()
    }
    fn valid(on_ground: bool) -> RestoreCheck {
        RestoreCheck {
            valid: true,
            ready: true,
            on_ground,
        }
    }
    fn rejected(on_ground: bool) -> RestoreCheck {
        RestoreCheck {
            valid: false,
            ready: true,
            on_ground,
        }
    }
    fn waiting() -> RestoreCheck {
        RestoreCheck {
            valid: false,
            ready: false,
            on_ground: false,
        }
    }
    fn site(y: f32, tier: SpawnTier) -> ColumnSpawn {
        ColumnSpawn {
            ready: true,
            site: Some(SpawnSite {
                position: [8.5, y, 8.5],
                tier,
            }),
        }
    }
    fn scan(w: &impl PlacementWorld) -> ColumnSpawn {
        scan_spawn_column(w, D, COL, 1.62).unwrap()
    }
    fn stone() -> CountingWorld {
        let mut w = CountingWorld::known();
        w.put(8, 63, 8, 2);
        w
    }

    #[test]
    fn body_and_support_source_cell_order() {
        let mut w = CountingWorld::known();
        for x in 0..=1 {
            for z in 0..=1 {
                w.ready.insert(key(D, x, z), 9);
            }
        }
        assert_eq!(
            body_space(&w, D, [16., 64., 16.]).unwrap(),
            BodySpace {
                free: true,
                ready: true
            }
        );
        assert_eq!(
            *w.trace.borrow(),
            vec![
                BlockPos::new(15, 64, 15),
                BlockPos::new(15, 64, 16),
                BlockPos::new(16, 64, 15),
                BlockPos::new(16, 64, 16),
                BlockPos::new(15, 65, 15),
                BlockPos::new(15, 65, 16),
                BlockPos::new(16, 65, 15),
                BlockPos::new(16, 65, 16)
            ]
        );
        w.reset();
        assert_eq!(
            support_contact(&w, D, [16., 64., 16.]).unwrap(),
            SupportContact {
                complete: false,
                any: false
            }
        );
        assert_eq!(
            *w.trace.borrow(),
            vec![
                BlockPos::new(15, 63, 15),
                BlockPos::new(15, 63, 16),
                BlockPos::new(16, 63, 15),
                BlockPos::new(16, 63, 16)
            ]
        );
        w.missing_cells.insert((D, BlockPos::new(15, 63, 15)));
        assert_eq!(
            support_contact(&w, D, [16., 64., 16.]).unwrap(),
            SupportContact {
                complete: false,
                any: false
            }
        );
    }
    #[test]
    fn current_airborne_exact() {
        let w = stone();
        let p = [8.5, 65.25, 8.5];
        assert_eq!(check(&w, p, false), valid(false));
        assert_eq!(check(&w, p, true), rejected(false));
        assert_eq!(p, [8.5, 65.25, 8.5]);
    }
    #[test]
    fn grounded_and_partial_support() {
        let w = stone();
        assert_eq!(
            support_contact(&w, D, [8.5, 64., 8.5]).unwrap(),
            SupportContact {
                complete: true,
                any: true
            }
        );
        assert_eq!(check(&w, [8.5, 64., 8.5], true), valid(true));
        let mut w = CountingWorld::known();
        for x in 0..=1 {
            for z in 0..=1 {
                w.ready.insert(key(D, x, z), 9);
            }
        }
        for (x, z) in [(15, 15), (15, 16), (16, 15)] {
            w.put(x, 63, z, 2);
        }
        assert_eq!(check(&w, [16., 64., 16.], false), valid(true));
        assert_eq!(check(&w, [16., 64., 16.], true), rejected(true));
        w.put(16, 63, 16, 2);
        assert_eq!(check(&w, [16., 64., 16.], true), valid(true));
    }
    #[test]
    fn door_partial_contact_is_distinct_from_full_support() {
        let mut w = CountingWorld::known();
        w.put(8, 63, 8, 62);
        assert_eq!(
            support_contact(&w, D, [8.5, 64., 8.9]).unwrap(),
            SupportContact {
                complete: false,
                any: true
            }
        );
        assert_eq!(check(&w, [8.5, 64., 8.9], false), valid(true));
        assert_eq!(check(&w, [8.5, 64., 8.9], true), rejected(true));
        assert_eq!(
            support_contact(&w, D, [8.5, 64., 8.5]).unwrap(),
            SupportContact {
                complete: false,
                any: false
            }
        );
    }
    #[test]
    fn all_ready_before_collision() {
        let mut w = CountingWorld::known();
        w.put(15, 64, 15, 2);
        assert_eq!(check(&w, [16., 64., 16.], false), waiting());
        assert_eq!(w.block_calls.get(), 0);
        for x in 0..=1 {
            for z in 0..=1 {
                w.ready.insert(key(D, x, z), 9);
            }
        }
        w.reset();
        assert_eq!(check(&w, [16., 64., 16.], false), rejected(false));
        assert_eq!(w.readiness.get(), 4);
        // A ready solid collision still computes all support cells.
        assert_eq!(w.block_calls.get(), 5);
        let trace = w.trace.borrow();
        assert_eq!(
            &trace[1..],
            &[
                BlockPos::new(15, 63, 15),
                BlockPos::new(15, 63, 16),
                BlockPos::new(16, 63, 15),
                BlockPos::new(16, 63, 16)
            ]
        );
    }
    #[test]
    fn inconsistent_ready_body_unknown_waits() {
        let mut w = stone();
        w.missing_cells.insert((D, BlockPos::new(8, 65, 8)));
        assert_eq!(
            body_space(&w, D, [8.5, 64., 8.5]).unwrap(),
            BodySpace {
                free: false,
                ready: false
            }
        );
        assert_eq!(check(&w, [8.5, 64., 8.5], false), waiting());
    }
    #[test]
    fn exact_reduced_tops_and_zero_box_blocks() {
        for (block, y) in [(2, 64.), (35, 63.9375), (76, 63.5625)] {
            let mut w = CountingWorld::known();
            w.put(8, 63, 8, block);
            assert_eq!(check(&w, [8.5, y, 8.5], true), valid(true));
            assert_eq!(scan(&w), site(y, SpawnTier::Dry));
        }
        for b in [0, 27, 34, 37, 71, 85, 70] {
            let mut w = CountingWorld::known();
            w.put(8, 63, 8, b);
            w.put(8, 64, 8, b);
            assert_eq!(
                support_contact(&w, D, [8.5, 64., 8.5]).unwrap(),
                SupportContact {
                    complete: false,
                    any: false
                }
            );
            assert_eq!(
                body_space(&w, D, [8.5, 64., 8.5]).unwrap(),
                BodySpace {
                    free: true,
                    ready: true
                }
            );
        }
    }
    #[test]
    fn bounds_are_ordinary_rejections_before_reads() {
        let w = CountingWorld::known();
        for p in [
            [8.5, -64.0625, 8.5],
            [8.5, 319., 8.5],
            [8.5, f32::NAN, 8.5],
            [8.5, f32::INFINITY, 8.5],
            [f32::MAX, 64., 8.5],
            [8.5, f32::MAX, 8.5],
        ] {
            assert_eq!(check(&w, p, false), rejected(false));
            assert_eq!(w.counts(), (0, 0, 0));
        }
        for p in [
            [f32::NAN, 64., 8.5],
            [f32::MAX, 64., 8.5],
            [8.5, f32::MAX, 8.5],
        ] {
            assert_eq!(body_space(&w, D, p), Err(INVALID));
            assert_eq!(support_contact(&w, D, p), Err(INVALID));
        }
        assert_eq!(
            candidate_chunks(candidate([f32::NAN, 64., 8.5], false)),
            Err(INVALID)
        );
        assert_eq!(
            candidate_chunks(candidate([f32::MAX, 64., 8.5], false)),
            Err(INVALID)
        );
        for y in [f32::NAN, f32::MAX] {
            assert_eq!(
                candidate_chunks(candidate([8.5, y, 8.5], false)).unwrap(),
                vec![key(D, 0, 0)]
            );
        }
    }
    #[test]
    fn negative_coordinates_face_contact_and_precision_empty_spans() {
        let mut w = CountingWorld::default();
        w.ready.insert(key(D, -1, -1), 9);
        w.put(-1, 63, -1, 2);
        assert_eq!(
            candidate_chunks(candidate([-0.5, 64., -0.5], false)).unwrap(),
            vec![key(D, -1, -1)]
        );
        assert_eq!(check(&w, [-0.5, 64., -0.5], true), valid(true));
        assert_eq!(
            body_space(&w, D, [-0.5, 63.9375, -0.5]).unwrap(),
            BodySpace {
                free: false,
                ready: true
            }
        );
        let w = CountingWorld::default();
        let p = [16_777_216., 64., 8.5];
        assert!(candidate_chunks(candidate(p, true)).unwrap().is_empty());
        assert_eq!(
            body_space(&w, D, p).unwrap(),
            BodySpace {
                free: true,
                ready: true
            }
        );
        assert_eq!(
            support_contact(&w, D, p).unwrap(),
            SupportContact {
                complete: true,
                any: false
            }
        );
        assert_eq!(w.block_calls.get(), 0);
    }
    #[test]
    fn water_restore_allowed() {
        let mut w = stone();
        w.put(8, 64, 8, 27);
        w.put(8, 65, 8, 34);
        assert_eq!(check(&w, [8.5, 64., 8.5], false), valid(true));
        assert_eq!(check(&w, [8.5, 64., 8.5], true), valid(true));
    }
    #[test]
    fn columns_exact_order_and_caps() {
        let expected = [
            (0, 0),
            (-1, 0),
            (0, -1),
            (0, 1),
            (1, 0),
            (-1, -1),
            (-1, 1),
            (1, -1),
            (1, 1),
        ]
        .map(|(x, z)| SpawnColumn { x, z });
        assert_eq!(spawn_columns(ChunkPos::new(0, 0), 1).unwrap(), expected);
        for (radius, count, keys) in [(16, 1089, 9), (64, 16641, 81)] {
            let cols = spawn_columns(ChunkPos::new(0, 0), radius).unwrap();
            assert_eq!(cols.len(), count);
            assert_eq!(spawn_chunk_keys(D, &cols).unwrap().len(), keys);
            assert_eq!(
                spawn_chunk_keys(D, &cols).unwrap(),
                spawn_chunk_keys(Dimension::DEPTHS, &cols)
                    .unwrap()
                    .into_iter()
                    .map(|k| key(D, k.pos.x(), k.pos.z()))
                    .collect::<Vec<_>>()
            );
        }
        for r in [0, 65] {
            assert_eq!(
                spawn_columns(ChunkPos::new(0, 0), r),
                Err(ServerError::InvalidInput {
                    field: "spawn_radius"
                })
            );
        }
        for a in [ChunkPos::new(i32::MAX, 0), ChunkPos::new(0, i32::MIN)] {
            assert_eq!(
                spawn_columns(a, 1),
                Err(ServerError::InvalidInput {
                    field: "spawn_anchor"
                })
            );
        }
        let mut cols: Vec<_> = (0..16641)
            .map(|x| SpawnColumn { x: x * 16, z: 0 })
            .collect();
        assert_eq!(spawn_chunk_keys(D, &cols).unwrap().len(), 16641);
        cols.push(COL);
        let before = cols.clone();
        assert_eq!(
            spawn_chunk_keys(D, &cols),
            Err(ServerError::InvalidInput {
                field: "spawn_columns"
            })
        );
        assert_eq!(cols, before);
    }
    #[test]
    fn column_top_and_void_literal_read_counts() {
        let w = stone();
        assert_eq!(scan(&w), site(64., SpawnTier::Dry));
        assert_eq!(w.counts(), (0, 263, 1));
        let trace = w.trace.borrow();
        assert_eq!(trace[0], BlockPos::new(8, 319, 8));
        assert_eq!(trace[256], BlockPos::new(8, 63, 8));
        assert_eq!(
            &trace[257..],
            &[
                BlockPos::new(8, 64, 8),
                BlockPos::new(8, 65, 8),
                BlockPos::new(8, 63, 8),
                BlockPos::new(8, 65, 8),
                BlockPos::new(8, 64, 8),
                BlockPos::new(8, 65, 8)
            ]
        );
        let w = CountingWorld::known();
        assert_eq!(
            scan(&w),
            ColumnSpawn {
                ready: true,
                site: None
            }
        );
        assert_eq!(w.counts(), (0, 384, 1));
        assert_eq!(w.trace.borrow().last(), Some(&BlockPos::new(8, -64, 8)));
        let w = CountingWorld::default();
        assert_eq!(
            scan(&w),
            ColumnSpawn {
                ready: false,
                site: None
            }
        );
        assert_eq!(w.counts(), (0, 1, 1));
        let mut w = CountingWorld::known();
        w.put(8, 319, 8, 2);
        assert_eq!(scan(&w), site(320., SpawnTier::Dry));
        assert_eq!(w.counts(), (0, 7, 1));
        let reader = CountingReader::new(&w);
        assert_eq!(scan(&reader), site(320., SpawnTier::Dry));
        assert_eq!(reader.counts(), (0, 7, 1, 2));
        w.reset();
        assert_eq!(check(&w, [8.5, 320., 8.5], false), rejected(false));
        assert_eq!(w.counts(), (0, 0, 0));
    }
    #[test]
    fn tier_ladder_highest_equal_and_unknown_row_discards_fallback() {
        let mut w = stone();
        w.put(8, 64, 8, 27);
        assert_eq!(scan(&w), site(64., SpawnTier::EyeDry));
        w.put(8, 65, 8, 27);
        assert_eq!(scan(&w), site(64., SpawnTier::Submerged));
        w.put(8, 30, 8, 2);
        assert_eq!(scan(&w), site(31., SpawnTier::Dry));
        w.put(8, 31, 8, 27);
        w.put(8, 32, 8, 27);
        assert_eq!(scan(&w), site(64., SpawnTier::Submerged));
        w.put(8, 32, 8, 0);
        assert_eq!(scan(&w), site(31., SpawnTier::EyeDry));
        w.put(8, 32, 8, 27);
        w.missing_cells.insert((D, BlockPos::new(8, 30, 8)));
        assert_eq!(
            scan(&w),
            ColumnSpawn {
                ready: false,
                site: None
            }
        );
    }
    #[test]
    fn eye_first_body_still_samples_when_eye_is_fluid() {
        let mut w = stone();
        w.put(8, 64, 8, 27);
        w.put(8, 65, 8, 27);
        assert_eq!(scan(&w), site(64., SpawnTier::Submerged));
        let trace = w.trace.borrow();
        let first_eye = trace
            .windows(3)
            .position(|v| {
                v == [
                    BlockPos::new(8, 63, 8),
                    BlockPos::new(8, 65, 8),
                    BlockPos::new(8, 64, 8),
                ]
            })
            .unwrap();
        assert!(first_eye >= 3);
    }
    #[test]
    fn eye_and_large_column_checked_failure_order() {
        let w = stone();
        for eye in [f32::NAN, f32::INFINITY] {
            assert_eq!(
                scan_spawn_column(&w, D, COL, eye),
                Err(ServerError::InvalidInput {
                    field: "eye_height"
                })
            );
            assert_eq!(w.counts(), (0, 0, 0));
        }
        assert_eq!(scan_spawn_column(&w, D, COL, f32::MAX), Err(INVALID));
        assert_eq!(w.counts(), (0, 260, 1));
        let c = SpawnColumn { x: i32::MAX, z: 8 };
        let mut w = CountingWorld::default();
        w.ready.insert(key(D, i32::MAX >> 4, 0), 9);
        w.put(i32::MAX, 319, 8, 2);
        assert_eq!(scan_spawn_column(&w, D, c, 1.62), Err(INVALID));
        assert_eq!(w.counts(), (0, 1, 1));
        w.blocks.clear();
        w.reset();
        assert_eq!(
            scan_spawn_column(&w, D, c, 1.62).unwrap(),
            ColumnSpawn {
                ready: true,
                site: None
            }
        );
        assert_eq!(w.counts(), (0, 384, 1));
    }
    #[test]
    fn precision_empty_collision_support_keeps_source_fluid_clamp() {
        let c = SpawnColumn {
            x: 16_777_216,
            z: 8,
        };
        let mut w = CountingWorld::default();
        w.ready.insert(key(D, c.x >> 4, 0), 9);
        w.put(c.x, 63, 8, 2);
        w.put(c.x, 64, 8, 27);
        assert_eq!(
            scan_spawn_column(&w, D, c, 1.62).unwrap(),
            ColumnSpawn {
                ready: true,
                site: Some(SpawnSite {
                    position: [16_777_216., 64., 8.5],
                    tier: SpawnTier::EyeDry
                })
            }
        );
        assert!(w.block_calls.get() <= 89_472);
    }
    struct Height<'a>(&'a CountingWorld, i32);
    impl PlacementWorld for Height<'_> {
        fn ready_revision(&self, k: ChunkKey) -> Option<u64> {
            self.0.ready_revision(k)
        }
        fn block_at(&self, d: Dimension, p: BlockPos) -> Option<u16> {
            self.0.block_at(d, p)
        }
        fn ready_column_height(&self, _d: Dimension, _x: i32, _z: i32) -> Option<i32> {
            self.0.height_calls.set(self.0.height_calls.get() + 1);
            Some(self.1)
        }
    }
    #[test]
    fn empty_and_invalid_height_certificate() {
        let w = CountingWorld::known();
        assert_eq!(
            scan(&Height(&w, -65)),
            ColumnSpawn {
                ready: true,
                site: None
            }
        );
        assert_eq!(w.counts(), (0, 0, 1));
        for h in [-66, 320] {
            w.reset();
            assert_eq!(
                scan_spawn_column(&Height(&w, h), D, COL, 1.62),
                Err(ServerError::Internal {
                    invariant: "actor placement height"
                })
            );
            assert_eq!(w.counts(), (0, 0, 1));
        }
    }
    struct CertifiedWorld<'a>(&'a CountingWorld);
    impl PlacementWorld for CertifiedWorld<'_> {
        fn ready_revision(&self, k: ChunkKey) -> Option<u64> {
            self.0.ready_revision(k)
        }
        fn block_at(&self, d: Dimension, p: BlockPos) -> Option<u16> {
            self.0.block_at(d, p)
        }
        fn ready_column_height(&self, d: Dimension, x: i32, z: i32) -> Option<i32> {
            assert!(self.0.missing_cells.is_empty());
            self.0.height_calls.set(self.0.height_calls.get() + 1);
            if !self.0.ready.contains_key(&key(d, x >> 4, z >> 4)) {
                return None;
            }
            Some(
                self.0
                    .blocks
                    .iter()
                    .filter(|((bd, p), b)| {
                        *bd == d
                            && p.x() == x
                            && p.z() == z
                            && (-64..320).contains(&p.y())
                            && **b != 0
                    })
                    .map(|((_, p), _)| p.y())
                    .max()
                    .unwrap_or(-65),
            )
        }
    }
    fn assert_bit_equal(a: ColumnSpawn, b: ColumnSpawn) {
        assert_eq!(a.ready, b.ready);
        match (a.site, b.site) {
            (None, None) => {}
            (Some(a), Some(b)) => {
                assert_eq!(a.tier, b.tier);
                assert_eq!(a.position.map(f32::to_bits), b.position.map(f32::to_bits));
            }
            _ => panic!("certificate changed site"),
        }
    }
    #[test]
    fn certified_skip_equivalence_static_source_scenes() {
        let mut scenes = vec![CountingWorld::known(), stone()];
        for block in [35, 76, 62, 27, 37, 71, 85, 70] {
            let mut w = CountingWorld::known();
            w.put(8, 63, 8, block);
            scenes.push(w);
        }
        let mut w = CountingWorld::known();
        w.put(8, 319, 8, 2);
        scenes.push(w);
        let mut w = stone();
        w.put(8, 64, 8, 27);
        scenes.push(w);
        let mut w = stone();
        w.put(8, 64, 8, 27);
        w.put(8, 65, 8, 27);
        scenes.push(w);
        let mut w = stone();
        w.put(8, 64, 8, 27);
        w.put(8, 65, 8, 27);
        w.put(8, 30, 8, 2);
        scenes.push(w);
        let mut w = stone();
        w.put(8, 64, 8, 27);
        w.put(8, 65, 8, 27);
        w.put(8, 30, 8, 2);
        w.put(8, 31, 8, 27);
        w.put(8, 32, 8, 27);
        scenes.push(w);
        for w in scenes {
            assert_bit_equal(scan(&w), scan(&CertifiedWorld(&w)));
            assert!(w.block_calls.get() <= 2 * 89_472);
        }
    }
    fn consume(
        w: &impl PlacementWorld,
        current: RestoreCandidate,
        safe: RestoreCandidate,
    ) -> (RestoreCheck, usize) {
        let c = validate_restore(w, current).unwrap();
        if c.valid || !c.ready {
            return (c, 1);
        }
        (validate_restore(w, safe).unwrap(), 2)
    }
    #[test]
    fn executing_consumer_double() {
        let mut w = stone();
        let current = candidate([8.5, 64., 8.5], false);
        let safe = candidate([8.5, 70., 8.5], true);
        w.ready.clear();
        assert_eq!(consume(&w, current, safe), (waiting(), 1));
        assert_eq!(w.block_calls.get(), 0);
        w.ready.insert(key(D, 0, 0), 9);
        w.put(8, 64, 8, 2);
        w.put(8, 69, 8, 2);
        assert_eq!(consume(&w, current, safe), (valid(true), 2));
    }

    impl PlacementWorld for &CountingWorld {
        fn ready_revision(&self, k: ChunkKey) -> Option<u64> {
            (**self).ready_revision(k)
        }
        fn block_at(&self, d: Dimension, p: BlockPos) -> Option<u16> {
            (**self).block_at(d, p)
        }
        fn ready_column_height(&self, d: Dimension, x: i32, z: i32) -> Option<i32> {
            (**self).ready_column_height(d, x, z)
        }
    }
    // Generic forwarding counts placement and raw in-height observations separately.
    struct CountingReader<W> {
        inner: W,
        ready: Cell<usize>,
        blocks: Cell<usize>,
        heights: Cell<usize>,
        raw: Cell<usize>,
    }
    impl<W> CountingReader<W> {
        fn new(inner: W) -> Self {
            Self {
                inner,
                ready: Cell::new(0),
                blocks: Cell::new(0),
                heights: Cell::new(0),
                raw: Cell::new(0),
            }
        }
        fn counts(&self) -> (usize, usize, usize, usize) {
            (
                self.ready.get(),
                self.blocks.get(),
                self.heights.get(),
                self.raw.get(),
            )
        }
    }
    impl<W: PlacementWorld> PlacementWorld for CountingReader<W> {
        fn ready_revision(&self, k: ChunkKey) -> Option<u64> {
            self.ready.set(self.ready.get() + 1);
            self.inner.ready_revision(k)
        }
        fn block_at(&self, d: Dimension, p: BlockPos) -> Option<u16> {
            self.blocks.set(self.blocks.get() + 1);
            if (-64..320).contains(&p.y()) {
                self.raw.set(self.raw.get() + 1);
            }
            self.inner.block_at(d, p)
        }
        fn ready_column_height(&self, d: Dimension, x: i32, z: i32) -> Option<i32> {
            self.heights.set(self.heights.get() + 1);
            self.inner.ready_column_height(d, x, z)
        }
    }
    fn authority() -> AuthorityState {
        AuthorityState::try_new(
            ServerLimits::try_new(8, 4096, 512, 64, 64, 1_048_576).unwrap(),
            42,
        )
        .unwrap()
    }
    fn base(x: i32, block: u16) -> ReadyChunk {
        let mut sections = vec![
            ContainerSnapshot {
                kind: StorageKind::Single,
                bits: 0,
                single: 0,
                palette: vec![],
                packed: vec![]
            };
            24
        ];
        sections[7].single = block;
        ReadyChunk::try_new(
            key(D, x, 0),
            1,
            9,
            Chunk {
                sections,
                drops: vec![Default::default(); 32],
                furnaces: vec![Default::default(); 32],
                chests: vec![Default::default(); 16],
            },
        )
        .unwrap()
    }
    fn reset_owners() {
        world::reset_ready_clones();
        world::reset_materializations();
    }
    fn unchanged_owners() {
        assert_eq!((world::ready_clones(), world::materializations()), (0, 0));
    }
    fn empty_schedules(a: &AuthorityState) {
        assert_eq!(
            (
                a.fluid_schedule().pending_fluid(D),
                a.fluid_schedule().pending_fluid(Dimension::DEPTHS),
                a.farmland_schedule().pending_candidates(D),
                a.farmland_schedule().pending_candidates(Dimension::DEPTHS)
            ),
            (0, 0, 0, 0)
        );
        assert_eq!(a.fluid_schedule().pending_rescans(), 0);
    }
    type ReadyProbe = (Option<u64>, Option<u16>, Option<u16>, usize);
    fn probe_vector(v: AuthorityReadView<'_>) -> Vec<ReadyProbe> {
        [0, 1, 2]
            .into_iter()
            .map(|x| {
                (
                    v.ready_chunk_revision(key(D, x, 0)),
                    v.observation(D, BlockPos::new(x * 16 + 8, 63, 8))
                        .map(|o| o.block),
                    v.observation(D, BlockPos::new(x * 16 + 8, 64, 8))
                        .map(|o| o.block),
                    v.actors().len(),
                )
            })
            .collect()
    }
    #[test]
    fn actual_read_view_source_height_air() {
        let mut a = authority();
        empty_schedules(&a);
        let mut ctx = TickContext::harness(&mut a, TickBudget::full());
        for (x, b) in [(0, 2), (1, 35), (2, 76)] {
            ctx.preload_ready_chunk(base(x, b));
        }
        let expected = vec![
            (Some(9), Some(2), Some(0), 0),
            (Some(9), Some(35), Some(0), 0),
            (Some(9), Some(76), Some(0), 0),
        ];
        assert_eq!(probe_vector(ctx.read()), expected);
        reset_owners();
        for (x, y, calls) in [(8, 64., 7), (24, 63.9375, 9), (40, 63.5625, 9)] {
            let reader = CountingReader::new(ctx.read());
            let result = scan_spawn_column(&reader, D, SpawnColumn { x, z: 8 }, 1.62).unwrap();
            assert_eq!(
                result,
                ColumnSpawn {
                    ready: true,
                    site: Some(SpawnSite {
                        position: [x as f32 + 0.5, y, 8.5],
                        tier: SpawnTier::Dry
                    })
                }
            );
            assert_eq!(reader.counts(), (0, calls, 1, calls));
            assert_eq!(
                validate_restore(&reader, candidate([x as f32 + 0.5, y, 8.5], true)).unwrap(),
                valid(true)
            );
            unchanged_owners();
        }
        let v = ctx.read();
        let p = BlockPos::new(160, 320, 8);
        assert_eq!(v.observation(D, p), None);
        assert_eq!(PlacementWorld::block_at(&v, D, p), Some(0));
        assert_eq!(
            PlacementWorld::block_at(&v, D, BlockPos::new(160, 319, 8)),
            None
        );
        let reader = CountingReader::new(v);
        assert_eq!(
            scan_spawn_column(&reader, D, SpawnColumn { x: 160, z: 8 }, 1.62).unwrap(),
            ColumnSpawn {
                ready: false,
                site: None
            }
        );
        assert_eq!(reader.counts(), (0, 1, 1, 1));
        assert_eq!(probe_vector(ctx.read()), expected);
        unchanged_owners();
        drop(ctx);
        empty_schedules(&a);
    }
    #[test]
    fn actual_air_certificate_tracks_real_pending_writes_and_erase() {
        let mut a = authority();
        empty_schedules(&a);
        let mut ctx = TickContext::harness(&mut a, TickBudget::full());
        ctx.preload_ready_chunk(base(0, 0));
        reset_owners();
        let reader = CountingReader::new(ctx.read());
        assert_eq!(reader.ready_column_height(D, 8, 8), Some(-65));
        let reader = CountingReader::new(ctx.read());
        assert_eq!(
            scan(&reader),
            ColumnSpawn {
                ready: true,
                site: None
            }
        );
        assert_eq!(reader.counts(), (0, 0, 1, 0));
        unchanged_owners();
        let observed = ctx.read().observation(D, BlockPos::new(8, 63, 8)).unwrap();
        ctx.transaction()
            .try_system(
                SystemRule::Support,
                vec![BlockWrite::try_new(observed, 2).unwrap()],
            )
            .unwrap();
        assert_eq!(ctx.read().ready_chunk_revision(key(D, 0, 0)), Some(10));
        assert_eq!(ctx.read().highest_non_air(D, 8, 8), Some(63));
        reset_owners();
        let reader = CountingReader::new(ctx.read());
        assert_eq!(scan(&reader), site(64., SpawnTier::Dry));
        assert_eq!(reader.counts(), (0, 7, 1, 7));
        unchanged_owners();
        let observed = ctx.read().observation(D, BlockPos::new(8, 63, 8)).unwrap();
        assert_eq!(observed.revision, 10);
        ctx.transaction()
            .try_system(
                SystemRule::Support,
                vec![BlockWrite::try_new(observed, 0).unwrap()],
            )
            .unwrap();
        assert_eq!(ctx.read().ready_chunk_revision(key(D, 0, 0)), Some(10));
        assert_eq!(ctx.read().highest_non_air(D, 8, 8), Some(-65));
        reset_owners();
        let reader = CountingReader::new(ctx.read());
        assert_eq!(
            scan(&reader),
            ColumnSpawn {
                ready: true,
                site: None
            }
        );
        assert_eq!(reader.counts(), (0, 0, 1, 0));
        unchanged_owners();
        drop(ctx);
        empty_schedules(&a);
    }

    #[test]
    fn safe_checkpoint_requires_ready_free_complete_support() {
        for row in 0..6 {
            let mut w = CountingWorld::known();
            let mut pose = [8.5, 64., 8.5];
            w.put(8, 63, 8, 1);
            match row {
                0 => {}
                1 => w.put(8, 64, 8, 27),
                2 => {
                    w.put(8, 63, 8, 76);
                    pose[1] = 63.5625;
                }
                3 => w.put(8, 63, 8, 0),
                4 => w.put(8, 64, 8, 1),
                5 => {
                    pose[0] = 15.9;
                    w.ready.insert(key(D, 1, 0), 9);
                    w.put(15, 63, 8, 1);
                    let contact = support_contact(&w, D, pose).unwrap();
                    assert!(contact.any && !contact.complete);
                    w.reset();
                }
                _ => unreachable!(),
            }
            assert_eq!(safe_location(&w, D, pose), Ok(row < 3), "row {row}");
        }
    }

    #[test]
    fn safe_checkpoint_checks_all_ready_before_cells() {
        let mut w = CountingWorld::known();
        let pose = [15.9, 64., 8.5];
        w.put(15, 64, 8, 1);
        assert_eq!(safe_location(&w, D, pose), Ok(false));
        assert_eq!(w.counts(), (2, 0, 0));
        w.ready.insert(key(D, 1, 0), 9);
        w.reset();
        assert_eq!(safe_location(&w, D, pose), Ok(false));
        assert!(w.counts().1 > 0);
        for missing in [BlockPos::new(8, 64, 8), BlockPos::new(8, 63, 8)] {
            let mut w = CountingWorld::known();
            w.put(8, 63, 8, 1);
            w.missing_cells.insert((D, missing));
            assert_eq!(safe_location(&w, D, [8.5, 64., 8.5]), Ok(false));
        }
        let mut w = CountingWorld::known();
        for (x, z) in [(0, 1), (1, 0), (1, 1)] {
            w.ready.insert(key(D, x, z), 9);
        }
        assert_eq!(safe_location(&w, D, [15.9, 64.25, 15.9]), Ok(false));
        assert_eq!(w.counts(), (4, 16, 0));
        let mut trace = Vec::new();
        for y in 64..=66 {
            for x in 15..=16 {
                for z in 15..=16 {
                    trace.push(BlockPos::new(x, y, z));
                }
            }
        }
        for x in 15..=16 {
            for z in 15..=16 {
                trace.push(BlockPos::new(x, 64, z));
            }
        }
        assert_eq!(*w.trace.borrow(), trace);
    }

    #[test]
    fn safe_checkpoint_preserves_height_and_float_edges() {
        let mut w = CountingWorld::known();
        w.put(8, 318, 8, 1);
        assert_eq!(safe_location(&w, D, [8.5, 319., 8.5]), Ok(true));
        w.reset();
        assert!(
            !validate_restore(&w, candidate([8.5, 319., 8.5], true))
                .unwrap()
                .valid
        );
        assert_eq!(w.counts(), (0, 0, 0));
        w.reset();
        w.put(8, -65, 8, 1);
        assert_eq!(safe_location(&w, D, [8.5, -64., 8.5]), Ok(false));
        let w = CountingWorld::default();
        let collapsed = [16777216., 64., 8.5];
        assert_eq!(safe_location(&w, D, collapsed), Ok(true));
        assert_eq!(w.counts(), (0, 0, 0));
        w.reset();
        assert_eq!(
            support_contact(&w, D, collapsed),
            Ok(SupportContact {
                complete: true,
                any: false
            })
        );
        assert_eq!(w.counts(), (0, 0, 0));
        w.reset();
        assert_eq!(safe_location(&w, D, [f32::MAX, 64., 8.5]), Err(INVALID));
        assert_eq!(w.counts(), (0, 0, 0));
    }

    #[test]
    fn trample_geometry_exact_source_coverage() {
        for (pose, expected) in [
            ([8.5, 64., 8.5], vec![BlockPos::new(8, 63, 8)]),
            ([0.3, 1., 0.3], vec![BlockPos::ORIGIN]),
            (
                [-0.1, 0.9375, -0.1],
                vec![
                    BlockPos::new(-1, 0, -1),
                    BlockPos::new(-1, 0, 0),
                    BlockPos::new(0, 0, -1),
                    BlockPos::ORIGIN,
                ],
            ),
            (
                [15.9, 63.9375, 15.9],
                vec![
                    BlockPos::new(15, 63, 15),
                    BlockPos::new(15, 63, 16),
                    BlockPos::new(16, 63, 15),
                    BlockPos::new(16, 63, 16),
                ],
            ),
            ([16777216., 64., 8.5], vec![]),
        ] {
            let (cells, len) = trample_cells(pose).unwrap();
            assert_eq!(&cells[..len], expected);
            assert!(cells[len..].iter().all(|p| *p == BlockPos::ORIGIN));
        }
    }
    #[test]
    fn trample_geometry_refuses_unrepresentable_without_panic() {
        for pose in [
            [f32::MAX, 64., 8.5],
            [f32::NAN, 64., 8.5],
            [f32::INFINITY, 64., 8.5],
            [2147483648., 64., 8.5],
            [-2147483648., 64., 8.5],
            [8.5, f32::MAX, 8.5],
            [8.5, f32::NAN, 8.5],
            [8.5, f32::INFINITY, 8.5],
            [8.5, 64., f32::MAX],
        ] {
            let result = std::panic::catch_unwind(|| trample_cells(pose));
            assert_eq!(
                result.unwrap(),
                Err(ServerError::InvalidInput {
                    field: "actor_geometry"
                })
            );
        }
        assert_eq!(trample_cells([2147483520., 64., 8.5]).unwrap().1, 0);
    }
}
