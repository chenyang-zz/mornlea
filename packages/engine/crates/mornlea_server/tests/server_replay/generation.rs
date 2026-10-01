//! Independent Go provider expectations for off-tick seeded generation.

use mornlea_domain::{ChunkPos, Dimension};
use mornlea_engine::native::contracts::world::{WorldgenOp, WorldgenParams, WorldgenScratch};
use mornlea_engine::native::worldgen::NativeWorldgen;
use mornlea_server::contracts::ChunkKey;
use mornlea_server::core::generation::ChunkGenerator;
use mornlea_storage::{ChestSlot, Chunk, DropSlot, FurnaceSlot, checked_section};
use serde_json::Value;
use sha2::{Digest, Sha256};

fn fixture() -> Value {
    serde_json::from_str(include_str!(
        "../../../../../../testdata/runtime-migration/server/worldgen-seeding.json"
    ))
    .expect("Go fixture")
}

fn header(params: &WorldgenParams) -> Vec<u8> {
    let mut bytes = b"MGW1".to_vec();
    bytes.extend(3u32.to_le_bytes());
    bytes.extend(params.seed().to_le_bytes());
    bytes.extend((-64i32).to_le_bytes());
    bytes.extend(320i32.to_le_bytes());
    for id in params.materials().as_slice() {
        bytes.extend(id.to_le_bytes());
    }
    bytes.extend(params.perm());
    bytes
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|value| format!("{value:02x}")).collect()
}

#[test]
fn seed42_header_matches_go() {
    let generator = ChunkGenerator::try_new(42, false).expect("generator");
    let params = generator.parameters(Dimension::OVERWORLD);
    let data = fixture();
    let expected = data["headers"]
        .as_array()
        .expect("headers")
        .iter()
        .find(|row| {
            row["seed"].as_i64() == Some(42)
                && row["dimension"].as_u64() == Some(0)
                && row["fluid"].as_bool() == Some(false)
        })
        .expect("seed42 header");
    assert_eq!(
        hex(&header(params)),
        expected["header_hex"].as_str().expect("header")
    );
}

#[test]
fn all_seed_dimension_fluid_headers_match_go() {
    let data = fixture();
    let rows = data["headers"].as_array().expect("headers");
    assert_eq!(rows.len(), 24);
    for row in rows {
        let seed = row["seed"].as_i64().expect("seed");
        let dimension = Dimension::new(row["dimension"].as_u64().expect("dimension") as u8)
            .expect("checked dimension");
        let generator = ChunkGenerator::try_new(seed, row["fluid"].as_bool().expect("fluid"))
            .expect("generator");
        let params = generator.parameters(dimension);
        assert_eq!(
            hex(&header(params)),
            row["header_hex"].as_str().expect("header"),
            "seed {seed}, dimension {dimension:?}"
        );
        assert_eq!(&params.perm()[..256], &params.perm()[256..]);
        let mut counts = [0u16; 256];
        for byte in &params.perm()[..256] {
            counts[usize::from(*byte)] += 1;
        }
        assert_eq!(counts, [1; 256]);
    }
}

fn key(dimension: Dimension, x: i32, z: i32) -> ChunkKey {
    ChunkKey {
        dimension,
        pos: ChunkPos::new(x, z),
    }
}

fn dense_hash(dense: &[u16]) -> String {
    let mut digest = Sha256::new();
    for value in dense {
        digest.update(value.to_le_bytes());
    }
    hex(&digest.finalize())
}

fn logical_dense(chunk: &Chunk) -> Vec<u16> {
    assert_eq!(chunk.sections.len(), 24);
    let mut dense = Vec::with_capacity(98_304);
    for raw in &chunk.sections {
        let checked = checked_section(raw).expect("checked generated section");
        for index in 0..4096 {
            dense.push(checked.block_at(index).expect("cell"));
        }
    }
    dense
}

fn compact_hash(chunk: &Chunk) -> String {
    let mut digest = Sha256::new();
    for section in &chunk.sections {
        digest.update([section.kind as u8, section.bits]);
        digest.update(section.single.to_le_bytes());
        digest.update((section.palette.len() as u32).to_le_bytes());
        digest.update((section.packed.len() as u32).to_le_bytes());
        for value in &section.palette {
            digest.update(value.to_le_bytes());
        }
        for value in &section.packed {
            digest.update(value.to_le_bytes());
        }
    }
    hex(&digest.finalize())
}

#[test]
fn dense_and_compact_chunks_match_go_and_reuse_scratch() {
    let data = fixture();
    let rows = data["chunks"].as_array().expect("chunks");
    assert_eq!(rows.len(), 12);
    let mut scratch = WorldgenScratch::try_new().expect("scratch");
    let mut native_dense = vec![0; 98_304];
    for row in rows {
        let seed = row["seed"].as_i64().expect("seed");
        let dimension = Dimension::new(row["dimension"].as_u64().expect("dimension") as u8)
            .expect("checked dimension");
        let x = row["position"][0].as_i64().expect("x") as i32;
        let z = row["position"][1].as_i64().expect("z") as i32;
        let mut generator = ChunkGenerator::try_new(seed, row["fluid"].as_bool().expect("fluid"))
            .expect("generator");
        assert_eq!(
            NativeWorldgen
                .generate_chunk(
                    generator.parameters(dimension),
                    [x, z],
                    &mut scratch,
                    &mut native_dense
                )
                .expect("native generation"),
            98_304
        );
        assert_eq!(
            dense_hash(&native_dense),
            row["dense_sha256"].as_str().expect("dense digest")
        );
        let chunk = generator
            .generate(key(dimension, x, z))
            .expect("compact generation");
        assert_eq!(
            dense_hash(&logical_dense(&chunk)),
            row["dense_sha256"].as_str().expect("dense digest")
        );
        assert_eq!(
            compact_hash(&chunk),
            row["compact_sha256"].as_str().expect("compact digest")
        );
        assert_eq!(chunk.drops, vec![DropSlot::default(); 32]);
        assert_eq!(chunk.furnaces, vec![FurnaceSlot::default(); 32]);
        assert_eq!(chunk.chests, vec![ChestSlot::default(); 16]);
        assert_eq!(
            generator.generate(key(dimension, x, z)).expect("repeat"),
            chunk
        );
        let other = if dimension == Dimension::OVERWORLD {
            Dimension::DEPTHS
        } else {
            Dimension::OVERWORLD
        };
        generator
            .generate(key(other, x, z))
            .expect("alternate dimension");
        assert_eq!(
            generator
                .generate(key(dimension, x, z))
                .expect("after alternate"),
            chunk
        );
    }
}

#[test]
fn seed42_nonorigin_chunks_match_existing_go_golden() {
    let golden = include_str!("../../../../../shared/worldgen/testdata/golden_seed42.txt");
    let mut generator = ChunkGenerator::try_new(42, false).expect("generator");
    for line in golden.lines() {
        let (position, expected) = line.split_once(' ').expect("golden row");
        let (x, z) = position
            .strip_prefix("chunk(")
            .expect("chunk")
            .strip_suffix(')')
            .expect("coordinates")
            .split_once(',')
            .expect("coordinate pair");
        let chunk = generator
            .generate(key(
                Dimension::OVERWORLD,
                x.parse().expect("x"),
                z.parse().expect("z"),
            ))
            .expect("generation");
        assert_eq!(dense_hash(&logical_dense(&chunk)), expected, "{position}");
    }
    assert_eq!(golden.lines().count(), 4);
}

#[test]
fn fluid_and_dimension_change_actual_fixture_terrain() {
    let data = fixture();
    let rows = data["chunks"].as_array().expect("chunks");
    let oracle = |dimension: u64| {
        rows.iter()
            .find(|row| {
                row["seed"].as_i64() == Some(42) && row["dimension"].as_u64() == Some(dimension)
            })
            .expect("seed42 oracle")["dense_sha256"]
            .as_str()
            .expect("digest")
    };
    let mut dry = ChunkGenerator::try_new(42, false).expect("dry generator");
    let mut wet = ChunkGenerator::try_new(42, true).expect("wet generator");
    let overworld = dense_hash(&logical_dense(
        &dry.generate(key(Dimension::OVERWORLD, 0, 0))
            .expect("overworld"),
    ));
    let depths = dense_hash(&logical_dense(
        &wet.generate(key(Dimension::DEPTHS, 0, 0)).expect("depths"),
    ));
    assert_eq!(overworld, oracle(0));
    assert_eq!(depths, oracle(1));
    assert_ne!(overworld, depths);
    let dry_depths = dense_hash(&logical_dense(
        &dry.generate(key(Dimension::DEPTHS, 0, 0))
            .expect("dry depths"),
    ));
    assert_ne!(dry_depths, oracle(1));
    let wet_overworld = dense_hash(&logical_dense(
        &wet.generate(key(Dimension::OVERWORLD, 0, 0))
            .expect("wet overworld"),
    ));
    assert_ne!(wet_overworld, oracle(0));
}
