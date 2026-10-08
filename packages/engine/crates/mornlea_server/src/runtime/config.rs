//! Read-only config loading mirrored from Go `packages/shared/config`.
//!
//! Semantics match `config.Load` / `config.Defaults` for the server-owned
//! fields (`version`, `physics`, `sim`, `fluidEnabled`):
//! - missing file => built-in defaults (never creates or writes a file)
//! - JSON syntax / wrong version / wrong field type => typed [`ConfigError`]
//! - missing keys keep defaults; out-of-range numerics clamp like Go `applyGroups`
//! - unknown keys are ignored (Go warns; this loader stays silent)
//!
//! Client-only groups (`logging`, `render`, `ai`, `audioVolume`, …) are not
//! consumed here; later gaps extend the freeze surface.

use std::fs;
use std::io;
use std::path::Path;

use mornlea_engine::native::contracts::PhysicsTuning;
use serde_json::{Map, Value};

use crate::core::contracts::{RuleTunables, ServerError};

/// Current config file version; mirrors `config.CurrentVersion`.
pub const CURRENT_VERSION: i64 = 1;

/// Hard error from a present but unusable config file.
#[derive(Debug)]
pub enum ConfigError {
    /// Filesystem failure other than "not found".
    Io(io::Error),
    /// JSON syntax or structural failure.
    Parse(String),
    /// `version` present but not [`CURRENT_VERSION`].
    UnsupportedVersion { found: i64 },
    /// A named field had the wrong JSON type or failed checked conversion.
    InvalidField { field: &'static str, detail: String },
    /// Checked tunable construction refused the clamped values.
    Tunables(ServerError),
}

impl std::fmt::Display for ConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(err) => write!(f, "config io: {err}"),
            Self::Parse(detail) => write!(f, "config parse: {detail}"),
            Self::UnsupportedVersion { found } => write!(
                f,
                "config: unsupported version {found}, expected {CURRENT_VERSION}"
            ),
            Self::InvalidField { field, detail } => {
                write!(f, "config: invalid field {field}: {detail}")
            }
            Self::Tunables(err) => write!(f, "config: tunables refused: {err:?}"),
        }
    }
}

impl std::error::Error for ConfigError {}

impl From<io::Error> for ConfigError {
    fn from(err: io::Error) -> Self {
        Self::Io(err)
    }
}

/// Frozen process configuration after load.
///
/// Plain accessors feed `core` (`RuleTunables`, fluid budgets, flags). The
/// config type itself never crosses into `core`.
#[derive(Clone, Debug)]
pub struct RuntimeConfig {
    version: i64,
    rule_tunables: RuleTunables,
    fluid_updates_per_tick: u32,
    fluid_rescan_cells_per_tick: u32,
    spawn_radius: i32,
    fluid_enabled: bool,
}

impl RuntimeConfig {
    /// Compile-time defaults matching Go `config.Defaults` for server fields.
    pub fn defaults() -> Self {
        Self::from_parts(
            CURRENT_VERSION,
            default_physics_parts(),
            default_sim_parts(),
            true,
        )
        .expect("Go-pinned defaults must construct")
    }

    /// Load `path`. Missing file yields [`Self::defaults`]; never writes.
    pub fn load(path: &Path) -> Result<Self, ConfigError> {
        match fs::read(path) {
            Ok(bytes) => Self::decode(&bytes),
            Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(Self::defaults()),
            Err(err) => Err(ConfigError::Io(err)),
        }
    }

    /// Decode JSON bytes with Go `decodeConfig` precedence for server fields.
    pub fn decode(bytes: &[u8]) -> Result<Self, ConfigError> {
        let top: Value =
            serde_json::from_slice(bytes).map_err(|err| ConfigError::Parse(err.to_string()))?;
        let Some(obj) = top.as_object() else {
            return Err(ConfigError::Parse(
                "top-level value must be a JSON object".into(),
            ));
        };

        let mut version = CURRENT_VERSION;
        if let Some(raw) = lookup_ci(obj, "version") {
            version = as_i64(raw, "version")?;
            if version != CURRENT_VERSION {
                return Err(ConfigError::UnsupportedVersion { found: version });
            }
        }

        let mut physics = default_physics_parts();
        if let Some(raw) = lookup_ci(obj, "physics") {
            apply_physics(&mut physics, raw)?;
        }

        let mut sim = default_sim_parts();
        if let Some(raw) = lookup_ci(obj, "sim") {
            apply_sim(&mut sim, raw)?;
        }

        let mut fluid_enabled = true;
        if let Some(raw) = lookup_ci(obj, "fluidEnabled") {
            fluid_enabled = as_bool(raw, "fluidEnabled")?;
        }

        Self::from_parts(version, physics, sim, fluid_enabled)
    }

    fn from_parts(
        version: i64,
        physics: PhysicsParts,
        sim: SimParts,
        fluid_enabled: bool,
    ) -> Result<Self, ConfigError> {
        let tuning = PhysicsTuning {
            fixed_delta_seconds: physics.fixed_delta_seconds,
            step_height: physics.step_height,
            walk_speed: physics.walk_speed,
            ground_acceleration: physics.ground_acceleration,
            ground_deceleration: physics.ground_deceleration,
            air_acceleration: physics.air_acceleration,
            jump_speed: physics.jump_speed,
            gravity: physics.gravity,
            terminal_fall_speed: physics.terminal_fall_speed,
            fluid_gravity: physics.fluid_gravity,
            fluid_sink_speed: physics.fluid_sink_speed,
            fluid_ascend_speed: physics.fluid_ascend_speed,
            fluid_horizontal_drag: physics.fluid_horizontal_drag,
            sprint_speed_multiplier: physics.sprint_speed_multiplier,
            sneak_speed_multiplier: physics.sneak_speed_multiplier,
        };
        let rule_tunables = RuleTunables::try_new(
            tuning,
            sim.regen_delay_ticks,
            sim.regen_interval_ticks,
            sim.drown_damage_interval_ticks,
            sim.starvation_damage_interval_ticks,
            sim.regen_hunger_threshold,
            sim.exhaustion_threshold_milli,
            sim.eating_ticks,
            sim.furnace_burn_ticks,
            sim.furnace_smelt_ticks,
            u64::from(sim.fluid_flow_delay_ticks),
            sim.random_ticks_per_section,
            sim.crop_growth_chance_percent,
            sim.interaction_reach,
            physics.eye_height,
            sim.drop_pickup_delay_ticks,
            sim.player_drop_pickup_delay_ticks,
            sim.drop_lifetime_ticks,
            sim.drop_pickup_range,
        )
        .map_err(ConfigError::Tunables)?;
        Ok(Self {
            version,
            rule_tunables,
            fluid_updates_per_tick: sim.fluid_updates_per_tick,
            fluid_rescan_cells_per_tick: sim.fluid_rescan_cells_per_tick,
            spawn_radius: sim.spawn_radius,
            fluid_enabled,
        })
    }

    pub fn version(&self) -> i64 {
        self.version
    }

    /// Plain rule snapshot for `core`; `core` never sees [`RuntimeConfig`].
    pub fn rule_tunables(&self) -> RuleTunables {
        self.rule_tunables
    }

    pub fn fluid_updates_per_tick(&self) -> u32 {
        self.fluid_updates_per_tick
    }

    pub fn fluid_rescan_cells_per_tick(&self) -> u32 {
        self.fluid_rescan_cells_per_tick
    }

    pub fn spawn_radius(&self) -> i32 {
        self.spawn_radius
    }

    pub fn fluid_enabled(&self) -> bool {
        self.fluid_enabled
    }
}

#[derive(Clone, Copy, Debug)]
struct PhysicsParts {
    fixed_delta_seconds: f32,
    eye_height: f32,
    step_height: f32,
    walk_speed: f32,
    ground_acceleration: f32,
    ground_deceleration: f32,
    air_acceleration: f32,
    jump_speed: f32,
    gravity: f32,
    terminal_fall_speed: f32,
    fluid_gravity: f32,
    fluid_sink_speed: f32,
    fluid_ascend_speed: f32,
    fluid_horizontal_drag: f32,
    sprint_speed_multiplier: f32,
    sneak_speed_multiplier: f32,
}

#[derive(Clone, Copy, Debug)]
struct SimParts {
    interaction_reach: f32,
    regen_delay_ticks: u32,
    regen_interval_ticks: u32,
    drown_damage_interval_ticks: u32,
    drop_pickup_delay_ticks: u8,
    player_drop_pickup_delay_ticks: u8,
    drop_lifetime_ticks: u32,
    drop_pickup_range: f32,
    spawn_radius: i32,
    furnace_smelt_ticks: u8,
    furnace_burn_ticks: u16,
    fluid_flow_delay_ticks: u32,
    fluid_updates_per_tick: u32,
    fluid_rescan_cells_per_tick: u32,
    random_ticks_per_section: u8,
    crop_growth_chance_percent: u8,
    starvation_damage_interval_ticks: u32,
    exhaustion_threshold_milli: u16,
    regen_hunger_threshold: u8,
    eating_ticks: u16,
}

fn default_physics_parts() -> PhysicsParts {
    PhysicsParts {
        fixed_delta_seconds: 0.05,
        eye_height: 1.62,
        step_height: 0.6,
        walk_speed: 4.3,
        ground_acceleration: 40.0,
        ground_deceleration: 50.0,
        air_acceleration: 8.0,
        jump_speed: 8.4,
        gravity: 32.0,
        terminal_fall_speed: 78.4,
        fluid_gravity: 6.4,
        fluid_sink_speed: 3.0,
        fluid_ascend_speed: 4.0,
        fluid_horizontal_drag: 0.8,
        sprint_speed_multiplier: 1.3,
        sneak_speed_multiplier: 0.3,
    }
}

fn default_sim_parts() -> SimParts {
    SimParts {
        interaction_reach: 6.0,
        regen_delay_ticks: 100,
        regen_interval_ticks: 40,
        drown_damage_interval_ticks: 20,
        drop_pickup_delay_ticks: 10,
        player_drop_pickup_delay_ticks: 40,
        drop_lifetime_ticks: 6000,
        drop_pickup_range: 1.25,
        spawn_radius: 16,
        furnace_smelt_ticks: 200,
        furnace_burn_ticks: 1600,
        fluid_flow_delay_ticks: 5,
        fluid_updates_per_tick: 512,
        fluid_rescan_cells_per_tick: 65_536,
        random_ticks_per_section: 3,
        crop_growth_chance_percent: 50,
        starvation_damage_interval_ticks: 80,
        exhaustion_threshold_milli: 4000,
        regen_hunger_threshold: 18,
        eating_ticks: 32,
    }
}

fn apply_physics(parts: &mut PhysicsParts, raw: &Value) -> Result<(), ConfigError> {
    let fields = as_object(raw, "physics")?;
    // Ranges from config.Fields() physics group (config.go:1435+).
    set_f32(fields, "eyeHeight", 1.0, 2.2, &mut parts.eye_height)?;
    set_f32(fields, "stepHeight", 0.0, 1.5, &mut parts.step_height)?;
    set_f32(fields, "walkSpeed", 0.5, 20.0, &mut parts.walk_speed)?;
    set_f32(
        fields,
        "groundAcceleration",
        1.0,
        200.0,
        &mut parts.ground_acceleration,
    )?;
    set_f32(
        fields,
        "groundDeceleration",
        1.0,
        200.0,
        &mut parts.ground_deceleration,
    )?;
    set_f32(
        fields,
        "airAcceleration",
        1.0,
        100.0,
        &mut parts.air_acceleration,
    )?;
    set_f32(fields, "jumpSpeed", 1.0, 30.0, &mut parts.jump_speed)?;
    set_f32(fields, "gravity", 1.0, 100.0, &mut parts.gravity)?;
    set_f32(
        fields,
        "terminalFallSpeed",
        1.0,
        200.0,
        &mut parts.terminal_fall_speed,
    )?;
    set_f32(fields, "fluidGravity", 0.0, 100.0, &mut parts.fluid_gravity)?;
    set_f32(
        fields,
        "fluidSinkSpeed",
        0.0,
        200.0,
        &mut parts.fluid_sink_speed,
    )?;
    set_f32(
        fields,
        "fluidAscendSpeed",
        0.0,
        30.0,
        &mut parts.fluid_ascend_speed,
    )?;
    set_f32(
        fields,
        "fluidHorizontalDrag",
        0.0,
        1.0,
        &mut parts.fluid_horizontal_drag,
    )?;
    set_f32(
        fields,
        "sprintSpeedMultiplier",
        1.0,
        3.0,
        &mut parts.sprint_speed_multiplier,
    )?;
    set_f32(
        fields,
        "sneakSpeedMultiplier",
        0.05,
        1.0,
        &mut parts.sneak_speed_multiplier,
    )?;
    Ok(())
}

fn apply_sim(parts: &mut SimParts, raw: &Value) -> Result<(), ConfigError> {
    let fields = as_object(raw, "sim")?;
    // Ranges from config.Fields() sim group (config.go:1454+).
    set_f32(
        fields,
        "interactionReach",
        1.0,
        32.0,
        &mut parts.interaction_reach,
    )?;
    set_u32(
        fields,
        "regenDelayTicks",
        0.0,
        2000.0,
        &mut parts.regen_delay_ticks,
    )?;
    set_u32(
        fields,
        "regenIntervalTicks",
        1.0,
        600.0,
        &mut parts.regen_interval_ticks,
    )?;
    set_u32(
        fields,
        "drownDamageIntervalTicks",
        1.0,
        600.0,
        &mut parts.drown_damage_interval_ticks,
    )?;
    set_u8(
        fields,
        "dropPickupDelayTicks",
        0.0,
        255.0,
        &mut parts.drop_pickup_delay_ticks,
    )?;
    set_u8(
        fields,
        "playerDropPickupDelayTicks",
        0.0,
        255.0,
        &mut parts.player_drop_pickup_delay_ticks,
    )?;
    set_u32(
        fields,
        "dropLifetimeTicks",
        1.0,
        120_000.0,
        &mut parts.drop_lifetime_ticks,
    )?;
    set_f32(
        fields,
        "dropPickupRange",
        0.1,
        16.0,
        &mut parts.drop_pickup_range,
    )?;
    set_i32(fields, "spawnRadius", 1.0, 64.0, &mut parts.spawn_radius)?;
    set_u8(
        fields,
        "furnaceSmeltTicks",
        1.0,
        200.0,
        &mut parts.furnace_smelt_ticks,
    )?;
    set_u16(
        fields,
        "furnaceBurnTicks",
        1.0,
        1600.0,
        &mut parts.furnace_burn_ticks,
    )?;
    set_u32(
        fields,
        "fluidFlowDelayTicks",
        0.0,
        2000.0,
        &mut parts.fluid_flow_delay_ticks,
    )?;
    set_u32(
        fields,
        "fluidUpdatesPerTick",
        1.0,
        65_536.0,
        &mut parts.fluid_updates_per_tick,
    )?;
    set_u32(
        fields,
        "fluidRescanCellsPerTick",
        1.0,
        1_048_576.0,
        &mut parts.fluid_rescan_cells_per_tick,
    )?;
    set_u8(
        fields,
        "randomTicksPerSection",
        0.0,
        64.0,
        &mut parts.random_ticks_per_section,
    )?;
    set_u8(
        fields,
        "cropGrowthChancePercent",
        0.0,
        100.0,
        &mut parts.crop_growth_chance_percent,
    )?;
    set_u32(
        fields,
        "starvationDamageIntervalTicks",
        1.0,
        2000.0,
        &mut parts.starvation_damage_interval_ticks,
    )?;
    set_u16(
        fields,
        "exhaustionThresholdMilli",
        1000.0,
        20_000.0,
        &mut parts.exhaustion_threshold_milli,
    )?;
    set_u8(
        fields,
        "regenHungerThreshold",
        0.0,
        20.0,
        &mut parts.regen_hunger_threshold,
    )?;
    set_u16(fields, "eatingTicks", 1.0, 200.0, &mut parts.eating_ticks)?;
    Ok(())
}

fn lookup_ci<'a>(obj: &'a Map<String, Value>, name: &str) -> Option<&'a Value> {
    obj.iter()
        .find(|(key, _)| key.eq_ignore_ascii_case(name))
        .map(|(_, value)| value)
}

fn as_object<'a>(
    raw: &'a Value,
    field: &'static str,
) -> Result<&'a Map<String, Value>, ConfigError> {
    raw.as_object().ok_or_else(|| ConfigError::InvalidField {
        field,
        detail: "must be a JSON object".into(),
    })
}

fn as_bool(raw: &Value, field: &'static str) -> Result<bool, ConfigError> {
    raw.as_bool().ok_or_else(|| ConfigError::InvalidField {
        field,
        detail: "must be a boolean".into(),
    })
}

fn as_i64(raw: &Value, field: &'static str) -> Result<i64, ConfigError> {
    raw.as_i64()
        .or_else(|| raw.as_u64().and_then(|v| i64::try_from(v).ok()))
        .or_else(|| {
            raw.as_f64().and_then(|v| {
                if v.fract() == 0.0 && v >= i64::MIN as f64 && v <= i64::MAX as f64 {
                    Some(v as i64)
                } else {
                    None
                }
            })
        })
        .ok_or_else(|| ConfigError::InvalidField {
            field,
            detail: "must be an integer".into(),
        })
}

fn as_f64(raw: &Value, field: &'static str) -> Result<f64, ConfigError> {
    raw.as_f64()
        .or_else(|| raw.as_i64().map(|v| v as f64))
        .or_else(|| raw.as_u64().map(|v| v as f64))
        .ok_or_else(|| ConfigError::InvalidField {
            field,
            detail: "must be a number".into(),
        })
}

fn clamp(value: f64, min: f64, max: f64) -> f64 {
    value.clamp(min, max)
}

fn set_f32(
    fields: &Map<String, Value>,
    name: &'static str,
    min: f64,
    max: f64,
    slot: &mut f32,
) -> Result<(), ConfigError> {
    let Some(raw) = lookup_ci(fields, name) else {
        return Ok(());
    };
    let value = as_f64(raw, name)?;
    *slot = clamp(value, min, max) as f32;
    Ok(())
}

fn set_u32(
    fields: &Map<String, Value>,
    name: &'static str,
    min: f64,
    max: f64,
    slot: &mut u32,
) -> Result<(), ConfigError> {
    let Some(raw) = lookup_ci(fields, name) else {
        return Ok(());
    };
    let value = as_f64(raw, name)?;
    *slot = clamp(value, min, max) as u32;
    Ok(())
}

fn set_u16(
    fields: &Map<String, Value>,
    name: &'static str,
    min: f64,
    max: f64,
    slot: &mut u16,
) -> Result<(), ConfigError> {
    let Some(raw) = lookup_ci(fields, name) else {
        return Ok(());
    };
    let value = as_f64(raw, name)?;
    *slot = clamp(value, min, max) as u16;
    Ok(())
}

fn set_u8(
    fields: &Map<String, Value>,
    name: &'static str,
    min: f64,
    max: f64,
    slot: &mut u8,
) -> Result<(), ConfigError> {
    let Some(raw) = lookup_ci(fields, name) else {
        return Ok(());
    };
    let value = as_f64(raw, name)?;
    *slot = clamp(value, min, max) as u8;
    Ok(())
}

fn set_i32(
    fields: &Map<String, Value>,
    name: &'static str,
    min: f64,
    max: f64,
    slot: &mut i32,
) -> Result<(), ConfigError> {
    let Some(raw) = lookup_ci(fields, name) else {
        return Ok(());
    };
    let value = as_f64(raw, name)?;
    *slot = clamp(value, min, max) as i32;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    /// Every Go-pinned server default; cite source file:line in the assert label.
    #[test]
    fn defaults_pin_every_go_server_field() {
        let cfg = RuntimeConfig::defaults();
        let t = cfg.rule_tunables();
        let p = t.physics();

        // version — packages/shared/config/config.go:33 CurrentVersion
        assert_eq!(cfg.version(), 1, "config.go:33 CurrentVersion");
        // fluidEnabled — packages/shared/config/config.go:236 Defaults
        assert!(
            cfg.fluid_enabled(),
            "config.go:236 FluidEnabled default true"
        );

        // physics.FixedDeltaSeconds — packages/shared/physics/types.go:15
        assert_eq!(p.fixed_delta_seconds, 0.05, "physics/types.go:15");
        // physics defaults — packages/shared/physics/types.go:48-74 + tunables.go:43
        assert_eq!(t.eye_height(), 1.62, "physics/types.go:48 defaultEyeHeight");
        assert_eq!(p.step_height, 0.6, "physics/types.go:49");
        assert_eq!(p.walk_speed, 4.3, "physics/types.go:50");
        assert_eq!(p.ground_acceleration, 40.0, "physics/types.go:51");
        assert_eq!(p.ground_deceleration, 50.0, "physics/types.go:52");
        assert_eq!(p.air_acceleration, 8.0, "physics/types.go:53");
        assert_eq!(p.jump_speed, 8.4, "physics/types.go:54");
        assert_eq!(p.gravity, 32.0, "physics/types.go:55");
        assert_eq!(p.terminal_fall_speed, 78.4, "physics/types.go:56");
        assert_eq!(p.fluid_gravity, 6.4, "physics/types.go:68");
        assert_eq!(p.fluid_sink_speed, 3.0, "physics/types.go:69");
        assert_eq!(p.fluid_ascend_speed, 4.0, "physics/types.go:70");
        assert_eq!(p.fluid_horizontal_drag, 0.8, "physics/types.go:71");
        assert_eq!(p.sprint_speed_multiplier, 1.3, "physics/types.go:73");
        assert_eq!(p.sneak_speed_multiplier, 0.3, "physics/types.go:74");

        // sim defaults — packages/shared/tuning/tunables.go
        assert_eq!(t.interaction_reach(), 6.0, "tuning/tunables.go:10");
        assert_eq!(t.regen_delay_ticks(), 100, "tuning/tunables.go:11");
        assert_eq!(t.regen_interval_ticks(), 40, "tuning/tunables.go:12");
        assert_eq!(t.drown_interval_ticks(), 20, "tuning/tunables.go:13");
        assert_eq!(t.drop_pickup_delay_ticks(), 10, "tuning/tunables.go:14");
        assert_eq!(
            t.player_drop_pickup_delay_ticks(),
            40,
            "tuning/tunables.go:15"
        );
        assert_eq!(t.drop_lifetime_ticks(), 6000, "tuning/tunables.go:16");
        assert_eq!(t.drop_pickup_range(), 1.25, "tuning/tunables.go:17");
        assert_eq!(cfg.spawn_radius(), 16, "tuning/tunables.go:18");
        assert_eq!(t.furnace_smelt_ticks(), 200, "core/furnace.go:8");
        assert_eq!(t.furnace_burn_ticks(), 1600, "core/furnace.go:9");
        assert_eq!(t.fluid_delay(), 5, "tuning/tunables.go:187");
        assert_eq!(
            cfg.fluid_updates_per_tick(),
            512,
            "tuning/tunables.go:189 defaultFluidUpdatesPerTick"
        );
        assert_eq!(
            cfg.fluid_rescan_cells_per_tick(),
            65_536,
            "tuning/tunables.go:197 defaultFluidRescanCellsPerTick"
        );
        assert_eq!(t.random_attempts(), 3, "tuning/tunables.go:206");
        assert_eq!(t.crop_growth_percent(), 50, "tuning/tunables.go:218");
        assert_eq!(t.starvation_interval_ticks(), 80, "tuning/tunables.go:21");
        assert_eq!(
            t.exhaustion_threshold_milli(),
            4000,
            "tuning/tunables.go:22"
        );
        assert_eq!(t.regen_hunger_threshold(), 18, "tuning/tunables.go:23");
        assert_eq!(t.eating_ticks(), 32, "tuning/tunables.go:24");
    }

    #[test]
    fn missing_file_returns_defaults_without_creating() {
        let path = PathBuf::from("/tmp/mornlea-runtime-config-missing-does-not-exist.json");
        assert!(!path.exists());
        let cfg = RuntimeConfig::load(&path).expect("missing file is defaults");
        assert_eq!(cfg.version(), RuntimeConfig::defaults().version());
        assert!(!path.exists(), "loader must never create the config file");
    }

    #[test]
    fn malformed_json_is_typed_error() {
        let err = RuntimeConfig::decode(b"{not json").expect_err("malformed");
        assert!(matches!(err, ConfigError::Parse(_)));
    }

    #[test]
    fn wrong_version_is_typed_error() {
        let err = RuntimeConfig::decode(br#"{"version":99}"#).expect_err("version");
        match err {
            ConfigError::UnsupportedVersion { found } => assert_eq!(found, 99),
            other => panic!("expected UnsupportedVersion, got {other}"),
        }
    }

    #[test]
    fn fluid_enabled_wrong_type_is_typed_error() {
        let err = RuntimeConfig::decode(br#"{"fluidEnabled":"yes"}"#).expect_err("type");
        assert!(matches!(
            err,
            ConfigError::InvalidField {
                field: "fluidEnabled",
                ..
            }
        ));
    }

    #[test]
    fn present_file_overrides_and_clamps() {
        let json = br#"{
            "version": 1,
            "fluidEnabled": false,
            "sim": {
                "fluidUpdatesPerTick": 999999,
                "fluidRescanCellsPerTick": 0,
                "interactionReach": 2.5
            },
            "physics": { "walkSpeed": 0.1 }
        }"#;
        let cfg = RuntimeConfig::decode(json).expect("valid");
        assert!(!cfg.fluid_enabled());
        // Clamp to Fields() max 65536 / min 1 (config.go:1466-1467).
        assert_eq!(cfg.fluid_updates_per_tick(), 65_536);
        assert_eq!(cfg.fluid_rescan_cells_per_tick(), 1);
        assert_eq!(cfg.rule_tunables().interaction_reach(), 2.5);
        // walkSpeed min 0.5 (config.go:1437)
        assert_eq!(cfg.rule_tunables().physics().walk_speed, 0.5);
    }

    #[test]
    fn case_insensitive_keys_match_go() {
        let json = br#"{"Version":1,"FluidEnabled":false,"SIM":{"FluidUpdatesPerTick":64}}"#;
        let cfg = RuntimeConfig::decode(json).expect("ci keys");
        assert!(!cfg.fluid_enabled());
        assert_eq!(cfg.fluid_updates_per_tick(), 64);
    }

    #[test]
    fn empty_object_keeps_defaults() {
        let cfg = RuntimeConfig::decode(b"{}").expect("empty");
        let d = RuntimeConfig::defaults();
        assert_eq!(cfg.fluid_updates_per_tick(), d.fluid_updates_per_tick());
        assert_eq!(cfg.fluid_enabled(), d.fluid_enabled());
    }

    #[test]
    fn load_never_writes_existing_file() {
        let dir =
            std::env::temp_dir().join(format!("mornlea-runtime-config-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.json");
        let original = br#"{"version":1,"fluidEnabled":false}"#;
        fs::write(&path, original).unwrap();
        let cfg = RuntimeConfig::load(&path).expect("load");
        assert!(!cfg.fluid_enabled());
        let after = fs::read(&path).unwrap();
        assert_eq!(after, original, "loader must not rewrite the file");
        let _ = fs::remove_dir_all(&dir);
    }

    /// Source-scan: modules that must not depend on `runtime`.
    #[test]
    fn dependency_direction_forbids_runtime_imports() {
        let crate_src = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src");
        let forbidden = ["core", "rules", "store", "transport", "agent"];
        let mut offenders = Vec::new();
        for name in forbidden {
            let root = crate_src.join(name);
            for entry in walkdir(&root) {
                let text = fs::read_to_string(&entry).unwrap_or_default();
                for (idx, line) in text.lines().enumerate() {
                    let trimmed = line.trim_start();
                    if trimmed.starts_with("//") {
                        continue;
                    }
                    if trimmed.contains("crate::runtime")
                        || trimmed.contains("mornlea_server::runtime")
                    {
                        offenders.push(format!("{}:{}: {trimmed}", entry.display(), idx + 1));
                    }
                }
            }
        }
        assert!(
            offenders.is_empty(),
            "runtime must not be imported from {:?}:\n{}",
            forbidden,
            offenders.join("\n")
        );
    }

    fn walkdir(root: &Path) -> Vec<PathBuf> {
        let mut out = Vec::new();
        let mut stack = vec![root.to_path_buf()];
        while let Some(dir) = stack.pop() {
            let Ok(entries) = fs::read_dir(&dir) else {
                continue;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    stack.push(path);
                } else if path.extension().and_then(|e| e.to_str()) == Some("rs") {
                    out.push(path);
                }
            }
        }
        out
    }
}
