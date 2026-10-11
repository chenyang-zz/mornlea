//! The checked drop-location geometry contract tests.
//!
//! These pin the private C2 helper's accepted source contract: ordinary
//! first/last block values match the packet examples, the 98304 index bound
//! rejects, and the extreme i32 chunk range stays exactly representable in
//! f64 instead of reproducing Go's narrowing wrap.

use mornlea_client_core::contracts::ClientError;
use mornlea_client_core::drop_position;
use mornlea_domain::{ChunkPos, DropId};

fn drop_id(x: i32, z: i32) -> DropId {
    DropId::try_new(7, ChunkPos::new(x, z), 0, 1).expect("checked drop identity")
}

/// `geometry::first_last_block`: the ordinary i32-world-coordinate cases
/// compare against the accepted source contract — chunk(-1,2) index 0 is
/// [-16,-64,32] and index 98303 is [-1,319,47].
#[test]
fn first_last_block() {
    let first = drop_position(drop_id(-1, 2), 0).expect("index 0 is valid");
    assert_eq!(first, [-16.0, -64.0, 32.0]);

    let last = drop_position(drop_id(-1, 2), 98303).expect("index 98303 is valid");
    assert_eq!(last, [-1.0, 319.0, 47.0]);

    // A mid-chunk value cross-checks the decomposition order: section 5,
    // local y 7, z 3, x 11.
    let index = 5 * 4096 + 7 * 256 + 3 * 16 + 11;
    let mid = drop_position(drop_id(3, -4), index).expect("mid index is valid");
    assert_eq!(mid, [59.0, 23.0, -61.0]);
}

/// `geometry::invalid_index`: the index at the 98304 bound is an invalid
/// input and rejects before any value is produced.
#[test]
fn invalid_index() {
    assert_eq!(
        drop_position(drop_id(0, 0), 98304),
        Err(ClientError::InvalidInput)
    );
    assert_eq!(
        drop_position(drop_id(0, 0), u32::MAX),
        Err(ClientError::InvalidInput)
    );
}

/// `geometry::checked_extreme_chunk`: the i32::MAX chunk produces the exact
/// finite coordinate 34359738352 through checked i64 arithmetic, preserving
/// the accepted actor identity; the negative extreme behaves symmetrically.
#[test]
fn checked_extreme_chunk() {
    let extreme = drop_position(drop_id(i32::MAX, 0), 0).expect("extreme chunk is admitted");
    assert_eq!(extreme[0], 34359738352.0, "exact i64 world x, no narrowing");
    assert_eq!(extreme[1], -64.0);
    assert_eq!(extreme[2], 0.0);

    let negative = drop_position(drop_id(i32::MIN, i32::MIN), 98303)
        .expect("negative extreme chunk is admitted");
    assert_eq!(
        negative[0], -34359738353.0,
        "the negative extreme stays exact in f64"
    );
    assert_eq!(negative[2], -34359738353.0);
    assert_eq!(negative[1], 319.0);

    // The raw drop dimension is preserved by identity, never normalized.
    let id = DropId::try_new(-190, ChunkPos::new(i32::MAX, 0), 0, 1).expect("raw dimension");
    let position = drop_position(id, 0).expect("position resolves");
    assert_eq!(position[0], 34359738352.0);
}
