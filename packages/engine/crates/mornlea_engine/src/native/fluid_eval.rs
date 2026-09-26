use crate::fluid_eval::{EVAL_ITEM_OUTPUT_BYTES, EVAL_SLOTS_PER_ITEM, SLOT_NO_WRITE, eval_one};
use crate::native::contracts::KernelError;
use crate::native::contracts::fluid::{FluidChange, FluidEvalOp, FluidWrites, NeighborSlot};

/// Largest item count the native lane accepts in one call. The legacy ABI
/// admits more; that compatibility path belongs to the ABI adapter and must
/// not weaken the bounded native contract.
const MAX_BATCH_ITEMS: usize = 4096;

/// Zero-sized native provider for bounded fluid rule evaluation.
///
/// Ownership: the provider is stateless and owns no scratch. Each call decodes
/// one item at a time into a caller-owned `FluidWrites`, so a warm caller
/// reuses its destination across every batch and the lane performs no
/// allocation at all.
pub struct NativeFluidEval;

/// Decodes one packed output item into the typed change list.
///
/// The four wire entries carry a slot byte of `SLOT_NO_WRITE` when unused; the
/// sentinel is skipped rather than published, because padding is an artifact
/// of the fixed-width encoding and not a write the caller asked for. Used
/// entries are already packed from index 0 by `eval_one`, so they surface in
/// the same order the rules produced them.
///
/// The decode is total: `eval_one` emits only neighbour slots `0..=6` or the
/// sentinel, so a different byte cannot be produced by the rule table. The
/// match stays exhaustive and the unreachable case falls back to a defined
/// slot with a debug-only assertion, which keeps the post-preflight loop free
/// of any failure path.
fn decode_writes(raw: &[u8; EVAL_ITEM_OUTPUT_BYTES]) -> FluidWrites {
    let mut writes = FluidWrites::default();
    for entry in raw.chunks_exact(3) {
        if entry[0] == SLOT_NO_WRITE {
            continue;
        }
        let slot = match entry[0] {
            0 => NeighborSlot::SelfCell,
            1 => NeighborSlot::Above,
            2 => NeighborSlot::Below,
            3 => NeighborSlot::PosX,
            4 => NeighborSlot::NegX,
            5 => NeighborSlot::PosZ,
            6 => NeighborSlot::NegZ,
            other => {
                debug_assert!(other <= 6, "eval_one emitted a slot byte outside 0..=6");
                NeighborSlot::SelfCell
            }
        };
        let block = u16::from_le_bytes([entry[1], entry[2]]);
        writes.changes[writes.len] = FluidChange { slot, block };
        writes.len += 1;
    }
    writes
}

impl FluidEvalOp for NativeFluidEval {
    /// Evaluates a batch of neighbourhoods into one typed change list each.
    ///
    /// Both admission checks run before any evaluation and in a fixed order:
    /// the count bound first, then destination capacity. The count bound is a
    /// property of the lane and must reject an oversized batch even when the
    /// destination is also too short, while the capacity check needs the count
    /// to report the exact `needed` value. A rejected call returns before the
    /// loop and leaves `dst` untouched, so a caller can inspect its buffer or
    /// retry with a larger one without unwinding partial results.
    ///
    /// After preflight every step is infallible: `eval_one` writes a fixed
    /// stack-local buffer, `decode_writes` cannot fail, and the destination is
    /// known to be long enough. The call therefore performs no allocation and
    /// has no error path between the first and the last item. An empty batch
    /// satisfies both checks and returns `Ok(0)` without touching `dst`.
    fn evaluate(
        &self,
        items: &[[u16; EVAL_SLOTS_PER_ITEM]],
        dst: &mut [FluidWrites],
    ) -> Result<usize, KernelError> {
        if items.len() > MAX_BATCH_ITEMS {
            return Err(KernelError::InvalidInput);
        }
        if dst.len() < items.len() {
            return Err(KernelError::OutputTooSmall {
                needed: items.len(),
                available: dst.len(),
            });
        }
        for (index, item) in items.iter().enumerate() {
            let mut raw = [0_u8; EVAL_ITEM_OUTPUT_BYTES];
            eval_one(item, &mut raw);
            dst[index] = decode_writes(&raw);
        }
        Ok(items.len())
    }
}
