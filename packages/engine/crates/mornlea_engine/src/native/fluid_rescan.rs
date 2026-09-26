//! Typed native provider for halo-safe bounded fluid rescan.
//!
//! The provider reproduces the legacy rescan accounting exactly through the
//! shared scan seam in `crate::fluid_rescan`, but reads through a typed
//! accessor that answers `KernelError::MissingHalo` for a coordinate the view
//! does not own instead of touching a slice outside its bounds.

use crate::fluid_eval::BARRIER;
use crate::fluid_rescan::{
    RescanSource, box_to_world, rescan_scan, section_cell_index, skirt_column,
};
use crate::native::contracts::KernelError;
use crate::native::contracts::fluid::{
    FluidRescanOp, RescanRequest, RescanScratch, RescanSection, RescanSummary, RescanView,
};

/// Sections per chunk and section edge, matching the typed view's shape.
const SECTIONS: usize = 24;
const SECTION_EDGE: usize = 16;
/// Box columns per axis: sixteen center columns plus a one-block halo.
const BOX_COLUMNS: i32 = 18;
/// Full world height in blocks.
const WORLD_HEIGHT: i32 = 384;

/// Typed read accessor over one `RescanRequest` view.
///
/// Ownership: the accessor copies the request's `RescanView`, which is a
/// borrowed snapshot of caller-owned section, skirt and metadata data; it owns
/// no cells of its own. Box coordinates resolve to the center chunk's section
/// records, the skirt halo column, or, for a y outside the world column, the
/// barrier rule `fluidRescanBlockAt` defines. A horizontal coordinate outside
/// the owned halo is reported as `MissingHalo` on an actual read only: the
/// accessor never reads outside the backing slices and never substitutes a
/// barrier for data the view does not carry.
struct RescanAccess<'a>(RescanView<'a>);

impl RescanAccess<'_> {
    /// Resolves one box cell to an owned value, or `None` when the coordinate
    /// lies outside the owned box halo.
    fn read(&self, bx: i32, y: i32, bz: i32) -> Option<u16> {
        if !(0..BOX_COLUMNS).contains(&bx) || !(0..BOX_COLUMNS).contains(&bz) {
            return None;
        }
        if !(0..WORLD_HEIGHT).contains(&y) {
            return Some(BARRIER);
        }
        if (1..=SECTION_EDGE as i32).contains(&bx) && (1..=SECTION_EDGE as i32).contains(&bz) {
            let section = y as usize / SECTION_EDGE;
            return Some(match self.0.sections[section] {
                RescanSection::Uniform(id) => id,
                RescanSection::Dense(cells) => cells[section_cell_index(bx, y, bz)],
            });
        }
        Some(self.0.skirt[skirt_column(bx, bz)][y as usize])
    }
}

impl RescanSource for RescanAccess<'_> {
    fn cell(&self, bx: i32, y: i32, bz: i32) -> Result<u16, KernelError> {
        self.read(bx, y, bz).ok_or(KernelError::MissingHalo)
    }

    /// Center section records answer the uniform id; the metadata table never
    /// overrides them, which keeps center-section precedence over metadata.
    fn section_uniform(&self, section: usize) -> Option<u16> {
        match self.0.sections.get(section)? {
            RescanSection::Uniform(id) => Some(*id),
            RescanSection::Dense(_) => None,
        }
    }

    fn meta_uniform(&self, dx: i32, dz: i32, section: usize) -> Option<u16> {
        if !(-1..=1).contains(&dx) || !(-1..=1).contains(&dz) {
            return None;
        }
        let chunk = crate::fluid_rescan::chunk_meta_index(dx, dz);
        self.0
            .metadata
            .get(chunk * SECTIONS + section)
            .copied()
            .flatten()
    }

    fn position(&self, bx: i32, y: i32, bz: i32) -> [i32; 3] {
        box_to_world(self.0.center(), bx, y, bz)
    }
}

/// Zero-sized native provider for halo-safe bounded fluid rescan.
///
/// Ownership: the provider is stateless. The caller owns the request view, the
/// scratch stage and the destination; positions accumulate into the scratch
/// stage and are published to the destination once, after the scan.
pub struct NativeFluidRescan;

impl FluidRescanOp for NativeFluidRescan {
    /// Runs one bounded rescan and publishes the emitted world positions.
    ///
    /// Admission runs before any scan and in a fixed order: the closed box
    /// range first, then the start section, then scratch capacity. The range
    /// gate pins the native lane to the interior columns `1..=16`, so a halo
    /// read is a seal-check neighbour and never a scanned cell; the scratch
    /// capacity gate preflights the worst-case position count
    /// `area * 16 * (24 - start_section)` because a scan must never discover
    /// mid-flight that its staging buffer is too small. A rejected call
    /// returns before the loop and leaves `dst` untouched.
    ///
    /// The scan writes only into `scratch.positions`, reusing its capacity
    /// without reallocating. Publication happens once at the end: if the
    /// destination is too short the call returns `OutputTooSmall` with `dst`
    /// untouched, so a caller can inspect the buffer or retry with a larger
    /// one without unwinding partial results. A zero budget is admitted and
    /// reports `spent 0`, `done false` and the unchanged resume section.
    fn rescan(
        &self,
        request: &RescanRequest<'_>,
        scratch: &mut RescanScratch,
        dst: &mut [[i32; 3]],
    ) -> Result<RescanSummary, KernelError> {
        let range = request.range;
        if range.x0 < 1
            || range.x0 > range.x1
            || range.x1 > SECTION_EDGE as u8
            || range.z0 < 1
            || range.z0 > range.z1
            || range.z1 > SECTION_EDGE as u8
        {
            return Err(KernelError::InvalidInput);
        }
        if request.start_section >= SECTIONS as u8 {
            return Err(KernelError::InvalidInput);
        }
        let area = usize::from(range.x1 - range.x0 + 1) * usize::from(range.z1 - range.z0 + 1);
        let needed = area * SECTION_EDGE * (SECTIONS - usize::from(request.start_section));
        if scratch.capacity < needed {
            return Err(KernelError::ScratchTooSmall {
                needed,
                available: scratch.capacity,
            });
        }
        scratch.positions.clear();
        let access = RescanAccess(request.view);
        let (spent, done, next_section) = rescan_scan(
            &access,
            (
                usize::from(range.x0),
                usize::from(range.x1),
                usize::from(range.z0),
                usize::from(range.z1),
            ),
            usize::from(request.start_section),
            request.budget,
            &mut |position| scratch.positions.push(position),
        )?;
        let used = scratch.positions.len();
        if dst.len() < used {
            return Err(KernelError::OutputTooSmall {
                needed: used,
                available: dst.len(),
            });
        }
        dst[..used].copy_from_slice(&scratch.positions[..used]);
        Ok(RescanSummary {
            written: used,
            spent: spent as u32,
            done,
            next_section: next_section as u8,
        })
    }
}
