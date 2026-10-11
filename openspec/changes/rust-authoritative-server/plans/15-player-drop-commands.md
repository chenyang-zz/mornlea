# Selected and inventory/crafting panel drops

Controller node2.8b4a consumes atomic outputs9c3ab40e and lifetime cb8bb604.
Container panel settlement is2.8b4b, after exact view binding is completed; it
will consume the checked foot-output function landed here. Main owns this
small coupled edit while isolated workers own support/environment/mining.

Editable S/src/rules/drops.rs, crafting.rs only three helper visibility changes
(grid_extent/view_slot/set_view_slot to pub(crate)), tests/server_replay/drops.rs,
guides/plans/status/ledger. No state/contracts/registries or other providers.

Keep drops::advance unchanged. Extend run with Interaction carrying exactly one
command and no actor/internal; it calls new public
`settle_command(ctx:&mut TickContext<'_>,envelope:&CommandEnvelope)
->Result<PhaseReport,RuleReject>`. This checked body retains wire rejection
reasons for the later reducer. run maps a refusal to the established collapsed
ServerError::InvalidInput{field:"drop_command"}; it does not publish events.
DropStep empty-shape behavior remains. Admission/defer order belongs to3.1:
Q/inventory/crafting settle among ordered interactions before DropStep; container
panel routes to later ContainerMove. Do not add a second queue or drain other
interaction families here. Wrong shape returns InvalidInput before effects.

Add `pub(crate) fn prepare_player_drop(ctx:&TickContext<'_>,session:SessionKey,
sequence:u64,stack:ItemStack)->Result<DropBatch,RuleReject>` for later container
consumer. It reads exactly ActorKey::Player(session), requires Active, derives
floor of authoritative f32 feet using f64 bounds before i32 conversion, rejects
out-of-i32 or Y outside[-64,320) as ChunkNotReady, and requires Ready foot chunk.
It reads environment delay player_drop_pickup_delay_ticks (absence invalid input),
creates Panel{session,sequence} from the same envelope owner, retains the actual
finite foot pose as origin (drop store owns flooring), dimension from actor,
one authoritative stack, and exact check_drop_batch. No source mutation or
invented client count/position enters this helper. Missing/inactive player is
PlayerNotReady. Ready absence precedes capacity. Staging repeats capacity.

settle_command accepts only DropSelectedItem or DropStack with Inventory/Crafting
view. Validate SessionKey, Active actor and inventory before source access.
Q selects the current authoritative hotbar slot, drops exactly1, decrements or
clears canonically, preserving durability. Inventory panel indices0..35 drop the
entire current stack. Crafting panel0..8 address grid,9..44 inventory offset9;
personal permits only grid indices<4 and workbench<9, using current crafting_size
(not arrival-time size). Empty/invalid effective slot is InvalidSlot; unsupported
command/container view is InvalidInput. Derive both before/after copies without
mutation; call prepare_player_drop then one Compound[InventoryPatch,Drops].
No crafting repack veto on debit (source Go removes capacity, it does not credit).
No dirty/no-op side effects before successful stage. Nonempty registered source
stacks keep exact item/count/durability. Report examined1/applied1, carried0,
rejected0 on success; preserve precise RuleReject on failure.

RED cases drive the callable body and run: Q1/remainder and final item; Q durable
stack; Inventory35 whole; Crafting personal last valid3 and first invalid4,
workbench8, inventory offset44; empty/inactive/missing Ready/full capacity/MAX
revision preserve complete snapshot; negative foot coordinates use floor and
actual dimension;32 physical slots with eligible same-cell merge succeeds;
no repack veto; two ordered operations read latest selected/inventory, e.g.
place/use before Q empties slot and Q refuses without minting; replaying a
cleared panel source refuses. Advance DropStep once after successful Q gives
age1/delay39 (container later retains40 until next tick, tested by its node).
Any test fixture must use actual Ready chunks for feet. Validated source order
is Go entity/drop.go:308-440 and tick.go ordered interactions.

Run pinned focused drops, full server, clippy/fmt, exact Go entity DropCommand/
DropStack/Pickup and runtime Drop tests, strict OpenSpec/diff. Independent review
then scoped commit freezes helper SHA before container consumer implementation.
No default runtime, protocol/schema or public item rule changes. Main owns
integration/rollback; revert this node only and invalidate its consumer evidence.
