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
    let other = cases
        .iter()
        .filter(|case| case.family != "save.region" && case.family != "save.player")
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
