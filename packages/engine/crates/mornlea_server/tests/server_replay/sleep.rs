//! Sleeping settlement and the seasonal morning transition replay.
//!
//! Every scene below mirrors a frozen Go oracle row, cited at each case:
//!
//! - Sleep entry (`executeInteractBed` in
//!   `packages/server/sim/entity/sleep.go` with `bedHalfPositions` in
//!   `packages/server/sim/entity/bed.go`): the authority look ray finds the
//!   bed, the night window reads the seasonized display phase
//!   (`IsDisplayNightPhase` over `EffectiveDayPhaseAt` in
//!   `packages/shared/core/day_phase.go`, 13000..23000 inclusive), a sneaking
//!   player refuses, a non-bed target settles silently, and either bed half
//!   records the foot cell as the respawn anchor
//!   (`TestBedInteractAtNightSleepsAndRecordsFootRespawn`,
//!   `TestBedSneakRefusesSleep`, `TestBedInteractNonBedTargetIsSilentNoop`).
//! - The night window follows the seasonized phase, not the linear phase
//!   (`TestBedNightFollowsSeasonalEffectivePhase`): the same linear phase
//!   12000 refuses at an equinox (day arc 12000, warp identity) and accepts
//!   at the winter solstice (day arc 8400, effective phase 14769).
//! - The day-refusal rows (`TestBedInteractOutsideNightWindowRejected`):
//!   phases 0, 12999, 23001 and 23999 refuse with a sentinel respawn record
//!   untouched, the season offset pinning each probe tick to the equinox
//!   where the warp is the identity.
//! - Wake conditions (`CommandPlayerInput` handling in
//!   `packages/server/sim/entity/tick.go`, `applyDamage` in
//!   `packages/server/sim/entity/player.go`): a move axis or the jump bit
//!   wakes, look-only input and the sprint bit do not, and real damage wakes.
//!   The damage wake consumes the accepted survival provider's victim-routed
//!   combat-hit observations; a hit naming a hostile target is an attacker
//!   confirmation, not damage, and does not wake.
//! - The seasonal morning transition
//!   (`TestSleepThroughNightLandsOnSeasonalMorning` with
//!   `settleSleepThroughNight` in `sleep.go` and `EffectiveMorningOffset`,
//!   `DayArcTicks`, `YearPhaseAt` in `day_phase.go`/`season.go`): with every
//!   active player asleep the display offset inverts to
//!   `EffectiveMorningOffset(world_time + 1,
//!   DayArcTicks(YearPhaseAt(completed, season_offset)), 0)` so the
//!   seasonized phase lands on the morning arc start once the tick completes,
//!   the absolute clock never moves, and every sleeper wakes. A disconnect
//!   mid-sleep shrinks the eligible set, and the unverified bed record
//!   survives the transition (the `TestDeathWithUnverifiedRespawnKeepsRecord`
//!   retention rule carried across the morning).
//!
//! No case chooses a value the oracle does not pin.

use mornlea_domain::{
    BlockPos, ChunkPos, CombatTarget, Dimension, Event, EventRecipient, FiniteVec3, HeldActions,
    LookAngles, MotionState, MotionStateParts, Movement, PlayerControl, PlayerControlParts, Season,
    SurvivalState, SurvivalStateParts, Weather, WorldState, WorldStateParts,
};
use mornlea_protocol::{LoginStart, admit_login};
use mornlea_server::contracts::{
    ActorAux, ActorBody, ActorLifecycle, ActorRecord, ActorRuntime, AuthorityInteraction,
    BlockObservation, ChunkKey, EnvironmentState, InteractionKind, PhaseReport, RuleCall,
    RuleEffect, RulePhase, RuleTunables, ServerError, ServerLimits, SessionKey, SleepState,
    TickBudget, TransportKind,
};
use mornlea_server::rules::sleep as provider;
use mornlea_server::state::{AuthorityState, TickContext};
use mornlea_storage::{ItemStack, PlayerLocation, PlayerSave};

const AIR: u16 = 0;
const STONE: u16 = 2;
const BED_FOOT_SOUTH: u16 = 76; // `core.BedFootSouthID`
const BED_HEAD_SOUTH: u16 = 80; // `core.BedHeadSouthID`

/// Winter-solstice season offset of the frozen Go morning fixture:
/// `216000 + core.YearTicks - (18000+1)%core.YearTicks` pins completed time
/// 18001 at year index 216000 (`TestSleepThroughNightLandsOnSeasonalMorning`).
const WINTER_SOLSTICE_SEASON_OFFSET: u32 = 485_999;
/// Absolute world time of the frozen morning fixture (`settleWorldTime`).
const SETTLE_WORLD_TIME: u64 = 18_000;
/// The completed time the offset inverts against (`worldTime + 1`).
const COMPLETED_WORLD_TIME: u64 = 18_001;
/// Winter day arc at year phase 0.75: `DayArcTicks` = round(0.35 * 24000).
const WINTER_DAY_ARC: u16 = 8_400;
/// `EffectiveMorningOffset(18001, 8400, 0)` = (0 + 24000 - 18001) % 24000.
const EXPECTED_MORNING_OFFSET: u16 = 5_999;
/// Season offsets of `TestBedNightFollowsSeasonalEffectivePhase` for linear
/// time 60000: the autumn-equinox pin `144000-60000` keeps the day arc at
/// 12000 (warp identity), the winter-solstice pin `216000-60000` compresses
/// linear 12000 into the night window.
const EQUINOX_SEASON_OFFSET: u32 = 84_000;
const SOLSTICE_SEASON_OFFSET: u32 = 156_000;

/// The equinox season offset that pins one probe tick's year index to zero,
/// where the day arc is 12000 and the warp is the identity
/// (`TestBedInteractOutsideNightWindowRejected`).
fn equinox_offset(world_time: u64) -> u32 {
    (288_000 - (world_time % 288_000) as u32) % 288_000
}

fn authority() -> AuthorityState {
    AuthorityState::try_new(
        ServerLimits::try_new(8, 4096, 512, 64, 64, 1_048_576).expect("limits"),
        0,
    )
    .expect("authority")
}

fn admitted(tag: u8, name: &str) -> mornlea_protocol::AdmittedLogin {
    let mut bytes = [0u8; 16];
    bytes[0] = tag.max(1);
    bytes[6] = 0x40;
    bytes[8] = 0x80;
    let id = mornlea_domain::PlayerId::try_from_bytes(bytes).expect("player id");
    let start = LoginStart::new(id, name, 8).expect("login start");
    let inbound = LoginStart::decode_inbound(&start.encode().expect("encoded")).expect("inbound");
    admit_login(inbound).expect("admitted login")
}

/// Mints `count` distinct session identities through the real admission path
/// on one throwaway authority. Session keys are process-local numbers, so
/// keys minted on a throwaway authority are valid fixture identities for the
/// replay authority, and one shared mint keeps the keys distinct.
fn mint_sessions(count: usize) -> Vec<SessionKey> {
    let mut mint = AuthorityState::try_new(
        ServerLimits::try_new(8, 4096, 512, 64, 64, 1_048_576).expect("limits"),
        0,
    )
    .expect("mint authority");
    (1..=count)
        .map(|tag| {
            mint.admit(
                admitted(tag as u8, &format!("sleep-{tag}")),
                TransportKind::Memory,
            )
            .expect("session")
        })
        .collect()
}

fn harness_context(authority: &mut AuthorityState) -> TickContext<'_> {
    TickContext::harness(authority, TickBudget::full())
}

fn environment(world_time: u64, day_phase_offset: u16, season_offset: u32) -> EnvironmentState {
    EnvironmentState {
        seed: 0,
        next_tick: 0,
        world_time,
        day_phase_offset,
        season_offset,
        weather: Weather::Clear,
        weather_remaining: 0,
        difficulty: 0,
        tunables: RuleTunables::source_defaults(),
    }
}

fn uuid(tag: u8) -> [u8; 16] {
    let mut bytes = [0u8; 16];
    bytes[0] = tag.max(1);
    bytes[6] = 0x40;
    bytes[8] = 0x80;
    bytes
}

fn player_actor(session: SessionKey, position: [f32; 3], yaw: f32, pitch: f32) -> ActorRecord {
    let save = PlayerSave {
        player_id: mornlea_storage::PlayerId::from_bytes(uuid(1)),
        revision: 1,
        display_name: "Tester".to_owned(),
        current: PlayerLocation {
            dimension: 0,
            position,
        },
        yaw,
        pitch,
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
    };
    ActorRecord::try_new(
        mornlea_server::contracts::ActorKey::Player(session),
        ActorLifecycle::Active,
        Dimension::OVERWORLD,
        MotionState::new(MotionStateParts {
            position: FiniteVec3::try_new(position).expect("position"),
            velocity: FiniteVec3::try_new([0.0; 3]).expect("velocity"),
            on_ground: true,
        }),
        LookAngles::try_new(yaw, pitch).expect("look"),
        SurvivalState::try_new(SurvivalStateParts {
            health: 20,
            oxygen: 300,
            hunger: 20,
            saturation_zero: false,
            armor_points: 0,
        })
        .expect("survival"),
        ActorBody::Player(save),
    )
    .expect("player actor")
}

#[allow(clippy::too_many_arguments)]
fn stage_controls(
    context: &mut TickContext<'_>,
    actor: mornlea_server::contracts::ActorKey,
    move_x: i8,
    move_z: i8,
    jump: bool,
    sprinting: bool,
    sneaking: bool,
    yaw: f32,
) {
    context
        .stage(RuleEffect::Runtime(ActorRuntime {
            key: actor,
            controls: Some(PlayerControl::new(PlayerControlParts {
                movement: Movement {
                    move_x,
                    move_z,
                    jump,
                },
                look: LookAngles::try_new(yaw, 0.0).expect("look"),
                actions: HeldActions {
                    primary: false,
                    eating: false,
                    sprinting,
                    sneaking,
                },
            })),
            has_view: false,
            reset: false,
            attack_cooldown: 0,
            hurt_cooldown: 0,
            burn_cooldown: 0,
            oxygen: 300,
            peak_y: 1.0,
            exhaustion_milli: 0,
            saturation_milli: 5_000,
            since_damage_ticks: 0,
            drown_ticks: 0,
            starvation_ticks: 0,
            eating: None,
            bow: None,
            path: None,
            aux: ActorAux::Player {
                respawn: None,
                workbench: None,
            },
        }))
        .expect("runtime");
}

fn overworld_key(pos: BlockPos) -> ChunkKey {
    ChunkKey {
        dimension: Dimension::OVERWORLD,
        pos: ChunkPos::new(pos.x() >> 4, pos.z() >> 4),
    }
}

fn observation(pos: BlockPos, block: u16) -> BlockObservation {
    BlockObservation::try_new(overworld_key(pos), 1, 1, pos, block).expect("block observation")
}

/// Stages a fully observed corridor around the eye-to-bed sight line so the
/// authority ray never reads an unobserved cell. The columns beside the aim
/// column are staged too because the look inverse is computed in `f32`, so a
/// rounding-drifted ray can graze a neighbor column; the Go oracle reads a
/// loaded region where those neighbors are observed air.
fn stage_corridor(context: &mut TickContext<'_>, x: i32, cells: &[(i32, i32, i32, u16)]) {
    for column in x - 1..=x + 1 {
        for y in 1..=3 {
            for z in 4..=10 {
                context.preload_block(observation(BlockPos::new(column, y, z), AIR));
            }
        }
    }
    for (cx, cy, cz, block) in cells {
        context.preload_block(observation(BlockPos::new(*cx, *cy, *cz), *block));
    }
}

/// The Go `lookAtPoint` inverse of `LookDirection`
/// (`packages/server/sim/entity/farming_test.go`).
fn look_at_point(eye: [f32; 3], point: [f32; 3]) -> (f32, f32) {
    let dx = f64::from(point[0] - eye[0]);
    let dy = f64::from(point[1] - eye[1]);
    let dz = f64::from(point[2] - eye[2]);
    let horizontal = dx.hypot(dz);
    let pitch = dy.atan2(horizontal) as f32;
    let yaw = (-dx).atan2(-dz) as f32;
    (yaw, pitch)
}

fn bed_interaction(
    session: SessionKey,
    yaw: f32,
    pitch: f32,
    sequence: u64,
) -> AuthorityInteraction {
    AuthorityInteraction {
        session,
        look: LookAngles::try_new(yaw, pitch).expect("look"),
        kind: InteractionKind::Bed,
        sequence,
    }
}

fn bed_call(interaction: &AuthorityInteraction) -> RuleCall<'_> {
    RuleCall {
        phase: RulePhase::SleepSettlement,
        actor: None,
        command: None,
        internal: Some(interaction),
    }
}

fn batch_call() -> RuleCall<'static> {
    RuleCall {
        phase: RulePhase::SleepSettlement,
        actor: None,
        command: None,
        internal: None,
    }
}

/// The publication world record the morning transition must leave behind:
/// the display offset changes, the absolute clock does not, and the season
/// fields derive from the not-yet-advanced tick-start time (year index
/// 215999: still autumn, in-season progress 255).
fn expected_world(day_phase_offset: u16) -> WorldState {
    WorldState::try_new(WorldStateParts {
        day_phase_offset,
        world_time_ticks: SETTLE_WORLD_TIME,
        weather: Weather::Clear,
        season: Season::Autumn,
        season_progress: 255,
        temperature: 0,
    })
    .expect("published world")
}

/// The test-local oracle for one seasonized phase, mirroring the Go
/// `EffectiveDayPhaseAt` chain (`YearPhaseAt`, `DayArcTicks`,
/// `EffectiveDayPhase` in `packages/shared/core/day_phase.go` and
/// `season.go`) so the frozen landing is checked, not trusted.
fn effective_phase_at(world_time: u64, offset: u16, season_offset: u32) -> u16 {
    let year_ticks = 288_000u64;
    let day_ticks = 24_000u32;
    let year_index = (world_time % year_ticks + u64::from(season_offset) % year_ticks) % year_ticks;
    let fraction =
        0.5 + 0.15 * (2.0 * std::f64::consts::PI * (year_index as f64 / year_ticks as f64)).sin();
    let mut arc = (fraction * f64::from(day_ticks)).round() as i64;
    if arc % 2 == 1 {
        arc -= 1;
    }
    let arc = arc as u32;
    let p =
        (((world_time % u64::from(day_ticks)) + u64::from(offset)) % u64::from(day_ticks)) as u32;
    if p < arc {
        (p * (day_ticks / 2) / arc) as u16
    } else {
        (day_ticks / 2 + (p - arc) * (day_ticks / 2) / (day_ticks - arc)) as u16
    }
}

/// The frozen fixture's day-arc precondition row: the winter arc is the short
/// one, exactly 8400 (`arc >= core.DayLengthTicks/2` guard in
/// `TestSleepThroughNightLandsOnSeasonalMorning`).
fn fixture_day_arc_is_winter_short() -> bool {
    let year_ticks = 288_000u64;
    let year_index = (COMPLETED_WORLD_TIME % year_ticks
        + u64::from(WINTER_SOLSTICE_SEASON_OFFSET) % year_ticks)
        % year_ticks;
    let fraction =
        0.5 + 0.15 * (2.0 * std::f64::consts::PI * (year_index as f64 / year_ticks as f64)).sin();
    let mut arc = (fraction * 24_000.0).round() as i64;
    if arc % 2 == 1 {
        arc -= 1;
    }
    arc < 12_000 && arc == i64::from(WINTER_DAY_ARC)
}

/// The frozen morning row, end to end
/// (`TestSleepThroughNightLandsOnSeasonalMorning`): two active sleepers
/// invert the display offset to the season's morning arc start while the
/// absolute clock stays put, a disconnect mid-sleep updates the eligible
/// count without changing the landing, one awake player and an empty active
/// set each keep the record unchanged, and the unverified bed anchor
/// survives the transition.
#[test]
fn seasonal_morning_disconnect_and_respawn() {
    let sessions = mint_sessions(2);
    let one = sessions[0];
    let two = sessions[1];
    // The unverified anchor points into a far chunk with no staged
    // observation, exactly the unready record the retention rule keeps.
    let unready_anchor = BlockPos::new(40, 1, 40);
    let record = SleepState::try_new(vec![(one, Dimension::OVERWORLD, unready_anchor)], 0, None)
        .expect("sleep record");

    // Both players asleep and active: the transition fires with the frozen
    // offset, the absolute clock stays at 18000, every sleeper wakes, and
    // the unready bed record is retained.
    let mut state = authority();
    let mut context = harness_context(&mut state);
    context
        .stage(RuleEffect::Environment(environment(
            SETTLE_WORLD_TIME,
            0,
            WINTER_SOLSTICE_SEASON_OFFSET,
        )))
        .expect("environment");
    let outcome = provider::settle(&mut context, &record, &[one, two], &[one, two])
        .expect("morning settlement");
    assert_eq!(
        outcome.report,
        PhaseReport {
            examined: 4,
            applied: 1,
            carried: 0,
            rejected: 0,
        }
    );
    assert_eq!(
        outcome.record,
        SleepState::try_new(
            vec![(one, Dimension::OVERWORLD, unready_anchor)],
            u64::from(EXPECTED_MORNING_OFFSET),
            Some(u64::from(EXPECTED_MORNING_OFFSET)),
        )
        .expect("settled record")
    );
    assert!(outcome.sleeping.is_empty(), "every sleeper wakes");
    assert_eq!(
        context.read().world(),
        Some(expected_world(EXPECTED_MORNING_OFFSET))
    );
    assert!(
        context.events().is_empty(),
        "the morning transition publishes no routed events"
    );
    // The Go oracle rows: the winter arc is the short one, and the
    // seasonized phase at the completed time under the new offset lands
    // exactly on the morning arc start.
    assert!(fixture_day_arc_is_winter_short());
    assert_eq!(
        effective_phase_at(
            COMPLETED_WORLD_TIME,
            EXPECTED_MORNING_OFFSET,
            WINTER_SOLSTICE_SEASON_OFFSET
        ),
        0
    );

    // A disconnect mid-sleep updates the eligible count: with session two
    // gone the remaining single sleeper is the whole active set, and the
    // transition fires with the identical landing.
    let mut state = authority();
    let mut context = harness_context(&mut state);
    context
        .stage(RuleEffect::Environment(environment(
            SETTLE_WORLD_TIME,
            0,
            WINTER_SOLSTICE_SEASON_OFFSET,
        )))
        .expect("environment");
    let outcome = provider::settle(&mut context, &record, &[one, two], &[one])
        .expect("settlement after disconnect");
    assert_eq!(
        outcome.report,
        PhaseReport {
            examined: 3,
            applied: 1,
            carried: 0,
            rejected: 0,
        }
    );
    assert_eq!(
        outcome.record.day_phase_offset,
        u64::from(EXPECTED_MORNING_OFFSET)
    );
    assert!(outcome.sleeping.is_empty());
    assert_eq!(
        context.read().world(),
        Some(expected_world(EXPECTED_MORNING_OFFSET))
    );

    // One awake player blocks the transition and the offset stays zero,
    // mirroring the two-player row of
    // `TestBedInteractAtNightSleepsAndRecordsFootRespawn`.
    let mut state = authority();
    let mut context = harness_context(&mut state);
    context
        .stage(RuleEffect::Environment(environment(
            SETTLE_WORLD_TIME,
            0,
            WINTER_SOLSTICE_SEASON_OFFSET,
        )))
        .expect("environment");
    let outcome =
        provider::settle(&mut context, &record, &[one], &[one, two]).expect("blocked settlement");
    assert_eq!(
        outcome.report,
        PhaseReport {
            examined: 3,
            applied: 0,
            carried: 0,
            rejected: 0,
        }
    );
    assert_eq!(outcome.record, record);
    assert_eq!(outcome.sleeping, vec![one]);
    assert_eq!(
        context.read().world(),
        None,
        "nothing publishes when blocked"
    );

    // No active players at all: the Go empty-session return keeps everything.
    let mut state = authority();
    let mut context = harness_context(&mut state);
    context
        .stage(RuleEffect::Environment(environment(
            SETTLE_WORLD_TIME,
            0,
            WINTER_SOLSTICE_SEASON_OFFSET,
        )))
        .expect("environment");
    let outcome =
        provider::settle(&mut context, &record, &[one], &[]).expect("empty active settlement");
    assert_eq!(outcome.report.applied, 0);
    assert_eq!(outcome.record, record);
    assert_eq!(context.read().world(), None);
}

/// Movement wake rows (`TestMovementInputCancelsSleepingKeepsRespawnPoint`
/// with the tick.go input handler): a move axis or the jump bit wakes, a
/// look-only input and the sprint bit do not, real damage wakes through the
/// survival provider's victim-routed combat hit, and a hostile-target hit is
/// an attacker confirmation that does not wake. Every wake keeps the bed
/// record.
#[test]
fn movement_and_damage_wake_keep_respawn_record() {
    let sessions = mint_sessions(2);
    let one = sessions[0];
    let two = sessions[1];
    let anchor = BlockPos::new(3, 1, 5);
    let record = SleepState::try_new(
        vec![
            (one, Dimension::OVERWORLD, anchor),
            (two, Dimension::OVERWORLD, anchor),
        ],
        0,
        None,
    )
    .expect("sleep record");

    // MoveX wakes: session one leaves the sleeping set, no transition fires
    // (an active player is awake), and both bed records survive.
    let mut state = authority();
    let mut context = harness_context(&mut state);
    context
        .stage(RuleEffect::Environment(environment(
            SETTLE_WORLD_TIME,
            0,
            equinox_offset(SETTLE_WORLD_TIME),
        )))
        .expect("environment");
    stage_controls(
        &mut context,
        mornlea_server::contracts::ActorKey::Player(one),
        1,
        0,
        false,
        false,
        false,
        0.0,
    );
    let outcome = provider::settle(&mut context, &record, &[one, two], &[one, two])
        .expect("move-wake settlement");
    assert_eq!(outcome.sleeping, vec![two]);
    assert_eq!(outcome.report.applied, 0);
    assert_eq!(outcome.record, record, "a wake never drops a bed record");

    // MoveZ wakes.
    let mut state = authority();
    let mut context = harness_context(&mut state);
    context
        .stage(RuleEffect::Environment(environment(
            SETTLE_WORLD_TIME,
            0,
            equinox_offset(SETTLE_WORLD_TIME),
        )))
        .expect("environment");
    stage_controls(
        &mut context,
        mornlea_server::contracts::ActorKey::Player(one),
        0,
        -1,
        false,
        false,
        false,
        0.0,
    );
    let outcome = provider::settle(&mut context, &record, &[one, two], &[one, two])
        .expect("movez-wake settlement");
    assert_eq!(outcome.sleeping, vec![two]);

    // Jump wakes.
    let mut state = authority();
    let mut context = harness_context(&mut state);
    context
        .stage(RuleEffect::Environment(environment(
            SETTLE_WORLD_TIME,
            0,
            equinox_offset(SETTLE_WORLD_TIME),
        )))
        .expect("environment");
    stage_controls(
        &mut context,
        mornlea_server::contracts::ActorKey::Player(one),
        0,
        0,
        true,
        false,
        false,
        0.0,
    );
    let outcome = provider::settle(&mut context, &record, &[one, two], &[one, two])
        .expect("jump-wake settlement");
    assert_eq!(outcome.sleeping, vec![two]);

    // Look-only input does not wake: the neutral input row keeps both
    // sleepers asleep and therefore fires the morning transition.
    let mut state = authority();
    let mut context = harness_context(&mut state);
    context
        .stage(RuleEffect::Environment(environment(
            SETTLE_WORLD_TIME,
            0,
            equinox_offset(SETTLE_WORLD_TIME),
        )))
        .expect("environment");
    stage_controls(
        &mut context,
        mornlea_server::contracts::ActorKey::Player(one),
        0,
        0,
        false,
        false,
        false,
        0.5,
    );
    let outcome = provider::settle(&mut context, &record, &[one, two], &[one, two])
        .expect("look-only settlement");
    assert!(outcome.sleeping.is_empty());
    assert_eq!(outcome.report.applied, 1);

    // The sprint bit alone does not wake either.
    let mut state = authority();
    let mut context = harness_context(&mut state);
    context
        .stage(RuleEffect::Environment(environment(
            SETTLE_WORLD_TIME,
            0,
            equinox_offset(SETTLE_WORLD_TIME),
        )))
        .expect("environment");
    stage_controls(
        &mut context,
        mornlea_server::contracts::ActorKey::Player(one),
        0,
        0,
        false,
        true,
        false,
        0.0,
    );
    let outcome = provider::settle(&mut context, &record, &[one, two], &[one, two])
        .expect("sprint-only settlement");
    assert!(outcome.sleeping.is_empty());
    assert_eq!(outcome.report.applied, 1);

    // Real damage wakes through the survival provider's victim-routed combat
    // hit (`TestDamageCancelsSleepingKeepsRespawnPoint`); the bed record
    // stays. A hit naming a hostile target is an attacker confirmation and
    // does not wake its recipient.
    let mut state = authority();
    let mut context = harness_context(&mut state);
    context
        .stage(RuleEffect::Environment(environment(
            SETTLE_WORLD_TIME,
            0,
            equinox_offset(SETTLE_WORLD_TIME),
        )))
        .expect("environment");
    let victim_hit = mornlea_domain::CombatHit::try_new(1, 2, CombatTarget::Player).expect("hit");
    let attacker_hit =
        mornlea_domain::CombatHit::try_new(1, 3, CombatTarget::Hostile).expect("hit");
    context
        .emit(mornlea_domain::RoutedEvent::new(
            EventRecipient::Session(one.get()),
            Event::CombatHit(victim_hit),
        ))
        .expect("emit victim hit");
    context
        .emit(mornlea_domain::RoutedEvent::new(
            EventRecipient::Session(two.get()),
            Event::CombatHit(attacker_hit),
        ))
        .expect("emit attacker hit");
    let outcome = provider::settle(&mut context, &record, &[one, two], &[one, two])
        .expect("damage-wake settlement");
    assert_eq!(
        outcome.sleeping,
        vec![two],
        "real damage wakes, a hostile hit does not"
    );
    assert_eq!(outcome.record, record, "a wake never drops a bed record");
}

/// Night entry records the foot anchor from either bed half
/// (`TestBedInteractAtNightSleepsAndRecordsFootRespawn`): the sleeping
/// record gains `(session, dimension, foot)`, the runtime respawn anchor the
/// survival death path reads points at the same foot cell, re-entry in a
/// second bed replaces the anchor instead of duplicating it, and the carried
/// display offset fields pass through untouched.
#[test]
fn night_entry_records_foot_respawn_from_either_half() {
    let sessions = mint_sessions(2);
    let one = sessions[0];
    let two = sessions[1];
    let foot = BlockPos::new(3, 1, 5);
    let head = BlockPos::new(3, 1, 6);
    let mut state = authority();
    let mut context = harness_context(&mut state);
    context
        .stage(RuleEffect::Environment(environment(
            SETTLE_WORLD_TIME,
            0,
            WINTER_SOLSTICE_SEASON_OFFSET,
        )))
        .expect("environment");
    context
        .stage(RuleEffect::Actor(player_actor(
            one,
            [3.5, 1.0, 9.5],
            0.0,
            0.0,
        )))
        .expect("actor");
    stage_corridor(
        &mut context,
        3,
        &[(3, 1, 5, BED_FOOT_SOUTH), (3, 1, 6, BED_HEAD_SOUTH)],
    );
    let eye = [3.5, 1.0 + 1.62, 9.5];
    let carried = SleepState::try_new(Vec::new(), u64::from(EXPECTED_MORNING_OFFSET), None)
        .expect("carried record");

    // Either half accepts the entry; the record stores the foot cell.
    for target in [foot, head] {
        let center = [
            target.x() as f32 + 0.5,
            target.y() as f32 + 0.5,
            target.z() as f32 + 0.5,
        ];
        let (yaw, pitch) = look_at_point(eye, center);
        let interaction = bed_interaction(one, yaw, pitch, 10);
        let outcome = provider::enter(&mut context, &carried, &interaction).expect("night entry");
        assert_eq!(
            outcome.0,
            PhaseReport {
                examined: 1,
                applied: 1,
                carried: 0,
                rejected: 0,
            }
        );
        assert_eq!(
            outcome.1.beds,
            vec![(one, Dimension::OVERWORLD, foot)],
            "the record stores the respawn anchor at the foot half"
        );
        assert_eq!(outcome.1.day_phase_offset, carried.day_phase_offset);
        assert_eq!(outcome.1.pending_offset, None);
        let staged = context
            .read()
            .runtime(mornlea_server::contracts::ActorKey::Player(one))
            .expect("runtime");
        assert_eq!(
            staged.aux,
            ActorAux::Player {
                respawn: Some((Dimension::OVERWORLD, foot)),
                workbench: None,
            },
            "the runtime respawn anchor the survival death path reads names the foot cell"
        );
    }

    // A second player entering adds their own row; a re-entry by the first
    // player replaces instead of duplicating.
    context
        .stage(RuleEffect::Actor(player_actor(
            two,
            [7.5, 1.0, 9.5],
            0.0,
            0.0,
        )))
        .expect("actor");
    stage_corridor(
        &mut context,
        7,
        &[(7, 1, 5, BED_FOOT_SOUTH), (7, 1, 6, BED_HEAD_SOUTH)],
    );
    let second_foot = BlockPos::new(7, 1, 5);
    let (yaw, pitch) = look_at_point([7.5, 1.0 + 1.62, 9.5], [7.5, 1.5, 5.5]);
    let outcome = provider::enter(
        &mut context,
        &carried,
        &bed_interaction(two, yaw, pitch, 11),
    )
    .expect("second entry");
    assert_eq!(outcome.1.beds.len(), 1);
    let (yaw, pitch) = look_at_point(eye, [3.5, 1.5, 5.5]);
    let outcome = provider::enter(
        &mut context,
        &outcome.1,
        &bed_interaction(one, yaw, pitch, 12),
    )
    .expect("re-entry");
    assert_eq!(outcome.1.beds.len(), 2);
    assert!(
        outcome.1.beds.contains(&(one, Dimension::OVERWORLD, foot))
            && outcome
                .1
                .beds
                .contains(&(two, Dimension::OVERWORLD, second_foot)),
        "re-entry replaces the session row and keeps the other player's anchor"
    );
}

/// Night entry preserves a staged workbench anchor while recording respawn:
/// Go sleep never clears the bench position, so the lifecycle keeps
/// revalidating a benched sleeper instead of silently disarming.
#[test]
fn night_entry_preserves_staged_workbench_anchor() {
    let one = mint_sessions(1)[0];
    let foot = BlockPos::new(3, 1, 5);
    let bench = BlockPos::new(0, 63, 0);
    let mut state = authority();
    let mut context = harness_context(&mut state);
    context
        .stage(RuleEffect::Environment(environment(
            SETTLE_WORLD_TIME,
            0,
            WINTER_SOLSTICE_SEASON_OFFSET,
        )))
        .expect("environment");
    context
        .stage(RuleEffect::Actor(player_actor(
            one,
            [3.5, 1.0, 9.5],
            0.0,
            0.0,
        )))
        .expect("actor");
    stage_corridor(
        &mut context,
        3,
        &[(3, 1, 5, BED_FOOT_SOUTH), (3, 1, 6, BED_HEAD_SOUTH)],
    );
    context
        .stage(RuleEffect::Runtime(ActorRuntime {
            key: mornlea_server::contracts::ActorKey::Player(one),
            controls: None,
            has_view: false,
            reset: false,
            attack_cooldown: 0,
            hurt_cooldown: 0,
            burn_cooldown: 0,
            oxygen: 300,
            peak_y: 1.0,
            exhaustion_milli: 0,
            saturation_milli: 5_000,
            since_damage_ticks: 0,
            drown_ticks: 0,
            starvation_ticks: 0,
            eating: None,
            bow: None,
            path: None,
            aux: ActorAux::Player {
                respawn: None,
                workbench: Some(bench),
            },
        }))
        .expect("runtime");
    let carried = SleepState::try_new(Vec::new(), u64::from(EXPECTED_MORNING_OFFSET), None)
        .expect("carried record");
    let eye = [3.5, 1.0 + 1.62, 9.5];
    let (yaw, pitch) = look_at_point(eye, [3.5, 1.5, 5.5]);
    provider::enter(
        &mut context,
        &carried,
        &bed_interaction(one, yaw, pitch, 10),
    )
    .expect("night entry");
    let staged = context
        .read()
        .runtime(mornlea_server::contracts::ActorKey::Player(one))
        .expect("runtime");
    assert_eq!(
        staged.aux,
        ActorAux::Player {
            respawn: Some((Dimension::OVERWORLD, foot)),
            workbench: Some(bench),
        },
        "sleep records respawn without clearing the workbench anchor"
    );
}

/// Sneak refusal, day refusal and a miss all leave the record untouched
/// (`TestBedSneakRefusesSleep`, `TestBedInteractOutsideNightWindowRejected`
/// with its sentinel respawn row).
#[test]
fn sneak_day_and_miss_refusals_keep_record() {
    let one = mint_sessions(1)[0];
    let sentinel = SleepState::try_new(
        vec![(one, Dimension::OVERWORLD, BlockPos::new(9, 8, 7))],
        0,
        None,
    )
    .expect("sentinel record");
    let eye = [3.5, 1.0 + 1.62, 9.5];
    let (yaw, pitch) = look_at_point(eye, [3.5, 1.5, 5.5]);
    let interaction = bed_interaction(one, yaw, pitch, 10);

    // Sneak refusal: a held sneak bit refuses the entry with zero state
    // change and no respawn anchor staged.
    let mut state = authority();
    let mut context = harness_context(&mut state);
    context
        .stage(RuleEffect::Environment(environment(
            SETTLE_WORLD_TIME,
            0,
            WINTER_SOLSTICE_SEASON_OFFSET,
        )))
        .expect("environment");
    context
        .stage(RuleEffect::Actor(player_actor(
            one,
            [3.5, 1.0, 9.5],
            yaw,
            pitch,
        )))
        .expect("actor");
    stage_corridor(
        &mut context,
        3,
        &[(3, 1, 5, BED_FOOT_SOUTH), (3, 1, 6, BED_HEAD_SOUTH)],
    );
    stage_controls(
        &mut context,
        mornlea_server::contracts::ActorKey::Player(one),
        0,
        0,
        false,
        false,
        true,
        yaw,
    );
    let outcome = provider::enter(&mut context, &sentinel, &interaction);
    assert_eq!(
        outcome,
        Err(ServerError::InvalidInput { field: "sleep" }),
        "sneaking refuses the entry"
    );
    let staged = context
        .read()
        .runtime(mornlea_server::contracts::ActorKey::Player(one))
        .expect("runtime");
    assert_eq!(
        staged.aux,
        ActorAux::Player {
            respawn: None,
            workbench: None,
        },
        "no anchor on refusal"
    );
    assert!(context.events().is_empty());

    // Day refusal at the frozen boundary phases: the season offset pins each
    // probe tick to the equinox so the seasonized phase equals the linear
    // phase, and no respawn anchor is staged on any refusal.
    for phase in [0u64, 12_999, 23_001, 23_999] {
        let mut state = authority();
        let mut context = harness_context(&mut state);
        context
            .stage(RuleEffect::Environment(environment(
                phase,
                0,
                equinox_offset(phase),
            )))
            .expect("environment");
        context
            .stage(RuleEffect::Actor(player_actor(
                one,
                [3.5, 1.0, 9.5],
                yaw,
                pitch,
            )))
            .expect("actor");
        stage_corridor(
            &mut context,
            3,
            &[(3, 1, 5, BED_FOOT_SOUTH), (3, 1, 6, BED_HEAD_SOUTH)],
        );
        let outcome = provider::enter(&mut context, &sentinel, &interaction);
        assert_eq!(
            outcome,
            Err(ServerError::InvalidInput { field: "sleep" }),
            "phase {phase} is outside the night window"
        );
        assert!(
            context
                .read()
                .runtime(mornlea_server::contracts::ActorKey::Player(one))
                .is_none()
        );
    }

    // A ray that meets no observed block within reach refuses with the
    // record untouched (the Go `RejectNoTarget` row).
    let mut state = authority();
    let mut context = harness_context(&mut state);
    context
        .stage(RuleEffect::Environment(environment(
            SETTLE_WORLD_TIME,
            0,
            WINTER_SOLSTICE_SEASON_OFFSET,
        )))
        .expect("environment");
    context
        .stage(RuleEffect::Actor(player_actor(
            one,
            [3.5, 1.0, 9.5],
            yaw,
            pitch,
        )))
        .expect("actor");
    stage_corridor(&mut context, 3, &[]);
    // The walk continues to the full interaction reach past the would-be
    // target, so the staged air covers the entire ray span.
    for column in 2..=4 {
        for y in 1..=3 {
            for z in 0..4 {
                context.preload_block(observation(BlockPos::new(column, y, z), AIR));
            }
        }
    }
    let outcome = provider::enter(&mut context, &sentinel, &interaction);
    assert_eq!(outcome, Err(ServerError::InvalidInput { field: "sleep" }));
}

/// The night window reads the seasonized phase
/// (`TestBedNightFollowsSeasonalEffectivePhase`): the same linear phase
/// 12000 refuses at the autumn equinox (day arc 12000, warp identity) and
/// accepts at the winter solstice (day arc 8400, effective phase 14769).
#[test]
fn night_window_follows_seasonal_effective_phase() {
    let one = mint_sessions(1)[0];
    let foot = BlockPos::new(3, 1, 5);
    let carried = SleepState::try_new(Vec::new(), 0, None).expect("record");
    let (yaw, pitch) = look_at_point([3.5, 1.0 + 1.62, 9.5], [3.5, 1.5, 5.5]);
    let interaction = bed_interaction(one, yaw, pitch, 10);
    const LINEAR_PHASE_TIME: u64 = 60_000;

    for (season_offset, accepted) in [
        (EQUINOX_SEASON_OFFSET, false),
        (SOLSTICE_SEASON_OFFSET, true),
    ] {
        let mut state = authority();
        let mut context = harness_context(&mut state);
        context
            .stage(RuleEffect::Environment(environment(
                LINEAR_PHASE_TIME,
                0,
                season_offset,
            )))
            .expect("environment");
        context
            .stage(RuleEffect::Actor(player_actor(
                one,
                [3.5, 1.0, 9.5],
                yaw,
                pitch,
            )))
            .expect("actor");
        stage_corridor(
            &mut context,
            3,
            &[(3, 1, 5, BED_FOOT_SOUTH), (3, 1, 6, BED_HEAD_SOUTH)],
        );
        let outcome = provider::enter(&mut context, &carried, &interaction);
        if accepted {
            let outcome = outcome.expect("winter effective phase is night");
            assert_eq!(
                outcome.1.beds,
                vec![(one, Dimension::OVERWORLD, foot)],
                "the winter entry records the foot anchor"
            );
        } else {
            assert_eq!(
                outcome,
                Err(ServerError::InvalidInput { field: "sleep" }),
                "the equinox effective phase 12000 is not night"
            );
        }
    }
}

/// A non-bed target settles silently
/// (`TestBedInteractNonBedTargetIsSilentNoop`): no rejection, no sleeping
/// record, no respawn anchor, no events.
#[test]
fn non_bed_target_is_silent_noop() {
    let one = mint_sessions(1)[0];
    let carried = SleepState::try_new(Vec::new(), 0, None).expect("record");
    let mut state = authority();
    let mut context = harness_context(&mut state);
    context
        .stage(RuleEffect::Environment(environment(
            SETTLE_WORLD_TIME,
            0,
            WINTER_SOLSTICE_SEASON_OFFSET,
        )))
        .expect("environment");
    context
        .stage(RuleEffect::Actor(player_actor(
            one,
            [0.5, 1.0, 8.5],
            0.0,
            0.0,
        )))
        .expect("actor");
    // The Go fixture's stone column at (0,2,5) with the air corridor to it.
    stage_corridor(&mut context, 0, &[(0, 2, 5, STONE)]);
    let eye = [0.5, 1.0 + 1.62, 8.5];
    let (yaw, pitch) = look_at_point(eye, [0.5, 2.5, 5.5]);
    let interaction = bed_interaction(one, yaw, pitch, 10);
    let outcome = provider::enter(&mut context, &carried, &interaction).expect("silent entry");
    assert_eq!(
        outcome.0,
        PhaseReport {
            examined: 1,
            applied: 0,
            carried: 0,
            rejected: 0,
        }
    );
    assert_eq!(outcome.1, carried, "a non-bed target changes nothing");
    assert!(
        context
            .read()
            .runtime(mornlea_server::contracts::ActorKey::Player(one))
            .is_none()
    );
    assert!(context.events().is_empty());
}

/// The checked S1 bound caps the record at eight bed rows
/// (`SleepState::try_new` with the frozen `MAX_SLEEP_BEDS` bound): a record
/// at the bound accepts a listed session's re-entry as a replace, so the row
/// count never grows past the bound. An over-bound row is unreachable through
/// real admission — admission caps at the same eight players, and session
/// keys number 1..=players process-locally, so no ninth distinct key exists
/// to mint — which leaves the replace row as the at-bound behavior.
#[test]
fn full_record_replaces_within_bound() {
    let sessions = mint_sessions(8);
    let anchor = BlockPos::new(3, 1, 5);
    let full: Vec<(SessionKey, Dimension, BlockPos)> = sessions
        .iter()
        .map(|session| (*session, Dimension::OVERWORLD, anchor))
        .collect();
    let carried = SleepState::try_new(full, 0, None).expect("full record");
    let mut state = authority();
    let mut context = harness_context(&mut state);
    context
        .stage(RuleEffect::Environment(environment(
            SETTLE_WORLD_TIME,
            0,
            WINTER_SOLSTICE_SEASON_OFFSET,
        )))
        .expect("environment");
    let eighth = sessions[7];
    context
        .stage(RuleEffect::Actor(player_actor(
            eighth,
            [3.5, 1.0, 9.5],
            0.0,
            0.0,
        )))
        .expect("actor");
    stage_corridor(
        &mut context,
        3,
        &[(3, 1, 5, BED_FOOT_SOUTH), (3, 1, 6, BED_HEAD_SOUTH)],
    );
    let (yaw, pitch) = look_at_point([3.5, 1.0 + 1.62, 9.5], [3.5, 1.5, 5.5]);

    // The eighth session re-entering from a second bed replaces its row
    // within the bound.
    let replaced_anchor = BlockPos::new(7, 1, 5);
    context
        .stage(RuleEffect::Actor(player_actor(
            eighth,
            [7.5, 1.0, 9.5],
            yaw,
            pitch,
        )))
        .expect("actor");
    stage_corridor(
        &mut context,
        7,
        &[(7, 1, 5, BED_FOOT_SOUTH), (7, 1, 6, BED_HEAD_SOUTH)],
    );
    let (yaw_seven, pitch_seven) = look_at_point([7.5, 1.0 + 1.62, 9.5], [7.5, 1.5, 5.5]);
    let outcome = provider::enter(
        &mut context,
        &carried,
        &bed_interaction(eighth, yaw_seven, pitch_seven, 11),
    )
    .expect("eighth re-entry");
    assert_eq!(outcome.1.beds.len(), 8);
    assert!(
        outcome
            .1
            .beds
            .contains(&(eighth, Dimension::OVERWORLD, replaced_anchor))
    );
}

/// The frozen phase entry: exactly the two owned shapes report zero work
/// from the bare call because both bodies consume reducer-carried
/// bookkeeping the frozen `RuleCall` cannot bring (the accepted furnace
/// batch precedent); every other shape refuses without effect, including the
/// door interaction kind that belongs to the placement provider.
#[test]
fn entry_shape_gate_reports_zero_work_and_refuses_foreign_shapes() {
    let one = mint_sessions(1)[0];
    let mut state = authority();
    let mut context = harness_context(&mut state);
    context
        .stage(RuleEffect::Environment(environment(
            SETTLE_WORLD_TIME,
            0,
            WINTER_SOLSTICE_SEASON_OFFSET,
        )))
        .expect("environment");
    let zero = PhaseReport {
        examined: 0,
        applied: 0,
        carried: 0,
        rejected: 0,
    };
    let interaction = bed_interaction(one, 0.0, 0.0, 10);
    assert_eq!(provider::run(&mut context, batch_call()), Ok(zero));
    assert_eq!(
        provider::run(&mut context, bed_call(&interaction)),
        Ok(zero)
    );
    assert_eq!(context.read().world(), None);
    assert!(context.events().is_empty());

    // A door-kind internal payload belongs to the placement provider.
    let door = AuthorityInteraction {
        session: one,
        look: LookAngles::try_new(0.0, 0.0).expect("look"),
        kind: InteractionKind::Door,
        sequence: 11,
    };
    assert_eq!(
        provider::run(&mut context, bed_call(&door)),
        Err(ServerError::InvalidInput { field: "phase" })
    );
    // Wrong phase, actor payload and command payload all refuse.
    let wrong_phase = RuleCall {
        phase: RulePhase::Interaction,
        actor: None,
        command: None,
        internal: Some(&interaction),
    };
    assert_eq!(
        provider::run(&mut context, wrong_phase),
        Err(ServerError::InvalidInput { field: "phase" })
    );
    let with_actor = RuleCall {
        phase: RulePhase::SleepSettlement,
        actor: Some(mornlea_server::contracts::ActorKey::Player(one)),
        command: None,
        internal: None,
    };
    assert_eq!(
        provider::run(&mut context, with_actor),
        Err(ServerError::InvalidInput { field: "phase" })
    );
}

/// Night entry shares the interaction classifier: the bed ray passes water
/// and open doors to the foot below, while a closed door is the hit and
/// settles as the silent non-bed no-op with the record untouched.
#[test]
fn night_entry_passes_transparent_cells() {
    const WATER_SOURCE: u16 = 27; // `core.WaterSourceID`
    const WATER_FLOWING: u16 = 34; // `core.WaterLevel7ID`
    const DOOR_LOWER_SOUTH_OPEN: u16 = 63; // `core.DoorLowerSouthOpen`
    const DOOR_LOWER_SOUTH_CLOSED: u16 = 62; // `core.DoorLowerSouthClosed`
    let foot = BlockPos::new(3, 1, 5);
    let eye = [3.5, 1.0 + 1.62, 9.5];
    for corridor in [WATER_SOURCE, WATER_FLOWING, DOOR_LOWER_SOUTH_OPEN] {
        let one = mint_sessions(1)[0];
        let mut state = authority();
        let mut context = harness_context(&mut state);
        context
            .stage(RuleEffect::Environment(environment(
                SETTLE_WORLD_TIME,
                0,
                WINTER_SOLSTICE_SEASON_OFFSET,
            )))
            .expect("environment");
        context
            .stage(RuleEffect::Actor(player_actor(
                one,
                [3.5, 1.0, 9.5],
                0.0,
                0.0,
            )))
            .expect("actor");
        stage_corridor(
            &mut context,
            3,
            &[
                (3, 1, 7, corridor),
                (3, 1, 5, BED_FOOT_SOUTH),
                (3, 1, 6, BED_HEAD_SOUTH),
            ],
        );
        let carried = SleepState::try_new(Vec::new(), u64::from(EXPECTED_MORNING_OFFSET), None)
            .expect("carried record");
        let (yaw, pitch) = look_at_point(eye, [3.5, 1.5, 5.5]);
        let outcome = provider::enter(
            &mut context,
            &carried,
            &bed_interaction(one, yaw, pitch, 10),
        )
        .expect("entry through a transparent cell");
        assert_eq!(
            outcome.1.beds,
            vec![(one, Dimension::OVERWORLD, foot)],
            "corridor {corridor} stays transparent to the bed ray"
        );
    }

    let one = mint_sessions(1)[0];
    let mut state = authority();
    let mut context = harness_context(&mut state);
    context
        .stage(RuleEffect::Environment(environment(
            SETTLE_WORLD_TIME,
            0,
            WINTER_SOLSTICE_SEASON_OFFSET,
        )))
        .expect("environment");
    context
        .stage(RuleEffect::Actor(player_actor(
            one,
            [3.5, 1.0, 9.5],
            0.0,
            0.0,
        )))
        .expect("actor");
    stage_corridor(
        &mut context,
        3,
        &[
            (3, 1, 7, DOOR_LOWER_SOUTH_CLOSED),
            (3, 1, 5, BED_FOOT_SOUTH),
            (3, 1, 6, BED_HEAD_SOUTH),
        ],
    );
    let carried = SleepState::try_new(Vec::new(), u64::from(EXPECTED_MORNING_OFFSET), None)
        .expect("carried record");
    let (yaw, pitch) = look_at_point(eye, [3.5, 1.5, 5.5]);
    let outcome = provider::enter(
        &mut context,
        &carried,
        &bed_interaction(one, yaw, pitch, 11),
    )
    .expect("a closed door is the silent non-bed hit");
    assert_eq!(outcome.0.applied, 0);
    assert!(outcome.1.beds.is_empty());
}
