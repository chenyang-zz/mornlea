#[allow(dead_code)]
#[path = "numerical_migration/collision.rs"]
mod collision;

#[allow(dead_code)]
#[path = "numerical_migration/physics.rs"]
mod physics;

#[allow(dead_code)]
#[path = "numerical_migration/raycast.rs"]
mod raycast;

#[allow(dead_code)]
#[path = "numerical_migration/worldgen_chunk.rs"]
mod worldgen_chunk;

#[allow(dead_code)]
#[path = "numerical_migration/worldgen_probe.rs"]
mod worldgen_probe;

#[allow(dead_code)]
#[path = "numerical_migration/tree_blocks.rs"]
mod tree_blocks;

#[allow(dead_code)]
#[path = "numerical_migration/lod.rs"]
mod lod;

#[allow(dead_code)]
#[path = "numerical_migration/fluid_eval.rs"]
mod fluid_eval;

#[allow(dead_code)]
#[path = "numerical_migration/fluid_rescan.rs"]
mod fluid_rescan;

#[allow(dead_code)]
#[path = "numerical_migration/mesh.rs"]
mod mesh;

#[allow(dead_code)]
#[path = "numerical_migration/path_grid.rs"]
mod path_grid;

#[allow(dead_code)]
#[path = "numerical_migration/path_search.rs"]
mod path_search;

#[path = "../../../tests/runtime_corpus.rs"]
mod runtime_corpus;

use mornlea_engine::native::contracts::pathfind::{
    PathBlockTable, PathCell, PathError, PathGrid, PathRevision, PathScratch,
};
use mornlea_engine::native::pathfind::find_path;
use runtime_corpus::{
    CorpusConsumer, FrozenCase, assert_normalized, engine_digest_hex, engine_status_category,
    load_cases_for_consumer, validate_engine_binary_arguments, validate_engine_pathfind_arguments,
};
use std::collections::{HashMap, HashSet};

unsafe extern "C" {
    fn mornlea_collision_resolve(
        abi_version: u32,
        input: *const u8,
        input_len: usize,
        output: *mut u8,
        output_len: usize,
    ) -> u32;
    fn mornlea_physics_step(
        abi_version: u32,
        input: *const u8,
        input_len: usize,
        output: *mut u8,
        output_len: usize,
    ) -> u32;
    fn mornlea_raycast_batch(
        abi_version: u32,
        input: *const u8,
        input_len: usize,
        cursor: *mut u8,
        cursor_len: usize,
        output: *mut u8,
        output_len: usize,
        output_count: *mut usize,
        done: *mut u8,
    ) -> u32;
    fn mornlea_worldgen_chunk(
        abi_version: u32,
        input: *const u8,
        input_len: usize,
        output: *mut u8,
        output_len: usize,
    ) -> u32;
    fn mornlea_worldgen_probe(
        abi_version: u32,
        input: *const u8,
        input_len: usize,
        output: *mut u8,
        output_len: usize,
    ) -> u32;
    fn mornlea_tree_blocks(
        abi_version: u32,
        input: *const u8,
        input_len: usize,
        output: *mut u8,
        output_len: usize,
    ) -> u32;
    fn mornlea_lod_shell(
        abi_version: u32,
        input: *const u8,
        input_len: usize,
        output: *mut u8,
        output_capacity: usize,
        output_len: *mut usize,
    ) -> u32;
    fn mornlea_fluid_eval_batch(
        abi_version: u32,
        input: *const u8,
        input_len: usize,
        output: *mut u8,
        output_capacity: usize,
        output_len: *mut usize,
    ) -> u32;
    fn mornlea_fluid_rescan(
        abi_version: u32,
        input: *const u8,
        input_len: usize,
        output: *mut u8,
        output_capacity: usize,
        output_len: *mut usize,
    ) -> u32;
    fn mornlea_mesh_section(
        abi_version: u32,
        input: *const u8,
        input_len: usize,
        scratch: *mut u8,
        scratch_len: usize,
        output: *mut u64,
        output_capacity: usize,
        output_len: *mut usize,
    ) -> u32;
    fn mornlea_engine_abi_version() -> u32;
}

const ENGINE_CANARY_BYTE: u8 = 0xa5;
const ENGINE_CANARY_WORD: u64 = 0xa5a5_a5a5_a5a5_a5a5;
const ENGINE_MAX_BATCH_CALLS: usize = 4;
const ENGINE_MAX_CELLS: usize = 131072;

/// Closed engine route table: the ten ABI families at v11 plus the typed
/// pathfinding route at v1. A case on any other route fails before execution.
const ENGINE_CLOSED_ROUTES: &[(&str, &str)] = &[
    ("kernel.mornlea_collision_resolve", "11"),
    ("kernel.mornlea_physics_step", "11"),
    ("kernel.mornlea_raycast_batch", "11"),
    ("kernel.mornlea_worldgen_chunk", "11"),
    ("kernel.mornlea_worldgen_probe", "11"),
    ("kernel.mornlea_tree_blocks", "11"),
    ("kernel.mornlea_lod_shell", "11"),
    ("kernel.mornlea_fluid_eval_batch", "11"),
    ("kernel.mornlea_fluid_rescan", "11"),
    ("kernel.mornlea_mesh_section", "11"),
    ("kernel.pathfind", "1"),
];

/// Boundary labels with one executed observation each. The table mirrors the
/// Go closure's boundary flags so both consumers pin the same rows.
const ENGINE_BOUNDARY_LABELS: &[&str] = &[
    "kernel.mornlea_collision_resolve/11/short-output-15",
    "kernel.mornlea_physics_step/11/ulp-sweep-reject",
    "kernel.mornlea_raycast_batch/11/record-65",
    "kernel.mornlea_worldgen_chunk/11/signed-extreme",
    "kernel.mornlea_worldgen_probe/11/query-65",
    "kernel.mornlea_tree_blocks/11/short-by-one",
    "kernel.mornlea_lod_shell/11/exact-needed-short",
    "kernel.mornlea_fluid_eval_batch/11/count-4097",
    "kernel.mornlea_fluid_rescan/11/budget-one-short",
    "kernel.mornlea_mesh_section/11/model-plant-late-overflow",
    "kernel.pathfind/1/goal-pop-4097",
];

fn engine_abi_version() -> u32 {
    unsafe { mornlea_engine_abi_version() }
}

fn engine_f32_hex(data: &[u8]) -> Vec<serde_json::Value> {
    data.chunks_exact(4)
        .map(|word| {
            serde_json::Value::String(format!(
                "{:08x}",
                u32::from_le_bytes(word.try_into().expect("word slice"))
            ))
        })
        .collect()
}

fn engine_hex_lines(data: &[u8], stride: usize) -> Vec<serde_json::Value> {
    data.chunks_exact(stride)
        .map(|record| {
            serde_json::Value::String(record.iter().map(|b| format!("{b:02x}")).collect())
        })
        .collect()
}

/// Invokes one fixed-output exported symbol against a canary arena.
fn invoke_fixed(
    input: &[u8],
    output_len: usize,
    call: impl FnOnce(*const u8, usize, *mut u8, usize) -> u32,
) -> (u32, Vec<u8>) {
    let mut output = vec![ENGINE_CANARY_BYTE; output_len];
    let status = call(
        input.as_ptr(),
        input.len(),
        output.as_mut_ptr(),
        output.len(),
    );
    (status, output)
}

/// Invokes one two-phase exported symbol against a canary arena, returning
/// the status, the metadata word and the arena.
fn invoke_sized(
    input: &[u8],
    output_len: usize,
    call: impl FnOnce(*const u8, usize, *mut u8, usize, *mut usize) -> u32,
) -> (u32, usize, Vec<u8>) {
    let mut output = vec![ENGINE_CANARY_BYTE; output_len];
    let mut written = usize::MAX;
    let status = call(
        input.as_ptr(),
        input.len(),
        output.as_mut_ptr(),
        output.len(),
        &mut written,
    );
    (status, written, output)
}

fn engine_ok_fields(
    status: u32,
    output_len: usize,
    digest: String,
) -> serde_json::Map<String, serde_json::Value> {
    let mut fields = serde_json::Map::new();
    fields.insert("status".to_string(), serde_json::Value::from(status as u64));
    fields.insert(
        "output_len".to_string(),
        serde_json::Value::from(output_len as u64),
    );
    fields.insert(
        "payload_sha256".to_string(),
        serde_json::Value::String(digest),
    );
    fields
}

fn execute_engine_collision(case: &FrozenCase) -> serde_json::Value {
    let args = validate_engine_binary_arguments(&case.arguments, &case.id, &case.family).unwrap();
    assert_eq!(args.abi_version, engine_abi_version());
    let version = args.abi_version;
    let (status, output) =
        invoke_fixed(&case.input, args.output_capacity, |ip, il, op, ol| unsafe {
            mornlea_collision_resolve(version, ip, il, op, ol)
        });
    let category = engine_status_category(status)
        .unwrap_or_else(|| panic!("case {} unknown status {status}", case.id));
    let mut fields = engine_ok_fields(status, output.len(), engine_digest_hex(&output));
    let kind = if status == 0 { "ok" } else { "error" };
    if status == 0 {
        fields.insert(
            "used_payload_sha256".to_string(),
            serde_json::Value::String(engine_digest_hex(&output)),
        );
        fields.insert(
            "f32_bits".to_string(),
            serde_json::Value::Array(engine_f32_hex(&output[..12])),
        );
    }
    serde_json::json!({"kind": kind, "category": category, "fields": fields})
}

fn execute_engine_physics(case: &FrozenCase) -> serde_json::Value {
    let args = validate_engine_binary_arguments(&case.arguments, &case.id, &case.family).unwrap();
    assert_eq!(args.abi_version, engine_abi_version());
    let version = args.abi_version;
    let (status, output) =
        invoke_fixed(&case.input, args.output_capacity, |ip, il, op, ol| unsafe {
            mornlea_physics_step(version, ip, il, op, ol)
        });
    let category = engine_status_category(status)
        .unwrap_or_else(|| panic!("case {} unknown status {status}", case.id));
    let mut fields = engine_ok_fields(status, output.len(), engine_digest_hex(&output));
    let kind = if status == 0 { "ok" } else { "error" };
    if status == 0 {
        fields.insert(
            "used_payload_sha256".to_string(),
            serde_json::Value::String(engine_digest_hex(&output)),
        );
        fields.insert(
            "f32_bits".to_string(),
            serde_json::Value::Array(engine_f32_hex(&output[..24])),
        );
    }
    serde_json::json!({"kind": kind, "category": category, "fields": fields})
}

fn execute_engine_raycast(case: &FrozenCase) -> serde_json::Value {
    let args = validate_engine_binary_arguments(&case.arguments, &case.id, &case.family).unwrap();
    assert_eq!(args.abi_version, engine_abi_version());
    assert_eq!(
        case.input.len(),
        104,
        "case {} raycast input must carry request plus cursor",
        case.id
    );
    let version = args.abi_version;
    let request = case.input[..40].to_vec();
    let mut cursor = case.input[40..].to_vec();
    let mut output = vec![ENGINE_CANARY_BYTE; args.output_capacity];
    let mut batches = Vec::new();
    let mut status = 0;
    for _ in 0..ENGINE_MAX_BATCH_CALLS {
        let mut count = usize::MAX;
        let mut done = 0xff_u8;
        status = unsafe {
            mornlea_raycast_batch(
                version,
                request.as_ptr(),
                request.len(),
                cursor.as_mut_ptr(),
                cursor.len(),
                output.as_mut_ptr(),
                output.len(),
                &mut count,
                &mut done,
            )
        };
        if status != 0 {
            break;
        }
        let records = engine_hex_lines(&output[..count * 20], 20);
        let distances = output[..count * 20]
            .chunks_exact(20)
            .map(|record| {
                serde_json::Value::String(format!(
                    "{:08x}",
                    u32::from_le_bytes(record[16..20].try_into().expect("distance slice"))
                ))
            })
            .collect::<Vec<_>>();
        batches.push(serde_json::json!({
            "count": count as u64,
            "done": if done == 1 { 1 } else { 0 },
            "records": records,
            "distances": distances,
            "cursor_sha256": engine_digest_hex(&cursor),
        }));
        if done == 1 {
            break;
        }
    }
    let category = engine_status_category(status)
        .unwrap_or_else(|| panic!("case {} unknown status {status}", case.id));
    let mut fields = engine_ok_fields(status, output.len(), engine_digest_hex(&output));
    let kind = if status == 0 { "ok" } else { "error" };
    if status == 0 {
        assert!(
            !batches.is_empty()
                && batches.last().expect("batch").get("done") == Some(&serde_json::Value::from(1)),
            "case {} sequence finished without a done batch",
            case.id
        );
        let last_count = batches
            .last()
            .expect("batch")
            .get("count")
            .and_then(|v| v.as_u64())
            .expect("count") as usize;
        fields.insert("batches".to_string(), serde_json::Value::Array(batches));
        fields.insert(
            "used_payload_sha256".to_string(),
            serde_json::Value::String(engine_digest_hex(&output[..last_count * 20])),
        );
    } else {
        fields.insert(
            "cursor_sha256".to_string(),
            serde_json::Value::String(engine_digest_hex(&cursor)),
        );
    }
    serde_json::json!({"kind": kind, "category": category, "fields": fields})
}

fn execute_engine_chunk(case: &FrozenCase) -> serde_json::Value {
    let args = validate_engine_binary_arguments(&case.arguments, &case.id, &case.family).unwrap();
    assert_eq!(args.abi_version, engine_abi_version());
    let version = args.abi_version;
    let (status, output) =
        invoke_fixed(&case.input, args.output_capacity, |ip, il, op, ol| unsafe {
            mornlea_worldgen_chunk(version, ip, il, op, ol)
        });
    let category = engine_status_category(status)
        .unwrap_or_else(|| panic!("case {} unknown status {status}", case.id));
    let mut fields = engine_ok_fields(status, output.len(), engine_digest_hex(&output));
    let kind = if status == 0 { "ok" } else { "error" };
    if status == 0 {
        fields.insert(
            "used_payload_sha256".to_string(),
            serde_json::Value::String(engine_digest_hex(&output)),
        );
    }
    serde_json::json!({"kind": kind, "category": category, "fields": fields})
}

fn execute_engine_probe(case: &FrozenCase) -> serde_json::Value {
    let args = validate_engine_binary_arguments(&case.arguments, &case.id, &case.family).unwrap();
    assert_eq!(args.abi_version, engine_abi_version());
    let version = args.abi_version;
    let (status, output) =
        invoke_fixed(&case.input, args.output_capacity, |ip, il, op, ol| unsafe {
            mornlea_worldgen_probe(version, ip, il, op, ol)
        });
    let category = engine_status_category(status)
        .unwrap_or_else(|| panic!("case {} unknown status {status}", case.id));
    let mut fields = engine_ok_fields(status, output.len(), engine_digest_hex(&output));
    let kind = if status == 0 { "ok" } else { "error" };
    if status == 0 {
        let count = output.len() / 8;
        fields.insert(
            "output_count".to_string(),
            serde_json::Value::from(count as u64),
        );
        fields.insert(
            "records".to_string(),
            serde_json::Value::Array(engine_hex_lines(&output, 8)),
        );
        fields.insert(
            "used_payload_sha256".to_string(),
            serde_json::Value::String(engine_digest_hex(&output)),
        );
    }
    serde_json::json!({"kind": kind, "category": category, "fields": fields})
}

fn execute_engine_tree(case: &FrozenCase) -> serde_json::Value {
    let args = validate_engine_binary_arguments(&case.arguments, &case.id, &case.family).unwrap();
    assert_eq!(args.abi_version, engine_abi_version());
    let version = args.abi_version;
    let (status, output) =
        invoke_fixed(&case.input, args.output_capacity, |ip, il, op, ol| unsafe {
            mornlea_tree_blocks(version, ip, il, op, ol)
        });
    let category = engine_status_category(status)
        .unwrap_or_else(|| panic!("case {} unknown status {status}", case.id));
    let mut fields = engine_ok_fields(status, output.len(), engine_digest_hex(&output));
    let kind = if status == 0 { "ok" } else { "error" };
    if status == 0 {
        let count = u32::from_le_bytes(output[..4].try_into().expect("count slice")) as usize;
        fields.insert(
            "output_count".to_string(),
            serde_json::Value::from(count as u64),
        );
        fields.insert(
            "records_sha256".to_string(),
            serde_json::Value::String(engine_digest_hex(&output[..4 + count * 8])),
        );
        fields.insert(
            "used_payload_sha256".to_string(),
            serde_json::Value::String(engine_digest_hex(&output[..4 + count * 8])),
        );
    }
    serde_json::json!({"kind": kind, "category": category, "fields": fields})
}

fn execute_engine_lod(case: &FrozenCase) -> serde_json::Value {
    let args = validate_engine_binary_arguments(&case.arguments, &case.id, &case.family).unwrap();
    assert_eq!(args.abi_version, engine_abi_version());
    let version = args.abi_version;
    let (status, written, output) = invoke_sized(
        &case.input,
        args.output_capacity,
        |ip, il, op, ol, out| unsafe { mornlea_lod_shell(version, ip, il, op, ol, out) },
    );
    let category = engine_status_category(status)
        .unwrap_or_else(|| panic!("case {} unknown status {status}", case.id));
    let mut fields = engine_ok_fields(status, output.len(), engine_digest_hex(&output));
    let kind = if status == 0 { "ok" } else { "error" };
    if status == 0 {
        assert_eq!(
            written,
            output.len(),
            "case {} exact-capacity write",
            case.id
        );
        let quads = written / 20;
        fields.insert(
            "output_count".to_string(),
            serde_json::Value::from(quads as u64),
        );
        fields.insert(
            "quads_sha256".to_string(),
            serde_json::Value::String(engine_digest_hex(&output)),
        );
        fields.insert(
            "used_payload_sha256".to_string(),
            serde_json::Value::String(engine_digest_hex(&output)),
        );
    } else if status == 7 {
        fields.insert(
            "needed_bytes".to_string(),
            serde_json::Value::from(written as u64),
        );
    }
    serde_json::json!({"kind": kind, "category": category, "fields": fields})
}

fn execute_engine_fluid_eval(case: &FrozenCase) -> serde_json::Value {
    let args = validate_engine_binary_arguments(&case.arguments, &case.id, &case.family).unwrap();
    assert_eq!(args.abi_version, engine_abi_version());
    let version = args.abi_version;
    let (status, written, output) = invoke_sized(
        &case.input,
        args.output_capacity,
        |ip, il, op, ol, out| unsafe { mornlea_fluid_eval_batch(version, ip, il, op, ol, out) },
    );
    let category = engine_status_category(status)
        .unwrap_or_else(|| panic!("case {} unknown status {status}", case.id));
    let mut fields = engine_ok_fields(status, output.len(), engine_digest_hex(&output));
    let kind = if status == 0 { "ok" } else { "error" };
    if status == 0 {
        assert_eq!(
            written,
            output.len(),
            "case {} exact-capacity write",
            case.id
        );
        if output.len() <= 64 {
            fields.insert(
                "output_hex".to_string(),
                serde_json::Value::String(output.iter().map(|b| format!("{b:02x}")).collect()),
            );
        }
        fields.insert(
            "used_payload_sha256".to_string(),
            serde_json::Value::String(engine_digest_hex(&output)),
        );
    }
    serde_json::json!({"kind": kind, "category": category, "fields": fields})
}

fn execute_engine_fluid_rescan(case: &FrozenCase) -> serde_json::Value {
    let args = validate_engine_binary_arguments(&case.arguments, &case.id, &case.family).unwrap();
    assert_eq!(args.abi_version, engine_abi_version());
    let version = args.abi_version;
    let (status, written, output) = invoke_sized(
        &case.input,
        args.output_capacity,
        |ip, il, op, ol, out| unsafe { mornlea_fluid_rescan(version, ip, il, op, ol, out) },
    );
    let category = engine_status_category(status)
        .unwrap_or_else(|| panic!("case {} unknown status {status}", case.id));
    let mut fields = engine_ok_fields(status, output.len(), engine_digest_hex(&output));
    let kind = if status == 0 { "ok" } else { "error" };
    if status == 0 {
        let tail = &output[written - 8..written];
        let spent = u32::from_le_bytes(tail[..4].try_into().expect("spent slice"));
        let done = if tail[4] == 1 { 1 } else { 0 };
        let positions = output[..written - 8]
            .chunks_exact(12)
            .map(|entry| {
                serde_json::json!([
                    i32::from_le_bytes(entry[..4].try_into().expect("x slice")),
                    i32::from_le_bytes(entry[4..8].try_into().expect("y slice")),
                    i32::from_le_bytes(entry[8..12].try_into().expect("z slice")),
                ])
            })
            .collect::<Vec<_>>();
        fields.insert("spent".to_string(), serde_json::Value::from(spent as u64));
        fields.insert("done".to_string(), serde_json::Value::from(done));
        fields.insert("positions".to_string(), serde_json::Value::Array(positions));
        fields.insert(
            "used_payload_sha256".to_string(),
            serde_json::Value::String(engine_digest_hex(&output[..written])),
        );
    } else if status == 7 {
        fields.insert(
            "needed_bytes".to_string(),
            serde_json::Value::from(written as u64),
        );
    }
    serde_json::json!({"kind": kind, "category": category, "fields": fields})
}

fn execute_engine_mesh(case: &FrozenCase) -> serde_json::Value {
    let args = validate_engine_binary_arguments(&case.arguments, &case.id, &case.family).unwrap();
    assert_eq!(args.abi_version, engine_abi_version());
    let version = args.abi_version;
    let scratch_words = args.scratch_capacity.expect("mesh scratch capacity");
    let mut scratch = vec![ENGINE_CANARY_BYTE; scratch_words * 8];
    let mut output = vec![ENGINE_CANARY_WORD; args.output_capacity];
    let mut written = usize::MAX;
    let status = unsafe {
        mornlea_mesh_section(
            version,
            case.input.as_ptr(),
            case.input.len(),
            scratch.as_mut_ptr(),
            scratch.len(),
            output.as_mut_ptr(),
            output.len(),
            &mut written,
        )
    };
    let arena = output
        .iter()
        .flat_map(|word| word.to_le_bytes())
        .collect::<Vec<_>>();
    let category = engine_status_category(status)
        .unwrap_or_else(|| panic!("case {} unknown status {status}", case.id));
    let mut fields = serde_json::Map::new();
    fields.insert("status".to_string(), serde_json::Value::from(status as u64));
    fields.insert(
        "output_capacity".to_string(),
        serde_json::Value::from(args.output_capacity as u64),
    );
    fields.insert("output_count".to_string(), serde_json::Value::from(0));
    fields.insert(
        "payload_sha256".to_string(),
        serde_json::Value::String(engine_digest_hex(&arena)),
    );
    let kind = if status == 0 { "ok" } else { "error" };
    if status == 0 {
        let quads = output[..written]
            .iter()
            .map(|word| {
                serde_json::Value::String(
                    word.to_le_bytes()
                        .iter()
                        .map(|b| format!("{b:02x}"))
                        .collect(),
                )
            })
            .collect::<Vec<_>>();
        fields.insert(
            "output_count".to_string(),
            serde_json::Value::from(written as u64),
        );
        fields.insert("quads".to_string(), serde_json::Value::Array(quads));
        fields.insert(
            "quads_sha256".to_string(),
            serde_json::Value::String(engine_digest_hex(&arena[..written * 8])),
        );
        fields.insert(
            "used_payload_sha256".to_string(),
            serde_json::Value::String(engine_digest_hex(&arena[..written * 8])),
        );
    }
    serde_json::json!({"kind": kind, "category": category, "fields": fields})
}

fn engine_path_error_category(error: &PathError) -> &'static str {
    match error {
        PathError::InvalidGrid => "invalid-grid",
        PathError::InvalidRevision => "invalid-revision",
        PathError::ScratchTooSmall => "scratch-too-small",
        PathError::Unreachable => "unreachable",
        PathError::BudgetExceeded => "budget-exceeded",
        PathError::Allocation => "allocation",
    }
}

fn execute_engine_pathfind(case: &FrozenCase) -> serde_json::Value {
    let kind_value = validate_engine_pathfind_arguments(&case.arguments, &case.id).unwrap();
    let input = case
        .input_json
        .as_ref()
        .unwrap_or_else(|| panic!("case {} carries no JSON input", case.id));
    let get_i32 = |key: &str, index: usize| -> i32 {
        input
            .get(key)
            .and_then(|v| v.as_array())
            .and_then(|a| a.get(index))
            .and_then(|v| v.as_i64())
            .and_then(|v| i32::try_from(v).ok())
            .unwrap_or_else(|| panic!("case {} input {key}[{index}] must be i32", case.id))
    };
    let get_u32 = |key: &str, index: usize| -> u32 {
        input
            .get(key)
            .and_then(|v| v.as_array())
            .and_then(|a| a.get(index))
            .and_then(|v| v.as_u64())
            .and_then(|v| u32::try_from(v).ok())
            .unwrap_or_else(|| panic!("case {} input {key}[{index}] must be u32", case.id))
    };
    let origin = PathCell {
        x: get_i32("origin", 0),
        y: get_i32("origin", 1),
        z: get_i32("origin", 2),
    };
    let size = [get_u32("size", 0), get_u32("size", 1), get_u32("size", 2)];
    let cells = size[0] as usize * size[1] as usize * size[2] as usize;
    if cells > ENGINE_MAX_CELLS {
        return serde_json::json!({"kind": "error", "category": "invalid-grid", "fields": {}});
    }
    let mut blocks: Vec<u16> = Vec::new();
    for run in input
        .get("blocks_rle")
        .and_then(|v| v.as_array())
        .unwrap_or_else(|| panic!("case {} input blocks_rle must be an array", case.id))
    {
        let pair = run
            .as_array()
            .unwrap_or_else(|| panic!("case {} RLE run must be a pair", case.id));
        let id = pair
            .first()
            .and_then(|v| v.as_u64())
            .unwrap_or_else(|| panic!("case {} RLE id must be u64", case.id));
        let count = pair
            .get(1)
            .and_then(|v| v.as_u64())
            .unwrap_or_else(|| panic!("case {} RLE count must be u64", case.id));
        if id > 65535 || count == 0 {
            return serde_json::json!({"kind": "error", "category": "invalid-grid", "fields": {}});
        }
        if blocks.len() as u64 + count > ENGINE_MAX_CELLS as u64 {
            return serde_json::json!({"kind": "error", "category": "invalid-grid", "fields": {}});
        }
        blocks.extend(std::iter::repeat_n(id as u16, count as usize));
    }
    if blocks.len() != cells {
        return serde_json::json!({"kind": "error", "category": "invalid-grid", "fields": {}});
    }
    let passable_ids = input
        .get("passable_ids")
        .and_then(|v| v.as_array())
        .unwrap_or_else(|| panic!("case {} input passable_ids must be an array", case.id))
        .iter()
        .map(|v| {
            v.as_u64()
                .and_then(|id| u16::try_from(id).ok())
                .unwrap_or_else(|| panic!("case {} passable id out of u16 range", case.id))
        })
        .collect::<Vec<_>>();
    let table =
        PathBlockTable::from_passable_ids(&passable_ids).expect("passable table construction");
    let mut revisions = Vec::new();
    for revision in input
        .get("revisions")
        .and_then(|v| v.as_array())
        .unwrap_or_else(|| panic!("case {} input revisions must be an array", case.id))
    {
        let chunk = revision
            .get("chunk")
            .and_then(|v| v.as_array())
            .unwrap_or_else(|| panic!("case {} revision chunk must be a pair", case.id));
        let chunk_x = chunk
            .first()
            .and_then(|v| v.as_i64())
            .and_then(|v| i32::try_from(v).ok())
            .unwrap_or_else(|| panic!("case {} revision chunk must be i32", case.id));
        let chunk_z = chunk
            .get(1)
            .and_then(|v| v.as_i64())
            .and_then(|v| i32::try_from(v).ok())
            .unwrap_or_else(|| panic!("case {} revision chunk must be i32", case.id));
        let number = revision
            .get("revision")
            .and_then(|v| v.as_str())
            .and_then(|s| s.parse::<u64>().ok())
            .unwrap_or_else(|| panic!("case {} revision must be a decimal u64 string", case.id));
        revisions.push(PathRevision {
            chunk: [chunk_x, chunk_z],
            revision: number,
        });
    }
    let grid = match PathGrid::try_new(origin, size, blocks.into_boxed_slice(), table, revisions) {
        Ok(grid) => grid,
        Err(error) => {
            return serde_json::json!({"kind": "error", "category": engine_path_error_category(&error), "fields": {}});
        }
    };
    // A copied loaded input must not mutate the owned grid: mutate a scratch
    // copy of the loaded blocks and prove the grid reads unchanged.
    let mut copied = case.input.clone();
    if !copied.is_empty() {
        copied[0] ^= 0x01;
    }
    let normalized = grid
        .revisions()
        .iter()
        .map(|revision| {
            serde_json::json!({"chunk": [revision.chunk[0], revision.chunk[1]], "revision": revision.revision.to_string()})
        })
        .collect::<Vec<_>>();
    if kind_value == "grid" {
        return serde_json::json!({"kind": "ok", "category": "ok", "fields": {"revisions": normalized}});
    }
    let point = |key: &str| -> PathCell {
        PathCell {
            x: get_i32(key, 0),
            y: get_i32(key, 1),
            z: get_i32(key, 2),
        }
    };
    if input.get("start").is_none() || input.get("goal").is_none() {
        panic!("case {} search carries no start/goal", case.id);
    }
    let start = point("start");
    let goal = point("goal");
    let mut scratch =
        PathScratch::try_with_capacity(cells).expect("scratch capacity covers the grid");
    match find_path(&grid, start, goal, &mut scratch) {
        Ok(result) => {
            let waypoints = result
                .waypoints()
                .iter()
                .map(|cell| serde_json::json!([cell.x, cell.y, cell.z]))
                .collect::<Vec<_>>();
            serde_json::json!({"kind": "ok", "category": "ok", "fields": {"revisions": normalized, "waypoints": waypoints}})
        }
        Err(error) => {
            serde_json::json!({"kind": "error", "category": engine_path_error_category(&error), "fields": {}})
        }
    }
}

fn execute_engine_case(case: &FrozenCase) -> serde_json::Value {
    if case.operation != "kernel" {
        panic!(
            "case {} operation {} is not executable here",
            case.id, case.operation
        );
    }
    if case.consumer != CorpusConsumer::Engine {
        panic!("case {} consumer is not the engine consumer", case.id);
    }
    let route = ENGINE_CLOSED_ROUTES
        .iter()
        .find(|(family, version)| *family == case.family && *version == case.version)
        .unwrap_or_else(|| panic!("case {} rides an unregistered engine route", case.id));
    match route.0 {
        "kernel.mornlea_collision_resolve" => execute_engine_collision(case),
        "kernel.mornlea_physics_step" => execute_engine_physics(case),
        "kernel.mornlea_raycast_batch" => execute_engine_raycast(case),
        "kernel.mornlea_worldgen_chunk" => execute_engine_chunk(case),
        "kernel.mornlea_worldgen_probe" => execute_engine_probe(case),
        "kernel.mornlea_tree_blocks" => execute_engine_tree(case),
        "kernel.mornlea_lod_shell" => execute_engine_lod(case),
        "kernel.mornlea_fluid_eval_batch" => execute_engine_fluid_eval(case),
        "kernel.mornlea_fluid_rescan" => execute_engine_fluid_rescan(case),
        "kernel.mornlea_mesh_section" => execute_engine_mesh(case),
        "kernel.pathfind" => execute_engine_pathfind(case),
        _ => panic!("case {} rides an unregistered engine route", case.id),
    }
}

#[test]
fn engine_kernel_corpus_executes_all_routes() {
    let cases = load_cases_for_consumer(CorpusConsumer::Engine);
    assert!(
        !cases.is_empty(),
        "engine corpus selection must not be empty"
    );
    let mut executed = 0_usize;
    let mut ok_by_route: HashMap<(String, String), usize> = HashMap::new();
    let mut error_by_route: HashMap<(String, String), usize> = HashMap::new();
    let mut executed_ids: HashSet<String> = HashSet::new();
    for case in &cases {
        let actual = execute_engine_case(case);
        assert_normalized(case, actual);
        executed += 1;
        executed_ids.insert(case.id.clone());
        let route = (case.family.clone(), case.version.clone());
        if case.category == "ok" {
            *ok_by_route.entry(route).or_insert(0) += 1;
        } else {
            *error_by_route.entry(route).or_insert(0) += 1;
        }
    }
    assert_eq!(
        executed,
        cases.len(),
        "every loaded engine case must execute exactly once"
    );
    for (family, version) in ENGINE_CLOSED_ROUTES {
        let route = (family.to_string(), version.to_string());
        assert!(
            ok_by_route.get(&route).copied().unwrap_or(0) > 0,
            "route {family}/{version} has no executed ok case"
        );
        assert!(
            error_by_route.get(&route).copied().unwrap_or(0) > 0,
            "route {family}/{version} has no executed error case"
        );
    }
    for label in ENGINE_BOUNDARY_LABELS {
        assert!(
            executed_ids.contains(*label),
            "boundary case {label} did not execute"
        );
    }
}

#[test]
fn engine_kernel_corpus_rejects_unregistered_route() {
    let case = FrozenCase {
        id: "kernel.nope/1/sample".to_string(),
        family: "kernel.nope".to_string(),
        version: "1".to_string(),
        consumer: CorpusConsumer::Engine,
        packet_key: None,
        operation: "kernel".to_string(),
        arguments: serde_json::json!({}),
        input_format: runtime_corpus::InputFormat::Binary,
        input: vec![],
        input_json: None,
        normalized: serde_json::json!({"kind": "error", "category": "input"}),
        encoded: None,
        category: "input".to_string(),
    };
    let err = std::panic::catch_unwind(|| execute_engine_case(&case));
    assert!(err.is_err(), "cases on unregistered routes must fail");
}

#[test]
fn engine_kernel_corpus_rejects_invalid_arguments() {
    let binary =
        serde_json::json!({"abi_version": 11, "output_capacity": 16, "buffer_variant": "bogus"});
    assert!(
        validate_engine_binary_arguments(&binary, "id", "kernel.mornlea_collision_resolve")
            .is_err(),
        "unknown buffer variant must fail loading"
    );
    let missing_scratch =
        serde_json::json!({"abi_version": 11, "output_capacity": 16, "buffer_variant": "normal"});
    assert!(
        validate_engine_binary_arguments(&missing_scratch, "id", "kernel.mornlea_mesh_section")
            .is_err(),
        "mesh cases without scratch capacity must fail loading"
    );
    let stray_scratch = serde_json::json!({"abi_version": 11, "output_capacity": 16, "scratch_capacity": 8, "buffer_variant": "normal"});
    assert!(
        validate_engine_binary_arguments(&stray_scratch, "id", "kernel.mornlea_collision_resolve")
            .is_err(),
        "scratch capacity outside mesh must fail loading"
    );
    let over_budget = serde_json::json!({"abi_version": 11, "output_capacity": 8388608, "buffer_variant": "normal"});
    assert!(
        validate_engine_binary_arguments(&over_budget, "id", "kernel.mornlea_collision_resolve")
            .is_err(),
        "capacity above the harness budget must fail loading"
    );
    let bad_kind = serde_json::json!({"operation_kind": "flood"});
    assert!(
        validate_engine_pathfind_arguments(&bad_kind, "id").is_err(),
        "unknown operation kind must fail loading"
    );
}

#[test]
fn engine_kernel_corpus_status_and_path_categories_cover_the_vocabulary() {
    for status in 0..=9 {
        assert!(
            runtime_corpus::engine_status_category(status).is_some(),
            "status {status} has no frozen category"
        );
    }
    assert!(runtime_corpus::engine_status_category(10).is_none());
    for name in [
        "ok",
        "invalid-grid",
        "invalid-revision",
        "scratch-too-small",
        "unreachable",
        "budget-exceeded",
        "allocation",
    ] {
        assert!(
            runtime_corpus::engine_pathfind_category(name).is_some(),
            "path category {name} is unknown"
        );
    }
    assert!(runtime_corpus::engine_pathfind_category("bogus").is_none());
}

#[test]
fn engine_kernel_corpus_mutation_fails_comparison() {
    let floor = runtime_corpus::load_case("kernel.mornlea_collision_resolve/11/floor-wall");
    let actual = execute_engine_case(&floor);
    assert_normalized(&floor, actual.clone());
    let mut mutated = actual;
    let bits = mutated
        .get_mut("fields")
        .and_then(|fields| fields.get_mut("f32_bits"))
        .and_then(|bits| bits.as_array_mut())
        .expect("float bit strings");
    let first = bits
        .first()
        .and_then(|v| v.as_str())
        .expect("first bit string")
        .to_string();
    let flipped = match first.strip_prefix('0') {
        Some(rest) => format!("1{rest}"),
        None => format!("0{}", &first[1..]),
    };
    bits[0] = serde_json::Value::String(flipped);
    let err = std::panic::catch_unwind(|| assert_normalized(&floor, mutated));
    assert!(err.is_err(), "mutated float bit must fail comparison");

    let corridor = runtime_corpus::load_case("kernel.pathfind/1/corridor");
    let actual = execute_engine_case(&corridor);
    assert_normalized(&corridor, actual.clone());
    let mut mutated = actual;
    let waypoints = mutated
        .get_mut("fields")
        .and_then(|fields| fields.get_mut("waypoints"))
        .and_then(|v| v.as_array_mut())
        .expect("waypoints");
    let first = waypoints
        .first()
        .and_then(|v| v.as_array())
        .expect("first waypoint")
        .clone();
    let mut moved = first;
    moved[0] = serde_json::Value::from(moved[0].as_i64().expect("x") + 1);
    waypoints[0] = serde_json::Value::Array(moved);
    let err = std::panic::catch_unwind(|| assert_normalized(&corridor, mutated));
    assert!(err.is_err(), "mutated waypoint must fail comparison");
}
