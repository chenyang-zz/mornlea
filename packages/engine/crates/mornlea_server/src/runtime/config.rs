//! Read-only config loading mirrored from Go `packages/shared/config`.
//!
//! A config file must roll back and forth between the Go and Rust servers, so
//! [`RuntimeConfig::decode`] accepts and rejects exactly what Go
//! `decodeConfig` does, apart from the known differences listed in the
//! rust-authoritative-server design (serde_json input strictness and
//! case-only duplicate keys). The shared fixtures under
//! `packages/shared/config/testdata/parity` pin that on both sides.
//!
//! - missing file => built-in defaults (never creates or writes a file)
//! - JSON syntax, wrong version, or a wrong type in any group Go type-checks
//!   => typed [`ConfigError`]
//! - JSON `null` behaves like Go: absent for objects, booleans and integers,
//!   zero (then clamped) for tunable numbers, refused for pointer fields
//! - keys match exactly first, then case-insensitively like Go
//!   `lookupCaseInsensitive`; two case-only variants with no exact match are
//!   an [`ConfigError::AmbiguousKey`] because Go picks one at random
//! - unknown keys, clamped values and unknown log levels are accepted with a
//!   [`ConfigWarning`], like Go's `slog.Warn`; [`RuntimeConfig::load`] and
//!   [`RuntimeConfig::resolve`] print them to stderr
//!
//! Server-owned values (`physics`, `sim`, `fluidEnabled`), `logging` and the
//! configured `ai` group are frozen; client-only groups are validated and
//! dropped.

mod client;
mod json;
#[cfg(test)]
mod parity_tests;
mod resolve;

use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use mornlea_domain::{CompanionId, CompanionName};
use mornlea_engine::native::contracts::PhysicsTuning;

use crate::core::contracts::{RuleTunables, ServerError};

pub use client::{LogLevel, LoggingConfig};
use json::{JsonObject, JsonValue, go_equal_fold, go_to_lower};
pub use resolve::{ConfigPaths, user_config_dir};

/// Current config file version; mirrors `config.CurrentVersion`.
pub const CURRENT_VERSION: i64 = 1;

/// Top-level keys Go `warnUnknownTopLevel` knows (lowercase).
const KNOWN_TOP_LEVEL: [&str; 11] = [
    "version",
    "logging",
    "physics",
    "sim",
    "render",
    "ai",
    "texturepackpath",
    "audiovolume",
    "windowsize",
    "fluidenabled",
    "cameramode",
];

/// Hard error from a present but unusable config file.
#[derive(Debug)]
pub enum ConfigError {
    /// Filesystem failure other than "not found".
    Io(io::Error),
    /// Filesystem failure while resolving a specific path.
    Path { path: PathBuf, detail: String },
    /// JSON syntax or top-level shape failure.
    Parse(String),
    /// `version` present but not [`CURRENT_VERSION`].
    UnsupportedVersion { found: i64 },
    /// A field had the wrong JSON type or failed a Go validation rule.
    InvalidField { field: String, detail: String },
    /// No exact key matched and several keys differ only by case. Go picks one
    /// nondeterministically; Rust refuses (documented known difference).
    AmbiguousKey { field: String, keys: Vec<String> },
    /// Checked tunable construction refused the clamped values.
    Tunables(ServerError),
    /// The default config file or directory failed Go's permission gates.
    InsecurePath { path: PathBuf, detail: String },
    /// The default config file changed identity between checks.
    Replaced { path: PathBuf },
    /// Only the legacy pre-rename file exists; Go must migrate it first.
    LegacyConfigNeedsMigration { legacy: PathBuf, current: PathBuf },
    /// The user config directory cannot be determined.
    NoConfigDir(String),
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(err) => write!(f, "config io: {err}"),
            Self::Path { path, detail } => write!(f, "config {}: {detail}", path.display()),
            Self::Parse(detail) => write!(f, "config parse: {detail}"),
            Self::UnsupportedVersion { found } => write!(
                f,
                "config: unsupported version {found}, expected {CURRENT_VERSION}"
            ),
            Self::InvalidField { field, detail } => {
                write!(f, "config: invalid field {field}: {detail}")
            }
            Self::AmbiguousKey { field, keys } => write!(
                f,
                "config: {field} is ambiguous: keys {keys:?} differ only by case"
            ),
            Self::Tunables(err) => write!(f, "config: tunables refused: {err:?}"),
            Self::InsecurePath { path, detail } => {
                write!(f, "config: refusing {}: {detail}", path.display())
            }
            Self::Replaced { path } => write!(
                f,
                "config: {} was replaced while it was being opened",
                path.display()
            ),
            Self::LegacyConfigNeedsMigration { legacy, current } => write!(
                f,
                "config: found legacy config {} but no {}; start Mornlea once with the Go \
                 server or client to migrate it (the Rust server never reads or migrates \
                 the legacy file)",
                legacy.display(),
                current.display()
            ),
            Self::NoConfigDir(detail) => write!(f, "config: no user config directory: {detail}"),
        }
    }
}

impl std::error::Error for ConfigError {}

impl From<io::Error> for ConfigError {
    fn from(err: io::Error) -> Self {
        Self::Io(err)
    }
}

/// Non-fatal finding, mirroring the `slog.Warn` calls in Go `decodeConfig`.
#[derive(Clone, Debug, PartialEq)]
pub enum ConfigWarning {
    /// Unknown key ignored.
    UnknownField { field: String },
    /// Out-of-range number clamped.
    Clamped {
        field: String,
        value: f64,
        clamped: f64,
    },
    /// Wrong type ignored (`render.lodEnabled`).
    InvalidTypeIgnored { field: String, want: &'static str },
    /// Illegal value replaced by its default (`render.lodStep`, `cameraMode`).
    Defaulted { field: String, value: String },
    /// Unknown log level name; the default level stays.
    UnknownLogLevel { field: String, value: String },
    /// Retired `ai` key ignored because no companion is configured.
    RetiredFieldIgnored { field: String },
}

impl fmt::Display for ConfigWarning {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownField { field } => write!(f, "unknown field {field} ignored"),
            Self::Clamped {
                field,
                value,
                clamped,
            } => write!(f, "{field} {value} out of range, clamped to {clamped}"),
            Self::InvalidTypeIgnored { field, want } => {
                write!(f, "{field} is not a {want}, ignored")
            }
            Self::Defaulted { field, value } => {
                write!(f, "{field} {value} is illegal, default used")
            }
            Self::UnknownLogLevel { field, value } => {
                write!(f, "{field} has unknown log level {value:?}, default used")
            }
            Self::RetiredFieldIgnored { field } => {
                write!(f, "retired field {field} ignored")
            }
        }
    }
}

/// Frozen `ai` group, present only when at least one companion is configured.
///
/// Holds the credential environment variable name, never its value; the
/// companion runtime reads the value at startup and keeps it out of logs.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AiConfig {
    companions: Vec<(CompanionId, CompanionName)>,
    endpoint: String,
    api_key_env: String,
    task_timeout_minutes: u32,
}

impl AiConfig {
    /// Configured companions in file order, at most four, ids and names unique.
    pub fn companions(&self) -> &[(CompanionId, CompanionName)] {
        &self.companions
    }

    /// Loopback Agent endpoint exactly as configured.
    pub fn endpoint(&self) -> &str {
        &self.endpoint
    }

    /// Name of the environment variable holding the Agent credential.
    pub fn api_key_env(&self) -> &str {
        &self.api_key_env
    }

    /// Effective task timeout in minutes (Go `AI.TaskTimeout`), 1..=60.
    pub fn task_timeout_minutes(&self) -> u32 {
        self.task_timeout_minutes
    }
}

/// Frozen process configuration after load.
///
/// Plain accessors feed `core` (`RuleTunables`, raw fluid budgets, flags).
/// The config type itself never crosses into `core`, and fluid budgets stay
/// raw numbers here; the fluid host builds its own checked budget from them.
#[derive(Clone, Debug)]
pub struct RuntimeConfig {
    version: i64,
    rule_tunables: RuleTunables,
    fluid_updates_per_tick: u32,
    fluid_rescan_cells_per_tick: u32,
    spawn_radius: i32,
    fluid_enabled: bool,
    logging: LoggingConfig,
    ai: Option<AiConfig>,
    warnings: Vec<ConfigWarning>,
}

impl RuntimeConfig {
    /// Compile-time defaults matching Go `config.Defaults` for server fields.
    pub fn defaults() -> Self {
        Self::from_parts(
            CURRENT_VERSION,
            default_physics_parts(),
            default_sim_parts(),
            true,
            LoggingConfig::default(),
            None,
            Vec::new(),
        )
        .expect("Go-pinned defaults must construct")
    }

    /// Go `config.Load`: read `path`; a missing file yields [`Self::defaults`].
    /// Never writes. Warnings are printed to stderr.
    pub fn load(path: &Path) -> Result<Self, ConfigError> {
        let config = match fs::read(path) {
            Ok(bytes) => Self::decode(&bytes)?,
            Err(err) if err.kind() == io::ErrorKind::NotFound => Self::defaults(),
            Err(err) => return Err(ConfigError::Io(err)),
        };
        config.emit_warnings();
        Ok(config)
    }

    /// Decode JSON bytes with Go `decodeConfig` acceptance. Pure: warnings are
    /// returned through [`Self::warnings`], not printed.
    pub fn decode(bytes: &[u8]) -> Result<Self, ConfigError> {
        Self::decode_with_env(bytes, &env_var_is_set)
    }

    /// [`Self::decode`] with an injectable "environment variable is non-empty"
    /// probe, used by the `ai.agentService.apiKeyEnv` check.
    pub(crate) fn decode_with_env(
        bytes: &[u8],
        env_is_set: &dyn Fn(&str) -> bool,
    ) -> Result<Self, ConfigError> {
        let top = json::parse(bytes).map_err(|err| ConfigError::Parse(err.to_string()))?;
        let empty = JsonObject::default();
        let obj = match &top {
            JsonValue::Object(obj) => obj,
            // Go unmarshals `null` into a nil map: every key is absent.
            JsonValue::Null => &empty,
            other => {
                return Err(ConfigError::Parse(format!(
                    "top-level value must be a JSON object, got {}",
                    other.kind()
                )));
            }
        };
        let mut warnings = Vec::new();

        let mut version = CURRENT_VERSION;
        if let Some(raw) = lookup(obj, "version", "")?
            && let Some(found) = go_int(raw, "version")?
        {
            version = found;
        }
        if version != CURRENT_VERSION {
            return Err(ConfigError::UnsupportedVersion { found: version });
        }

        let logging = match lookup(obj, "logging", "")? {
            Some(raw) => client::decode_logging(raw, &mut warnings)?,
            None => LoggingConfig::default(),
        };
        let ai = match lookup(obj, "ai", "")? {
            Some(raw) => client::validate_ai(raw, env_is_set, &mut warnings)?,
            None => None,
        };
        if let Some(raw) = lookup(obj, "texturePackPath", "")? {
            client::validate_texture_pack_path(raw)?;
        }
        if let Some(raw) = lookup(obj, "audioVolume", "")? {
            client::validate_audio_volume(raw)?;
        }
        if let Some(raw) = lookup(obj, "windowSize", "")? {
            client::validate_window_size(raw)?;
        }
        let mut fluid_enabled = true;
        if let Some(raw) = lookup(obj, "fluidEnabled", "")?
            && let Some(value) = go_bool(raw, "fluidEnabled")?
        {
            fluid_enabled = value;
        }
        if let Some(raw) = lookup(obj, "cameraMode", "")?
            && let Some(mode) = go_int(raw, "cameraMode")?
            && !(0..=2).contains(&mode)
        {
            warnings.push(ConfigWarning::Defaulted {
                field: "cameraMode".into(),
                value: mode.to_string(),
            });
        }

        let mut physics = default_physics_parts();
        if let Some(fields) = group(obj, "physics")? {
            apply_physics(&mut physics, fields, &mut warnings)?;
            warn_unknown_group_fields(fields, "physics", &PHYSICS_FIELDS, &mut warnings);
        }
        let mut sim = default_sim_parts();
        if let Some(fields) = group(obj, "sim")? {
            apply_sim(&mut sim, fields, &mut warnings)?;
            warn_unknown_group_fields(fields, "sim", &SIM_FIELDS, &mut warnings);
        }
        if let Some(fields) = group(obj, "render")? {
            client::validate_render(fields, &mut warnings)?;
            let known: Vec<&str> = client::RENDER_FIELDS
                .iter()
                .map(|(name, _, _)| *name)
                .chain(client::RENDER_LOD_FIELDS)
                .collect();
            warn_unknown_group_fields(fields, "render", &known, &mut warnings);
        }
        for (key, _) in obj.map_entries() {
            if !KNOWN_TOP_LEVEL.contains(&go_to_lower(key).as_str()) {
                warnings.push(ConfigWarning::UnknownField { field: key.into() });
            }
        }

        Self::from_parts(version, physics, sim, fluid_enabled, logging, ai, warnings)
    }

    fn from_parts(
        version: i64,
        physics: PhysicsParts,
        sim: SimParts,
        fluid_enabled: bool,
        logging: LoggingConfig,
        ai: Option<AiConfig>,
        warnings: Vec<ConfigWarning>,
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
            logging,
            ai,
            warnings,
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

    /// Frozen `logging` group; the serve host applies it.
    pub fn logging(&self) -> &LoggingConfig {
        &self.logging
    }

    /// Frozen `ai` group; `None` when no companion is configured.
    pub fn ai(&self) -> Option<&AiConfig> {
        self.ai.as_ref()
    }

    /// Findings Go would `slog.Warn` while decoding.
    pub fn warnings(&self) -> &[ConfigWarning] {
        &self.warnings
    }

    fn emit_warnings(&self) {
        for warning in &self.warnings {
            eprintln!("config warning: {warning}");
        }
    }
}

/// Go `os.Getenv(name) != ""`. Names Go could never find (`=` or NUL) count
/// as unset.
fn env_var_is_set(name: &str) -> bool {
    !name.is_empty()
        && !name.contains(['=', '\0'])
        && std::env::var_os(name).is_some_and(|value| !value.is_empty())
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

/// Physics keys from Go `config.Fields()` (config.go:1435+).
const PHYSICS_FIELDS: [&str; 15] = [
    "eyeHeight",
    "stepHeight",
    "walkSpeed",
    "groundAcceleration",
    "groundDeceleration",
    "airAcceleration",
    "jumpSpeed",
    "gravity",
    "terminalFallSpeed",
    "fluidGravity",
    "fluidSinkSpeed",
    "fluidAscendSpeed",
    "fluidHorizontalDrag",
    "sprintSpeedMultiplier",
    "sneakSpeedMultiplier",
];

/// Sim keys from Go `config.Fields()` (config.go:1454+).
const SIM_FIELDS: [&str; 20] = [
    "interactionReach",
    "regenDelayTicks",
    "regenIntervalTicks",
    "drownDamageIntervalTicks",
    "dropPickupDelayTicks",
    "playerDropPickupDelayTicks",
    "dropLifetimeTicks",
    "dropPickupRange",
    "spawnRadius",
    "furnaceSmeltTicks",
    "furnaceBurnTicks",
    "fluidFlowDelayTicks",
    "fluidUpdatesPerTick",
    "fluidRescanCellsPerTick",
    "randomTicksPerSection",
    "cropGrowthChancePercent",
    "starvationDamageIntervalTicks",
    "exhaustionThresholdMilli",
    "regenHungerThreshold",
    "eatingTicks",
];

/// Go `applyGroups` for one numeric field: decode as `float64` (null is 0),
/// clamp to the `Fields()` range with a warning, and hand back the clamped
/// value for the caller's narrowing cast (Go `setFloat` truncates the same
/// way). `None` means the key is absent and the default stays.
fn group_number(
    fields: &JsonObject,
    group: &str,
    name: &str,
    min: f64,
    max: f64,
    warnings: &mut Vec<ConfigWarning>,
) -> Result<Option<f64>, ConfigError> {
    let Some(raw) = lookup(fields, name, group)? else {
        return Ok(None);
    };
    let path = format!("{group}.{name}");
    let value = go_f64(raw, &path)?;
    Ok(Some(warn_clamp(warnings, path, value, min, max)))
}

macro_rules! set_group_fields {
    ($fields:expr, $group:literal, $warnings:expr, $parts:expr,
     $( $name:literal => $slot:ident : $ty:ty , $min:expr , $max:expr ; )*) => {
        $(
            if let Some(value) = group_number($fields, $group, $name, $min, $max, $warnings)? {
                $parts.$slot = value as $ty;
            }
        )*
    };
}

fn apply_physics(
    parts: &mut PhysicsParts,
    fields: &JsonObject,
    warnings: &mut Vec<ConfigWarning>,
) -> Result<(), ConfigError> {
    // Ranges from config.Fields() physics group (config.go:1435+).
    set_group_fields!(fields, "physics", warnings, parts,
        "eyeHeight" => eye_height: f32, 1.0, 2.2;
        "stepHeight" => step_height: f32, 0.0, 1.5;
        "walkSpeed" => walk_speed: f32, 0.5, 20.0;
        "groundAcceleration" => ground_acceleration: f32, 1.0, 200.0;
        "groundDeceleration" => ground_deceleration: f32, 1.0, 200.0;
        "airAcceleration" => air_acceleration: f32, 1.0, 100.0;
        "jumpSpeed" => jump_speed: f32, 1.0, 30.0;
        "gravity" => gravity: f32, 1.0, 100.0;
        "terminalFallSpeed" => terminal_fall_speed: f32, 1.0, 200.0;
        "fluidGravity" => fluid_gravity: f32, 0.0, 100.0;
        "fluidSinkSpeed" => fluid_sink_speed: f32, 0.0, 200.0;
        "fluidAscendSpeed" => fluid_ascend_speed: f32, 0.0, 30.0;
        "fluidHorizontalDrag" => fluid_horizontal_drag: f32, 0.0, 1.0;
        "sprintSpeedMultiplier" => sprint_speed_multiplier: f32, 1.0, 3.0;
        "sneakSpeedMultiplier" => sneak_speed_multiplier: f32, 0.05, 1.0;
    );
    Ok(())
}

fn apply_sim(
    parts: &mut SimParts,
    fields: &JsonObject,
    warnings: &mut Vec<ConfigWarning>,
) -> Result<(), ConfigError> {
    // Ranges from config.Fields() sim group (config.go:1454+).
    set_group_fields!(fields, "sim", warnings, parts,
        "interactionReach" => interaction_reach: f32, 1.0, 32.0;
        "regenDelayTicks" => regen_delay_ticks: u32, 0.0, 2000.0;
        "regenIntervalTicks" => regen_interval_ticks: u32, 1.0, 600.0;
        "drownDamageIntervalTicks" => drown_damage_interval_ticks: u32, 1.0, 600.0;
        "dropPickupDelayTicks" => drop_pickup_delay_ticks: u8, 0.0, 255.0;
        "playerDropPickupDelayTicks" => player_drop_pickup_delay_ticks: u8, 0.0, 255.0;
        "dropLifetimeTicks" => drop_lifetime_ticks: u32, 1.0, 120_000.0;
        "dropPickupRange" => drop_pickup_range: f32, 0.1, 16.0;
        "spawnRadius" => spawn_radius: i32, 1.0, 64.0;
        "furnaceSmeltTicks" => furnace_smelt_ticks: u8, 1.0, 200.0;
        "furnaceBurnTicks" => furnace_burn_ticks: u16, 1.0, 1600.0;
        "fluidFlowDelayTicks" => fluid_flow_delay_ticks: u32, 0.0, 2000.0;
        "fluidUpdatesPerTick" => fluid_updates_per_tick: u32, 1.0, 65_536.0;
        "fluidRescanCellsPerTick" => fluid_rescan_cells_per_tick: u32, 1.0, 1_048_576.0;
        "randomTicksPerSection" => random_ticks_per_section: u8, 0.0, 64.0;
        "cropGrowthChancePercent" => crop_growth_chance_percent: u8, 0.0, 100.0;
        "starvationDamageIntervalTicks" => starvation_damage_interval_ticks: u32, 1.0, 2000.0;
        "exhaustionThresholdMilli" => exhaustion_threshold_milli: u16, 1000.0, 20_000.0;
        "regenHungerThreshold" => regen_hunger_threshold: u8, 0.0, 20.0;
        "eatingTicks" => eating_ticks: u16, 1.0, 200.0;
    );
    Ok(())
}

pub(crate) fn invalid(field: impl Into<String>, detail: impl Into<String>) -> ConfigError {
    ConfigError::InvalidField {
        field: field.into(),
        detail: detail.into(),
    }
}

/// Go `lookupCaseInsensitive` over a Go map: the exact key wins (last
/// duplicate); otherwise one case-insensitive match. Several case-only
/// matches are refused because Go's map iteration would pick one at random.
pub(crate) fn lookup<'a>(
    obj: &'a JsonObject,
    key: &str,
    parent: &str,
) -> Result<Option<&'a JsonValue>, ConfigError> {
    if let Some(value) = obj.get_exact(key) {
        return Ok(Some(value));
    }
    let mut matches = obj
        .map_entries()
        .filter(|(candidate, _)| go_equal_fold(candidate, key));
    let Some((first_key, first)) = matches.next() else {
        return Ok(None);
    };
    let rest: Vec<String> = matches.map(|(candidate, _)| candidate.to_owned()).collect();
    if rest.is_empty() {
        return Ok(Some(first));
    }
    let mut keys = vec![first_key.to_owned()];
    keys.extend(rest);
    Err(ConfigError::AmbiguousKey {
        field: if parent.is_empty() {
            key.to_owned()
        } else {
            format!("{parent}.{key}")
        },
        keys,
    })
}

/// A `physics`/`sim`/`render` group: object, or `null` treated as present
/// but empty (Go unmarshals it into a nil map).
fn group<'a>(obj: &'a JsonObject, name: &str) -> Result<Option<&'a JsonObject>, ConfigError> {
    static EMPTY: std::sync::OnceLock<JsonObject> = std::sync::OnceLock::new();
    match lookup(obj, name, "")? {
        None => Ok(None),
        Some(JsonValue::Null) => Ok(Some(EMPTY.get_or_init(JsonObject::default))),
        Some(JsonValue::Object(fields)) => Ok(Some(fields)),
        Some(other) => Err(invalid(
            name,
            format!("must be an object, got {}", other.kind()),
        )),
    }
}

/// Go `json.Unmarshal` into `int`: integer literal within `int64`; `null`
/// leaves the target unchanged (`None`).
pub(crate) fn go_int(raw: &JsonValue, field: &str) -> Result<Option<i64>, ConfigError> {
    match raw {
        JsonValue::Null => Ok(None),
        JsonValue::Number(number) => number
            .as_go_int()
            .map(Some)
            .ok_or_else(|| invalid(field, "must be an integer literal within int64")),
        other => Err(invalid(
            field,
            format!("must be an integer, got {}", other.kind()),
        )),
    }
}

/// Go `json.Unmarshal` into `bool`; `null` leaves the target unchanged.
fn go_bool(raw: &JsonValue, field: &str) -> Result<Option<bool>, ConfigError> {
    match raw {
        JsonValue::Null => Ok(None),
        JsonValue::Bool(value) => Ok(Some(*value)),
        other => Err(invalid(
            field,
            format!("must be a boolean, got {}", other.kind()),
        )),
    }
}

/// Go `json.Unmarshal` into a zeroed `float64`: `null` yields 0.
pub(crate) fn go_f64(raw: &JsonValue, field: &str) -> Result<f64, ConfigError> {
    match raw {
        JsonValue::Null => Ok(0.0),
        JsonValue::Number(number) => number
            .as_go_f64()
            .ok_or_else(|| invalid(field, "must be a number within float64")),
        other => Err(invalid(
            field,
            format!("must be a number, got {}", other.kind()),
        )),
    }
}

/// Clamp like Go `applyGroups` and record the same warning it logs.
pub(crate) fn warn_clamp(
    warnings: &mut Vec<ConfigWarning>,
    field: String,
    value: f64,
    min: f64,
    max: f64,
) -> f64 {
    let clamped = value.clamp(min, max);
    if clamped != value {
        warnings.push(ConfigWarning::Clamped {
            field,
            value,
            clamped,
        });
    }
    clamped
}

/// Go warns about group keys outside `Fields()` (compared lowercase).
fn warn_unknown_group_fields(
    fields: &JsonObject,
    group: &str,
    known: &[&str],
    warnings: &mut Vec<ConfigWarning>,
) {
    for (key, _) in fields.map_entries() {
        let lower = go_to_lower(key);
        if !known.iter().any(|name| name.to_ascii_lowercase() == lower) {
            warnings.push(ConfigWarning::UnknownField {
                field: format!("{group}.{key}"),
            });
        }
    }
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
            ConfigError::InvalidField { ref field, .. } if field == "fluidEnabled"
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
                    if mentions_runtime_module(trimmed) {
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

    /// `\bruntime::` (any path into the module, including grouped `use
    /// crate::{runtime::..}` and `super::super::runtime::..`) or a bare
    /// `crate::runtime` / `mornlea_server::runtime` import.
    fn mentions_runtime_module(line: &str) -> bool {
        let word_path = line.match_indices("runtime::").any(|(index, _)| {
            line[..index]
                .chars()
                .next_back()
                .is_none_or(|c| !(c.is_alphanumeric() || c == '_'))
        });
        word_path || line.contains("crate::runtime") || line.contains("mornlea_server::runtime")
    }

    #[test]
    fn runtime_import_matcher_catches_grouped_and_relative_paths() {
        assert!(mentions_runtime_module(
            "use crate::{core, runtime::config};"
        ));
        assert!(mentions_runtime_module(
            "use super::super::runtime::RuntimeConfig;"
        ));
        assert!(mentions_runtime_module("use crate::runtime;"));
        assert!(mentions_runtime_module(
            "let c = runtime::RuntimeConfig::defaults();"
        ));
        assert!(!mentions_runtime_module("let tokio_runtime::x = 1;"));
        assert!(!mentions_runtime_module("fn runtime_budget() {}"));
    }

    #[test]
    fn version_must_be_an_integer_literal() {
        for body in [
            r#"{"version":1.0}"#,
            r#"{"version":1e0}"#,
            r#"{"version":"1"}"#,
        ] {
            assert!(
                matches!(
                    RuntimeConfig::decode(body.as_bytes()),
                    Err(ConfigError::InvalidField { ref field, .. }) if field == "version"
                ),
                "{body}"
            );
        }
    }

    #[test]
    fn audio_volume_rounds_the_literal_once_like_go() {
        // Go reads 1.0000001 (one float32 rounding) and refuses it as above 1.
        assert!(matches!(
            RuntimeConfig::decode(br#"{"audioVolume":1.00000005960464477539062500000000001}"#),
            Err(ConfigError::InvalidField { ref field, .. }) if field == "audioVolume"
        ));
        RuntimeConfig::decode(br#"{"audioVolume":1.000000059604644775390625}"#)
            .expect("exact midpoint ties to 1.0 in Go");
    }

    #[test]
    fn negative_zero_integer_literal_decodes_as_zero_in_every_int_field() {
        // cameraMode: Go reads 0, a valid mode, with no warning.
        let cfg = RuntimeConfig::decode(br#"{"cameraMode":-0}"#).expect("cameraMode -0");
        assert!(cfg.warnings().is_empty());
        // version: Go reads 0 and refuses it as an unsupported version.
        assert!(matches!(
            RuntimeConfig::decode(br#"{"version":-0}"#),
            Err(ConfigError::UnsupportedVersion { found: 0 })
        ));
        // ai.taskTimeoutMinutes: Go reads 0 and refuses it by range.
        let ai = |minutes: &str| {
            format!(
                r#"{{"ai":{{"agentService":{{"endpoint":"http://127.0.0.1:8765","apiKeyEnv":"K"}},"companions":[{{"id":"3f2b8c1e-4a5d-4e6f-8a7b-9c0d1e2f3a4b","name":"Mira"}}],"taskTimeoutMinutes":{minutes}}}}}"#
            )
        };
        let err = RuntimeConfig::decode_with_env(ai("-0").as_bytes(), &|name| name == "K")
            .expect_err("taskTimeoutMinutes -0");
        assert!(
            matches!(err, ConfigError::InvalidField { ref field, ref detail }
                if field == "ai.taskTimeoutMinutes" && detail.contains("outside 1..60")),
            "{err}"
        );
        // Zeros with a fraction or exponent stay type errors, as in Go.
        for (body, field) in [
            (r#"{"cameraMode":-0.0}"#.to_owned(), "cameraMode"),
            (r#"{"cameraMode":-0e0}"#.to_owned(), "cameraMode"),
            (r#"{"version":-0e0}"#.to_owned(), "version"),
            (ai("-0.0"), "ai.taskTimeoutMinutes"),
        ] {
            let err = RuntimeConfig::decode_with_env(body.as_bytes(), &|name| name == "K")
                .expect_err(&body);
            assert!(
                matches!(err, ConfigError::InvalidField { field: ref got, ref detail }
                    if got == field && detail.contains("integer literal")),
                "{body}: {err}"
            );
        }
    }

    #[test]
    fn nulls_go_treats_as_absent_are_accepted() {
        for body in [
            "null",
            r#"{"version":null}"#,
            r#"{"fluidEnabled":null}"#,
            r#"{"physics":null,"sim":null,"render":null,"logging":null,"ai":null}"#,
            r#"{"cameraMode":null}"#,
        ] {
            let cfg = RuntimeConfig::decode(body.as_bytes()).expect(body);
            assert!(cfg.fluid_enabled(), "{body}");
            assert_eq!(cfg.version(), CURRENT_VERSION);
        }
        for body in [
            r#"{"audioVolume":null}"#,
            r#"{"windowSize":null}"#,
            r#"{"texturePackPath":null}"#,
        ] {
            assert!(RuntimeConfig::decode(body.as_bytes()).is_err(), "{body}");
        }
    }

    #[test]
    fn null_tunable_is_zero_then_clamped_like_go() {
        let cfg = RuntimeConfig::decode(
            br#"{"physics":{"walkSpeed":null,"stepHeight":null},"sim":{"spawnRadius":null,"regenDelayTicks":null}}"#,
        )
        .expect("null tunables");
        let t = cfg.rule_tunables();
        assert_eq!(t.physics().walk_speed, 0.5);
        assert_eq!(t.physics().step_height, 0.0);
        assert_eq!(cfg.spawn_radius(), 1);
        assert_eq!(t.regen_delay_ticks(), 0);
    }

    #[test]
    fn sim_bounds_match_go_fields() {
        let cfg = RuntimeConfig::decode(
            br#"{"sim":{"spawnRadius":0,"fluidRescanCellsPerTick":2000000,"fluidUpdatesPerTick":70000}}"#,
        )
        .expect("clamped");
        assert_eq!(cfg.spawn_radius(), 1);
        assert_eq!(cfg.fluid_rescan_cells_per_tick(), 1_048_576);
        assert_eq!(cfg.fluid_updates_per_tick(), 65_536);
    }

    #[test]
    fn sim_and_physics_type_errors_are_rejected() {
        for body in [
            r#"{"sim":{"spawnRadius":"8"}}"#,
            r#"{"sim":{"eatingTicks":true}}"#,
            r#"{"sim":{"fluidUpdatesPerTick":[1]}}"#,
            r#"{"sim":[]}"#,
            r#"{"physics":{"walkSpeed":{}}}"#,
            r#"{"physics":"fast"}"#,
        ] {
            assert!(
                matches!(
                    RuntimeConfig::decode(body.as_bytes()),
                    Err(ConfigError::InvalidField { .. })
                ),
                "{body}"
            );
        }
    }

    #[test]
    fn exact_key_wins_over_case_variants() {
        let cfg = RuntimeConfig::decode(br#"{"Version":2,"version":1}"#).expect("exact wins");
        assert_eq!(cfg.version(), 1);
        assert!(RuntimeConfig::decode(br#"{"version":2,"Version":1}"#).is_err());
        assert!(RuntimeConfig::decode(br#"{"sim":{"SpawnRadius":8,"spawnRadius":"x"}}"#).is_err());
        let cfg = RuntimeConfig::decode(br#"{"sim":{"SpawnRadius":"x","spawnRadius":8}}"#)
            .expect("exact wins");
        assert_eq!(cfg.spawn_radius(), 8);
    }

    #[test]
    fn case_only_duplicates_without_exact_key_are_ambiguous() {
        // Go picks one of these at random, so it accepts or rejects this file
        // nondeterministically; Rust always refuses.
        let err = RuntimeConfig::decode(br#"{"sim":{"SpawnRadius":8,"SPAWNRADIUS":"x"}}"#)
            .expect_err("ambiguous");
        match err {
            ConfigError::AmbiguousKey { field, keys } => {
                assert_eq!(field, "sim.spawnRadius");
                assert_eq!(keys.len(), 2);
            }
            other => panic!("expected AmbiguousKey, got {other}"),
        }
        assert!(matches!(
            RuntimeConfig::decode(br#"{"FluidEnabled":true,"FLUIDENABLED":false}"#),
            Err(ConfigError::AmbiguousKey { .. })
        ));
    }

    #[test]
    fn logging_is_parsed_and_kept() {
        let cfg = RuntimeConfig::decode(
            br#"{"logging":{"default":" Debug ","modules":{"net":"warning","sim":"loud"},"Modules":{"store":"error"}}}"#,
        )
        .expect("logging");
        let logging = cfg.logging();
        assert_eq!(logging.default, LogLevel::Debug);
        assert_eq!(logging.default.slog_value(), -4);
        // Repeated `modules` objects merge, as Go decoding into one map does.
        assert_eq!(logging.modules.get("net"), Some(&LogLevel::Warn));
        assert_eq!(logging.modules.get("store"), Some(&LogLevel::Error));
        assert!(!logging.modules.contains_key("sim"));
        assert!(cfg.warnings().contains(&ConfigWarning::UnknownLogLevel {
            field: "logging.modules.sim".into(),
            value: "loud".into(),
        }));
        let reset =
            RuntimeConfig::decode(br#"{"logging":{"modules":{"net":"warn"},"modules":null}}"#)
                .expect("null resets");
        assert!(reset.logging().modules.is_empty());
        assert!(RuntimeConfig::decode(br#"{"logging":{"default":5,"default":"info"}}"#).is_err());
        assert_eq!(
            RuntimeConfig::defaults().logging(),
            &LoggingConfig::default()
        );
    }

    #[test]
    fn client_only_scalars_follow_go_rules() {
        let ok = |body: &str| RuntimeConfig::decode(body.as_bytes()).is_ok();
        assert!(ok(r#"{"audioVolume":1.00000001}"#));
        assert!(!ok(r#"{"audioVolume":1.0000001}"#));
        assert!(!ok(r#"{"audioVolume":"0.5"}"#));
        assert!(ok(r#"{"windowSize":"960x540"}"#));
        assert!(!ok(r#"{"windowSize":"800x600"}"#));
        assert!(ok(&format!(
            r#"{{"texturePackPath":"{}"}}"#,
            "p".repeat(1024)
        )));
        assert!(!ok(&format!(
            r#"{{"texturePackPath":"{}"}}"#,
            "p".repeat(1025)
        )));
        assert!(!ok(r#"{"texturePackPath":"a\nb"}"#));
        assert!(ok(r#"{"cameraMode":7}"#));
        assert!(!ok(r#"{"cameraMode":1.0}"#));
        assert!(!ok(r#"{"cameraMode":"1"}"#));
        assert!(ok(r#"{"render":{"lodEnabled":"yes","lodStep":3}}"#));
        assert!(!ok(r#"{"render":{"lodStep":"4"}}"#));
        assert!(!ok(r#"{"render":{"fovDegrees":true}}"#));
    }

    #[test]
    fn warnings_mirror_go_slog_findings() {
        let cfg = RuntimeConfig::decode(
            br#"{"future":1,"cameraMode":9,
                "physics":{"walkSpeed":99,"futureKnob":1},
                "render":{"lodEnabled":"yes","lodStep":3},
                "ai":{"model":"m","futureKnob":1}}"#,
        )
        .expect("warnings only");
        let warnings = cfg.warnings();
        let has = |want: ConfigWarning| warnings.contains(&want);
        assert!(has(ConfigWarning::UnknownField {
            field: "future".into()
        }));
        assert!(has(ConfigWarning::UnknownField {
            field: "physics.futureKnob".into()
        }));
        assert!(has(ConfigWarning::UnknownField {
            field: "ai.futureKnob".into()
        }));
        assert!(has(ConfigWarning::Clamped {
            field: "physics.walkSpeed".into(),
            value: 99.0,
            clamped: 20.0
        }));
        assert!(has(ConfigWarning::InvalidTypeIgnored {
            field: "render.lodEnabled".into(),
            want: "bool"
        }));
        assert!(has(ConfigWarning::Defaulted {
            field: "render.lodStep".into(),
            value: "3".into()
        }));
        assert!(has(ConfigWarning::Defaulted {
            field: "cameraMode".into(),
            value: "9".into()
        }));
        assert!(has(ConfigWarning::RetiredFieldIgnored {
            field: "ai.model".into()
        }));
        assert!(RuntimeConfig::decode(b"{}").unwrap().warnings().is_empty());
    }

    #[test]
    fn ai_is_judged_only_with_companions() {
        const COMPANION: &str = r#"[{"id":"3f2b8c1e-4a5d-4e6f-8a7b-9c0d1e2f3a4b","name":"Mira"}]"#;
        let with_env = |body: &str, set: bool| {
            RuntimeConfig::decode_with_env(body.as_bytes(), &|name| set && name == "K").is_ok()
        };
        let ai = |service: &str, extra: &str| {
            format!(r#"{{"ai":{{"agentService":{service},"companions":{COMPANION}{extra}}}}}"#)
        };
        let service = r#"{"endpoint":"http://127.0.0.1:8765","apiKeyEnv":"K"}"#;
        assert!(with_env(&ai(service, ""), true));
        assert!(
            !with_env(&ai(service, ""), false),
            "named env var must be non-empty"
        );
        assert!(!with_env(&ai(service, r#","taskTimeoutMinutes":0"#), true));
        assert!(with_env(&ai(service, r#","taskTimeoutMinutes":60"#), true));
        assert!(
            !with_env(&ai(service, r#","endpoint":"x""#), true),
            "retired key"
        );
        assert!(!with_env(
            &ai(r#"{"endpoint":"http://10.0.0.1:1","apiKeyEnv":"K"}"#, ""),
            true
        ));
        assert!(with_env(
            r#"{"ai":{"companions":[],"agentService":7}}"#,
            false
        ));
        assert!(!with_env(
            r#"{"ai":{"companions":[],"COMPANIONS":[]}}"#,
            false
        ));
    }

    /// Fluid budgets stay raw numbers here; the fluid host owns `TickBudget`.
    #[test]
    fn runtime_never_constructs_tick_budget() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src/runtime");
        // Split so this test's own source does not match.
        let needle = ["Tick", "Budget"].concat();
        for entry in walkdir(&root) {
            let text = fs::read_to_string(&entry).unwrap();
            let code: String = text
                .lines()
                .filter(|line| !line.trim_start().starts_with("//"))
                .collect();
            assert!(
                !code.contains(&needle),
                "{} must not construct the fluid tick budget",
                entry.display()
            );
        }
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
