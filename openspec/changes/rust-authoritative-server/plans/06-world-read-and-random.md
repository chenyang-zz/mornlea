# Ready world reads and deterministic random rules

This refines the approved S1 provider work without changing wire/save schemas.
The controller uses the installed brainstorming and writing-plans workflow;
tasks.md remains the only status checklist. Baseline: `4eaae498`.

## Decision and shared contract

Choose indexed compact Ready chunks plus the existing sparse mutation overlay.
Do not expand every chunk into a global cell map or let each consumer invent
readiness/sky from missing cells. The new shared node 2.4d0 is accepted before
random rules, snapshot projection and the reducer consume it. The controller
owns integration/rollback and every shared edit below.

Editable: `S/src/core/world.rs` (new), `S/src/core/mod.rs`,
`S/src/core/state.rs`, `S/src/core/contracts.rs`, `S/AGENTS.md`,
`S/tests/server_contract/world_read.rs` (new), its existing test entry, and this
change's design/tasks/ledger. All provider files, Go sources and F1 codecs are
read-only. No edited file is a hashed migration-corpus input.

`ReadyChunk::try_new(key:ChunkKey,generation:u64,revision:u64,chunk:storage::Chunk)
->Result<ReadyChunk,ServerError>` validates with the F1 logical validator,
retains owned compact data behind Arc, and builds a 256-column height cache.
This preparation is off-tick worker/fixture work, never a synchronous load in
a rule. `TickContext::preload_ready_chunk(ReadyChunk)` installs that checked
value without decoding. Fixture initialization installs its declared chunks;
invalid fixture chunks fail loudly at the fixture-only constructor. Readiness
is independent of sparse `preload_block` fixture observations.

`AuthorityReadView::ready_chunk(key)->bool` and
`highest_non_air(dimension,x,z)->Option<i32>` expose only Ready state. Empty
column is -65; every non-air block counts, including crops, water and glass.
Block/observation lookup derives chunk coordinates using arithmetic shift and
does an exact BTreeMap lookup, then compact base fallback; it never scans the
world map. Out-of-height lookup is None. Sparse fixtures retain their existing
observation semantics. Existing staged writes are visible before the next
sample. Raising a column is constant time; removing its top scans at most384
indexed cells. Compounds restore the height cache on refusal.

Keep per-cell staged CAS revisions separate from durable chunk revision. Go
Dimension.SetBlock does not change durable revision; Mutation.Commit advances
it once per changed chunk. `snapshot_state` carries declared chunks, applies
the sparse final writes, and advances each changed chunk once (unchanged chunks
retain their compact bytes). Repack only changed sections into valid Direct15
storage; do not add a codec or mutate a retained immutable base. Preserve
unchanged drop/furnace/chest slots. Explicit snapshot materialization is an
off-hot-path replay operation here; actual reducer ownership remains3.1.
Reject a base revision u64::MAX before any changed Ready write so snapshot
cannot silently wrap. Snapshot preserves generation. Add read-only tunable
getters `random_attempts()->u8` and `crop_growth_percent()->u8`; constructor
ceilings remain64/100. No tunable or provider defaults are duplicated.

Tests first use compiling empty Ready stubs to fail `ready_air_and_missing`,
`negative_boundary_and_packed`, `height_tracks_overlay_and_snapshot`,
`multiwrite_one_durable_revision`, `max_revision_refuses`,
`fixture_loads_ready_chunks`, and `invalid_chunk_refuses` assertions.
Cover Single/Indexed/Direct compact reads, missing versus air, -1/-16/-17
coordinates, transparent non-air, top removal, failed multi-chunk transaction,
retained base/slots, immutable snapshot, and both tunable getter endpoints.
Run world_read filter, full server crate, fmt/clippy, Go world height and realm
mutation oracles, strict change validation and diff checks. Commit
`fix(server): complete indexed ready world reads` before dependent providers.

## Random provider refinement

Exclusive files and source algorithms remain plan04's random packet. Consume
the accepted shared SHA. API:
`run(&mut TickContext,RuleCall)->Result<PhaseReport,ServerError>` checks the
RandomBlock batch shape; `advance(&mut TickContext,active:&[ChunkKey])
->Result<PhaseReport,ServerError>` executes the body. The reducer supplies its
already bounded Ready active set; sort/deduplicate it, ignore unavailable keys,
then visit 24 sections and at most64 samples each. No sync loads. Duplicate
active keys do not repeat simulation. Reject chunk coordinates whose whole
16-cell span does not fit i32 before any sample/mutation. Active-set admission
and subscription resource policy remain the reducer, not a new gameplay cap.

Use the EnvironmentState seed, next_tick, tunables and start-of-tick climate.
Mirror Go YearPhaseAt/DayArcTicks/EffectiveDayPhaseAt/TemperatureAt privately
for this provider with source anchor tests; do not change unrelated providers.
No user-supplied temperature or sky override enters the production API. Sample
helper `sample_cells(seed:i64,tick:u64,key:ChunkKey,section:u8,attempts:u8)
->Result<Vec<u16>,ServerError>` validates section<24 and attempts<=64. The
source sampler KAT (-42,777,dim1,chunk5/-7,section9,8) is
[3835,318,2309,2970,832,2023,3058,3225]. Samples are with replacement. A private
cell-settlement unit is independently testable in the provider; integration
tests also hit selected actual sampled positions through advance, not a second
test-only simulation. Keep the source 128-attempt crop characterization as a
Go-only oracle, never bypass Rust's64 bound.

Crop/dry/sapling/grass/snow algorithms and salts in plan04 remain exact. Tree
root max is311 (exclusive MaxY320 minus9); NativeTree is the only geometry
owner. Obtain all observations before one atomic full-tree transaction. Use
the read height cache for sky and inspect the latest overlay on each repeated
sample. Grass checks +X,-X,+Z,-Z with checked coordinate additions. Snow's new
layer requires top==ground Y, existing growth top<=layer Y, and melt ignores
roof. Test source climate anchors and a hold-band case (about1.13C), hot/cold
cap and roof behavior; use actual climate combinations rather than injecting
arbitrary temperature. Test zero/64/65 attempts, source KAT, deterministic
repeated run, all three crops/mature/dry/roof cases, tree atomic unavailable
neighbor/root311/312, grass first-neighbor ordering and unready, sorted/dedup
active sets, changed height seen by later samples, malformed batch and extreme
coordinate refusal. Real reducer phase order acceptance remains3.1.

Review focus: false air for missing chunks; stale sky after tree writes;
per-cell CAS confused with durable revision; sparse/compact disagreement;
coordinate and revision overflow. Each is assigned a named contract or
provider case above. Architecture skill: no change; these remain new local
seams until integrated behavior is accepted.

Review rulings: equal-value writes produce no overlay or changed receipt and
retain durable revision, including u64::MAX. A real A→B→A sequence still bumps
once. Sparse preload is fixture-only and asserts before inserting into a Ready
key; complete Ready fixtures must initialize the compact input. Checked system
writes remain the sole path to change those cells and their height cache. This
keeps sparse-only existing fixtures working without a second sky policy.

The existing mutation test module joins controller ownership only for two stale
revision fixture setups: they previously used air→air to force a revision.
Use real air→stone→air writes and expect the two per-cell CAS advances. This
retains the stale-observation assertion without requiring the corrected no-op
path to fabricate a mutation. Run the complete contract target after this edit.

The same stale-fixture correction applies to one case each in the existing
`tests/server_replay/world_mutation.rs` and `crops.rs`; these two test-only
setups join controller ownership. No provider behavior or assertion weakens.
