//! Validated compact chunk bases prepared away from the authoritative tick.

use std::collections::BTreeSet;
use std::sync::Arc;

use mornlea_domain::{BlockPos, chunk_block_index};
use mornlea_storage::{Chunk, ChunkSave, ContainerSnapshot, StorageKind};

use super::contracts::{BlockObservation, ChunkKey, RecoveredChunk, ServerError};

#[cfg(test)]
thread_local! {
    static PAGE_WORK: std::cell::Cell<(usize, usize, usize, usize)> = const { std::cell::Cell::new((0, 0, 0, 0)) };
    static MATERIALIZATIONS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    static READY_CLONES: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    static TICK_FINISHES: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

#[cfg(test)]
pub(crate) fn reset_tick_finishes() {
    TICK_FINISHES.with(|count| count.set(0));
}

#[cfg(test)]
pub(crate) fn tick_finishes() -> usize {
    TICK_FINISHES.with(std::cell::Cell::get)
}

#[cfg(test)]
pub(crate) fn reset_ready_clones() {
    READY_CLONES.with(|count| count.set(0));
}

#[cfg(test)]
pub(crate) fn ready_clones() -> usize {
    READY_CLONES.with(std::cell::Cell::get)
}

#[cfg(test)]
pub(crate) fn reset_materializations() {
    MATERIALIZATIONS.with(|count| count.set(0));
}
#[cfg(test)]
pub(crate) fn materializations() -> usize {
    MATERIALIZATIONS.with(std::cell::Cell::get)
}

/// An immutable save target. Equality identifies a capture, rather than its bytes;
/// cloning shares one owner and never traverses blocks or physical slot arrays.
#[derive(Clone)]
pub struct ChunkSaveView(Arc<ChunkCapture>);

struct ChunkCapture {
    key: ChunkKey,
    generation: u64,
    revision: u64,
    base: Arc<Chunk>,
    pages: Option<Arc<PageNode>>,
    drops: [mornlea_storage::DropSlot; 32],
    furnaces: [mornlea_storage::FurnaceSlot; 32],
    chests: [mornlea_storage::ChestSlot; 16],
}
impl std::fmt::Debug for ChunkSaveView {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ChunkSaveView")
            .field("key", &self.key())
            .field("generation", &self.generation())
            .field("revision", &self.revision())
            .finish()
    }
}
impl PartialEq for ChunkSaveView {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}
impl Eq for ChunkSaveView {}
impl ChunkSaveView {
    pub fn key(&self) -> ChunkKey {
        self.0.key
    }
    pub fn generation(&self) -> u64 {
        self.0.generation
    }
    pub fn revision(&self) -> u64 {
        self.0.revision
    }

    /// Expands the captured target off the tick. The disk owner calls this
    /// before codec validation; authority admission and polls must never do so.
    pub fn materialize(&self) -> ChunkSave {
        #[cfg(test)]
        MATERIALIZATIONS.with(|count| count.set(count.get() + 1));
        let mut chunk = (*self.0.base).clone();
        let mut converted = BTreeSet::new();
        visit_pages(&self.0.pages, 0, &mut |page, cells| {
            for (offset, block) in cells.iter().enumerate() {
                apply_cell(&mut chunk, &mut converted, page * 128 + offset, *block);
            }
        });
        chunk.drops = self.0.drops.to_vec();
        chunk.furnaces = self.0.furnaces.to_vec();
        chunk.chests = self.0.chests.to_vec();
        ChunkSave {
            key: mornlea_storage::ChunkKey {
                dimension: i32::from(self.key().dimension.get()),
                x: self.key().pos.x(),
                z: self.key().pos.z(),
            },
            revision: self.revision(),
            chunk,
        }
    }
}

// A route has exactly ten branches. Persistent path copying retains at most
// one current leaf per page; old paths exist only while a save/undo owns them.
// Keeping the fixed leaf inline makes each copied leaf one node allocation.
#[allow(clippy::large_enum_variant)]
enum PageNode {
    Branch {
        left: Option<Arc<PageNode>>,
        right: Option<Arc<PageNode>>,
    },
    Leaf([u16; 128]),
}
impl Clone for PageNode {
    fn clone(&self) -> Self {
        match self {
            Self::Branch { left, right } => {
                observe_page_work(1, 0, 0, 0);
                Self::Branch {
                    left: left.clone(),
                    right: right.clone(),
                }
            }
            Self::Leaf(cells) => {
                observe_page_work(0, 1, 0, 0);
                Self::Leaf(*cells)
            }
        }
    }
}
fn observe_page_work(branches: usize, leaves: usize, reads: usize, visits: usize) {
    #[cfg(test)]
    PAGE_WORK.with(|count| {
        let (b, l, r, v) = count.get();
        count.set((b + branches, l + leaves, r + reads, v + visits));
    });
    #[cfg(not(test))]
    let _ = (branches, leaves, reads, visits);
}
fn page_leaf(root: &Option<Arc<PageNode>>, page: usize) -> Option<&[u16; 128]> {
    let mut node = root.as_deref()?;
    for bit in (0..10).rev() {
        let PageNode::Branch { left, right } = node else {
            unreachable!("fixed page depth")
        };
        node = if page & (1 << bit) == 0 { left } else { right }.as_deref()?;
    }
    let PageNode::Leaf(cells) = node else {
        unreachable!("terminal page leaf")
    };
    Some(cells)
}
fn write_page(
    root: &mut Option<Arc<PageNode>>,
    base: &Chunk,
    page: usize,
    depth: usize,
    offset: usize,
    block: u16,
) {
    assert!(page < 768, "validated chunk cell maps to a page");
    if depth == 10 {
        if root.is_none() {
            let mut cells = [0; 128];
            for (cell, target) in cells.iter_mut().enumerate() {
                let index = page * 128 + cell;
                *target = section_block(&base.sections[index / 4096], index % 4096);
                observe_page_work(0, 0, 1, 0);
            }
            observe_page_work(0, 1, 0, 0);
            *root = Some(Arc::new(PageNode::Leaf(cells)));
        }
        let PageNode::Leaf(cells) = Arc::make_mut(root.as_mut().unwrap()) else {
            unreachable!("terminal page leaf")
        };
        cells[offset] = block;
        return;
    }
    observe_page_work(0, 0, 0, 1);
    if root.is_none() {
        observe_page_work(1, 0, 0, 0);
        *root = Some(Arc::new(PageNode::Branch {
            left: None,
            right: None,
        }));
    }
    let PageNode::Branch { left, right } = Arc::make_mut(root.as_mut().unwrap()) else {
        unreachable!("fixed page depth")
    };
    let child = if page & (1 << (9 - depth)) == 0 {
        left
    } else {
        right
    };
    write_page(child, base, page, depth + 1, offset, block);
}
fn visit_pages(root: &Option<Arc<PageNode>>, index: usize, f: &mut impl FnMut(usize, &[u16; 128])) {
    match root.as_deref() {
        None => (),
        Some(PageNode::Leaf(cells)) => f(index, cells),
        Some(PageNode::Branch { left, right }) => {
            visit_pages(left, index * 2, f);
            visit_pages(right, index * 2 + 1, f);
        }
    }
}
fn apply_cell(chunk: &mut Chunk, converted: &mut BTreeSet<usize>, index: usize, block: u16) {
    let section_index = index / 4096;
    if converted.insert(section_index) {
        let old = &chunk.sections[section_index];
        let mut packed = vec![0u64; 1024];
        for cell in 0..4096 {
            packed[cell / 4] |= u64::from(section_block(old, cell)) << ((cell % 4) * 15);
        }
        chunk.sections[section_index] = ContainerSnapshot {
            kind: StorageKind::Direct,
            bits: 15,
            single: 0,
            palette: Vec::new(),
            packed,
        };
    }
    let cell = index % 4096;
    let shift = (cell % 4) * 15;
    let word = &mut chunk.sections[section_index].packed[cell / 4];
    *word = (*word & !(0x7fffu64 << shift)) | (u64::from(block) << shift);
}

/// A load's immutable compact base, prepared entirely away from the tick.
/// Source revision, persisted revision and recovery flags remain distinct until
/// the acquisition owner chooses installation, rewrite or unload.
#[derive(Clone)]
pub struct PreparedChunk {
    ready: ReadyChunk,
    persisted_revision: u64,
    needs_rewrite: bool,
    recovered: bool,
}

impl PreparedChunk {
    pub fn try_new(
        key: ChunkKey,
        generation: u64,
        loaded: RecoveredChunk,
    ) -> Result<Self, ServerError> {
        if generation == 0 {
            return Err(ServerError::InvalidInput {
                field: "chunk_generation",
            });
        }
        if loaded.revision == 0 || loaded.persisted_revision > loaded.revision {
            return Err(ServerError::InvalidInput {
                field: "chunk_revision",
            });
        }
        Ok(Self {
            ready: ReadyChunk::try_new(key, generation, loaded.revision, loaded.chunk)?,
            persisted_revision: loaded.persisted_revision,
            needs_rewrite: loaded.needs_rewrite,
            recovered: loaded.recovered,
        })
    }

    pub fn key(&self) -> ChunkKey {
        self.ready.key
    }
    pub fn generation(&self) -> u64 {
        self.ready.generation
    }
    pub fn revision(&self) -> u64 {
        self.ready.revision
    }
    pub fn persisted_revision(&self) -> u64 {
        self.persisted_revision
    }
    pub fn needs_rewrite(&self) -> bool {
        self.needs_rewrite
    }
    pub fn recovered(&self) -> bool {
        self.recovered
    }

    /// Move the prepared base and facts into their authority owner without
    /// expanding, decoding or copying a chunk body on the tick.
    pub fn into_parts(self) -> (ReadyChunk, u64, bool, bool) {
        (
            self.ready,
            self.persisted_revision,
            self.needs_rewrite,
            self.recovered,
        )
    }
}

/// Immutable compact storage with a derived, tick-local non-air height cache.
/// Construction validates save associations and builds heights off the tick;
/// installing or cloning the value never expands or decodes its block storage.
pub struct ReadyChunk {
    pub(crate) key: ChunkKey,
    pub(crate) generation: u64,
    pub(crate) revision: u64,
    base: Arc<Chunk>,
    pages: Option<Arc<PageNode>>,
    heights: [i16; 256],
    blocks_dirty: bool,
}

impl Clone for ReadyChunk {
    fn clone(&self) -> Self {
        #[cfg(test)]
        READY_CLONES.with(|count| count.set(count.get() + 1));
        Self {
            key: self.key,
            generation: self.generation,
            revision: self.revision,
            base: Arc::clone(&self.base),
            pages: self.pages.clone(),
            heights: self.heights,
            blocks_dirty: self.blocks_dirty,
        }
    }
}

impl ReadyChunk {
    pub fn try_new(
        key: ChunkKey,
        generation: u64,
        revision: u64,
        chunk: Chunk,
    ) -> Result<Self, ServerError> {
        let save = ChunkSave {
            key: mornlea_storage::ChunkKey {
                dimension: i32::from(key.dimension.get()),
                x: key.pos.x(),
                z: key.pos.z(),
            },
            revision,
            chunk,
        };
        mornlea_storage::chunk_logical_len(&save, mornlea_storage::CHUNK_CURRENT_SCHEMA).map_err(
            |_| ServerError::InvalidInput {
                field: "ready_chunk",
            },
        )?;
        let mut ready = Self {
            key,
            generation,
            revision,
            base: Arc::new(save.chunk),
            pages: None,
            heights: [-65; 256],
            blocks_dirty: false,
        };
        for z in 0..16 {
            for x in 0..16 {
                for y in (-64..320).rev() {
                    let index = ((y + 64) * 256 + z * 16 + x) as usize;
                    if ready.block_index(index) != 0 {
                        ready.heights[(z * 16 + x) as usize] = y as i16;
                        break;
                    }
                }
            }
        }
        Ok(ready)
    }

    pub(crate) fn block(&self, position: BlockPos) -> Option<u16> {
        if !(-64..320).contains(&position.y())
            || position.x() >> 4 != self.key.pos.x()
            || position.z() >> 4 != self.key.pos.z()
        {
            return None;
        }
        Some(self.block_index(chunk_block_index(position) as usize))
    }

    fn block_index(&self, index: usize) -> u16 {
        if let Some(cells) = page_leaf(&self.pages, index / 128) {
            return cells[index % 128];
        }
        let section = &self.base.sections[index / 4096];
        section_block(section, index % 4096)
    }

    pub(crate) fn height(&self, x: i32, z: i32) -> i32 {
        i32::from(self.heights[((z & 15) * 16 + (x & 15)) as usize])
    }

    pub(crate) fn set_height(&mut self, x: i32, z: i32, y: i32) {
        self.heights[((z & 15) * 16 + (x & 15)) as usize] = y as i16;
    }

    pub(crate) fn set_block(&mut self, position: BlockPos, block: u16) {
        let index = chunk_block_index(position) as usize;
        write_page(
            &mut self.pages,
            &self.base,
            index / 128,
            0,
            index % 128,
            block,
        );
    }

    pub(crate) fn mark_blocks_dirty(&mut self) {
        self.blocks_dirty = true;
    }

    /// One durable identity covers every accepted mutation in this tick.
    /// Carried cell overlays alone never create another revision.
    pub(crate) fn pending_revision(&self, slots_dirty: bool) -> u64 {
        if self.blocks_dirty || slots_dirty {
            self.revision
                .checked_add(1)
                .expect("write preflight rejects exhausted durable revision")
        } else {
            self.revision
        }
    }

    /// Commits only revision metadata; compact data stays shared and overlays
    /// remain owned by the resident maps until an explicit save materializes them.
    pub(crate) fn finish_tick(&mut self, slots_dirty: bool) {
        #[cfg(test)]
        TICK_FINISHES.with(|count| count.set(count.get() + 1));
        self.revision = self.pending_revision(slots_dirty);
        self.blocks_dirty = false;
    }

    pub(crate) fn container_state(&self) -> super::container_store::ContainerState {
        super::container_store::ContainerState::new(&self.base, self.revision)
    }

    pub(crate) fn drop_slots(&self) -> &[mornlea_storage::DropSlot] {
        &self.base.drops
    }

    pub(crate) fn capture(
        &self,
        drops: Option<&super::drop_store::DropState>,
        containers: Option<&super::container_store::ContainerState>,
    ) -> ChunkSaveView {
        ChunkSaveView(Arc::new(ChunkCapture {
            key: self.key,
            generation: self.generation,
            revision: self.pending_revision(
                drops.is_some_and(|state| state.dirty)
                    || containers.is_some_and(|state| state.dirty),
            ),
            base: self.base.clone(),
            pages: self.pages.clone(),
            drops: drops.map(|state| state.slots).unwrap_or_else(|| {
                self.base
                    .drops
                    .as_slice()
                    .try_into()
                    .expect("validated drop slots")
            }),
            furnaces: containers.map(|state| state.furnaces).unwrap_or_else(|| {
                self.base
                    .furnaces
                    .as_slice()
                    .try_into()
                    .expect("validated furnace slots")
            }),
            chests: containers.map(|state| state.chests).unwrap_or_else(|| {
                self.base
                    .chests
                    .as_slice()
                    .try_into()
                    .expect("validated chest slots")
            }),
        }))
    }

    /// Materializes only explicit replay/save snapshots. A changed chunk gets
    /// one durable revision; per-cell overlay CAS counters are never persisted.
    pub(crate) fn snapshot<'a>(
        &self,
        writes: impl Iterator<Item = &'a BlockObservation>,
        drops: Option<&super::drop_store::DropState>,
        containers: Option<&super::container_store::ContainerState>,
    ) -> (ChunkKey, u64, u64, Chunk) {
        let view = self.capture(drops, containers);
        let mut save = view.materialize();
        let mut converted = BTreeSet::new();
        for write in writes {
            apply_cell(
                &mut save.chunk,
                &mut converted,
                chunk_block_index(write.pos) as usize,
                write.block,
            );
        }
        (self.key, self.generation, save.revision, save.chunk)
    }
}

// The F1 validator has checked all widths, lengths, palettes and indices before
// this read path is installed. Packed slots do not cross word boundaries.
fn section_block(section: &ContainerSnapshot, index: usize) -> u16 {
    match section.kind {
        StorageKind::Single => section.single,
        StorageKind::Indexed | StorageKind::Direct => {
            let per_word = 64 / usize::from(section.bits);
            let value = ((section.packed[index / per_word]
                >> ((index % per_word) * usize::from(section.bits)))
                & ((1 << section.bits) - 1)) as u16;
            if section.kind == StorageKind::Indexed {
                section.palette[usize::from(value)]
            } else {
                value
            }
        }
    }
}

#[cfg(test)]
mod chunk_view_tests {
    use super::*;
    use mornlea_domain::{ChunkPos, Dimension};

    fn ready() -> ReadyChunk {
        ReadyChunk::try_new(
            ChunkKey {
                dimension: Dimension::OVERWORLD,
                pos: ChunkPos::new(-2, -3),
            },
            7,
            5,
            Chunk {
                sections: vec![
                    ContainerSnapshot {
                        kind: StorageKind::Single,
                        bits: 0,
                        single: 2,
                        palette: Vec::new(),
                        packed: Vec::new()
                    };
                    24
                ],
                drops: vec![Default::default(); 32],
                furnaces: vec![Default::default(); 32],
                chests: vec![Default::default(); 16],
            },
        )
        .unwrap()
    }
    fn position(index: usize) -> BlockPos {
        BlockPos::new(
            -32 + (index % 16) as i32,
            -64 + (index / 256) as i32,
            -48 + ((index % 256) / 16) as i32,
        )
    }
    fn work() -> (usize, usize, usize, usize) {
        PAGE_WORK.with(std::cell::Cell::get)
    }
    fn reset_work() {
        PAGE_WORK.with(|count| count.set((0, 0, 0, 0)));
    }
    fn count(root: &Option<Arc<PageNode>>) -> (usize, usize) {
        match root.as_deref() {
            None => (0, 0),
            Some(PageNode::Leaf(_)) => (1, 1),
            Some(PageNode::Branch { left, right }) => {
                let (ln, ll) = count(left);
                let (rn, rl) = count(right);
                (1 + ln + rn, ll + rl)
            }
        }
    }
    #[test]
    fn persistent_page_writes_measure_fixed_route_and_preserve_pinned_pages() {
        let mut ready = ready();
        reset_work();
        ready.set_block(position(0), 4);
        assert_eq!(work(), (10, 1, 128, 10));
        let old = ready.capture(None, None);
        reset_work();
        ready.set_block(position(1), 5);
        assert_eq!(work(), (10, 1, 0, 10));
        let same_page = ready.capture(None, None);
        let unchanged = page_leaf(&same_page.0.pages, 0).unwrap().as_ptr();
        reset_work();
        ready.set_block(position(128), 6);
        assert_eq!(work(), (10, 1, 128, 10));
        assert_eq!(page_leaf(&ready.pages, 0).unwrap().as_ptr(), unchanged);
        assert_ne!(old, same_page);
        assert_eq!(old, old.clone());
        assert_eq!(section_block(&old.materialize().chunk.sections[0], 1), 2);
        assert_eq!(
            section_block(&same_page.materialize().chunk.sections[0], 1),
            5
        );
        assert_eq!(
            section_block(&same_page.materialize().chunk.sections[0], 128),
            2
        );
        for edit in 0..10_000 {
            ready.set_block(position(0), (edit % 100 + 1) as u16);
        }
        assert_eq!(count(&ready.pages), (12, 2));
        assert_eq!(ready.block(position(0)), Some(100));
    }
    #[test]
    fn every_page_and_world_boundary_materializes_final_values_without_history() {
        let mut ready = ready();
        for page in 0..768 {
            ready.set_block(position(page * 128), (page + 1) as u16);
            ready.set_block(position(page * 128 + 127), (page + 2) as u16);
        }
        assert_eq!(count(&ready.pages), (1536, 768));
        let view = ready.capture(None, None);
        let saved = view.materialize();
        for page in 0..768 {
            for (offset, expected) in [(0, (page + 1) as u16), (127, (page + 2) as u16), (1, 2)] {
                let index = page * 128 + offset;
                assert_eq!(ready.block(position(index)), Some(expected));
                assert_eq!(
                    section_block(&saved.chunk.sections[index / 4096], index % 4096),
                    expected
                );
            }
        }
        assert_eq!(position(0), BlockPos::new(-32, -64, -48));
        assert_eq!(position(98_303), BlockPos::new(-17, 319, -33));
        assert_eq!(
            ready.snapshot(std::iter::empty(), None, None).3,
            saved.chunk
        );
        assert_ne!(view, ready.capture(None, None));
        assert_eq!(view.materialize(), ready.capture(None, None).materialize());
    }
    #[test]
    fn untouched_sections_preserve_original_compact_layout() {
        let mut ready = ready();
        let original = ready.base.sections[1].clone();
        ready.set_block(position(0), 32767);
        let saved = ready.capture(None, None).materialize();
        assert_eq!(saved.chunk.sections[1], original);
        assert_eq!(saved.chunk.sections[0].bits, 15);
        assert_eq!(saved.chunk.sections[0].packed.len(), 1024);
        assert_eq!(section_block(&saved.chunk.sections[0], 0), 32767);
    }
}
