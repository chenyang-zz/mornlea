//! Public generation-owner replay against immutable independent Go terrain.

use std::thread;
use std::time::{Duration, Instant};

use mornlea_domain::{ChunkPos, Dimension, WorldState};
use mornlea_server::contracts::{
    ChunkKey, ChunkRequestId, Deadline, GenerationPoll, GenerationPort, TickBudget, WorkerLifecycle,
};
use mornlea_server::core::generation_worker::GenerationPool;
use mornlea_server::core::world::PreparedChunk;
use mornlea_server::state::{AuthorityState, TickContext};
use mornlea_storage::{Chunk, checked_section};
use sha2::{Digest, Sha256};

fn key(dimension: Dimension, x: i32, z: i32) -> ChunkKey {
    ChunkKey {
        dimension,
        pos: ChunkPos::new(x, z),
    }
}
fn deadline() -> Deadline {
    Deadline::at(Instant::now() + Duration::from_secs(5))
}
fn poll_actual(pool: &mut GenerationPool, request: ChunkRequestId) -> PreparedChunk {
    let until = deadline();
    loop {
        match pool.poll_generation(request) {
            GenerationPoll::Ready(prepared) => return prepared,
            GenerationPoll::Failed(error) => panic!("actual generation failed: {error:?}"),
            GenerationPoll::Pending => {
                assert!(!until.expired(Instant::now()), "actual provider deadline");
                thread::sleep(Duration::from_millis(1));
            }
        }
    }
}
fn dense_hash(prepared: PreparedChunk, expected: ChunkKey, generation: u64) -> String {
    assert_eq!(prepared.key(), expected);
    assert_eq!(prepared.generation(), generation);
    assert_eq!(prepared.revision(), 1);
    assert_eq!(prepared.persisted_revision(), 0);
    assert!(!prepared.needs_rewrite());
    assert!(!prepared.recovered());
    let (ready, persisted, rewrite, recovered) = prepared.into_parts();
    assert_eq!((persisted, rewrite, recovered), (0, false, false));
    // The existing offline harness exposes the opaque compact base solely for
    // provider observation; this test does not exercise live acquisition.
    let mut authority = AuthorityState::try_new(super::limits(), 42).unwrap();
    let mut observation = TickContext::harness(&mut authority, TickBudget::full());
    observation.preload_ready_chunk(ready);
    let snapshot = observation.snapshot_state(
        WorldState::try_new(mornlea_domain::WorldStateParts {
            day_phase_offset: 0,
            world_time_ticks: 0,
            weather: mornlea_domain::Weather::Clear,
            season: mornlea_domain::Season::Spring,
            season_progress: 0,
            temperature: 0,
        })
        .unwrap(),
    );
    assert_eq!(snapshot.chunks.len(), 1);
    let (actual_key, actual_generation, revision, chunk) = &snapshot.chunks[0];
    assert_eq!(
        (*actual_key, *actual_generation, *revision),
        (expected, generation, 1)
    );
    hash_chunk(chunk)
}
fn hash_chunk(chunk: &Chunk) -> String {
    assert_eq!(chunk.sections.len(), 24);
    assert_eq!(chunk.drops, vec![Default::default(); 32]);
    assert_eq!(chunk.furnaces, vec![Default::default(); 32]);
    assert_eq!(chunk.chests, vec![Default::default(); 16]);
    let mut digest = Sha256::new();
    for section in &chunk.sections {
        let checked = checked_section(section).unwrap();
        for cell in 0..4096 {
            digest.update(checked.block_at(cell).unwrap().to_le_bytes());
        }
    }
    digest
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

#[test]
fn public_pool_matches_origin_negative_goldens_with_independent_ids_and_reuse() {
    let mut pool = GenerationPool::try_new(42, false, 2).unwrap();
    let origin = key(Dimension::OVERWORLD, 0, 0);
    let negative = key(Dimension::OVERWORLD, -1, -1);
    let first = pool.start_generation(origin, 17).unwrap();
    let second = pool.start_generation(negative, 29).unwrap();
    assert_ne!(first, second);
    // Poll in reverse request order so no queue-position association can pass.
    assert_eq!(
        dense_hash(poll_actual(&mut pool, second), negative, 29),
        "53b3a8b4fef2a2522dbc04caf39dc933930701bada8ad6ee3ba39d867a6d537b"
    );
    assert_eq!(
        dense_hash(poll_actual(&mut pool, first), origin, 17),
        "15e2b654770b749dde7046c8ce56764eea329ec3e09917ffc118ace2d792ef7a"
    );
    assert!(matches!(
        pool.poll_generation(first),
        GenerationPoll::Pending
    ));
    assert_eq!(pool.owned_jobs(), 0);
    for generation in [31, 32] {
        let request = pool.start_generation(negative, generation).unwrap();
        assert!(request.get() > second.get());
        assert_eq!(
            dense_hash(poll_actual(&mut pool, request), negative, generation),
            "53b3a8b4fef2a2522dbc04caf39dc933930701bada8ad6ee3ba39d867a6d537b"
        );
    }
    pool.close(deadline()).unwrap();
    pool.close(deadline()).unwrap();
    assert_eq!(pool.owned_jobs(), 0);
}

#[test]
fn public_pool_matches_depths_seed_fixture_and_preserves_prepared_facts() {
    let fixture: serde_json::Value = serde_json::from_str(include_str!(
        "../../../../../../testdata/runtime-migration/server/worldgen-seeding.json"
    ))
    .unwrap();
    let expected = fixture["chunks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["seed"].as_i64() == Some(42) && row["dimension"].as_u64() == Some(1))
        .unwrap();
    let fluid = expected["fluid"].as_bool().unwrap();
    let mut pool = GenerationPool::try_new(42, fluid, 1).unwrap();
    let depths = key(
        Dimension::DEPTHS,
        expected["position"][0].as_i64().unwrap() as i32,
        expected["position"][1].as_i64().unwrap() as i32,
    );
    for generation in [41, 42] {
        let request = pool.start_generation(depths, generation).unwrap();
        assert_eq!(
            dense_hash(poll_actual(&mut pool, request), depths, generation),
            expected["dense_sha256"].as_str().unwrap()
        );
    }
    pool.close(deadline()).unwrap();
    assert_eq!(pool.owned_jobs(), 0);
}
