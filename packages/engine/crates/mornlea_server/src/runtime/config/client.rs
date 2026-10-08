//! Groups the Rust server does not run but must judge exactly like Go.
//!
//! A config file rolls back and forth between the Go and Rust servers, so
//! every group Go `decodeConfig` type-checks is checked here with the same
//! accept/reject outcome. `logging` is kept (the host applies it later);
//! `render`, `ai`, `texturePackPath`, `audioVolume` and `windowSize` are
//! validated and dropped. `ai` stays validation-only until companion startup
//! parses it, and persona files are not read here because Go only warns about
//! them.

use std::collections::BTreeMap;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

use super::json::{JsonObject, JsonValue, go_equal_fold, go_to_lower};
use super::{ConfigError, ConfigWarning, go_f64, invalid, lookup, warn_clamp};

/// Mirrors `config.MaxTexturePackPathBytes`.
const MAX_TEXTURE_PACK_PATH_BYTES: usize = 1024;
/// Mirrors `companion.MaxActive`.
const MAX_ACTIVE_COMPANIONS: usize = 4;
/// Mirrors the `companion.ValidateTaskTimeoutMinutes` range.
const TASK_TIMEOUT_MINUTES: std::ops::RangeInclusive<i64> = 1..=60;
/// Mirrors the `WindowSize` presets.
const WINDOW_SIZES: [&str; 3] = ["640x360", "960x540", "1280x720"];
/// Mirrors `knownAIFieldKeys`; anything else under `ai` only warns.
const KNOWN_AI_FIELDS: [&str; 6] = [
    "companions",
    "agentService",
    "endpoint",
    "model",
    "apiKeyEnv",
    "taskTimeoutMinutes",
];
const RETIRED_AI_FIELDS: [&str; 3] = ["endpoint", "model", "apiKeyEnv"];

/// `slog.Level` values Go `logging.ParseLevel` accepts.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LogLevel {
    Debug,
    Info,
    Warn,
    Error,
}

impl LogLevel {
    /// Numeric `slog.Level`, kept so hosts and tests compare with Go directly.
    pub fn slog_value(self) -> i32 {
        match self {
            Self::Debug => -4,
            Self::Info => 0,
            Self::Warn => 4,
            Self::Error => 8,
        }
    }

    /// Go `logging.ParseLevel`: trimmed, case-insensitive name.
    fn parse_go(text: &str) -> Option<Self> {
        match go_to_lower(text.trim()).as_str() {
            "debug" => Some(Self::Debug),
            "info" => Some(Self::Info),
            "warn" | "warning" => Some(Self::Warn),
            "error" => Some(Self::Error),
            _ => None,
        }
    }
}

/// Frozen `logging` group: global level plus per-module overrides.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LoggingConfig {
    pub default: LogLevel,
    pub modules: BTreeMap<String, LogLevel>,
}

impl Default for LoggingConfig {
    /// Go `config.Defaults`: `slog.LevelInfo` with no module overrides.
    fn default() -> Self {
        Self {
            default: LogLevel::Info,
            modules: BTreeMap::new(),
        }
    }
}

/// Go decodes `logging` into a struct, so every key (duplicates included)
/// that folds to `default` or `modules` is type-checked in source order, and
/// repeated `modules` objects merge into one map while `null` resets it.
/// Unknown level names warn and keep the default, like `applyLogging`.
pub(super) fn decode_logging(
    raw: &JsonValue,
    warnings: &mut Vec<ConfigWarning>,
) -> Result<LoggingConfig, ConfigError> {
    let fields = match raw {
        JsonValue::Null => return Ok(LoggingConfig::default()),
        JsonValue::Object(fields) => fields,
        other => {
            return Err(invalid(
                "logging",
                format!("must be an object, got {}", other.kind()),
            ));
        }
    };
    let mut default_text = String::new();
    let mut module_texts: Option<BTreeMap<String, String>> = None;
    for (key, value) in fields.all_entries() {
        if go_equal_fold(key, "default") {
            match value {
                JsonValue::Null => {}
                JsonValue::String(text) => default_text = text.clone(),
                other => {
                    return Err(invalid(
                        "logging.default",
                        format!("must be a string, got {}", other.kind()),
                    ));
                }
            }
        } else if go_equal_fold(key, "modules") {
            match value {
                JsonValue::Null => module_texts = None,
                JsonValue::Object(modules) => {
                    let merged = module_texts.get_or_insert_with(BTreeMap::new);
                    for (name, level) in modules.all_entries() {
                        match level {
                            // A null map element decodes to the zero string.
                            JsonValue::Null => merged.insert(name.to_owned(), String::new()),
                            JsonValue::String(text) => merged.insert(name.to_owned(), text.clone()),
                            other => {
                                return Err(invalid(
                                    format!("logging.modules.{name}"),
                                    format!("must be a string, got {}", other.kind()),
                                ));
                            }
                        };
                    }
                }
                other => {
                    return Err(invalid(
                        "logging.modules",
                        format!("must be an object, got {}", other.kind()),
                    ));
                }
            }
        }
    }

    let mut logging = LoggingConfig::default();
    if !default_text.is_empty() {
        match LogLevel::parse_go(&default_text) {
            Some(level) => logging.default = level,
            None => warnings.push(ConfigWarning::UnknownLogLevel {
                field: "logging.default".into(),
                value: default_text,
            }),
        }
    }
    if let Some(module_texts) = module_texts {
        for (name, text) in module_texts {
            match LogLevel::parse_go(&text) {
                Some(level) => {
                    logging.modules.insert(name, level);
                }
                None => warnings.push(ConfigWarning::UnknownLogLevel {
                    field: format!("logging.modules.{name}"),
                    value: text,
                }),
            }
        }
    }
    Ok(logging)
}

/// `render` keys from `config.Fields()` plus the three LOD keys handled by
/// `applyRenderLOD`.
pub(super) const RENDER_FIELDS: [(&str, f64, f64); 3] = [
    ("viewDistance", 2.0, 64.0),
    ("fovDegrees", 30.0, 110.0),
    ("mouseSensitivity", 0.1, 5.0),
];
pub(super) const RENDER_LOD_FIELDS: [&str; 3] = ["lodEnabled", "lodFarMultiplier", "lodStep"];

/// Go `applyGroups`/`applyRenderLOD` for `render`: numeric keys must decode as
/// numbers (null counts as 0), a non-boolean `lodEnabled` only warns, and an
/// illegal `lodStep` only warns.
pub(super) fn validate_render(
    fields: &JsonObject,
    warnings: &mut Vec<ConfigWarning>,
) -> Result<(), ConfigError> {
    for (name, min, max) in RENDER_FIELDS {
        if let Some(raw) = lookup(fields, name, "render")? {
            let path = format!("render.{name}");
            let value = go_f64(raw, &path)?;
            warn_clamp(warnings, path, value, min, max);
        }
    }
    if let Some(raw) = lookup(fields, "lodEnabled", "render")?
        && !matches!(raw, JsonValue::Bool(_) | JsonValue::Null)
    {
        warnings.push(ConfigWarning::InvalidTypeIgnored {
            field: "render.lodEnabled".into(),
            want: "bool",
        });
    }
    if let Some(raw) = lookup(fields, "lodFarMultiplier", "render")? {
        let value = go_f64(raw, "render.lodFarMultiplier")?;
        warn_clamp(warnings, "render.lodFarMultiplier".into(), value, 2.0, 8.0);
    }
    if let Some(raw) = lookup(fields, "lodStep", "render")? {
        let value = go_f64(raw, "render.lodStep")?;
        if value != 2.0 && value != 4.0 && value != 8.0 {
            warnings.push(ConfigWarning::Defaulted {
                field: "render.lodStep".into(),
                value: value.to_string(),
            });
        }
    }
    Ok(())
}

pub(super) fn validate_texture_pack_path(raw: &JsonValue) -> Result<(), ConfigError> {
    let JsonValue::String(path) = raw else {
        return Err(invalid(
            "texturePackPath",
            format!("must be a string, got {}", raw.kind()),
        ));
    };
    if path.len() > MAX_TEXTURE_PACK_PATH_BYTES {
        return Err(invalid(
            "texturePackPath",
            format!(
                "{} UTF-8 bytes exceed {MAX_TEXTURE_PACK_PATH_BYTES}",
                path.len()
            ),
        ));
    }
    if path.contains(['\r', '\n']) {
        return Err(invalid("texturePackPath", "must be a single line"));
    }
    Ok(())
}

/// Go decodes into `*float32`: null is refused, and the range check runs on
/// the rounded `float32`, so `1.00000001` is accepted as exactly 1.
pub(super) fn validate_audio_volume(raw: &JsonValue) -> Result<(), ConfigError> {
    let volume = match raw {
        JsonValue::Number(number) => number.as_go_f32(),
        _ => None,
    };
    match volume {
        Some(volume) if (0.0..=1.0).contains(&volume) => Ok(()),
        Some(volume) => Err(invalid("audioVolume", format!("{volume} is outside 0..1"))),
        None => Err(invalid("audioVolume", "must be a float32 number in 0..1")),
    }
}

pub(super) fn validate_window_size(raw: &JsonValue) -> Result<(), ConfigError> {
    match raw {
        JsonValue::String(size) if WINDOW_SIZES.contains(&size.as_str()) => Ok(()),
        JsonValue::String(size) => Err(invalid(
            "windowSize",
            format!("unsupported preset {size:?}"),
        )),
        other => Err(invalid(
            "windowSize",
            format!("must be a preset string, got {}", other.kind()),
        )),
    }
}

/// Go `applyAI`. The group is only judged in depth when `companions` is a
/// non-empty array; otherwise everything but the object shape and key
/// collisions is ignored, retired keys included.
pub(super) fn validate_ai(
    raw: &JsonValue,
    env_is_set: &dyn Fn(&str) -> bool,
    warnings: &mut Vec<ConfigWarning>,
) -> Result<(), ConfigError> {
    let empty = JsonObject::default();
    let fields = match raw {
        JsonValue::Null => &empty,
        JsonValue::Object(fields) => fields,
        other => {
            return Err(invalid(
                "ai",
                format!("must be an object, got {}", other.kind()),
            ));
        }
    };
    reject_case_fold_collisions(fields, "ai")?;
    for (key, _) in fields.map_entries() {
        if !KNOWN_AI_FIELDS
            .iter()
            .any(|known| go_equal_fold(key, known))
        {
            warnings.push(ConfigWarning::UnknownField {
                field: format!("ai.{key}"),
            });
        }
    }
    let mut retired = Vec::new();
    for key in RETIRED_AI_FIELDS {
        if lookup(fields, key, "ai")?.is_some() {
            retired.push(format!("ai.{key}"));
        }
    }
    let warn_retired = |warnings: &mut Vec<ConfigWarning>| {
        for field in &retired {
            warnings.push(ConfigWarning::RetiredFieldIgnored {
                field: field.clone(),
            });
        }
    };
    let entries = match lookup(fields, "companions", "ai")? {
        None | Some(JsonValue::Null) => {
            warn_retired(warnings);
            return Ok(());
        }
        Some(JsonValue::Array(entries)) => entries,
        Some(other) => {
            return Err(invalid(
                "ai.companions",
                format!("must be an array, got {}", other.kind()),
            ));
        }
    };
    if entries.is_empty() {
        warn_retired(warnings);
        return Ok(());
    }

    let mut endpoint = String::new();
    let mut api_key_env = String::new();
    if let Some(raw) = lookup(fields, "agentService", "ai")? {
        let service = match raw {
            JsonValue::Null => &empty,
            JsonValue::Object(service) => service,
            other => {
                return Err(invalid(
                    "ai.agentService",
                    format!("must be an object, got {}", other.kind()),
                ));
            }
        };
        reject_case_fold_collisions(service, "ai.agentService")?;
        for (key, _) in service.map_entries() {
            if !go_equal_fold(key, "endpoint") && !go_equal_fold(key, "apiKeyEnv") {
                warnings.push(ConfigWarning::UnknownField {
                    field: format!("ai.agentService.{key}"),
                });
            }
        }
        if let Some(raw) = lookup(service, "endpoint", "ai.agentService")? {
            go_string_into(raw, "ai.agentService.endpoint", &mut endpoint)?;
        }
        if let Some(raw) = lookup(service, "apiKeyEnv", "ai.agentService")? {
            go_string_into(raw, "ai.agentService.apiKeyEnv", &mut api_key_env)?;
        }
    }
    if let Some(raw) = lookup(fields, "taskTimeoutMinutes", "ai")? {
        // Null leaves the zero value, which the range check then refuses.
        let minutes = super::go_int(raw, "ai.taskTimeoutMinutes")?.unwrap_or(0);
        if !TASK_TIMEOUT_MINUTES.contains(&minutes) {
            return Err(invalid(
                "ai.taskTimeoutMinutes",
                format!("{minutes} is outside 1..60"),
            ));
        }
    }

    let mut definitions = Vec::with_capacity(entries.len());
    for (index, entry) in entries.iter().enumerate() {
        let path = format!("ai.companions[{index}]");
        let entry_fields = match entry {
            JsonValue::Null => &empty,
            JsonValue::Object(entry_fields) => entry_fields,
            other => {
                return Err(invalid(
                    path,
                    format!("must be an object, got {}", other.kind()),
                ));
            }
        };
        reject_case_fold_collisions(entry_fields, &path)?;
        for (key, _) in entry_fields.map_entries() {
            if !["id", "name", "persona"]
                .iter()
                .any(|known| go_equal_fold(key, known))
            {
                warnings.push(ConfigWarning::UnknownField {
                    field: format!("{path}.{key}"),
                });
            }
        }
        // `companion.ID` is a `TextUnmarshaler`: strings must parse, null
        // leaves the zero ID, and any other JSON kind is a type error.
        let mut id = None;
        if let Some(raw) = lookup(entry_fields, "id", &path)? {
            match raw {
                JsonValue::Null => {}
                JsonValue::String(text) => match parse_uuid_v4(text) {
                    Some(parsed) => id = Some(parsed),
                    None => {
                        return Err(invalid(
                            format!("{path}.id"),
                            "must be lowercase UUIDv4 text",
                        ));
                    }
                },
                other => {
                    return Err(invalid(
                        format!("{path}.id"),
                        format!("must be a string, got {}", other.kind()),
                    ));
                }
            }
        }
        let mut name = String::new();
        if let Some(raw) = lookup(entry_fields, "name", &path)? {
            go_string_into(raw, &format!("{path}.name"), &mut name)?;
        }
        let mut persona = String::new();
        if let Some(raw) = lookup(entry_fields, "persona", &path)? {
            go_string_into(raw, &format!("{path}.persona"), &mut persona)?;
        }
        definitions.push((id, name));
    }

    // `companion.ValidateDefinitions`.
    if definitions.len() > MAX_ACTIVE_COMPANIONS {
        return Err(invalid(
            "ai.companions",
            format!(
                "{} definitions exceed {MAX_ACTIVE_COMPANIONS}",
                definitions.len()
            ),
        ));
    }
    let mut seen_ids = Vec::new();
    let mut seen_names: Vec<&str> = Vec::new();
    for (index, (id, name)) in definitions.iter().enumerate() {
        let Some(id) = id else {
            return Err(invalid(format!("ai.companions[{index}].id"), "invalid"));
        };
        if seen_ids.contains(id) {
            return Err(invalid(format!("ai.companions[{index}].id"), "duplicate"));
        }
        if !valid_companion_name(name) {
            return Err(invalid(
                format!("ai.companions[{index}].name"),
                "must be canonical, 1..32 runes, at most 128 bytes, no whitespace or control",
            ));
        }
        if seen_names.contains(&name.as_str()) {
            return Err(invalid(format!("ai.companions[{index}].name"), "duplicate"));
        }
        seen_ids.push(*id);
        seen_names.push(name);
    }
    if !retired.is_empty() {
        return Err(invalid(
            "ai",
            format!(
                "{} retired; move the Agent address and credentials to ai.agentService",
                retired.join(", ")
            ),
        ));
    }
    if !valid_agent_endpoint(&endpoint) {
        return Err(invalid(
            "ai.agentService.endpoint",
            "must be an http URL on a loopback IP literal without userinfo, query or fragment",
        ));
    }
    if api_key_env.is_empty() {
        return Err(invalid("ai.agentService.apiKeyEnv", "missing"));
    }
    if !env_is_set(&api_key_env) {
        return Err(invalid(
            "ai.agentService.apiKeyEnv",
            "names an empty environment variable",
        ));
    }
    Ok(())
}

/// Go `json.Unmarshal` into `string`: null keeps the previous value.
fn go_string_into(raw: &JsonValue, field: &str, slot: &mut String) -> Result<(), ConfigError> {
    match raw {
        JsonValue::Null => Ok(()),
        JsonValue::String(text) => {
            slot.clone_from(text);
            Ok(())
        }
        other => Err(invalid(
            field,
            format!("must be a string, got {}", other.kind()),
        )),
    }
}

/// Go `rejectCaseFoldCollisions`: distinct map keys whose `strings.ToLower`
/// forms collide are refused, unknown keys included.
fn reject_case_fold_collisions(fields: &JsonObject, path: &str) -> Result<(), ConfigError> {
    let mut seen: BTreeMap<String, &str> = BTreeMap::new();
    for (key, _) in fields.map_entries() {
        if let Some(existing) = seen.insert(go_to_lower(key), key) {
            return Err(invalid(
                path,
                format!("keys {existing:?} and {key:?} collide by case"),
            ));
        }
    }
    Ok(())
}

/// Go `core.ParsePlayerID` plus `Valid`: canonical lowercase UUIDv4 text.
fn parse_uuid_v4(text: &str) -> Option<[u8; 16]> {
    let bytes = text.as_bytes();
    if bytes.len() != 36 {
        return None;
    }
    let mut id = [0_u8; 16];
    let mut nibbles = 0;
    for (index, &byte) in bytes.iter().enumerate() {
        if matches!(index, 8 | 13 | 18 | 23) {
            if byte != b'-' {
                return None;
            }
            continue;
        }
        let nibble = match byte {
            b'0'..=b'9' => byte - b'0',
            b'a'..=b'f' => byte - b'a' + 10,
            _ => return None,
        };
        id[nibbles / 2] |= nibble << if nibbles % 2 == 0 { 4 } else { 0 };
        nibbles += 1;
    }
    let valid = id != [0; 16] && id[6] >> 4 == 4 && id[8] & 0xc0 == 0x80;
    valid.then_some(id)
}

/// Go `companion.ValidateName`: the name must equal its trimmed form, so with
/// the whitespace ban it reduces to no whitespace, no control characters,
/// 1..32 runes and at most 128 bytes.
fn valid_companion_name(name: &str) -> bool {
    let runes = name.chars().count();
    (1..=32).contains(&runes)
        && name.len() <= 128
        && !name.chars().any(|c| c.is_whitespace() || c.is_control())
}

/// Go `AgentServiceSettings.Validate` endpoint rules over Go 1.26 `url.Parse`
/// (strict colons): scheme `http` in any case, no userinfo, empty query and
/// fragment, valid path escapes, optional port in 1..65535, and a loopback IP
/// literal host (IPv4-mapped loopback included, zones excluded).
pub(super) fn valid_agent_endpoint(endpoint: &str) -> bool {
    if endpoint.is_empty() {
        return false;
    }
    let (url, fragment) = endpoint.split_once('#').unwrap_or((endpoint, ""));
    if !fragment.is_empty() || url.bytes().any(|byte| byte < 0x20 || byte == 0x7f) {
        return false;
    }
    let Some((scheme, mut rest)) = url.split_once(':') else {
        return false;
    };
    if !scheme.eq_ignore_ascii_case("http") {
        return false;
    }
    if rest.ends_with('?') && rest.matches('?').count() == 1 {
        rest = &rest[..rest.len() - 1];
    } else if let Some((before, query)) = rest.split_once('?') {
        if !query.is_empty() {
            return false;
        }
        rest = before;
    }
    let Some(after) = rest.strip_prefix("//") else {
        return false;
    };
    let (authority, path) = after.find('/').map_or((after, ""), |i| after.split_at(i));
    if authority.contains('@') || !valid_path_escapes(path) {
        return false;
    }
    let (ip, port) = if let Some(open) = authority.rfind('[') {
        // Go keeps only the bracketed part: text before `[` is dropped.
        let Some(close) = authority.rfind(']') else {
            return false;
        };
        let colon_port = &authority[close + 1..];
        if !valid_optional_port(colon_port) || close < open {
            return false;
        }
        let inner = &authority[open + 1..close];
        if inner.contains('%') {
            return false;
        }
        let Ok(ip) = inner.parse::<Ipv6Addr>() else {
            return false;
        };
        (IpAddr::V6(ip), colon_port.get(1..).unwrap_or(""))
    } else {
        let (host, colon_port) = authority
            .find(':')
            .map_or((authority, ""), |i| authority.split_at(i));
        if !valid_optional_port(colon_port) || host.contains('%') {
            return false;
        }
        let Ok(ip) = host.parse::<Ipv4Addr>() else {
            return false;
        };
        (IpAddr::V4(ip), colon_port.get(1..).unwrap_or(""))
    };
    if !port.is_empty() && !matches!(port.parse::<u16>(), Ok(value) if value != 0) {
        return false;
    }
    match ip {
        IpAddr::V4(v4) => v4.is_loopback(),
        IpAddr::V6(v6) => {
            v6.is_loopback() || v6.to_ipv4_mapped().is_some_and(|v4| v4.is_loopback())
        }
    }
}

fn valid_optional_port(colon_port: &str) -> bool {
    colon_port.is_empty()
        || colon_port
            .strip_prefix(':')
            .is_some_and(|digits| digits.bytes().all(|byte| byte.is_ascii_digit()))
}

fn valid_path_escapes(path: &str) -> bool {
    let bytes = path.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            let hex = |offset: usize| bytes.get(index + offset).is_some_and(u8::is_ascii_hexdigit);
            if !hex(1) || !hex(2) {
                return false;
            }
            index += 3;
        } else {
            index += 1;
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn endpoint_rules_follow_go_url_parse() {
        for ok in [
            "http://127.0.0.1:8765",
            "HTTP://[::1]:08765/agent?",
            "http://[::ffff:127.0.0.2]/#",
            "http://127.0.0.1:/",
            "http://x[::1]:80",
            "http://127.9.9.9",
        ] {
            assert!(valid_agent_endpoint(ok), "{ok}");
        }
        for bad in [
            "",
            "https://127.0.0.1",
            "http://localhost:1",
            "http://192.168.0.1:1",
            "http://user@127.0.0.1",
            "http://127.0.0.1/?a",
            "http://127.0.0.1/#a",
            "http://127.0.0.1:0",
            "http://127.0.0.1:65536",
            "http://127.0.0.1/%zz",
            "http://::1:80",
            "http://[127.0.0.1]:80",
            "http://[::1%25lo]:80",
            "http://127.0.0.01",
            "http:127.0.0.1",
            "http:/127.0.0.1",
            "http://127.0.0.1\u{7f}",
        ] {
            assert!(!valid_agent_endpoint(bad), "{bad}");
        }
    }

    #[test]
    fn uuid_and_name_rules_follow_go() {
        assert!(parse_uuid_v4("3f2b8c1e-4a5d-4e6f-8a7b-9c0d1e2f3a4b").is_some());
        assert!(parse_uuid_v4("3F2B8C1E-4A5D-4E6F-8A7B-9C0D1E2F3A4B").is_none());
        assert!(parse_uuid_v4("3f2b8c1e-4a5d-1e6f-8a7b-9c0d1e2f3a4b").is_none());
        assert!(parse_uuid_v4("3f2b8c1e-4a5d-4e6f-0a7b-9c0d1e2f3a4b").is_none());
        assert!(valid_companion_name("Mira"));
        assert!(valid_companion_name(&"界".repeat(32)));
        assert!(!valid_companion_name(&"界".repeat(43)));
        assert!(!valid_companion_name("Mira Lee"));
        assert!(!valid_companion_name("Mira\u{a0}"));
        assert!(!valid_companion_name(""));
    }

    #[test]
    fn log_levels_parse_like_go() {
        assert_eq!(LogLevel::parse_go(" WARNING "), Some(LogLevel::Warn));
        assert_eq!(LogLevel::parse_go("\u{130}nfo"), Some(LogLevel::Info));
        assert_eq!(LogLevel::parse_go("trace"), None);
    }
}
