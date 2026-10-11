//! Pure source-compatible harvest dice. Callers own world validation and atomic
//! output settlement; scalar dimensions here are hash input, not admission.

use mornlea_domain::BlockPos;

fn mix(mut value: u64) -> u64 {
    value = value.wrapping_add(0x9e37_79b9_7f4a_7c15);
    value = (value ^ (value >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    value = (value ^ (value >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    value ^ (value >> 31)
}

fn position_hash(mut value: u64, dimension: u32, pos: BlockPos) -> u64 {
    // Go zero-extends the signed coordinate's uint32 representation.
    for component in [dimension, pos.x() as u32, pos.y() as u32, pos.z() as u32] {
        value = mix(value ^ u64::from(component));
    }
    value
}

fn crop_hash(seed: i64, salt: u64, tick: u64, dimension: u32, pos: BlockPos) -> u64 {
    position_hash(mix(mix(seed as u64 ^ salt) ^ tick), dimension, pos)
}

/// Completion-tick wheat and seed counts, in the source's output order.
pub fn wheat(seed: i64, tick: u64, dimension: u32, pos: BlockPos) -> (u8, u8) {
    let hash = crop_hash(seed, 0x5eed_feed_face_face, tick, dimension, pos);
    ((hash % 3) as u8 + 1, (mix(hash) % 3) as u8 + 1)
}

/// Human mature-potato yield; flooding and trampling retain their own policy.
pub fn potato(seed: i64, tick: u64, dimension: u32, pos: BlockPos) -> u8 {
    (crop_hash(seed, 0x70a7_0a51_5eed_face, tick, dimension, pos) % 4) as u8 + 1
}

/// Human mature-carrot yield from its independent source stream.
pub fn carrot(seed: i64, tick: u64, dimension: u32, pos: BlockPos) -> u8 {
    (crop_hash(seed, 0xca77_0770_1ace_5eed, tick, dimension, pos) % 4) as u8 + 1
}

/// The optional second potato stack uses an independent completion-tick roll.
pub fn poison_potato(seed: i64, tick: u64, dimension: u32, pos: BlockPos) -> bool {
    crop_hash(seed, 0xdead_beef_cafe_1234, tick, dimension, pos).is_multiple_of(50)
}

/// Position-stable seed decision: a capacity retry cannot reroll by tick.
pub fn short_grass(seed: i64, dimension: u32, pos: BlockPos) -> bool {
    position_hash(mix(seed as u64 ^ 0x4752_4153_5353_4544), dimension, pos) & 7 == 0
}

/// Position-stable extra sapling for human leaf mining; companions do not roll.
pub fn leaf_sapling(seed: i64, dimension: u32, pos: BlockPos) -> bool {
    position_hash(mix(seed as u64 ^ 0x5341_504c_494e_4753), dimension, pos) & 7 == 0
}
