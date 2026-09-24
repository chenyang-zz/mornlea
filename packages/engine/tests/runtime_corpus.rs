//! Shared test-only corpus helpers for offline evidence verification.

use serde::de::{DeserializeSeed, Deserializer, MapAccess, SeqAccess, Visitor};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

pub const MAX_MANIFEST_BYTES: u64 = 4 * 1024 * 1024;
pub const MAX_CASE_JSON_BYTES: u64 = 256 * 1024;
pub const MAX_BINARY_BYTES: u64 = 4 * 1024 * 1024;
pub const MAX_CASES: usize = 8192;

const CLOSED_OPERATIONS: &[&str] = &[
    "decode",
    "encode",
    "migrate",
    "admit",
    "order",
    "kernel",
    "agent-contract",
];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CorpusConsumer {
    Frame,
    Domain,
    ExternalAgentContract,
    ExternalRuntimeAuthority,
    Protocol,
    Storage,
}

impl CorpusConsumer {
    #[allow(dead_code)]
    pub fn as_str(&self) -> &'static str {
        match self {
            CorpusConsumer::Frame => "corpus_frame",
            CorpusConsumer::Domain => "mornlea_domain",
            CorpusConsumer::ExternalAgentContract => "external:agent-contract",
            CorpusConsumer::ExternalRuntimeAuthority => "external:runtime-authority",
            CorpusConsumer::Protocol => "mornlea_protocol",
            CorpusConsumer::Storage => "mornlea_storage",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "corpus_frame" => Some(CorpusConsumer::Frame),
            "mornlea_domain" => Some(CorpusConsumer::Domain),
            "external:agent-contract" => Some(CorpusConsumer::ExternalAgentContract),
            "external:runtime-authority" => Some(CorpusConsumer::ExternalRuntimeAuthority),
            "mornlea_protocol" => Some(CorpusConsumer::Protocol),
            "mornlea_storage" => Some(CorpusConsumer::Storage),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InputFormat {
    Binary,
    Json,
}

impl InputFormat {
    #[allow(dead_code)]
    pub fn as_str(&self) -> &'static str {
        match self {
            InputFormat::Binary => "binary",
            InputFormat::Json => "json",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "binary" => Some(InputFormat::Binary),
            "json" => Some(InputFormat::Json),
            _ => None,
        }
    }
}

#[derive(Debug)]
pub struct CorpusError {
    pub message: String,
}

impl std::fmt::Display for CorpusError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for CorpusError {}

/// Test-only manifest identity, retained separately from production packet types.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct FrozenPacketKey {
    pub direction: String,
    pub state: String,
    pub id: u32,
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct FrozenCase {
    pub id: String,
    pub family: String,
    pub version: String,
    pub consumer: CorpusConsumer,
    pub packet_key: Option<FrozenPacketKey>,
    pub operation: String,
    pub arguments: serde_json::Value,
    pub input_format: InputFormat,
    pub input: Vec<u8>,
    pub input_json: Option<serde_json::Value>,
    pub normalized: serde_json::Value,
    pub encoded: Option<Vec<u8>>,
    pub category: String,
}

fn parse_packet_key(
    case: &serde_json::Value,
    cid: &str,
    family: &str,
) -> Result<Option<FrozenPacketKey>, CorpusError> {
    let required = family.starts_with("protocol.client.") || family.starts_with("protocol.server.");
    let Some(value) = case.get("packet_key") else {
        if required {
            return Err(CorpusError {
                message: format!("case {cid} missing packet_key"),
            });
        }
        return Ok(None);
    };
    if family == "protocol.frame" {
        return Err(CorpusError {
            message: format!("frame case {cid} must not carry packet_key"),
        });
    }
    let object = value.as_object().ok_or_else(|| CorpusError {
        message: format!("case {cid} packet_key must be an object"),
    })?;
    if object.len() != 3
        || !object.contains_key("direction")
        || !object.contains_key("state")
        || !object.contains_key("id")
    {
        return Err(CorpusError {
            message: format!("case {cid} packet_key must contain exactly direction, state, id"),
        });
    }
    let direction = object["direction"].as_str().ok_or_else(|| CorpusError {
        message: format!("case {cid} packet_key direction must be a string"),
    })?;
    let state = object["state"].as_str().ok_or_else(|| CorpusError {
        message: format!("case {cid} packet_key state must be a string"),
    })?;
    let id = object["id"]
        .as_u64()
        .and_then(|id| u32::try_from(id).ok())
        .ok_or_else(|| CorpusError {
            message: format!("case {cid} packet_key id must be a u32"),
        })?;
    Ok(Some(FrozenPacketKey {
        direction: direction.to_string(),
        state: state.to_string(),
        id,
    }))
}

struct StrictValueVisitor;
struct StrictValueSeed;

impl<'de> DeserializeSeed<'de> for StrictValueSeed {
    type Value = serde_json::Value;

    fn deserialize<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_any(StrictValueVisitor)
    }
}

impl<'de> Visitor<'de> for StrictValueVisitor {
    type Value = serde_json::Value;

    fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("any valid JSON value without duplicate keys")
    }

    fn visit_bool<E>(self, v: bool) -> Result<Self::Value, E> {
        Ok(serde_json::Value::Bool(v))
    }

    fn visit_i64<E>(self, v: i64) -> Result<Self::Value, E> {
        Ok(serde_json::Value::Number(v.into()))
    }

    fn visit_u64<E>(self, v: u64) -> Result<Self::Value, E> {
        Ok(serde_json::Value::Number(v.into()))
    }

    fn visit_f64<E>(self, v: f64) -> Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        serde_json::Number::from_f64(v)
            .map(serde_json::Value::Number)
            .ok_or_else(|| serde::de::Error::custom("non-finite float"))
    }

    fn visit_str<E>(self, v: &str) -> Result<Self::Value, E> {
        Ok(serde_json::Value::String(v.to_string()))
    }

    fn visit_string<E>(self, v: String) -> Result<Self::Value, E> {
        Ok(serde_json::Value::String(v))
    }

    fn visit_none<E>(self) -> Result<Self::Value, E> {
        Ok(serde_json::Value::Null)
    }

    fn visit_some<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_any(StrictValueVisitor)
    }

    fn visit_unit<E>(self) -> Result<Self::Value, E> {
        Ok(serde_json::Value::Null)
    }

    fn visit_seq<S>(self, mut access: S) -> Result<Self::Value, S::Error>
    where
        S: SeqAccess<'de>,
    {
        let mut vec = Vec::new();
        while let Some(elem) = access.next_element_seed(StrictValueSeed)? {
            vec.push(elem);
        }
        Ok(serde_json::Value::Array(vec))
    }

    fn visit_map<M>(self, mut access: M) -> Result<Self::Value, M::Error>
    where
        M: MapAccess<'de>,
    {
        let mut map = serde_json::Map::new();
        while let Some(key) = access.next_key::<String>()? {
            if map.contains_key(&key) {
                return Err(serde::de::Error::custom(format!(
                    "duplicate object key \"{key}\""
                )));
            }
            let value = access.next_value_seed(StrictValueSeed)?;
            map.insert(key, value);
        }
        Ok(serde_json::Value::Object(map))
    }
}

pub fn decode_strict_json(bytes: &[u8], context: &str) -> Result<serde_json::Value, CorpusError> {
    let mut de = serde_json::Deserializer::from_slice(bytes);
    let val = StrictValueSeed
        .deserialize(&mut de)
        .map_err(|e| CorpusError {
            message: format!("{context}: {e}"),
        })?;
    de.end().map_err(|e| CorpusError {
        message: format!("{context}: trailing content: {e}"),
    })?;
    Ok(val)
}

#[allow(dead_code)]
pub fn find_repo_root() -> PathBuf {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let mut current = manifest_dir.as_path();
    loop {
        let candidate = current.join("testdata/runtime-migration/contracts.json");
        if candidate.is_file() {
            return current.to_path_buf();
        }
        match current.parent() {
            Some(parent) => current = parent,
            None => panic!(
                "could not find testdata/runtime-migration/contracts.json walking up from {}",
                manifest_dir.display()
            ),
        }
    }
}

pub fn check_relative_path(path_str: &str) -> Result<(), CorpusError> {
    if path_str.is_empty() {
        return Err(CorpusError {
            message: "path cannot be empty".to_string(),
        });
    }
    if path_str.starts_with('/') {
        return Err(CorpusError {
            message: format!("path cannot be absolute: {path_str}"),
        });
    }
    if path_str.contains('\\') {
        return Err(CorpusError {
            message: format!("path must use forward slashes: {path_str}"),
        });
    }
    for component in path_str.split('/') {
        if component.is_empty() {
            return Err(CorpusError {
                message: format!("path contains empty component: {path_str}"),
            });
        }
        if component == ".." {
            return Err(CorpusError {
                message: format!("path cannot contain '..': {path_str}"),
            });
        }
        if component == "." {
            return Err(CorpusError {
                message: format!("path cannot contain '.': {path_str}"),
            });
        }
    }
    Ok(())
}

#[allow(dead_code)]
pub fn validate_relative_path(path_str: &str) {
    if let Err(e) = check_relative_path(path_str) {
        panic!("{}", e.message);
    }
}

fn validate_sha256_format(sha: &str, context: &str) -> Result<(), CorpusError> {
    if sha.len() != 71 || !sha.starts_with("sha256:") {
        return Err(CorpusError {
            message: format!("{context}: invalid sha256 format '{sha}'"),
        });
    }
    let hex_part = &sha[7..];
    if !hex_part
        .chars()
        .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
    {
        return Err(CorpusError {
            message: format!("{context}: invalid sha256 hex digits '{sha}'"),
        });
    }
    Ok(())
}

fn read_and_validate_file(
    canonical_root: &Path,
    rel_path: &str,
    max_bytes: u64,
    context: &str,
) -> Result<(Vec<u8>, String), CorpusError> {
    check_relative_path(rel_path)?;

    let mut current = canonical_root.to_path_buf();
    let components: Vec<&str> = rel_path.split('/').collect();
    for (i, component) in components.iter().enumerate() {
        current.push(component);
        let meta = fs::symlink_metadata(&current).map_err(|e| CorpusError {
            message: format!("{context}: stat {}: {e}", current.display()),
        })?;
        if meta.file_type().is_symlink() {
            return Err(CorpusError {
                message: format!(
                    "{context}: symlink forbidden at component {}",
                    current.display()
                ),
            });
        }
        if i == components.len() - 1 {
            if !meta.file_type().is_file() {
                return Err(CorpusError {
                    message: format!("{context}: must be a regular file: {}", current.display()),
                });
            }
            if meta.len() > max_bytes {
                return Err(CorpusError {
                    message: format!(
                        "{context}: file size {} exceeds budget {}",
                        meta.len(),
                        max_bytes
                    ),
                });
            }
        } else if !meta.file_type().is_dir() {
            return Err(CorpusError {
                message: format!(
                    "{context}: expected directory at component {}",
                    current.display()
                ),
            });
        }
    }

    let canonical_leaf = current.canonicalize().map_err(|e| CorpusError {
        message: format!("{context}: canonicalize leaf {}: {e}", current.display()),
    })?;
    if !canonical_leaf.starts_with(canonical_root) {
        return Err(CorpusError {
            message: format!(
                "{context}: path {} escapes root {}",
                canonical_leaf.display(),
                canonical_root.display()
            ),
        });
    }

    let bytes = fs::read(&canonical_leaf).map_err(|e| CorpusError {
        message: format!("{context}: read {}: {e}", canonical_leaf.display()),
    })?;
    let mut hasher = Sha256::new();
    hasher.update(&bytes);
    let digest = format!("sha256:{:x}", hasher.finalize());
    Ok((bytes, digest))
}

#[allow(dead_code)]
pub fn read_bounded_file(path: &Path, max_bytes: u64) -> (Vec<u8>, String) {
    let meta =
        fs::symlink_metadata(path).unwrap_or_else(|e| panic!("stat {}: {e}", path.display()));
    assert!(
        !meta.file_type().is_symlink(),
        "symlinks forbidden: {}",
        path.display()
    );
    assert!(
        meta.file_type().is_file(),
        "must be a regular file: {}",
        path.display()
    );
    assert!(
        meta.len() <= max_bytes,
        "file {} size {} exceeds budget {}",
        path.display(),
        meta.len(),
        max_bytes
    );
    let bytes = fs::read(path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    let mut hasher = Sha256::new();
    hasher.update(&bytes);
    let digest = format!("sha256:{:x}", hasher.finalize());
    (bytes, digest)
}

struct FamilyMeta {
    supported_versions: HashSet<String>,
    declared_cases: HashSet<String>,
}

fn load_cases_filtered(
    root: &Path,
    target_consumer: Option<CorpusConsumer>,
    target_id: Option<&str>,
) -> Result<Vec<FrozenCase>, CorpusError> {
    let canonical_root = root.canonicalize().map_err(|e| CorpusError {
        message: format!("canonicalize root {}: {e}", root.display()),
    })?;

    let manifest_rel = "testdata/runtime-migration/contracts.json";
    let (manifest_bytes, _manifest_digest) = read_and_validate_file(
        &canonical_root,
        manifest_rel,
        MAX_MANIFEST_BYTES,
        "manifest testdata/runtime-migration/contracts.json",
    )?;

    let manifest = decode_strict_json(
        &manifest_bytes,
        "manifest testdata/runtime-migration/contracts.json",
    )?;

    let schema_version = manifest
        .get("schema_version")
        .and_then(|v| v.as_i64())
        .ok_or_else(|| CorpusError {
            message: "manifest missing integer schema_version".to_string(),
        })?;
    if schema_version != 2 {
        return Err(CorpusError {
            message: format!("contracts.json schema_version must be 2, got {schema_version}"),
        });
    }

    let cases = manifest
        .get("cases")
        .and_then(|v| v.as_array())
        .ok_or_else(|| CorpusError {
            message: "manifest missing cases array".to_string(),
        })?;
    if cases.len() > MAX_CASES {
        return Err(CorpusError {
            message: format!("cases count {} exceeds maximum {}", cases.len(), MAX_CASES),
        });
    }

    let families = manifest
        .get("families")
        .and_then(|v| v.as_array())
        .ok_or_else(|| CorpusError {
            message: "manifest missing families array".to_string(),
        })?;

    let mut family_map: HashMap<String, FamilyMeta> = HashMap::new();
    for f in families {
        let fid = f
            .get("id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| CorpusError {
                message: "family missing string id".to_string(),
            })?;
        if fid.is_empty() {
            return Err(CorpusError {
                message: "family id cannot be empty".to_string(),
            });
        }
        let supported_versions_arr = f
            .get("supported_versions")
            .and_then(|v| v.as_array())
            .ok_or_else(|| CorpusError {
                message: format!("family {fid} missing supported_versions array"),
            })?;
        let mut supported_versions = HashSet::new();
        for v in supported_versions_arr {
            let ver = v.as_str().ok_or_else(|| CorpusError {
                message: format!("family {fid} supported_version must be string"),
            })?;
            if !supported_versions.insert(ver.to_string()) {
                return Err(CorpusError {
                    message: format!("family {fid} contains duplicate supported_version: {ver}"),
                });
            }
        }

        let mut declared_cases = HashSet::new();
        if let Some(cases_val) = f.get("cases") {
            if let Some(family_cases_arr) = cases_val.as_array() {
                for c in family_cases_arr {
                    let cid = c.as_str().ok_or_else(|| CorpusError {
                        message: format!("family {fid} case element must be string"),
                    })?;
                    if !declared_cases.insert(cid.to_string()) {
                        return Err(CorpusError {
                            message: format!("family {fid} contains duplicate case id: {cid}"),
                        });
                    }
                }
            } else if !cases_val.is_null() {
                return Err(CorpusError {
                    message: format!("family {fid} cases must be array or null"),
                });
            }
        }

        if family_map.contains_key(fid) {
            return Err(CorpusError {
                message: format!("duplicate family id: {fid}"),
            });
        }
        family_map.insert(
            fid.to_string(),
            FamilyMeta {
                supported_versions,
                declared_cases,
            },
        );
    }

    let mut seen_case_ids = HashSet::new();
    let mut actual_cases_by_family: HashMap<String, HashSet<String>> = HashMap::new();

    for c in cases {
        let cid = c
            .get("id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| CorpusError {
                message: "case missing string id".to_string(),
            })?;
        if cid.is_empty() {
            return Err(CorpusError {
                message: "case id cannot be empty".to_string(),
            });
        }
        if !seen_case_ids.insert(cid.to_string()) {
            return Err(CorpusError {
                message: format!("duplicate case ID: {cid}"),
            });
        }

        let family = c
            .get("family")
            .and_then(|v| v.as_str())
            .ok_or_else(|| CorpusError {
                message: format!("case {cid} missing string family"),
            })?;
        let fam_meta = family_map.get(family).ok_or_else(|| CorpusError {
            message: format!("case {cid} references unknown family {family}"),
        })?;

        actual_cases_by_family
            .entry(family.to_string())
            .or_default()
            .insert(cid.to_string());

        let version = c
            .get("version")
            .and_then(|v| v.as_str())
            .ok_or_else(|| CorpusError {
                message: format!("case {cid} missing string version"),
            })?;
        if !fam_meta.supported_versions.contains(version) {
            return Err(CorpusError {
                message: format!(
                    "case {cid} version {version} not in family {family} supported_versions"
                ),
            });
        }

        let expected_prefix = format!("{family}/{version}/");
        if !cid.starts_with(&expected_prefix) || cid.len() <= expected_prefix.len() {
            return Err(CorpusError {
                message: format!(
                    "case id {cid} must start with {expected_prefix} followed by non-empty label"
                ),
            });
        }

        let operation = c
            .get("operation")
            .and_then(|v| v.as_str())
            .ok_or_else(|| CorpusError {
                message: format!("case {cid} missing string operation"),
            })?;
        if !CLOSED_OPERATIONS.contains(&operation) {
            return Err(CorpusError {
                message: format!("case {cid} invalid operation {operation}"),
            });
        }

        let checkpoints = c
            .get("checkpoints")
            .and_then(|v| v.as_array())
            .ok_or_else(|| CorpusError {
                message: format!("case {cid} missing checkpoints array"),
            })?;
        if checkpoints.is_empty() {
            return Err(CorpusError {
                message: format!("case {cid} checkpoints cannot be empty"),
            });
        }
        for cp_val in checkpoints {
            let cp_str = cp_val.as_str().ok_or_else(|| CorpusError {
                message: format!("case {cid} checkpoint must be string"),
            })?;
            if cp_str.is_empty() || !cp_str.chars().all(|ch| ch.is_ascii_digit()) {
                return Err(CorpusError {
                    message: format!("case {cid} checkpoint {cp_str} must be decimal u64"),
                });
            }
            if cp_str.parse::<u64>().is_err() {
                return Err(CorpusError {
                    message: format!("case {cid} checkpoint {cp_str} out of u64 range"),
                });
            }
        }

        let input_fmt_str = c
            .get("input_format")
            .and_then(|v| v.as_str())
            .ok_or_else(|| CorpusError {
                message: format!("case {cid} missing string input_format"),
            })?;
        if InputFormat::parse(input_fmt_str).is_none() {
            return Err(CorpusError {
                message: format!(
                    "case {cid} invalid input_format {input_fmt_str} (must be 'binary' or 'json')"
                ),
            });
        }

        let rust_consumer_str =
            c.get("rust_consumer")
                .and_then(|v| v.as_str())
                .ok_or_else(|| CorpusError {
                    message: format!("case {cid} missing string rust_consumer"),
                })?;
        if CorpusConsumer::parse(rust_consumer_str).is_none() {
            return Err(CorpusError {
                message: format!("case {cid} unknown consumer {rust_consumer_str}"),
            });
        }

        // Validate asset relative path lexical form early
        let input_path = c["input"]["path"].as_str().ok_or_else(|| CorpusError {
            message: format!("case {cid} input missing path"),
        })?;
        check_relative_path(input_path)?;
        let input_sha = c["input"]["sha256"].as_str().ok_or_else(|| CorpusError {
            message: format!("case {cid} input missing sha256"),
        })?;
        validate_sha256_format(input_sha, &format!("case {cid} input"))?;

        let expected_path = c["expected"]["path"].as_str().ok_or_else(|| CorpusError {
            message: format!("case {cid} expected missing path"),
        })?;
        check_relative_path(expected_path)?;
        let expected_sha = c["expected"]["sha256"]
            .as_str()
            .ok_or_else(|| CorpusError {
                message: format!("case {cid} expected missing sha256"),
            })?;
        validate_sha256_format(expected_sha, &format!("case {cid} expected"))?;

        if let Some(encoded_obj) = c.get("encoded").filter(|v| !v.is_null()) {
            let enc_path = encoded_obj["path"].as_str().ok_or_else(|| CorpusError {
                message: format!("case {cid} encoded missing path"),
            })?;
            check_relative_path(enc_path)?;
            let enc_sha = encoded_obj["sha256"].as_str().ok_or_else(|| CorpusError {
                message: format!("case {cid} encoded missing sha256"),
            })?;
            validate_sha256_format(enc_sha, &format!("case {cid} encoded"))?;
        }
        parse_packet_key(c, cid, family)?;
    }

    for (fid, fam_meta) in &family_map {
        let actual_cases = actual_cases_by_family.get(fid).cloned().unwrap_or_default();
        if fam_meta.declared_cases != actual_cases {
            return Err(CorpusError {
                message: format!("family {fid} declared cases do not match actual manifest cases"),
            });
        }
    }

    let mut result = Vec::new();
    for c in cases {
        let cid = c["id"].as_str().unwrap();
        let family = c["family"].as_str().unwrap();
        let version = c["version"].as_str().unwrap();
        let operation = c["operation"].as_str().unwrap();
        let rust_consumer_str = c["rust_consumer"].as_str().unwrap();
        let consumer = CorpusConsumer::parse(rust_consumer_str).unwrap();

        if target_consumer.is_some_and(|target_cons| consumer != target_cons) {
            continue;
        }
        if target_id.is_some_and(|tid| cid != tid) {
            continue;
        }

        let arguments = c
            .get("arguments")
            .cloned()
            .unwrap_or(serde_json::Value::Null);

        let input_fmt_str = c["input_format"].as_str().unwrap();
        let input_format = InputFormat::parse(input_fmt_str).unwrap();
        let input_path = c["input"]["path"].as_str().unwrap();
        let input_sha = c["input"]["sha256"].as_str().unwrap();

        if input_format == InputFormat::Json && !input_path.ends_with(".json") {
            return Err(CorpusError {
                message: format!(
                    "case {cid} json input asset must have .json extension: {input_path}"
                ),
            });
        }
        if input_format == InputFormat::Binary && input_path.ends_with(".go") {
            return Err(CorpusError {
                message: format!(
                    "case {cid} binary input asset cannot be a Go source file: {input_path}"
                ),
            });
        }

        let max_input_bytes = if input_format == InputFormat::Json {
            MAX_CASE_JSON_BYTES
        } else {
            MAX_BINARY_BYTES
        };

        let (input_bytes, actual_input_sha) = read_and_validate_file(
            &canonical_root,
            input_path,
            max_input_bytes,
            &format!("case {cid} input {input_path}"),
        )?;
        if input_sha != actual_input_sha {
            return Err(CorpusError {
                message: format!(
                    "case {cid} input sha256 mismatch: expected {input_sha}, got {actual_input_sha}"
                ),
            });
        }

        let input_json = if input_format == InputFormat::Json {
            Some(decode_strict_json(
                &input_bytes,
                &format!("case {cid} input {input_path}"),
            )?)
        } else {
            None
        };

        let expected_path = c["expected"]["path"].as_str().unwrap();
        let expected_sha = c["expected"]["sha256"].as_str().unwrap();

        if !expected_path.ends_with(".json") {
            return Err(CorpusError {
                message: format!(
                    "case {cid} expected asset must have .json extension: {expected_path}"
                ),
            });
        }

        let (expected_bytes, actual_expected_sha) = read_and_validate_file(
            &canonical_root,
            expected_path,
            MAX_CASE_JSON_BYTES,
            &format!("case {cid} expected {expected_path}"),
        )?;
        if expected_sha != actual_expected_sha {
            return Err(CorpusError {
                message: format!(
                    "case {cid} expected sha256 mismatch: expected {expected_sha}, got {actual_expected_sha}"
                ),
            });
        }

        let normalized = decode_strict_json(
            &expected_bytes,
            &format!("case {cid} expected {expected_path}"),
        )?;
        let category = normalized
            .get("category")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string();

        let encoded = if let Some(encoded_obj) = c.get("encoded").filter(|v| !v.is_null()) {
            let enc_path = encoded_obj["path"].as_str().unwrap();
            let enc_sha = encoded_obj["sha256"].as_str().unwrap();
            let (enc_bytes, actual_enc_sha) = read_and_validate_file(
                &canonical_root,
                enc_path,
                MAX_BINARY_BYTES,
                &format!("case {cid} encoded {enc_path}"),
            )?;
            if enc_sha != actual_enc_sha {
                return Err(CorpusError {
                    message: format!(
                        "case {cid} encoded sha256 mismatch: expected {enc_sha}, got {actual_enc_sha}"
                    ),
                });
            }
            Some(enc_bytes)
        } else {
            None
        };

        result.push(FrozenCase {
            id: cid.to_string(),
            family: family.to_string(),
            version: version.to_string(),
            consumer,
            packet_key: parse_packet_key(c, cid, family)?,
            operation: operation.to_string(),
            arguments,
            input_format,
            input: input_bytes,
            input_json,
            normalized,
            encoded,
            category,
        });
    }

    Ok(result)
}

#[allow(dead_code)]
pub fn try_load_cases_from_root(
    root: &Path,
    consumer: CorpusConsumer,
) -> Result<Vec<FrozenCase>, CorpusError> {
    load_cases_filtered(root, Some(consumer), None)
}

#[allow(dead_code)]
pub fn load_cases_for_consumer(consumer: CorpusConsumer) -> Vec<FrozenCase> {
    let root = find_repo_root();
    try_load_cases_from_root(&root, consumer)
        .unwrap_or_else(|e| panic!("load_cases_for_consumer: {}", e.message))
}

#[allow(dead_code)]
pub fn load_case(id: &str) -> FrozenCase {
    let root = find_repo_root();
    let cases = load_cases_filtered(&root, None, Some(id))
        .unwrap_or_else(|e| panic!("load_case({id}): {}", e.message));
    match cases.len() {
        1 => cases.into_iter().next().unwrap(),
        0 => panic!("case '{id}' not found in contracts.json"),
        n => panic!("case '{id}' matched {n} times (expected exactly 1)"),
    }
}

#[allow(dead_code)]
pub fn assert_normalized(case: &FrozenCase, actual: serde_json::Value) {
    assert_eq!(
        case.normalized, actual,
        "case {} normalized JSON mismatch",
        case.id
    );
}

#[allow(dead_code)]
pub fn assert_rejected_unchanged<T: PartialEq + std::fmt::Debug>(
    before: &T,
    after: &T,
    error: bool,
) {
    assert!(error, "expected operation to fail");
    assert_eq!(before, after, "state changed on rejected operation");
}
