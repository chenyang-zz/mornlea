# Crafting partial and quick move implementation plan

Goal: settle the existing protocol's Crafting-view partial and quick commands through the actual command reducer, preserving Go state, atomic refusal and owner publication.

Scope authority: the existing supported-outcome parity requirement, not a new feature or protocol. Baseline is `9bce77afa5843da2915accf07b00882af4b936e8`. The controller directly authors this serial node; native agents only collect facts or independently review. No Loom, Claude invocation, clone/worktree, push, deployment or default-startup changes. Task status lives only in `tasks.md`.

## Ownership and prerequisites

Editable source: `packages/engine/crates/mornlea_server/src/rules/crafting.rs`; append tests to `tests/server_replay/crafting.rs` and `tests/server_replay/publication_projection.rs`; add a concise ownership note to the crate `AGENTS.md`. Controller also owns this packet and the change's design/tasks/ledger. Read-only: domain command types, inventory/container/drop providers, `core/state.rs`, `core/step.rs`, publication implementation, all Go files, manifests and prior tests. Accepted existing types are `PartialMove`, `StackSource`, `StackView::Crafting`, `InventoryRecord`, `InventoryPatch` and `TickContext::record_crafting_command_publication_dirty(SessionKey)`. No new shared boundary or contract landing is needed.

Source evidence: Go `entity/tick.go:358–394,445–478`, `crafting.go:49–71,110–159`, `quick_move.go:125–172`; Rust currently accepts neither command in `crafting::admit`. Inventory and container providers explicitly own only their respective views. Existing core dispatch already calls crafting during PlayerCommand.

## Frozen implementation

Partial move: accept only Crafting view; reject pack-to-pack indices and either grid index outside the current `size*size`. Domain already checks static bounds and distinct endpoints. Read the current source stack on the local record; refuse empty. Derive `single ? 1 : (count+1)/2`; use existing `move_view_stack`, whose local rehearsal rejects unlike/full targets or failed complete crafting repack. Stage one whole-record patch then mark both owner publication lanes after success. Refusal stages and marks nothing.

Quick move: accept only Crafting view and reject inactive grid indices before indexing. Refuse empty source. For grid-to-pack, reuse existing `add_stack` (hotbar merge, hotbar empty, backpack merge, backpack empty), refuse zero absorption and keep remainder at source. For pack-to-grid, visit effective cells in ascending index order, stop at the first empty or same-item nonfull cell, absorb at most that one cell's capacity, retain source remainder and target durability. Empty destination inherits source durability. Rehearse complete repack on the copied record; failed rehearsal refuses rather than trying a later cell. Stage once and mark both owner lanes after success. Fixed scans are at most nine grid cells plus existing 36-slot insertion/repack, no I/O, workers, heap queues or new ownership.

Keep source command gate ownership unchanged: this node does not introduce typed rejection receipts, repair all other dirty writers or accept a runtime. Existing hard-tick failure policy applies. Do not put task identifiers in new source/test comments. The existing directory guide is inherited; no new directory is created.

## Test-first sequence and oracles

Append provider cases using the existing real `TickContext::harness`, `scene`, `admit`, `inventory_of` helpers; label these prepared rule cases, not executable evidence. Expected records are literal copies with hand-derived deltas:

- Count-five half pack-to-grid moves three, retaining two; single moves one; settle a second command against the updated stack.
- Same-item target at 63 accepts one and retains remainder; unlike/full targets refuse exactly.
- Pack-to-pack and personal inactive grid ends refuse unchanged.
- Quick pack-to-grid chooses the first fitting cell even if a later empty cell could absorb more; partial capacity leaves source remainder. Bench extent permits cell eight.
- Quick grid-to-pack honors hotbar-empty before backpack-merge; zero absorption refuses.
- Both quick directions and partial reject a copied state whose remaining grid cannot fully repack, preserving every field.

Append real reducer/publication cases using existing authority, world, login and submit helpers: half then quick return to the original full record in one tick; quick in/out round trip. Assert complete final owner InventoryState/CraftingState exactly once despite an unchanged record, foreign owner zero and next quiet tick zero. Each missing route must fail against baseline for refused provider or absent publication, not construction/compile/setup failure. Existing refused provider controls may already pass.

After recording genuine RED, author only the minimal provider helper/match arms. Run new cases, all crafting/provider cases, inventory/container/drop regressions and full server replay. At the commit boundary run Rust fmt and Clippy plus full Rust workspace tests, source-bound Go crafting/partial/quick tests using the existing native release build and Python/previous-package environment, and strict OpenSpec validation. Independent review checks exact source, source Go predicates, unchanged historical prefixes and evidence identity. Commit this coherent node before starting the next implementation node.

Derived consumers: ordinary Cargo replay registration and dependent crate compilation; `capability-inventory.json` and full-corpus tests pin Go byte ranges, so Rust-only edits do not refresh those sealed Go hashes. Run the full-corpus gate anyway. No contract/save/ABI/version impact. Controller owns integration and any correction commit; rollback is reverting this isolated local commit through ordinary tooling only if requested. Broad activation, actor cache/bootstrap, background publisher, configuration/runtime integration and full-stage acceptance remain open. Architecture skill: no change.
