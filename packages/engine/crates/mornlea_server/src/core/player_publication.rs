//! Tick-end private observations, derived from the settled resident records.
//!
//! Providers may emit provisional fixture observations. The live reducer owns
//! the final projection and consumes reset only after capturing this record.

use mornlea_domain::{
    BlockPos, MiningState, MiningStateParts, PlayerState, PlayerStateParts, SurvivalState,
    SurvivalStateParts, Weather, WorldState, WorldStateParts,
};

use super::contracts::{
    ActorBody, ActorLifecycle, ActorRecord, ActorRuntime, EnvironmentState, InventoryRecord,
    MiningProgress, ServerError,
};
use super::mutation::mining_rule;
use crate::rules::environment::{season_at, season_progress_at, year_phase_at};
use crate::rules::inventory::armor_points;

pub(crate) fn project(
    tick: u64,
    input_sequence: u64,
    actor: &ActorRecord,
    inventory: Option<&InventoryRecord>,
    runtime: Option<&ActorRuntime>,
    mining: Option<&MiningProgress>,
    environment: &EnvironmentState,
) -> Result<PlayerState, ServerError> {
    let ActorBody::Player(body) = &actor.body else {
        return Err(ServerError::InvalidInput {
            field: "player_publication",
        });
    };
    let mining = match mining {
        None => MiningState::try_new(MiningStateParts {
            active: false,
            target: BlockPos::ORIGIN,
            progress: 0,
            required: 0,
            harvestable: false,
        }),
        Some(progress) if progress.actor == actor.key && progress.dimension == actor.dimension => {
            let narrowed = u16::try_from(progress.elapsed)
                .ok()
                .zip(u16::try_from(progress.required).ok());
            let Some((elapsed, required)) = narrowed else {
                return Err(ServerError::InvalidInput {
                    field: "mining_publication",
                });
            };
            MiningState::try_new(MiningStateParts {
                active: true,
                target: progress.target,
                progress: elapsed,
                required,
                harvestable: mining_rule(progress.observed_block, progress.tool.item).1,
            })
        }
        Some(_) => {
            return Err(ServerError::InvalidInput {
                field: "mining_publication",
            });
        }
    }
    .map_err(|_| ServerError::InvalidInput {
        field: "mining_publication",
    })?;
    let survival = SurvivalState::try_new(SurvivalStateParts {
        health: actor.survival.health(),
        oxygen: actor.survival.oxygen(),
        hunger: actor.survival.hunger(),
        saturation_zero: runtime.map_or(body.saturation_milli == 0, |runtime| {
            runtime.saturation_milli == 0
        }),
        armor_points: armor_points(inventory.map_or(&body.armor, |inventory| &inventory.armor)),
    })
    .map_err(|_| ServerError::InvalidInput {
        field: "player_publication",
    })?;
    let world = WorldState::try_new(WorldStateParts {
        day_phase_offset: environment.day_phase_offset,
        world_time_ticks: environment.world_time,
        weather: environment.weather,
        season: season_at(environment.world_time, environment.season_offset),
        season_progress: season_progress_at(environment.world_time, environment.season_offset),
        temperature: temperature(environment, actor.motion.position().get()[1]),
    })
    .map_err(|_| ServerError::InvalidInput {
        field: "player_publication",
    })?;
    Ok(PlayerState::new(PlayerStateParts {
        server_tick: tick,
        last_input_sequence: input_sequence,
        dimension: actor.dimension,
        motion: actor.motion,
        look: actor.look,
        ready: actor.lifecycle == ActorLifecycle::Active,
        reset: runtime.is_some_and(|runtime| runtime.reset),
        mining,
        survival,
        world,
    }))
}

/// Go's seasonal day warp and temperature narrow to f32 before wire rounding.
fn temperature(environment: &EnvironmentState, y: f32) -> i8 {
    let phase = year_phase_at(environment.world_time, environment.season_offset);
    let angle = 2.0 * std::f64::consts::PI * phase;
    let mut arc = ((0.5 + 0.15 * angle.sin()) * 24_000.0).round() as u32;
    if !arc.is_multiple_of(2) {
        arc -= 1;
    }
    let display = ((environment.world_time % 24_000 + u64::from(environment.day_phase_offset))
        % 24_000) as u32;
    let effective = if display < arc {
        display * 12_000 / arc
    } else {
        12_000 + (display - arc) * 12_000 / (24_000 - arc)
    };
    let precipitation = if environment.weather == Weather::Clear {
        0.0
    } else {
        -4.0
    };
    let lapse = if y > 64.0 {
        -1.25 * f64::from(y - 64.0)
    } else {
        0.0
    };
    let degrees = 11.0
        + 19.0 * angle.sin()
        + 5.0 * (2.0 * std::f64::consts::PI * (f64::from(effective) - 6_000.0) / 24_000.0).sin()
        + precipitation
        + lapse;
    let narrowed = degrees.clamp(-40.0, 45.0) as f32;
    narrowed.round() as i8
}
