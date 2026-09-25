## Context

The code currently exposes ten numerical operations through engine ABI v11, but `mornlea_engine/src/lib.rs` keeps their Rust modules private. The frozen migration inventory names those ten plus `kernel.pathfind`; none is complete numerical evidence yet. The accepted F1 domain/event, protocol, region and storage changes establish adjacent contracts, while F2 remains planning-only. The final ownership direction is Rust server and client core over a windowless Rust numerical owner, with Godot/Python restricted to presentation.

The historical foundation [kernel plan](../archive/2026-09-21-rust-runtime-foundation/plans/05-kernel.md) records field-level compatibility choices. This change copies the relevant executable decisions into [worker-briefs.md](worker-briefs.md), then changes the dependency topology so implementation can proceed by independent family after a contract landing. Current code and tests win where the historical extraction is stale.

## Goals / Non-Goals

**Goals:** Freeze the native input/output/error contract first; allow independent family implementations and consumer mocks; preserve one numerical algorithm per operation; close all eleven numerical corpus routes with actual execution.

**Non-Goals:** Online Rust authority, new gameplay, renderer work, raw ABI use from new Rust callers, or declaring all of F1 accepted before the separate integrated zero-gap gate.

## Decisions

### Interface first, then independent lanes

`mornlea_engine::native::contracts` owns request/result types, `KernelError`, capacity units, and a public operation trait per independent family. A single interface node creates the module tree and compile-only contract tests, including test-double implementations; it does not ship an unavailable production fallback or claim numerical parity. Every concrete provider and its tests live in a family-owned `native/<family>.rs` module. Contract fields are private to external crates but `pub(crate)` where family implementations need access; family modules add inherent constructor implementations without editing the frozen type declarations. Common exports, contracts, and `lib.rs` have one controller-owned landing and remain read-only for family workers. A worker that needs a contract change reports the conflict; the controller revises the design, contracts, and all consumers before implementation resumes.

The shared type owners are fixed: collision owns `CollisionGrid` consumed by physics; world generation owns `WorldgenParams` consumed by probe and LOD; `KernelError` owns typed element-count failures; pathfinding owns its own `PathError` and immutable snapshot. F2/F3 consumers can compile and test against operation-trait doubles before concrete providers land. Provider work reuses the already present crate-private collision/world algorithms; final native/ABI integration checks the shared-core edges after the related providers are reviewed. Interface stability makes work start independent; it does not erase the final integration dependency.

| Lane | Public operation signature or contract | Read-only peer contract | Exclusive implementation files |
| --- | --- | --- | --- |
| Collision + physics | `CollisionOp::resolve(&CollisionRequest) -> Result<CollisionResult, KernelError>`; `PhysicsOp::step(&PhysicsRequest) -> Result<PhysicsResult, KernelError>` | `CollisionGrid`, `CollisionCells` and `PhysicsTuning` contract definitions | `src/collision.rs`, `src/step.rs`, `src/native/collision.rs`, `src/native/physics.rs`, matching tests |
| Ray traversal | `RaycastOp::next_batch(&mut RayCursor) -> Result<RayBatch, KernelError>` | Private cursor state, public `Ray`/records | `src/raycast.rs`, `src/native/raycast.rs`, matching tests |
| World sampling | `WorldgenOp::generate_chunk(&WorldgenParams, [i32;2], &mut WorldgenScratch, &mut [u16]) -> Result<usize, KernelError>`; `ProbeOp::probe(&WorldgenParams, &[ProbeQuery], &mut [ProbeValue]) -> Result<usize, KernelError>`; `TreeOp::tree_blocks(&TreeRequest) -> Result<TreeBlocks, KernelError>`; `LodOp::build(&LodRequest, &mut LodScratch, &mut [LodQuad]) -> Result<usize, KernelError>` | One `WorldgenParams` owner; read-only sampling contract | `src/worldgen.rs`, `src/lod.rs`, `src/native/worldgen.rs`, `src/native/world_probe.rs`, `src/native/tree.rs`, `src/native/lod.rs`, matching tests |
| Fluids | `FluidEvalOp::evaluate(&[[u16;7]], &mut [FluidWrites]) -> Result<usize, KernelError>`; `FluidRescanOp::rescan(&RescanRequest, &mut RescanScratch, &mut [[i32;3]]) -> Result<RescanSummary, KernelError>` | Stable neighbor order and halo view | `src/fluid_eval.rs`, `src/fluid_rescan.rs`, `src/native/fluid_eval.rs`, `src/native/fluid_rescan.rs`, matching tests |
| Mesh and light | `MeshOp::mesh(&MeshView, &mut MeshScratch, &mut [MeshQuad]) -> Result<usize, KernelError>` | Registry and quad semantic types | `src/input.rs`, `src/light.rs`, `src/quad.rs`, `src/greedy/`, `src/native/mesh.rs`, matching tests |
| Pathfinding | `PathfindOp::find(&PathGrid, PathCell, PathCell, &mut PathScratch) -> Result<PathResult, PathError>` | Owned snapshot and passability table | new `src/pathfind.rs`, `src/native/pathfind.rs`, matching tests |

The field maps, constructors, accepted raw values, limits and error precedence for these signatures are pinned in [worker-briefs.md](worker-briefs.md). The family methods above are traits so an F2/F3 consumer can compile with a test double as soon as the contract node lands. Concrete providers are stateless; scratch is caller-owned and `&mut` ensures one active owner. All result values are owned or tied only to immutable request input. No trait permits storage, session, Godot, or network access.

### Shared algorithm and ABI integration

Family workers expose the existing algorithm through typed validated views rather than serialize an ABI request or copy whole voxel grids. Existing little-endian parsing and pointer checks remain in `ffi.rs`; crate-private view traits let the algorithm consume either validated native slices or validated ABI byte views. The controller alone integrates family adapters in `ffi.rs` after each family is reviewed, in separate scoped commits. ABI v11 valid outputs and status meanings remain frozen. Metadata alias validation precedes any metadata clear; mesh stages its complete payload before publication. Caught internal invariants map to status 9 as before; native calls return a typed error without unwinding.

### Work bounds and error policy

Collision/physics each admit at most 4096 cells. Ray batches contain at most 64 records. A chunk is exactly 98,304 `u16` cells; probes admit 1–64 queries; trees at most 128 blocks. LOD stages at most 3136 quads. Native fluid evaluation admits 0–4096 items; ABI v11 retains its already-validated larger batch behavior. Native rescan admits interior coordinates 1–16 with a one-cell halo, at most 98,304 emitted positions and section overshoot at most 4095 work units. Mesh stages at most 40,960 quads and uses the existing fixed 110,592-cell light workspace. Path grids admit at most 131,072 cells, nine revisions before deduplication and 4096 expansions. All size arithmetic is checked before allocation; destination capacity errors count typed elements, not ABI bytes. A failed call leaves destination bytes/elements unchanged. Scratch may mutate but resets before reuse.

### Pathfinding compatibility

The immutable grid uses Y-fast indexing `((x * size_z) + z) * size_y + y`, an injected passability table and sorted `(chunk_x, chunk_z)` revisions. Unknown block IDs are blocked. Expand `-X,+X,-Z,+Z`; for each direction independently consider flat, jump-up, fall-one and gap-two, with costs 1,2,1,2. Use horizontal Manhattan distance, first-insertion tie order, strictly better `g` replacement and an indexed heap. Check the expansion budget before the 4097th pop. Endpoint standing validation precedes start=goal. Results own their waypoints and revisions. Go pathfinding is an offline oracle only; there is no Go production fallback.

### Evidence and integration graph

The contract node is the only start dependency for the six lanes. Within world sampling, concrete chunk/probe/LOD integration follows the shared sampler implementation; contract-double tests can run earlier. FFI adaptation follows a reviewed family, not the other lanes. The corpus owner merges family fragments and runs all eleven routes only after every lane and adapter is integrated. Final F1 acceptance is separate and can consume this numerical closure together with accepted domain/protocol/storage evidence. `tasks.md` is the sole status source; the linked briefs are execution detail without duplicate checkboxes.

## Risks / Trade-offs

- Interface types can drift from the legacy accepted inputs → compare every field and failure boundary with current code and engine ABI tests before contract landing; revise the frozen contract centrally if the evidence contradicts it.
- A parallel native wrapper can accidentally become a second numerical algorithm → require family core reuse and a post-review ABI adapter that invokes the same core, plus native/ABI bitwise comparison.
- Mesh and world generation can expose debug-only overflow or partial writes → pin release ABI results at extremes before changing arithmetic and use canaries for late failure publication.
- Broad concurrent edits can collide in shared exports or `ffi.rs` → contract and ABI integration are controller-owned serial points; family workers receive exclusive files and isolated checkouts.
- A green manifest can hide no-op tests → require nonempty test discovery, actual operation dispatch, scalar/path mutations, source digests and per-family case counts.

## Migration Plan

Land the interface contract with compiler and test-double evidence. Implement six independent lanes, reviewing and committing each behavior node with its Go/ABI source-bound cases. Integrate FFI adapters one family at a time, preserving ABI v11. Merge the eleven corpus routes and run numerical closure, then pass its evidence to the separate complete F1 acceptance. Rollback reverts the affected native provider, adapter and corpus fragment together; no default runtime or save is switched.
