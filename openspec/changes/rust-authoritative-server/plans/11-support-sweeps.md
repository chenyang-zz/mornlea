# Ordered support removal

Node2.4c consumes accepted atomic output contract9c3ab40e (main now also includes
pickup cb8bb604 and disk647f5599). The independent source audit verified Go
realm/environment.go:1306-1705, realm/mutation.go:45-96 and runtime/engine_step.go.
Main owns these decisions and integration; an isolated worker implements this
one provider and its private changed-position input, not subsequent producers.
Tasks.md remains the only status source.

## Ownership and interface

Editable S=packages/engine/crates/mornlea_server: src/rules/supports.rs,
tests/server_replay/supports.rs, and narrowly src/core/state.rs for the changed
position port specified below. No shared declarations/registries/manifests,
other rules, guides, plans or tests. All accepted APIs, Go source and F1 types
are read-only. Main reserves all other edits and does not modify state.rs while
this isolated packet is active. Worker is not alone and preserves others' work.

Implement `supports::run(ctx:&mut TickContext<'_>,call:RuleCall<'_>)
->Result<PhaseReport,ServerError>`. Only Support with no actor/command/internal
is valid; other shapes return InvalidInput{field:"support"} before effects.
Snapshot environment once; missing environment refuses before mutation. Require
its current tick for system provenance. Emit no wire events; publication is the
reducer's later responsibility.

Add `TickContext::changed_blocks(&self)->Vec<BlockObservation>`. A private
BTreeMap<(ChunkKey,u32),BlockObservation> tracks only successful changed writes
in this context, ordered by chunk key then compact block index. No sparse
preload or Ready base cell enters this map. No-op writes add nothing. A later
write to the same cell replaces its value, even if restored to the initial
value: the fact of mutation remains. Initialize empty for fixtures, and clear
that key's entries if preload_ready_chunk replaces its generation. Defensive
Compound rollback restores this map alongside the existing blocks. Populate
only in apply_writes after validation; a refused transaction adds no entry.
Return a cloned entry snapshot, not an iterator that observes newly inserted
changes. This is one provider-owned input, not a new shared boundary dispatched
to multiple independently accepted consumers.

Do not invent a4096 global changed-cell truncation from the existing per-effect
component cap. The changed map is bounded by the finite admitted write set and
98,304 possible cells per owned chunk; support runs O(changed cells), never
scans all loaded blocks or enqueues recursively. The actual loaded-interest and
per-phase budgets remain the serial reducer's acceptance responsibility. Each
pass makes its own entry snapshot, so later passes see earlier pass outputs
while a pass cannot recursively revisit its own new changes.

## Exact algorithm

Run short grass, sapling, torch, bed, in that order. Count examined as the sum
of the four entry snapshot lengths; applied counts successful changed cells
(bed can add two). Silent skip leaves rejected unchanged; an attempted atomic
transaction refusal increments rejected once. Carried=0. No gameplay rejection
event is synthesized. Read current staged observations for every decision.
Coordinates follow source int32 wrapping in X/Z neighbor arithmetic; guard Y
against world [-64,320) rather than reading an invalid block index.

- Short grass: for each changed support cell inspect exactly above. If Ready
  ShortGrass84 and current support Ready and not Grass4, write Air with
  try_system(Support). Never reserve/mint drops, including full32 slots.
- Sapling89: exactly above; keep if support unreadable or Dirt3/Grass4. Otherwise
  one ItemSapling57 with delay tunables.drop_pickup_delay, origin canonical cell
  center, source System{Support,current tick,target:plant}. Use one
  try_system_with_drops for Air plus batch; refusal keeps both unchanged.
- Torch71..75: for each entry inspect neighbors in +X,-X,+Y,-Y,+Z,-Z order.
  Only a torch whose oriented support equals this changed cell is relevant.
  Standing supports below; wall+X supports-X, wall-X supports+X, wall+Z supports-Z,
  wall-Z supports+Z. Support is registered and excludes Air, fluids27..34,
  plants(crops37..44/46..61,shortgrass84,sapling89), torches71..75 and upperdoor70.
  Unreadable support is not solid. Otherwise glass/leaves/beds/lower doors count.
  Unsupported torch clears plus one ItemTorch44 atomically at the torch cell.
- Bed76..83: inspect above only. Keep if current support unreadable or is
  farmland35/36 or registered nonair excluding glass20/leaves19/fluids/plants/
  all doors62..70. Decode foot/head orientation exactly as Go bedHalfPositions.
  Read both target cells; unavailable counterpart refuses whole. Do not invent
  a requirement that the other loaded cell already has a matching bed form:
  Go clearBedPair clears both coordinates after choosing them from the struck
  form. One ItemBed46 drops at the struck half, two Air writes in one atomic
  system transaction. Both preimages are current; capacity/revision failure
  leaves all cells and drops unchanged. A previous removal makes later duplicate
  candidates observe Air and produce no second item.

Use existing F1 registered_block and frozen block constants; do not implement a
new registry or broaden gameplay. Keep compact slots and integer target identity
from9c3ab40e. The one-stack batch is equivalent to source PrepareDrop/CommitDrop
for saplings and torches, including lowest-slot merge, generation and delay.

## RED cases and validation

First make the exact signatures compile with an empty run result, then run
behavioral failures for grass/sapling/torch/bed removal through actual ctx writes.
Tests pin: preload-only cells never swept; changed entries deduped/sorted by
chunk and compact index; multiple writes final support restored preserves all
four types; same pass does not recurse, later torch pass sees removed grass;
all five torch orientations and solid-policy difference from bed; full32 drops
still clear short grass, preserve sapling/torch/bed, and eligible partial stack
allows a merge; sapling delay/body; cross-chunk bed unavailable/MAX refusal and
successful pair at boundary emits exactly one item; missing support policies;
both dimensions; no-op and failed/rolled-back mutation add no changed entry;
per-pass work proportional to changed snapshot, empty/wrong-call no mutation.
Use real Ready chunks for slot capacity and F1 snapshot encode/reload. Expected
block numbers/order come from Go tests, never provider-generated expected hashes.

Run pinned `rustup run 1.97.1 cargo test --manifest-path packages/engine/Cargo.toml
-p mornlea_server --test server_replay supports --locked`, full crate tests,
clippy all-targets -D warnings and fmt check; make rust before focused Go oracle
`go test ./packages/server/sim/realm -run 'Support|Sapling|Torch|Bed' -count=1` and
`go test ./packages/server/sim/runtime -run 'Support|Sapling|Torch|Bed' -count=1`.
Report RED/GREEN counts, source rulings, exact paths and commit. Main performs
independent review, integration, guide/status/ledger updates and scoped commit.
Rollback is this node's explicit files with dependent evidence invalidated, never
resetting another worker. Actual phase invocation/publication stays3.1.
