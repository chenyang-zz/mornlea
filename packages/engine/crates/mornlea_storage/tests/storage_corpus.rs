//! Executable storage corpus routes for `mornlea_storage`.
//!
//! Save cases arrive one family at a time. Before the controller integrates a
//! reviewed selection, this suite must refuse a zero-case run rather than
//! claiming an empty route table succeeded.

#[path = "../../../tests/runtime_corpus.rs"]
mod runtime_corpus;

#[path = "storage_corpus/value_digest.rs"]
mod value_digest;

#[path = "storage_corpus/region.rs"]
mod region;

#[path = "storage_corpus/player.rs"]
mod player;

#[path = "storage_corpus/metadata.rs"]
mod metadata;

#[path = "storage_corpus/hostile.rs"]
mod hostile;

#[path = "storage_corpus/passive.rs"]
mod passive;

#[path = "storage_corpus/chunk.rs"]
mod chunk;

use runtime_corpus::{load_cases_for_consumer, CorpusConsumer, FrozenCase, try_load_cases_from_root};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
struct StorageRoute {
    family: &'static str,
    version: &'static str,
    operation: &'static str,
}

/// Closed route table for save corpus execution. Node 1.3 appends the first
/// real `save.region` routes after the controller integrates reviewed assets.
const REGISTERED_STORAGE_ROUTES: &[StorageRoute] = &[
    StorageRoute {
        family: "save.player",
        version: "1",
        operation: "decode",
    },
    StorageRoute {
        family: "save.player",
        version: "2",
        operation: "decode",
    },
    StorageRoute {
        family: "save.player",
        version: "3",
        operation: "decode",
    },
    StorageRoute {
        family: "save.player",
        version: "4",
        operation: "decode",
    },
    StorageRoute {
        family: "save.player",
        version: "5",
        operation: "decode",
    },
    StorageRoute {
        family: "save.player",
        version: "6",
        operation: "decode",
    },
    StorageRoute {
        family: "save.player",
        version: "7",
        operation: "decode",
    },
    StorageRoute {
        family: "save.player",
        version: "8",
        operation: "decode",
    },
    StorageRoute {
        family: "save.player",
        version: "9",
        operation: "decode",
    },
    StorageRoute {
        family: "save.player",
        version: "9",
        operation: "encode",
    },
    StorageRoute {
        family: "save.region",
        version: "1",
        operation: "decode",
    },
    StorageRoute {
        family: "save.region",
        version: "1",
        operation: "encode",
    },
    StorageRoute {
        family: "save.region",
        version: "1",
        operation: "order",
    },
    StorageRoute {
        family: "save.world-metadata",
        version: "1",
        operation: "decode",
    },
    StorageRoute {
        family: "save.world-metadata",
        version: "2",
        operation: "decode",
    },
    StorageRoute {
        family: "save.world-metadata",
        version: "3",
        operation: "decode",
    },
    StorageRoute {
        family: "save.world-metadata",
        version: "4",
        operation: "decode",
    },
    StorageRoute {
        family: "save.world-metadata",
        version: "5",
        operation: "decode",
    },
    StorageRoute {
        family: "save.world-metadata",
        version: "6",
        operation: "decode",
    },
    StorageRoute {
        family: "save.world-metadata",
        version: "6",
        operation: "encode",
    },
    StorageRoute {
        family: "save.hostile",
        version: "1",
        operation: "decode",
    },
    StorageRoute {
        family: "save.hostile",
        version: "2",
        operation: "decode",
    },
    StorageRoute {
        family: "save.hostile",
        version: "2",
        operation: "encode",
    },
    StorageRoute {
        family: "save.passive",
        version: "1",
        operation: "decode",
    },
    StorageRoute {
        family: "save.passive",
        version: "1",
        operation: "encode",
    },
    StorageRoute {
        family: "save.chunk",
        version: "1",
        operation: "decode",
    },
    StorageRoute {
        family: "save.chunk",
        version: "2",
        operation: "decode",
    },
    StorageRoute {
        family: "save.chunk",
        version: "3",
        operation: "decode",
    },
    StorageRoute {
        family: "save.chunk",
        version: "4",
        operation: "decode",
    },
];

fn route_is_registered_for_case(case: &FrozenCase) -> bool {
    REGISTERED_STORAGE_ROUTES.iter().any(|route| {
        route.family == case.family
            && route.version == case.version
            && route.operation == case.operation
    })
}

/// Executes every selected storage case through the registered route table.
fn execute_storage_selection(cases: &[FrozenCase]) {
    if cases.is_empty() {
        panic!("storage corpus selection executed zero cases");
    }
    for case in cases {
        assert_eq!(
            case.consumer,
            CorpusConsumer::Storage,
            "case {} carries the wrong consumer",
            case.id
        );
        assert!(
            !case.operation.is_empty(),
            "case {} names no operation",
            case.id
        );
        assert!(
            route_is_registered_for_case(case),
            "unregistered storage route {}/{}/{} for case {}",
            case.family,
            case.version,
            case.operation,
            case.id
        );
    }
    let region_cases: Vec<FrozenCase> = cases
        .iter()
        .filter(|case| case.family == "save.region")
        .cloned()
        .collect();
    if !region_cases.is_empty() {
        region::execute_region_cases(&region_cases);
    }
    let player_cases: Vec<FrozenCase> = cases
        .iter()
        .filter(|case| case.family == "save.player")
        .cloned()
        .collect();
    if !player_cases.is_empty() {
        player::execute_player_cases(&player_cases);
    }
    let metadata_cases: Vec<FrozenCase> = cases
        .iter()
        .filter(|case| case.family == "save.world-metadata")
        .cloned()
        .collect();
    if !metadata_cases.is_empty() {
        metadata::execute_metadata_cases(&metadata_cases);
    }
    let hostile_cases: Vec<FrozenCase> = cases
        .iter()
        .filter(|case| case.family == "save.hostile")
        .cloned()
        .collect();
    if !hostile_cases.is_empty() {
        hostile::execute_hostile_cases(&hostile_cases);
    }
    let passive_cases: Vec<FrozenCase> = cases
        .iter()
        .filter(|case| case.family == "save.passive")
        .cloned()
        .collect();
    if !passive_cases.is_empty() {
        passive::execute_passive_cases(&passive_cases);
    }
    let chunk_cases: Vec<FrozenCase> = cases
        .iter()
        .filter(|case| case.family == "save.chunk")
        .cloned()
        .collect();
    if !chunk_cases.is_empty() {
        chunk::execute_chunk_cases(&chunk_cases);
    }
    let other = cases
        .iter()
        .filter(|case| {
            case.family != "save.region"
                && case.family != "save.player"
                && case.family != "save.world-metadata"
                && case.family != "save.hostile"
                && case.family != "save.passive"
                && case.family != "save.chunk"
        })
        .count();
    if other > 0 {
        panic!("storage corpus has {other} case(s) on families without a dispatcher");
    }
}

#[test]
fn storage_corpus_consumer_parse_and_as_str_round_trip() {
    assert_eq!(CorpusConsumer::Storage.as_str(), "mornlea_storage");
    assert_eq!(
        CorpusConsumer::parse("mornlea_storage"),
        Some(CorpusConsumer::Storage)
    );
    assert!(CorpusConsumer::parse("mornlea_storage-unknown").is_none());
}

/// Before the controller integrates reviewed save cases, the manifest carries
/// no `mornlea_storage` selection. This must fail the suite outright rather
/// than looking like a passing zero-case run.
#[test]
fn storage_corpus_rejects_empty_selection() {
    let cases = load_cases_for_consumer(CorpusConsumer::Storage);
    execute_storage_selection(&cases);
}

#[test]
fn storage_corpus_rejects_unregistered_route() {
    let case = FrozenCase {
        id: "save.player/9/order/sample".to_string(),
        family: "save.player".to_string(),
        version: "9".to_string(),
        consumer: CorpusConsumer::Storage,
        packet_key: None,
        operation: "order".to_string(),
        arguments: serde_json::json!({}),
        input_format: runtime_corpus::InputFormat::Binary,
        input: vec![],
        input_json: None,
        normalized: serde_json::json!({"kind": "error", "category": "corrupt"}),
        encoded: None,
        category: "corrupt".to_string(),
    };
    let err = std::panic::catch_unwind(|| execute_storage_selection(std::slice::from_ref(&case)));
    assert!(
        err.is_err(),
        "cases on unregistered routes must fail before codec execution"
    );
    let payload = err.unwrap_err();
    let message = payload
        .downcast_ref::<&str>()
        .copied()
        .or_else(|| payload.downcast_ref::<String>().map(String::as_str))
        .unwrap_or("");
    assert!(
        message.contains("unregistered storage route"),
        "expected unregistered-route panic, got: {message}"
    );
}

static TEMP_CORPUS_COUNTER: AtomicU64 = AtomicU64::new(0);

struct TempCorpusDir {
    path: PathBuf,
}

impl TempCorpusDir {
    fn new() -> Self {
        let pid = std::process::id();
        let count = TEMP_CORPUS_COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!("mornlea-storage-corpus-{pid}-{count}"));
        fs::create_dir_all(&path).expect("create temp corpus dir");
        Self { path }
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TempCorpusDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

fn sha256_digest(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    format!("sha256:{:x}", hasher.finalize())
}

#[test]
fn storage_corpus_loader_accepts_storage_consumer_in_manifest() {
    let temp = TempCorpusDir::new();
    let base = temp.path();
    let cases_dir = base.join("testdata/runtime-migration/cases/storage/sample");
    fs::create_dir_all(&cases_dir).expect("create case dir");

    let input_rel = "testdata/runtime-migration/cases/storage/sample/case.input.bin";
    let expected_rel = "testdata/runtime-migration/cases/storage/sample/case.expected.json";
    let input_bytes = vec![0x00];
    let expected_bytes = br#"{"kind":"error","category":"corrupt"}"#;
    fs::write(base.join(input_rel), &input_bytes).expect("write input");
    fs::write(base.join(expected_rel), expected_bytes).expect("write expected");

    let manifest = serde_json::json!({
        "schema_version": 2,
        "source_revision": "b6043f004176055a2e39a98508b662691c3e4ef7",
        "identities": {
            "protocol": 45,
            "chunk_schema": 9,
            "player_schema": 9,
            "world_metadata": 6,
            "companions_ai_schema": 5,
            "hostile_mobs_schema": 2,
            "passive_mobs_schema": 1,
            "engine_abi": 11,
            "region_format": 1,
            "agent_http": "v1",
            "agent_mcp": "v1"
        },
        "families": [{
            "id": "save.player",
            "kind": "save",
            "role": "event",
            "current_version": "9",
            "supported_versions": ["9"],
            "source": "packages/server/storage/player/player_codec.go",
            "eventual_owner": "mornlea_storage",
            "numeric_semantics": "little-endian integers; CRC integrity; exact byte round-trip; no implicit repair",
            "sources": [],
            "cases": ["save.player/9/decode/sample"]
        }],
        "cases": [{
            "id": "save.player/9/decode/sample",
            "family": "save.player",
            "version": "9",
            "operation": "decode",
            "arguments": {"requested_player_id": "0123456789abcdef0123456789abcdef"},
            "input": {
                "path": input_rel,
                "sha256": sha256_digest(&input_bytes)
            },
            "input_format": "binary",
            "expected": {
                "path": expected_rel,
                "sha256": sha256_digest(expected_bytes)
            },
            "checkpoints": ["0"],
            "rust_consumer": "mornlea_storage"
        }]
    });

    let manifest_path = base.join("testdata/runtime-migration/contracts.json");
    fs::create_dir_all(manifest_path.parent().unwrap()).expect("create manifest dir");
    fs::write(
        manifest_path,
        serde_json::to_vec_pretty(&manifest).expect("serialize manifest"),
    )
    .expect("write manifest");

    let cases = try_load_cases_from_root(base, CorpusConsumer::Storage)
        .expect("loader must accept mornlea_storage consumer");
    assert_eq!(cases.len(), 1);
    assert_eq!(cases[0].consumer, CorpusConsumer::Storage);
}

const METADATA_INTEGRATED_CASE_IDS: &[&str] = &[
    "save.world-metadata/1/decode/v1-canonical",
    "save.world-metadata/2/decode/v2-canonical",
    "save.world-metadata/3/decode/v3-canonical",
    "save.world-metadata/4/decode/v4-canonical",
    "save.world-metadata/5/decode/v5-canonical",
    "save.world-metadata/5/decode/wrong-dimension-count",
    "save.world-metadata/6/decode/corrupt-crc",
    "save.world-metadata/6/decode/invalid-difficulty-3",
    "save.world-metadata/6/decode/invalid-version-future",
    "save.world-metadata/6/decode/invalid-version-zero",
    "save.world-metadata/6/decode/trailing-byte",
    "save.world-metadata/6/decode/truncated-record",
    "save.world-metadata/6/decode/v6-weather-255",
    "save.world-metadata/6/decode/wrong-header",
    "save.world-metadata/6/encode/v6-boundary",
];

#[test]
fn metadata_corpus_executes_all_integrated_case_ids() {
    use std::collections::BTreeMap;

    let cases: Vec<FrozenCase> = load_cases_for_consumer(CorpusConsumer::Storage)
        .into_iter()
        .filter(|case| case.family == "save.world-metadata")
        .collect();
    let found: BTreeMap<&str, &FrozenCase> = cases
        .iter()
        .map(|case| (case.id.as_str(), case))
        .collect();
    for id in METADATA_INTEGRATED_CASE_IDS {
        if !found.contains_key(id) {
            panic!("integrated manifest missing metadata case {id}");
        }
    }
    if cases.len() != METADATA_INTEGRATED_CASE_IDS.len() {
        panic!(
            "integrated manifest has {} save.world-metadata cases, want {}",
            cases.len(),
            METADATA_INTEGRATED_CASE_IDS.len()
        );
    }
    metadata::execute_metadata_cases(&cases);
}

const HOSTILE_INTEGRATED_CASE_IDS: &[&str] = &[
    "save.hostile/1/decode/truncated-tail",
    "save.hostile/1/decode/v1-fixture",
    "save.hostile/2/decode/cooldown-20",
    "save.hostile/2/decode/corrupt-absent-target-keeps-id",
    "save.hostile/2/decode/corrupt-attack-cooldown",
    "save.hostile/2/decode/corrupt-bool",
    "save.hostile/2/decode/corrupt-burn-cooldown",
    "save.hostile/2/decode/corrupt-count-payload",
    "save.hostile/2/decode/corrupt-crc",
    "save.hostile/2/decode/corrupt-descending-ids",
    "save.hostile/2/decode/corrupt-dimension",
    "save.hostile/2/decode/corrupt-distant-above",
    "save.hostile/2/decode/corrupt-duplicate-id",
    "save.hostile/2/decode/corrupt-envelope-future",
    "save.hostile/2/decode/corrupt-envelope-zero",
    "save.hostile/2/decode/corrupt-health-above",
    "save.hostile/2/decode/corrupt-health-zero",
    "save.hostile/2/decode/corrupt-hurt-cooldown",
    "save.hostile/2/decode/corrupt-inf-velocity",
    "save.hostile/2/decode/corrupt-kind-above",
    "save.hostile/2/decode/corrupt-magic",
    "save.hostile/2/decode/corrupt-nan-position",
    "save.hostile/2/decode/corrupt-nan-yaw",
    "save.hostile/2/decode/corrupt-payload-length",
    "save.hostile/2/decode/corrupt-revision-zero",
    "save.hostile/2/decode/corrupt-target-bad-variant",
    "save.hostile/2/decode/corrupt-target-bad-version",
    "save.hostile/2/decode/corrupt-target-zero-id",
    "save.hostile/2/decode/corrupt-y-at-top",
    "save.hostile/2/decode/corrupt-y-below",
    "save.hostile/2/decode/corrupt-zero-id",
    "save.hostile/2/decode/count-65",
    "save.hostile/2/decode/empty",
    "save.hostile/2/decode/invalid-version-future",
    "save.hostile/2/decode/invalid-version-zero",
    "save.hostile/2/decode/max-records",
    "save.hostile/2/decode/trailing-byte",
    "save.hostile/2/decode/truncated-header-only",
    "save.hostile/2/decode/truncated-short-record",
    "save.hostile/2/decode/truncated-tail",
    "save.hostile/2/decode/v2-fixture",
    "save.hostile/2/decode/y-max-boundary",
    "save.hostile/2/decode/y-min-boundary",
    "save.hostile/2/encode/capacity-minus-one",
    "save.hostile/2/encode/count-65",
    "save.hostile/2/encode/empty",
    "save.hostile/2/encode/max-records",
    "save.hostile/2/encode/unsorted-canonical",
    "save.hostile/2/encode/v1-fixture-reencode",
    "save.hostile/2/encode/v2-fixture-exact",
];

#[test]
fn hostile_corpus_executes_all_integrated_case_ids() {
    use std::collections::BTreeMap;

    let cases: Vec<FrozenCase> = load_cases_for_consumer(CorpusConsumer::Storage)
        .into_iter()
        .filter(|case| case.family == "save.hostile")
        .collect();
    let found: BTreeMap<&str, &FrozenCase> = cases
        .iter()
        .map(|case| (case.id.as_str(), case))
        .collect();
    for id in HOSTILE_INTEGRATED_CASE_IDS {
        if !found.contains_key(id) {
            panic!("integrated manifest missing hostile case {id}");
        }
    }
    if cases.len() != HOSTILE_INTEGRATED_CASE_IDS.len() {
        panic!(
            "integrated manifest has {} save.hostile cases, want {}",
            cases.len(),
            HOSTILE_INTEGRATED_CASE_IDS.len()
        );
    }
    hostile::execute_hostile_cases(&cases);
}

const PASSIVE_INTEGRATED_CASE_IDS: &[&str] = &[
    "save.passive/1/decode/corrupt-bool",
    "save.passive/1/decode/corrupt-count-payload",
    "save.passive/1/decode/corrupt-crc",
    "save.passive/1/decode/corrupt-descending-ids",
    "save.passive/1/decode/corrupt-dimension",
    "save.passive/1/decode/corrupt-duplicate-id",
    "save.passive/1/decode/corrupt-envelope-future",
    "save.passive/1/decode/corrupt-envelope-zero",
    "save.passive/1/decode/corrupt-health-above",
    "save.passive/1/decode/corrupt-health-zero",
    "save.passive/1/decode/corrupt-inf-velocity",
    "save.passive/1/decode/corrupt-magic",
    "save.passive/1/decode/corrupt-nan-position",
    "save.passive/1/decode/corrupt-nan-yaw",
    "save.passive/1/decode/corrupt-payload-length",
    "save.passive/1/decode/corrupt-reserved-00",
    "save.passive/1/decode/corrupt-reserved-01",
    "save.passive/1/decode/corrupt-reserved-02",
    "save.passive/1/decode/corrupt-reserved-03",
    "save.passive/1/decode/corrupt-reserved-04",
    "save.passive/1/decode/corrupt-reserved-05",
    "save.passive/1/decode/corrupt-reserved-06",
    "save.passive/1/decode/corrupt-reserved-07",
    "save.passive/1/decode/corrupt-reserved-08",
    "save.passive/1/decode/corrupt-reserved-09",
    "save.passive/1/decode/corrupt-reserved-10",
    "save.passive/1/decode/corrupt-reserved-11",
    "save.passive/1/decode/corrupt-reserved-12",
    "save.passive/1/decode/corrupt-reserved-13",
    "save.passive/1/decode/corrupt-reserved-14",
    "save.passive/1/decode/corrupt-reserved-15",
    "save.passive/1/decode/corrupt-reserved-16",
    "save.passive/1/decode/corrupt-reserved-17",
    "save.passive/1/decode/corrupt-reserved-18",
    "save.passive/1/decode/corrupt-reserved-19",
    "save.passive/1/decode/corrupt-reserved-20",
    "save.passive/1/decode/corrupt-reserved-21",
    "save.passive/1/decode/corrupt-reserved-22",
    "save.passive/1/decode/corrupt-reserved-23",
    "save.passive/1/decode/corrupt-reserved-24",
    "save.passive/1/decode/corrupt-reserved-25",
    "save.passive/1/decode/corrupt-reserved-26",
    "save.passive/1/decode/corrupt-reserved-27",
    "save.passive/1/decode/corrupt-reserved-28",
    "save.passive/1/decode/corrupt-reserved-29",
    "save.passive/1/decode/corrupt-revision-zero",
    "save.passive/1/decode/corrupt-y-at-top",
    "save.passive/1/decode/corrupt-y-below",
    "save.passive/1/decode/corrupt-zero-id",
    "save.passive/1/decode/count-33",
    "save.passive/1/decode/empty",
    "save.passive/1/decode/health-max-boundary",
    "save.passive/1/decode/health-min-boundary",
    "save.passive/1/decode/invalid-version-future",
    "save.passive/1/decode/invalid-version-zero",
    "save.passive/1/decode/max-records",
    "save.passive/1/decode/trailing-byte",
    "save.passive/1/decode/truncated-fixture",
    "save.passive/1/decode/truncated-header-only",
    "save.passive/1/decode/truncated-short-record",
    "save.passive/1/decode/truncated-tail",
    "save.passive/1/decode/v1-fixture",
    "save.passive/1/decode/y-max-boundary",
    "save.passive/1/decode/y-min-boundary",
    "save.passive/1/encode/capacity-minus-one",
    "save.passive/1/encode/count-33",
    "save.passive/1/encode/empty",
    "save.passive/1/encode/max-records",
    "save.passive/1/encode/unsorted-canonical",
    "save.passive/1/encode/v1-fixture-exact",
];

#[test]
fn passive_corpus_executes_all_integrated_case_ids() {
    use std::collections::BTreeMap;

    let cases: Vec<FrozenCase> = load_cases_for_consumer(CorpusConsumer::Storage)
        .into_iter()
        .filter(|case| case.family == "save.passive")
        .collect();
    let found: BTreeMap<&str, &FrozenCase> = cases
        .iter()
        .map(|case| (case.id.as_str(), case))
        .collect();
    for id in PASSIVE_INTEGRATED_CASE_IDS {
        if !found.contains_key(id) {
            panic!("integrated manifest missing passive case {id}");
        }
    }
    if cases.len() != PASSIVE_INTEGRATED_CASE_IDS.len() {
        panic!(
            "integrated manifest has {} save.passive cases, want {}",
            cases.len(),
            PASSIVE_INTEGRATED_CASE_IDS.len()
        );
    }
    passive::execute_passive_cases(&cases);
}

const CHUNK_INTEGRATED_CASE_IDS: &[&str] = &[
    "save.chunk/1/decode/truncated-payload",
    "save.chunk/1/decode/v1-fixture",
    "save.chunk/2/decode/corrupt-crc",
    "save.chunk/2/decode/v2-fixture",
    "save.chunk/3/decode/invalid-version-zero",
    "save.chunk/3/decode/v3-fixture",
    "save.chunk/4/decode/invalid-version-future",
    "save.chunk/4/decode/v4-fixture",
];

#[test]
fn chunk_legacy_early_corpus_executes_all_integrated_case_ids() {
    use std::collections::BTreeMap;

    let cases: Vec<FrozenCase> = load_cases_for_consumer(CorpusConsumer::Storage)
        .into_iter()
        .filter(|case| case.family == "save.chunk")
        .collect();
    let found: BTreeMap<&str, &FrozenCase> = cases
        .iter()
        .map(|case| (case.id.as_str(), case))
        .collect();
    for id in CHUNK_INTEGRATED_CASE_IDS {
        if !found.contains_key(id) {
            panic!("integrated manifest missing chunk case {id}");
        }
    }
    if cases.len() != CHUNK_INTEGRATED_CASE_IDS.len() {
        panic!(
            "integrated manifest has {} save.chunk cases, want {}",
            cases.len(),
            CHUNK_INTEGRATED_CASE_IDS.len()
        );
    }
    chunk::execute_chunk_cases(&cases);
}
