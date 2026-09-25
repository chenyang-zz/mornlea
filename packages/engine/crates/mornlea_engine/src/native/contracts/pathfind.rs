#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PathCell {
    pub x: i32,
    pub y: i32,
    pub z: i32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PathRevision {
    pub chunk: [i32; 2],
    pub revision: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PathError {
    InvalidGrid,
    InvalidRevision,
    ScratchTooSmall,
    Unreachable,
    BudgetExceeded,
    Allocation,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PathBlockTable {
    pub(crate) table: Box<[bool; 65536]>,
}

impl PathBlockTable {
    pub fn from_passable_ids(passable: &[u16]) -> Result<Self, PathError> {
        let mut table = vec![false; 65536].into_boxed_slice();
        for &id in passable {
            table[id as usize] = true;
        }
        let table = match table.try_into() {
            Ok(b) => b,
            Err(_) => return Err(PathError::Allocation),
        };
        Ok(Self { table })
    }

    pub fn is_passable(&self, id: u16) -> bool {
        self.table[id as usize]
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PathGrid {
    pub(crate) origin: PathCell,
    pub(crate) size: [u32; 3],
    pub(crate) blocks: Box<[u16]>,
    pub(crate) passability: PathBlockTable,
    pub(crate) revisions: Vec<PathRevision>,
}

impl PathGrid {
    pub fn try_new(
        origin: PathCell,
        size: [u32; 3],
        blocks: Box<[u16]>,
        passability: PathBlockTable,
        mut revisions: Vec<PathRevision>,
    ) -> Result<Self, PathError> {
        if size[0] == 0 || size[1] == 0 || size[2] == 0 {
            return Err(PathError::InvalidGrid);
        }
        let cells = size[0] as usize * size[1] as usize * size[2] as usize;
        if cells > 131072 || blocks.len() != cells {
            return Err(PathError::InvalidGrid);
        }
        #[allow(clippy::needless_range_loop)]
        for axis in 0..3 {
            let o = match axis {
                0 => origin.x as i64,
                1 => origin.y as i64,
                _ => origin.z as i64,
            };
            let far = o + (size[axis] - 1) as i64;
            if far < i32::MIN as i64 || far > i32::MAX as i64 {
                return Err(PathError::InvalidGrid);
            }
        }
        if revisions.len() > 9 {
            return Err(PathError::InvalidRevision);
        }
        revisions.sort_by_key(|a| a.chunk);
        let mut deduped: Vec<PathRevision> = Vec::with_capacity(revisions.len());
        for r in revisions {
            #[allow(clippy::collapsible_if)]
            if let Some(last) = deduped.last() {
                if last.chunk == r.chunk {
                    if last.revision != r.revision {
                        return Err(PathError::InvalidRevision);
                    }
                    continue;
                }
            }
            deduped.push(r);
        }
        Ok(Self {
            origin,
            size,
            blocks,
            passability,
            revisions: deduped,
        })
    }

    pub fn origin(&self) -> PathCell {
        self.origin
    }

    pub fn size(&self) -> [u32; 3] {
        self.size
    }

    pub fn revisions(&self) -> &[PathRevision] {
        &self.revisions
    }
}

pub struct PathScratch {
    #[allow(dead_code)]
    pub(crate) cells: usize,
}

impl PathScratch {
    pub fn try_with_capacity(cells: usize) -> Result<Self, PathError> {
        if cells > 131072 {
            return Err(PathError::InvalidGrid);
        }
        Ok(Self { cells })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PathResult {
    pub(crate) waypoints: Box<[PathCell]>,
    pub(crate) revisions: Box<[[i32; 2]]>,
}

impl PathResult {
    pub fn new(waypoints: Box<[PathCell]>, revisions: Box<[[i32; 2]]>) -> Self {
        Self {
            waypoints,
            revisions,
        }
    }

    pub fn waypoints(&self) -> &[PathCell] {
        &self.waypoints
    }

    pub fn revisions(&self) -> &[[i32; 2]] {
        &self.revisions
    }
}

pub trait PathfindOp {
    fn find(
        &self,
        grid: &PathGrid,
        start: PathCell,
        goal: PathCell,
        scratch: &mut PathScratch,
    ) -> Result<PathResult, PathError>;
}
