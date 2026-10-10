//! Go-generated fixtures pin the companion planning projections.
//!
//! Each fixture under `packages/server/server/testdata/companion_planning/`
//! records the inputs and the Go `buildPlanSnapshot` output produced by
//! `TestCompanionPlanningProjectionFixtures`. The authority is rebuilt from
//! the same inputs here and every projected part must match exactly.
use super::*;
use mornlea_domain::{MotionStateParts, SurvivalState, SurvivalStateParts};
use mornlea_storage::{ContainerSnapshot, ItemStack, StorageKind};
use serde_json::Value;

pub(crate) const FIXTURES: [(&str, &str); 3] = [
    (
        "running_surface_capped",
        include_str!(
            "../../../../../server/server/testdata/companion_planning/running_surface_capped.json"
        ),
    ),
    (
        "planning_world_bottom_sparse",
        include_str!(
            "../../../../../server/server/testdata/companion_planning/planning_world_bottom_sparse.json"
        ),
    ),
    (
        "queued_negative_world_top",
        include_str!(
            "../../../../../server/server/testdata/companion_planning/queued_negative_world_top.json"
        ),
    ),
];

/// One rebuilt fixture authority with the companion under test.
pub(crate) struct PlanningFixture {
    pub(crate) authority: AuthorityState,
    pub(crate) companion: CompanionId,
    pub(crate) expected: Value,
}

fn uuid(text: &str) -> [u8; 16] {
    let hex: Vec<u8> = text.bytes().filter(|byte| *byte != b'-').collect();
    assert_eq!(hex.len(), 32, "uuid {text}");
    let mut bytes = [0u8; 16];
    for (index, pair) in hex.chunks(2).enumerate() {
        bytes[index] = u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap();
    }
    bytes
}

fn base64(text: &str) -> Vec<u8> {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = Vec::with_capacity(text.len() / 4 * 3);
    let mut buffer = 0u32;
    let mut bits = 0;
    for byte in text.bytes().filter(|byte| *byte != b'=') {
        let value = ALPHABET.iter().position(|c| *c == byte).expect("base64") as u32;
        buffer = (buffer << 6) | value;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((buffer >> bits) as u8);
            buffer &= (1 << bits) - 1;
        }
    }
    out
}

/// Go encodes empty slices as null; both mean no entries.
fn items(value: &Value) -> &[Value] {
    match value {
        Value::Null => &[],
        value => value.as_array().expect("array"),
    }
}

fn i32_of(value: &Value) -> i32 {
    i32::try_from(value.as_i64().expect("integer")).expect("i32")
}

fn f32_of(value: &Value) -> f32 {
    let wide = value.as_f64().expect("number");
    let narrow = wide as f32;
    assert_eq!(f64::from(narrow), wide, "fixture float is not exact f32");
    narrow
}

fn vec3(value: &Value) -> [f32; 3] {
    let items = value.as_array().expect("vec3");
    [f32_of(&items[0]), f32_of(&items[1]), f32_of(&items[2])]
}

fn look_hit(value: &Value) -> Option<BlockPos> {
    value
        .as_array()
        .map(|items| BlockPos::new(i32_of(&items[0]), i32_of(&items[1]), i32_of(&items[2])))
}

fn inventory(value: &Value) -> [ItemStack; 36] {
    let mut slots = [ItemStack::default(); 36];
    for slot in items(value) {
        slots[slot["slot"].as_u64().unwrap() as usize] = ItemStack {
            item: slot["item"].as_u64().unwrap() as u16,
            count: slot["count"].as_u64().unwrap() as u8,
            durability: slot["durability"].as_u64().unwrap() as u16,
        };
    }
    slots
}

/// Builds one compact chunk: layers fill every column, then explicit blocks.
fn fixture_chunk(input: &Value) -> Chunk {
    let chunk_x = i32_of(&input["x"]);
    let chunk_z = i32_of(&input["z"]);
    let mut cells = vec![0u16; 24 * 4096];
    let mut set = |x: i32, y: i32, z: i32, block: u16| {
        let index = mornlea_domain::chunk_block_index(BlockPos::new(x, y, z)) as usize;
        cells[index] = block;
    };
    for layer in input["layers"].as_array().into_iter().flatten() {
        for y in i32_of(&layer["fromY"])..=i32_of(&layer["toY"]) {
            for z in 0..16 {
                for x in 0..16 {
                    set(x, y, z, layer["block"].as_u64().unwrap() as u16);
                }
            }
        }
    }
    for block in input["blocks"].as_array().into_iter().flatten() {
        let block = block.as_array().unwrap();
        let (x, z) = (i32_of(&block[0]), i32_of(&block[2]));
        assert_eq!((x >> 4, z >> 4), (chunk_x, chunk_z), "fixture block chunk");
        set(x, i32_of(&block[1]), z, block[3].as_u64().unwrap() as u16);
    }
    let sections = cells
        .chunks(4096)
        .map(|section| {
            if section.iter().all(|cell| *cell == section[0]) {
                return ContainerSnapshot {
                    kind: StorageKind::Single,
                    bits: 0,
                    single: section[0],
                    palette: vec![],
                    packed: vec![],
                };
            }
            let mut packed = vec![0u64; 1024];
            for (index, cell) in section.iter().enumerate() {
                packed[index / 4] |= u64::from(*cell) << ((index % 4) * 15);
            }
            ContainerSnapshot {
                kind: StorageKind::Direct,
                bits: 15,
                single: 0,
                palette: vec![],
                packed,
            }
        })
        .collect();
    Chunk {
        sections,
        drops: vec![Default::default(); 32],
        furnaces: vec![Default::default(); 32],
        chests: vec![Default::default(); 16],
    }
}

fn survival() -> SurvivalState {
    SurvivalState::try_new(SurvivalStateParts {
        health: 20,
        oxygen: 300,
        hunger: 20,
        saturation_zero: false,
        armor_points: 0,
    })
    .unwrap()
}

fn motion(position: [f32; 3]) -> MotionState {
    MotionState::new(MotionStateParts {
        position: FiniteVec3::try_new(position).unwrap(),
        velocity: FiniteVec3::try_new([0.0; 3]).unwrap(),
        on_ground: true,
    })
}

fn admit(authority: &mut AuthorityState, id: PlayerId, name: &str) -> SessionKey {
    let start = mornlea_protocol::LoginStart::new(id, name, 8).unwrap();
    let admitted = mornlea_protocol::admit_login(
        mornlea_protocol::LoginStart::decode_inbound(&start.encode().unwrap()).unwrap(),
    )
    .unwrap();
    let session = authority.prepare(admitted, TransportKind::Memory).unwrap();
    authority.install(session, None).unwrap();
    authority.activate(session).unwrap();
    session
}

/// Rebuilds the fixture inputs as settled authority state.
pub(crate) fn planning_fixture(text: &str) -> PlanningFixture {
    let fixture: Value = serde_json::from_str(text).expect("fixture json");
    let input = &fixture["input"];
    let limits = ServerLimits::try_new(8, 4096, 512, 64, 64, 1_048_576).unwrap();
    let mut metadata = AuthorityState::try_new(limits, 42).unwrap().metadata;
    metadata.world_time_ticks = input["worldTimeTicks"].as_u64().unwrap();
    let mut authority = AuthorityState::try_new_with_metadata(limits, metadata).unwrap();
    // Go's engine tick counter counts completed steps, as `next_tick` does.
    authority.next_tick = input["ticks"].as_u64().expect("fixture ticks");
    let dimension = Dimension::new(input["dimension"].as_u64().unwrap() as u8).unwrap();

    for chunk in items(&input["chunks"]) {
        let key = ChunkKey {
            dimension,
            pos: ChunkPos::new(i32_of(&chunk["x"]), i32_of(&chunk["z"])),
        };
        let ready = ReadyChunk::try_new(
            key,
            1,
            chunk["revision"].as_u64().unwrap(),
            fixture_chunk(chunk),
        )
        .expect("fixture ready chunk");
        authority.residents.ready.insert(key, ready);
    }

    for (index, player) in items(&input["players"]).iter().enumerate() {
        let id = PlayerId::try_from_bytes(uuid(player["id"].as_str().unwrap())).unwrap();
        let session = admit(&mut authority, id, &format!("P{index}"));
        let position = vec3(&player["position"]);
        let mut save = canonical_player(id, &format!("P{index}")).unwrap();
        save.current.position = position;
        authority.residents.actors.push(
            ActorRecord::try_new(
                ActorKey::Player(session),
                ActorLifecycle::Active,
                Dimension::OVERWORLD,
                motion(position),
                LookAngles::try_new(f32_of(&player["yaw"]), f32_of(&player["pitch"])).unwrap(),
                survival(),
                ActorBody::Player(save),
            )
            .unwrap(),
        );
    }

    let body = &input["companion"];
    let companion = CompanionId::try_from_bytes(uuid(body["id"].as_str().unwrap())).unwrap();
    let position = vec3(&body["position"]);
    authority.residents.actors.push(
        ActorRecord::try_new(
            ActorKey::Companion(companion),
            ActorLifecycle::Active,
            dimension,
            motion(position),
            LookAngles::try_new(f32_of(&body["yaw"]), f32_of(&body["pitch"])).unwrap(),
            survival(),
            ActorBody::Companion(CompanionBody {
                id: StoredPlayerId::from_bytes(companion.bytes()),
                dimension: i32::from(dimension.get()),
                position,
                yaw: f32_of(&body["yaw"]),
                pitch: f32_of(&body["pitch"]),
                inventory: Default::default(),
            }),
        )
        .unwrap(),
    );
    let mut record = InventoryRecord::empty();
    record.slots = inventory(&body["inventory"]);
    authority
        .residents
        .inventories
        .insert(ActorKey::Companion(companion), record);

    let issuer = &input["issuer"];
    let issuer_id = PlayerId::try_from_bytes(uuid(issuer["id"].as_str().unwrap())).unwrap();
    authority
        .companion_chat
        .apply_configuration(&[(
            companion,
            CompanionName::try_from_canonical("Nova".into()).unwrap(),
        )])
        .unwrap();
    authority
        .companion_chat
        .try_admit(
            companion,
            CommandText::try_from_canonical(input["command"].as_str().unwrap().to_owned()).unwrap(),
            CompanionChatIssuer {
                session: None,
                player_id: issuer_id,
                player_name: DisplayName::try_from_canonical("Issuer".into()).unwrap(),
                position: FiniteVec3::try_new(vec3(&issuer["position"])).unwrap(),
                look: LookAngles::try_new(f32_of(&issuer["yaw"]), f32_of(&issuer["pitch"]))
                    .unwrap(),
                look_hit: look_hit(&issuer["lookHit"]),
            },
            0,
        )
        .unwrap();
    authority.companion_chat.promote_heads().unwrap();
    let task = input["task"].as_str().unwrap();
    if task != "queued" {
        let taken = authority
            .companion_chat
            .take_queued_for_planning(companion)
            .unwrap();
        if task == "running" {
            authority
                .companion_chat
                .commit_install(
                    companion,
                    taken.generation,
                    AgentPlan::try_new(
                        "fixture".into(),
                        vec![PlanStep::GoTo { x: 1, y: 64, z: 1 }],
                    )
                    .unwrap(),
                )
                .unwrap();
        }
    }
    PlanningFixture {
        authority,
        companion,
        expected: fixture["expected"].clone(),
    }
}

/// Decoded dense planes of one expected projection.
struct ExpectedTerrain {
    origin: BlockPos,
    ready: Vec<u8>,
    heights: Vec<i16>,
    blocks: Vec<u16>,
}

fn expected_terrain(expected: &Value) -> ExpectedTerrain {
    let origin = expected["origin"].as_array().unwrap();
    ExpectedTerrain {
        origin: BlockPos::new(i32_of(&origin[0]), i32_of(&origin[1]), i32_of(&origin[2])),
        ready: base64(expected["readyColumnsB64"].as_str().unwrap()),
        heights: base64(expected["heightsBEI16B64"].as_str().unwrap())
            .chunks(2)
            .map(|pair| i16::from_be_bytes([pair[0], pair[1]]))
            .collect(),
        blocks: base64(expected["blocksBEU16B64"].as_str().unwrap())
            .chunks(2)
            .map(|pair| u16::from_be_bytes([pair[0], pair[1]]))
            .collect(),
    }
}

fn expected_revisions(expected: &Value) -> Vec<SnapshotChunkRevision> {
    items(&expected["chunkRevisions"])
        .iter()
        .map(|revision| SnapshotChunkRevision {
            pos: ChunkPos::new(i32_of(&revision["x"]), i32_of(&revision["z"])),
            revision: revision["revision"].as_u64().unwrap(),
        })
        .collect()
}

fn expected_players(expected: &Value) -> Vec<SnapshotPlayer> {
    items(&expected["onlinePlayers"])
        .iter()
        .map(|player| SnapshotPlayer {
            player_id: PlayerId::try_from_bytes(uuid(player["id"].as_str().unwrap())).unwrap(),
            position: FiniteVec3::try_new(vec3(&player["position"])).unwrap(),
            look: LookAngles::try_new(f32_of(&player["yaw"]), f32_of(&player["pitch"])).unwrap(),
            look_hit: look_hit(&player["lookHit"]),
        })
        .collect()
}

#[test]
fn planning_snapshot_matches_go_fixtures() {
    for (name, text) in FIXTURES {
        let fixture = planning_fixture(text);
        let expected = &fixture.expected;
        let snapshot = fixture
            .authority
            .companion_planning_snapshot(fixture.companion)
            .unwrap_or_else(|error| panic!("{name}: projection failed: {error:?}"));
        let input: Value = serde_json::from_str(text).unwrap();
        let input = &input["input"];

        let terrain = expected_terrain(expected);
        assert_eq!(snapshot.terrain.origin, terrain.origin, "{name}: origin");
        assert_eq!(snapshot.terrain.dimensions, [33, 17, 33], "{name}: dims");
        assert_eq!(
            snapshot.terrain.ready_columns, terrain.ready,
            "{name}: ready columns"
        );
        let ready_count: u32 = snapshot
            .terrain
            .ready_columns
            .iter()
            .map(|byte| byte.count_ones())
            .sum();
        assert_eq!(
            u64::from(ready_count),
            expected["readyColumns"].as_u64().unwrap(),
            "{name}: ready column count"
        );
        assert_eq!(snapshot.terrain.heights, terrain.heights, "{name}: heights");
        for (index, (actual, wanted)) in snapshot
            .terrain
            .blocks
            .iter()
            .zip(terrain.blocks.iter())
            .enumerate()
        {
            assert_eq!(actual, wanted, "{name}: terrain block slot {index}");
        }
        assert_eq!(
            snapshot.terrain.blocks.len(),
            terrain.blocks.len(),
            "{name}: block count"
        );

        let exposed: Vec<SnapshotBlock> = items(&expected["exposedBlocks"])
            .iter()
            .map(|block| {
                let block = block.as_array().unwrap();
                SnapshotBlock {
                    position: BlockPos::new(
                        i32_of(&block[0]),
                        i32_of(&block[1]),
                        i32_of(&block[2]),
                    ),
                    block_id: block[3].as_u64().unwrap() as u16,
                }
            })
            .collect();
        assert_eq!(snapshot.exposed_blocks, exposed, "{name}: exposed blocks");
        assert_eq!(
            snapshot.chunk_revisions,
            expected_revisions(expected),
            "{name}: chunk revisions"
        );
        assert_eq!(
            snapshot.online_players,
            expected_players(expected),
            "{name}: online players"
        );
        assert_eq!(
            snapshot.companion.inventory,
            inventory(&expected["inventory"]),
            "{name}: inventory"
        );
        assert_eq!(
            snapshot.companion.task_status.as_str(),
            expected["taskStatus"].as_str().unwrap(),
            "{name}: task status"
        );
        assert_eq!(
            snapshot.world_time_ticks,
            expected["worldTimeTicks"].as_u64().unwrap(),
            "{name}: world time"
        );
        assert_eq!(snapshot.companion.companion_id, fixture.companion);
        assert_eq!(
            snapshot.companion.position.get(),
            vec3(&input["companion"]["position"]),
            "{name}: companion position"
        );
        assert_eq!(
            snapshot.companion.look,
            LookAngles::try_new(
                f32_of(&input["companion"]["yaw"]),
                f32_of(&input["companion"]["pitch"])
            )
            .unwrap(),
            "{name}: companion look"
        );
        assert_eq!(
            snapshot.instruction.as_str(),
            input["command"].as_str().unwrap(),
            "{name}: instruction"
        );
        let issuer = &input["issuer"];
        assert_eq!(
            snapshot.issuer,
            SnapshotIssuer {
                player_id: PlayerId::try_from_bytes(uuid(issuer["id"].as_str().unwrap())).unwrap(),
                position: FiniteVec3::try_new(vec3(&issuer["position"])).unwrap(),
                look: LookAngles::try_new(f32_of(&issuer["yaw"]), f32_of(&issuer["pitch"]))
                    .unwrap(),
                look_hit: look_hit(&issuer["lookHit"]),
            },
            "{name}: issuer"
        );
        assert_eq!(
            snapshot.source_tick(),
            expected["sourceTick"].as_u64().expect("fixture sourceTick"),
            "{name}: source tick"
        );
    }
}

#[test]
fn current_world_matches_go_fixtures() {
    for (name, text) in FIXTURES {
        let fixture = planning_fixture(text);
        let expected = &fixture.expected;
        let world = fixture
            .authority
            .companion_current_world(fixture.companion)
            .unwrap_or_else(|error| panic!("{name}: current world failed: {error:?}"));
        let terrain = expected_terrain(expected);
        let mut blocks = BTreeMap::new();
        for x in 0..33 {
            for y in 0..17 {
                for z in 0..33 {
                    let column = (x * 33 + z) as usize;
                    let world_y = terrain.origin.y() + y;
                    if terrain.ready[column / 8] & (1 << (column % 8)) == 0
                        || !(-64..320).contains(&world_y)
                    {
                        continue;
                    }
                    blocks.insert(
                        BlockPos::new(terrain.origin.x() + x, world_y, terrain.origin.z() + z),
                        terrain.blocks[((x * 17 + y) * 33 + z) as usize],
                    );
                }
            }
        }
        assert_eq!(world.blocks.len(), blocks.len(), "{name}: observable cells");
        assert!(world.blocks == blocks, "{name}: current blocks");
        let revisions: BTreeMap<ChunkPos, u64> = expected_revisions(expected)
            .into_iter()
            .map(|revision| (revision.pos, revision.revision))
            .collect();
        assert_eq!(
            world.chunk_revisions, revisions,
            "{name}: current revisions"
        );
        assert_eq!(
            world.inventory,
            inventory(&expected["inventory"]),
            "{name}: current inventory"
        );
        let players: BTreeMap<PlayerId, [f32; 3]> = expected_players(expected)
            .into_iter()
            .map(|player| (player.player_id, player.position.get()))
            .collect();
        assert_eq!(world.online_players, players, "{name}: current players");
        assert_eq!(
            world.tick,
            expected["sourceTick"].as_u64().expect("fixture sourceTick"),
            "{name}: current tick"
        );
    }
}

fn companion_actor(authority: &mut AuthorityState, companion: CompanionId) -> &mut ActorRecord {
    authority
        .residents
        .actors
        .iter_mut()
        .find(|actor| actor.key == ActorKey::Companion(companion))
        .unwrap()
}

#[test]
fn planning_snapshot_requires_a_current_task() {
    let mut fixture = planning_fixture(FIXTURES[0].1);
    // A live companion the chat book holds no task for.
    let idle = CompanionId::try_from_bytes(uuid("c0ffee00-0000-4000-8000-0000000000dd")).unwrap();
    let busy = fixture.companion;
    companion_actor(&mut fixture.authority, busy).key = ActorKey::Companion(idle);
    let inventory = fixture
        .authority
        .residents
        .inventories
        .remove(&ActorKey::Companion(busy))
        .unwrap();
    fixture
        .authority
        .residents
        .inventories
        .insert(ActorKey::Companion(idle), inventory);
    assert_eq!(
        fixture.authority.companion_planning_snapshot(idle),
        Err(ServerError::InvalidInput {
            field: "planning_task"
        })
    );
    // The current-world rebuild does not depend on the task.
    assert!(fixture.authority.companion_current_world(idle).is_ok());
}

#[test]
fn projections_refuse_an_inactive_companion() {
    let mut fixture = planning_fixture(FIXTURES[0].1);
    let companion = fixture.companion;
    companion_actor(&mut fixture.authority, companion).lifecycle = ActorLifecycle::Dead;
    let refused = ServerError::InvalidInput {
        field: "planning_companion",
    };
    assert_eq!(
        fixture.authority.companion_planning_snapshot(companion),
        Err(refused)
    );
    assert_eq!(
        fixture.authority.companion_current_world(companion),
        Err(refused)
    );
}

#[test]
fn projections_refuse_an_invalid_companion_inventory() {
    let mut fixture = planning_fixture(FIXTURES[0].1);
    let companion = fixture.companion;
    fixture
        .authority
        .residents
        .inventories
        .get_mut(&ActorKey::Companion(companion))
        .unwrap()
        .slots[3] = ItemStack {
        item: 0,
        count: 1,
        durability: 0,
    };
    assert_eq!(
        fixture.authority.companion_current_world(companion),
        Err(ServerError::InvalidInput {
            field: "planning_companion"
        })
    );
}

#[test]
fn projections_refuse_a_window_past_the_coordinate_range() {
    for position in [
        [2_147_483_632.0, 64.0, 0.0],
        [0.0, 64.0, -2_147_483_648.0],
        [0.0, 2_147_483_640.0, 0.0],
    ] {
        let mut fixture = planning_fixture(FIXTURES[0].1);
        let companion = fixture.companion;
        companion_actor(&mut fixture.authority, companion).motion = motion(position);
        assert_eq!(
            fixture.authority.companion_current_world(companion),
            Err(ServerError::InvalidInput {
                field: "planning_terrain"
            }),
            "{position:?}"
        );
    }
}

#[test]
fn projections_read_settled_world_changes() {
    let mut fixture = planning_fixture(FIXTURES[0].1);
    let companion = fixture.companion;
    let before = fixture
        .authority
        .companion_current_world(companion)
        .unwrap();
    let key = ChunkKey {
        dimension: Dimension::OVERWORLD,
        pos: ChunkPos::new(1, 0),
    };
    let chunk = fixture.authority.residents.ready.remove(&key).unwrap();
    let after = fixture
        .authority
        .companion_current_world(companion)
        .unwrap();
    assert!(before.chunk_revisions.contains_key(&key.pos));
    assert!(!after.chunk_revisions.contains_key(&key.pos));
    assert!(
        before
            .blocks
            .keys()
            .any(|pos| pos.x() >> 4 == 1 && pos.z() >> 4 == 0)
    );
    assert!(
        !after
            .blocks
            .keys()
            .any(|pos| pos.x() >> 4 == 1 && pos.z() >> 4 == 0)
    );
    fixture.authority.residents.ready.insert(key, chunk);
    assert_eq!(
        fixture
            .authority
            .companion_current_world(companion)
            .unwrap(),
        before
    );
}
