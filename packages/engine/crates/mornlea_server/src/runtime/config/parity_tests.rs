//! Shared Go/Rust config parity fixtures.
//!
//! The same files feed Go `decodeConfig` in
//! `packages/shared/config/parity_fixtures_test.go`; see that file for the
//! directory contract. Here `bad/` must be refused, `good/` accepted with the
//! effective values in `expected/` (required for every good fixture), and `known_difference/` refused with the
//! specific error each documented difference produces.

use std::fs;
use std::path::{Path, PathBuf};

use serde_json::Value;

use super::{ConfigError, RuntimeConfig};

const API_KEY_ENV: &str = "MORNLEA_CONFIG_PARITY_API_KEY";

fn fixture_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../../shared/config/testdata/parity")
        .canonicalize()
        .expect("shared parity fixtures")
}

fn fixtures(kind: &str) -> Vec<PathBuf> {
    let mut paths: Vec<PathBuf> = fs::read_dir(fixture_root().join(kind))
        .unwrap_or_else(|err| panic!("read {kind} fixtures: {err}"))
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "json"))
        .collect();
    paths.sort();
    assert!(!paths.is_empty(), "no {kind} fixtures");
    paths
}

fn name(path: &Path) -> String {
    path.file_stem().unwrap().to_string_lossy().into_owned()
}

/// Mirrors the Go test environment: the parity key is set, nothing else is.
fn decode(path: &Path) -> Result<RuntimeConfig, ConfigError> {
    let bytes = fs::read(path).unwrap();
    RuntimeConfig::decode_with_env(&bytes, &|name| name == API_KEY_ENV)
}

#[test]
fn bad_fixtures_are_rejected_like_go() {
    let groups = [
        "syntax",
        "top_level",
        "version",
        "logging",
        "ai",
        "texturePackPath",
        "audioVolume",
        "windowSize",
        "fluidEnabled",
        "cameraMode",
        "physics",
        "sim",
        "render",
    ];
    let paths = fixtures("bad");
    let accepted: Vec<String> = paths
        .iter()
        .filter(|path| decode(path).is_ok())
        .map(|path| name(path))
        .collect();
    assert!(
        accepted.is_empty(),
        "Rust accepted files Go rejects: {accepted:?}"
    );
    for group in groups {
        assert!(
            paths
                .iter()
                .any(|path| name(path).starts_with(&format!("{group}_"))),
            "no bad fixture covers {group}"
        );
    }
}

#[test]
fn good_fixtures_are_accepted_with_go_effective_values() {
    let mut failures = Vec::new();
    for path in fixtures("good") {
        let fixture = name(&path);
        match decode(&path) {
            Err(err) => failures.push(format!("{fixture}: rejected: {err}")),
            Ok(config) => {
                let expected = fixture_root()
                    .join("expected")
                    .join(format!("{fixture}.json"));
                let text = match fs::read_to_string(&expected) {
                    Ok(text) => text,
                    Err(err) => {
                        failures.push(format!(
                            "{fixture}: every good fixture needs {}: {err}",
                            expected.display()
                        ));
                        continue;
                    }
                };
                let want = normalize(serde_json::from_str(&text).unwrap());
                let got = effective_values(&config);
                if got != want {
                    failures.push(format!("{fixture}: values\n got {got}\nwant {want}"));
                }
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn known_differences_are_rejected_by_rust() {
    let mut seen = Vec::new();
    for path in fixtures("known_difference") {
        let fixture = name(&path);
        let err = decode(&path).expect_err(&fixture);
        let ok = match fixture.as_str() {
            "ambiguous_case_only_keys" => matches!(err, ConfigError::AmbiguousKey { .. }),
            "unknown_key_invalid_utf8_bytes"
            | "unknown_key_lone_surrogate_escape"
            | "unknown_key_huge_number"
            | "nesting_at_go_limit"
            | "nesting_over_serde_limit" => matches!(err, ConfigError::Parse(_)),
            other => panic!("undocumented known difference fixture {other}"),
        };
        assert!(ok, "{fixture}: unexpected error {err}");
        seen.push(fixture);
    }
    assert_eq!(seen.len(), 6, "every documented difference keeps a fixture");
}

#[test]
fn fixture_counts_match_go() {
    // Same numbers as `parityFixtureCounts` in the Go test.
    for (kind, want) in [("bad", 111), ("good", 35), ("known_difference", 6)] {
        assert_eq!(fixtures(kind).len(), want, "{kind} fixtures");
    }
}

/// Go's refusal reason for each `-0` fixture, as pinned by `parityBadReasons`
/// in the Go test.
enum NegativeZeroReason {
    UnsupportedVersionZero,
    TaskTimeoutOutOfRange,
    NotAnIntegerLiteral(&'static str),
}

#[test]
fn negative_zero_bad_fixtures_fail_for_go_reasons() {
    use NegativeZeroReason::*;
    let cases = [
        ("version_negative_zero_integer", UnsupportedVersionZero),
        (
            "ai_task_timeout_negative_zero_integer",
            TaskTimeoutOutOfRange,
        ),
        (
            "version_negative_zero_exponent",
            NotAnIntegerLiteral("version"),
        ),
        (
            "cameraMode_negative_zero_exponent_literal",
            NotAnIntegerLiteral("cameraMode"),
        ),
        (
            "cameraMode_negative_zero_fraction",
            NotAnIntegerLiteral("cameraMode"),
        ),
        (
            "ai_task_timeout_negative_zero_float",
            NotAnIntegerLiteral("ai.taskTimeoutMinutes"),
        ),
    ];
    for (fixture, reason) in cases {
        let path = fixture_root().join("bad").join(format!("{fixture}.json"));
        let err = decode(&path).expect_err(fixture);
        let ok = match reason {
            UnsupportedVersionZero => matches!(err, ConfigError::UnsupportedVersion { found: 0 }),
            TaskTimeoutOutOfRange => matches!(&err, ConfigError::InvalidField { field, detail }
                if field == "ai.taskTimeoutMinutes" && detail.contains("outside 1..60")),
            NotAnIntegerLiteral(want) => matches!(&err, ConfigError::InvalidField { field, detail }
                if field == want && detail.contains("integer literal")),
        };
        assert!(ok, "{fixture}: unexpected error {err}");
    }
}

/// The Go test's `parityValues`: server-owned values as float64, keyed by Go
/// field name.
fn effective_values(config: &RuntimeConfig) -> Value {
    let t = config.rule_tunables();
    let p = t.physics();
    let mut map = serde_json::Map::new();
    let mut put = |key: &str, value: f64| {
        map.insert(key.to_owned(), Value::from(value));
    };
    put("physics.eyeHeight", f64::from(t.eye_height()));
    put("physics.stepHeight", f64::from(p.step_height));
    put("physics.walkSpeed", f64::from(p.walk_speed));
    put(
        "physics.groundAcceleration",
        f64::from(p.ground_acceleration),
    );
    put(
        "physics.groundDeceleration",
        f64::from(p.ground_deceleration),
    );
    put("physics.airAcceleration", f64::from(p.air_acceleration));
    put("physics.jumpSpeed", f64::from(p.jump_speed));
    put("physics.gravity", f64::from(p.gravity));
    put(
        "physics.terminalFallSpeed",
        f64::from(p.terminal_fall_speed),
    );
    put("physics.fluidGravity", f64::from(p.fluid_gravity));
    put("physics.fluidSinkSpeed", f64::from(p.fluid_sink_speed));
    put("physics.fluidAscendSpeed", f64::from(p.fluid_ascend_speed));
    put(
        "physics.fluidHorizontalDrag",
        f64::from(p.fluid_horizontal_drag),
    );
    put(
        "physics.sprintSpeedMultiplier",
        f64::from(p.sprint_speed_multiplier),
    );
    put(
        "physics.sneakSpeedMultiplier",
        f64::from(p.sneak_speed_multiplier),
    );
    put("sim.interactionReach", f64::from(t.interaction_reach()));
    put("sim.regenDelayTicks", f64::from(t.regen_delay_ticks()));
    put(
        "sim.regenIntervalTicks",
        f64::from(t.regen_interval_ticks()),
    );
    put(
        "sim.drownDamageIntervalTicks",
        f64::from(t.drown_interval_ticks()),
    );
    put(
        "sim.dropPickupDelayTicks",
        f64::from(t.drop_pickup_delay_ticks()),
    );
    put(
        "sim.playerDropPickupDelayTicks",
        f64::from(t.player_drop_pickup_delay_ticks()),
    );
    put("sim.dropLifetimeTicks", f64::from(t.drop_lifetime_ticks()));
    put("sim.dropPickupRange", f64::from(t.drop_pickup_range()));
    put("sim.spawnRadius", f64::from(config.spawn_radius()));
    put("sim.furnaceSmeltTicks", f64::from(t.furnace_smelt_ticks()));
    put("sim.furnaceBurnTicks", f64::from(t.furnace_burn_ticks()));
    put("sim.fluidFlowDelayTicks", t.fluid_delay() as f64);
    put(
        "sim.fluidUpdatesPerTick",
        f64::from(config.fluid_updates_per_tick()),
    );
    put(
        "sim.fluidRescanCellsPerTick",
        f64::from(config.fluid_rescan_cells_per_tick()),
    );
    put("sim.randomTicksPerSection", f64::from(t.random_attempts()));
    put(
        "sim.cropGrowthChancePercent",
        f64::from(t.crop_growth_percent()),
    );
    put(
        "sim.starvationDamageIntervalTicks",
        f64::from(t.starvation_interval_ticks()),
    );
    put(
        "sim.exhaustionThresholdMilli",
        f64::from(t.exhaustion_threshold_milli()),
    );
    put(
        "sim.regenHungerThreshold",
        f64::from(t.regen_hunger_threshold()),
    );
    put("sim.eatingTicks", f64::from(t.eating_ticks()));
    let logging = config.logging();
    put("logging.default", f64::from(logging.default.slog_value()));
    map.insert("fluidEnabled".into(), Value::Bool(config.fluid_enabled()));
    let modules: serde_json::Map<String, Value> = logging
        .modules
        .iter()
        .map(|(name, level)| (name.clone(), Value::from(f64::from(level.slog_value()))))
        .collect();
    map.insert("logging.modules".into(), Value::Object(modules));
    normalize(Value::Object(map))
}

/// Integral floats compare equal to the Go-written integers in `expected/`.
fn normalize(value: Value) -> Value {
    match value {
        Value::Number(number) => match number.as_f64() {
            Some(f) if f.fract() == 0.0 && f.abs() < 1e15 => Value::from(f as i64),
            _ => Value::Number(number),
        },
        Value::Object(map) => {
            Value::Object(map.into_iter().map(|(k, v)| (k, normalize(v))).collect())
        }
        other => other,
    }
}
