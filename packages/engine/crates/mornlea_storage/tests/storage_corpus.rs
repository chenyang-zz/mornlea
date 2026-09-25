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

#[path = "storage_corpus/companion.rs"]
mod companion;

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
    StorageRoute {
        family: "save.chunk",
        version: "5",
        operation: "decode",
    },
    StorageRoute {
        family: "save.chunk",
        version: "6",
        operation: "decode",
    },
    StorageRoute {
        family: "save.chunk",
        version: "7",
        operation: "decode",
    },
    StorageRoute {
        family: "save.chunk",
        version: "8",
        operation: "decode",
    },
    StorageRoute {
        family: "save.chunk",
        version: "9",
        operation: "decode",
    },
    StorageRoute {
        family: "save.chunk",
        version: "9",
        operation: "encode",
    },
    StorageRoute {
        family: "save.companion",
        version: "1",
        operation: "decode",
    },
    StorageRoute {
        family: "save.companion",
        version: "2",
        operation: "decode",
    },
    StorageRoute {
        family: "save.companion",
        version: "3",
        operation: "decode",
    },
    StorageRoute {
        family: "save.companion",
        version: "4",
        operation: "decode",
    },
    StorageRoute {
        family: "save.companion",
        version: "5",
        operation: "decode",
    },
    StorageRoute {
        family: "save.companion",
        version: "5",
        operation: "encode",
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
    let companion_cases: Vec<FrozenCase> = cases
        .iter()
        .filter(|case| case.family == "save.companion")
        .cloned()
        .collect();
    if !companion_cases.is_empty() {
        companion::execute_companion_cases(&companion_cases);
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
                && case.family != "save.companion"
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

const CHUNK_LEGACY_EARLY_INTEGRATED_CASE_IDS: &[&str] = &[
    "save.chunk/1/decode/truncated-payload",
    "save.chunk/1/decode/v1-fixture",
    "save.chunk/2/decode/corrupt-crc",
    "save.chunk/2/decode/v2-fixture",
    "save.chunk/3/decode/invalid-version-zero",
    "save.chunk/3/decode/v3-fixture",
    "save.chunk/4/decode/invalid-version-future",
    "save.chunk/4/decode/v4-fixture",
];

const CHUNK_LEGACY_LATE_INTEGRATED_CASE_IDS: &[&str] = &[
    "save.chunk/5/decode/corrupt-crc",
    "save.chunk/5/decode/v5-fixture",
    "save.chunk/6/decode/truncated-payload",
    "save.chunk/6/decode/v6-fixture",
    "save.chunk/7/decode/invalid-version-zero",
    "save.chunk/7/decode/v7-fixture",
    "save.chunk/8/decode/invalid-version-future",
    "save.chunk/8/decode/v8-fixture",
    "save.chunk/9/decode/truncated-payload",
    "save.chunk/9/decode/v9-fixture",
];

const CHUNK_CURRENT_INTEGRATED_CASE_IDS: &[&str] = &[
    "save.chunk/9/decode/v9-chest-registry",
    "save.chunk/9/decode/v9-fluid-fixture",
    "save.chunk/9/encode/v9-fixture-exact",
];

const CHUNK_ADVERSARIAL_INTEGRATED_CASE_IDS: &[&str] = &[
    "save.chunk/4/decode/insufficient-drop-slots",
    "save.chunk/9/decode/active-container-mismatch",
    "save.chunk/9/decode/checksum",
    "save.chunk/9/decode/compressed-oversize",
    "save.chunk/9/decode/declared-logical-oversize",
    "save.chunk/9/decode/invalid-version-future",
    "save.chunk/9/decode/invalid-version-zero",
    "save.chunk/9/decode/palette-error",
    "save.chunk/9/decode/trailing-byte",
    "save.chunk/9/decode/truncated-envelope",
    "save.chunk/9/decode/truncated-frame",
    "save.chunk/9/decode/wrong-key",
    "save.chunk/9/decode/wrong-revision",
];

const CHUNK_CROSS_INTEGRATED_CASE_IDS: &[&str] = &["save.chunk/9/decode/rust-v9-frame"];

fn chunk_gate_cases(ids: &[&str], gate: fn(&str) -> bool, label: &str) -> Vec<FrozenCase> {
    use std::collections::BTreeMap;

    let cases: Vec<FrozenCase> = load_cases_for_consumer(CorpusConsumer::Storage)
        .into_iter()
        .filter(|case| case.family == "save.chunk" && gate(&case.id))
        .collect();
    let found: BTreeMap<&str, &FrozenCase> = cases
        .iter()
        .map(|case| (case.id.as_str(), case))
        .collect();
    for id in ids {
        if !found.contains_key(id) {
            panic!("integrated manifest missing {label} chunk case {id}");
        }
    }
    if cases.len() != ids.len() {
        panic!(
            "integrated manifest has {} {label} save.chunk cases, want {}",
            cases.len(),
            ids.len()
        );
    }
    cases
}

#[test]
fn chunk_legacy_early_corpus_executes_all_integrated_case_ids() {
    let cases = chunk_gate_cases(
        CHUNK_LEGACY_EARLY_INTEGRATED_CASE_IDS,
        chunk::chunk_legacy_early_case,
        "legacy early",
    );
    chunk::execute_chunk_cases(&cases);
}

#[test]
fn chunk_legacy_late_corpus_executes_all_integrated_case_ids() {
    let cases = chunk_gate_cases(
        CHUNK_LEGACY_LATE_INTEGRATED_CASE_IDS,
        chunk::chunk_legacy_late_case,
        "legacy late",
    );
    chunk::execute_chunk_cases(&cases);
}

#[test]
fn chunk_current_corpus_executes_all_integrated_case_ids() {
    let cases = chunk_gate_cases(
        CHUNK_CURRENT_INTEGRATED_CASE_IDS,
        chunk::chunk_current_case,
        "current",
    );
    chunk::execute_chunk_cases(&cases);
}

#[test]
fn chunk_adversarial_corpus_executes_all_integrated_case_ids() {
    let cases = chunk_gate_cases(
        CHUNK_ADVERSARIAL_INTEGRATED_CASE_IDS,
        chunk::chunk_adversarial_case,
        "adversarial",
    );
    chunk::execute_chunk_cases(&cases);
}

#[test]
fn chunk_cross_corpus_executes_all_integrated_case_ids() {
    let cases = chunk_gate_cases(
        CHUNK_CROSS_INTEGRATED_CASE_IDS,
        chunk::chunk_cross_case,
        "cross",
    );
    chunk::execute_chunk_cases(&cases);
}

const COMPANION_LEGACY_INTEGRATED_CASE_IDS: &[&str] = &[
    "save.companion/1/decode/truncated-payload",
    "save.companion/1/decode/v1-fixture",
    "save.companion/2/decode/corrupt-crc",
    "save.companion/2/decode/v2-fixture",
    "save.companion/3/decode/invalid-version-zero",
    "save.companion/3/decode/v3-fixture",
    "save.companion/4/decode/invalid-version-future",
    "save.companion/4/decode/v4-fixture",
];

const COMPANION_CURRENT_INTEGRATED_CASE_IDS: &[&str] = &[
    "save.companion/5/decode/v5-fixture",
    "save.companion/5/decode/v5-roundtrip-alt",
    "save.companion/5/decode/max-legal-size",
    "save.companion/5/encode/v5-canonical",
    "save.companion/5/encode/capacity-minus-one",
];

const COMPANION_ADVERSARIAL_INTEGRATED_CASE_IDS: &[&str] = &[
    "save.companion/5/decode/active-count-five",
    "save.companion/5/decode/body-count-65",
    "save.companion/5/decode/command-over-limit",
    "save.companion/5/decode/corrupt-crc",
    "save.companion/5/decode/duplicate-lifecycle",
    "save.companion/5/decode/fifo-over-limit",
    "save.companion/5/decode/inactive-queue",
    "save.companion/5/decode/invalid-version-future",
    "save.companion/5/decode/invalid-version-zero",
    "save.companion/5/decode/malformed-uuid",
    "save.companion/5/decode/missing-lifecycle",
    "save.companion/5/decode/orphan-queue",
    "save.companion/5/decode/plan-steps-over-limit",
    "save.companion/5/decode/summary-over-limit",
    "save.companion/5/decode/trailing-byte",
    "save.companion/5/decode/truncated-header",
];

fn companion_gate_cases(ids: &[&str], gate: fn(&str) -> bool, label: &str) -> Vec<FrozenCase> {
    use std::collections::BTreeMap;

    let cases: Vec<FrozenCase> = load_cases_for_consumer(CorpusConsumer::Storage)
        .into_iter()
        .filter(|case| case.family == "save.companion" && gate(&case.id))
        .collect();
    let found: BTreeMap<&str, &FrozenCase> = cases
        .iter()
        .map(|case| (case.id.as_str(), case))
        .collect();
    for id in ids {
        if !found.contains_key(id) {
            panic!("integrated manifest missing {label} companion case {id}");
        }
    }
    if cases.is_empty() {
        panic!("integrated manifest selected zero save.companion cases");
    }
    if cases.len() != ids.len() {
        panic!(
            "integrated manifest has {} {label} save.companion cases, want {}",
            cases.len(),
            ids.len()
        );
    }
    cases
}

#[test]
fn companion_legacy_corpus_executes_all_integrated_case_ids() {
    let cases = companion_gate_cases(
        COMPANION_LEGACY_INTEGRATED_CASE_IDS,
        companion::companion_legacy_case,
        "legacy",
    );
    companion::execute_companion_cases(&cases);
}

#[test]
fn companion_current_corpus_executes_all_integrated_case_ids() {
    let cases = companion_gate_cases(
        COMPANION_CURRENT_INTEGRATED_CASE_IDS,
        companion::companion_current_case,
        "current",
    );
    companion::execute_companion_cases(&cases);
}

#[test]
fn companion_adversarial_corpus_executes_all_integrated_case_ids() {
    let cases = companion_gate_cases(
        COMPANION_ADVERSARIAL_INTEGRATED_CASE_IDS,
        companion::companion_adversarial_case,
        "adversarial",
    );
    companion::execute_companion_cases(&cases);
}

#[test]
fn companion_corpus_rejects_zero_case_selection() {
    let err = std::panic::catch_unwind(|| companion::execute_companion_cases(&[]));
    assert!(err.is_err(), "zero selected companion cases must fail");
}

#[test]
fn companion_legacy_gate_excludes_v5_integrated_cases() {
    for id in COMPANION_CURRENT_INTEGRATED_CASE_IDS {
        assert!(
            !companion::companion_legacy_case(id),
            "v5 current case {id} must not match the legacy gate"
        );
    }
    for id in COMPANION_ADVERSARIAL_INTEGRATED_CASE_IDS {
        assert!(
            !companion::companion_legacy_case(id),
            "v5 adversarial case {id} must not match the legacy gate"
        );
    }
}

fn execute_storage_family_case(case: &FrozenCase) -> Result<(), String> {
    match case.family.as_str() {
        "save.region" => region::execute_region_case(case),
        "save.player" => player::execute_player_case(case),
        "save.world-metadata" => metadata::execute_metadata_case(case),
        "save.hostile" => hostile::execute_hostile_case(case),
        "save.passive" => passive::execute_passive_case(case),
        "save.chunk" => chunk::execute_chunk_case(case),
        "save.companion" => companion::execute_companion_case(case),
        other => Err(format!("unsupported storage family {other}")),
    }
}

fn successful_decode_cases(cases: &[FrozenCase]) -> Vec<&FrozenCase> {
    cases
        .iter()
        .filter(|case| {
            case.operation == "decode"
                && case.normalized.get("kind").and_then(|value| value.as_str()) == Some("ok")
        })
        .collect()
}

#[test]
fn storage_corpus_stale_value_digest_mutation_fails() {
    let cases = load_cases_for_consumer(CorpusConsumer::Storage);
    let case = successful_decode_cases(&cases)
        .into_iter()
        .find(|case| case.family == "save.player")
        .unwrap_or_else(|| panic!("integrated manifest missing successful save.player decode case"));
    let mut mutated = case.clone();
    let mut normalized = case.normalized.clone();
    let object = normalized.as_object_mut().expect("ok outcome must be an object");
    object.insert(
        "value_sha256".to_string(),
        serde_json::json!("sha256:0000000000000000000000000000000000000000000000000000000000000001"),
    );
    mutated.normalized = normalized;
    let err = execute_storage_family_case(&mutated).expect_err("stale value_sha256 must fail");
    assert!(
        err.contains("value digest mismatch"),
        "unexpected error: {err}"
    );
}

const CROSS_INPUT_SAVE_FAMILIES: &[&str] = &[
    "save.region",
    "save.player",
    "save.world-metadata",
    "save.hostile",
    "save.passive",
    "save.chunk",
    "save.companion",
];

fn region_cross_input_candidate(case: &FrozenCase) -> bool {
    case.id.contains("bank-") && !case.id.contains("superblock")
}

fn discover_cross_input_pair<'a>(
    cases: &'a [FrozenCase],
    family: &str,
) -> Option<(&'a FrozenCase, &'a FrozenCase)> {
    if family == "save.world-metadata" {
        return discover_metadata_cross_input_pair(cases);
    }
    discover_same_route_cross_input_pair(cases, family)
}

fn discover_metadata_cross_input_pair<'a>(
    cases: &'a [FrozenCase],
) -> Option<(&'a FrozenCase, &'a FrozenCase)> {
    let mut ok_cases: Vec<&FrozenCase> = Vec::new();
    for case in successful_decode_cases(cases) {
        if case.family != "save.world-metadata" {
            continue;
        }
        if case
            .normalized
            .get("value_sha256")
            .and_then(|value| value.as_str())
            .is_some()
        {
            ok_cases.push(case);
        }
    }
    for (left_index, left) in ok_cases.iter().enumerate() {
        let left_digest = left
            .normalized
            .get("value_sha256")
            .and_then(|value| value.as_str())?;
        for right in ok_cases.iter().skip(left_index + 1) {
            let right_digest = right
                .normalized
                .get("value_sha256")
                .and_then(|value| value.as_str())?;
            if left_digest != right_digest {
                return Some((left, right));
            }
        }
    }
    None
}

fn discover_same_route_cross_input_pair<'a>(
    cases: &'a [FrozenCase],
    family: &str,
) -> Option<(&'a FrozenCase, &'a FrozenCase)> {
    use std::collections::BTreeMap;

    let mut by_route: BTreeMap<(&str, &str), Vec<&FrozenCase>> = BTreeMap::new();
    for case in successful_decode_cases(cases) {
        if case.family != family {
            continue;
        }
        if family == "save.region" && !region_cross_input_candidate(case) {
            continue;
        }
        by_route
            .entry((case.version.as_str(), case.operation.as_str()))
            .or_default()
            .push(case);
    }
    for group in by_route.values() {
        for (index, left) in group.iter().enumerate() {
            let Some(left_digest) = left
                .normalized
                .get("value_sha256")
                .and_then(|value| value.as_str())
            else {
                continue;
            };
            for right in group.iter().skip(index + 1) {
                let Some(right_digest) = right
                    .normalized
                    .get("value_sha256")
                    .and_then(|value| value.as_str())
                else {
                    continue;
                };
                if left_digest != right_digest {
                    return Some((left, right));
                }
            }
        }
    }
    None
}

#[test]
fn storage_corpus_cross_input_value_digest_mutation_fails_for_every_family() {
    let cases = load_cases_for_consumer(CorpusConsumer::Storage);
    for family in CROSS_INPUT_SAVE_FAMILIES {
        let (case_a, case_b) = discover_cross_input_pair(&cases, family).unwrap_or_else(|| {
            panic!("integrated manifest has no cross-input decode pair for family {family}")
        });
        let mut hybrid = (*case_b).clone();
        hybrid.normalized = case_a.normalized.clone();
        let err = execute_storage_family_case(&hybrid).unwrap_err();
        assert!(
            err.contains("value digest mismatch"),
            "family {family} pair {} vs {}: unexpected error: {err}",
            case_a.id,
            case_b.id
        );
    }
}
