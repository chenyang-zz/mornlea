//! The checked drop-location geometry helper owned by the terrain/actor
//! presentation seam.
//!
//! The accepted domain has no inverse chunk-index helper, so this module
//! lands it as the checked C2 helper the actor workers read. It computes the
//! exact finite world position of one chunk-local block index with checked
//! i64 arithmetic: the full admitted i32 chunk range times 16 stays exactly
//! representable in f64, so world coordinates are never narrowed to i32 and
//! no protocol-valid identity is rejected or wrapped.

use mornlea_domain::DropId;

use crate::contracts::ClientError;
use mornlea_protocol::MAX_CHUNK_BLOCK_INDEX;

/// Resolves one drop's chunk-local block index into the exact finite world
/// position, preserving the drop's raw dimension instead of normalizing it
/// into any known world.
///
/// The decomposition is the accepted source contract: section = index / 4096,
/// local = index % 4096, x = local & 15, z = (local / 16) & 15 and
/// y = -64 + section * 16 + local / 256. World x and z are computed in i64 as
/// `chunk * 16 + local` and converted directly to f64. An index at or above
/// the chunk cell count (24 sections of 4096 blocks, so the 98304 bound the
/// protocol publishes as `MAX_CHUNK_BLOCK_INDEX`) is an invalid input and
/// rejects before any value is produced.
pub fn drop_position(id: DropId, index: u32) -> Result<[f64; 3], ClientError> {
    if index >= MAX_CHUNK_BLOCK_INDEX {
        return Err(ClientError::InvalidInput);
    }
    let section = index / 4096;
    let local = index % 4096;
    let local_x = (local & 15) as i64;
    let local_z = ((local / 16) & 15) as i64;
    let local_y = (local / 256) as i64;
    let world_x = i64::from(id.chunk().x()) * 16 + local_x;
    let world_z = i64::from(id.chunk().z()) * 16 + local_z;
    let world_y = i64::from(mornlea_protocol::MIN_Y) + i64::from(section) * 16 + local_y;
    // The full i32 chunk range times 16 is exactly representable in f64, so
    // the conversions below are exact for every admitted world coordinate.
    Ok([world_x as f64, world_y as f64, world_z as f64])
}
