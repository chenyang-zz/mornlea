# Exact container views and panel drops

Node2.8b4b consumes checked foot outputs ebae6c57 and atomic world outputs
9c3ab40e; support state integration cea8ce2c must precede this node. Main owns
this design and integration. One isolated implementer owns the tightly coupled
container/view state trace; no other task may edit these files concurrently.

Editable S/src/rules/containers.rs; S/src/core/state.rs only viewer ownership;
S/src/core/container_store.rs only explicit patch dirty semantics;
S/src/rules/crafting.rs only pure close preview extraction; and
S/tests/server_replay/containers.rs, crafting.rs only related close assertions;
S/tests/server_contract/world_outputs.rs only the existing ordered-container
patch/equal-payload/MAX-revision test, revised to explicit durable-touch semantics.
S denotes packages/engine/crates/mornlea_server. All contracts, registries,
other providers, Go source, guides and planning/status files are read-only.
Do not widen wire ContainerRef beyond its accepted Overworld constraint.

## View ownership and checked entry

Add public containers::settle_command(ctx:&mut TickContext<'_>,
envelope:&CommandEnvelope)->Result<PhaseReport,RuleReject>. It settles one
OpenContainer/CloseContainer/MoveContainer/container MovePartial/QuickMove/
DropStack, without queue admission or events. Success report examined1/applied1,
carried0/rejected0. Unsupported family is Wire(InvalidInput). Invalid session
is InvalidInput; missing/inactive player or inventory for open/moves is
PlayerNotReady. Keep run's existing admission/drain shape for compatibility,
add container DropStack to admission, and have drain call the checked body in
queue order. Remove history/view_open/bind-on-first-move: an open itself owns
its exact lease. Production reducer calls the checked body directly at its Go
phase: close in PlayerCommand, open in Interaction, transfers/drops in
ContainerMove. This node does not accept that future scheduling.

TickContext owns a complete net viewer map seeded once by cloning authority.views
in from_parts before borrowing authority into Self. ReadView::viewer consults
only this map; remove committed fallback. Viewer(None) removes from the net map,
so a close cannot resurrect last tick's lease. Add
TickContext::viewer_leases(&self)->BTreeMap<SessionKey,ViewLease>, returning a
clone of this net set for AuthorityState::commit_viewers (existing full-set
replacement). Update comments: the reducer owns actual commit/retired-session
pruning and the eight admitted player ceiling. Preserve compound rollback of
this map. Do not add independent view queues or invent a second lifetime owner.

Open uses the existing authoritative ray, Active player, current environment
eye_height/interaction_reach, and current runtime.controls.actions().sneaking.
No hit or non-container hit -> NoTarget; unavailable observed/ray chunk ->
ChunkNotReady; numerical ray refusal -> InvalidRay; missing environment ->
InvalidInput; sneaking -> InvalidInput after classifying an eligible hit.
Require actual Ready hit chunk (sparse observations never authorize an open).
Resolve container_at(Overworld,hit.pos,kind), including actual nonzero slot and
generation; missing active record -> NoTarget. One Viewer(Some(actual ref))
replaces the prior lease. A failed open preserves the previous lease. A wrong
actor dimension refuses InvalidInput before authorizing any Overworld reference.
Workbench remains a separate provider; its anchor/open mutual exclusion is an
explicit follow-up, not an invented container record here.

Move basis requires current lease equals command reference, actual Ready
reference chunk and exact live record/generation. No first-move mutation.
Wrong/missing lease, retired slot/generation or missing Ready container is
InvalidInput. Preserve existing move/partial/quick slot, credit and repack
algorithms; their ordinary refusal maps InvalidInput. Source Go validates reach
in publishContainers AFTER transfers, not per transfer: do not introduce a
per-move reach veto. Future publication owns reach invalidation/end-once.

## Close preview

Extract crafting::closed_inventory(before:InventoryRecord)
->Option<InventoryRecord> as pub(crate), using existing add_stack. Personal
returns unchanged Some; Workbench previews only cells4..8, ascending, credits
fully or None, clears only these cells and sets Personal. Existing close_bench
reuses it and keeps its old no-change bool convention. Container Close uses the
same preview; one Compound contains changed InventoryPatch (if any) and
Viewer(None). A preview failure -> InvalidInput with inventory and lease intact.
Missing player/inventory still clears the valid session's view, as source close
succeeds without an attached player. Do not repack cells0..3 or require the
entire personal grid to fit. Source Go crafting.go237-277 and tick.go551-568.

## Container drop and durable touch

For container DropStack, validate move basis first. Read current source with
existing chest_slot/furnace_slot. Empty -> InvalidSlot; copy both inventory and
container, clear whole source via existing setters. Furnace input removal resets
progress; fuel/output removal preserves burn and unrelated progress, output38 is
legal as source. Keep durability. No repack veto on debit.

Call drops::prepare_player_drop(ctx,session,envelope.sequence(),source), then
stage one Compound[InventoryPatch,Container{before,after},Drops]. Preserve typed
RuleReject from preflight/stage, no output/source change on any refusal. A
container-view inventory-region source still needs the exact open lease and
live container. All source/foot/container copies stay private until successful
stage. Container phase follows DropStep in the later reducer, so a new output
keeps delay40/age0 until the next tick; include a direct ordered-provider test.

Source Go applyContainerMove touches the referenced chunk on EVERY successful
transfer/drop, including an inventory-only transfer with unchanged container
slots (container.go500/528/561/586, drop.go225, realm/state.goTouch). Therefore
an explicit validated ContainerState::patch is a durable write: set dirty=true
after full preimage/payload checks even if slots compare equal, for Chest and
Furnace. Ordinary container commit always includes its Container effect after
successful move calculation; do not omit an equal after record. A dirty MAX
revision refuses atomically through the existing check. Furnace provider already
stages only actual advancement, so no-op furnaces remain clean. The older
world_outputs test treating an explicit equal Container patch as clean is
superseded: revision8 becomes9; MAX returns StaleObservation with complete
snapshot unchanged. Preserve its ordered two-patch rehearsal assertions. No new effect,
wire/save version or broad dirty policy change. Compound copies guarantee that
failed later drops discard this touch as well.

## Concrete validation and acceptance

Write failing tests before behavior. Required independent cases:
- Opening actual Ready chest slot7/gen9 immediately binds7/9; then first move
  naming a different live slot refuses with both sources/lease unchanged.
- Actual furnace/chest reopen replaces old view; failed ray or sneaking open
  preserves it. Block-only sparse target and missing active slot cannot open.
- Seed committed lease, create context, close, then move with live source:
  no resurrection. Commit viewer_leases, recreate context: still absent.
  A committed unclosed lease works in a fresh context without another open.
- Close Workbench credits only4..8, leaves0..3, returns Personal and clears
  lease; failed extended repack preserves all. Personal with full pack and
  nonempty personal grid still closes. Missing actor close clears lease.
- Chest cells36 and62, inventory35 through container view, furnace input36,
  fuel37/output38 drop whole current stack; input resets progress only.
- Empty/stale ref/wrong view/foot unready/full32/MAX container revision refuse
  with complete chunk/inventory/view equality. Full32 same-cell merge succeeds;
  durable tool source exact. Two players share source: first clears, second
  sees InvalidSlot without minting. No repack veto with full inventory+grid.
- Active DropStep, then container drop -> new age0/delay40; next DropStep ->
  age1/delay39. No publication events at this provider layer.
- Inventory-only successful container move/drop dirties referenced chunk even
  if foot is a different chunk; two touched chunks each advance revision once.
  MAX referenced revision refuses inventory-only transfer/drop atomically.

Migrate existing container replay helpers to real Ready chunks with active
slots at the actual ray target; do not retain fixtures whose ref chunk(0,0)
disagrees with target(0,65,-1). Preserve old transfer coverage. Use a local
fixture installer that builds/updates compact Chunk.sections plus fixed arrays,
key=target x/z>>4, correct block_index and generation; all actual state tests
read the resulting exact reference. Changes to old expectations must cite the
removed history-first binding/close bug, not hide a new regression.

Run pinned Rust1.97.1 focused containers/crafting/drops/furnaces, full server,
clippy -D warnings, fmt, make rust then Go entity/runtime filters
'Container|Chest|Furnace|DropStack|CloseWorkbench'. List source tests first so
no-match is not evidence. Main strict OpenSpec and diff checks, independent
review, then scoped acceptance commit. Main owns integration/rollback of the
whole node and revalidation of its shared foot/crafting/container dependencies.

## Verified live command-phase reconciliation

Actual Go source opens and closes during PlayerCommand before Eating. Packet126 consumes existing checked settlement inline in the live reducer; lifecycle envelopes never enter the historical deferred container/bench bags. The raw deferred provider-call compatibility contract and its isolated report controls remain deliberately available, but are not evidence of source live command timing. Automatic lifecycle revalidation stays after Mining and transfers stay deferred. See [the live command packet](126-immediate-container-lifecycle-commands.md). No historical acceptance is erased or broadened.
