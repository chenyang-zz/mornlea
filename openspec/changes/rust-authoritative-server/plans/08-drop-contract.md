# Bounded drop ownership and producer handoff

The controller adds prerequisite 2.8b0 before lifecycle/producer nodes. Baseline
`e1816aa4`; all providers consume its accepted SHA only after behavioral gates.
This supplies the existing Drops/DropPatch effects with real state ownership.

## Contract owner and semantics

Main owns core/contracts.rs, state.rs, world.rs, new core/drop_store.rs, core
module export, tests/server_contract/drop_store.rs and test entry; crate guide,
design, tasks and ledger. Existing rule providers are read-only in this landing.
No hashed/generated consumer changes. New module inherits crate guidance.

DropSource gains `System { rule: SystemRule, tick: u64 }`. DropBatch admission
uses the Go max(28,36)=36 input-stack limit, independently of 32 physical slots.
Its public fields remain revalidated at staging. Empty stacks are skipped;
invalid stacks or out-of-world/int32 origin refuse without mutation. Floor the
origin, resolve its chunk and local block index, store the cell center. A
per-chunk private DropState retains all 32 storage slots (including inactive
generations) plus a slot-ordered active DropRecord view and a tick dirty bit.

ReadyChunk installation imports validated fixed slots. Sparse fixture setup
creates empty fixed slots for its observed chunk; this remains a fixture-only
compatibility seam, never runtime readiness. Fixture DropRecords seed slots;
snapshot replay restores both fixed slots and top-level active records without
duplicating an identical full record. A nonzero inactive generation cannot be
reseeded by a stale flat fixture. Full-range stored chunk coordinates reconstruct
centers with Go int32 wrapping then float32 addition, and repeated flat records
never recover an already loaded block index from a rounded center. Missing chunks refuse new drops with
StaleObservation. `read().drops(key)` remains the immutable ordered active slice.
Add `read().check_drop_batch(&DropBatch) -> Result<(), RuleReject>` for producers
to reserve whole-output feasibility under the same exclusive TickContext. The
eventual mutation commit must repeat validation, not rely on an old preflight.

Replay each input stack on a 32-slot copy. Choose lowest active slot with same
item AND same block index below item limit; otherwise lowest inactive slot whose
generation is not u32::MAX. Merge retains generation/age and extends delay with
max; birth increments generation and starts age0. On exhaustion return wire
DropCapacity and publish no slot. DropPatch compares the entire before record,
requires the same ID/position/item/durability and nonzero valid remaining count
no greater than before; it may change age/delay. None clears all fields except
generation. Count reduction/removal and actual birth/merge mark dirty. Changes
to age/delay alone do not mark dirty, matching the source's lifetime behavior.
An unchanged patch is a no-op. Mutating a chunk at u64::MAX refuses before any
publication; counter-only updates still work because they do not advance it.

Compound validation uses a scratch map containing only affected chunk slot
copies, replaying drop effects in order. Apply these copies only after all other
effect components succeed; refusal leaves slots, generation and dirty state
unchanged. No all-world clone is introduced for drop staging. Save/replay
snapshot materializes fixed slots, and advances the chunk revision once when
either blocks or durable drop fields changed (never once per slot or producer).
It also returns the active flat record view for replay observation. Fixture
restoration of an unchanged chunk preserves inactive generations.

## Source compatibility ruling

Go counter-only aging does not Touch the chunk: clean Ready chunks can unload
or restart with their last durable counters. Preserve that behavior during this
migration; do not claim a new persistence guarantee or write changed counters
under an equal durable revision. An already dirty snapshot includes current
counters. The lifecycle inactive/restart fixture saves after drop birth while
dirty; this is distinct from a previously durable counter-only update. Any
later durability improvement requires its own approved behavior change.

Mining/container removal, trample's two-write exception, fluid/support loot,
death batches, inventory pickup and reducer scheduling are later consumers and
remain unaccepted here. Existing no-op mining success tests are not proof of
drop integration; their assertions will change with the producer landing.

## Behavioral RED and acceptance

First exercise: 36 inputs fit through merging while 37 refuses; lowest slot and
center normalization; merge age/ID/max-delay; invalid/full/refused compound
preserves exact fixed slots and generations; clear/rebirth increments generation;
exhausted inactive generation skips; stale/identity-changing/increasing patches
refuse; compound remove-then-birth order; Ready load/replay preserves inactive
slots and active identity; block+drop edits advance one chunk revision; age-only
keeps revision; exhausted revision refuses births but permits aging; unknown
chunk and invalid origin refuse. Use real TickContext/Ready compact values and
source Go drop tests, not only a stand-in allocator.

Run focused contract test then full server crate, clippy -D warnings/fmt,
Go packages/shared/world Drop oracle (make rust already satisfied on accepted
baseline), strict OpenSpec and diff checks. Independent reviewer examines
ownership, revision and compound refusal before main commits accepted SHA.
Controller integrates/rolls back and owns subsequent producer task refinement.
