//! Compact Ready reads and tick-local writes share one observable world.

use mornlea_domain::{BlockPos, ChunkPos, Dimension, Season, Weather, WorldState, WorldStateParts};
use mornlea_server::contracts::*;
use mornlea_server::core::world::ReadyChunk;
use mornlea_server::state::{AuthorityState, TickContext};
use mornlea_storage::{Chunk, ContainerSnapshot, StorageKind};

fn key(x: i32) -> ChunkKey {
    ChunkKey {
        dimension: Dimension::OVERWORLD,
        pos: ChunkPos::new(x, 0),
    }
}
fn empty() -> Chunk {
    Chunk {
        sections: vec![
            ContainerSnapshot {
                kind: StorageKind::Single,
                bits: 0,
                single: 0,
                palette: vec![],
                packed: vec![]
            };
            24
        ],
        drops: vec![Default::default(); 32],
        furnaces: vec![Default::default(); 32],
        chests: vec![Default::default(); 16],
    }
}
fn authority() -> AuthorityState {
    AuthorityState::try_new(
        ServerLimits::try_new(8, 4096, 512, 64, 64, 1_048_576).unwrap(),
        0,
    )
    .unwrap()
}
fn budget() -> TickBudget {
    TickBudget::try_new(0, 0, 0, 0, 0).unwrap()
}
fn world() -> WorldState {
    WorldState::try_new(WorldStateParts {
        day_phase_offset: 0,
        world_time_ticks: 0,
        weather: Weather::Clear,
        season: Season::Spring,
        season_progress: 0,
        temperature: 0,
    })
    .unwrap()
}
fn write(
    ctx: &mut TickContext<'_>,
    pos: BlockPos,
    block: u16,
) -> Result<MutationOutcome, RuleReject> {
    let old = ctx.read().observation(Dimension::OVERWORLD, pos).unwrap();
    ctx.transaction().try_system(
        SystemRule::RandomBlock,
        vec![BlockWrite::try_new(old, block).unwrap()],
    )
}

#[test]
fn ready_air_and_missing() {
    let mut state = authority();
    let mut ctx = TickContext::harness(&mut state, budget());
    ctx.preload_ready_chunk(ReadyChunk::try_new(key(0), 3, 7, empty()).unwrap());
    assert!(ctx.read().ready_chunk(key(0)));
    assert!(!ctx.read().ready_chunk(key(1)));
    assert_eq!(
        ctx.read()
            .block(Dimension::OVERWORLD, BlockPos::new(0, -64, 0)),
        Some(0)
    );
    assert_eq!(
        ctx.read()
            .block(Dimension::OVERWORLD, BlockPos::new(16, 0, 0)),
        None
    );
    assert_eq!(
        ctx.read()
            .block(Dimension::OVERWORLD, BlockPos::new(0, 320, 0)),
        None
    );
    assert_eq!(
        ctx.read().highest_non_air(Dimension::OVERWORLD, 0, 0),
        Some(-65)
    );
    assert_eq!(ctx.read().highest_non_air(Dimension::DEPTHS, 0, 0), None);
}

#[test]
fn negative_boundary_and_packed() {
    let mut state = authority();
    let mut ctx = TickContext::harness(&mut state, budget());
    let mut chunk = empty();
    chunk.sections[0] = ContainerSnapshot {
        kind: StorageKind::Indexed,
        bits: 4,
        single: 0,
        palette: vec![0, 20],
        packed: vec![0x1111_1111_1111_1111; 256],
    };
    let mut words = vec![0u64; 1024];
    words[0] = 89;
    chunk.sections[1] = ContainerSnapshot {
        kind: StorageKind::Direct,
        bits: 15,
        single: 0,
        palette: vec![],
        packed: words,
    };
    ctx.preload_ready_chunk(ReadyChunk::try_new(key(-1), 8, 19, chunk).unwrap());
    assert_eq!(
        ctx.read()
            .block(Dimension::OVERWORLD, BlockPos::new(-1, -64, 0)),
        Some(20)
    );
    assert_eq!(
        ctx.read()
            .block(Dimension::OVERWORLD, BlockPos::new(-16, -48, 0)),
        Some(89)
    );
    assert_eq!(
        ctx.read()
            .block(Dimension::OVERWORLD, BlockPos::new(-17, -48, 0)),
        None
    );
    let obs = ctx
        .read()
        .observation(Dimension::OVERWORLD, BlockPos::new(-16, -48, 0))
        .unwrap();
    assert_eq!((obs.key, obs.generation, obs.revision), (key(-1), 8, 19));
    assert_eq!(
        ctx.read().highest_non_air(Dimension::OVERWORLD, -16, 0),
        Some(-48)
    );
}

#[test]
fn height_tracks_overlay_and_snapshot() {
    let mut state = authority();
    let mut ctx = TickContext::harness(&mut state, budget());
    let original = empty();
    ctx.preload_ready_chunk(ReadyChunk::try_new(key(0), 3, 7, original.clone()).unwrap());
    write(&mut ctx, BlockPos::new(0, 60, 0), 1).unwrap();
    write(&mut ctx, BlockPos::new(0, 80, 0), 20).unwrap();
    assert_eq!(
        ctx.read().highest_non_air(Dimension::OVERWORLD, 0, 0),
        Some(80)
    );
    let frozen = ctx.snapshot_state(world());
    write(&mut ctx, BlockPos::new(0, 80, 0), 0).unwrap();
    assert_eq!(
        ctx.read().highest_non_air(Dimension::OVERWORLD, 0, 0),
        Some(60)
    );
    let snapshot = ctx.snapshot_state(world());
    assert_eq!(snapshot.chunks.len(), 1);
    assert_eq!(snapshot.chunks[0].1, 3);
    assert_eq!(snapshot.chunks[0].2, 8);
    assert_eq!(snapshot.chunks[0].3.drops, original.drops);
    let mut next_state = authority();
    let next = TickContext::from_fixture(&mut next_state, &frozen, budget());
    assert_eq!(
        next.read()
            .block(Dimension::OVERWORLD, BlockPos::new(0, 80, 0)),
        Some(20)
    );
    assert_eq!(
        next.read().highest_non_air(Dimension::OVERWORLD, 0, 0),
        Some(80)
    );
}

#[test]
fn multiwrite_one_durable_revision() {
    let mut state = authority();
    let mut ctx = TickContext::harness(&mut state, budget());
    ctx.preload_ready_chunk(ReadyChunk::try_new(key(0), 3, 7, empty()).unwrap());
    ctx.preload_ready_chunk(ReadyChunk::try_new(key(1), 3, 9, empty()).unwrap());
    let positions = [BlockPos::new(0, 1, 0), BlockPos::new(1, 1, 0)];
    let writes: Vec<_> = positions
        .iter()
        .map(|p| {
            BlockWrite::try_new(ctx.read().observation(Dimension::OVERWORLD, *p).unwrap(), 1)
                .unwrap()
        })
        .collect();
    ctx.transaction()
        .try_system(SystemRule::RandomBlock, writes.clone())
        .unwrap();
    write(&mut ctx, positions[0], 2).unwrap();
    let mut bad = writes;
    bad[0].observed = ctx
        .read()
        .observation(Dimension::OVERWORLD, positions[0])
        .unwrap();
    bad[0].replacement = 20;
    assert!(
        ctx.transaction()
            .try_system(SystemRule::RandomBlock, bad)
            .is_err()
    );
    assert_eq!(
        ctx.read().block(Dimension::OVERWORLD, positions[0]),
        Some(2)
    );
    assert_eq!(
        ctx.read().highest_non_air(Dimension::OVERWORLD, 0, 0),
        Some(1)
    );
    let snapshot = ctx.snapshot_state(world());
    assert_eq!(
        snapshot.chunks.iter().map(|r| r.2).collect::<Vec<_>>(),
        vec![8, 9]
    );
    assert_eq!(snapshot.chunks[1].3, empty());
}

#[test]
fn max_revision_refuses() {
    let mut state = authority();
    let mut ctx = TickContext::harness(&mut state, budget());
    ctx.preload_ready_chunk(ReadyChunk::try_new(key(0), 3, u64::MAX, empty()).unwrap());
    assert!(write(&mut ctx, BlockPos::new(0, 1, 0), 1).is_err());
    assert_eq!(
        ctx.read()
            .block(Dimension::OVERWORLD, BlockPos::new(0, 1, 0)),
        Some(0)
    );
    assert_eq!(
        ctx.read().highest_non_air(Dimension::OVERWORLD, 0, 0),
        Some(-65)
    );
}

#[test]
fn fixture_loads_ready_chunks() {
    let fixture = FixtureState {
        runtime: vec![],
        actors: vec![],
        chunks: vec![(key(0), 2, 5, empty())],
        inventories: vec![],
        containers: vec![],
        work: WorkState::default(),
        sleep: SleepState {
            beds: vec![],
            day_phase_offset: 0,
            pending_offset: None,
        },
        projectiles: vec![],
        drops: vec![],
        world: world(),
    };
    let mut state = authority();
    let ctx = TickContext::from_fixture(&mut state, &fixture, budget());
    assert!(ctx.read().ready_chunk(key(0)));
    assert_eq!(ctx.snapshot_state(world()).chunks, fixture.chunks);
}

#[test]
fn invalid_chunk_refuses() {
    let mut chunk = empty();
    chunk.sections.pop();
    assert!(ReadyChunk::try_new(key(0), 1, 1, chunk).is_err());
    let mut chunk = empty();
    chunk.sections[0].single = 90;
    assert!(ReadyChunk::try_new(key(0), 1, 1, chunk).is_err());
    let mut chunk = empty();
    chunk.drops[0].active = true;
    assert!(ReadyChunk::try_new(key(0), 1, 1, chunk).is_err());
}

#[test]
fn unchanged_write_retains_durable_revision() {
    for revision in [7, u64::MAX] {
        let mut state = authority();
        let mut ctx = TickContext::harness(&mut state, budget());
        ctx.preload_ready_chunk(ReadyChunk::try_new(key(0), 3, revision, empty()).unwrap());
        let report = write(&mut ctx, BlockPos::new(0, 1, 0), 0).unwrap();
        assert!(report.changed.is_empty());
        assert_eq!(ctx.snapshot_state(world()).chunks[0].2, revision);
    }
}

#[test]
fn sparse_fixture_cannot_override_ready_height() {
    let mut state = authority();
    let mut ctx = TickContext::harness(&mut state, budget());
    ctx.preload_ready_chunk(ReadyChunk::try_new(key(0), 3, 7, empty()).unwrap());
    let observed = BlockObservation::try_new(key(0), 3, 7, BlockPos::new(0, 100, 0), 20).unwrap();
    assert!(
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| ctx.preload_block(observed)))
            .is_err()
    );
    assert_eq!(
        ctx.read().highest_non_air(Dimension::OVERWORLD, 0, 0),
        Some(-65)
    );
    assert_eq!(
        ctx.read().block(Dimension::OVERWORLD, observed.pos),
        Some(0)
    );
    write(&mut ctx, observed.pos, 20).unwrap();
    write(&mut ctx, observed.pos, 0).unwrap();
    assert_eq!(ctx.snapshot_state(world()).chunks[0].2, 8);
}

#[test]
fn checked_random_tunable_endpoints() {
    let defaults = RuleTunables::source_defaults();
    assert_eq!(
        (defaults.random_attempts(), defaults.crop_growth_percent()),
        (3, 50)
    );
    for (attempts, chance) in [(0, 0), (64, 100)] {
        let tunables = RuleTunables::try_new(
            defaults.physics(),
            100,
            40,
            20,
            80,
            18,
            4000,
            32,
            1600,
            200,
            5,
            attempts,
            chance,
            6.0,
            1.62,
            10,
            40,
            6000,
            1.25,
        )
        .unwrap();
        assert_eq!(
            (tunables.random_attempts(), tunables.crop_growth_percent()),
            (attempts, chance)
        );
    }
    assert!(
        RuleTunables::try_new(
            defaults.physics(),
            100,
            40,
            20,
            80,
            18,
            4000,
            32,
            1600,
            200,
            5,
            65,
            100,
            6.0,
            1.62,
            10,
            40,
            6000,
            1.25
        )
        .is_err()
    );
}
