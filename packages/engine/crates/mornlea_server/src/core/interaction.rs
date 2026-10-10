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

/// Source look components retain the float32 casts between trigonometry and
/// multiplication. Callers supply checked angles; this value grants no target.
pub fn look_direction(yaw: f32, pitch: f32) -> [f32; 3] {
    let cos_pitch = f64::from(pitch).cos() as f32;
    [
        -(f64::from(yaw).sin() as f32) * cos_pitch,
        f64::from(pitch).sin() as f32,
        -(f64::from(yaw).cos() as f32) * cos_pitch,
    ]
}

/// Normalize a ray without overflowing a float32 squared sum. Invalid or tiny
/// input refuses before a caller starts observing authoritative cells.
pub fn normalized_direction(direction: [f32; 3]) -> Option<[f32; 3]> {
    if direction.iter().any(|component| !component.is_finite()) {
        return None;
    }
    let length = f64::from(direction[0])
        .hypot(f64::from(direction[1]))
        .hypot(f64::from(direction[2]));
    if length < 1e-6 {
        return None;
    }
    let inverse = (1.0 / length) as f32;
    Some(direction.map(|component| component * inverse))
}
