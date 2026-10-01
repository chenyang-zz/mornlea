//! Validated compact chunk bases prepared away from the authoritative tick.

use std::collections::BTreeSet;
use std::sync::Arc;

use mornlea_domain::{BlockPos, chunk_block_index};
use mornlea_storage::{Chunk, ChunkSave, ContainerSnapshot, StorageKind};

use super::container_store::ContainerState;
use super::contracts::{BlockObservation, ChunkKey, RecoveredChunk, ServerError};
use super::drop_store::DropState;

#[cfg(test)]
thread_local! {
    // Logical prepared cells, differing updates, copied counts, recounted entries.
    static PAYLOAD_WORK: std::cell::Cell<(usize, usize, usize, usize)> = const { std::cell::Cell::new((0, 0, 0, 0)) };
    static PAGE_WORK: std::cell::Cell<(usize, usize, usize, usize)> = const { std::cell::Cell::new((0, 0, 0, 0)) };
    static MATERIALIZATIONS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    static READY_CLONES: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    static TICK_FINISHES: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    static NETWORK_EXPANSIONS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

#[cfg(test)]
pub(crate) fn reset_payload_work() {
    PAYLOAD_WORK.with(|count| count.set((0, 0, 0, 0)));
}

#[cfg(test)]
pub(crate) fn payload_work() -> (usize, usize, usize, usize) {
    PAYLOAD_WORK.with(std::cell::Cell::get)
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
    network_compact_sections: u32,
    estimate_valid: bool,
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

    /// Extracts owned checked network sections off tick; the byte charge counts
    /// section payloads only, excluding headers, framing and persistence slots.
    /// Callers must keep copying, validation and packing outside the authoritative tick.
    pub fn network_snapshot(&self) -> Result<(mornlea_domain::ChunkSnapshot, usize), ServerError> {
        use mornlea_domain::{ChunkSnapshot, ChunkSnapshotParts, PalettedSection};
        let invalid = || ServerError::InvalidInput {
            field: "chunk_network_snapshot",
        };
        // Private unchecked edits retain their legacy disk pages, but cannot
        // cross the checked network boundary, even if later edited back.
        if !self.0.estimate_valid || self.generation() == 0 || self.revision() == 0 {
            return Err(invalid());
        }
        let mut sections = Vec::with_capacity(24);
        let mut bytes = 0;
        for index in 0..24 {
            let (section, charge) = if self.0.network_compact_sections & (1 << index) == 0 {
                // Untouched loaded storage keeps its exact palette and words,
                // including unused entries and an uncompact Direct section.
                let source = &self.0.base.sections[index];
                match source.kind {
                    StorageKind::Single => (PalettedSection::single(source.single), 2),
                    StorageKind::Indexed => (
                        PalettedSection::indexed(
                            source.bits,
                            source.palette.clone().into_boxed_slice(),
                            source.packed.clone().into_boxed_slice(),
                        ),
                        source.packed.len() * 8 + source.palette.len() * 2,
                    ),
                    StorageKind::Direct => (
                        PalettedSection::direct(source.packed.clone().into_boxed_slice()),
                        source.packed.len() * 8,
                    ),
                }
            } else {
                // First appearance in YZX order matches source compaction.
                // Only one section's cell-index scratch and fixed lookup live
                // here; extraction acquires no frequency owners or physical slots.
                let mut lookup = [u8::MAX; 90];
                let mut palette = Vec::with_capacity(90);
                let mut cells = [0u8; 4096];
                for page_offset in 0..32 {
                    let page = index * 32 + page_offset;
                    let leaf = page_leaf(&self.0.pages, page);
                    for offset in 0..128 {
                        let cell = page_offset * 128 + offset;
                        let block = leaf.map_or_else(
                            || section_block(&self.0.base.sections[index], cell),
                            |values| values[offset],
                        );
                        #[cfg(test)]
                        NETWORK_EXPANSIONS.with(|count| count.set(count.get() + 1));
                        let slot = lookup.get_mut(usize::from(block)).ok_or_else(invalid)?;
                        if *slot == u8::MAX {
                            *slot = palette.len() as u8;
                            palette.push(block);
                        }
                        cells[cell] = *slot;
                    }
                }
                if palette.len() == 1 {
                    (PalettedSection::single(palette[0]), 2)
                } else {
                    let bits = if palette.len() <= 16 { 4 } else { 8 };
                    let per_word = 64 / usize::from(bits);
                    let mut words = vec![0; 4096 / per_word];
                    for (cell, slot) in cells.iter().enumerate() {
                        words[cell / per_word] |=
                            u64::from(*slot) << ((cell % per_word) * usize::from(bits));
                    }
                    let charge = words.len() * 8 + palette.len() * 2;
                    (
                        PalettedSection::indexed(
                            bits,
                            palette.into_boxed_slice(),
                            words.into_boxed_slice(),
                        ),
                        charge,
                    )
                }
            };
            sections.push(section.map_err(|_| invalid())?);
            bytes += charge;
        }
        let sections = sections
            .into_boxed_slice()
            .try_into()
            .map_err(|_| invalid())?;
        let snapshot = ChunkSnapshot::try_new(ChunkSnapshotParts {
            dimension: self.key().dimension,
            chunk: self.key().pos,
            revision: self.revision(),
            sections,
        })
        .map_err(|_| invalid())?;
        Ok((snapshot, bytes))
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
    drops: DropState,
    containers: ContainerState,
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
        let ready = ReadyChunk::try_new(key, generation, loaded.revision, loaded.chunk)?;
        let drops = DropState::new(
            key,
            ready
                .drop_slots()
                .try_into()
                .expect("validated fixed drops"),
        );
        let containers = ready.container_state();
        Ok(Self {
            ready,
            drops,
            containers,
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

    /// Moves fixed slot owners prepared off tick alongside the immutable base.
    pub(crate) fn into_live_parts(
        self,
    ) -> (ReadyChunk, DropState, ContainerState, u64, bool, bool) {
        (
            self.ready,
            self.drops,
            self.containers,
            self.persisted_revision,
            self.needs_rewrite,
            self.recovered,
        )
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

// Frequencies belong only to Ready/undo owners, never to immutable save views.
// The registered block sentinel bounds each copy independently of section cells.
struct SectionFrequencies {
    counts: [u16; 90],
}

impl Clone for SectionFrequencies {
    fn clone(&self) -> Self {
        #[cfg(test)]
        PAYLOAD_WORK.with(|count| {
            let (prepared, updates, copied, scans) = count.get();
            count.set((prepared, updates, copied + 90, scans));
        });
        Self {
            counts: self.counts,
        }
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
    frequencies: [Arc<SectionFrequencies>; 24],
    section_estimates: [usize; 24],
    changed_sections: u32,
    // Base storage never rebases; this union survives tick finalization.
    network_compact_sections: u32,
    estimated_payload: usize,
    estimate_valid: bool,
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
            frequencies: self.frequencies.clone(),
            section_estimates: self.section_estimates,
            changed_sections: self.changed_sections,
            network_compact_sections: self.network_compact_sections,
            estimated_payload: self.estimated_payload,
            estimate_valid: self.estimate_valid,
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
        // Preserve the loaded representation, including unused palette entries.
        // Physical slots cost the source fixed amount regardless of active flags.
        let section_estimates = std::array::from_fn(|index| {
            let section = &save.chunk.sections[index];
            match section.kind {
                StorageKind::Single => 0,
                StorageKind::Indexed => section.packed.len() * 8 + section.palette.len() * 10,
                StorageKind::Direct => section.packed.len() * 8,
            }
        });
        let frequencies = std::array::from_fn(|index| {
            let section = &save.chunk.sections[index];
            let mut counts = [0u16; 90];
            if section.kind == StorageKind::Single {
                counts[usize::from(section.single)] = 4096;
            } else {
                for cell in 0..4096 {
                    counts[usize::from(section_block(section, cell))] += 1;
                }
            }
            #[cfg(test)]
            PAYLOAD_WORK.with(|count| {
                let (prepared, updates, copied, scans) = count.get();
                count.set((prepared + 4096, updates, copied, scans));
            });
            Arc::new(SectionFrequencies { counts })
        });
        let estimated_payload =
            512 + 32 * 19 + 32 * 21 + 16 * 144 + section_estimates.iter().sum::<usize>();
        let mut ready = Self {
            key,
            generation,
            revision,
            base: Arc::new(save.chunk),
            pages: None,
            heights: [-65; 256],
            blocks_dirty: false,
            frequencies,
            section_estimates,
            changed_sections: 0,
            network_compact_sections: 0,
            estimated_payload,
            estimate_valid: true,
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
        let previous = self.block_index(index);
        if previous != block {
            self.network_compact_sections |= 1 << (index / 4096);
        }
        if self.estimate_valid && previous != block {
            if mornlea_domain::registered_block(previous) && mornlea_domain::registered_block(block)
            {
                let section = index / 4096;
                let frequencies = Arc::make_mut(&mut self.frequencies[section]);
                frequencies.counts[usize::from(previous)] -= 1;
                frequencies.counts[usize::from(block)] += 1;
                self.changed_sections |= 1 << section;
                #[cfg(test)]
                PAYLOAD_WORK.with(|count| {
                    let (prepared, updates, copied, scans) = count.get();
                    count.set((prepared, updates + 1, copied, scans));
                });
            } else {
                // Private fixtures can write outside the checked production domain.
                // Retain their pages, but never publish a fabricated source estimate.
                self.estimate_valid = false;
            }
        }
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

    /// Commits durable identity and recounts only changed fixed frequency records.
    /// Persistent pages already contain writes; body expansion stays off tick.
    pub(crate) fn finish_tick(&mut self, slots_dirty: bool) {
        #[cfg(test)]
        TICK_FINISHES.with(|count| count.set(count.get() + 1));
        self.revision = self.pending_revision(slots_dirty);
        if self.estimate_valid {
            while self.changed_sections != 0 {
                let section = self.changed_sections.trailing_zeros() as usize;
                self.changed_sections &= !(1 << section);
                let used = self.frequencies[section]
                    .counts
                    .iter()
                    .filter(|count| **count != 0)
                    .count();
                #[cfg(test)]
                PAYLOAD_WORK.with(|count| {
                    let (prepared, updates, copied, scans) = count.get();
                    count.set((prepared, updates, copied, scans + 90));
                });
                // Source `Compact` rebuilds only modified sections at commit.
                let estimate = match used {
                    1 => 0,
                    2..=16 => 2048 + used * 10,
                    17..=90 => 4096 + used * 10,
                    _ => unreachable!("validated frequencies cover one section"),
                };
                self.estimated_payload =
                    self.estimated_payload - self.section_estimates[section] + estimate;
                self.section_estimates[section] = estimate;
            }
        }
        self.changed_sections = 0;
        self.blocks_dirty = false;
    }

    /// Source logical scheduling metric, independent of RSS and store reservation.
    /// Checked production writes preserve validity; private unregistered writes
    /// make this cache unavailable for the remainder of that Ready owner's life.
    pub(crate) fn payload_estimate(&self) -> Option<usize> {
        self.estimate_valid.then_some(self.estimated_payload)
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
            network_compact_sections: self.network_compact_sections,
            estimate_valid: self.estimate_valid,
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

#[cfg(test)]
mod payload_estimate_tests {
    use super::*;
    use mornlea_domain::{ChunkPos, Dimension};

    fn key() -> ChunkKey {
        ChunkKey {
            dimension: Dimension::OVERWORLD,
            pos: ChunkPos::new(0, 0),
        }
    }
    fn single(block: u16) -> ContainerSnapshot {
        ContainerSnapshot {
            kind: StorageKind::Single,
            bits: 0,
            single: block,
            palette: vec![],
            packed: vec![],
        }
    }
    fn chunk(section: ContainerSnapshot) -> Chunk {
        let mut sections = vec![single(0); 24];
        sections[0] = section;
        Chunk {
            sections,
            drops: vec![Default::default(); 32],
            furnaces: vec![Default::default(); 32],
            chests: vec![Default::default(); 16],
        }
    }
    fn ready(section: ContainerSnapshot) -> ReadyChunk {
        ReadyChunk::try_new(key(), 7, 5, chunk(section)).unwrap()
    }
    fn position(index: usize) -> BlockPos {
        BlockPos::new(
            (index % 16) as i32,
            -64 + (index / 256) as i32,
            ((index % 256) / 16) as i32,
        )
    }
    fn indexed(bits: u8, n: u16) -> ContainerSnapshot {
        ContainerSnapshot {
            kind: StorageKind::Indexed,
            bits,
            single: 0,
            palette: (0..n).collect(),
            packed: vec![0; 4096 / (64 / usize::from(bits))],
        }
    }
    fn direct() -> ContainerSnapshot {
        ContainerSnapshot {
            kind: StorageKind::Direct,
            bits: 15,
            single: 0,
            palette: vec![],
            packed: vec![0; 1024],
        }
    }

    #[test]
    fn initial_source_formula_preserves_unused_palette_and_direct_storage() {
        for (section, expected) in [
            (single(89), 4096),
            (indexed(4, 16), 6304),
            (indexed(8, 90), 9092),
            (direct(), 12288),
        ] {
            let mut r = ready(section);
            assert_eq!(r.payload_estimate(), Some(expected));
            r.finish_tick(false);
            r.finish_tick(true);
            assert_eq!(r.payload_estimate(), Some(expected));
            r.set_block(position(0), r.block(position(0)).unwrap());
            r.finish_tick(false);
            assert_eq!(r.payload_estimate(), Some(expected));
        }
        assert!(ReadyChunk::try_new(key(), 7, 5, chunk(single(90))).is_err());
        let mut invalid = indexed(4, 2);
        invalid.palette[1] = 90;
        assert!(ReadyChunk::try_new(key(), 7, 5, chunk(invalid)).is_err());
        let mut invalid = direct();
        invalid.packed[0] = 90;
        assert!(ReadyChunk::try_new(key(), 7, 5, chunk(invalid)).is_err());
    }

    #[test]
    fn compact_formula_recounts_only_changed_sections_and_preserves_pinned_metadata() {
        reset_payload_work();
        let mut r = ready(direct());
        assert_eq!(payload_work(), (24 * 4096, 0, 0, 0));
        assert!(
            r.frequencies
                .iter()
                .all(|f| f.counts.iter().map(|n| usize::from(*n)).sum::<usize>() == 4096)
        );
        let pinned = r.clone();
        let save = r.capture(None, None);
        reset_payload_work();
        reset_ready_clones();
        reset_materializations();
        r.set_block(position(0), 1);
        r.set_block(position(1), 1);
        r.set_block(position(1), 1);
        assert_eq!(payload_work(), (0, 2, 90, 0));
        for section in 0..24 {
            assert_eq!(
                Arc::ptr_eq(&r.frequencies[section], &pinned.frequencies[section]),
                section != 0
            );
        }
        r.mark_blocks_dirty();
        r.finish_tick(false);
        assert_eq!(payload_work(), (0, 2, 90, 90));
        assert_eq!(r.payload_estimate(), Some(6164));
        assert_eq!(pinned.payload_estimate(), Some(12288));
        assert_eq!(pinned.frequencies[0].counts[0], 4096);
        assert_eq!((ready_clones(), materializations()), (0, 0));
        assert_eq!(Arc::strong_count(&r.frequencies[0]), 1);
        reset_payload_work();
        r.finish_tick(false);
        r.finish_tick(true);
        r.set_block(position(1), 1);
        r.finish_tick(false);
        assert_eq!(payload_work(), (0, 0, 0, 0));
        drop(pinned);
        r.set_block(position(4096), 89);
        r.finish_tick(false);
        assert_eq!(payload_work(), (0, 1, 0, 90));
        assert_eq!(r.payload_estimate(), Some(8232));
        assert_eq!(save.revision(), 5);
    }

    #[test]
    fn all_changed_sections_scan_at_most_fixed_frequency_records() {
        let mut r = ready(single(0));
        reset_payload_work();
        for section in 0..24 {
            r.set_block(position(section * 4096), 89);
        }
        r.finish_tick(false);
        assert_eq!(payload_work(), (0, 24, 0, 24 * 90));
        assert_eq!(r.payload_estimate(), Some(4096 + 24 * 2068));
    }

    #[test]
    fn physical_slot_values_do_not_change_fixed_source_estimate() {
        let mut c = chunk(single(0));
        c.drops[31].generation = 19;
        c.furnaces[31].generation = 20;
        c.chests[15].generation = 21;
        assert_eq!(
            ReadyChunk::try_new(key(), 7, 5, c)
                .unwrap()
                .payload_estimate(),
            Some(4096)
        );
        let mut r = ready(single(0));
        r.set_block(position(0), 11);
        r.set_block(position(1), 9);
        r.finish_tick(true);
        let mut containers = r.container_state();
        containers.chests[0].generation = 1;
        containers.chests[0].active = true;
        containers.chests[0].block_index = 0;
        containers.furnaces[0].generation = 1;
        containers.furnaces[0].active = true;
        containers.furnaces[0].block_index = 1;
        let mut drops = DropState::new(key(), [Default::default(); 32]);
        drops.slots[31] = mornlea_storage::DropSlot {
            generation: 19,
            active: true,
            stack: mornlea_storage::ItemStack {
                item: 2,
                count: 1,
                durability: 0,
            },
            block_index: 4095,
            age_ticks: 7,
            pickup_delay_ticks: 5,
        };
        let capture = r.capture(Some(&drops), Some(&containers));
        assert_eq!(r.payload_estimate(), Some(6174));
        assert!(capture.materialize().chunk.drops[31].active);
        assert!(capture.materialize().chunk.chests[0].active);
        assert!(capture.materialize().chunk.furnaces[0].active);
    }

    #[test]
    fn unregistered_private_writes_refuse_estimates_without_breaking_pages() {
        let mut r = ready(single(0));
        reset_payload_work();
        r.set_block(position(1), 1);
        r.set_block(position(0), 32767);
        r.mark_blocks_dirty();
        r.finish_tick(false);
        assert_eq!(r.revision, 6);
        assert_eq!(r.changed_sections, 0);
        assert_eq!(payload_work(), (0, 1, 0, 0));
        assert_eq!(r.payload_estimate(), None);
        assert_eq!(r.block(position(0)), Some(32767));
        assert_eq!(
            section_block(&r.capture(None, None).materialize().chunk.sections[0], 0),
            32767
        );
        reset_payload_work();
        r.set_block(position(0), 1);
        r.mark_blocks_dirty();
        r.finish_tick(false);
        assert_eq!(r.revision, 7);
        assert_eq!(r.block(position(0)), Some(1));
        assert_eq!(payload_work(), (0, 0, 0, 0));
        assert_eq!(r.payload_estimate(), None);
    }

    #[test]
    fn actual_unchanged_go_new_chunk_set_compact_matches_ready_sequence() {
        use std::{
            fs,
            io::Read,
            process::{Child, Command, Stdio},
            time::{Duration, Instant, SystemTime, UNIX_EPOCH},
        };
        struct Root(std::path::PathBuf);
        impl Drop for Root {
            fn drop(&mut self) {
                let _ = fs::remove_dir_all(&self.0);
            }
        }
        struct OwnedChild(Child);
        impl Drop for OwnedChild {
            fn drop(&mut self) {
                let _ = self.0.kill();
                let _ = self.0.wait();
            }
        }
        let root = Root(std::env::temp_dir().join(format!(
                "mornlea-payload-go-{}-{}",
                std::process::id(),
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            )));
        fs::create_dir(&root.0).unwrap();
        let source = root.0.join("main.go");
        fs::write(
            &source,
            r#"package main
import (
 "fmt"
 "github.com/channing771/mornlea/packages/shared/core"
 "github.com/channing771/mornlea/packages/shared/world"
)
func main() {
 c := world.NewChunk(core.ChunkPos{})
 b := c.Section(0).Blocks
 fmt.Println(c.PayloadBytes())
 b.Set(0, 0, 0, 1); b.Compact(); fmt.Println(c.PayloadBytes())
 for i := 1; i < 15; i++ { b.Set(i, 0, 0, world.BlockID(i+1)) }
 b.Compact(); fmt.Println(c.PayloadBytes())
 b.Set(15, 0, 0, 16); b.Compact(); fmt.Println(c.PayloadBytes())
 for i := 0; i < 4096; i++ { b.Set(i%16, i/256, (i%256)/16, 1) }
 b.Compact(); fmt.Println(c.PayloadBytes())
}
"#,
        )
        .unwrap();
        let repository = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../..");
        let stdout = root.0.join("stdout");
        let stderr = root.0.join("stderr");
        let mut child = OwnedChild(
            Command::new("go")
                .arg("run")
                .arg(&source)
                .current_dir(repository)
                .stdin(Stdio::null())
                .stdout(fs::File::create(&stdout).unwrap())
                .stderr(fs::File::create(&stderr).unwrap())
                .spawn()
                .expect("actual Go compiler is required"),
        );
        let deadline = Instant::now() + Duration::from_secs(120);
        let status = loop {
            if let Some(status) = child.0.try_wait().unwrap() {
                break status;
            }
            assert!(Instant::now() < deadline, "actual Go helper timed out");
            std::thread::sleep(Duration::from_millis(10));
        };
        let bounded_text = |path: &std::path::Path| {
            let mut text = String::new();
            fs::File::open(path)
                .unwrap()
                .take(4097)
                .read_to_string(&mut text)
                .unwrap();
            assert!(text.len() <= 4096, "Go report exceeded its bound");
            text
        };
        assert!(
            status.success(),
            "Go helper failed: {}",
            bounded_text(&stderr)
        );
        let actual: Vec<usize> = bounded_text(&stdout)
            .split_whitespace()
            .map(|s| s.parse().unwrap())
            .collect();
        assert_eq!(actual, [4096, 6164, 6304, 8362, 4096]);
        let mut r = ready(single(0));
        let mut estimates = vec![r.payload_estimate().unwrap()];
        r.set_block(position(0), 1);
        r.finish_tick(false);
        estimates.push(r.payload_estimate().unwrap());
        for i in 1..15 {
            r.set_block(position(i), (i + 1) as u16);
        }
        r.finish_tick(false);
        estimates.push(r.payload_estimate().unwrap());
        r.set_block(position(15), 16);
        r.finish_tick(false);
        estimates.push(r.payload_estimate().unwrap());
        for i in 0..4096 {
            r.set_block(position(i), 1);
        }
        r.finish_tick(false);
        estimates.push(r.payload_estimate().unwrap());
        assert_eq!(estimates, actual);
    }
}

#[cfg(test)]
mod network_view_tests {
    use super::*;
    use mornlea_domain::{ChunkPos, Dimension};
    use mornlea_protocol::{ChunkSnapshot as WireSnapshot, ServerPacket};

    fn single(block: u16) -> ContainerSnapshot {
        ContainerSnapshot {
            kind: StorageKind::Single,
            bits: 0,
            single: block,
            palette: vec![],
            packed: vec![],
        }
    }

    fn indexed(bits: u8, palette: Vec<u16>) -> ContainerSnapshot {
        ContainerSnapshot {
            kind: StorageKind::Indexed,
            bits,
            single: 0,
            palette,
            packed: vec![0; 4096 / (64 / usize::from(bits))],
        }
    }

    fn direct() -> ContainerSnapshot {
        ContainerSnapshot {
            kind: StorageKind::Direct,
            bits: 15,
            single: 0,
            palette: vec![],
            packed: vec![0; 1024],
        }
    }

    fn ready(sections: Vec<ContainerSnapshot>) -> ReadyChunk {
        ReadyChunk::try_new(
            ChunkKey {
                dimension: Dimension::DEPTHS,
                pos: ChunkPos::new(-2, 3),
            },
            7,
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

    fn position(index: usize) -> BlockPos {
        BlockPos::new(
            -32 + (index % 16) as i32,
            -64 + (index / 256) as i32,
            48 + ((index % 256) / 16) as i32,
        )
    }

    fn commit(r: &mut ReadyChunk) {
        r.mark_blocks_dirty();
        r.finish_tick(false);
    }

    fn wire(view: &ChunkSaveView) -> (WireSnapshot, usize) {
        let (snapshot, bytes) = view.network_snapshot().unwrap();
        let ServerPacket::ChunkSnapshot(packet) =
            ServerPacket::try_from(mornlea_domain::Event::ChunkSnapshot(snapshot)).unwrap()
        else {
            panic!("snapshot conversion changed packet family")
        };
        packet.validate().unwrap();
        assert_eq!(
            bytes,
            packet
                .sections
                .iter()
                .map(|s| s.payload_bytes())
                .sum::<usize>()
        );
        let logical = packet.encode_logical_checked().unwrap();
        assert_eq!(WireSnapshot::decode_logical(&logical).unwrap(), packet);
        (packet, bytes)
    }

    #[test]
    fn air_network_charge_excludes_disk_slots_and_envelope() {
        let r = ready(vec![single(0); 24]);
        let view = r.capture(None, None);
        let (snapshot, bytes) = wire(&view);
        assert_eq!(bytes, 48);
        assert_eq!(r.payload_estimate(), Some(4096));
        assert_eq!(snapshot.dimension, Dimension::DEPTHS);
        assert_eq!(
            (snapshot.chunk_x, snapshot.chunk_z, snapshot.revision),
            (-2, 3, 9)
        );
        assert!(snapshot.sections.iter().all(|s| s.single == 0));
        assert!(snapshot.logical_size() > bytes);
    }

    #[test]
    fn untouched_loaded_storage_survives_equal_writes_and_slot_commits() {
        let mut sections = vec![single(0); 24];
        sections[0] = single(89);
        sections[1] = indexed(4, vec![2, 1, 0, 89]);
        sections[2] = indexed(8, (0..90).rev().collect());
        sections[3] = direct();
        sections[3].packed[0] = 1 | (17 << 15) | (89 << 45);
        let mut r = ready(sections.clone());
        let before = r.capture(None, None);
        for section in 0..24 {
            let p = position(section * 4096);
            r.set_block(p, r.block(p).unwrap());
        }
        r.finish_tick(true);
        let mut drops = DropState::new(r.key, [Default::default(); 32]);
        drops.slots[31].generation = 19;
        drops.slots[31].age_ticks = 123;
        let after = r.capture(Some(&drops), None);
        let (initial, bytes) = wire(&before);
        let (current, current_bytes) = wire(&after);
        assert_eq!(initial.sections, current.sections);
        assert_eq!(bytes, current_bytes);
        assert_eq!((initial.revision, current.revision), (9, 10));
        for (i, source) in sections.iter().enumerate() {
            let section = &current.sections[i];
            assert_eq!(section.y, i as i32);
            assert_eq!(section.single, source.single);
            assert_eq!(section.bits, source.bits);
            assert_eq!(section.palette, source.palette);
            assert_eq!(section.packed, source.packed);
            assert_eq!(section.storage.wire(), source.kind as u8);
        }
        assert_eq!(after.materialize().chunk.drops[31].age_ticks, 123);
    }

    #[test]
    fn changed_sections_pack_first_appearance_in_yzx_order_across_words() {
        let mut r = ready(vec![single(0); 24]);
        for (i, block) in [(0, 1), (15, 17), (16, 2), (255, 3), (256, 4), (4095, 89)] {
            r.set_block(position(i), block);
        }
        commit(&mut r);
        let view = r.capture(None, None);
        let (packet, bytes) = wire(&view);
        let s = &packet.sections[0];
        assert_eq!(
            (s.bits, s.palette.as_slice()),
            (4, &[1, 0, 17, 2, 3, 4, 89][..])
        );
        assert_eq!(s.packed.len(), 256);
        assert_eq!(s.packed[0], 0x2111_1111_1111_1110);
        assert_eq!(s.packed[1], 0x1111_1111_1111_1113);
        assert_eq!(s.packed[15], 0x4111_1111_1111_1111);
        assert_eq!(s.packed[16], 0x1111_1111_1111_1115);
        assert_eq!(s.packed[255], 0x6111_1111_1111_1111);
        assert_eq!(bytes, 2048 + 14 + 46);
        let disk = view.materialize();
        assert_eq!(disk.chunk.sections[0].kind, StorageKind::Direct);
        assert_eq!(disk.chunk.sections[0].bits, 15);
        let snapshot = view.network_snapshot().unwrap().0;
        for i in 0..4096 {
            assert_eq!(snapshot.sections()[0].block_at(i), r.block(position(i)));
        }
    }

    #[test]
    fn palette_width_boundary_and_uniform_downgrade_keep_old_captures() {
        let mut r = ready(vec![single(0); 24]);
        for i in 0..15 {
            r.set_block(position(i), (i + 1) as u16);
        }
        commit(&mut r);
        let narrow = r.capture(None, None);
        r.set_block(position(15), 16);
        commit(&mut r);
        let wide = r.capture(None, None);
        for i in 0..4096 {
            r.set_block(position(i), 1);
        }
        commit(&mut r);
        let uniform = r.capture(None, None);
        assert_ne!(narrow, wide);
        assert_eq!(narrow, narrow.clone());
        let (narrow_packet, narrow_bytes) = wire(&narrow);
        let (wide_packet, wide_bytes) = wire(&wide);
        let (uniform_packet, uniform_bytes) = wire(&uniform);
        assert_eq!(narrow_packet.sections[0].bits, 4);
        assert_eq!(
            narrow_packet.sections[0].palette,
            (1..16).chain([0]).collect::<Vec<_>>()
        );
        assert_eq!(wide_packet.sections[0].bits, 8);
        assert_eq!(
            wide_packet.sections[0].palette,
            (1..17).chain([0]).collect::<Vec<_>>()
        );
        assert_eq!(wide_packet.sections[0].packed[0], 0x0706_0504_0302_0100);
        assert_eq!(wide_packet.sections[0].packed[1], 0x0f0e_0d0c_0b0a_0908);
        assert_eq!((narrow_bytes, wide_bytes, uniform_bytes), (2126, 4176, 48));
        assert_eq!(uniform_packet.sections[0].single, 1);
        assert_eq!(
            (
                narrow_packet.revision,
                wide_packet.revision,
                uniform_packet.revision
            ),
            (10, 11, 12)
        );
        assert_eq!(wire(&narrow).0, narrow_packet);
        assert_eq!(wire(&wide).0, wide_packet);
    }

    #[test]
    fn reverted_loaded_section_remains_in_changed_union_across_commits() {
        let mut sections = vec![single(0); 24];
        sections[0] = direct();
        let mut r = ready(sections);
        let loaded = r.capture(None, None);
        r.set_block(position(0), 1);
        commit(&mut r);
        let changed = r.capture(None, None);
        r.set_block(position(0), 0);
        commit(&mut r);
        let reverted = r.capture(None, None);
        r.finish_tick(true);
        assert_eq!(wire(&loaded).0.sections[0].bits, 15);
        assert_eq!(wire(&changed).0.sections[0].palette, [1, 0]);
        assert_eq!(wire(&reverted).1, 48);
        assert_eq!(wire(&r.capture(None, None)).0.sections[0].single, 0);
        assert_eq!(wire(&reverted).0.revision, 11);
        assert_eq!(reverted.materialize().chunk.sections[0].bits, 15);
    }

    #[test]
    fn invalid_private_captures_refuse_before_expansion_preserving_legacy_pages() {
        let mut r = ready(vec![single(0); 24]);
        let valid = r.capture(None, None);
        r.set_block(position(0), 32767);
        commit(&mut r);
        let invalid = r.capture(None, None);
        r.set_block(position(0), 1);
        commit(&mut r);
        let repaired = r.capture(None, None);
        let mut clean = ready(vec![single(0); 24]);
        clean.generation = 0;
        let zero_generation = clean.capture(None, None);
        clean.generation = 7;
        clean.revision = 0;
        let zero_revision = clean.capture(None, None);
        NETWORK_EXPANSIONS.with(|count| count.set(0));
        reset_materializations();
        for capture in [&invalid, &repaired, &zero_generation, &zero_revision] {
            assert_eq!(
                capture.network_snapshot(),
                Err(ServerError::InvalidInput {
                    field: "chunk_network_snapshot",
                })
            );
        }
        assert_eq!(NETWORK_EXPANSIONS.with(std::cell::Cell::get), 0);
        assert_eq!(materializations(), 0);
        assert_eq!(invalid.revision(), 10);
        assert_eq!(
            section_block(&invalid.materialize().chunk.sections[0], 0),
            32767
        );
        assert_eq!(
            section_block(&repaired.materialize().chunk.sections[0], 0),
            1
        );
        assert_eq!(wire(&valid).1, 48);
    }

    #[test]
    fn all_section_changes_and_ready_clones_preserve_network_union() {
        let mut r = ready(vec![single(0); 24]);
        for section in 0..24 {
            r.set_block(position(section * 4096 + 4095), 89);
        }
        commit(&mut r);
        let cloned = r.clone();
        let view = cloned.capture(None, None);
        NETWORK_EXPANSIONS.with(|count| count.set(0));
        let (packet, bytes) = wire(&view);
        assert_eq!(bytes, 24 * 2052);
        assert_eq!(NETWORK_EXPANSIONS.with(std::cell::Cell::get), 24 * 4096);
        for section in &packet.sections {
            assert_eq!(section.palette, [0, 89]);
            assert_eq!(section.bits, 4);
            assert_eq!(&section.packed[..255], &[0; 255]);
            assert_eq!(section.packed[255], 1 << 60);
        }
        assert_eq!(wire(&r.capture(None, None)).0, packet);
    }

    #[test]
    fn checked_section_validation_refuses_malformed_private_sources() {
        let r = ready(vec![single(0); 24]);
        let mut malformed_indexed = indexed(4, vec![0, 1]);
        malformed_indexed.packed[0] = 2;
        let mut malformed_direct = direct();
        malformed_direct.packed[0] = 1 << 60;
        for source in [
            single(90),
            indexed(8, vec![0, 0]),
            malformed_indexed,
            malformed_direct,
        ] {
            let mut capture = r.capture(None, None);
            let inner = Arc::get_mut(&mut capture.0).unwrap();
            Arc::make_mut(&mut inner.base).sections[0] = source;
            assert_eq!(
                capture.network_snapshot(),
                Err(ServerError::InvalidInput {
                    field: "chunk_network_snapshot",
                })
            );
        }
    }

    #[test]
    fn ordinary_writes_commits_and_captures_do_no_network_expansion() {
        let mut r = ready(vec![single(0); 24]);
        let pinned = r.capture(None, None);
        NETWORK_EXPANSIONS.with(|count| count.set(0));
        PAGE_WORK.with(|count| count.set((0, 0, 0, 0)));
        reset_payload_work();
        reset_ready_clones();
        reset_materializations();
        r.set_block(position(0), 1);
        r.set_block(position(1), 1);
        r.set_block(position(1), 1);
        commit(&mut r);
        let view = r.capture(None, None);
        assert_eq!(NETWORK_EXPANSIONS.with(std::cell::Cell::get), 0);
        assert_eq!((ready_clones(), materializations()), (0, 0));
        assert_eq!(PAGE_WORK.with(std::cell::Cell::get), (10, 1, 128, 30));
        assert_eq!(payload_work(), (0, 2, 0, 90));
        assert_eq!(Arc::strong_count(&r.frequencies[0]), 1);
        assert_eq!(wire(&pinned).1, 48);
        assert_eq!(NETWORK_EXPANSIONS.with(std::cell::Cell::get), 0);
        assert_eq!(wire(&view).1, 2098);
        assert_eq!(NETWORK_EXPANSIONS.with(std::cell::Cell::get), 4096);
        assert_eq!((ready_clones(), materializations()), (0, 0));
        assert_eq!(payload_work(), (0, 2, 0, 90));
    }

    fn go_reports() -> serde_json::Value {
        use std::{
            fs,
            io::Read,
            process::{Child, Command, Stdio},
            time::{Duration, Instant, SystemTime, UNIX_EPOCH},
        };
        struct Root(std::path::PathBuf);
        impl Drop for Root {
            fn drop(&mut self) {
                let _ = fs::remove_dir_all(&self.0);
            }
        }
        struct OwnedChild(Child);
        impl Drop for OwnedChild {
            fn drop(&mut self) {
                let _ = self.0.kill();
                let _ = self.0.wait();
            }
        }
        let root = Root(std::env::temp_dir().join(format!(
            "mornlea-network-go-{}-{}",
            std::process::id(),
            SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos()
        )));
        fs::create_dir(&root.0).unwrap();
        let source = root.0.join("main.go");
        let program = r#"package main
import (
 "encoding/json"
 "os"
 "github.com/channing771/mornlea/packages/shared/core"
 "github.com/channing771/mornlea/packages/shared/network"
 "github.com/channing771/mornlea/packages/shared/world"
 "github.com/channing771/mornlea/packages/server/server"
)
type report struct {
 Snapshot network.ChunkSnapshot
 PayloadBytes int
 DiskPayloadBytes int
}
func main() {
 c := world.NewChunk(core.ChunkPos{X:-2, Z:3})
 revision := uint64(9)
 reports := []report{}
 record := func() {
  snapshot, err := server.BuildChunkSnapshot(core.Depths, c, revision)
  if err != nil { panic(err) }
  for i := range snapshot.Sections {
   s := &snapshot.Sections[i]
   if s.Palette == nil { s.Palette = []core.BlockID{} }
   if s.Packed == nil { s.Packed = []uint64{} }
  }
  reports = append(reports, report{snapshot, snapshot.PayloadBytes(), c.PayloadBytes()})
 }
 record()
 palette := make([]core.BlockID,90)
 for i := range palette { palette[i] = core.BlockID(i) }
 loaded, err := world.NewPalettedContainerFromSnapshot(world.ContainerSnapshot{
  Kind:world.StorageIndexed, Bits:8, Palette:palette, Packed:make([]uint64,512),
 })
 if err != nil { panic(err) }; c.Section(1).Blocks = loaded
 loaded, err = world.NewPalettedContainerFromSnapshot(world.ContainerSnapshot{
  Kind:world.StorageDirect, Bits:15, Packed:make([]uint64,1024),
 })
 if err != nil { panic(err) }; c.Section(2).Blocks = loaded
 record()
 b := c.Section(0).Blocks
 b.Set(0,0,0,1); b.Compact(); revision++; record()
 b.Set(1,0,0,17); b.Compact(); revision++; record()
 for i:=0;i<90;i++ { b.Set(i%16,i/256,(i%256)/16,core.BlockID(89-i)) }
 b.Compact(); revision++; record()
 for i:=0;i<4096;i++ { b.Set(i%16,i/256,(i%256)/16,1) }
 b.Compact(); revision++; record()
 c.Section(1).Blocks.Set(0,0,0,1); c.Section(1).Blocks.Compact(); revision++; record()
 c.Section(2).Blocks.Set(0,0,0,1); c.Section(2).Blocks.Compact(); revision++; record()
 if err := json.NewEncoder(os.Stdout).Encode(reports); err != nil { panic(err) }
}
"#;
        fs::write(&source, program).unwrap();
        let repository = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../..");
        let stdout = root.0.join("stdout");
        let stderr = root.0.join("stderr");
        println!(
            "actual command: go run {} (cwd {})\n{program}",
            source.display(),
            repository.display()
        );
        let mut child = OwnedChild(
            Command::new("go")
                .arg("run")
                .arg(&source)
                .current_dir(repository)
                .stdin(Stdio::null())
                .stdout(fs::File::create(&stdout).unwrap())
                .stderr(fs::File::create(&stderr).unwrap())
                .spawn()
                .expect("actual Go compiler is required"),
        );
        let deadline = Instant::now() + Duration::from_secs(120);
        let status = loop {
            assert!(
                fs::metadata(&stdout).unwrap().len() <= 1024 * 1024,
                "Go stdout exceeded its bound"
            );
            assert!(
                fs::metadata(&stderr).unwrap().len() <= 4096,
                "Go stderr exceeded its bound"
            );
            if let Some(status) = child.0.try_wait().unwrap() {
                break status;
            }
            assert!(Instant::now() < deadline, "actual Go helper timed out");
            std::thread::sleep(Duration::from_millis(10));
        };
        let bounded_text = |path: &std::path::Path, bound: u64| {
            let mut text = String::new();
            fs::File::open(path)
                .unwrap()
                .take(bound + 1)
                .read_to_string(&mut text)
                .unwrap();
            assert!(text.len() as u64 <= bound, "Go report exceeded its bound");
            text
        };
        let error = bounded_text(&stderr, 4096);
        assert!(status.success(), "Go helper failed: {error}");
        let output = bounded_text(&stdout, 1024 * 1024);
        println!("actual Go output: {output}");
        serde_json::from_str(&output).expect("actual Go helper returned invalid JSON")
    }

    fn report(packet: &WireSnapshot, bytes: usize) -> serde_json::Value {
        serde_json::json!({
            "Snapshot": {
                "Dimension": packet.dimension.get(),
                "Chunk": {"X":packet.chunk_x, "Z":packet.chunk_z},
                "Revision": packet.revision,
                "Sections": packet.sections.iter().map(|s| serde_json::json!({
                    "Y":s.y, "Storage":s.storage.wire(), "Single":s.single,
                    "Bits":s.bits, "Palette":s.palette, "Packed":s.packed,
                })).collect::<Vec<_>>(),
            },
            "PayloadBytes":bytes,
        })
    }

    #[test]
    fn actual_unchanged_go_build_snapshot_matches_every_captured_stage() {
        let actual = go_reports();
        let actual = actual
            .as_array()
            .expect("actual Go report must be a stage array");
        assert_eq!(actual.len(), 8);
        let air = ready(vec![single(0); 24]);
        let mut sections = vec![single(0); 24];
        sections[1] = indexed(8, (0..90).collect());
        sections[2] = direct();
        let mut r = ready(sections);
        let mut captures = vec![air.capture(None, None), r.capture(None, None)];
        let mut disk_estimates = vec![
            air.payload_estimate().unwrap(),
            r.payload_estimate().unwrap(),
        ];
        let mut record = |r: &ReadyChunk| {
            captures.push(r.capture(None, None));
            disk_estimates.push(r.payload_estimate().unwrap());
        };
        r.set_block(position(0), 1);
        commit(&mut r);
        record(&r);
        r.set_block(position(1), 17);
        commit(&mut r);
        record(&r);
        for i in 0..90 {
            r.set_block(position(i), (89 - i) as u16);
        }
        commit(&mut r);
        record(&r);
        for i in 0..4096 {
            r.set_block(position(i), 1);
        }
        commit(&mut r);
        record(&r);
        r.set_block(position(4096), 1);
        commit(&mut r);
        record(&r);
        r.set_block(position(8192), 1);
        commit(&mut r);
        record(&r);
        for (i, capture) in captures.iter().enumerate() {
            let (packet, bytes) = wire(capture);
            let mut expected = report(&packet, bytes);
            expected["DiskPayloadBytes"] = serde_json::json!(disk_estimates[i]);
            assert_eq!(expected, actual[i], "actual Go snapshot stage {i}");
        }
        let (stone, _) = wire(&captures[2]);
        assert_eq!(stone.sections[0].bits, 4);
        assert_eq!(stone.sections[0].palette, [1, 0]);
        let disk = captures[2].materialize();
        assert_eq!(
            (disk.chunk.sections[0].kind, disk.chunk.sections[0].bits),
            (StorageKind::Direct, 15)
        );
        println!(
            "first-cell stone: accepted disk Direct15; actual Go network Indexed4 palette [1, 0]"
        );
    }
}
