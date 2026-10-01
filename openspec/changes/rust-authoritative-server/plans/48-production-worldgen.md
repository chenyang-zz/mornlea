# Production seed-compatible chunk generation

Baseline `1eb29f46d428929358ee0dce219bbd4d444472c2`; node `3.7g0` precedes the real acquisition worker. Spec: the active design's source-parity, bounded off-tick preparation and real-provider requirements. Goal: produce actual Go-compatible compact terrain from the stored world seed in both dimensions, using the accepted NativeWorldgen kernel. Root used the installed brainstorming and writing-plans skills; project orchestration keeps this packet in OpenSpec and status solely in tasks.md. No additional approval or parallel ledger is required for the user's continued development instruction.

## Decision and ownership

Reuse NativeWorldgen, rather than duplicate numerical terrain logic or call the Go process at runtime. A small private compatibility RNG supplies the exact Go math/rand permutation. One public off-tick generator owns two checked parameter values, one WorldgenScratch and one reusable 98,304-cell destination. It returns one owned compact Chunk; the future acquisition owner alone supplies request identity, revision1/persisted0 and Ready preparation. No authority handle, disk lease, thread, packet or save acknowledgment belongs to generation.

Exact additive API in `src/core/generation.rs`:

```rust
pub struct ChunkGenerator { /* private owned parameters and scratch */ }
impl ChunkGenerator {
    pub fn try_new(seed: i64, fluid_enabled: bool) -> Result<Self, ServerError>;
    pub fn parameters(&self, dimension: mornlea_domain::Dimension)
        -> &mornlea_engine::native::contracts::world::WorldgenParams;
    pub fn generate(&mut self, key: ChunkKey) -> Result<mornlea_storage::Chunk, ServerError>;
}
```

Public module `core::generation`, private module `core::go_random`. Private helper `pub(super) fn permutation(seed: i64) -> [u8; 512]`. No separate shared-contract landing: there is one implementation owner and no parallel consumer until root accepts this actual provider SHA. Later acquisition consumes this exact API serially. Dimension is already checked to 0/1; ChunkPos remains the existing unrestricted i32 pair and NativeWorldgen retains its source wrapping-coordinate semantics.

Canonical material order is air0, stone2, dirt3, grass4, bedrock5, snow25, sand15, clay24, gravel16, iron_ore8, coal_ore7, oak_log17, leaves19, water27 when enabled else0, short_grass84. Overworld seed is unchanged; Depths seed is `(seed as u64 ^ 0x9e3779b97f4a7c15) as i64`. Each dimension constructs its own permutation from its derived seed. `WorldgenParams::try_new` and `WorldgenScratch::try_new` failures map to `ServerError::Internal { invariant: "worldgen parameters" }` and `"worldgen scratch"`; kernel error maps to `"worldgen kernel"`. Generated used length must be98,304 else Internal `"worldgen output length"`. No production fallback.

## Compatibility algorithm

Port the Go1.26.0 math/rand rngSource and Shuffle's private int31n, with the original 607 cooked i64 constants from `/workspace/.mornlea-env/go/src/math/rand/rng.go`. Preserve the full Go BSD notice in the new private helper (copyright, all three conditions, disclaimer), and identify Go1.26.0 rng.go/rand.go as the compatibility source in English. No dependency or unsafe code is needed.

Initial tap0/feed334. Normalize `seed % 2147483647`, add modulus if negative, replace zero with89482311; signed remainder handles i64MIN without negation. Park-Miller step: hi=x/44488, lo=x%44488, x=48271*lo-3399*hi, add2147483647 if negative. Iterate i=-20 through606 inclusive: advance once; when i>=0 combine `(x as u64)<<40`, advance and XOR `(x as u64)<<20`, advance and XOR x, then XOR cooked[i] as u64. All 64-bit state is wrapping bits. Uint64 decrements tap/feed with wrap607 and stores/returns `vec[feed].wrapping_add(vec[tap])`; Int63 clears the top bit; Uint32 is `(Int63 >> 31) as u32`.

Initialize base0..255; for i descending255..1, n=i+1, v=Uint32, prod=u64(v)*u64(n), low=prod as u32. If low<n, threshold=`0u32.wrapping_sub(n)%n`, resample while low<threshold. j=prod>>32, swap base[i]/base[j]. Duplicate the resulting256 bytes. Public Int31n's mask/mod algorithm is a different stream and must not replace this multiply/reject operation, even for powers of two.

Dense to compact conversion scans each of24 contiguous4096-cell YZX sections. Palette order is first appearance in that scan, matching Go Compact (not sorted IDs and not initial historical palette). Uniform gives Single/bits0/singleID/no arrays. Nonuniform <=16 distinct gives Indexed/bits4/single0;17..256 gives Indexed/bits8/single0; more gives Direct/bits15/single0/no palette. Pack without crossing u64 boundaries: perWord=64/bits, slot shift=(index%perWord)*bits, words=(4096+perWord-1)/perWord; unused high bits stay zero. Validate each dense ID is <=32767, otherwise Internal `"worldgen block id"`. Bounded conversion:24 sections, palette collection at most257 IDs (then direct),4096 scans per section and linear search at most256; no expanding chunk-coordinate work. Canonical actual terrain uses at most15 materials. Append exactly32 default DropSlot,32 default FurnaceSlot,16 default ChestSlot; all inactive generation0.

## Exact files and evidence

Worker editable paths beneath packages/engine/crates/mornlea_server: new src/core/generation.rs, new src/core/go_random.rs, src/core/mod.rs (only module declarations), tests/server_replay.rs (only generation registration), new tests/server_replay/generation.rs, crate AGENTS.md (only generation ownership). Root owns this packet, tasks.md and new `testdata/runtime-migration/server/worldgen-seeding.json`. All storage/engine/domain source, sealed F1 corpus, protocol/save schemas, Go runtime, bin, acquisition, state, store, Agent and transport are read-only.

Independent oracle was generated by the actual Go shared worldgen provider at db695136 with Go1.26.0, using disposable `/workspace/scratch/mornlea-worldgen-seeding-oracle.go` (SHA25648f9a4d0432cb3dc61c1f3f4f3fb17d12d1c018645356789d622203220c01187). Fixture SHA256562d78be9f931972d75c1e31b559fa8a798e5fdc6a56f8d5e9ba282233efc468,33,644 bytes. Generator.go SHA2567801106a960b99748d56a66b4226bfcf23b14ab0b479ad125e98f805c0326d93; palette.go SHA256e8de5358121eca64fb362afeeec90e0336ae4ba4b969c142b8976f61a15e704a. Canonical native library SHA2560423917bb6209cb565e9bfbc471d4a2a9fb1e505ef1cf51fa48b77b3836794ad remained unchanged. This is a new runtime provider oracle, not a change to sealed numerical fixtures.

The fixture contains24 full566-byte MGW1 headers (seed0,1,-1,42,i64MIN,i64MAX; dims0/1; fluidfalse/true) and12 actual dense and compact SHA256 pairs (both dimensions for each seed; negative seeds use(-1,-1), others(0,0), fluid enabled for Depths). Header serialization: MGW1, LEu32 format3, LEi64 derivedseed, LEi32 minY-64/maxY320,15 LEu16 materials,512 permutation bytes. Dense digest:98,304 LEu16 cells. Compact digest: each section kind:u8,bits:u8,single:LEu16,palette_len:LEu32,packed_len:LEu32,palette LEu16[],packed LEu64[]. Tests may reconstruct logical dense cells only outside the tick.

## Test-first steps and acceptance

1. Register generation tests and read the independent fixture with existing serde_json Value helpers. First compare an existing checked NativeWorldgen parameter header made with identity0..255 duplicated permutation to seed42/fluidfalse/Overworld oracle. Run that exact test and record an assertion mismatch (the current synthetic seed preparation fails behavioral compatibility). A missing new API import is not RED. Replace only the parameter construction with the production generator after implementing it; the same oracle assertion must pass.
2. Implement private RNG/permutation with retained license and both parameter values/scratch. Require exact header agreement for all24 cases, half equality and each byte exactly once per half. Explicit seed0/-1/i64MIN cases prevent signed normalization accidents.
3. Implement generate and compact conversion. Compare all12 actual NativeWorldgen dense digests and returned compact digests to the Go fixture. Verify fixed slots and all24 checked_section validations; generate twice with the same owner and compare exact chunks, then alternate dimensions to exclude scratch contamination. Add a private compact-section unit test for uniform air, first-order [19,2,19,84],16→17 and256→257 transitions, noncrossing words/high padding and out-of-domain ID rejection; this private helper need not become a public general builder.
4. Additional Go golden dry seed42 Overworld chunks(0,0),(1,0),(-1,-1),(37,-104) must match `packages/shared/worldgen/testdata/golden_seed42.txt` dense hashes. This exercises non-origin trees and negative coordinates without regenerating expected values. Add different-fluid and different-dimension actual digest inequality checks using fixture outputs, not probabilistic expectations.
5. Run discovery and focused generation tests, all server_replay, server library, all-target clippy and workspace fmt; record nonzero counts/logs. Controller independently reviews, integrates and runs the full server crate on the accepted SHA before closing3.7g0. No new tests claim acquisition, login or executable acceptance.

Commands: source /workspace/.mornlea-env/env.sh; export CARGO_TARGET_DIR=/workspace/scratch/mornlea-worldgen-target; cargo test --manifest-path packages/engine/Cargo.toml -p mornlea_server --test server_replay --locked generation:: -- --list, same without --list, cargo test ... --lib, cargo test ... --test server_replay, cargo clippy --manifest-path packages/engine/Cargo.toml -p mornlea_server --all-targets --locked -- -D warnings, cargo fmt --manifest-path packages/engine/Cargo.toml --all --check, git diff --check. Native shared library is not needed by this direct Rust provider. Worker commit `feat(server): generate seeded compact chunks with Go parity`.

Readiness: source algorithms, bounds, exact APIs/errors and independently generated expected data are frozen; one serial provider and no dependency cycle. Fresh isolated worker is justified by the multi-file RNG/license/provider trace; root owns integration and exact-scope rollback. Rejecting this provider does not invalidate accepted storage/player publication. Architecture skill: no change pending acceptance.
