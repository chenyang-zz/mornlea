//! Frozen planning snapshot registry: digest parity and lifecycle.
//!
//! Expected values mirror the frozen contract sources:
//! `packages/shared/companion/snapshot_digest.go` (canonical JSON, SHA-256,
//! byte bounds), `packages/shared/companion/snapshot_registry.go` (capacity 4,
//! deadline-checked registration, uniform unavailability, deep-copy freeze),
//! and `packages/shared/companion/terrain_projection.go` (dense data plane).
//! Float spellings follow Go `encoding/json` for `float32`: `0.1` stays
//! `0.1` and negative zero renders as `-0`, never `-0.0`.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use mornlea_domain::{
    BlockPos, ChunkPos, CommandText, CompanionId, FiniteVec3, LookAngles, PlayerId,
};
use mornlea_server::agent::http::parse_canonical_uuid;
use mornlea_server::agent::snapshot::{
    MAX_SNAPSHOT_DIGEST_BYTES, MAX_TERRAIN_DIGEST_BYTES, REGISTRY_CAPACITY, SNAPSHOT_EXPIRY_GRACE,
    SnapshotEntropy, SnapshotRegistry, canonical_snapshot_digest, canonical_terrain_digest,
};
use mornlea_server::contracts::{
    Clock, Deadline, NamespaceId, PlanningSnapshot, Resource, ServerError, SnapshotBlock,
    SnapshotChunkRevision, SnapshotCompanion, SnapshotId, SnapshotIssuer, SnapshotPlayer,
    SnapshotPort, SnapshotTaskStatusText, SnapshotTerrain,
};
use mornlea_storage::ItemStack;

/// Step clock the harness can advance across registry expiry boundaries.
struct StepClock {
    now: Mutex<Instant>,
}

impl StepClock {
    fn start() -> (Instant, Arc<Self>) {
        let now = Instant::now();
        (
            now,
            Arc::new(Self {
                now: Mutex::new(now),
            }),
        )
    }

    fn advance(&self, delta: Duration) {
        let mut now = self.now.lock().unwrap();
        *now = now.checked_add(delta).expect("clock advance");
    }
}

impl Clock for StepClock {
    fn monotonic(&self) -> Instant {
        *self.now.lock().unwrap()
    }

    fn unix_ms(&self) -> i64 {
        1_800_000_000_000
    }
}

/// Deterministic entropy mirroring the Go `snapshotTestEntropy` byte cycle.
struct CycleEntropy {
    next: Mutex<u8>,
    fail: Mutex<bool>,
}

impl CycleEntropy {
    fn next_byte(&self) -> Result<u8, ServerError> {
        if *self.fail.lock().unwrap() {
            return Err(ServerError::InvalidInput { field: "entropy" });
        }
        let mut next = self.next.lock().unwrap();
        let byte = *next;
        *next = next.wrapping_add(1);
        Ok(byte)
    }
}

impl SnapshotEntropy for CycleEntropy {
    fn fill(&self, out: &mut [u8; 32]) -> Result<(), ServerError> {
        for slot in out.iter_mut() {
            *slot = self.next_byte()?;
        }
        Ok(())
    }
}

fn entropy() -> Arc<CycleEntropy> {
    Arc::new(CycleEntropy {
        next: Mutex::new(0),
        fail: Mutex::new(false),
    })
}

/// Snapshot bearers must depend on operating-system entropy rather than
/// observable process state, even when the entropy provider fails.
#[test]
fn system_entropy_uses_os_rng_without_predictable_fallback() {
    let source = include_str!("../../src/agent/snapshot.rs");
    let production = source
        .split_once("pub struct SystemEntropy")
        .expect("production entropy provider")
        .1
        .split_once("struct Record")
        .expect("registry records follow entropy provider")
        .0;
    assert!(
        production.contains("getrandom::fill(out)"),
        "snapshot capabilities must use the operating-system RNG"
    );
    for predictable in [
        "SystemTime",
        "UNIX_EPOCH",
        "process::id",
        "as_ptr",
        "*const",
        "AtomicU64",
        "sha256(",
    ] {
        assert!(
            !production.contains(predictable),
            "predictable entropy fallback: {predictable}"
        );
    }
}

fn player_id(text: &str) -> PlayerId {
    PlayerId::try_from_bytes(parse_canonical_uuid(text).expect("fixture uuid"))
        .expect("fixture player id")
}

fn companion_id(text: &str) -> CompanionId {
    CompanionId::try_from_bytes(parse_canonical_uuid(text).expect("fixture uuid"))
        .expect("fixture companion id")
}

fn look(yaw: f32, pitch: f32) -> LookAngles {
    LookAngles::try_new(yaw, pitch).expect("fixture look")
}

fn position(values: [f32; 3]) -> FiniteVec3 {
    FiniteVec3::try_new(values).expect("fixture position")
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack
        .windows(needle.len())
        .any(|window| window == needle)
}

/// Dense terrain projection matching the Go `testSnapshot` data plane:
/// origin `(-10, 57, -16)`, every column ready at height 64, grass at
/// `(8, 63, -2)` and stone at `(6, 64, 0)`.
fn fixture_terrain() -> SnapshotTerrain {
    let mut ready = vec![0u8; 137];
    for byte in ready.iter_mut().take(136) {
        *byte = 0xff;
    }
    ready[136] = 0x01;
    let heights = vec![64i16; 1089];
    let mut blocks = vec![0u16; 18_513];
    // Voxel `(dx * 17 + dy) * 33 + dz` relative to the projection origin.
    blocks[(18 * 17 + 6) * 33 + 14] = 4;
    blocks[(16 * 17 + 7) * 33 + 16] = 2;
    SnapshotTerrain::try_new(
        BlockPos::new(-10, 57, -16),
        [33, 17, 33],
        ready,
        heights,
        blocks,
    )
    .expect("fixture terrain")
}

/// Planning snapshot matching the Go `testSnapshot` field for field, so the
/// golden digests below are cross-language byte equality checks.
fn fixture_snapshot(source_tick: u64) -> PlanningSnapshot {
    let mut inventory = [ItemStack {
        item: 0,
        count: 0,
        durability: 0,
    }; 36];
    inventory[0] = ItemStack {
        item: 21,
        count: 3,
        durability: 0,
    };
    PlanningSnapshot::try_new(
        source_tick,
        6000,
        CommandText::try_from_canonical("去那棵橡树旁边".to_owned()).unwrap(),
        SnapshotIssuer {
            player_id: player_id("0f2a3b4c-5d6e-4f7a-8b9c-0d1e2f3a4b5c"),
            position: position([8.5, 65.0, -1.5]),
            look: look(0.25, -0.1),
            look_hit: Some(BlockPos::new(9, 64, -1)),
        },
        SnapshotCompanion {
            companion_id: companion_id("1a2b3c4d-5e6f-4a7b-9c8d-0e1f2a3b4c5d"),
            position: position([6.5, 65.0, 0.5]),
            look: look(3.0, 0.0),
            task_status: SnapshotTaskStatusText::try_new("空闲".to_owned()).unwrap(),
            inventory,
        },
        vec![
            SnapshotPlayer {
                player_id: player_id("0f2a3b4c-5d6e-4f7a-8b9c-0d1e2f3a4b5c"),
                position: position([8.5, 65.0, -1.5]),
                look: look(0.25, -0.1),
                look_hit: Some(BlockPos::new(9, 64, -1)),
            },
            SnapshotPlayer {
                player_id: player_id("2b3c4d5e-6f70-4a81-9b2d-1f2a3b4c5d6e"),
                position: position([-3.5, 66.0, 12.25]),
                look: look(1.5, 0.0),
                look_hit: None,
            },
        ],
        vec![SnapshotChunkRevision {
            pos: ChunkPos::new(0, -1),
            revision: 7,
        }],
        vec![
            SnapshotBlock {
                position: BlockPos::new(8, 63, -2),
                block_id: 4,
            },
            SnapshotBlock {
                position: BlockPos::new(9, 63, -2),
                block_id: 2,
            },
            SnapshotBlock {
                position: BlockPos::new(9, 64, -1),
                block_id: 17,
            },
        ],
        fixture_terrain(),
    )
    .expect("fixture snapshot")
}

fn namespace() -> NamespaceId {
    NamespaceId::try_from_bytes(
        parse_canonical_uuid("4d5e6f70-8192-4aa3-8b4f-3a4b5c6d7e8f").unwrap(),
    )
    .unwrap()
}

fn test_registry(clock: &Arc<StepClock>, entropy: &Arc<CycleEntropy>) -> SnapshotRegistry {
    SnapshotRegistry::try_new(
        clock.clone(),
        entropy.clone(),
        "http://127.0.0.1:9/mcp".to_owned(),
    )
    .expect("registry")
}

#[test]
fn digest_matches_go_goldens() {
    let snapshot = fixture_snapshot(4242);
    let (canonical, digest) = canonical_snapshot_digest(&snapshot).expect("fixture digest");
    let hex: String = digest.iter().map(|byte| format!("{byte:02x}")).collect();
    assert_eq!(
        hex, "b1bcb780f32b59233983c3bc06fb662ffd42260cba9d6c7cf964c146fd7a8a50",
        "snapshot golden digest drift"
    );
    assert!(
        canonical.len() <= MAX_SNAPSHOT_DIGEST_BYTES,
        "snapshot canonical {} exceeds bound",
        canonical.len()
    );
    assert!(
        canonical.starts_with(b"{\"chunk_revisions\":"),
        "canonical keys not sorted: {:.80}",
        String::from_utf8_lossy(&canonical[..canonical.len().min(80)])
    );
    assert!(
        !contains(&canonical, b"\"heights\":"),
        "legacy heights field leaked into digest"
    );
    assert!(
        contains(&canonical, b"\"heights_be_i16_b64\":"),
        "dense height plane missing from digest"
    );
    let again = canonical_snapshot_digest(&snapshot).expect("redigest");
    assert_eq!(
        (canonical.clone(), digest),
        again,
        "digest not deterministic"
    );

    let terrain = canonical_terrain_digest(&snapshot.terrain).expect("terrain digest");
    assert!(
        terrain.len() < MAX_TERRAIN_DIGEST_BYTES,
        "terrain canonical {} reaches exclusive bound",
        terrain.len()
    );
    let terrain_digest = sha256_hex(&terrain);
    assert_eq!(
        terrain_digest, "2942dddd4743b1c783b153a3b4e2f5e7026584844bb741bff2a6135ca350f363",
        "terrain golden digest drift"
    );

    // The `0.1f32` pitch spelling is the Go-compatible float form, and a
    // terrain change alters the digest.
    assert!(
        contains(&canonical, b"\"pitch\":-0.1"),
        "missing Go float spelling"
    );
    let mut changed = fixture_snapshot(4242);
    changed_terrain_block(&mut changed, 3);
    let (_, changed_digest) = canonical_snapshot_digest(&changed).expect("changed digest");
    assert_ne!(digest, changed_digest, "terrain change ignored by digest");
}

#[test]
fn negative_zero_spelling_matches_go() {
    // Negative zero must render as `-0`, matching Go `encoding/json`; a
    // `-0.0` spelling would be serializer drift.
    let mut snapshot = fixture_snapshot(7);
    snapshot.companion.look = look(-0.0, 0.0);
    let (canonical, digest) = canonical_snapshot_digest(&snapshot).expect("digest");
    let hex: String = digest.iter().map(|byte| format!("{byte:02x}")).collect();
    assert_eq!(
        hex, "268ef50bb304183037b2f1ff845f3c6e0910c4eb1f95b4c8b7d7ddafb224f99c",
        "negative-zero variant digest drift vs Go"
    );
    assert!(
        contains(&canonical, b"\"yaw\":-0}"),
        "negative zero yaw missing: {}",
        String::from_utf8_lossy(&canonical)
    );
    assert!(
        !contains(&canonical, b"-0.0"),
        "negative zero drifted to -0.0"
    );
}

#[test]
fn four_then_five_expire_cancel() {
    assert_eq!(REGISTRY_CAPACITY, 4);
    assert_eq!(SNAPSHOT_EXPIRY_GRACE, Duration::from_secs(5));
    let (start, clock) = StepClock::start();
    let entropy = entropy();
    let mut registry = test_registry(&clock, &entropy);
    let companion = companion_id("1a2b3c4d-5e6f-4a7b-9c8d-0e1f2a3b4c5d");

    let mut registrations = Vec::new();
    for generation in 1..=4u64 {
        let registration = registry
            .register(
                namespace(),
                companion,
                generation,
                fixture_snapshot(generation),
                Deadline::at(start + Duration::from_secs(1)),
            )
            .unwrap_or_else(|error| panic!("register {generation}: {error:?}"));
        assert_eq!(registration.capability.len(), 32);
        assert_eq!(registration.mcp_endpoint, "http://127.0.0.1:9/mcp");
        assert_eq!(registration.digest.len(), 32);
        registrations.push(registration);
    }
    let started = Instant::now();
    let full = registry.register(
        namespace(),
        companion,
        9,
        fixture_snapshot(9),
        Deadline::at(start + Duration::from_secs(1)),
    );
    assert!(
        matches!(
            full,
            Err(ServerError::Capacity {
                resource: Resource::Snapshots,
                limit: 4,
                observed: 4,
            })
        ),
        "fifth slot: {full:?}"
    );
    assert!(
        started.elapsed() < Duration::from_millis(50),
        "fifth slot waited"
    );

    let first = registry
        .lookup(&bearer(&registrations[0].capability))
        .expect("lookup first");
    assert_eq!(first.snapshot_id(), registrations[0].id);
    assert_eq!(first.generation(), 1);
    assert_eq!(first.namespace_id(), namespace());
    assert_eq!(first.digest(), registrations[0].digest);
    registry.complete(registrations[0].id).expect("complete");
    assert!(
        first.checkpoint().is_err(),
        "completed lease still checkpointed"
    );
    assert!(
        registry
            .lookup(&bearer(&registrations[0].capability))
            .is_err(),
        "completed capability still resolvable"
    );
    registry
        .register(
            namespace(),
            companion,
            10,
            fixture_snapshot(10),
            Deadline::at(start + Duration::from_secs(1)),
        )
        .expect("complete did not free capacity");

    // Cancel fires the outstanding read lease while the record is live.
    let live = registry
        .lookup(&bearer(&registrations[1].capability))
        .expect("lookup live");
    registry.cancel(registrations[1].id).expect("cancel");
    assert!(live.checkpoint().is_err(), "cancelled lease still live");
    assert!(
        registry
            .lookup(&bearer(&registrations[1].capability))
            .is_err(),
        "cancelled capability still resolvable"
    );

    clock.advance(Duration::from_secs(6));
    for registration in &registrations[2..] {
        assert!(
            registry.lookup(&bearer(&registration.capability)).is_err(),
            "expired capability still resolvable"
        );
    }
    for generation in 20..24u64 {
        registry
            .register(
                namespace(),
                companion,
                generation,
                fixture_snapshot(generation),
                Deadline::at(clock.monotonic() + Duration::from_secs(1)),
            )
            .unwrap_or_else(|error| panic!("register after expiry {generation}: {error:?}"));
    }

    registry.close().expect("close");
    registry.close().expect("close idempotent");
    assert!(
        registry
            .register(
                namespace(),
                companion,
                99,
                fixture_snapshot(99),
                Deadline::at(clock.monotonic() + Duration::from_secs(60)),
            )
            .is_err(),
        "register after close accepted"
    );
    assert!(
        registry.lookup("missing").is_err(),
        "lookup after close accepted"
    );
}

#[test]
fn copy_and_source_tick_not_wire() {
    // The registry freezes a deep copy: mutating the submitted value after
    // registration never reaches the stored snapshot.
    let (start, clock) = StepClock::start();
    let entropy = entropy();
    let mut registry = test_registry(&clock, &entropy);
    let companion = companion_id("1a2b3c4d-5e6f-4a7b-9c8d-0e1f2a3b4c5d");
    let mut submitted = fixture_snapshot(11);
    let registration = registry
        .register(
            namespace(),
            companion,
            7,
            submitted.clone(),
            Deadline::at(start + Duration::from_secs(60)),
        )
        .expect("register");
    submitted.exposed_blocks[0].block_id = 5;
    submitted.online_players[0].position = position([999.0, 1.0, 1.0]);
    submitted.chunk_revisions[0].revision = 999;
    let lease = registry
        .lookup(&bearer(&registration.capability))
        .expect("lookup");
    assert_eq!(lease.snapshot().exposed_blocks[0].block_id, 4);
    assert_eq!(lease.snapshot().chunk_revisions[0].revision, 7);

    // `source_tick` is private correlation: two snapshots differing only in
    // it share one digest, and the canonical bytes never name it.
    let (canonical, digest) = canonical_snapshot_digest(&fixture_snapshot(1)).expect("digest a");
    let (_, other) = canonical_snapshot_digest(&fixture_snapshot(2)).expect("digest b");
    assert_eq!(digest, other, "source_tick leaked into digest");
    assert!(
        !contains(&canonical, b"source_tick"),
        "source_tick serialized"
    );
}

#[test]
fn planning_status_digest_and_validation() {
    // The task-status text enters the digest exactly; an empty status is a
    // valid schema boundary.
    let mut planning = fixture_snapshot(3);
    planning.companion.task_status = SnapshotTaskStatusText::try_new("规划中".to_owned()).unwrap();
    let (canonical, digest) = canonical_snapshot_digest(&planning).expect("planning digest");
    assert!(
        contains(&canonical, "\"task_status\":\"规划中\"".as_bytes()),
        "status text missing from canonical"
    );
    let (_, idle_digest) = canonical_snapshot_digest(&fixture_snapshot(3)).expect("idle digest");
    assert_ne!(digest, idle_digest, "status text ignored by digest");

    let mut empty = fixture_snapshot(3);
    empty.companion.task_status = SnapshotTaskStatusText::try_new(String::new()).unwrap();
    canonical_snapshot_digest(&empty).expect("empty status valid");

    // Structural violations fail closed instead of being silently fixed.
    let mut unordered = fixture_snapshot(3);
    unordered.exposed_blocks.swap(0, 1);
    assert!(
        canonical_snapshot_digest(&unordered).is_err(),
        "unordered exposed blocks accepted"
    );
    let mut duplicated = fixture_snapshot(3);
    duplicated.exposed_blocks[1] = duplicated.exposed_blocks[0];
    assert!(
        canonical_snapshot_digest(&duplicated).is_err(),
        "duplicate exposed blocks accepted"
    );
    let mut many_players = fixture_snapshot(3);
    while many_players.online_players.len() <= 8 {
        many_players
            .online_players
            .push(many_players.online_players[0].clone());
    }
    assert!(
        canonical_snapshot_digest(&many_players).is_err(),
        "oversize online set accepted"
    );
    let mut bad_origin = fixture_snapshot(3);
    bad_origin.terrain = SnapshotTerrain::try_new(
        BlockPos::new(-9, 57, -16),
        [33, 17, 33],
        vec![0u8; 137],
        vec![-65i16; 1089],
        vec![0u16; 18_513],
    )
    .expect("shifted terrain");
    assert!(
        canonical_snapshot_digest(&bad_origin).is_err(),
        "terrain origin mismatch accepted"
    );
}

#[test]
fn register_rejects_bad_identity_and_deadline() {
    let (start, clock) = StepClock::start();
    let entropy = entropy();
    let mut registry = test_registry(&clock, &entropy);
    let companion = companion_id("1a2b3c4d-5e6f-4a7b-9c8d-0e1f2a3b4c5d");
    assert!(
        registry
            .register(
                namespace(),
                companion,
                0,
                fixture_snapshot(1),
                Deadline::at(start + Duration::from_secs(1)),
            )
            .is_err(),
        "zero generation accepted"
    );
    assert!(
        registry
            .register(
                namespace(),
                companion,
                1,
                fixture_snapshot(1),
                Deadline::at(start),
            )
            .is_err(),
        "non-future deadline accepted"
    );
    let mut mismatched = fixture_snapshot(1);
    mismatched.companion.companion_id = companion_id("2b3c4d5e-6f70-4a81-9b2d-1f2a3b4c5d6e");
    assert!(
        registry
            .register(
                namespace(),
                companion,
                1,
                mismatched,
                Deadline::at(start + Duration::from_secs(1)),
            )
            .is_err(),
        "companion scope mismatch accepted"
    );
    assert!(
        registry.complete(unknown_id()).is_err(),
        "unknown complete accepted"
    );
    assert!(
        registry.cancel(unknown_id()).is_err(),
        "unknown cancel accepted"
    );
    *entropy.fail.lock().unwrap() = true;
    assert!(
        registry
            .register(
                namespace(),
                companion,
                1,
                fixture_snapshot(1),
                Deadline::at(start + Duration::from_secs(1)),
            )
            .is_err(),
        "entropy failure accepted"
    );
    *entropy.fail.lock().unwrap() = false;
    for generation in 1..=REGISTRY_CAPACITY as u64 {
        registry
            .register(
                namespace(),
                companion,
                generation,
                fixture_snapshot(generation),
                Deadline::at(start + Duration::from_secs(1)),
            )
            .expect("failed registration must retain no snapshot slot");
    }
}

// ---------------------------------------------------------------------------
// Helpers local to this topic.
// ---------------------------------------------------------------------------

/// Renders raw capability bytes as unpadded base64url, the HTTP/MCP bearer.
fn bearer(capability: &[u8]) -> String {
    const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut text = String::with_capacity(43);
    for chunk in capability.chunks(3) {
        let word = match chunk.len() {
            3 => ((chunk[0] as u32) << 16) | ((chunk[1] as u32) << 8) | chunk[2] as u32,
            2 => ((chunk[0] as u32) << 16) | ((chunk[1] as u32) << 8),
            _ => (chunk[0] as u32) << 16,
        };
        let width = match chunk.len() {
            3 => 4,
            2 => 3,
            _ => 2,
        };
        for index in 0..width {
            let shift = 18 - 6 * index;
            text.push(ALPHABET[((word >> shift) & 63) as usize] as char);
        }
    }
    text
}

fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    format!("{:x}", hasher.finalize())
}

fn changed_terrain_block(snapshot: &mut PlanningSnapshot, block: u16) {
    let terrain = &snapshot.terrain;
    let mut blocks = terrain.blocks.clone();
    blocks[(18 * 17 + 6) * 33 + 14] = block;
    snapshot.terrain = SnapshotTerrain::try_new(
        terrain.origin,
        terrain.dimensions,
        terrain.ready_columns.clone(),
        terrain.heights.clone(),
        blocks,
    )
    .expect("changed terrain");
}

fn unknown_id() -> SnapshotId {
    SnapshotId::try_from_bytes(
        parse_canonical_uuid("aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa").unwrap(),
    )
    .unwrap()
}
