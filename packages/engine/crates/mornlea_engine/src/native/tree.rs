use crate::native::contracts::KernelError;
use crate::native::contracts::world::{TreeBlock, TreeBlocks, TreeOp, TreeRequest};
use crate::worldgen::{self, TreeBlocksRequest};

/// Capacity of the fixed local stage. The shared visitor checks this bound
/// before every emission, so the stage is never indexed past its last slot.
const STAGE_RECORDS: usize = 128;

/// Zero-sized native provider for bounded runtime tree geometry.
///
/// Ownership: the provider is stateless and holds no scratch. Ordinary-oak
/// geometry stays owned by the shared world-generation visitor; every call
/// stages records in a fixed local array without allocation.
pub struct NativeTree;

impl TreeOp for NativeTree {
    /// Evaluates the runtime tree at the requested root into an owned record
    /// block.
    ///
    /// Admission mirrors the shared tree request gate before any geometry is
    /// read: the X/Z +-2 neighborhood must be representable because geometry
    /// reaches it and unchecked addition would wrap, and the root Y must keep
    /// the worst-case tree inside the world (`-64..=311`). Every seed is
    /// accepted. A rejected request returns `InvalidInput` and produces no
    /// records.
    ///
    /// The shared visitor owns all tree geometry and enforces the 128-record
    /// bound before each emission, so the fixed local stage needs no second
    /// bounds check. A refused overflow means a 129th record would have been
    /// emitted: that violates the bounded tree contract instead of yielding a
    /// short output, so the call fails with `OutputInvariant` and never
    /// publishes a truncated tree.
    fn tree_blocks(&self, request: &TreeRequest) -> Result<TreeBlocks, KernelError> {
        let root = request.root;
        let neighborhood_fits = root[0].checked_add(2).is_some()
            && root[0].checked_sub(2).is_some()
            && root[2].checked_add(2).is_some()
            && root[2].checked_sub(2).is_some();
        if !neighborhood_fits || !(-64..=311).contains(&root[1]) {
            return Err(KernelError::InvalidInput);
        }
        let blocks_request = TreeBlocksRequest {
            seed: request.seed,
            x: root[0],
            y: root[1],
            z: root[2],
        };
        let mut records = [TreeBlock {
            offset: [0; 3],
            block: 0,
        }; STAGE_RECORDS];
        let mut len = 0usize;
        let count = worldgen::visit_tree_blocks(&blocks_request, |item| {
            records[len] = TreeBlock {
                offset: [item.dx, item.dy, item.dz],
                block: item.block,
            };
            len += 1;
            true
        });
        match count {
            Some(count) if count == len => TreeBlocks::from_parts(records, len),
            _ => Err(KernelError::OutputInvariant),
        }
    }
}
