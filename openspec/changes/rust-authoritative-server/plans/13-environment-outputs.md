# Flood and footprint drop settlement

Node2.8b3b consumes accepted samplers f087d018 and atomic world outputs9c3ab40e.
Main froze this source reconciliation; one isolated worker owns both consumers
because their environmental crop policy and exceptional partial trample share
one small private helper. It does not consume prospective support-state edits.

Editable S/src/rules/{fluids,crops}.rs and tests/server_replay/{fluids,crops}.rs.
Read-only all shared contracts, harvest.rs, mutation/state, registries, guides,
plans and other providers. Main owns integration/review/rollback/status. Worker
is not alone. Main keeps its edits disjoint; no other worker owns these files.

## Exact behavior

Add a crate-private pure helper in crops.rs, used by both owned providers:
`environment_plant_outputs(seed:i64,tick:u64,dimension:Dimension,pos:BlockPos,
block:u16)->Option<Vec<ItemStack>>`. Return Some for wheat37..43→seed34x1,
wheat44→wheat35 then seeds34 with harvest::wheat counts, potato46..53→item40x1,
carrot54..61→item41x1, sapling89→item57x1. None for all other blocks, including
shortgrass84, farmland35/36, workbench45. This is environmental policy: mature
potato/carrot still yield one here; do not import human mining yields or poison.
All stacks durability0, length<=2. No public shared declaration or new registry.

Require a staged environment before `fluids::update` pops its schedule and
before `crops::settle_tramples` collects/drains pending work; absence returns
InvalidInput{field:"environment"} without changes. Update formerly sparse
fixtures to stage explicit environment; never use guessed seed/tunables.
Fluid yield tick is update's authoritative `now`; footprint yield tick is
ctx.read().tick(). The production reducer must pass the same authority tick to
both. DropSource System uses ctx.read().tick(), rule Fluid or Footprint and exact
integer target; delay comes from env.tunables.drop_pickup_delay_ticks().

Fluid evaluation keeps its accepted tick-start neighborhood snapshot, budgets,
strongest merge and sorted target order. Replace the incorrect whole merged
batch commit with per-target settlement, matching Go fluidWorld.SetBlock in
realm/environment.go:538-637. Skip no-op. A fluid replacement of a crop/sapling
prepares the entire output batch and commits that one write plus outputs via
try_system_with_drops. Full drop capacity leaves the plant unchanged and requeues
target plus six neighbors at now+delay with earliest-due dedup, exactly Go
enqueueFluidUpdate:174-180; do not add a changed cell or applied count. Other
refusals retain the existing fatal phase error for the reducer's overlay-abort
policy. Other successful target writes remain independently selected; no plant
capacity failure discards a successful different fluid target. Short grass
floods with zero drop even with32 full slots. Successful changes requeue as
before. applied counts actually changed cells; rejected stays0, carried keeps
its existing due-but-unstarted meaning (capacity retry is future scheduled).
Future farmland wakeups consume actual successful block deltas in the serial
reducer; this node does not invent another enqueue/membership owner.

Footprints replace count-only capacity estimation with exact check_drop_batch.
Keep `commit_trample(ctx,ground,crop)->TrampleCommit` callable for the existing
fault case. If crop is Some, derive batch and preflight it before any ground
write; missing environment/noncrop/failed capacity returns Refused. Bare ground
writes Dirt with no drop and returns Complete. With crop, commit ground→Dirt
first, then crop→Air plus prepared outputs in one try_system_with_drops. A
refused second write returns GroundOnly, preserving ground Dirt, original crop,
and no minted output; never roll back the ground or mint before crop removal.
The accepted exact merge preflight allows full32 physical slots with an eligible
partial same-cell stack. Existing duplicate landing order/progress and snow
footprint logic remain unchanged. All return/report semantics stay unchanged.

## RED and acceptance

First tests must fail behavior: flood immature/mature wheat/sapling produces
exact ordered items and delay; immature/mature potato/carrot each one; shortgrass
zero-output with full slots; wheat second-stack capacity fails whole plant and
queues retry, then succeeds after freeing capacity; two independent fluid targets
where one refused plant does not prevent other water write; full slots partial
same-cell legal merge. Pin source seed/tick wheat vector using f087d018, not a
new expected result computed by the provider. Run both dimensions and boundary
positions with real Ready drops and F1 snapshot/reload.

Trample tests: immature item and mature wheat tuple; body/two drops share one
chunk revision; full unmergeable capacity preserves ground and crop; full32 merge
succeeds; stale crop after capture gives GroundOnly/no drops; ground stale gives
Refused/no effect; bare farmland and duplicate landing remain zero/once as Go.
Missing environment refuses before schedule drain and all changes. Existing
source footprint suite must continue passing after explicit environment setup.

Run pinned focused server_replay fluids and crops, full server, clippy/fmt,
make rust and Go `go test ./packages/server/sim/realm -run
'Flood|FluidCrop|SaplingFlood|FluidWildGrass' -count=1`,
`go test ./packages/server/sim/entity -run 'Trample|SnowFootprint' -count=1`.
No zero-test oracle is acceptance; broaden exact source filter if names differ.
Worker reports behavioral RED/GREEN, tests/counts/commit and source conflicts.
Main reviews/integrates and commits one coherent node; real phase wiring stays3.1.
