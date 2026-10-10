# Drop lifetime and pickup provider

This is node 2.8b1, an independently testable prerequisite of full producer and
command acceptance in 2.8b. Accepted slot contract: `0ea40207`; current integrated
baseline `3dbf99aa`. It does not depend on hostile death: death produces the same
accepted DropBatch, while this node consumes stored slots. This refines the old
2.7c-before-2.8b dependency without accepting death or producer integration.

## Ownership and exact surface

Worker exclusively edits `S/src/rules/drops.rs`, `S/tests/server_replay/drops.rs`,
and changes only the existing `add_stack` and `can_repack` functions in
`S/src/rules/crafting.rs` from private to `pub(crate)`. Their accepted algorithms
and signatures stay byte-for-byte otherwise. S is
`packages/engine/crates/mornlea_server`. Core contracts, other rules and test
entries are read-only. No new shared abstraction, generated artifact or hashed
source consumer. The main owns integration, rollback, guides and plan status.

```rust
pub fn run(ctx: &mut TickContext<'_>, call: RuleCall<'_>)
    -> Result<PhaseReport, ServerError>;
pub fn advance(ctx: &mut TickContext<'_>, active: &[ChunkKey])
    -> Result<PhaseReport, ServerError>;
```

run accepts only DropStep with actor/command/internal all None and returns a
zero report, matching other caller-owned batch providers. Wrong shapes refuse
without mutation. The real reducer supplies active-interest keys to advance;
commands and loot creation remain outside this node. Empty active input returns
zero without requiring environment. Nonempty requires the current environment.

## Fixed algorithm and bounds

Input has <=200 keys (8 active players times 25 interest columns); reject larger
input before allocation or mutation with Capacity/ChunkResults limit200. Sort
and deduplicate (dimension,x,z). Skip unknown/not-Ready keys. Collect only Active
player actors, sort SessionKey, enforce <=8 before mutation; other actor types
never pick up. The reducer owns deriving radius2 interest; this provider never
infers Ready from a sparse fixture and never advances inactive chunks.

For each Ready key, copy the <=32 active records in physical slot order. Each
record: decrement nonzero delay, increment age with u32 wrapping (Go uint32++),
then if age>=tunables.drop_lifetime_ticks remove before any pickup. Otherwise
stage the counter patch, and if delay remains nonzero continue. Counter-only
changes do not dirty/bump revision under accepted slot contract. An age of
u32::MAX wraps0 before comparing, preserving source behavior rather than a new
saturation rule. Use the environment snapshot's lifetime/range throughout.

For each eligible remaining drop, visit sorted active sessions. Same dimension
and f32 Euclidean center-to-player distance <= tunable drop_pickup_range are
required (sqrt of the three squared f32 differences, matching mgl32 Len).
Use the latest inventory from read().inventory and existing crafting::add_stack:
hotbar merge, hotbar empty, backpack merge, backpack empty in ascending index.
If taken0, continue. Rehearse existing crafting::can_repack on the new slots
with all nine current grid cells; failure vetoes this player's entire proposed
pickup, including a partial proposal. On acceptance stage one Compound of
InventoryPatch and DropPatch (None if exhausted, otherwise reduced count).
It atomically credits and debits; a stale/refused effect returns error, not a
silent partial success. Remaining quantity is offered to the next session using
the latest staged inventory. Never alter armor, selection, crafting or source
durability. No special pickup event: final reducer inventory/drop publication
owns observations. No producer can mint items through this provider.

Report examined is number of initially active slots visited in Ready active
chunks, applied counts slots changed by expiry or any accepted pickup once,
carried/rejected0 on successful phase. Counter-only slots are examined but not
applied. Contract/staging failure maps to InvalidInput field "drop_step"; source
gameplay vetoes (range/full/repack) are normal no-ops. No save or I/O here.

## RED cases, gates and handoff

First failing test: stored delay40 after39 active advances is1 and unpicked;
40th is0 and eligible. Assert inventory/drop conservation. Add expiry5999→6000
before pickup; source u32 wrap; no active key/no Ready chunk freezes counters;
same seed/state key-order permutations/duplicates yield identical snapshot;
session order competition with two players; distance exactly1.25 vs above,
wrong dimension/inactive actor; four-phase insertion and durability preservation;
partial remainder accepted by next player; full pack unchanged; crafting repack
veto even for partial pickup; exhausted chunk revision refuses removal with no
inventory credit; input201/player9 refuse before counters. Source defaults6000
and1.25 are tunables, so also use a nondefault lifetime/range case.

inactive_delay_and_restart: birth a delay40 drop through real staging, advance
10 active→30/10, advance60 with no interest→unchanged, encode/decode its dirty
chunk with F1 and reconstruct Ready/TickContext, then one active→29/11. Retain
inactive generations. A separate test for a previously durable clean chunk
asserts counter updates do not increment its revision; do not claim they are
automatically saved. Plan08 documents this source compatibility limitation.

Run focused drops tests, full server tests, clippy all-targets -D warnings, fmt,
make rust on clean baseline before Go, then
`go test ./packages/server/sim/entity -run 'Drop|Pickup' -count=1` and strict
change validation. No foreground game. Record behavioral RED/GREEN and exact
commit; report policy conflicts before inventing behavior. Main reviews and
accepts provider separately from real reducer/command/death/mining integration.
