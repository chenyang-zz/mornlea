# Deterministic harvest sampler boundary

Controller prerequisite2.8b3a precedes the independently reviewed environmental
outputs and human mining outputs. Both consume the wheat-yield algorithm, so
land this compile-ready boundary first and record its tested SHA. Existing
mutation/drop contract9c3ab40e remains unchanged. This packet does not accept a
mining/fluid/trample consumer or alter the source gameplay distributions.

Main owns S/src/rules/{mod,harvest}.rs, tests/server_contract/{harvest.rs} and
its test entry, and plans/guide/ledger. Workers keep these files read-only.
Public pure functions in rules::harvest have exact signatures:

```rust
wheat(seed:i64,tick:u64,dimension:u32,pos:BlockPos)->(u8,u8);
potato(seed:i64,tick:u64,dimension:u32,pos:BlockPos)->u8;
carrot(seed:i64,tick:u64,dimension:u32,pos:BlockPos)->u8;
poison_potato(seed:i64,tick:u64,dimension:u32,pos:BlockPos)->bool;
short_grass(seed:i64,dimension:u32,pos:BlockPos)->bool;
leaf_sapling(seed:i64,dimension:u32,pos:BlockPos)->bool;
```

The scalar dimension is hash input, not a world-admission port; all u32 bit
patterns remain testable against existing Go vectors, and consumers derive it
from checked Dimension.get(). Signed coordinates first cast to u32 then widen
to u64; seed casts directly to u64. Functions are allocation-free, deterministic
and contain no mutable RNG. Position need not be a loaded cell; world/mutation
validation belongs to consumers. No arbitrary block/item registry is added.

Use Go updates/sampler.go SplitMix64 (wrapping addition/multiplication constants
9e3779b97f4a7c15, bf58476d1ce4e5b9,94d049bb133111eb). Start mix(seed^salt), for
crop families mix tick, then mix dimension, x,y,z in that order. Wheat salt
5eedfeedfaceface produces hash%3+1 wheat and mix(hash)%3+1 seeds. Potato salt
70a70a515eedface and carrot salt ca7707701ace5eed yield hash%4+1. Poison salt
deadbeefcafe1234 hits hash%50==0. Grass salt4752415353534544 and leaf salt
5341504c494e4753 omit the tick fold entirely and hit hash&7==0. A zero tick fold
is not equivalent to omitting tick. Keep this separate from random-block
sampling's section hashes; do not refactor unrelated accepted random providers.

Tests copy literal Go KAT rows from sampler_test.go:106-143 and
sampler_entity_test.go:17-55, including negative seed/coordinates, nonzero
uint32 dimensions, u64 tick above32bits and poison hit. First compile inert
functions and record behavioral RED. Run consumer examples which turn returned
wheat counts into two-item DropBatch values in wheat-then-seed order for both
Mining and System sources. These are callable contract doubles only. Probe
bounds/repeatability and seed/position stability; the typed no-tick APIs prevent
accidentally rerolling short grass/leaves by time.

Run pinned server_contract harvest, full crate tests, clippy/fmt and existing
Go `go test ./packages/server/updates -run 'Sampler(CropYieldRolls|ShortGrassSeedDropRoll|LeavesSaplingDropRoll)KAT' -count=1`.
Independent review verifies salts/folds/literals against frozen source. Main
records acceptance, guides and commit, then freezes exact consumer packets.
Rollback owns only these source/test declarations and invalidates both consumer
briefs; no provider work begins on a prospective signature.
