use mornlea_engine::native::contracts::KernelError;
use mornlea_engine::native::contracts::fluid::{
    FluidChange, FluidEvalOp, FluidWrites, NeighborSlot,
};
use mornlea_engine::native::fluid_eval::NativeFluidEval;

/// Stable block ids of the fluid rule table, matching `internal/core/block.go`.
const AIR: u16 = 0;
const STONE: u16 = 2;
const WATER_SOURCE: u16 = 27;

/// Largest batch the native lane accepts. The legacy ABI admits more; that is
/// adapter-layer compatibility and not a native bound.
const MAX_BATCH_ITEMS: usize = 4096;

/// Destination canary: the inert default record publishes no changes, while
/// every case below that asserts a canary also produces at least one real
/// write, so an untouched element is always distinguishable from real output.
fn canary() -> FluidWrites {
    FluidWrites::default()
}

fn change(slot: NeighborSlot, block: u16) -> FluidChange {
    FluidChange { slot, block }
}

/// Cells in slot order: self, above, below, +x, -x, +z, -z.
fn cells(
    self_id: u16,
    above: u16,
    below: u16,
    pos_x: u16,
    neg_x: u16,
    pos_z: u16,
    neg_z: u16,
) -> [u16; 7] {
    [self_id, above, below, pos_x, neg_x, pos_z, neg_z]
}

/// A source cell whose only open neighbour is straight down.
fn source_drops_down() -> [u16; 7] {
    cells(WATER_SOURCE, STONE, AIR, STONE, STONE, STONE, STONE)
}

/// A source cell whose down direction is sealed, so it spreads sideways.
fn source_spreads() -> [u16; 7] {
    cells(WATER_SOURCE, STONE, STONE, AIR, AIR, AIR, AIR)
}

#[test]
fn fluid_priority_and_count() {
    let op = NativeFluidEval;

    // A replaceable cell below wins outright: the single write goes down and
    // no horizontal neighbour is touched.
    let mut dst = [canary(); 1];
    let res = op.evaluate(&[source_drops_down()], &mut dst);
    assert_eq!(res, Ok(1));
    assert_eq!(
        dst[0].changes(),
        [change(NeighborSlot::Below, WATER_SOURCE + 1)]
    );

    // With the cell below sealed, a surviving flowing cell spreads sideways.
    // The +x neighbour holds the supporting source and is not replaceable, so
    // exactly the remaining three horizontals are written in slot order.
    let mut dst = [canary(); 1];
    let res = op.evaluate(
        &[cells(
            WATER_SOURCE + 3,
            STONE,
            STONE,
            WATER_SOURCE,
            AIR,
            AIR,
            AIR,
        )],
        &mut dst,
    );
    assert_eq!(res, Ok(1));
    assert_eq!(
        dst[0].changes(),
        [
            change(NeighborSlot::NegX, WATER_SOURCE + 4),
            change(NeighborSlot::PosZ, WATER_SOURCE + 4),
            change(NeighborSlot::NegZ, WATER_SOURCE + 4),
        ]
    );
}

#[test]
fn four_write_slots_are_contiguous() {
    let op = NativeFluidEval;

    let mut dst = [canary(); 1];
    let res = op.evaluate(&[source_spreads()], &mut dst);
    assert_eq!(res, Ok(1));
    let writes = dst[0].changes();
    assert_eq!(writes.len(), 4);
    assert_eq!(
        writes,
        [
            change(NeighborSlot::PosX, WATER_SOURCE + 1),
            change(NeighborSlot::NegX, WATER_SOURCE + 1),
            change(NeighborSlot::PosZ, WATER_SOURCE + 1),
            change(NeighborSlot::NegZ, WATER_SOURCE + 1),
        ]
    );
}

#[test]
fn unused_entries_are_not_published() {
    let op = NativeFluidEval;

    // A downward write fills exactly one entry. The three unused wire slots
    // carry the no-write sentinel and must never surface as padding changes.
    let mut dst = [canary(); 1];
    let res = op.evaluate(&[source_drops_down()], &mut dst);
    assert_eq!(res, Ok(1));
    let writes = dst[0].changes();
    assert_eq!(writes.len(), 1);
    assert_eq!(writes[0], change(NeighborSlot::Below, WATER_SOURCE + 1));
    assert!(
        !writes
            .iter()
            .any(|entry| entry.slot == NeighborSlot::SelfCell && entry.block == 0),
        "padding entries must not be published"
    );
}

#[test]
fn unknown_id_is_nonfluid() {
    let op = NativeFluidEval;

    // A stale queue item whose self cell is not a fluid and not air is
    // accepted and produces no write at all.
    let item = cells(65535, STONE, STONE, STONE, STONE, STONE, STONE);
    let mut dst = [canary(); 1];
    let res = op.evaluate(&[item], &mut dst);
    assert_eq!(res, Ok(1));
    assert_eq!(dst[0].changes(), []);
}

#[test]
fn count_bounds() {
    let op = NativeFluidEval;

    // An empty batch succeeds and touches nothing.
    let mut dst = [canary(); 4];
    let before = dst;
    let res = op.evaluate(&[], &mut dst);
    assert_eq!(res, Ok(0));
    assert_eq!(dst, before);

    // One item and the full native batch both succeed.
    let mut one = [canary(); 1];
    assert_eq!(op.evaluate(&[source_drops_down()], &mut one), Ok(1));
    let full = vec![source_drops_down(); MAX_BATCH_ITEMS];
    let mut staged = vec![canary(); MAX_BATCH_ITEMS];
    assert_eq!(op.evaluate(&full, &mut staged), Ok(MAX_BATCH_ITEMS));
    assert!(
        !staged.contains(&canary()),
        "all destination slots must be written"
    );

    // The count bound is checked before capacity: an oversized batch is
    // InvalidInput even when the destination is also too small.
    let over = vec![source_drops_down(); MAX_BATCH_ITEMS + 1];
    let mut untouched = vec![canary(); MAX_BATCH_ITEMS + 1];
    let before = untouched.clone();
    assert_eq!(
        op.evaluate(&over, &mut untouched),
        Err(KernelError::InvalidInput)
    );
    assert_eq!(untouched, before);

    let mut short = vec![canary(); 4];
    let before = short.clone();
    assert_eq!(
        op.evaluate(&over, &mut short),
        Err(KernelError::InvalidInput)
    );
    assert_eq!(short, before);
}

#[test]
fn capacity_and_canary() {
    let op = NativeFluidEval;
    let full = vec![source_drops_down(); MAX_BATCH_ITEMS];

    // An exactly sized destination succeeds and publishes every element.
    let mut dst = vec![canary(); MAX_BATCH_ITEMS];
    assert_eq!(op.evaluate(&full, &mut dst), Ok(MAX_BATCH_ITEMS));
    assert!(!dst.contains(&canary()));

    // A one-slot-short destination fails before any evaluation and leaves
    // every canary element unchanged.
    let mut short = vec![canary(); MAX_BATCH_ITEMS - 1];
    let before = short.clone();
    assert_eq!(
        op.evaluate(&full, &mut short),
        Err(KernelError::OutputTooSmall {
            needed: MAX_BATCH_ITEMS,
            available: MAX_BATCH_ITEMS - 1,
        })
    );
    assert_eq!(short, before);

    // A surplus destination publishes only the used prefix.
    let two = [source_drops_down(), source_spreads()];
    let mut padded = [canary(); 4];
    assert_eq!(op.evaluate(&two, &mut padded), Ok(2));
    assert_ne!(padded[0], canary());
    assert_ne!(padded[1], canary());
    assert_eq!(padded[2], canary());
    assert_eq!(padded[3], canary());
}

#[test]
fn existing_rules_hold() {
    let op = NativeFluidEval;

    // Non-source decay: an unsupported flowing cell writes air to itself and
    // stops, even though the cell below is open.
    let decay = cells(WATER_SOURCE + 2, STONE, AIR, STONE, STONE, STONE, STONE);
    // Flowing survival: a stronger horizontal source keeps this cell alive, so
    // it spreads to the three open horizontals one level weaker.
    let survival = cells(WATER_SOURCE + 3, STONE, STONE, WATER_SOURCE, AIR, AIR, AIR);
    // Infinite source: an air cell between two horizontal sources upgrades
    // itself to a source instead of taking on a level.
    let infinite = cells(AIR, STONE, STONE, WATER_SOURCE, WATER_SOURCE, STONE, STONE);
    // Vertical over horizontal: an open cell below suppresses all sideways
    // spreading for the same tick.
    let priority = source_drops_down();

    let items = [decay, survival, infinite, priority];
    let mut dst = [canary(); 4];
    assert_eq!(op.evaluate(&items, &mut dst), Ok(4));

    assert_eq!(dst[0].changes(), [change(NeighborSlot::SelfCell, AIR)]);
    assert_eq!(
        dst[1].changes(),
        [
            change(NeighborSlot::NegX, WATER_SOURCE + 4),
            change(NeighborSlot::PosZ, WATER_SOURCE + 4),
            change(NeighborSlot::NegZ, WATER_SOURCE + 4),
        ]
    );
    assert_eq!(
        dst[2].changes(),
        [change(NeighborSlot::SelfCell, WATER_SOURCE)]
    );
    assert_eq!(
        dst[3].changes(),
        [change(NeighborSlot::Below, WATER_SOURCE + 1)]
    );
}
