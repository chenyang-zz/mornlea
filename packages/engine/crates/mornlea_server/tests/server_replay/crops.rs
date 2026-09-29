//! Trample and snow-footprint replay.
//!
//! Every gate below mirrors a frozen Go oracle row, cited at each case:
//!
//! - The landing-edge geometry (edge = airborne the previous authority tick
//!   and grounded this tick, support layer at `floor(y - 1e-4)`, half width
//!   0.3 so at most a 2x2 strict column overlap is collected, X-then-Z
//!   append order) from `packages/server/sim/entity/trample.go`
//!   (`noteTrampleLanding`).
//! - The dual-landing idempotence from
//!   `TestTrampleDualPlayerLandingSameCellIsIdempotent` in
//!   `packages/server/sim/entity/trample_test.go`: two actors landing on one
//!   cell in the same tick append two equivalent candidates and settle the
//!   cell exactly once, because the second candidate reads the already
//!   reverted ground as non-farmland and passes.
//! - The capacity row from `TestTrampleCapacityFailureKeepsCellIntact` in the
//!   same file: a full drop budget abandons the whole cell silently — the
//!   farmland and the crop stay byte-identical and no drop appears.
//! - The exceptional second-write row from `commitTrample` in `trample.go`:
//!   when the ground revert commits but the crop write is refused, the
//!   ground→dirt result stands while the crop and the drop stay uncommitted.
//!   The source reaches this state through two sequential writes, so the
//!   Rust mirror keeps the same ordered stage (never one all-or-nothing
//!   transaction) and the staleness is induced between capture and commit
//!   the same way the accepted mutation contract tests induce it.
//!
//! No case chooses a value the oracles do not pin.

use mornlea_domain::{
    BlockPos, ChunkPos, Dimension, DropId, FiniteVec3, LookAngles, MotionState, MotionStateParts,
    Season, SurvivalState, SurvivalStateParts, Weather, WorldState, WorldStateParts,
};
use mornlea_protocol::{LoginStart, admit_login};
use mornlea_server::contracts::{
    ActorBody, ActorKey, ActorLifecycle, ActorRecord, BlockObservation, BlockWrite, ChunkKey,
    DropRecord, FixtureState, PhaseReport, RuleCall, RuleEffect, RulePhase, ServerLimits,
    SessionKey, SleepState, SystemRule, TickBudget, TransportKind, WorkState,
};
use mornlea_server::rules::crops as provider;
use mornlea_server::state::{AuthorityState, TickContext};
use mornlea_storage::{ItemStack, PlayerLocation, PlayerSave};

// Stable block numbers, mirrored from the frozen const block in
// `packages/shared/core/block.go`: `AirID` 0, `DirtID` 3, `FarmlandDryID` 35,
// `FarmlandWetID` 36 and `WheatStage7ID` 44 (the mature wheat form).
const AIR: u16 = 0;
const DIRT: u16 = 3;
const FARMLAND_DRY: u16 = 35;
const FARMLAND_WET: u16 = 36;
const WHEAT_MATURE: u16 = 44;

/// Fixed drop slots one chunk holds (`core.DropsPerChunk`,
/// `packages/shared/core/drop.go`): the capacity gate the trample settlement
/// shares with the human mining preflight.
const DROPS_PER_CHUNK: usize = 32;

fn authority() -> AuthorityState {
    AuthorityState::try_new(
        ServerLimits::try_new(8, 4096, 512, 64, 64, 1_048_576).expect("limits"),
        7,
    )
    .expect("authority")
}

fn admitted(tag: u8, name: &str) -> mornlea_protocol::AdmittedLogin {
    let mut bytes = [0u8; 16];
    bytes[0] = tag;
    bytes[6] = 0x40;
    bytes[8] = 0x80;
    let id = mornlea_domain::PlayerId::try_from_bytes(bytes).expect("player id");
    let start = LoginStart::new(id, name, 8).expect("login start");
    let inbound = LoginStart::decode_inbound(&start.encode().expect("encoded")).expect("inbound");
    admit_login(inbound).expect("admitted login")
}

fn uuid(tag: u8) -> [u8; 16] {
    let mut bytes = [0u8; 16];
    bytes[0] = tag.max(1);
    bytes[6] = 0x40;
    bytes[8] = 0x80;
    bytes
}

fn player_body(position: [f32; 3]) -> PlayerSave {
    PlayerSave {
        player_id: mornlea_storage::PlayerId::from_bytes(uuid(1)),
        revision: 1,
        display_name: "Tester".to_owned(),
        current: PlayerLocation {
            dimension: 0,
            position,
        },
        yaw: 0.0,
        pitch: 0.0,
        safe: None,
        inventory: mornlea_storage::Inventory::default(),
        health: 20,
        hunger: 20,
        saturation_milli: 5_000,
        exhaustion_milli: 0,
        respawn_present: false,
        respawn_position: [0.0, 0.0, 0.0],
        respawn_dimension: 0,
        armor: [ItemStack::default(); 4],
    }
}

fn player_actor(session: SessionKey, position: [f32; 3], on_ground: bool) -> ActorRecord {
    ActorRecord::try_new(
        ActorKey::Player(session),
        ActorLifecycle::Active,
        Dimension::OVERWORLD,
        MotionState::new(MotionStateParts {
            position: FiniteVec3::try_new(position).expect("position"),
            velocity: FiniteVec3::try_new([0.0, 0.0, 0.0]).expect("velocity"),
            on_ground,
        }),
        LookAngles::try_new(0.0, 0.0).expect("look"),
        SurvivalState::try_new(SurvivalStateParts {
            health: 20,
            oxygen: 300,
            hunger: 20,
            saturation_zero: false,
            armor_points: 0,
        })
        .expect("survival"),
        ActorBody::Player(player_body(position)),
    )
    .expect("player actor")
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
    .expect("world")
}

/// Builds a context whose pre-step poses are airborne and whose staged
/// post-motion poses are landed at `position`: the landing edge
/// `noteTrampleLanding` collects on. The fixture actors are the pre-step
/// snapshot by the context's own construction rule.
fn landing_context<'a>(
    authority: &'a mut AuthorityState,
    sessions: &[SessionKey],
    position: [f32; 3],
) -> TickContext<'a> {
    let initial = FixtureState {
        runtime: Vec::new(),
        actors: sessions
            .iter()
            .map(|session| {
                player_actor(
                    *session,
                    [position[0], position[1] + 3.0, position[2]],
                    false,
                )
            })
            .collect(),
        chunks: Vec::new(),
        inventories: Vec::new(),
        containers: Vec::new(),
        work: WorkState::default(),
        sleep: SleepState {
            beds: Vec::new(),
            day_phase_offset: 0,
            pending_offset: None,
        },
        projectiles: Vec::new(),
        drops: Vec::new(),
        world: world(),
    };
    let mut context = TickContext::from_fixture(authority, &initial, TickBudget::full());
    for session in sessions {
        context
            .stage(RuleEffect::Actor(player_actor(*session, position, true)))
            .expect("landed actor");
    }
    context
}

fn observation(pos: BlockPos, block: u16) -> BlockObservation {
    BlockObservation::try_new(chunk_key(pos), 1, 1, pos, block).expect("block observation")
}

fn chunk_key(pos: BlockPos) -> ChunkKey {
    ChunkKey {
        dimension: Dimension::OVERWORLD,
        pos: ChunkPos::new(pos.x() >> 4, pos.z() >> 4),
    }
}

/// One occupied drop slot in the crop's chunk: coal at 64, unmergeable with
/// the wheat batch, the pre-occupy pattern of the Go capacity oracle
/// (`TestPrepareDropBatchWorstCaseFailureLeavesBytesUnchanged`).
fn occupied_drop(pos: BlockPos, slot: u8) -> DropRecord {
    DropRecord {
        id: DropId::try_new(0, ChunkPos::new(0, 0), slot, 1).expect("drop id"),
        position: FiniteVec3::try_new([0.5, pos.y() as f32 + 0.5, 2.5]).expect("drop position"),
        stack: ItemStack {
            item: 5, // `core.ItemCoal`
            count: 64,
            durability: 0,
        },
        pickup_delay: 0,
        age: 0,
    }
}

/// Two actors landing on one farmland cell in the same tick settle it exactly
/// once (`TestTrampleDualPlayerLandingSameCellIsIdempotent`): both candidates
/// are examined in the frozen actor order, the first reverts the ground and
/// removes the crop through the ordered stage, and the second reads the
/// already-reverted ground as non-farmland and passes. A third actor on bare
/// farmland pins the bare row: the ground reverts to dirt through the
/// accepted transaction with zero drop side effect.
#[test]
fn dual_landing_once() {
    let mut authority = authority();
    let first = authority
        .admit(admitted(1, "Ada"), TransportKind::Memory)
        .expect("session");
    let second = authority
        .admit(admitted(2, "Ben"), TransportKind::Memory)
        .expect("session");
    let third = authority
        .admit(admitted(3, "Cy"), TransportKind::Memory)
        .expect("session");
    let foot = BlockPos::new(0, 0, 0);
    let crop = BlockPos::new(0, 1, 0);
    let bare = BlockPos::new(5, 0, 0);
    let mut context = landing_context(&mut authority, &[first, second, third], [0.5, 1.0, 0.5]);
    // Move the third actor's column onto its own bare farmland cell.
    context
        .stage(RuleEffect::Actor(player_actor(
            third,
            [5.5, 1.0, 0.5],
            true,
        )))
        .expect("third actor");
    context.preload_block(observation(foot, FARMLAND_WET));
    context.preload_block(observation(crop, WHEAT_MATURE));
    context.preload_block(observation(bare, FARMLAND_DRY));

    // The batch entry stages nothing for its own shapes and rejects foreign
    // shapes without effect, the shared batch-entry refusal.
    let mut schedule = provider::FootprintSchedule::new();
    let batch = provider::run(
        &mut context,
        RuleCall {
            phase: RulePhase::Trample,
            actor: None,
            command: None,
            internal: None,
        },
    )
    .expect("batch entry");
    assert_eq!(
        batch,
        PhaseReport {
            examined: 0,
            applied: 0,
            carried: 0,
            rejected: 0
        }
    );
    let refused = provider::run(
        &mut context,
        RuleCall {
            phase: RulePhase::Interaction,
            actor: None,
            command: None,
            internal: None,
        },
    )
    .expect("foreign shape refusal");
    assert_eq!(
        refused,
        PhaseReport {
            examined: 0,
            applied: 0,
            carried: 0,
            rejected: 0
        }
    );

    let report = provider::settle_tramples(&mut schedule, &mut context).expect("settle");
    assert_eq!(
        report,
        PhaseReport {
            examined: 3,
            applied: 2,
            carried: 0,
            rejected: 0
        }
    );
    // Exactly once: the ground is dirt and the crop is air, with no duplicate
    // settlement to undo.
    assert_eq!(context.read().block(Dimension::OVERWORLD, foot), Some(DIRT));
    assert_eq!(context.read().block(Dimension::OVERWORLD, crop), Some(AIR));
    // Bare farmland reverts without any drop side effect.
    assert_eq!(context.read().block(Dimension::OVERWORLD, bare), Some(DIRT));
    assert_eq!(context.read().drops(chunk_key(foot)).len(), 0);
    // Nothing is carried: every collected candidate settles or passes in the
    // same call and the pending buffer empties.
    assert_eq!(schedule.trample_pending(), 0);
}

/// Full drop capacity abandons the whole cell silently, and the exceptional
/// second-write fault preserves ground→dirt with the crop and the drop
/// uncommitted (`TestTrampleCapacityFailureKeepsCellIntact` plus the
/// `commitTrample` second-write arm of `trample.go`).
#[test]
fn capacity_and_second_write_fault() {
    // Capacity leg: 31 occupied slots leave one free slot, the mature wheat
    // batch needs two (wheat plus seeds), so the preflight refuses before any
    // write and the cell stays byte-identical.
    let mut authority = authority();
    let session = authority
        .admit(admitted(1, "Ada"), TransportKind::Memory)
        .expect("session");
    let foot = BlockPos::new(0, 0, 0);
    let crop = BlockPos::new(0, 1, 0);
    let mut context = landing_context(&mut authority, &[session], [0.5, 1.0, 0.5]);
    context.preload_block(observation(foot, FARMLAND_DRY));
    context.preload_block(observation(crop, WHEAT_MATURE));
    for slot in 0..(DROPS_PER_CHUNK - 1) as u8 {
        context.preload_drop(occupied_drop(foot, slot));
    }
    let mut schedule = provider::FootprintSchedule::new();
    let report = provider::settle_tramples(&mut schedule, &mut context).expect("settle");
    assert_eq!(
        report,
        PhaseReport {
            examined: 1,
            applied: 0,
            carried: 0,
            rejected: 0
        }
    );
    assert_eq!(
        context.read().block(Dimension::OVERWORLD, foot),
        Some(FARMLAND_DRY)
    );
    assert_eq!(
        context.read().block(Dimension::OVERWORLD, crop),
        Some(WHEAT_MATURE)
    );
    assert_eq!(
        context.read().drops(chunk_key(foot)).len(),
        DROPS_PER_CHUNK - 1
    );

    // Second-write leg: the crop observation advances between capture and
    // commit — the exposure window the source's two-phase design exists for —
    // induced the same way the accepted mutation contract tests induce staleness.
    // The ordered stage must keep the ground revert while the crop write and
    // the drop stay uncommitted, exactly like the Go `commitTrample` arm.
    let mut fault_authority = AuthorityState::try_new(
        ServerLimits::try_new(8, 4096, 512, 64, 64, 1_048_576).expect("limits"),
        7,
    )
    .expect("authority");
    let mut fault = TickContext::harness(&mut fault_authority, TickBudget::full());
    fault.preload_block(observation(foot, FARMLAND_DRY));
    fault.preload_block(observation(crop, WHEAT_MATURE));
    let ground = fault
        .read()
        .observation(Dimension::OVERWORLD, foot)
        .expect("ground");
    let stale_crop = fault
        .read()
        .observation(Dimension::OVERWORLD, crop)
        .expect("crop");
    for block in [0, WHEAT_MATURE] {
        let current = fault
            .read()
            .observation(Dimension::OVERWORLD, crop)
            .expect("crop");
        let bump = BlockWrite::try_new(current, block).expect("revision advance");
        fault
            .transaction()
            .try_system(SystemRule::Support, vec![bump])
            .expect("unrelated write");
    }
    let outcome = provider::commit_trample(&mut fault, ground, Some(stale_crop));
    assert_eq!(outcome, provider::TrampleCommit::GroundOnly);
    assert_eq!(fault.read().block(Dimension::OVERWORLD, foot), Some(DIRT));
    assert_eq!(
        fault.read().block(Dimension::OVERWORLD, crop),
        Some(WHEAT_MATURE)
    );
    assert_eq!(fault.read().drops(chunk_key(foot)).len(), 0);
}
