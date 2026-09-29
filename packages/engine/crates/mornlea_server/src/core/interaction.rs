//! World-aware target classification shared by authoritative interaction rays.
use super::state::AuthorityReadView;
use mornlea_domain::{BlockPos, Dimension};

/// Classify an already observed cell. The caller owns observation/readiness
/// errors; a target is not proof of collision, support or harvestability.
pub fn target_block(
    view: &AuthorityReadView<'_>,
    dimension: Dimension,
    pos: BlockPos,
    block: u16,
) -> bool {
    if block == 0 || (27..=34).contains(&block) {
        return false;
    }
    if block == 70 {
        let lower = view.block(
            dimension,
            BlockPos::new(pos.x(), pos.y().saturating_sub(1), pos.z()),
        );
        return !matches!(lower, Some(id) if (62..=69).contains(&id) && !id.is_multiple_of(2));
    }
    !(62..=69).contains(&block) || block.is_multiple_of(2)
}
