//! Frozen MCP service over planning snapshots.
//!
//! The service is a stateless HTTP POST endpoint at a loopback `/mcp`
//! listener speaking protocol `2025-11-25` for application `v1`, mirroring
//! the Go `companionMCPService` in
//! `packages/server/server/companion_mcp.go` with the outer validation order
//! of `companion_mcp_outer.go`. There are no sessions, SSE streams, batches,
//! or capabilities beyond the six frozen tools, which run in manifest order:
//! `get_planning_context`, `list_affordances`, `inspect_inventory`,
//! `find_visible_blocks`, `query_terrain`, and `validate_plan`. Tool input
//! and output shapes are the exact checked-in schemas; the method contract
//! never gains a key.
//!
//! Without an MCP SDK dependency the dispatch below implements the same
//! observable surface: `initialize` pins the capability set and version,
//! `notifications/initialized` answers 202 with an empty body, `tools/list`
//! serves the manifest-ordered schemas, and `tools/call` returns one text
//! content that is canonically equal to the structured content. Domain
//! refusals carry `isError: false`; transport failures carry `isError: true`
//! with a fixed payload and never echo the input. Every outer refusal is the
//! exact `{"error":{"code":"..."}}` shape with no leak.
//!
//! Request bodies are capped at 256 KiB and wire responses at 160 KiB, both
//! inclusive at the limit. Read-header, read, write, and idle timeouts are
//! 5 s, 35 s, 35 s, and 5 s; connections serve exactly one request and then
//! close, so there is no keep-alive idle state to time out. Closing shuts the
//! registry first, then the HTTP listener; a serve failure only settles the
//! done channel and never touches the world.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::time::Duration;

use crate::agent::snapshot::{SnapshotLease, SnapshotRegistry, canonical_snapshot_digest};
use crate::contracts::ServerError;

/// Frozen MCP protocol version required after `initialize`.
pub const MCP_PROTOCOL_VERSION: &str = "2025-11-25";
/// Frozen application contract version served by this endpoint.
pub const MCP_APP_VERSION: &str = "v1";
/// Frozen endpoint path; the query string must be empty.
pub const MCP_ENDPOINT_PATH: &str = "/mcp";
/// Inclusive request body cap, mirroring the manifest.
pub const MAX_MCP_REQUEST_BYTES: usize = 256 * 1024;
/// Inclusive wire response cap, mirroring the manifest.
pub const MAX_MCP_RESPONSE_BYTES: usize = 160 * 1024;
/// Inclusive canonical cap for a `validate_plan` candidate, mirroring the
/// planner bound the manifest pins.
pub const MAX_PLAN_INPUT_BYTES: usize = 64 * 1024;
/// Header block budget including the request line.
pub const MAX_MCP_HEADER_BYTES: usize = 16 * 1024;
/// Header read budget per connection.
pub const MCP_READ_HEADER_TIMEOUT: Duration = Duration::from_secs(5);
/// Body read budget per connection.
pub const MCP_READ_TIMEOUT: Duration = Duration::from_secs(35);
/// Response write budget per connection.
pub const MCP_WRITE_TIMEOUT: Duration = Duration::from_secs(35);
/// Idle budget; connections serve one request and close, so no keep-alive
/// idle state exists beyond this documented value.
pub const MCP_IDLE_TIMEOUT: Duration = Duration::from_secs(5);

/// World block Y span all position fields share with the digest.
const BLOCK_Y_MIN: i32 = -64;
const BLOCK_Y_MAX: i32 = 320;

fn invalid(field: &'static str) -> ServerError {
    ServerError::InvalidInput { field }
}

// ---------------------------------------------------------------------------
// Embedded contract.
// ---------------------------------------------------------------------------

const MANIFEST_JSON: &str =
    include_str!("../../../../../../packages/contracts/companion-agent/mcp-v1/manifest.json");
const SCHEMA_JSON: &str =
    include_str!("../../../../../../packages/contracts/companion-agent/mcp-v1/schema.json");

/// One frozen tool advertisement with its resolved schemas.
pub struct McpToolContract {
    pub name: String,
    pub input_schema: String,
    pub output_schema: String,
    pub canonical_limit: usize,
    pub domain_codes: Vec<String>,
    pub model_visible: bool,
    pub max_calls_per_run: Option<u32>,
}

/// The parsed frozen contract backing `tools/list` and dispatch.
pub struct McpContract {
    pub protocol_version: String,
    pub app_version: String,
    pub endpoint_path: String,
    pub stateless: bool,
    pub json_response: bool,
    pub sse: bool,
    pub sessions: bool,
    pub request_limit: usize,
    pub response_limit: usize,
    pub plan_limit: usize,
    pub tools: Vec<McpToolContract>,
}

/// Manifest order of the six tools; the fixed graph also calls the first and
/// last without model visibility.
const TOOL_ORDER: [&str; 6] = [
    "get_planning_context",
    "list_affordances",
    "inspect_inventory",
    "find_visible_blocks",
    "query_terrain",
    "validate_plan",
];

/// Canonical result caps in manifest order.
const TOOL_LIMITS: [usize; 6] = [
    24 * 1024,
    24 * 1024,
    8 * 1024,
    16 * 1024,
    16 * 1024,
    72 * 1024,
];

/// Domain result codes in manifest order.
const TOOL_DOMAIN_CODES: [&[&str]; 6] = [
    &[],
    &[],
    &[],
    &["unknown_block"],
    &["out_of_bounds"],
    &[
        "invalid_schema",
        "out_of_bounds",
        "unknown_player",
        "unmineable_target",
        "unknown_block",
        "missing_item",
        "snapshot_mismatch",
    ],
];

/// Model visibility in manifest order: the context and validator tools are
/// fixed graph calls, never model-visible.
const TOOL_MODEL_VISIBLE: [bool; 6] = [false, true, true, true, true, false];

/// Maximum calls per run in manifest order. The host enforces the validator
/// budget; the service only advertises it.
const TOOL_MAX_CALLS: [Option<u32>; 6] = [None, None, None, None, None, Some(2)];

/// Canonical result cap of one tool, or `None` for an unknown name.
pub fn tool_canonical_limit(name: &str) -> Option<usize> {
    TOOL_ORDER
        .iter()
        .position(|known| *known == name)
        .map(|index| TOOL_LIMITS[index])
}

/// Domain result codes of one tool, or `None` for an unknown name.
pub fn tool_domain_codes(name: &str) -> Option<&'static [&'static str]> {
    TOOL_ORDER
        .iter()
        .position(|known| *known == name)
        .map(|index| TOOL_DOMAIN_CODES[index])
}

/// Model visibility of one tool, or `None` for an unknown name.
pub fn tool_model_visible(name: &str) -> Option<bool> {
    TOOL_ORDER
        .iter()
        .position(|known| *known == name)
        .map(|index| TOOL_MODEL_VISIBLE[index])
}

/// Maximum calls per run of one tool, or `None` when unbounded or unknown.
pub fn tool_max_calls_per_run(name: &str) -> Option<u32> {
    TOOL_ORDER
        .iter()
        .position(|known| *known == name)
        .and_then(|index| TOOL_MAX_CALLS[index])
}

/// Loads the embedded manifest and schema, pins the frozen metadata, and
/// resolves every tool schema to a `$ref`-free object. Any drift fails
/// service construction instead of serving a partial contract.
pub fn load_contract() -> Result<McpContract, ServerError> {
    let manifest = JsonVal::parse(MANIFEST_JSON.as_bytes()).map_err(|_| invalid("mcp_manifest"))?;
    let schema = JsonVal::parse(SCHEMA_JSON.as_bytes()).map_err(|_| invalid("mcp_schema"))?;
    let string_field = |value: &JsonVal, key: &str| -> Result<String, ServerError> {
        value
            .get(key)
            .and_then(|field| field.as_str())
            .map(str::to_owned)
            .ok_or_else(|| invalid("mcp_manifest"))
    };
    if string_field(&manifest, "application_contract_version")? != MCP_APP_VERSION
        || string_field(&manifest, "mcp_protocol_version")? != MCP_PROTOCOL_VERSION
        || string_field(&manifest, "endpoint_path")? != MCP_ENDPOINT_PATH
    {
        return Err(invalid("mcp_manifest"));
    }
    let bool_field = |value: &JsonVal, key: &str| -> Result<bool, ServerError> {
        value
            .get(key)
            .and_then(|field| field.as_bool())
            .ok_or_else(|| invalid("mcp_manifest"))
    };
    let stateless = bool_field(&manifest, "stateless")?;
    let json_response = bool_field(&manifest, "json_response")?;
    let sse = bool_field(&manifest, "sse")?;
    let sessions = bool_field(&manifest, "sessions")?;
    if !stateless || !json_response || sse || sessions {
        return Err(invalid("mcp_manifest"));
    }
    let limits = manifest
        .get("limits")
        .ok_or_else(|| invalid("mcp_manifest"))?;
    let limit = |key: &str| -> Result<usize, ServerError> {
        limits
            .get(key)
            .and_then(|field| field.as_usize())
            .ok_or_else(|| invalid("mcp_manifest"))
    };
    let request_limit = limit("request_body_bytes")?;
    let response_limit = limit("wire_response_bytes")?;
    let plan_limit = limit("plan_input_bytes")?;
    if request_limit != MAX_MCP_REQUEST_BYTES
        || response_limit != MAX_MCP_RESPONSE_BYTES
        || plan_limit != MAX_PLAN_INPUT_BYTES
    {
        return Err(invalid("mcp_manifest"));
    }
    let defs = schema.get("$defs").ok_or_else(|| invalid("mcp_schema"))?;
    let manifest_tools = manifest
        .get("tools")
        .and_then(|tools| tools.as_array())
        .ok_or_else(|| invalid("mcp_manifest"))?;
    if manifest_tools.len() != TOOL_ORDER.len() {
        return Err(invalid("mcp_manifest"));
    }
    let mut tools = Vec::with_capacity(TOOL_ORDER.len());
    for (index, entry) in manifest_tools.iter().enumerate() {
        let name = string_field(entry, "name")?;
        if name != TOOL_ORDER[index] {
            return Err(invalid("mcp_manifest"));
        }
        let input_name = string_field(entry, "input_schema")?;
        let output_name = string_field(entry, "result_schema")?;
        let manifest_codes = entry
            .get("domain_result_codes")
            .and_then(|codes| codes.as_array())
            .ok_or_else(|| invalid("mcp_manifest"))?;
        let mut codes_text = Vec::with_capacity(manifest_codes.len());
        for code in manifest_codes {
            codes_text.push(
                code.as_str()
                    .ok_or_else(|| invalid("mcp_manifest"))?
                    .to_owned(),
            );
        }
        if codes_text != tool_domain_codes(&name).unwrap_or(&[]) {
            return Err(invalid("mcp_manifest"));
        }
        let canonical_limit = entry
            .get("canonical_result_bytes")
            .and_then(|field| field.as_usize())
            .ok_or_else(|| invalid("mcp_manifest"))?;
        if Some(canonical_limit) != tool_canonical_limit(&name) {
            return Err(invalid("mcp_manifest"));
        }
        let input_schema = resolve_schema(defs, &input_name)?;
        let output_schema = resolve_schema(defs, &output_name)?;
        if input_schema.get("type").and_then(|kind| kind.as_str()) != Some("object") {
            return Err(invalid("mcp_schema"));
        }
        tools.push(McpToolContract {
            input_schema: input_schema.write_sorted(),
            output_schema: output_schema.write_sorted(),
            canonical_limit,
            domain_codes: codes_text,
            model_visible: tool_model_visible(&name).unwrap_or(false),
            max_calls_per_run: tool_max_calls_per_run(&name),
            name,
        });
    }
    validate_place_enum(&tools)?;
    Ok(McpContract {
        protocol_version: MCP_PROTOCOL_VERSION.to_owned(),
        app_version: MCP_APP_VERSION.to_owned(),
        endpoint_path: MCP_ENDPOINT_PATH.to_owned(),
        stateless,
        json_response,
        sse,
        sessions,
        request_limit,
        response_limit,
        plan_limit,
        tools,
    })
}

/// Resolves one named definition, inlining `#/$defs/` references with cycle
/// detection. External references fail closed.
fn resolve_schema(defs: &JsonVal, name: &str) -> Result<JsonVal, ServerError> {
    let mut stack = vec![name.to_owned()];
    resolve_value(
        defs.get(name).ok_or_else(|| invalid("mcp_schema"))?,
        defs,
        &mut stack,
    )
}

fn resolve_value(
    value: &JsonVal,
    defs: &JsonVal,
    stack: &mut Vec<String>,
) -> Result<JsonVal, ServerError> {
    match value {
        JsonVal::Object(entries) if entries.len() == 1 && entries[0].0 == "$ref" => {
            let target = entries[0].1.as_str().ok_or_else(|| invalid("mcp_schema"))?;
            let name = target
                .strip_prefix("#/$defs/")
                .ok_or_else(|| invalid("mcp_schema"))?;
            if stack.contains(&name.to_owned()) {
                return Err(invalid("mcp_schema"));
            }
            stack.push(name.to_owned());
            let resolved = resolve_value(
                defs.get(name).ok_or_else(|| invalid("mcp_schema"))?,
                defs,
                stack,
            )?;
            stack.pop();
            Ok(resolved)
        }
        JsonVal::Object(entries) => {
            let mut resolved = Vec::with_capacity(entries.len());
            for (key, item) in entries {
                resolved.push((key.clone(), resolve_value(item, defs, stack)?));
            }
            Ok(JsonVal::Object(resolved))
        }
        JsonVal::Array(items) => {
            let mut resolved = Vec::with_capacity(items.len());
            for item in items {
                resolved.push(resolve_value(item, defs, stack)?);
            }
            Ok(JsonVal::Array(resolved))
        }
        JsonVal::Null | JsonVal::Bool(_) | JsonVal::Number(_) | JsonVal::Str(_) => {
            Ok(value.clone())
        }
    }
}

/// The `validate_plan` place enum must stay the fixed 23-name delivery set in
/// manifest order, matching the core canonical registry.
fn validate_place_enum(tools: &[McpToolContract]) -> Result<(), ServerError> {
    let validator = tools
        .iter()
        .find(|tool| tool.name == "validate_plan")
        .ok_or_else(|| invalid("mcp_schema"))?;
    let parsed =
        JsonVal::parse(validator.input_schema.as_bytes()).map_err(|_| invalid("mcp_schema"))?;
    let steps = parsed
        .get("properties")
        .and_then(|properties| properties.get("plan"))
        .and_then(|plan| plan.get("properties"))
        .and_then(|properties| properties.get("steps"))
        .and_then(|steps| steps.get("items"))
        .and_then(|items| items.get("oneOf"))
        .and_then(|one| one.as_array())
        .ok_or_else(|| invalid("mcp_schema"))?;
    let mut names = Vec::new();
    for branch in steps {
        let Some(values) = branch
            .get("properties")
            .and_then(|properties| properties.get("block"))
            .and_then(|block| block.get("enum"))
            .and_then(|values| values.as_array())
        else {
            continue;
        };
        for value in values {
            names.push(
                value
                    .as_str()
                    .ok_or_else(|| invalid("mcp_schema"))?
                    .to_owned(),
            );
        }
    }
    let want: Vec<String> = PLACE_TABLE
        .iter()
        .map(|(name, _, _)| (*name).to_owned())
        .collect();
    if names != want {
        return Err(invalid("mcp_schema"));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Block and item tables from the frozen core registry.
// ---------------------------------------------------------------------------

/// Canonical block names by stable number, from the Go core registry.
const BLOCK_NAMES: [&str; 90] = [
    "air",
    "barrier",
    "stone",
    "dirt",
    "grass",
    "bedrock",
    "stone_brick",
    "coal_ore",
    "iron_ore",
    "furnace",
    "iron_block",
    "chest",
    "light_block",
    "cobblestone",
    "smooth_stone",
    "sand",
    "gravel",
    "oak_log",
    "oak_planks",
    "leaves",
    "glass",
    "brick",
    "white_wool",
    "roof_tile",
    "clay",
    "snow_block",
    "mossy_cobblestone",
    "water_source",
    "water_level_1",
    "water_level_2",
    "water_level_3",
    "water_level_4",
    "water_level_5",
    "water_level_6",
    "water_level_7",
    "farmland_dry",
    "farmland_wet",
    "wheat_stage_0",
    "wheat_stage_1",
    "wheat_stage_2",
    "wheat_stage_3",
    "wheat_stage_4",
    "wheat_stage_5",
    "wheat_stage_6",
    "wheat_stage_7",
    "workbench",
    "potato_stage_0",
    "potato_stage_1",
    "potato_stage_2",
    "potato_stage_3",
    "potato_stage_4",
    "potato_stage_5",
    "potato_stage_6",
    "potato_stage_7",
    "carrot_stage_0",
    "carrot_stage_1",
    "carrot_stage_2",
    "carrot_stage_3",
    "carrot_stage_4",
    "carrot_stage_5",
    "carrot_stage_6",
    "carrot_stage_7",
    "door_lower_south_closed",
    "door_lower_south_open",
    "door_lower_west_closed",
    "door_lower_west_open",
    "door_lower_north_closed",
    "door_lower_north_open",
    "door_lower_east_closed",
    "door_lower_east_open",
    "door_upper",
    "torch_standing",
    "torch_wall_pos_x",
    "torch_wall_neg_x",
    "torch_wall_pos_z",
    "torch_wall_neg_z",
    "bed_foot_south",
    "bed_foot_west",
    "bed_foot_north",
    "bed_foot_east",
    "bed_head_south",
    "bed_head_west",
    "bed_head_north",
    "bed_head_east",
    "short_grass",
    "snow_layer_1",
    "snow_layer_2",
    "snow_layer_3",
    "snow_layer_4",
    "oak_sapling",
];

/// Canonical item names by stable number, from the Go core registry: explicit
/// machine names win, otherwise the placed block name is reused.
const ITEM_NAMES: [&str; 66] = [
    "",
    "stone",
    "dirt",
    "grass",
    "stone_brick",
    "coal",
    "raw_iron",
    "iron_ingot",
    "furnace",
    "iron_block",
    "stone_pickaxe",
    "iron_pickaxe",
    "broken_stone_pickaxe",
    "broken_iron_pickaxe",
    "chest",
    "light_block",
    "cobblestone",
    "smooth_stone",
    "sand",
    "gravel",
    "oak_log",
    "oak_planks",
    "leaves",
    "glass",
    "brick",
    "white_wool",
    "roof_tile",
    "clay",
    "snow_block",
    "mossy_cobblestone",
    "stone_hoe",
    "iron_hoe",
    "broken_stone_hoe",
    "broken_iron_hoe",
    "wheat_seeds",
    "wheat",
    "bread",
    "stick",
    "workbench",
    "bone_meal",
    "potato",
    "carrot",
    "poisonous_potato",
    "door",
    "torch",
    "rotten_flesh",
    "bed",
    "wooden_sword",
    "stone_sword",
    "iron_sword",
    "broken_wooden_sword",
    "broken_stone_sword",
    "broken_iron_sword",
    "raw_beef",
    "cooked_beef",
    "empty_bucket",
    "water_bucket",
    "oak_sapling",
    "iron_helmet",
    "iron_chestplate",
    "iron_leggings",
    "iron_boots",
    "bow",
    "arrow",
    "bone",
    "broken_bow",
];

/// Single-product mining drops by block number, from the Go `BlockDrop` table.
const BLOCK_DROPS: [(u16, u16); 52] = [
    (2, 1),
    (3, 2),
    (4, 3),
    (6, 4),
    (7, 5),
    (8, 6),
    (9, 8),
    (10, 9),
    (11, 14),
    (12, 15),
    (13, 16),
    (14, 17),
    (15, 18),
    (16, 19),
    (17, 20),
    (18, 21),
    (19, 22),
    (20, 23),
    (21, 24),
    (22, 25),
    (23, 26),
    (24, 27),
    (25, 28),
    (26, 29),
    (35, 2),
    (36, 2),
    (37, 34),
    (38, 34),
    (39, 34),
    (40, 34),
    (41, 34),
    (42, 34),
    (43, 34),
    (44, 35),
    (45, 38),
    (46, 40),
    (47, 40),
    (48, 40),
    (49, 40),
    (50, 40),
    (51, 40),
    (52, 40),
    (53, 40),
    (62, 43),
    (63, 43),
    (64, 43),
    (65, 43),
    (66, 43),
    (67, 43),
    (68, 43),
    (69, 43),
    (70, 43),
];

/// Torch, bed, sapling drops share one item per family.
const FAMILY_DROPS: [(u16, u16); 14] = [
    (71, 44),
    (72, 44),
    (73, 44),
    (74, 44),
    (75, 44),
    (76, 46),
    (77, 46),
    (78, 46),
    (79, 46),
    (80, 46),
    (81, 46),
    (82, 46),
    (83, 46),
    (89, 57),
];

/// Place delivery set in manifest order: name, block, and item numbers.
const PLACE_TABLE: [(&str, u16, u16); 23] = [
    ("brick", 21, 24),
    ("chest", 11, 14),
    ("clay", 24, 27),
    ("cobblestone", 13, 16),
    ("dirt", 3, 2),
    ("furnace", 9, 8),
    ("glass", 20, 23),
    ("grass", 4, 3),
    ("gravel", 16, 19),
    ("iron_block", 10, 9),
    ("leaves", 19, 22),
    ("light_block", 12, 15),
    ("mossy_cobblestone", 26, 29),
    ("oak_log", 17, 20),
    ("oak_planks", 18, 21),
    ("roof_tile", 23, 26),
    ("sand", 15, 18),
    ("smooth_stone", 14, 17),
    ("snow_block", 25, 28),
    ("stone", 2, 1),
    ("stone_brick", 6, 4),
    ("white_wool", 22, 25),
    ("workbench", 45, 38),
];

fn block_name(block: u16) -> Option<&'static str> {
    BLOCK_NAMES
        .get(block as usize)
        .copied()
        .filter(|name| !name.is_empty())
}

fn block_by_name(name: &str) -> Option<u16> {
    BLOCK_NAMES
        .iter()
        .position(|known| *known == name)
        .map(|index| index as u16)
}

fn item_name(item: u16) -> Option<&'static str> {
    ITEM_NAMES
        .get(item as usize)
        .copied()
        .filter(|name| !name.is_empty())
}

fn block_drop(block: u16) -> Option<u16> {
    BLOCK_DROPS
        .iter()
        .find(|(id, _)| *id == block)
        .map(|(_, item)| *item)
        .or_else(|| {
            FAMILY_DROPS
                .iter()
                .find(|(id, _)| *id == block)
                .map(|(_, item)| *item)
        })
}

fn is_crop(block: u16) -> bool {
    (37..=44).contains(&block) || (46..=53).contains(&block) || (54..=61).contains(&block)
}

fn is_farmland(block: u16) -> bool {
    (35..=36).contains(&block)
}

fn is_torch(block: u16) -> bool {
    (71..=75).contains(&block)
}

fn is_wild_grass(block: u16) -> bool {
    block == 84
}

/// Mine semantics and drop name of one exposed block, from the planner rule:
/// containers batch, mature multi-drop crops stay undelivered, crops,
/// farmland, torches, and wild grass are forbidden, and everything else
/// follows the single-drop table.
fn mine_description(block: u16) -> (String, Option<String>) {
    let semantics = if block == 11 || block == 9 {
        "container_batch"
    } else if block == 44 || block == 53 || block == 61 {
        "undelivered_multi_drop"
    } else if is_crop(block) || is_farmland(block) {
        "forbidden_farming"
    } else if is_torch(block) {
        "forbidden_torch"
    } else if block_drop(block).is_some() {
        "single_drop"
    } else {
        "no_drop"
    };
    let drop = block_drop(block).and_then(item_name).map(str::to_owned);
    (semantics.to_owned(), drop)
}

/// Planner mine gate: crops, farmland, torches, and wild grass are explicitly
/// refused even where a drop is registered; the rest need a single drop.
fn mineable_block(block: u16) -> bool {
    if is_crop(block) || is_farmland(block) || is_torch(block) || is_wild_grass(block) {
        return false;
    }
    block_drop(block).is_some()
}

fn place_entry(name: &str) -> Option<(u16, u16)> {
    PLACE_TABLE
        .iter()
        .find(|(known, _, _)| *known == name)
        .map(|(_, block, item)| (*block, *item))
}

// ---------------------------------------------------------------------------
// Minimal JSON value model with strict parsing and canonical output.
// ---------------------------------------------------------------------------

/// Strict JSON value: duplicate object keys, trailing data, and non-finite
/// numbers are refused at parse time. Numbers keep their literal text so a
/// canonical re-encode preserves the sender's spelling exactly like the Go
/// `json.Number` round trip.
#[derive(Clone, Debug)]
pub enum JsonVal {
    Null,
    Bool(bool),
    Number(JsonNumber),
    Str(String),
    Array(Vec<JsonVal>),
    Object(Vec<(String, JsonVal)>),
}

/// A validated JSON number literal with its integer value when the spelling
/// is a plain in-range integer.
#[derive(Clone, Debug)]
pub struct JsonNumber {
    text: String,
    int: Option<i64>,
}

impl JsonNumber {
    fn text(&self) -> &str {
        &self.text
    }

    fn int(&self) -> Option<i64> {
        self.int
    }
}

struct JsonParser<'a> {
    input: &'a [u8],
    pos: usize,
    depth: usize,
}

impl<'a> JsonParser<'a> {
    fn error(&self) -> ServerError {
        invalid("mcp_json")
    }

    fn peek(&self) -> Option<u8> {
        self.input.get(self.pos).copied()
    }

    fn skip_ws(&mut self) {
        while matches!(
            self.peek(),
            Some(b' ') | Some(b'\t') | Some(b'\n') | Some(b'\r')
        ) {
            self.pos += 1;
        }
    }

    fn parse(&mut self) -> Result<JsonVal, ServerError> {
        self.skip_ws();
        let value = self.parse_value()?;
        self.skip_ws();
        if self.pos != self.input.len() {
            return Err(self.error());
        }
        Ok(value)
    }

    fn parse_value(&mut self) -> Result<JsonVal, ServerError> {
        self.depth += 1;
        if self.depth > 1024 {
            return Err(self.error());
        }
        let value = match self.peek() {
            Some(b'{') => self.parse_object(),
            Some(b'[') => self.parse_array(),
            Some(b'"') => Ok(JsonVal::Str(self.parse_string()?)),
            Some(b't') => self.parse_literal("true", JsonVal::Bool(true)),
            Some(b'f') => self.parse_literal("false", JsonVal::Bool(false)),
            Some(b'n') => self.parse_literal("null", JsonVal::Null),
            Some(b'-') | Some(b'0'..=b'9') => Ok(JsonVal::Number(self.parse_number()?)),
            _ => Err(self.error()),
        };
        self.depth -= 1;
        value
    }

    fn parse_literal(&mut self, word: &str, value: JsonVal) -> Result<JsonVal, ServerError> {
        if self.input[self.pos..].starts_with(word.as_bytes()) {
            self.pos += word.len();
            Ok(value)
        } else {
            Err(self.error())
        }
    }

    fn parse_object(&mut self) -> Result<JsonVal, ServerError> {
        self.pos += 1;
        let mut entries = Vec::new();
        self.skip_ws();
        if self.peek() == Some(b'}') {
            self.pos += 1;
            return Ok(JsonVal::Object(entries));
        }
        loop {
            self.skip_ws();
            if self.peek() != Some(b'"') {
                return Err(self.error());
            }
            let key = self.parse_string()?;
            self.skip_ws();
            if self.peek() != Some(b':') {
                return Err(self.error());
            }
            self.pos += 1;
            self.skip_ws();
            let value = self.parse_value()?;
            if entries.iter().any(|(known, _)| *known == key) {
                return Err(self.error());
            }
            entries.push((key, value));
            self.skip_ws();
            match self.peek() {
                Some(b',') => {
                    self.pos += 1;
                }
                Some(b'}') => {
                    self.pos += 1;
                    return Ok(JsonVal::Object(entries));
                }
                _ => return Err(self.error()),
            }
        }
    }

    fn parse_array(&mut self) -> Result<JsonVal, ServerError> {
        self.pos += 1;
        let mut items = Vec::new();
        self.skip_ws();
        if self.peek() == Some(b']') {
            self.pos += 1;
            return Ok(JsonVal::Array(items));
        }
        loop {
            self.skip_ws();
            items.push(self.parse_value()?);
            self.skip_ws();
            match self.peek() {
                Some(b',') => {
                    self.pos += 1;
                }
                Some(b']') => {
                    self.pos += 1;
                    return Ok(JsonVal::Array(items));
                }
                _ => return Err(self.error()),
            }
        }
    }

    fn parse_string(&mut self) -> Result<String, ServerError> {
        self.pos += 1;
        let mut out = String::new();
        loop {
            let byte = self
                .input
                .get(self.pos)
                .copied()
                .ok_or_else(|| self.error())?;
            match byte {
                b'"' => {
                    self.pos += 1;
                    return Ok(out);
                }
                b'\\' => {
                    self.pos += 1;
                    let escaped = self
                        .input
                        .get(self.pos)
                        .copied()
                        .ok_or_else(|| self.error())?;
                    self.pos += 1;
                    match escaped {
                        b'"' => out.push('"'),
                        b'\\' => out.push('\\'),
                        b'/' => out.push('/'),
                        b'b' => out.push('\u{0008}'),
                        b'f' => out.push('\u{000C}'),
                        b'n' => out.push('\n'),
                        b'r' => out.push('\r'),
                        b't' => out.push('\t'),
                        b'u' => out.push(self.parse_hex()?),
                        _ => return Err(self.error()),
                    }
                }
                0x00..=0x1f => return Err(self.error()),
                _ => {
                    let rest =
                        std::str::from_utf8(&self.input[self.pos..]).map_err(|_| self.error())?;
                    let ch = rest.chars().next().ok_or_else(|| self.error())?;
                    out.push(ch);
                    self.pos += ch.len_utf8();
                }
            }
        }
    }

    fn parse_hex(&mut self) -> Result<char, ServerError> {
        let unit = self.parse_hex_unit()?;
        if (0xd800..0xdc00).contains(&unit) {
            if self.input.get(self.pos..self.pos + 2) != Some(b"\\u".as_slice()) {
                return Err(self.error());
            }
            self.pos += 2;
            let low = self.parse_hex_unit()?;
            if !(0xdc00..0xe000).contains(&low) {
                return Err(self.error());
            }
            let scalar = 0x10000 + ((unit - 0xd800) << 10) + (low - 0xdc00);
            return char::from_u32(scalar).ok_or_else(|| self.error());
        }
        if (0xdc00..0xe000).contains(&unit) {
            return Err(self.error());
        }
        char::from_u32(unit).ok_or_else(|| self.error())
    }

    fn parse_hex_unit(&mut self) -> Result<u32, ServerError> {
        if self.pos + 4 > self.input.len() {
            return Err(self.error());
        }
        let mut unit = 0u32;
        for byte in &self.input[self.pos..self.pos + 4] {
            let digit = match byte {
                b'0'..=b'9' => (byte - b'0') as u32,
                b'a'..=b'f' => (byte - b'a' + 10) as u32,
                b'A'..=b'F' => (byte - b'A' + 10) as u32,
                _ => return Err(self.error()),
            };
            unit = unit * 16 + digit;
        }
        self.pos += 4;
        Ok(unit)
    }

    fn parse_number(&mut self) -> Result<JsonNumber, ServerError> {
        let start = self.pos;
        if self.peek() == Some(b'-') {
            self.pos += 1;
        }
        let int_start = self.pos;
        match self.peek() {
            Some(b'0') => {
                self.pos += 1;
            }
            Some(b'1'..=b'9') => {
                while matches!(self.peek(), Some(b'0'..=b'9')) {
                    self.pos += 1;
                }
            }
            _ => return Err(self.error()),
        }
        let mut is_int = true;
        if self.peek() == Some(b'.') {
            is_int = false;
            self.pos += 1;
            if !matches!(self.peek(), Some(b'0'..=b'9')) {
                return Err(self.error());
            }
            while matches!(self.peek(), Some(b'0'..=b'9')) {
                self.pos += 1;
            }
        }
        if matches!(self.peek(), Some(b'e') | Some(b'E')) {
            is_int = false;
            self.pos += 1;
            if matches!(self.peek(), Some(b'+') | Some(b'-')) {
                self.pos += 1;
            }
            if !matches!(self.peek(), Some(b'0'..=b'9')) {
                return Err(self.error());
            }
            while matches!(self.peek(), Some(b'0'..=b'9')) {
                self.pos += 1;
            }
        }
        let text = std::str::from_utf8(&self.input[start..self.pos])
            .map_err(|_| self.error())?
            .to_owned();
        let _ = int_start;
        let int = if is_int {
            text.parse::<i64>().ok()
        } else {
            None
        };
        Ok(JsonNumber { text, int })
    }
}

impl JsonVal {
    /// Strictly parses one JSON document with no trailing data.
    pub fn parse(input: &[u8]) -> Result<JsonVal, ServerError> {
        std::str::from_utf8(input).map_err(|_| invalid("mcp_json"))?;
        JsonParser {
            input,
            pos: 0,
            depth: 0,
        }
        .parse()
    }

    pub fn get(&self, key: &str) -> Option<&JsonVal> {
        match self {
            JsonVal::Object(entries) => entries
                .iter()
                .find(|(known, _)| known == key)
                .map(|(_, value)| value),
            _ => None,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            JsonVal::Str(value) => Some(value),
            _ => None,
        }
    }

    pub fn as_bool(&self) -> Option<bool> {
        match self {
            JsonVal::Bool(value) => Some(*value),
            _ => None,
        }
    }

    pub fn as_i64(&self) -> Option<i64> {
        match self {
            JsonVal::Number(number) => number.int(),
            _ => None,
        }
    }

    pub fn as_usize(&self) -> Option<usize> {
        self.as_i64().and_then(|value| usize::try_from(value).ok())
    }

    pub fn as_array(&self) -> Option<&[JsonVal]> {
        match self {
            JsonVal::Array(items) => Some(items),
            _ => None,
        }
    }

    pub fn as_object(&self) -> Option<&[(String, JsonVal)]> {
        match self {
            JsonVal::Object(entries) => Some(entries),
            _ => None,
        }
    }

    pub fn is_null(&self) -> bool {
        matches!(self, JsonVal::Null)
    }

    /// Canonical encoding: object keys sorted, compact, Go string escaping,
    /// number literals preserved.
    pub fn write_sorted(&self) -> String {
        let mut out = Vec::new();
        self.write_into(&mut out);
        String::from_utf8(out).expect("canonical utf8")
    }

    fn write_into(&self, out: &mut Vec<u8>) {
        match self {
            JsonVal::Null => out.extend_from_slice(b"null"),
            JsonVal::Bool(true) => out.extend_from_slice(b"true"),
            JsonVal::Bool(false) => out.extend_from_slice(b"false"),
            JsonVal::Number(number) => out.extend_from_slice(number.text().as_bytes()),
            JsonVal::Str(value) => escape_into(out, value),
            JsonVal::Array(items) => {
                out.push(b'[');
                for (index, item) in items.iter().enumerate() {
                    if index > 0 {
                        out.push(b',');
                    }
                    item.write_into(out);
                }
                out.push(b']');
            }
            JsonVal::Object(entries) => {
                let mut ordered: Vec<&(String, JsonVal)> = entries.iter().collect();
                ordered.sort_by(|a, b| a.0.cmp(&b.0));
                out.push(b'{');
                for (index, (key, value)) in ordered.iter().enumerate() {
                    if index > 0 {
                        out.push(b',');
                    }
                    escape_into(out, key);
                    out.push(b':');
                    value.write_into(out);
                }
                out.push(b'}');
            }
        }
    }
}

/// Go string escaping with HTML escaping disabled; shared with the digest so
/// tool text and digests spell strings identically.
fn escape_into(out: &mut Vec<u8>, value: &str) {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    out.push(b'"');
    for ch in value.chars() {
        match ch {
            '"' => out.extend_from_slice(b"\\\""),
            '\\' => out.extend_from_slice(b"\\\\"),
            '\n' => out.extend_from_slice(b"\\n"),
            '\r' => out.extend_from_slice(b"\\r"),
            '\t' => out.extend_from_slice(b"\\t"),
            '\u{2028}' => out.extend_from_slice(b"\\u2028"),
            '\u{2029}' => out.extend_from_slice(b"\\u2029"),
            c if (c as u32) < 0x20 => {
                out.extend_from_slice(b"\\u00");
                out.push(HEX[(c as u8 >> 4) as usize]);
                out.push(HEX[(c as u8 & 0x0f) as usize]);
            }
            c => {
                let mut encoded = [0u8; 4];
                out.extend_from_slice(c.encode_utf8(&mut encoded).as_bytes());
            }
        }
    }
    out.push(b'"');
}

// ---------------------------------------------------------------------------
// Tool execution.
// ---------------------------------------------------------------------------

/// Successful tool run: canonical bytes that serve as both the text content
/// and the structured content. `domain_failure` marks a normal
/// code-and-hint refusal, which still answers with `isError: false`.
pub struct ToolSuccess {
    pub canonical: Vec<u8>,
    pub domain_failure: bool,
}

/// Tool failure classes: invalid input and oversized results become
/// `isError: true` tool results, while unavailability aborts the request.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ToolFault {
    InvalidInput,
    TooLarge,
    Unavailable,
}

/// Tool runner seam behind the MCP dispatcher. The production runner is
/// [`FrozenTools`]; tests inject counting, gated, or fixed doubles.
pub trait PlanningTools: Send + Sync {
    fn execute(
        &self,
        lease: &SnapshotLease,
        tool: &str,
        input: &[u8],
    ) -> Result<ToolSuccess, ToolFault>;
}

/// The six frozen planning tools over one lease's deep-copy snapshot.
pub struct FrozenTools;

impl PlanningTools for FrozenTools {
    fn execute(
        &self,
        lease: &SnapshotLease,
        tool: &str,
        input: &[u8],
    ) -> Result<ToolSuccess, ToolFault> {
        lease.checkpoint().map_err(|_| ToolFault::Unavailable)?;
        let outcome = match tool {
            "get_planning_context" => context_tool(lease, input),
            "list_affordances" => affordances_tool(lease, input),
            "inspect_inventory" => inventory_tool(lease, input),
            "find_visible_blocks" => find_blocks_tool(lease, input),
            "query_terrain" => terrain_tool(lease, input),
            "validate_plan" => validate_plan_tool(lease, input),
            _ => return Err(ToolFault::InvalidInput),
        }?;
        lease.checkpoint().map_err(|_| ToolFault::Unavailable)?;
        Ok(outcome)
    }
}

fn tool_checkpoint(lease: &SnapshotLease) -> Result<(), ToolFault> {
    lease.checkpoint().map_err(|_| ToolFault::Unavailable)
}

fn finish_tool(
    lease: &SnapshotLease,
    name: &str,
    canonical: Vec<u8>,
    domain_failure: bool,
) -> Result<ToolSuccess, ToolFault> {
    let limit = tool_canonical_limit(name).ok_or(ToolFault::InvalidInput)?;
    if canonical.len() > limit {
        return Err(ToolFault::TooLarge);
    }
    tool_checkpoint(lease)?;
    Ok(ToolSuccess {
        canonical,
        domain_failure,
    })
}

fn text_valid(value: &str, max_bytes: usize, require_non_empty: bool) -> bool {
    value.len() <= max_bytes
        && !value.chars().any(|ch| ch.is_control())
        && (!require_non_empty || !value.trim().is_empty())
}

fn strict_object(input: &[u8], allowed: &[&str]) -> Result<HashMap<String, JsonVal>, ToolFault> {
    let value = JsonVal::parse(input).map_err(|_| ToolFault::InvalidInput)?;
    let entries = value.as_object().ok_or(ToolFault::InvalidInput)?;
    let mut fields = HashMap::with_capacity(entries.len());
    for (key, item) in entries {
        if !allowed.contains(&key.as_str()) {
            return Err(ToolFault::InvalidInput);
        }
        if fields.insert(key.clone(), item.clone()).is_some() {
            return Err(ToolFault::InvalidInput);
        }
    }
    Ok(fields)
}

fn uuid_text(bytes: [u8; 16]) -> String {
    let hex: String = bytes.iter().map(|byte| format!("{byte:02x}")).collect();
    format!(
        "{}-{}-{}-{}-{}",
        &hex[0..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..32]
    )
}

/// Canonical bytes of one already-shaped value with the tool bound applied.
fn encode_tool(
    lease: &SnapshotLease,
    name: &str,
    body: &str,
    domain_failure: bool,
) -> Result<ToolSuccess, ToolFault> {
    tool_checkpoint(lease)?;
    let canonical = JsonVal::parse(body.as_bytes())
        .map(|value| value.write_sorted().into_bytes())
        .map_err(|_| ToolFault::Unavailable)?;
    finish_tool(lease, name, canonical, domain_failure)
}

fn go_float(value: f32) -> Result<String, ToolFault> {
    crate::agent::snapshot::go_float_text(value).map_err(|_| ToolFault::Unavailable)
}

fn context_tool(lease: &SnapshotLease, input: &[u8]) -> Result<ToolSuccess, ToolFault> {
    if !strict_object(input, &[])?.is_empty() {
        return Err(ToolFault::InvalidInput);
    }
    tool_checkpoint(lease)?;
    let snapshot = lease.snapshot();
    let mut out = String::from("{\"chunk_revisions\":[");
    for (index, revision) in snapshot.chunk_revisions.iter().enumerate() {
        tool_checkpoint(lease)?;
        if index > 0 {
            out.push(',');
        }
        out.push_str(&format!(
            "{{\"revision\":{},\"x\":{},\"z\":{}}}",
            revision.revision,
            revision.pos.x(),
            revision.pos.z()
        ));
    }
    let issuer = &snapshot.issuer;
    let issuer_position = issuer.position.get();
    let companion = &snapshot.companion;
    let companion_position = companion.position.get();
    out.push_str(&format!(
        "],\"companion\":{{\"companion_id\":\"{}\",\"pitch\":{},\"position\":[{},{},{}],\"task_status\":",
        uuid_text(companion.companion_id.bytes()),
        go_float(companion.look.pitch())?,
        go_float(companion_position[0])?,
        go_float(companion_position[1])?,
        go_float(companion_position[2])?,
    ));
    let mut escaped = Vec::new();
    escape_into(&mut escaped, companion.task_status.as_str());
    out.push_str(std::str::from_utf8(&escaped).map_err(|_| ToolFault::Unavailable)?);
    out.push_str(&format!(
        ",\"yaw\":{}}},\"instruction\":",
        go_float(companion.look.yaw())?
    ));
    let mut escaped = Vec::new();
    escape_into(&mut escaped, snapshot.instruction.as_str());
    out.push_str(std::str::from_utf8(&escaped).map_err(|_| ToolFault::Unavailable)?);
    out.push_str(&format!(
        ",\"issuer\":{{\"look_hit\":{}",
        match issuer.look_hit {
            Some(hit) => format!("{{\"x\":{},\"y\":{},\"z\":{}}}", hit.x(), hit.y(), hit.z()),
            None => "null".to_owned(),
        }
    ));
    out.push_str(&format!(
        ",\"pitch\":{},\"player_id\":\"{}\",\"position\":[{},{},{}],\"yaw\":{}}}",
        go_float(issuer.look.pitch())?,
        uuid_text(issuer.player_id.bytes()),
        go_float(issuer_position[0])?,
        go_float(issuer_position[1])?,
        go_float(issuer_position[2])?,
        go_float(issuer.look.yaw())?,
    ));
    out.push_str(&format!(
        ",\"snapshot_digest\":\"{}\",\"world_time_ticks\":{}}}",
        lease.digest_hex(),
        snapshot.world_time_ticks
    ));
    encode_tool(lease, "get_planning_context", &out, false)
}

fn affordances_tool(lease: &SnapshotLease, input: &[u8]) -> Result<ToolSuccess, ToolFault> {
    if !strict_object(input, &[])?.is_empty() {
        return Err(ToolFault::InvalidInput);
    }
    tool_checkpoint(lease)?;
    let snapshot = lease.snapshot();
    let mut players = String::from("[");
    for (index, player) in snapshot.online_players.iter().enumerate() {
        tool_checkpoint(lease)?;
        if index > 0 {
            players.push(',');
        }
        let position = player.position.get();
        players.push_str(&format!(
            "{{\"player_id\":\"{}\",\"position\":[{},{},{}]}}",
            uuid_text(player.player_id.bytes()),
            go_float(position[0])?,
            go_float(position[1])?,
            go_float(position[2])?,
        ));
    }
    players.push(']');
    let mut blocks: Vec<String> = Vec::with_capacity(snapshot.exposed_blocks.len());
    for block in &snapshot.exposed_blocks {
        tool_checkpoint(lease)?;
        let name = block_name(block.block_id).ok_or(ToolFault::Unavailable)?;
        let (semantics, drop) = mine_description(block.block_id);
        let drop_text = match drop {
            Some(name) => {
                let mut escaped = Vec::new();
                escape_into(&mut escaped, &name);
                std::str::from_utf8(&escaped)
                    .map_err(|_| ToolFault::Unavailable)?
                    .to_owned()
            }
            None => "null".to_owned(),
        };
        blocks.push(format!(
            "{{\"block_id\":{},\"block_name\":\"{name}\",\"drop_item\":{drop_text},\"mine_semantics\":\"{semantics}\",\"position\":{{\"x\":{},\"y\":{},\"z\":{}}}}}",
            block.block_id,
            block.position.x(),
            block.position.y(),
            block.position.z(),
        ));
    }
    // Longest coordinate-ordered complete prefix that fits the manifest cap;
    // an empty source still encodes, while a source whose first item alone
    // overflows is a hard failure.
    let prefix = bounded_prefix(lease, &players, &blocks)?;
    let mut out = String::from("{\"online_players\":");
    out.push_str(&players);
    out.push_str(",\"step_kinds\":[\"go_to\",\"follow\",\"mine\",\"place\"],\"visible_blocks\":[");
    out.push_str(&prefix.join(","));
    out.push_str("]}");
    encode_tool(lease, "list_affordances", &out, false)
}

fn bounded_prefix(
    lease: &SnapshotLease,
    players: &str,
    blocks: &[String],
) -> Result<Vec<String>, ToolFault> {
    let limit = tool_canonical_limit("list_affordances").ok_or(ToolFault::InvalidInput)?;
    let fits = |count: usize| -> Result<bool, ToolFault> {
        tool_checkpoint(lease)?;
        let mut candidate = String::from("{\"online_players\":");
        candidate.push_str(players);
        candidate.push_str(
            ",\"step_kinds\":[\"go_to\",\"follow\",\"mine\",\"place\"],\"visible_blocks\":[",
        );
        candidate.push_str(&blocks[..count].join(","));
        candidate.push_str("]}");
        let canonical = JsonVal::parse(candidate.as_bytes())
            .map(|value| value.write_sorted())
            .map_err(|_| ToolFault::Unavailable)?;
        tool_checkpoint(lease)?;
        Ok(canonical.len() <= limit)
    };
    if blocks.is_empty() {
        if !fits(0)? {
            return Err(ToolFault::TooLarge);
        }
        return Ok(Vec::new());
    }
    if !fits(1)? {
        return Err(ToolFault::TooLarge);
    }
    let (mut low, mut high) = (1usize, blocks.len() + 1);
    while low < high {
        let middle = low + (high - low) / 2;
        if fits(middle)? {
            low = middle + 1;
        } else {
            high = middle;
        }
    }
    Ok(blocks[..low - 1].to_vec())
}

fn inventory_tool(lease: &SnapshotLease, input: &[u8]) -> Result<ToolSuccess, ToolFault> {
    let fields = strict_object(input, &["offset", "limit"])?;
    let offset = fields
        .get("offset")
        .and_then(|value| value.as_i64())
        .ok_or(ToolFault::InvalidInput)?;
    let limit = fields
        .get("limit")
        .and_then(|value| value.as_i64())
        .ok_or(ToolFault::InvalidInput)?;
    if !(0..=35).contains(&offset) || !(1..=36).contains(&limit) {
        return Err(ToolFault::InvalidInput);
    }
    let end = (offset + limit).min(36);
    let mut slots = String::from("[");
    let mut first = true;
    for slot in offset..end {
        tool_checkpoint(lease)?;
        let stack = lease.snapshot().companion.inventory[slot as usize];
        if stack.item == 0 {
            continue;
        }
        let name = item_name(stack.item).ok_or(ToolFault::Unavailable)?;
        if !first {
            slots.push(',');
        }
        first = false;
        slots.push_str(&format!(
            "{{\"count\":{},\"item\":\"{name}\",\"slot\":{slot}}}",
            stack.count
        ));
    }
    slots.push(']');
    encode_tool(
        lease,
        "inspect_inventory",
        &format!("{{\"slots\":{slots}}}"),
        false,
    )
}

fn find_blocks_tool(lease: &SnapshotLease, input: &[u8]) -> Result<ToolSuccess, ToolFault> {
    let fields = strict_object(input, &["block_names", "limit"])?;
    let names = fields
        .get("block_names")
        .and_then(|value| value.as_array())
        .ok_or(ToolFault::InvalidInput)?;
    let limit = fields
        .get("limit")
        .and_then(|value| value.as_i64())
        .ok_or(ToolFault::InvalidInput)?;
    if names.is_empty() || names.len() > 16 || !(1..=64).contains(&limit) {
        return Err(ToolFault::InvalidInput);
    }
    let mut seen = Vec::with_capacity(names.len());
    for name in names {
        tool_checkpoint(lease)?;
        let text = name.as_str().ok_or(ToolFault::InvalidInput)?;
        if !text_valid(text, 64, true) || seen.contains(&text) {
            return Err(ToolFault::InvalidInput);
        }
        seen.push(text);
    }
    // Every name is text-valid before any lookup, so a schema violation is
    // never masked by an earlier domain failure.
    let mut wanted = Vec::with_capacity(names.len());
    for name in seen {
        tool_checkpoint(lease)?;
        match block_by_name(name) {
            Some(block) => wanted.push(block),
            None => {
                return encode_tool(
                    lease,
                    "find_visible_blocks",
                    "{\"code\":\"unknown_block\",\"hint\":\"unknown canonical block name\"}",
                    true,
                );
            }
        }
    }
    let mut matches = String::from("[");
    let mut count = 0i64;
    for block in &lease.snapshot().exposed_blocks {
        tool_checkpoint(lease)?;
        if !wanted.contains(&block.block_id) {
            continue;
        }
        let name = block_name(block.block_id).ok_or(ToolFault::Unavailable)?;
        let (_, drop) = mine_description(block.block_id);
        let drop_text = match drop {
            Some(name) => {
                let mut escaped = Vec::new();
                escape_into(&mut escaped, &name);
                std::str::from_utf8(&escaped)
                    .map_err(|_| ToolFault::Unavailable)?
                    .to_owned()
            }
            None => "null".to_owned(),
        };
        if count > 0 {
            matches.push(',');
        }
        matches.push_str(&format!(
            "{{\"block_name\":\"{name}\",\"drop_item\":{drop_text},\"position\":{{\"x\":{},\"y\":{},\"z\":{}}}}}",
            block.position.x(),
            block.position.y(),
            block.position.z(),
        ));
        count += 1;
        if count == limit {
            break;
        }
    }
    matches.push(']');
    encode_tool(
        lease,
        "find_visible_blocks",
        &format!("{{\"matches\":{matches}}}"),
        false,
    )
}

fn terrain_lookup(lease: &SnapshotLease, x: i32, y: i32, z: i32) -> Option<(u16, i32)> {
    let terrain = &lease.snapshot().terrain;
    if !(BLOCK_Y_MIN..BLOCK_Y_MAX).contains(&y) {
        return None;
    }
    let dx = x.checked_sub(terrain.origin.x())?;
    let dz = z.checked_sub(terrain.origin.z())?;
    let dy = y.checked_sub(terrain.origin.y())?;
    if !(0..33).contains(&dx) || !(0..33).contains(&dz) || !(0..17).contains(&dy) {
        return None;
    }
    let column = dx as usize * 33 + dz as usize;
    if terrain.ready_columns[column / 8] & (1 << (column % 8)) == 0 {
        return None;
    }
    let index = (dx as usize * 17 + dy as usize) * 33 + dz as usize;
    Some((
        terrain.blocks.get(index).copied().unwrap_or(0),
        i32::from(terrain.heights.get(column).copied().unwrap_or(-65)),
    ))
}

fn terrain_tool(lease: &SnapshotLease, input: &[u8]) -> Result<ToolSuccess, ToolFault> {
    let fields = strict_object(input, &["positions"])?;
    let positions = fields
        .get("positions")
        .and_then(|value| value.as_array())
        .ok_or(ToolFault::InvalidInput)?;
    if positions.is_empty() || positions.len() > 64 {
        return Err(ToolFault::InvalidInput);
    }
    let mut points = String::from("[");
    for (index, position) in positions.iter().enumerate() {
        tool_checkpoint(lease)?;
        let entries = position.as_object().ok_or(ToolFault::InvalidInput)?;
        if entries.len() != 3
            || entries
                .iter()
                .any(|(key, _)| !["x", "y", "z"].contains(&key.as_str()))
        {
            return Err(ToolFault::InvalidInput);
        }
        let coord = |key: &str| -> Result<i32, ToolFault> {
            let value = position
                .get(key)
                .and_then(|field| field.as_i64())
                .ok_or(ToolFault::InvalidInput)?;
            i32::try_from(value).map_err(|_| ToolFault::InvalidInput)
        };
        let (x, y, z) = (coord("x")?, coord("y")?, coord("z")?);
        if !(BLOCK_Y_MIN..BLOCK_Y_MAX).contains(&y) {
            return Err(ToolFault::InvalidInput);
        }
        let Some((block, height)) = terrain_lookup(lease, x, y, z) else {
            return encode_tool(
                lease,
                "query_terrain",
                "{\"code\":\"out_of_bounds\",\"hint\":\"position is outside the frozen projection\"}",
                true,
            );
        };
        let name = block_name(block).ok_or(ToolFault::Unavailable)?;
        if index > 0 {
            points.push(',');
        }
        points.push_str(&format!(
            "{{\"block_name\":\"{name}\",\"height\":{height},\"position\":{{\"x\":{x},\"y\":{y},\"z\":{z}}}}}"
        ));
    }
    points.push(']');
    encode_tool(
        lease,
        "query_terrain",
        &format!("{{\"terrain\":{points}}}"),
        false,
    )
}

// ---------------------------------------------------------------------------
// Plan validation.
// ---------------------------------------------------------------------------

const HINT_INVALID_SCHEMA: &str = "candidate plan does not match the strict schema";
const HINT_OUT_OF_BOUNDS: &str = "position is outside the frozen projection";
const HINT_UNKNOWN_PLAYER: &str = "follow target is not online in this snapshot";
const HINT_UNMINEABLE: &str = "target is not mineable in this snapshot";
const HINT_UNKNOWN_BLOCK: &str = "block name is not in the place registry";
const HINT_MISSING_ITEM: &str = "required place item is missing from inventory";
const HINT_SNAPSHOT_MISMATCH: &str = "snapshot digest does not match the frozen view";

fn validator_failure(
    lease: &SnapshotLease,
    code: &str,
    hint: &str,
) -> Result<ToolSuccess, ToolFault> {
    let hint = if hint.is_empty() || hint.len() > 256 {
        "candidate plan was rejected"
    } else {
        hint
    };
    let mut escaped = Vec::new();
    escape_into(&mut escaped, hint);
    let hint_text = std::str::from_utf8(&escaped).map_err(|_| ToolFault::Unavailable)?;
    encode_tool(
        lease,
        "validate_plan",
        &format!("{{\"accepted\":false,\"code\":\"{code}\",\"hint\":{hint_text}}}"),
        true,
    )
}

struct WireStep {
    kind: String,
    x: Option<i64>,
    y: Option<i64>,
    z: Option<i64>,
    block: Option<String>,
    player_id: Option<String>,
    appeared: Vec<String>,
}

impl WireStep {
    fn has(&self, field: &str) -> bool {
        self.appeared.iter().any(|known| known == field)
    }
}

enum DecodedStep {
    GoTo { x: i32, y: i32, z: i32 },
    Mine { x: i32, y: i32, z: i32 },
    Place { x: i32, y: i32, z: i32, block: u16 },
    Follow { player: [u8; 16] },
}

fn decode_wire_step(step: &JsonVal) -> Result<WireStep, ToolFault> {
    let entries = step.as_object().ok_or(ToolFault::InvalidInput)?;
    let mut wire = WireStep {
        kind: String::new(),
        x: None,
        y: None,
        z: None,
        block: None,
        player_id: None,
        appeared: Vec::with_capacity(entries.len()),
    };
    for (key, value) in entries {
        wire.appeared.push(key.clone());
        match key.as_str() {
            "kind" => {
                if !value.is_null() {
                    wire.kind = value.as_str().ok_or(ToolFault::InvalidInput)?.to_owned();
                }
            }
            "x" => wire.x = decode_wire_int(value)?,
            "y" => wire.y = decode_wire_int(value)?,
            "z" => wire.z = decode_wire_int(value)?,
            "block" => {
                wire.block = if value.is_null() {
                    None
                } else {
                    Some(value.as_str().ok_or(ToolFault::InvalidInput)?.to_owned())
                };
            }
            "player_id" => {
                wire.player_id = if value.is_null() {
                    None
                } else {
                    Some(value.as_str().ok_or(ToolFault::InvalidInput)?.to_owned())
                };
            }
            _ => return Err(ToolFault::InvalidInput),
        }
    }
    Ok(wire)
}

fn decode_wire_int(value: &JsonVal) -> Result<Option<i64>, ToolFault> {
    if value.is_null() {
        return Ok(None);
    }
    value.as_i64().map(Some).ok_or(ToolFault::InvalidInput)
}

fn narrow_position(value: i64) -> Result<i32, ToolFault> {
    i32::try_from(value).map_err(|_| ToolFault::InvalidInput)
}

fn decode_plan_step(step: &WireStep) -> Result<DecodedStep, ToolFault> {
    match step.kind.as_str() {
        "go_to" | "mine" => {
            if step.has("block") || step.has("player_id") {
                return Err(ToolFault::InvalidInput);
            }
            let (x, y, z) = (
                step.x.ok_or(ToolFault::InvalidInput)?,
                step.y.ok_or(ToolFault::InvalidInput)?,
                step.z.ok_or(ToolFault::InvalidInput)?,
            );
            let (x, y, z) = (
                narrow_position(x)?,
                narrow_position(y)?,
                narrow_position(z)?,
            );
            if step.kind == "mine" {
                Ok(DecodedStep::Mine { x, y, z })
            } else {
                Ok(DecodedStep::GoTo { x, y, z })
            }
        }
        "place" => {
            if step.has("player_id") {
                return Err(ToolFault::InvalidInput);
            }
            let (x, y, z) = (
                narrow_position(step.x.ok_or(ToolFault::InvalidInput)?)?,
                narrow_position(step.y.ok_or(ToolFault::InvalidInput)?)?,
                narrow_position(step.z.ok_or(ToolFault::InvalidInput)?)?,
            );
            if !step.has("block") || step.block.is_none() {
                return Err(ToolFault::InvalidInput);
            }
            let (block, _) = place_entry(step.block.as_deref().unwrap_or_default())
                .ok_or(ToolFault::InvalidInput)?;
            Ok(DecodedStep::Place { x, y, z, block })
        }
        "follow" => {
            if step.has("x") || step.has("y") || step.has("z") || step.has("block") {
                return Err(ToolFault::InvalidInput);
            }
            if !step.has("player_id") || step.player_id.is_none() {
                return Err(ToolFault::InvalidInput);
            }
            let bytes = crate::agent::http::parse_canonical_uuid(
                step.player_id.as_deref().unwrap_or_default(),
            )
            .ok_or(ToolFault::InvalidInput)?;
            Ok(DecodedStep::Follow { player: bytes })
        }
        _ => Err(ToolFault::InvalidInput),
    }
}

fn validate_plan_tool(lease: &SnapshotLease, input: &[u8]) -> Result<ToolSuccess, ToolFault> {
    if input.len() > MAX_PLAN_INPUT_BYTES {
        return validator_failure(lease, "invalid_schema", HINT_INVALID_SCHEMA);
    }
    let fields = strict_object(input, &["plan"])?;
    let Some(plan) = fields.get("plan").filter(|value| !value.is_null()) else {
        return validator_failure(lease, "invalid_schema", HINT_INVALID_SCHEMA);
    };
    tool_checkpoint(lease)?;
    let canonical = JsonVal::parse(input).map_err(|_| ToolFault::InvalidInput)?;
    if canonical.write_sorted().len() > MAX_PLAN_INPUT_BYTES {
        return validator_failure(lease, "invalid_schema", HINT_INVALID_SCHEMA);
    }
    tool_checkpoint(lease)?;
    let Some(entries) = plan.as_object() else {
        return validator_failure(lease, "invalid_schema", HINT_INVALID_SCHEMA);
    };
    if entries
        .iter()
        .any(|(key, _)| key != "summary" && key != "steps")
    {
        return validator_failure(lease, "invalid_schema", HINT_INVALID_SCHEMA);
    }
    let summary = plan
        .get("summary")
        .and_then(|value| value.as_str())
        .unwrap_or_default();
    if !text_valid(summary, 512, true) {
        return validator_failure(lease, "invalid_schema", HINT_INVALID_SCHEMA);
    }
    let steps = plan
        .get("steps")
        .and_then(|value| value.as_array())
        .unwrap_or(&[]);
    if steps.is_empty() || steps.len() > 5000 {
        return validator_failure(lease, "invalid_schema", HINT_INVALID_SCHEMA);
    }
    let mut decoded = Vec::with_capacity(steps.len());
    for step in steps {
        tool_checkpoint(lease)?;
        let wire = match decode_wire_step(step) {
            Ok(wire) => wire,
            Err(_) => return validator_failure(lease, "invalid_schema", HINT_INVALID_SCHEMA),
        };
        // A fully addressed place step with an unknown block name is a domain
        // refusal; every other place shape error is a schema violation.
        if wire.kind == "place"
            && wire.x.is_some()
            && wire.y.is_some()
            && wire.z.is_some()
            && wire.block.is_some()
            && !wire.has("player_id")
            && place_entry(wire.block.as_deref().unwrap_or_default()).is_none()
        {
            return validator_failure(lease, "unknown_block", HINT_UNKNOWN_BLOCK);
        }
        match decode_plan_step(&wire) {
            Ok(step) => {
                // Coordinate steps outside the world span are domain
                // refusals, matching the planner's out-of-bounds code.
                let y = match step {
                    DecodedStep::GoTo { y, .. }
                    | DecodedStep::Mine { y, .. }
                    | DecodedStep::Place { y, .. } => Some(y),
                    DecodedStep::Follow { .. } => None,
                };
                if y.is_some_and(|y| !(BLOCK_Y_MIN..BLOCK_Y_MAX).contains(&y)) {
                    return validator_failure(lease, "out_of_bounds", HINT_OUT_OF_BOUNDS);
                }
                decoded.push(step);
            }
            Err(_) => return validator_failure(lease, "invalid_schema", HINT_INVALID_SCHEMA),
        }
    }
    // Structural sequence check: follow may only close the plan.
    for (index, step) in decoded.iter().enumerate() {
        tool_checkpoint(lease)?;
        if matches!(step, DecodedStep::Follow { .. }) && index + 1 != decoded.len() {
            return validator_failure(lease, "invalid_schema", HINT_INVALID_SCHEMA);
        }
    }
    tool_checkpoint(lease)?;
    // A corrupt frozen view reports a mismatch; cancellation aborts through
    // the checkpoints above instead.
    let expected = lease.digest();
    let actual = match canonical_snapshot_digest(lease.snapshot()) {
        Ok((_, digest)) => digest,
        Err(_) => return validator_failure(lease, "snapshot_mismatch", HINT_SNAPSHOT_MISMATCH),
    };
    if !constant_time_eq(&expected, &actual) {
        return validator_failure(lease, "snapshot_mismatch", HINT_SNAPSHOT_MISMATCH);
    }
    tool_checkpoint(lease)?;
    let snapshot = lease.snapshot();
    for step in &decoded {
        tool_checkpoint(lease)?;
        match step {
            DecodedStep::Follow { player } => {
                let online = snapshot
                    .online_players
                    .iter()
                    .any(|entry| entry.player_id.bytes() == *player);
                if !online {
                    return validator_failure(lease, "unknown_player", HINT_UNKNOWN_PLAYER);
                }
            }
            DecodedStep::Mine { x, y, z } => {
                let Some((block, _)) = terrain_lookup(lease, *x, *y, *z) else {
                    return validator_failure(lease, "out_of_bounds", HINT_OUT_OF_BOUNDS);
                };
                if !mineable_block(block) {
                    return validator_failure(lease, "unmineable_target", HINT_UNMINEABLE);
                }
            }
            DecodedStep::Place { block, .. } => {
                let name = block_name(*block).ok_or(ToolFault::Unavailable)?;
                let (_, item) = place_entry(name).ok_or(ToolFault::Unavailable)?;
                let holds = snapshot
                    .companion
                    .inventory
                    .iter()
                    .any(|stack| stack.item == item && stack.count >= 1);
                if !holds {
                    return validator_failure(lease, "missing_item", HINT_MISSING_ITEM);
                }
            }
            DecodedStep::GoTo { .. } => {}
        }
    }
    let mut plan_out = String::from("{\"accepted\":true,\"plan\":{\"steps\":[");
    for (index, step) in decoded.iter().enumerate() {
        tool_checkpoint(lease)?;
        if index > 0 {
            plan_out.push(',');
        }
        match step {
            DecodedStep::GoTo { x, y, z } => {
                plan_out.push_str(&format!(
                    "{{\"kind\":\"go_to\",\"x\":{x},\"y\":{y},\"z\":{z}}}"
                ));
            }
            DecodedStep::Mine { x, y, z } => {
                plan_out.push_str(&format!(
                    "{{\"kind\":\"mine\",\"x\":{x},\"y\":{y},\"z\":{z}}}"
                ));
            }
            DecodedStep::Place { x, y, z, block } => {
                let name = block_name(*block).ok_or(ToolFault::Unavailable)?;
                plan_out.push_str(&format!(
                    "{{\"block\":\"{name}\",\"kind\":\"place\",\"x\":{x},\"y\":{y},\"z\":{z}}}"
                ));
            }
            DecodedStep::Follow { player } => {
                plan_out.push_str(&format!(
                    "{{\"kind\":\"follow\",\"player_id\":\"{}\"}}",
                    uuid_text(*player)
                ));
            }
        }
    }
    let mut escaped = Vec::new();
    escape_into(&mut escaped, summary);
    let summary_text = std::str::from_utf8(&escaped).map_err(|_| ToolFault::Unavailable)?;
    plan_out.push_str(&format!(
        "],\"summary\":{summary_text}}},\"snapshot_digest\":\"{}\"}}",
        lease.digest_hex()
    ));
    encode_tool(lease, "validate_plan", &plan_out, false)
}

fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    if left.len() != right.len() {
        return false;
    }
    let mut diff = 0u8;
    for (a, b) in left.iter().zip(right.iter()) {
        diff |= a ^ b;
    }
    diff == 0
}

// ---------------------------------------------------------------------------
// HTTP service.
// ---------------------------------------------------------------------------

struct ServiceInner {
    registry: SnapshotRegistry,
    tools: Arc<dyn PlanningTools>,
    contract: McpContract,
    authority: String,
    origin: String,
    endpoint: String,
    closing: AtomicBool,
    done: mpsc::Sender<Result<(), String>>,
}

/// Stateless MCP service on a loopback listener. `close` is idempotent and
/// settles the serve channel exactly once.
pub struct McpService {
    inner: Arc<ServiceInner>,
    accept: Mutex<Option<std::thread::JoinHandle<()>>>,
    done: Mutex<Option<mpsc::Receiver<Result<(), String>>>>,
}

impl McpService {
    /// Binds an ephemeral loopback listener with the frozen tool runner.
    pub fn try_new(registry: SnapshotRegistry) -> Result<Self, ServerError> {
        let listener = TcpListener::bind("127.0.0.1:0").map_err(|_| invalid("mcp_listen"))?;
        Self::try_new_with(registry, Arc::new(FrozenTools), listener)
    }

    /// Serves on an already-bound listener with an injected tool runner.
    pub fn try_new_with(
        registry: SnapshotRegistry,
        tools: Arc<dyn PlanningTools>,
        listener: TcpListener,
    ) -> Result<Self, ServerError> {
        let contract = load_contract()?;
        let authority = listener
            .local_addr()
            .map_err(|_| invalid("mcp_listen"))?
            .to_string();
        let origin = format!("http://{authority}");
        let endpoint = format!("http://{authority}{MCP_ENDPOINT_PATH}");
        let (done_tx, done_rx) = mpsc::channel();
        let service = Self {
            inner: Arc::new(ServiceInner {
                registry,
                tools,
                contract,
                authority,
                origin,
                endpoint,
                closing: AtomicBool::new(false),
                done: done_tx,
            }),
            accept: Mutex::new(None),
            done: Mutex::new(None),
        };
        let inner = service.inner.clone();
        let handle = std::thread::Builder::new()
            .name("mornlea-mcp-accept".to_owned())
            .spawn(move || serve_loop(inner, listener))
            .map_err(|_| invalid("mcp_listen"))?;
        *service.accept.lock().unwrap() = Some(handle);
        *service.done.lock().unwrap() = Some(done_rx);
        Ok(service)
    }

    pub fn endpoint(&self) -> &str {
        &self.inner.endpoint
    }

    pub fn authority(&self) -> &str {
        &self.inner.authority
    }

    /// Idempotent close: the registry goes first so outstanding read leases
    /// die before the HTTP listener, then the accept loop is joined.
    pub fn close(&self) {
        if self.inner.closing.swap(true, Ordering::SeqCst) {
            return;
        }
        self.inner.registry.close_shared();
        if let Some(handle) = self.accept.lock().unwrap().take() {
            let _ = handle.join();
        }
    }

    /// Waits for the serve loop to settle: `Some(Ok)` on clean close,
    /// `Some(Err)` on a serve failure that never touches the world, and
    /// `None` on timeout while still serving.
    pub fn wait_done(&self, timeout: Duration) -> Option<Result<(), String>> {
        let guard = self.done.lock().unwrap();
        guard.as_ref()?.recv_timeout(timeout).ok()
    }
}

fn serve_loop(inner: Arc<ServiceInner>, listener: TcpListener) {
    listener.set_nonblocking(true).unwrap_or_default();
    loop {
        if inner.closing.load(Ordering::SeqCst) {
            let _ = inner.done.send(Ok(()));
            return;
        }
        match listener.accept() {
            Ok((stream, _)) => {
                let inner = inner.clone();
                let _ = std::thread::Builder::new()
                    .name("mornlea-mcp-connection".to_owned())
                    .spawn(move || serve_connection(&inner, stream));
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(10));
            }
            Err(error) => {
                let _ = inner.done.send(Err(error.to_string()));
                return;
            }
        }
    }
}

struct RequestHead {
    method: String,
    path: String,
    query: String,
    headers: Vec<(String, String)>,
}

fn header_values<'a>(headers: &'a [(String, String)], name: &str) -> Vec<&'a str> {
    headers
        .iter()
        .filter(|(key, _)| key.eq_ignore_ascii_case(name))
        .map(|(_, value)| value.as_str())
        .collect()
}

fn read_head(stream: &mut TcpStream) -> Result<RequestHead, OuterError> {
    stream
        .set_read_timeout(Some(MCP_READ_HEADER_TIMEOUT))
        .map_err(|_| OuterError::unavailable(503))?;
    let mut raw = Vec::new();
    let mut byte = [0u8; 1];
    loop {
        if raw.len() > MAX_MCP_HEADER_BYTES {
            return Err(OuterError::shaped(431, "invalid_request"));
        }
        match stream.read(&mut byte) {
            Ok(0) => return Err(OuterError::shaped(400, "invalid_request")),
            Ok(_) => {
                raw.push(byte[0]);
                if raw.ends_with(b"\r\n\r\n") {
                    break;
                }
            }
            Err(error)
                if error.kind() == std::io::ErrorKind::WouldBlock
                    || error.kind() == std::io::ErrorKind::TimedOut =>
            {
                return Err(OuterError::unavailable(503));
            }
            Err(_) => return Err(OuterError::unavailable(503)),
        }
    }
    let text = String::from_utf8(raw).map_err(|_| OuterError::shaped(400, "invalid_request"))?;
    let mut lines = text.lines();
    let request_line = lines
        .next()
        .ok_or_else(|| OuterError::shaped(400, "invalid_request"))?;
    let mut parts = request_line.split_whitespace();
    let (method, target, version) = (
        parts
            .next()
            .ok_or_else(|| OuterError::shaped(400, "invalid_request"))?
            .to_owned(),
        parts
            .next()
            .ok_or_else(|| OuterError::shaped(400, "invalid_request"))?
            .to_owned(),
        parts
            .next()
            .ok_or_else(|| OuterError::shaped(400, "invalid_request"))?,
    );
    if parts.next().is_some() || (version != "HTTP/1.1" && version != "HTTP/1.0") {
        return Err(OuterError::shaped(400, "invalid_request"));
    }
    let (path, query) = match target.split_once('?') {
        Some((path, query)) => (path.to_owned(), query.to_owned()),
        None => (target, String::new()),
    };
    // Origin-form targets carry the path directly; absolute-form targets
    // name the same loopback authority the Host check pins below.
    let path = match path.strip_prefix("http://") {
        Some(rest) => rest
            .split_once('/')
            .map(|(_, tail)| format!("/{tail}"))
            .unwrap_or_default(),
        None => path,
    };
    let mut headers = Vec::new();
    for line in lines {
        if line.is_empty() {
            continue;
        }
        let (name, value) = line
            .split_once(':')
            .ok_or_else(|| OuterError::shaped(400, "invalid_request"))?;
        headers.push((name.trim().to_owned(), value.trim().to_owned()));
    }
    Ok(RequestHead {
        method,
        path,
        query,
        headers,
    })
}

struct OuterError {
    status: u16,
    code: &'static str,
}

impl OuterError {
    fn shaped(status: u16, code: &'static str) -> Self {
        Self { status, code }
    }

    fn unavailable(status: u16) -> Self {
        Self {
            status,
            code: "unavailable",
        }
    }
}

fn reason(status: u16) -> &'static str {
    match status {
        200 => "OK",
        202 => "Accepted",
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        413 => "Request Entity Too Large",
        415 => "Unsupported Media Type",
        431 => "Request Header Fields Too Large",
        502 => "Bad Gateway",
        503 => "Service Unavailable",
        _ => "Error",
    }
}

fn respond(stream: &mut TcpStream, status: u16, extra: &[(String, String)], body: &[u8]) {
    let _ = stream.set_write_timeout(Some(MCP_WRITE_TIMEOUT));
    let mut head = format!(
        "HTTP/1.1 {status} {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n",
        reason(status),
        body.len()
    );
    for (name, value) in extra {
        head.push_str(&format!("{name}: {value}\r\n"));
    }
    head.push_str("\r\n");
    let _ = stream.write_all(head.as_bytes());
    let _ = stream.write_all(body);
    let _ = stream.flush();
}

fn respond_error(stream: &mut TcpStream, error: &OuterError) {
    let mut extra = Vec::new();
    if error.status == 405 {
        extra.push(("Allow".to_owned(), "POST".to_owned()));
    }
    respond(
        stream,
        error.status,
        &extra,
        format!("{{\"error\":{{\"code\":\"{}\"}}}}", error.code).as_bytes(),
    );
}

struct Envelope {
    id_raw: String,
    method: String,
    params: Option<JsonVal>,
}

fn parse_envelope(body: &[u8], protocol_headers: &[&str]) -> Result<Envelope, ()> {
    let trimmed = trim_json(body);
    if trimmed.is_empty() || trimmed[0] != b'{' {
        return Err(());
    }
    let value = JsonVal::parse(trimmed).map_err(|_| ())?;
    let entries = value.as_object().ok_or(())?;
    for (key, _) in entries {
        match key.as_str() {
            "jsonrpc" | "id" | "method" | "params" => {}
            _ => return Err(()),
        }
    }
    let get = |key: &str| {
        entries
            .iter()
            .find(|(known, _)| known == key)
            .map(|(_, value)| value)
    };
    let jsonrpc = get("jsonrpc").and_then(|value| value.as_str()).ok_or(())?;
    if jsonrpc != "2.0" {
        return Err(());
    }
    let method = get("method").and_then(|value| value.as_str()).ok_or(())?;
    if ![
        "initialize",
        "notifications/initialized",
        "tools/list",
        "tools/call",
    ]
    .contains(&method)
    {
        return Err(());
    }
    let id_raw = match get("id") {
        Some(id) => {
            if method == "notifications/initialized" {
                return Err(());
            }
            Some(canonical_id(id)?)
        }
        None => {
            if method != "notifications/initialized" {
                return Err(());
            }
            None
        }
    };
    let params = match get("params") {
        Some(params) => {
            if params.as_object().is_none() {
                return Err(());
            }
            Some(params.clone())
        }
        None => {
            if method == "initialize" || method == "tools/call" {
                return Err(());
            }
            None
        }
    };
    if method == "initialize" {
        if protocol_headers.len() > 1
            || (protocol_headers.len() == 1 && protocol_headers[0] != MCP_PROTOCOL_VERSION)
        {
            return Err(());
        }
        let version = params
            .as_ref()
            .and_then(|params| params.get("protocolVersion"))
            .and_then(|value| value.as_str())
            .ok_or(())?;
        if version != MCP_PROTOCOL_VERSION {
            return Err(());
        }
    } else if protocol_headers != [MCP_PROTOCOL_VERSION] {
        return Err(());
    }
    Ok(Envelope {
        id_raw: id_raw.unwrap_or_default(),
        method: method.to_owned(),
        params,
    })
}

/// Request ids are nonempty strings or JSON integers, echoed verbatim.
fn canonical_id(id: &JsonVal) -> Result<String, ()> {
    match id {
        JsonVal::Str(text) if !text.is_empty() => {
            let mut escaped = Vec::new();
            escape_into(&mut escaped, text);
            String::from_utf8(escaped).map_err(|_| ())
        }
        JsonVal::Number(number) if number.int().is_some() => Ok(number.text().to_owned()),
        _ => Err(()),
    }
}

fn trim_json(body: &[u8]) -> &[u8] {
    let mut start = 0;
    while start < body.len() && matches!(body[start], b' ' | b'\t' | b'\n' | b'\r') {
        start += 1;
    }
    let mut end = body.len();
    while end > start && matches!(body[end - 1], b' ' | b'\t' | b'\n' | b'\r') {
        end -= 1;
    }
    &body[start..end]
}

/// Reads exactly the declared body, or discards up to one byte past the cap
/// and reports over-limit without consuming an unbounded sender.
fn read_body_limited(
    stream: &mut TcpStream,
    declared: usize,
) -> Result<Option<Vec<u8>>, OuterError> {
    if declared > MAX_MCP_REQUEST_BYTES {
        let mut discard = vec![0u8; 4096];
        let mut remaining = MAX_MCP_REQUEST_BYTES + 1;
        while remaining > 0 {
            let want = remaining.min(discard.len());
            match stream.read(&mut discard[..want]) {
                Ok(0) => break,
                Ok(read) => remaining -= read,
                Err(_) => break,
            }
        }
        return Ok(None);
    }
    let mut body = vec![0u8; declared];
    stream
        .read_exact(&mut body)
        .map_err(|_| OuterError::shaped(400, "invalid_request"))?;
    Ok(Some(body))
}

fn content_type_is_json(headers: &[(String, String)]) -> bool {
    let values = header_values(headers, "Content-Type");
    if values.len() != 1 {
        return false;
    }
    let media = values[0].split(';').next().unwrap_or_default().trim();
    media.eq_ignore_ascii_case("application/json")
}

fn serve_connection(inner: &Arc<ServiceInner>, mut stream: TcpStream) {
    // Accepted sockets inherit the listener nonblocking mode, while the
    // request path assumes blocking reads with timeouts. Restore blocking
    // mode so an early read waits for bytes instead of surfacing
    // `WouldBlock` as a bogus 503 or 400.
    if stream.set_nonblocking(false).is_err() {
        let refused = OuterError::unavailable(503);
        respond_error(&mut stream, &refused);
        return;
    }
    let result = serve_request(inner, &mut stream);
    if let Err(error) = result {
        respond_error(&mut stream, &error);
    }
}

fn serve_request(inner: &Arc<ServiceInner>, stream: &mut TcpStream) -> Result<(), OuterError> {
    let head = read_head(stream)?;
    // The body is consumed (up to the cap) before any refusal so a refused
    // connection still closes cleanly instead of resetting with unread
    // bytes. Check order below is unchanged, so every status is identical.
    let lengths = header_values(&head.headers, "Content-Length");
    if lengths.len() != 1 {
        return Err(OuterError::shaped(400, "invalid_request"));
    }
    let declared = lengths[0]
        .parse::<usize>()
        .map_err(|_| OuterError::shaped(400, "invalid_request"))?;
    stream
        .set_read_timeout(Some(MCP_READ_TIMEOUT))
        .map_err(|_| OuterError::unavailable(503))?;
    let body = read_body_limited(stream, declared)?;
    let Some(body) = body else {
        return Err(OuterError::shaped(413, "request_too_large"));
    };
    if head.path != MCP_ENDPOINT_PATH || !head.query.is_empty() {
        return Err(OuterError::shaped(404, "not_found"));
    }
    if head.method != "POST" {
        return Err(OuterError::shaped(405, "method_not_allowed"));
    }
    let hosts = header_values(&head.headers, "Host");
    if hosts.len() != 1 || hosts[0] != inner.authority {
        return Err(OuterError::shaped(403, "forbidden"));
    }
    let origins = header_values(&head.headers, "Origin");
    if origins.len() > 1 || (origins.len() == 1 && origins[0] != inner.origin) {
        return Err(OuterError::shaped(403, "forbidden"));
    }
    let authorizations = header_values(&head.headers, "Authorization");
    if authorizations.len() != 1 || !authorizations[0].starts_with("Bearer ") {
        return Err(OuterError::shaped(401, "unauthorized"));
    }
    let capability = authorizations[0].trim_start_matches("Bearer ");
    if capability.is_empty()
        || capability.len() > 512
        || capability.contains([' ', '\t', '\r', '\n'])
    {
        return Err(OuterError::shaped(401, "unauthorized"));
    }
    let auth = inner
        .registry
        .authorize(capability)
        .map_err(|_| OuterError::shaped(401, "unauthorized"))?;
    if !content_type_is_json(&head.headers) {
        return Err(OuterError::shaped(415, "invalid_content_type"));
    }
    std::str::from_utf8(&body).map_err(|_| OuterError::shaped(400, "invalid_request"))?;
    let protocols = header_values(&head.headers, "Mcp-Protocol-Version");
    let envelope = parse_envelope(&body, &protocols)
        .map_err(|_| OuterError::shaped(400, "invalid_request"))?;
    if inner.closing.load(Ordering::SeqCst) {
        return Err(OuterError::unavailable(503));
    }
    let lease = inner
        .registry
        .materialize(&auth)
        .map_err(|_| OuterError::unavailable(503))?;
    if lease.checkpoint().is_err() || inner.closing.load(Ordering::SeqCst) {
        return Err(OuterError::unavailable(503));
    }
    // The SDK-equivalent layer: Accept first, then method dispatch.
    let accepts: Vec<&str> = header_values(&head.headers, "Accept");
    let accepted = accepts
        .iter()
        .any(|value| value.contains("text/event-stream"));
    if !accepted {
        return Err(OuterError::shaped(400, "invalid_request"));
    }
    match envelope.method.as_str() {
        "initialize" => {
            let body = format!(
                "{{\"id\":{},\"jsonrpc\":\"2.0\",\"result\":{{\"capabilities\":{{\"tools\":{{\"listChanged\":false}}}},\"protocolVersion\":\"{MCP_PROTOCOL_VERSION}\",\"serverInfo\":{{\"name\":\"mornlea-companion-agent-mcp\",\"version\":\"{MCP_APP_VERSION}\"}}}}}}",
                envelope.id_raw
            );
            commit(inner, &lease, stream, 200, body.as_bytes())
        }
        "notifications/initialized" => {
            let _ = stream.set_write_timeout(Some(MCP_WRITE_TIMEOUT));
            let _ = stream.write_all(
                b"HTTP/1.1 202 Accepted\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
            );
            let _ = stream.flush();
            Ok(())
        }
        "tools/list" => {
            let mut tools_json = String::from("[");
            for (index, tool) in inner.contract.tools.iter().enumerate() {
                if index > 0 {
                    tools_json.push(',');
                }
                tools_json.push_str(&format!(
                    "{{\"inputSchema\":{},\"name\":\"{}\",\"outputSchema\":{}}}",
                    tool.input_schema, tool.name, tool.output_schema
                ));
            }
            tools_json.push(']');
            let body = format!(
                "{{\"id\":{},\"jsonrpc\":\"2.0\",\"result\":{{\"tools\":{tools_json}}}}}",
                envelope.id_raw
            );
            commit(inner, &lease, stream, 200, body.as_bytes())
        }
        "tools/call" => {
            let params = envelope
                .params
                .as_ref()
                .ok_or_else(|| OuterError::shaped(400, "invalid_request"))?;
            // A missing, mistyped, or unknown tool name is a controlled
            // method error, never an outer refusal or an input echo.
            let name = params
                .get("name")
                .and_then(|value| value.as_str())
                .unwrap_or_default();
            if tool_canonical_limit(name).is_none() {
                return method_error(stream, &envelope.id_raw);
            }
            call_tool(inner, &lease, stream, &envelope.id_raw, name, params)
        }
        _ => Err(OuterError::shaped(400, "invalid_request")),
    }
}

/// JSON-RPC method error with the controlled payload: no input echo.
fn method_error(stream: &mut TcpStream, id_raw: &str) -> Result<(), OuterError> {
    let body = format!(
        "{{\"error\":{{\"code\":-32603,\"message\":\"unavailable\"}},\"id\":{id_raw},\"jsonrpc\":\"2.0\"}}"
    );
    if body.len() > MAX_MCP_RESPONSE_BYTES {
        return Err(OuterError::unavailable(502));
    }
    respond(stream, 200, &[], body.as_bytes());
    Ok(())
}

fn call_tool(
    inner: &Arc<ServiceInner>,
    lease: &SnapshotLease,
    stream: &mut TcpStream,
    id_raw: &str,
    name: &str,
    params: &JsonVal,
) -> Result<(), OuterError> {
    let Some(arguments) = params.get("arguments") else {
        return tool_result_error(stream, id_raw);
    };
    if arguments.as_object().is_none() {
        return tool_result_error(stream, id_raw);
    }
    let raw = arguments.write_sorted();
    match inner.tools.execute(lease, name, raw.as_bytes()) {
        Ok(outcome) => {
            // Structured content and text carry the same canonical value; the
            // error flag is omitted when false.
            let canonical =
                String::from_utf8(outcome.canonical).map_err(|_| OuterError::unavailable(502))?;
            let mut text = Vec::new();
            escape_into(&mut text, &canonical);
            let text = String::from_utf8(text).map_err(|_| OuterError::unavailable(502))?;
            let body = format!(
                "{{\"id\":{id_raw},\"jsonrpc\":\"2.0\",\"result\":{{\"content\":[{{\"text\":{text},\"type\":\"text\"}}],\"structuredContent\":{canonical}}}}}"
            );
            commit(inner, lease, stream, 200, body.as_bytes())
        }
        Err(ToolFault::InvalidInput | ToolFault::TooLarge) => tool_result_error(stream, id_raw),
        Err(ToolFault::Unavailable) => Err(OuterError::unavailable(502)),
    }
}

/// Tool-level failure: the result carries `isError: true` with the fixed
/// payload and no structured content.
fn tool_result_error(stream: &mut TcpStream, id_raw: &str) -> Result<(), OuterError> {
    let body = format!(
        "{{\"id\":{id_raw},\"jsonrpc\":\"2.0\",\"result\":{{\"content\":[{{\"text\":\"{{\\\"code\\\":\\\"unavailable\\\"}}\",\"type\":\"text\"}}],\"isError\":true}}}}"
    );
    // The fixed payload is tiny; the bound below only guards the envelope.
    if body.len() > MAX_MCP_RESPONSE_BYTES {
        return Err(OuterError::unavailable(502));
    }
    respond(stream, 200, &[], body.as_bytes());
    Ok(())
}

/// Commits a response body after rechecking validity, the wire cap, and
/// cancellation. The body is fully buffered before this point, matching the
/// Go recorder: nothing commits after a cancellation.
fn commit(
    inner: &Arc<ServiceInner>,
    lease: &SnapshotLease,
    stream: &mut TcpStream,
    status: u16,
    body: &[u8],
) -> Result<(), OuterError> {
    if JsonVal::parse(body).is_err() {
        return Err(OuterError::unavailable(502));
    }
    if body.len() > MAX_MCP_RESPONSE_BYTES {
        return Err(OuterError::unavailable(502));
    }
    if lease.checkpoint().is_err() || inner.closing.load(Ordering::SeqCst) {
        return Err(OuterError::unavailable(502));
    }
    respond(stream, status, &[], body);
    Ok(())
}
