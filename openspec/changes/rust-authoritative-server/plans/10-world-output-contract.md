# Atomic world outputs and fixed container ownership

Controller prerequisite 2.8b2 completes the already declared BlockTxn output
lanes before support/loot producers consume them. It follows drop contract
`0ea40207`. This is a serial controller landing; provider workers must name its
accepted commit, not this prospective packet. Tasks.md is the status source.

## Source rulings and ownership

Go container simulation can allocate and mine in both dimensions, while the
existing v45 container wire validators admit only dimension0. Do not widen that
wire contract or alter F1 domain ContainerRef. Internal ownership is keyed by
core ChunkKey, and its dimension must never be inferred from the wire reference.
An internal `(ChunkKey, ContainerRecord)` identifies a captured container. Wire
view operations continue to use dimension0. The later reducer must retain
dimension when enumerating internal furnace work; it cannot send a depths
container reference over the unchanged wire surface.

Main owns core/{contracts,state,world,mutation,mod}.rs, new
core/container_store.rs, tests/server_contract/{world_outputs,mutation}.rs and
test entry, tests/server_replay/containers.rs for the exhausted-revision atomicity
regression, narrowly affected mining fixtures, and rules/containers.rs only for
making its existing inventory/container settlement a single Compound. The
current separate inventory-then-container calls would lose inventory if the
new revision guard rejects the second operation. Other providers are read-only;
in particular the isolated drop-lifecycle worker owns drops.rs and the two
crafting helper visibility changes. Guides/plans/ledger are main-owned. No
hashed source, generated artifact, schema or protocol bytes change.

## Exact interface completion

Add `CapturedContainer { key: ChunkKey, record: ContainerRecord }` and use it for
BlockTxn's private containers vector. Add
`RuleEffect::WorldContainer { dimension: Dimension, before: ContainerRecord,
after: ContainerRecord }`; existing Container means the same operation in
dimension0. Read ports:

```rust
world_container(dimension: Dimension, reference: ContainerRef)
    -> Option<ContainerRecord>;
container_at(dimension: Dimension, pos: BlockPos, kind: ContainerKind)
    -> Option<ContainerRecord>;
container_refs(key: ChunkKey) -> Vec<ContainerRef>; // at most 48, slot ordered
```

Existing container(ref) delegates to world_container(OVERWORLD,ref). Add
`MutationTxn::try_system_with_drops(producer: SystemRule,
writes: Vec<BlockWrite>, drops: DropBatch) -> Result<MutationOutcome,RuleReject>`.
It requires the System source producer/current tick and uses the same complete
commit path. Existing try_system and resolved actor methods keep their types.

## Fixed container state and snapshots

A private ContainerState owns each Ready chunk's 32 FurnaceSlots and16
ChestSlots, the base revision and tick dirty flag. Construction copies already
validated arrays off tick. Exact reference lookup checks kind/slot/generation
and active state, producing ContainerRecord with the unchanged base revision.
Position lookup scans the bounded fixed array's block_index, never assumes
slot0/generation1. Enumeration returns active refs only, furnace then chest slot
order. Internal callers always pair refs with the ChunkKey dimension.

Existing preload_container is an explicitly sparse fixture store. Read methods
use it only when there is no Ready owner for that chunk; no fallback after a
Ready lookup miss. Its old slot0/generation1 position convention is retained
only for those sparse fixtures. A flat fixture record accompanying a Ready
chunk must equal the actual slot, otherwise fail setup; it cannot resurrect a
removed generation. Production never populates sparse fixture records.

Container patches compare entire preimages and preserve reference and base
revision. Convert after.slots through existing F1 chest/furnace validators:
registered stacks, input/output families, coal-only fuel, progress<200 and
burn<=1600; check narrowing before u32→u8/u16. Refuse exhausted chunk revision
before any real slot change; an identical patch is a no-op. All durable slot
changes, including furnace counters, mark the same tick dirty bit as blocks and
drops. Save/replay materializes both fixed arrays and advances the chunk once
when any of these owners changed; inactive generations remain unchanged.

## Transaction algorithm

Stage BlockTxn as one effect, including inventory preimage, captured containers
and drop preflight. The previous MutationTxn commit must no longer apply writes
directly and discard its output fields. Validate every captured record against
its keyed current owner. For Ready chunks rehearse only affected fixed arrays:
each changed container block removed/replaced must have the matching capture;
remove it by clearing all fields except generation. A new chest/furnace block
reserves the lowest inactive slot whose generation is not u32::MAX, rejects
an existing active slot at the same index, and increments generation with empty
contents/timers. If no slot exists return wire ContainerCapacity, without
writing blocks or debiting inventory. Go PrepareChest/Furnace is the oracle.
Sparse fixtures can remove captured records through their fixture map; they do
not pretend to establish real fixed-array ownership.

All container and drop rehearsals are bounded copies keyed only by affected
chunks. Ordered compound effects operate on those copies, including two patches
of the same container. Publish them only when the other effect arms succeed.
Then actual block writes/inventory debit, cleared fixture captures and prepared
slot outputs appear atomically. Failure leaves all records, generations, blocks,
inventory, tool and dirty bits unchanged. No all-world clone is introduced for
these new owners. MutationOutcome keeps its existing changed observations and
inventory flag; drops_created counts nonempty input stacks, as its existing
contract test does, not allocated slots after merging.

Human/companion mining capture uses container_at in the actor's dimension.
Human mining replaces the occupancy+input-count approximation with the accepted
check_drop_batch; commit repeats this after staleness checks. A mined container
removes its record and transfers its entire contents once. Companion credit
keeps the accepted inventory algorithm but removes the same captured container.

Source block-origin outputs retain exact integer coordinates. Extend the newly
introduced DropSource::System with target:BlockPos. For Mining/System sources,
batch location uses that exact target and checks origin equals the source's
float32 cell-center observation. Panel/Death sources continue flooring their
authoritative actor position. This prevents large-coordinate float rounding
from moving mined/system loot to a neighboring slot or chunk. Reject invalid Y;
the int32 target type is already range bounded. Retain source int32 wrapping
only when decoding existing stored chunk indices, as in plan08.

## RED acceptance and integration

Create real Ready chunks with chest slot5/gen7 and furnace slot3/gen9, verify
position/ref lookup, both dimensions without aliasing, and sparse record cannot
override a Ready miss. Patches preserve one dirty revision and F1 encode/decode
counter/content state; invalid payload/ref/revision and stale second compound
patch preserve every lane. Loaded inactive generations survive unchanged.

Place chest/furnace through the actual authority resolver on Ready chunks:
lowest reusable slot, exhausted generations skipped, capacity refusal preserves
block/inventory, removal/rebirth changes generation. Mine a loaded nonzero-slot
chest/furnace: items and body reach real drops, record disappears, inactive
generation persists and the F1 saved chunk validates. Change captured contents
after resolution and prove commit refuses without spending tool or dropping;
exhaust drop capacity after resolution and prove the same. Full32 physical drop
slots with an eligible partial same-cell stack still accept a merge. Companion
mining transfers once to inventory and removes captured slot. Preserve the
preflight order (inventory/tool before drop capacity). Rewrite the old success
test which incorrectly expected a mined container to remain intact.

System writes plus drops use one atomic method: stale/capacity refusal leaves
all cells/slots unchanged, and large integer target keeps exact block_index
through F1 snapshot despite rounded center. Include ordered Compound block and
container/drop interactions, plus full crate regression on former sparse tests.

Run focused world_outputs/mutation then full server, clippy/fmt, exact Go shared
world Chest/Furnace/Drop and entity mining/placement tests, strict OpenSpec and
diff checks. Main obtains independent review before accepting the contract SHA.
Container view distance/open binding, dimensioned furnace scheduling, every
rule producer and actual endpoint integration remain their explicit later
serial owners; this landing must not mark them accepted.
