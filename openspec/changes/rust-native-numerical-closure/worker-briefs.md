# Native numerical worker briefs

**Goal:** safe typed Rust access to the ten existing numerical families and one bounded Rust-native pathfinder. **Architecture:** contract types and test-double traits first, then independent immutable validated views → one existing algorithm → staged typed result; legacy byte layout exists only at FFI. **Tech stack:** Rust std, reusable caller-owned scratch, existing Go nativeabi for compatibility evidence. **Spec:** [the active delta](specs/rust-runtime-foundation/spec.md), [design](design.md), and current code/tests. **Global constraints:** current code and tests override the historical [kernel extraction](../archive/2026-09-21-rust-runtime-foundation/kernel-contracts.md). **Review focus:** no Rust→ABI serialization, no caller-reachable unchecked parser, no hidden output mutation, preserved tie/order/float semantics and no per-neighbor allocation.

`ENGINE` is `packages/engine/crates/mornlea_engine`. All source paths below are under `ENGINE/src`. The controller-owned interface landing defines types and operation traits in `src/native/contracts/`, publishes the module tree from `src/lib.rs`, and tests that independently written doubles compile and return the declared result types. Family workers edit only their exclusive implementation/test files named in `tasks.md`; they do not edit `src/native/contracts/`, `src/lib.rs`, `src/ffi.rs`, corpus manifest or shared test registration. The controller integrates each reviewed provider into the existing `ffi.rs` serially and owns corpus registration. Existing core topic tests stay in their modules; external native API cases live under `ENGINE/tests/native_contract/` with one family file each. Numerical migration cases live under `ENGINE/tests/numerical_migration/` with one family file each. The controller updates ENGINE/AGENTS.md in the interface task.

All requests below are public Rust contract types, with private validated view fields and immutable getters. The signature text spells the concrete provider method; the matching public trait has the same method arguments and result and is implementable by a consumer's test double. A provider validates public request fields on every call. A pointer obtained by unsafe callers is not a safe request constructor. `KernelError` is a non-exhaustive enum: InvalidInput, DisplacementOutOfBounds, OutputTooSmall{needed:usize,available:usize}, ScratchTooSmall{needed:usize,available:usize}, InvalidRegistry, EmissionOutOfRange, QueueOverflow, MissingHalo, OutputInvariant, Allocation. Needed/available are **typed element counts**, not bytes; FFI translates to the existing byte/count units. Family-specific path errors are separate. Try-reserve failures return Allocation where allocation is an explicit setup operation. Warm numerical calls perform no I/O and no scratch growth.

Internal reuse uses sealed, crate-private read traits: `CollisionCells::cell(index)->CollisionCell` and family-specific mesh/rescan accessors. Native typed slices and validated ABI little-endian byte views implement these traits. The algorithm is generic over or borrows these read views; do not copy/serialize all typed cells into ABI bytes. Parsing, pointers and output packing remain adapters. No unsafe typed cast of unaligned wire bytes.

The contract landing publishes the following operation traits. The request/result fields and constructor rules immediately below are normative for the referenced types; `&mut` scratch and cursor arguments confer exclusive call ownership. Each concrete family provider implements its trait without taking ownership of network, storage, or scene state. Test doubles can implement these traits without numerical code.

```rust
pub trait CollisionOp { fn resolve(&self, request: &CollisionRequest<'_>) -> Result<CollisionResult, KernelError>; }
pub trait PhysicsOp { fn step(&self, request: &PhysicsRequest<'_>) -> Result<PhysicsResult, KernelError>; }
pub trait RaycastOp { fn next_batch(&self, cursor: &mut RayCursor) -> Result<RayBatch, KernelError>; }
pub trait WorldgenOp { fn generate_chunk(&self, params: &WorldgenParams, chunk: [i32; 2], scratch: &mut WorldgenScratch, dst: &mut [u16]) -> Result<usize, KernelError>; }
pub trait ProbeOp { fn probe(&self, params: &WorldgenParams, queries: &[ProbeQuery], dst: &mut [ProbeValue]) -> Result<usize, KernelError>; }
pub trait TreeOp { fn tree_blocks(&self, request: &TreeRequest) -> Result<TreeBlocks, KernelError>; }
pub trait LodOp { fn build(&self, request: &LodRequest<'_>, scratch: &mut LodScratch, dst: &mut [LodQuad]) -> Result<usize, KernelError>; }
pub trait FluidEvalOp { fn evaluate(&self, items: &[[u16; 7]], dst: &mut [FluidWrites]) -> Result<usize, KernelError>; }
pub trait FluidRescanOp { fn rescan(&self, request: &RescanRequest<'_>, scratch: &mut RescanScratch, dst: &mut [[i32; 3]]) -> Result<RescanSummary, KernelError>; }
pub trait MeshOp { fn mesh(&self, view: &MeshView<'_>, scratch: &mut MeshScratch, dst: &mut [MeshQuad]) -> Result<usize, KernelError>; }
pub trait PathfindOp { fn find(&self, grid: &PathGrid, start: PathCell, goal: PathCell, scratch: &mut PathScratch) -> Result<PathResult, PathError>; }
```

Contract type declarations can define opaque scratch fields and private cursor state before provider methods exist; the corresponding constructors become callable in their owning provider node. Every trait is object-safe, including those taking borrowed views, so F2/F3 can inject a deterministic double behind `&dyn Trait`. The contract test compile-checks a distinct double for every trait and executes the fluid-evaluation double through a generic consumer for one success and one typed error; provider nodes later supply executable fixtures for every operation. `KernelError` has `Debug`, `Clone`, `PartialEq`, and `Eq`; result structs with floats compare component bit patterns in tests instead of deriving `Eq`.

For each node, add listed red/boundary tests first, then refactor the existing core behind the fixed API. Use only the exact file-owner and validation mapping in `tasks.md`; this brief supplies field maps and behavior, not competing task status. All new code comments are English.

Discover `native_contract` with the topic and `-- --list`, requiring nonempty behavioral cases. ABI integration runs `make rust` and the exact family-filtered `go test ./packages/shared/nativeabi` command in `tasks.md`. No foreground window. Every family contributes a package-local Go producer and named corpus cases; where a nativeabi wrapper panics on rejected status, use existing package-local raw test helpers in nativeabi, never new external cgo. Rust unit tests can directly exercise its own exported FFI. No production fallback implementation.

Legacy ABI remains11: symbols, layouts, sizes, successful outputs and status meanings stay pinned. Existing malformed-input bugs are corrected explicitly: aliased metadata is not cleared before validation; mesh stages payload; native rescan reports missing halo rather than panicking. The FFI mapping for previously caught internal invariant failures remains PANIC9. This is not permission to change valid gameplay results. Existing `catch_unwind` stays at FFI; native value validation does not use panic catching.

<a id="node-5-1"></a>

## 5.1 — Compile-ready common contract

Dependencies: accepted domain/protocol/storage source SHAs in their archived ledgers; no complete F1 or F2 acceptance claim. The controller owns `src/lib.rs`, `src/native/mod.rs`, `src/native/contracts/mod.rs` and one contract file per family, `ENGINE/AGENTS.md`, and `tests/native_contract.rs`. Add all types, error variants, exact trait signatures and shared typed-element capacity helper from this brief. Compile-check a deterministic test double for every trait, and execute one valid and one error result through a generic `FluidEvalOp` consumer. This tests that a later F2/F3 consumer can compile against the contract before concrete providers exist. No public method calls `todo!`, panics as an unavailable placeholder, or returns a synthetic production success. The trait-only contract is the deliverable. Run the named test with `-- --list` and normal execution, then `cargo clippy` for the engine crate. Commit `feat(engine): freeze native numerical contracts`.

ABI publication correction is a separate controller-owned integration node after the first provider: for mesh, LOD, eval, rescan validate ABI/pointer addresses, lengths/alignment and metadata↔all buffer overlaps **before clearing output_len**. A valid nonaliased metadata pointer is then cleared and remains zero on errors except exact-required overflow for LOD/rescan. Invalid/aliased metadata is never dereferenced or changed. Preserve each existing status precedence: invalid metadata pointer→INVALID_ARGUMENT, scratch failure→SCRATCH, output short as family-specific status. Pointer/lifetime validity beyond numeric range remains unsafe FFI caller responsibility.

Tests use canary-backed overlapping metadata/input, metadata/output and metadata/scratch for every affected family and assert all aliased bytes unchanged. Existing baseline writes zero too early. Test nonaliased valid metadata is reset on an invalid request. Topic `publication`; existing filter `metadata`; Go filter `Test.*(FailureAtomicity|InvalidArguments|InvalidBuffers)`. Commit `fix(engine): validate metadata aliases before publication`.

<a id="node-5-2"></a>

## 5.2 — Collision typed grid and result

Dependencies:5.1 contract only. Own `collision.rs`, `native/collision.rs`, and family tests; `ffi.rs` and contract types are read-only during provider work. Types frozen by 5.1:

```rust
pub struct Aabb { pub minimum:[f32;3], pub maximum:[f32;3] }
pub struct CollisionCell { loaded:bool, boxes:[Aabb;8], used:u8 }
pub struct CollisionGrid<'a> { origin:[i32;3], dimensions:[u32;3], cells:&'a [CollisionCell] }
pub struct CollisionRequest<'a> {
    pub position:[f32;3], pub displacement:[f32;3], pub began_grounded:bool,
    pub step_height:f32, pub grid:CollisionGrid<'a>,
}
pub struct CollisionResult {
    pub position:[f32;3], pub clipped:[bool;3], pub on_ground:bool,
    pub used_step:bool, pub hit_unknown:bool,
}
pub fn resolve_collision(request:&CollisionRequest<'_>)->Result<CollisionResult,KernelError>;
```

`CollisionCell::try_new(loaded:bool,boxes:[Aabb;8],used:u8)` checks used<=8 and finite components only in used boxes. It does not order/clamp AABBs or validate unused slots. `CollisionGrid::try_new(origin,dimensions,cells)` checks positive dimensions, product<=4096,exact count,checked origin+dim−1 and Y/X/Z cell layout `((y*dx)+x)*dz+z`. Grid is Copy/Clone borrowed metadata only.

Before core execution check finite request scalars and swept coverage using the exact formula in the archived [kernel extraction](../archive/2026-09-21-rust-runtime-foundation/kernel-contracts.md), Collision/Required coverage. No positivity constraint on step height. The algorithm retains Y→X→Z resolution, unknown-as-solid, step selection and ground probe. Return typed result directly; the controller's later FFI integration packs 16 bytes locally then copies once.

Cases: finite identity movement; wall/floor clipping; unknown cell; rejected alternate step does not leak unknown flag;4096/4097 cells;zero dimensions;origin endpoint overflow;missing swept cell;negative step height accepted when covered;AABB[-.25,1.5] accepted;unused NaN allowed/used NaN rejected. Compare typed result bits to existing ABI including negative zero. Topic/existing filter `collision`; Go `TestCollision`; commit `feat(engine): expose validated native collision`.

<a id="node-5-3"></a>

## 5.3 — Physics with explicit controls and tunables

Dependencies:5.1 contract. Own `step.rs`, `native/physics.rs`, and family tests; reuse the existing crate-private collision-parts core, so the native collision provider can land later. `ffi.rs` and contract types remain read-only. Types frozen by 5.1:

```rust
pub struct PhysicsState { pub position:[f32;3], pub velocity:[f32;3], pub on_ground:bool }
pub struct PhysicsControls {
    pub move_x:i8, pub move_z:i8, pub jump:bool,
    pub yaw_sin:f32, pub yaw_cos:f32,
    pub body_in_fluid:bool, pub sprinting:bool, pub sneaking:bool,
}
pub struct PhysicsTuning {
    pub fixed_delta_seconds:f32, pub step_height:f32, pub walk_speed:f32,
    pub ground_acceleration:f32, pub ground_deceleration:f32, pub air_acceleration:f32,
    pub jump_speed:f32, pub gravity:f32, pub terminal_fall_speed:f32,
    pub fluid_gravity:f32, pub fluid_sink_speed:f32, pub fluid_ascend_speed:f32,
    pub fluid_horizontal_drag:f32, pub sprint_speed_multiplier:f32, pub sneak_speed_multiplier:f32,
}
pub struct SweepBounds { pub minimum:[f32;3], pub maximum:[f32;3] }
pub struct PhysicsRequest<'a> {
    pub state:PhysicsState, pub controls:PhysicsControls, pub tuning:PhysicsTuning,
    pub sweep:SweepBounds, pub grid:CollisionGrid<'a>,
}
pub struct PhysicsResult { pub state:PhysicsState, pub clipped:[bool;3], pub used_step:bool, pub hit_unknown:bool }
pub fn step_physics(request:&PhysicsRequest<'_>)->Result<PhysicsResult,KernelError>;
```

All used floats finite,axes−1..1,sweep min<=max; do not impose tunable positivity or sin²+cos²=1. Integrate using existing f32 order, check displacement against min.next_down()/max.next_up(), then call shared collision **parts core**, not standalone resolve_collision's stronger swept-grid validator. Physics intentionally retains its existing prism acceptance. Clipped velocity becomes0; FFI packs32 bytes including reserved zeros.

Cases: floor landing;fluid gravity/drag; jump;sneak priority over sprint;one ULP outside sweep accepted/two rejected;inverted sweep,axis2,NaN;negative finite dt/tunable retained;grid accepted by physics but rejected by standalone swept coverage stays so. Use current step tests' concrete finite tunables as Go/ABI seeds, then toggle exactly one field per case. Topic `physics`; existing `physics_step`; Go `TestPhysicsStep`; commit `feat(engine): expose typed physics controls and results`.

<a id="node-5-4"></a>

## 5.4 — Opaque ray continuation

Dependencies:5.1 contract. Own `raycast.rs`, `native/raycast.rs`, and family tests; `ffi.rs` and contract types are read-only. `Ray {origin:[f32;3],direction:[f32;3],maximum:f32}` fields public; `RayCursor::try_new(ray:Ray)->Result<Self,KernelError>` owns validated ray and private continuation. `RayRecord {cell:[i32;3],face:RayFace,distance:f32}`, RayFace=Origin255,NegX0,PosX1,NegY2,PosY3,NegZ4,PosZ5; `RayBatch {records:[RayRecord;64],len:usize,done:bool}` has private unused slots and `records()->&[RayRecord]`; `RayCursor::next_batch(&mut self)->Result<RayBatch,KernelError>`. Compute from a local cursor copy and commit cursor only on success. Native caller cannot pair a cursor with a different ray or invent its bytes.

Validate finite origin/direction, nonzero direction,finite maximum>0. Direction need not normalize. Preserve negative floor, out-of-i32 floor→i32::MIN, wrapping cell advance, strict X/Y/Z ties,inclusive exact endpoint and existing exceptional infinity/NaN continuation arithmetic. No global traversal cap; each call<=64. FFI can restore its historically accepted byte cursor through a private validated adapter with the same existing weak history checks; no public cursor deserializer is added.

Cases origin(−0.25,0,0),direction(1,0,0),max2; equalXYZ direction tie;exact endpoint and next f32;65+ records across batches;done repeated returns empty/done;zero ray rejected without cursor publication;extreme floor/wrap;ABI tampered cursor matrix. Compare complete batch sequence and final cursor meaning, not first hit only. Topic/existing `raycast`; Go `TestRaycast`; commit `feat(engine): provide opaque raycast continuation`.

<a id="node-5-5"></a>

## 5.5 — World parameters and chunk generation

Dependencies:5.1 contract. The world-sampling lane owns `worldgen.rs`, `native/worldgen.rs`, and family tests for 5.5–5.8; the lane executes its own nodes serially while other lanes proceed. `ffi.rs` and contract types are read-only. Public Materials has exactly 15 u16 fields from the archived [kernel extraction](../archive/2026-09-21-rust-runtime-foundation/kernel-contracts.md). `WorldgenParams::try_new(seed:i64,materials:Materials,perm:[u8;512])->Result<Self,KernelError>` stores private fields; material IDs pairwise distinct except water==air. Perm bytes are opaque 512 bytes, not revalidated as a permutation. `WorldgenScratch::try_new()->Result<Self,KernelError>` allocates a boxed 98304-u16 stage on heap. `generate_chunk(params:&WorldgenParams,chunk:[i32;2],scratch:&mut WorldgenScratch,dst:&mut[u16])->Result<usize,KernelError>` requires capacity>=98304 and returns98304, leaving extra destination unchanged. Full preflight before scratch/core; stage then one typed copy. FFI still requires **exact 196608 output bytes**, encodes stage LE and publishes once.

Coordinate ruling: preserve release-runtime two's-complement semantics for every i32 input and make debug/release deterministic. Chunk base is `chunk.wrapping_shl(4)`; x/z fringe/root/difference arithmetic in tree/short-grass lookup uses explicit wrapping_add/sub. Keep signed shifts and the existing ascending inclusive candidate ranges after computing their wrapped endpoints; an inverted range is empty, not a wrapped traversal. Do not widen a wrapped range into billions of iterations or reinterpret it as a toroidal world. Y remains bounded−64..319 with ordinary checked/in-range arithmetic. Existing seed/hash wrapping is unchanged. Audit these operations in generate_chunk,apply_oak_trees,apply_short_grass,tree_block_at and oak_tree_block_at as one node; do not disable overflow checking globally. This makes a formerly debug-only overflow an ordinary deterministic result matching deployed release ABI, not a new coordinate restriction.

Cases seeds0/1/−1,chunk(0,0),(−1,2),permutation allzero accepted;water==air accepted/another duplicate rejected;exact/short output;all section sentinels;pointwise parity;chunks whose base is i32::MIN and i32::MAX−15;probe fringe at signed extremes in5.6. Freeze extreme results from current **release** ABI before arithmetic refactor, then compare debug and release Rust bit-for-bit. No new worldgen algorithm/noise normalization. Topic `worldgen_chunk`; existing `worldgen`; Go `TestWorldgen`; commit `feat(engine): expose bounded native chunk generation`.

<a id="node-5-6"></a>

## 5.6 — Typed world probes

Dependencies:5.1 contract; integration with 5.5 concrete sampler happens in the world lane. Own probe path in `worldgen.rs`, `native/world_probe.rs`, and probe tests. `ProbeQuery {Height{x:i32,z:i32},Terrain{position:[i32;3]},Base{position:[i32;3]}}`; `ProbeValue {Height(i32),Block(u16)}`. `probe_world(params:&WorldgenParams,queries:&[ProbeQuery],dst:&mut[ProbeValue])->Result<usize,KernelError>` accepts1..64,capacity>=N. Compute into a fixed64-result local stage then copy used prefix. FFI raw mode0 ignores suppliedwy, modes1/2 map positions; invalid mode rejects;unused output field/reserved byteszero; exact8N-byte output preserved.

Cases0/1/64/65 queries;each mode;terrain out-of-height returnsair;Height ignores wy in ABI;Base seeded tree decoration;arbitraryi32 X/Z including±3 fringe overflow uses5.5 wrapping ruling;mutated perm retained;output short unchanged. Topic `worldgen_probe`; existing `probe`; Go `TestWorldgenProbe`; commit `feat(engine): expose typed bounded world probes`.

<a id="node-5-7"></a>

## 5.7 — Runtime tree blocks

Dependencies:5.1 contract. Own runtime-tree path in `worldgen.rs`, `native/tree.rs`, and tree tests within the world lane. `TreeRequest {seed:i64,root:[i32;3]}`; `TreeBlock {offset:[i8;3],block:u16}`; `TreeBlocks` privately stores `[TreeBlock;128]`,len with read-only slice. `tree_blocks(request:&TreeRequest)->Result<TreeBlocks,KernelError>` validates rootY−64..311,X/Z±2 representable, all seeds accepted. Emit existing ordinary oak dy/dz/dx order,offsetX/Z−2..2,Y0..8,blocks17/19; no rare worldgen crown feature added. Fixed local128 buffer replaces per-result Vec; overflow returns OutputInvariant, never truncates. FFI packs4+8N bytes and requires only actual capacity; larger output suffix stays unchanged.

Cases rootsY−64/311 allowed,−65/312 rejected,X=i32::MIN+2/MAX−2 allowed and one beyond rejected;all geometry offsets/order;seeds producing height5/6/7 and fluffy crown from existing tests;N<=128 (existing max65);output3 bytes/actual−1 unchanged. Topic/existing `tree_blocks`; Go `TestTreeBlocks`; commit `feat(engine): expose bounded runtime tree geometry`.

<a id="node-5-8"></a>

## 5.8 — LOD shell with reusable samples and stage

Dependencies:5.1 contract; integration with 5.5 concrete sampler happens in the world lane. Own `lod.rs`, `native/lod.rs`, and LOD tests. `LodStep {Two,Four,Eight}`; `LodRequest<'a>{params:&'a WorldgenParams,tile:[i32;2],step:LodStep}`; public immutable LodQuad fields/types exactly in the archived [kernel extraction](../archive/2026-09-21-rust-runtime-foundation/kernel-contracts.md), LodFace same5 variants. `LodScratch::try_new()->Result<Self,KernelError>` owns max34² sample records and3136-quad stage. `build_lod(request:&LodRequest,scratch:&mut LodScratch,dst:&mut[LodQuad])->Result<usize,KernelError>` checks tile*64,base−8,base+64 before work; retains existing aggregation/merge/skirt ordering. N=64/step,stage cap3N²+2N=3136/800/208. Compute exact required count, return OutputTooSmall with count and unchanged destination or copy prefix. No scratch reallocation in warm calls. FFI translates needed*20 to exact overflow output_len and preserves old null-output rules.

Cases empty tile;uniform one top;boundary taller side owns skirt;sea clamp;all three steps;short0/needed−1;tile multiplication/extrema;water==air gate;34² sample reuse after different step. Topic/existing `lod`; Go `TestLodShell`; commit `feat(engine): bound native lod scratch and output`.

<a id="node-5-9"></a>

## 5.9 — Fluid evaluation with native bounded batch

Dependencies:5.1 contract. Own `fluid_eval.rs`, `native/fluid_eval.rs`, and eval tests; `ffi.rs` and contracts are read-only. `NeighborSlot {SelfCell,Above,Below,PosX,NegX,PosZ,NegZ}`; `FluidChange {slot:NeighborSlot,block:u16}`; `FluidWrites` owns fixed4 changes and private len, with Default representing no writes. `eval_fluid_batch(items:&[[u16;7]],dst:&mut[FluidWrites])->Result<usize,KernelError>` accepts0..4096; input order self/above/below/+x/−x/+z/−z. Allu16 blockIDs accepted;unknown IDs retain nonfluid/nonreplaceable behavior. PreflightN and destination first; after that eval_one is a total allocation-free function and writes one result per input with no remaining fallible branch. Zero batch succeeds without touchingdst.

FFI legacy count has **no4096 cap**. Validate exact8+14N byte input with checked arithmetic and actual length first; allocate one complete output stage of12N bytes (bounded by representable validated input), evaluate chunks of<=4096 through the same core, then copy once. Preserve zero input acceptance and non-null output rule; short output remains INVALID_ARGUMENT2, notOVERFLOW7. This compatibility allocation path is not the new native hot API and cannot be called from new Rust authority. It can be removed only after its Go callers retire.

Cases all existing fluid source/death/horizontal/vertical/infinite-source rules;unused entries encode(slot255,block0);N0/1/4096 accepted,N4097 native rejects while valid ABI4097 succeeds;short destination unchanged;unknown65535 accepted;vertical priority over horizontal;all 4 outputslots contiguous. Topic/existing `fluid_eval`; Go `TestFluidEval`; commit `feat(engine): expose bounded native fluid batches`.

<a id="node-5-10"></a>

## 5.10 — Fluid rescan safe halo and explicit continuation

Dependencies:5.1 contract. Own `fluid_rescan.rs`, `native/fluid_rescan.rs`, and rescan tests; `ffi.rs` and contracts are read-only. Types:

```rust
pub enum RescanSection<'a> { Uniform(u16), Dense(&'a [u16;4096]) }
pub struct RescanView<'a> {
    sections:[RescanSection<'a>;24], skirt:&'a [[u16;384];68],
    metadata:&'a [Option<u16>;216], center:[i32;2],
}
pub struct RescanRange { pub x0:u8, pub x1:u8, pub z0:u8, pub z1:u8 }
pub struct RescanRequest<'a> { pub view:RescanView<'a>, pub range:RescanRange, pub start_section:u8, pub budget:u32 }
pub struct RescanSummary { pub written:usize, pub spent:u32, pub done:bool, pub next_section:u8 }
pub fn rescan_fluids(request:&RescanRequest<'_>,scratch:&mut RescanScratch,dst:&mut[[i32;3]])
    ->Result<RescanSummary,KernelError>;
```

View::try_new checks center*16,base−1,base+16, while typed lengths enforce the24/68/216 shapes. Metadata order stays9 chunks×24 with existing chunk_meta_index; skirt index/order is exactly existing skirt_column. No block registry/metadata-consistency validation added. Native ranges use **existing box coordinates1..16**, closed,ordered;start<24. A one-cell halo is therefore guaranteed. `RescanScratch::try_with_capacity(max_positions:usize)` allows up to124416 typed positions, no warm growth. Native required worst capacity is16*A*(24−start); reject undersized scratch before scanning. Destination may be smaller: compute exact output into scratch, then return needed or copy once.

Algorithm preserves section-before-budget check and entered-section completion,uniform fast paths,center section precedence over metadata,Y/Z/X scan order. Native summary adds next_section without changing ABI bytes: on budget break it is the unentered section, on completion24. Budget0 returns written0,spent0,donefalse,next=start. Max native overshoot4095; spent fitsu32. Unknown/out-of-worldY reads barrier as before.

Legacy FFI adapter still accepts outer range0..17. Internal shared view reads X/Z outside available halo as `Err(MissingHalo)` when actually accessed; do not silently invent barrier or read outside the slice. Preserve formerly successful outer/nonfluid scans. MissingHalo maps to existing PANIC9 for the legacy call (previously a caught index panic), with unchanged payload and metadata0; native interior requests cannot trigger it. Outer maximum124416 andovershoot5183 remain confined to legacy adapter. FFI output summary remains8 bytes and overflow reports exact12N+8.

Cases uniform nonfluid spent1;sealedsource;unsealededge;dense mixed exact positions;budget0/1 and section overshoot;start23;native range0 rejected;legacy outer air succeeds;legacy outer source accessing absent halo yields9 with canaries;metadata contradiction uses actualcenter;short output exactrequired;reuse after error;next_section matches Go accounting replay. Topic/existing `fluid_rescan`; Go `TestFluidRescan`; commit `feat(engine): make native fluid rescan halo-safe`.

<a id="node-5-11"></a>

## 5.11 — Mesh/light typed views and atomic geometry

Dependencies:5.1 contract. Own `input.rs`, `light.rs`, `quad.rs`, `greedy/` emitted-model files, `native/mesh.rs`, and mesh tests; `ffi.rs` and contracts are read-only. `MeshRegistryEntry` fields exactly id:u16,opaque:bool,emission:u8,material:[u16;6],fluid_height:u8,light_attenuation:u8,block_top_raw:u8,model:MeshModel; MeshModel=Default0/StandingTorch1/WallTorchPosX2/WallTorchNegX3/WallTorchPosZ4/WallTorchNegZ5/Bed6; Default includes full, short, fluid and plant forms. `MeshRegistry::try_new(entries:&[MeshRegistryEntry],visibility:&[u64],air:u16,barrier:u16)` validates1..96 entries,strictID order,sentinels,word countR*ceil(R/64),and current semantic ranges. Native view contains `blocks:&[u16;110592]`, `heights_present:&[bool;9]`, `heights:&[[i16;256];9]`,section_origin_y:i32,registry. Native typed views are fully validated; FFI all-air shortcut retains its **structural-only** validation exemption for unused raw registry.

`MeshScratch::try_new()` owns levels110592u8,queue110592u32 and a **40960-quad staging capacity**. `MeshQuad` privately stores existing Quad fields (x/y/z/w/h:u8,face,material:u16,ao/light:u8,corners:[u8;4],back:bool), with read-only getters and `packed()->u64` after checked construction. MeshQuad implements Default as a valid unit NegX quad at(0,0,0), material0,ao0,light0,zero corners,backfalse solely to initialize caller output storage; only the returned prefix is meaningful. `mesh_section(view:&MeshView,scratch:&mut MeshScratch,dst:&mut[MeshQuad])->Result<usize,KernelError>` returns valid prefix count, untouched destination on error; scratch may mutate and is reset before reuse. Packing is a render-consumer representation, not an ABI request serialization.

Capacity ruling:24576 is the bound for current **production-consistent** registry. A structurally/semantically accepted custom registry can combine plants with another emission path. Use conservative4096*(6 axial+4 plant)=40960; model path emits<=5 **instead of** axial, so combined model+plant<=9/cell is covered. Preserve these accepted combinations; do not silently add registry restrictions. Every emission constructs a checked MeshQuad:unitplant/zero corners,axial no plant material/no back,width/height1..16,unit cornerquad/corners<=15,x/y/z0..15. A packing inconsistency returns OutputInvariant before publishing anything; FFI maps it to oldPANIC9. Geometry retains face/slice/row/plant/model emission order and merge rules. No per-quad allocation.

Light owns its separate boundedqueue;resetlevels/tail;EmissionOutOfRange→6,QueueOverflow→8. Native semantic range errors fail before light. Compute complete geometry in stage; ifdestination too short return needed, otherwise copy once. FFI preserves preflight output minimum24576u64 even forall-air; converts stagedquads into a local/reusable packing stage then publishes only success; late actualcount>declared capacity→OVERFLOW7 with count0 and **unchanged payload**. Legacy scratch ABI stays552960 bytes/8-byte alignment; new Rust MeshScratch is separate and is not stuffed into that buffer. FFI may allocate its additional boundedstage; measure/report its cost, no extra production fallback.

Cases isolated six faces;all-air native and raw semantic-invalid all-air FFI;max registry96/97;unknown block IDs;fluidheight1 accepted and15 rejected;model6 accepted/7 rejected;source emission16;queue exact/one short;meshplants,torches,bed; constructed model+plant exceeding6/cell;axial plant packing failure after an earlier validquad;late shortoutput canary;aliasmetadata;warm reset dark after bright. Compare all output quad bits/order viaABI, plus retained scratch counters. Topic `mesh`; existing `greedy` and `light` and `input`; Go `TestMeshSection`; commit `feat(engine): stage safe native mesh and light output`.

<a id="node-5-12"></a>

## 5.12 — Immutable path grid and Go corpus

Dependencies:5.1 contract. Own new `pathfind.rs`, `native/pathfind.rs`, `tests/numerical_migration/path_grid.rs`, and `packages/tools/cmd/runtime-oracle/pathfind_test.go`. No search implementation yet. Types: `PathCell {x:i32,y:i32,z:i32}`, `PathRevision {chunk:[i32;2],revision:u64}`, `PathBlockTable` private `[bool;65536]` stored boxed to avoid stack construction. `PathBlockTable::from_passable_ids(&[u16])->Result<Self,PathError>` accepts duplicateIDs idempotently and defaults every unspecified IDblocked. `PathGrid::try_new(origin:PathCell,size:[u32;3],blocks:Box<[u16]>,passability:PathBlockTable,revisions:Vec<PathRevision>)->Result<Self,PathError>` takes ownership without copying block arrays. PathError=InvalidGrid,InvalidRevision,ScratchTooSmall,Unreachable,BudgetExceeded,Allocation. Grid exposes no mutable blocks/revision/table getters.

Validate sizepositive,checked cell count<=131072,exactblocks length,checkedorigin+size−1 in i32;max9 revisions **before** dedup;sortchunkX/Z,equal-revision duplicates collapse,conflicts reject. Index=`((x*size_z)+z)*size_y+y`. Standing means feet/head passable and supporting cell in-bounds/nonpassable;unknown blocks are blocked. Grid constructor does not validate a start/goal yet. Production33×9×33=9801 cells;no runtime chunkloading/cooldown/cancellation state.

Go adapter invokes pathfind.NewPathBlockTable/NewPathGrid/FindPath, using passableIDs[air0] and explicit flat floor block1. Cases size0/product overflow/131072/131073,wrong block length,coordinate overflow,9/10 revisions,equal/conflicting duplicate revisions,negative origin,Y-fast distinguishing layout,caller-owned source mutation cannot change an owned snapshot. Constructor failures return no partial grid. Run `rustup run 1.97.1 cargo test --manifest-path packages/engine/Cargo.toml -p mornlea_engine --test numerical_migration --locked path_grid`; `go test ./packages/tools/cmd/runtime-oracle -run '^TestPathfindOracleGrid$' -count=1`. Commit `feat(engine): validate immutable pathfinding grids`.

<a id="node-5-13"></a>

## 5.13 — Indexed-heap A* preserving Go choices

Dependencies:5.12. Own `pathfind.rs` algorithm, `tests/numerical_migration/path_search.rs`, and extend the Go pathfind producer. `PathScratch::try_with_capacity(cells:usize)->Result<Self,PathError>` owns cell-indexed g:u32,parent:usize,state,generation,heap-position,insertion ordinal arrays and a minheap of cell indices. `find_path(grid:&PathGrid,start:PathCell,goal:PathCell,scratch:&mut PathScratch)->Result<PathResult,PathError>`; PathResult privately owns boxed waypoints and cloned/small normalized revisions. Only successful final result allocates ownedpath;warm search itself no allocations. No borrowed result pointing into scratch. Validate endpoints standing,then scratch capacity. start=goal returns one waypoint after standing validation.

Algorithm pseudocode is normative:

```text
reset generation (on wrap clear generation/state arrays); insertion_counter=0
insert start: g=0,parent=none,ordinal=insertion_counter++
while heap not empty:
    if expanded == 4096: return BudgetExceeded
    pop minimum by (g + abs(dx_to_goal) + abs(dz_to_goal), ordinal)
    mark closed; expanded += 1
    if cell == goal: reconstruct parent chain, reverse, publish owned result
    for direction in [-X,+X,-Z,+Z]:
        consider flat(cost1), jump_up(cost2), fall_one(cost1), gap_two(cost2)
        evaluate and emit every admissible transition independently, in this order
        if destination closed: skip
        candidate = checked(g + cost)
        if unseen: set g,parent,first ordinal; heap insert
        else if candidate < g: update g,parent; heap decrease-key, retaining ordinal
        else: retain original parent and heap ordinal
return Unreachable
```

Transition predicates are **four independent checks**, not an else-if chain: emit flat if adjacent standing; emit jump if raised adjacent standing and current upper-head clearance; emit fall if lowered adjacent standing; emit gap if two-away standing and intermediate feet/head passable. A flat step and a gap step may both be emitted in one direction, so suppressing gap after flat changes insertion ties and paths. Invalid/nonstanding endpoints return Unreachable, matching Go, not a new failure category. Coordinates checked/widened before conversion; Manhattan computed in i64/u64,not overflow-prone i32. Heap keyf is u64. The heuristic ignoresY. No reopening closed nodes,wall-clock budget or neighbor allocation. Expansion budget is checked before the4097th pop; goal popped as4096 succeeds,goal requiring4097 is BudgetExceeded;emptyfrontier after4096 isUnreachable. Scratch resets on every call,including failure. Result construction checksparent chain length<=cells.

Concrete corpus: floor corridor start(1,1,1)→(3,1,1);one-block step and blocked head;one-cell drop;two-cell gap and blocked intermediate head;diamond symmetric routes freezing first insertion;fixture requiring decrease-key;equal-g parenttie;start=goal;blocked feet/head/support;unknownblock65535;unreachable closed room;generated corridors withgoal expansion4096/4097;success→failure→success scratch reuse;retain first result while running again. Go FindPath supplies exact waypoints/revisions, not handcrafted Rust goldens. Verify first Go rule against its existing transition tests during red setup; if fixture construction contradicts the frozen rule, report to controller rather than altering ordering.

Run `go test ./packages/shared/pathfind -count=1`; `go test ./packages/tools/cmd/runtime-oracle -run '^TestPathfindOracleSearch$' -count=1`; `rustup run 1.97.1 cargo test --manifest-path packages/engine/Cargo.toml -p mornlea_engine --test numerical_migration --locked path_search`. Report small/9801/max/budget allocations and informational timing. Commit `feat(engine): preserve deterministic paths with indexed a star`.

<a id="node-5-14"></a>

## 5.14 — Numerical corpus closure

Dependencies:all reviewed providers and serial ABI integrations for 5.2–5.11, plus 5.13. Controller owns aggregate Go `packages/tools/cmd/runtime-oracle/kernel_test.go`, Rust native-contract/numerical-migration registration, and `testdata/runtime-migration/contracts.json`/case assets. Each earlier node already implements its producer; this node executes all ten families plus pathfinding, records warmed allocation/retained scratch and validates every supported family is callable. Cases exercise success,invalid input,exact capacity,short capacity,reuse and ordering for each API;fixed-result APIs use the corresponding FFI capacity case.

Existing-kernel comparison is safe Rust versus established ABI calling the **same numerical core**, proving boundary equivalence; it is not advertised as a second independent algorithm. Pathfinding compares actual Go algorithm against Rust. For every seeded family runseed0 and1;seed-independent operations recordnot-applicable rather than fabricated divergence. Run `go test ./packages/tools/cmd/runtime-oracle -run 'TestKernelOracle|TestPathfindOracle' -count=1`, `go test ./packages/shared/nativeabi -count=1`,full ENGINE tests and both nonempty integration targets. Commit `test(engine): close native numerical compatibility coverage`.

## Rollback and exclusions

Each family rollback reverts that native wrapper/core refactor and its FFI adaptation together; retain already accepted unrelated families. Protocol/storage/domain do not gain a production dependency on engine. Kernel work contains no simulation tick owner,task queue,cooldown, retry,async world loading,Godot,renderer context or graphics window. Full acceptance6.3 waits for these nodes; it does not block their start.
