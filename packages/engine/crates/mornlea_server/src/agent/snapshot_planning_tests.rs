//! The authority-projected planning snapshot hashes to the Go digests.
//!
//! Fixtures come from the Go `buildPlanSnapshot` generator; the Rust
//! projection over the same inputs must produce byte-identical canonical
//! terrain and snapshot digests.
use super::*;
use crate::core::state::companion_planning::tests::{FIXTURES, planning_fixture};

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[test]
fn projected_snapshot_digests_match_go_fixtures() {
    for (name, text) in FIXTURES {
        let fixture = planning_fixture(text);
        let snapshot = fixture
            .authority
            .companion_planning_snapshot(fixture.companion)
            .unwrap_or_else(|error| panic!("{name}: projection failed: {error:?}"));
        let terrain = canonical_terrain_digest(&snapshot.terrain).unwrap();
        assert_eq!(
            hex(&sha256(&terrain)),
            fixture.expected["terrainSha256"].as_str().unwrap(),
            "{name}: terrain digest"
        );
        let (_, digest) = canonical_snapshot_digest(&snapshot).unwrap();
        assert_eq!(
            hex(&digest),
            fixture.expected["snapshotSha256"].as_str().unwrap(),
            "{name}: snapshot digest"
        );
    }
}
