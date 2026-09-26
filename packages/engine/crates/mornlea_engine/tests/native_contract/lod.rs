use mornlea_engine::native::contracts::KernelError;
use mornlea_engine::native::contracts::world::{
    LodFace, LodOp, LodQuad, LodRequest, LodScratch, LodStep, Materials, WorldgenParams,
};
use mornlea_engine::native::lod::NativeLod;

/// Stage ceiling of the caller-owned scratch: the fixed worst-case bound for
/// the largest step size (`3 * N * N + 2 * N` at `N = 32`). Real shells publish
/// only their used prefix.
const STAGE_QUADS: usize = 3136;

/// Sea-level constant of the shared world generator, duplicated here because
/// the clamp is asserted through the typed contract only.
const SEA_LEVEL_Y: i32 = 64;

/// The basin tile whose windows clamp to the sea surface under the identity
/// permutation, located from the release-ABI observation run.
const SEA_TILE: [i32; 2] = [-3, 2];

/// The all-basin tile that clamps to a single uniform top quad, located from
/// the release-ABI observation run.
const UNIFORM_TILE: [i32; 2] = [-4, 1];

/// Destination canary: the inert default record has both spans 1 while every
/// real quad spans at least one full step, so an untouched element is always
/// distinguishable from real output.
fn canary() -> LodQuad {
    LodQuad::default()
}

fn materials_with_water(water: u16) -> Materials {
    Materials {
        air: 0,
        stone: 1,
        dirt: 2,
        grass: 3,
        bedrock: 4,
        snow: 5,
        sand: 6,
        clay: 7,
        gravel: 8,
        iron_ore: 9,
        coal_ore: 10,
        oak_log: 11,
        leaves: 12,
        water,
        short_grass: 14,
    }
}

/// Identity-permutation world parameters. The shell reads only terrain heights
/// and surface materials, so the seed never reaches the output.
fn params_with_water(water: u16) -> WorldgenParams {
    let mut perm = [0u8; 512];
    for (i, entry) in perm.iter_mut().enumerate() {
        *entry = (i & 255) as u8;
    }
    WorldgenParams::try_new(42, materials_with_water(water), perm).unwrap()
}

/// Builds one tile through the provider on the caller's scratch and returns
/// the published prefix as an ordered list.
fn build(
    scratch: &mut LodScratch,
    params: &WorldgenParams,
    tile: [i32; 2],
    step: LodStep,
) -> Result<Vec<LodQuad>, KernelError> {
    let mut dst = [canary(); STAGE_QUADS];
    let count = NativeLod.build(&LodRequest { params, tile, step }, scratch, &mut dst)?;
    Ok(dst[..count].to_vec())
}

/// Finds the top quad covering the given world column, if any.
fn top_covering(tops: &[LodQuad], x: i32, z: i32) -> Option<LodQuad> {
    tops.iter().copied().find(|quad| {
        quad.x() <= x
            && x < quad.x() + i32::from(quad.w())
            && quad.z() <= z
            && z < quad.z() + i32::from(quad.d())
    })
}

/// Walks the ordered shell contract through the public getters: tops first in
/// strictly increasing (z, x) start order, then X skirts, then Z skirts. Every
/// skirt must close its height step exactly with the taller side owning the
/// wall, pinning the skirt fields `face`, `y`, `w`, `d`, `material` and
/// `shade` against the tops it connects. At a tile boundary the lower side is
/// the neighbor's window and has no top here; only the taller side is checked,
/// which is exactly the case where the taller side keeps the skirt.
fn check_ordered_shell(step: i32, tile: [i32; 2], quads: &[LodQuad]) {
    let base_x = tile[0] * 64;
    let base_z = tile[1] * 64;
    let mut tops: Vec<LodQuad> = Vec::new();
    let mut phase = 0u8;
    let mut prev_key: Option<(i32, i32)> = None;
    for quad in quads {
        let (next_phase, key) = match quad.face() {
            LodFace::Top => {
                assert_eq!(quad.shade(), 255, "top shading is always full");
                assert!(i32::from(quad.w()) >= step && i32::from(quad.w()) % step == 0);
                assert!(i32::from(quad.d()) >= step && i32::from(quad.d()) % step == 0);
                tops.push(*quad);
                (0, (quad.z(), quad.x()))
            }
            LodFace::NegX | LodFace::PosX => {
                assert_eq!(quad.shade(), 153, "x-skirt shading is the x-axis weight");
                assert_eq!(i32::from(quad.w()), step, "a skirt is one window wide");
                (1, (quad.z(), quad.x()))
            }
            LodFace::NegZ | LodFace::PosZ => {
                assert_eq!(quad.shade(), 204, "z-skirt shading is the z-axis weight");
                assert_eq!(i32::from(quad.w()), step, "a skirt is one window wide");
                (2, (quad.x(), quad.z()))
            }
        };
        assert!(
            next_phase >= phase,
            "face groups must run tops, X skirts, then Z skirts"
        );
        if next_phase != phase {
            phase = next_phase;
            prev_key = None;
        }
        if let Some(prev) = prev_key {
            assert!(
                prev < key,
                "quads inside a face group must be strictly ordered"
            );
        }
        prev_key = Some(key);
    }

    for quad in quads.iter().filter(|quad| quad.face() != LodFace::Top) {
        // The skirt's own corner sits on the taller side of the wall; the
        // neighbor across the wall is the lower side.
        let high = top_covering(&tops, quad.x(), quad.z())
            .expect("the taller side of a skirt always covers its wall corner");
        let (low_x, low_z) = match quad.face() {
            LodFace::PosX => (quad.x() + 1, quad.z()),
            LodFace::NegX => (quad.x() - 1, quad.z()),
            LodFace::PosZ => (quad.x(), quad.z() + 1),
            LodFace::NegZ => (quad.x(), quad.z() - 1),
            LodFace::Top => unreachable!(),
        };
        assert!(i32::from(quad.d()) >= 1);
        assert_eq!(
            quad.y() + i32::from(quad.d()),
            high.y() + 1,
            "skirt must reach the taller top plane"
        );
        assert_eq!(
            quad.material(),
            high.material(),
            "skirt carries the taller window's material"
        );
        match top_covering(&tops, low_x, low_z) {
            Some(low) => {
                assert!(high.y() > low.y(), "the taller side must own the skirt");
                assert_eq!(quad.y(), low.y() + 1, "skirt starts on the lower top plane");
                assert_eq!(
                    i32::from(quad.d()),
                    high.y() - low.y(),
                    "skirt spans exactly the height step"
                );
                match quad.face() {
                    LodFace::PosX => {
                        assert_eq!(high.x() + i32::from(high.w()), quad.x() + 1);
                        assert_eq!(low.x(), quad.x() + 1);
                    }
                    LodFace::NegX => {
                        assert_eq!(high.x(), quad.x());
                        assert_eq!(low.x() + i32::from(low.w()), quad.x());
                    }
                    LodFace::PosZ => {
                        assert_eq!(high.z() + i32::from(high.d()), quad.z() + 1);
                        assert_eq!(low.z(), quad.z() + 1);
                    }
                    LodFace::NegZ => {
                        assert_eq!(high.z(), quad.z());
                        assert_eq!(low.z() + i32::from(low.d()), quad.z());
                    }
                    LodFace::Top => unreachable!(),
                }
            }
            None => {
                // The lower side belongs to the neighbor tile; this tile only
                // owns the skirt when the wall sits on its own boundary.
                match quad.face() {
                    LodFace::PosX => {
                        assert_eq!(quad.x() + 1, base_x + 64);
                        assert_eq!(high.x() + i32::from(high.w()), quad.x() + 1);
                    }
                    LodFace::NegX => {
                        assert_eq!(quad.x(), base_x);
                        assert_eq!(high.x(), quad.x());
                    }
                    LodFace::PosZ => {
                        assert_eq!(quad.z() + 1, base_z + 64);
                        assert_eq!(high.z() + i32::from(high.d()), quad.z() + 1);
                    }
                    LodFace::NegZ => {
                        assert_eq!(quad.z(), base_z);
                        assert_eq!(high.z(), quad.z());
                    }
                    LodFace::Top => unreachable!(),
                }
            }
        }
    }
}

#[test]
fn lod_steps_and_skirts() {
    for (step, span) in [
        (LodStep::Two, 2i32),
        (LodStep::Four, 4),
        (LodStep::Eight, 8),
    ] {
        let params = params_with_water(13);
        let mut scratch = LodScratch::try_new().unwrap();
        let first = build(&mut scratch, &params, [-3, 2], step).unwrap();
        let second = build(&mut scratch, &params, [-3, 2], step).unwrap();
        assert_eq!(first, second, "step {span} shell must be deterministic");
        assert!(!first.is_empty(), "step {span} shell must be non-empty");
        check_ordered_shell(span, [-3, 2], &first);
    }

    // Uniform single-top case: an all-basin tile clamps every window to the
    // sea surface, so the greedy merge collapses the whole tile to exactly one
    // top quad and the boundary generates no skirt the tile owns. The same
    // quad appears at every step size.
    for (step, span) in [
        (LodStep::Two, 2i32),
        (LodStep::Four, 4),
        (LodStep::Eight, 8),
    ] {
        let params = params_with_water(13);
        let mut scratch = LodScratch::try_new().unwrap();
        let quads = build(&mut scratch, &params, UNIFORM_TILE, step).unwrap();
        assert_eq!(
            quads.len(),
            1,
            "step {span} uniform tile must hold one quad"
        );
        let quad = &quads[0];
        assert_eq!(
            (
                quad.face(),
                quad.x(),
                quad.z(),
                quad.y(),
                quad.w(),
                quad.d(),
                quad.material(),
                quad.shade(),
            ),
            (LodFace::Top, -256, 64, 64, 64, 64, 13, 255),
            "step {span} uniform top fields must match the observation pin"
        );
    }

    // Empty shells (zero quads) cannot be produced by any request that passes
    // `WorldgenParams::try_new`: every sampled window is non-air because the
    // surface query always answers a terrain material within the world Y
    // range. The zero-quad shape stays pinned at the shared seam in `src/lod.rs`
    // with synthetic window fields.

    // Taller-side boundary pin: the first skirt of the step-eight case, with
    // the exact ordered fields recorded from the release-ABI observation run
    // (the material is the identity table's grass entry).
    let params = params_with_water(13);
    let mut scratch = LodScratch::try_new().unwrap();
    let quads = build(&mut scratch, &params, [-3, 2], LodStep::Eight).unwrap();
    let skirt = quads
        .iter()
        .find(|quad| quad.face() != LodFace::Top)
        .expect("the tile must contain at least one height step");
    assert_eq!(
        (
            skirt.face(),
            skirt.x(),
            skirt.z(),
            skirt.y(),
            skirt.w(),
            skirt.d(),
            skirt.material(),
            skirt.shade(),
        ),
        (LodFace::NegX, -152, 160, 65, 8, 1, 3, 153),
        "first skirt fields must match the observation pin"
    );
}

#[test]
fn sea_clamp() {
    // A window whose solid terrain top sits below sea level is clamped to the
    // sea surface with the water material while fluid is enabled, and keeps
    // its unclamped terrain when the water entry equals the air entry. Both
    // covering tops are pinned from the release-ABI observation run.
    let clamped = {
        let params = params_with_water(13);
        let mut scratch = LodScratch::try_new().unwrap();
        build(&mut scratch, &params, SEA_TILE, LodStep::Four).unwrap()
    };
    let water_top = clamped
        .iter()
        .find(|quad| {
            quad.face() == LodFace::Top && quad.y() == SEA_LEVEL_Y && quad.material() == 13
        })
        .expect("the basin tile must contain a clamped water surface");
    assert_eq!(
        (
            water_top.x(),
            water_top.z(),
            water_top.y(),
            water_top.w(),
            water_top.d(),
            water_top.material(),
            water_top.shade(),
        ),
        (-192, 128, 64, 64, 24, 13, 255),
        "clamped water top fields must match the observation pin"
    );

    let unclamped = {
        let params = params_with_water(0);
        let mut scratch = LodScratch::try_new().unwrap();
        build(&mut scratch, &params, SEA_TILE, LodStep::Four).unwrap()
    };
    let terrain_top = top_covering(&unclamped, water_top.x(), water_top.z())
        .expect("the gate-off shell must cover the same window");
    assert!(
        terrain_top.y() < SEA_LEVEL_Y,
        "with the fluid gate off the same window keeps its solid terrain top"
    );
    assert_eq!(
        (
            terrain_top.x(),
            terrain_top.z(),
            terrain_top.y(),
            terrain_top.w(),
            terrain_top.d(),
            terrain_top.material(),
            terrain_top.shade(),
        ),
        (-192, 128, 47, 4, 8, 6, 255),
        "unclamped terrain top fields must match the observation pin"
    );
}

#[test]
fn tile_overflow_preflight() {
    // Tile admission mirrors the legacy gate: per axis, tile * 64, base + 64
    // and base - 8 must all be representable before any sampling happens. The
    // chains are independent per axis because each axis overflows at its own
    // boundary.
    let params = params_with_water(13);
    let mut scratch = LodScratch::try_new().unwrap();
    for tile in [
        [i32::MIN / 64, 0],
        [i32::MIN / 64 - 1, 0],
        [i32::MAX / 64, 0],
        [i32::MAX / 64 + 1, 0],
        [0, i32::MIN / 64],
        [0, i32::MIN / 64 - 1],
        [0, i32::MAX / 64],
        [0, i32::MAX / 64 + 1],
    ] {
        let mut dst = [canary(); STAGE_QUADS];
        let result = NativeLod.build(
            &LodRequest {
                params: &params,
                tile,
                step: LodStep::Four,
            },
            &mut scratch,
            &mut dst,
        );
        assert_eq!(result, Err(KernelError::InvalidInput), "tile {tile:?}");
        assert!(
            dst.iter().all(|quad| *quad == canary()),
            "tile {tile:?} must be rejected before any output"
        );
    }

    // The extreme tiles admitted by the same gate must actually build.
    for tile in [[33554430, -33554431], [-33554431, 33554430]] {
        let built = build(&mut scratch, &params, tile, LodStep::Eight);
        assert!(built.is_ok(), "admitted tile {tile:?} must build");
    }
}

#[test]
fn capacity_and_canary() {
    let params = params_with_water(13);
    let mut scratch = LodScratch::try_new().unwrap();
    let request = LodRequest {
        params: &params,
        tile: [-3, 2],
        step: LodStep::Four,
    };

    let mut probe = [canary(); STAGE_QUADS];
    let needed = NativeLod.build(&request, &mut scratch, &mut probe).unwrap();
    assert!(needed > 0);

    // Exact capacity succeeds and overwrites every published slot.
    let mut exact = [canary(); STAGE_QUADS];
    let written = NativeLod.build(&request, &mut scratch, &mut exact).unwrap();
    assert_eq!(written, needed);
    assert!(exact[..needed].iter().all(|quad| *quad != canary()));

    // One slot short reports the exact required count and leaves the
    // destination untouched; the stage ceiling is never substituted for the
    // actual count. A zero-capacity destination reports the same count.
    for available in [0usize, needed - 1] {
        let mut short = [canary(); STAGE_QUADS];
        let result = NativeLod.build(&request, &mut scratch, &mut short[..available]);
        assert_eq!(
            result,
            Err(KernelError::OutputTooSmall { needed, available })
        );
        assert!(short.iter().all(|quad| *quad == canary()));
    }

    // A surplus destination publishes only the used prefix.
    let mut surplus = [canary(); STAGE_QUADS];
    let written = NativeLod
        .build(&request, &mut scratch, &mut surplus)
        .unwrap();
    assert_eq!(written, needed);
    assert!(surplus[needed..].iter().all(|quad| *quad == canary()));
}

#[test]
fn warm_scratch_reuse() {
    // One caller-owned scratch serves all three step sizes and two tiles,
    // including a rejected call in between: later calls keep succeeding and
    // keep producing the same quads a fresh scratch would.
    let params = params_with_water(13);
    let mut warm = LodScratch::try_new().unwrap();

    let two_a = build(&mut warm, &params, [-3, 2], LodStep::Two).unwrap();
    let four_b = build(&mut warm, &params, [1, -1], LodStep::Four).unwrap();

    let mut short = [canary(); STAGE_QUADS];
    let rejected = NativeLod.build(
        &LodRequest {
            params: &params,
            tile: [-3, 2],
            step: LodStep::Eight,
        },
        &mut warm,
        &mut short[..1],
    );
    assert!(matches!(rejected, Err(KernelError::OutputTooSmall { .. })));

    let eight_a = build(&mut warm, &params, [-3, 2], LodStep::Eight).unwrap();
    let two_a_again = build(&mut warm, &params, [-3, 2], LodStep::Two).unwrap();
    let four_b_again = build(&mut warm, &params, [1, -1], LodStep::Four).unwrap();
    assert_eq!(two_a, two_a_again);
    assert_eq!(four_b, four_b_again);

    let mut fresh = LodScratch::try_new().unwrap();
    assert_eq!(
        two_a,
        build(&mut fresh, &params, [-3, 2], LodStep::Two).unwrap()
    );
    assert_eq!(
        four_b,
        build(&mut fresh, &params, [1, -1], LodStep::Four).unwrap()
    );
    assert_eq!(
        eight_a,
        build(&mut fresh, &params, [-3, 2], LodStep::Eight).unwrap()
    );
}
