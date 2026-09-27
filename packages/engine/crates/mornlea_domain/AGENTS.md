# Domain contracts

`packages/engine/crates/mornlea_domain` owns shared identifiers, value rules,
and semantic input/event records for the Rust runtime foundation. It is a
windowless rlib. Production code must not depend on protocol codecs, save
codecs, `mornlea_engine`, `mornlea_client`, or `mornlea_godot`. Direction is
enforced by `tests/runtime_contract.rs` (`production_manifest_has_no_codec_kernel_or_host_dependencies`).

## Inventory freeze (`src/lib.rs`, `tests/runtime_contract.rs`)

- Eventual owner of the `domain.input` and `domain.event` inventory rows.
- Registration tests fail if the frozen corpus drops those rows or assigns
  this crate a production dependency on a codec, kernel, or graphical host.
- Family semantics are ported behind `runtime_contract` cases; this crate
  must not infer parity from an unimplemented subset.

## Identity completeness (`src/identity.rs`, `tests/runtime_contract.rs`)

- `Identities::current` is the Rust pin for supported protocol, save, ABI,
  and agent versions. `current_identities_match_frozen_inventory` compares it
  with `testdata/runtime-migration/contracts.json`; do not keep a second
  matrix in this guide.
- `Identities::validate` and `ReplayIdentity::new` reject a zero version,
  empty agent label, empty source revision, or empty corpus digest as
  `IncompleteIdentity` before a replay identity is published
  (`incomplete_identity_is_rejected`).

## Identity and text values (`src/identity.rs`, `src/text.rs`, `tests/identity_values.rs`)

- `PlayerId` and `CompanionId` are distinct checked UUIDv4 newtypes over the
  same bits (`try_from_bytes`/`bytes`); `HostileId`, `PassiveId` and
  `ProjectileId` are checked nonzero `u64` wrappers (`try_new`/`get`). No
  unchecked public constructor and no valid NONE companion identity exists in
  this crate.
- `DisplayName`, `CompanionName`, `CommandText` and `SpeechText` are owned
  `String` newtypes built through `try_from_canonical`, which performs no trim
  and no normalization: admission owns the trim, exactly as Go's normalizer
  returns its trimmed output. `CompanionName` additionally rejects embedded
  Unicode whitespace.
- The whitespace and control predicates are explicit character ranges
  (U+0009..000D, 0020, 0085, 00A0, 1680, 2000..200A, 2028, 2029, 202F, 205F,
  3000 and U+0000..001F, U+007F..009F), never a Unicode crate, so behavior
  cannot drift with a Unicode-version bump and the crate's empty production
  dependency set holds. U+200B stays accepted, as in Go; there is no NFC.
- The display-name admission rule routes its scalar inspection through the
  private `is_canonical_display_name_with_visit` seam
  (`oversized_display_name_skips_scalar_scan` in `src/text.rs`): the empty and
  128-byte checks run before any `chars()` iterator is built, so an oversized
  input is rejected without inspecting a single scalar. `CompanionName`
  inherits that byte-first gate before its embedded-whitespace scan.
- `trim_pinned_whitespace(&str) -> &str` is the one trim rule, exposed as a
  borrowing helper so an admission path can trim before it constructs a
  canonical value: it trims the same pinned whitespace set the validation
  predicates use and returns a subslice of the caller's text. It exists
  because the Go admission path trims a raw name before validating it, and
  routing that trim through `str::trim` would read the standard library's
  Unicode tables. `DisplayName::try_from_canonical` itself stays strict and
  performs no trim; the caller owns the ordering
  (`trim_pinned_whitespace_trims_only_the_pinned_set` in `src/text.rs`).

## Resource bounds (`src/lib.rs`, `src/event/world.rs`, `src/event/people.rs`, `src/event/mobs.rs`, `src/event/objects.rs`, `tests/resource_bounds.rs`)

- `MAX_SEMANTIC_BATCH_RECORDS` (`4096`) is the shared semantic work cap for
  `BlockChanges`, `ForgetChunks`, `RemotePlayerStates`, `CompanionStates`, the
  three hostile batches, the three passive batches, the three projectile
  batches and the two item-drop batches. Each constructor checks it at its
  first line and reports `BatchTooLarge` before every content relation, so an
  oversized input costs a length compare instead of proportional scans, copies
  or sorts. It is a work bound, not a wire budget: the protocol packet ceilings
  (4096 block changes and forget chunks, 7 remote-player records, 4 companion
  records, 64 hostile and passive records, 128 projectile records, 32 drops)
  stay transport budgets in `mornlea_protocol`.
- `DomainError::NonFiniteValue` is the vector finiteness error
  (`FiniteVec3::try_new`); `NonFiniteRotation` stays the rotation error owned
  by `LookAngles::try_new`. `DomainError::Allocation` is the typed mapping of
  a failed scratch reservation: `ForgetChunks` sorts a `try_reserve_exact`
  scratch (`reserve_sorted_scratch` in `event/world.rs`,
  `allocation_failure_maps_to_typed_error`) and never copies, sorts or
  publishes before the reserve succeeds. The mob, projectile and item-drop
  batches own the provided box and reserve no scratch, so they have no
  allocation-failure path to pin.
- Precedence is pinned by `tests/resource_bounds.rs`: all fifteen batch types
  admit a 4096-record input and reject a 4097-record input that also violates a
  later content rule with `BatchTooLarge`, proving the gate precedes the
  content scan. The empty block-change revision barrier stays admitted.

## Item, drop and container values (`src/items.rs`, `src/locations.rs`, `tests/items_locations.rs`)

- Exactly one authoritative Rust copy of the item tables (stack limits,
  durability maxima, smelting output and product predicates) is ported from the
  verified Go core and compared exhaustively by item ID across 0..66; do not
  fork a second copy into another module.
- `ItemStack::try_new` reproduces the Go `Valid` precedence (registration,
  count, durability), admits the empty triple as exactly `(0,0,0)`, and rejects
  a durable item at durability 0. Equipped broken armor is deliberately
  outside this type: armor-owned slots are raw fidelity data and must not
  relax ordinary stack rules.
- `DropId::try_new` retains an arbitrary raw dimension because Go's
  `DropID.Valid` checks only the slot range and the generation; a stricter Rust
  rule is a parity break. Ordering is dimension, chunk x, chunk z, slot,
  generation.
- `ContainerRef::try_new` is inherently overworld and takes no dimension: the
  protocol conversion validates the raw wire dimension before construction.
  Absence is `Option<ContainerRef>`; there is no invalid domain reference.
- `ChunkPos` and `FiniteVec3`/`LookAngles` are plain coordinates and finite
  vectors: private fields, getters, no normalization, no clamping, and exact
  `f32` bit preservation including negative zero. `FiniteVec3::try_new`
  reports a non-finite component as `NonFiniteValue`, while `LookAngles`
  keeps the narrower `NonFiniteRotation` for its own angles.

## Value and input bounds (`src/values.rs`, `src/input.rs`, `src/input/control.rs`, `src/input/inventory.rs`, `src/input/chat.rs`, `tests/runtime_contract.rs`, `tests/command_control.rs`, `tests/command_inventory.rs`)

- `Dimension` accepts only overworld and depths; any other ID is
  `InvalidDimension` (`invalid_dimension_and_hotbar_ranges_are_rejected`).
- `HotbarSlot` accepts `0..COUNT-1`; `PlacementIntent::try_new` and
  `Command::SelectHotbar` reuse that range
  (`invalid_dimension_and_hotbar_ranges_are_rejected`,
  `command_control_placement_slot_eight_succeeds_and_nine_fails`).
- `LookAngles` is the single owner of the finite-rotation rule: NaN and
  infinities are `NonFiniteRotation`, and the bits are preserved exactly
  including negative zero. The grouped payloads in `input/control.rs` carry an
  already-validated `LookAngles`, so `PlayerControl::new` is total and named
  `new` rather than `try_new` (`non_finite_input_rotation_is_rejected`,
  `command_control_non_finite_rotation_fails_look_angles`).
- `Movement` and `HeldActions` are plain grouped controls with public fields:
  the move axes keep their full `i8` range because the −1..1 rule belongs to
  the authority, and `HeldActions::primary` maps exactly to the Go `Mining`
  bit without implying that mining wins over combat. No payload names a
  target cell, a hit entity, a placed block, a consumed item or an outcome.
- `Command` is the extensible intent enum over all 19 sequenced play
  variants: the movement and ray variants (`PlayerInput`, `PlaceBlock`,
  `Resync`, `SelectHotbar`, `OpenContainer`, `TillSoil`, `BoneMeal`,
  `CollectWater`, `PlaceWater`) plus the inventory and container variants
  (`MoveInventory`, `MoveCrafting`, `MoveContainer`, `CloseContainer`,
  `DropSelectedItem`, `TakeCraftingOutput`, `EquipArmor`, `MovePartial`,
  `QuickMove`, `DropStack`). No payload carries a sequence, an envelope, a
  target cell, a hit entity, a placed block, a consumed item or an outcome.
  `ResyncIntent` accepts a zero `have_revision` because it names a chunk the
  client holds nothing for.
- The inventory and container payloads in `input/inventory.rs` pin the exact
  Go bounds per command. `StackView {Inventory, Crafting, Container(ContainerRef)}`
  replaces the raw view numbers and the all-zero sentinel reference: absence of
  a container is expressed by the variant itself, so a zero `ContainerRef`
  never enters the domain type. `MoveCrafting` alone rejects two
  inventory-region indices, and `MoveContainer` alone rejects the furnace
  output slot as a destination; `MovePartial`, `QuickMove` and `DropStack`
  share the view and reference bounds without those stricter rules because
  the protocol does not reject them and the authority applies item and slot
  rules later. A malformed real container reference is rejected before slot
  checks. `TakeCraftingOutput` carries nothing but its variant because its
  nonzero-sequence rule belongs to envelope construction.
- `ChatIntent { text: CommandText }` in `input/chat.rs` is deliberately
  outside `Command`: chat has no wire sequence and is consumed through its own
  FIFO. It performs no addressing, warp, stop or queue policy, and the text is
  retained verbatim including a leading mention.

## Command envelope and ordering (`src/input/order.rs`, `tests/command_order.rs`)

- `CommandEnvelopeParts { tick, session, sequence, arrival_index: u64, command: Command }`
  is the only place intake metadata meets a payload. `CommandEnvelope::try_new`
  keeps the fields verbatim and rejects a zero sequence for
  `TakeCraftingOutput` alone, because that command has to take part in command
  acknowledgement; every other command accepts sequence zero, which is
  wire-valid. No payload carries a sequence of its own.
- `CommandOrderScratch::try_with_capacity` owns one `(u64, u64, u64)` key slot
  per command and never grows: `capacity` is the caller's budget, and an
  unreservable request fails closed with `InsufficientScratch` rather than
  aborting. `order_commands` refuses a batch larger than the scratch before it
  fills a single key.
- `order_commands` orders in place by `(tick, session, sequence,
  arrival_index)`. No key consults the command kind, so a same-sequence pair of
  different kinds resolves to the earliest arrival — the Go authority's
  behavior in `packages/server/sim/runtime/engine_step.go`, which has no
  kind-name tiebreaker. Session generation stays runtime ingress lifecycle data
  and is neither filtered nor checked here.
- The arrival key `(tick, session, arrival_index)` is validated before any
  command moves: a duplicate is `DuplicateArrival` and leaves the caller's
  slice byte for byte unchanged, because a caller cannot otherwise tell which
  of the two it submitted first. A warm call allocates nothing — the slice is
  sorted in place and the keys come from the scratch.
- Focused entry: `cargo test -p mornlea_domain --test command_order --locked`
  (12 cases: the red input, reverse arrival, same-kind duplicates, the same
  sequence across sessions, two ticks, duplicate arrival with unchanged input,
  an empty batch, exact and short scratch, scratch reuse after failure, the
  zero-sequence rule, the unreservable capacity, and the envelope carrying
  every payload family unchanged).

## Command outcomes and player publications (`src/event/outcome.rs`, `src/event/player.rs`, `tests/event_player.rs`)

- `CommandRejection`, `PlacementSuccess` and `CombatHit` in `event/outcome.rs` are
  the three results an authoritative tick publishes to the session that caused
  them. `CommandRejection::new` and `PlacementSuccess::new` are total and keep a
  zero sequence, which the Go wire accepts and the `/warp` rejects publish;
  `CombatHit::try_new` rejects a zero server tick and a damage value outside
  `1..=MAX_HEALTH`, which is the Go `CombatHit.Validate` rule.
- `RejectReason` is the closed set of the fifteen published reasons. Its wire
  value comes from the explicit `wire_id` match, never from a discriminant cast,
  so reordering the variants cannot change what the wire carries; the Go
  internal enum runs `0..14` while the wire enum runs `1..15`.
  `CombatTarget` is the same shape for the three published kinds
  (`try_new`/`wire_id`), and `Weather` and `Season` pin their wire IDs the same
  way.
- `PlayerState` in `event/player.rs` is the private per-session publication and
  nothing else: body, survival and world scalars. It carries no inventory, no
  crafting, no container and no equipped-armor field, because those are
  separate records owned by later nodes; the seed literal in the test lists
  every field explicitly, so an added field fails to compile.
- The grouped records follow the crate's parts convention.
  `MotionState::new` is total because its parts carry already-validated
  `FiniteVec3`s; `SurvivalState::try_new` rejects a health, oxygen, hunger or
  armor value above the Go `core` maximum rather than clamping it;
  `WorldState::try_new` rejects a day phase offset at or above one display day;
  season progress and temperature are the two full-range scalars and carry no
  sub-range rule.
- `MiningState::try_new` is the checked conversion of one wire mining block, not
  an inference. An inactive block has to be entirely empty — exact zero target,
  zero progress and requirement, false harvestable flag — because the Go
  validator rejects an inactive block that still carries part of a swing, and a
  client would otherwise have to guess whether a stale target still applies. An
  active block goes through `ActiveMining::try_new`, which requires
  `0 < progress < required` so a completed swing is published as inactive.
- `BlockPos` in `src/locations.rs` is the world block coordinate triple. Like
  `ChunkPos` it carries no invariant and constructs totally, because the Go
  `core.BlockPos` imposes no coordinate rule on the player-state mining target;
  `BlockPos::ORIGIN` is the exact zero target an inactive block has to carry.
- Focused entry: `cargo test -p mornlea_domain --test event_player --locked`
  (13 cases: the seed record and its field map, the absent inventory and
  armor, the four survival maxima plus one, the offset, weather, season and
  dimension boundaries, the full-range scalars, the non-finite pose and
  angles, the inactive mining residue, the active mining range, the zero
  sequences, the frozen reject-reason wire table, and the combat hit and
  target-kind boundaries).

## Section storage and world observations (`src/sections.rs`, `src/event/world.rs`, `tests/event_world.rs`)

- `sections.rs` owns the domain's single copy of the registered block numbering
  and the world geometry. `registered_block` is the exported numbering
  predicate beside the item predicates, and `chunk_block_index` is the
  chunk-ordered index a sorted block-change batch compares; both are ported
  from the Go `core` rules. This crate sits below `mornlea_protocol` and
  cannot import it, so the numbering lives here and the tests pin every value
  against the Go rule; the protocol crate re-exports `registered_block` and
  consumes this copy rather than growing a second one.
- `PalettedSection` is a private validated enum-backed value: a private
  `SectionStorage` enum holds `Single`, `Indexed4`, `Indexed8` and `Direct15`,
  so no caller can assemble storage the rules reject. `single`, `indexed` and
  `direct` are all fallible and check the Go `protocol.SectionData.Validate`
  rules — slot width, palette bounds, palette uniqueness and registration, the
  exact word count, every packed slot inside the palette, the direct words'
  unused high bits, and every decoded block's registration. The admitted set
  is exactly the Go validator's; only the check order differs, because the
  indexed constructor scans the palette once for uniqueness and registration
  before it validates the word count while the Go validator counts words
  first, so a doubly invalid section can report a different error without any
  input changing sides. `block_at` reads one cell straight out of the packed
  layout, and `as_single`/`as_indexed`/`as_direct` hand out borrowed slices, so
  no accessor exposes mutable storage and a codec conversion preserves the
  representation instead of recompressing or reordering a palette. The section
  count is the fixed array's type, and a 4096-cell section has no tail entry
  at any published width.
- `event/world.rs` holds the three compact world observations.
  `ChunkSnapshot` carries the fixed 24-section array, so Y is implicit in
  array order and the value has no Y field to disagree with its position; a
  zero revision is the only relation left to check because the dimension and
  every section are already validated. `BlockChanges` owns the revision
  transition (a base inside `1..=u64::MAX-1` and a new revision of exactly
  `base + 1`), the world Y span, the announced-chunk membership and the
  strictly increasing chunk-ordered index, and admits the empty batch as the
  revision barrier the Go validator allows. `ForgetChunks` requires a nonempty
  batch of distinct chunks and preserves the input order, sorting a fallible
  reserved scratch for the uniqueness check so the recorded sequence survives
  and a failed reservation maps to `Allocation` without publishing. The
  protocol packet ceilings and the wire-only section Y stay in the protocol
  layer, because they are transport budgets and layout fields rather than
  semantic relations; the domain adds only the shared
  `MAX_SEMANTIC_BATCH_RECORDS` work cap ahead of every content relation.
- Focused entry: `cargo test -p mornlea_domain --test event_world --locked`
  (17 cases: single air and the sentinel, the 4-bit two-entry palette with one
  cell set, the 8-bit ninety-ID palette, direct 15-bit block 89 at the last
  cell, the 255/256/257 word counts, the duplicate, unregistered, oversized
  and empty palettes, the unknown width, the slot outside the palette, the
  direct high bits, the preserved section 23 and the zero revision, the empty
  revision barrier, the negative chunk and world local index, the duplicate
  and unsorted changes, the revision relations, the positions outside the
  chunk or the world, the unregistered change block, and the forget order and
  rejections).

## Inventory and container publications (`src/event/inventory.rs`, `tests/event_inventory.rs`)

- `InventoryState`/`InventoryStateParts`, `CraftingState`/`CraftingStateParts`,
  `FurnaceState`/`FurnaceStateParts`, `ChestState`/`ChestStateParts` and
  `ContainerClosed` are the item and container publications an authoritative
  session sends to the one player that owns them. Every value rule is the Go
  `protocol` validator's rule for the same record, so a record this crate
  admits is a record the protocol layer admits.
- `InventoryState::new` is total because every part is already validated: the
  selected index is a `HotbarSlot` and every slot a validated `ItemStack`,
  which together are the exact Go `Inventory.Valid` rule.
  `CraftingState::try_new` rejects a non-empty slot beyond the personal grid's
  own cells (`InvalidCraftingResidue`), because the wire always carries all
  nine slots and a client would otherwise have to guess whether a residue slot
  still applies. `CraftingSize` is the closed `Personal`/`Workbench` enum
  behind the wire's `2`/`3` side lengths.
- `FurnaceState::try_new` and `ChestState::try_new` reuse node 2.3's
  `ContainerRef` validation rather than re-deriving the bounds: the reference
  has to name the record's own kind (`InvalidContainerKind`), while its slot
  range and generation were already checked when the reference was built.
  `ContainerClosed::new` is total and permits either kind, because the Go
  packet is named `FurnaceEnd` for historical reasons but carries both
  container kinds.
- `FurnaceState` also rejects a progress at or above the smelt requirement or
  a burn time above its maximum (`InvalidFurnaceTimers`) and a slot holding an
  item that slot cannot contain (`InvalidFurnaceSlot`): the input accepts the
  empty stack or a registered smelting input, the fuel the empty stack or
  coal, and the output the empty stack or a registered smelting product. The
  whitelists are read from the shared smelting tables in `items.rs` instead of
  being listed again. No timer consistency rule relating the progress, the
  burn time and the three stacks exists in the Go validator, and none is
  invented here.
- Three Go rules deliberately stay out of this module: a container reference's
  wire dimension (validated by the protocol conversion before a `ContainerRef`
  exists), the per-chunk slot counts and the nonzero generation (enforced by
  `ContainerRef` itself), and the per-stack validity rules (enforced by
  `ItemStack`). No record carries an equipped-armor array, exactly as the
  private player publication carries no inventory.
- Focused entry: `cargo test -p mornlea_domain --test event_inventory --locked`
  (17 cases: the all-empty inventory with the last slot selected and the
  preserved selected index and stacks, the personal residue at slots 4 and 8
  beside the four usable slots, the workbench's ninth slot, the furnace seed
  and its idle form, every smelting input and product, the three slot
  whitelists, the progress and burn boundaries, both wrong-kind references,
  the chest seed, the both-kinds closure, and the absent equipped-armor
  array).

## Remote-player and companion observations (`src/event/people.rs`, `tests/event_people.rs`)

- `RemotePlayerSpawn`/`RemotePlayerSpawnParts`, `RemotePlayerDespawn`,
  `RemotePlayerState`/`RemotePlayerStateParts`,
  `RemotePlayerStates`/`RemotePlayerStatesParts`,
  `CompanionSpawn`/`CompanionSpawnParts`, `CompanionDespawn`,
  `CompanionState`/`CompanionStateParts` and
  `CompanionStates`/`CompanionStatesParts` are the visibility-derived
  observations an authoritative session publishes about the other players and
  the companions one subscriber can see. Every value rule is the Go `protocol`
  validator's rule for the same record, so a record this crate admits is a
  record the protocol layer admits.
- The batch record types omit the enclosing tick because the batch owns it:
  `server_tick` is the domain's single name for the publish tick, and the Go
  `CompanionStates.Tick` is renamed to match rather than keeping a second name
  for the same concept. A spawn's flattened yaw and pitch become one `look`.
  A despawn is a checked `PlayerId` / `CompanionId`, never raw bytes.
- The batch structs own `states: Box<[...]>` of already-validated records and
  require a batch inside the shared `MAX_SEMANTIC_BATCH_RECORDS` work cap
  (`BatchTooLarge`), nonempty (`EmptyStateBatch`), and strictly increasing by
  identity (`InvalidStateOrder`). The 7-record remote-player and 4-record
  companion wire maxima stay in the protocol layer because they are transport
  budgets, so a domain batch of eight or five records is still publishable and
  a protocol adapter splits it.
- The per-record rules differ by subject and the difference is the Go one: a
  remote-player record accepts either playable dimension and any finite pitch
  (`4.0` is publishable), while a companion record accepts the overworld alone
  and a pitch inside the inclusive ±pi/2 range
  (`InvalidCompanionDimension`, `InvalidCompanionPitch`). Both rules are pinned
  per record and inside the batch.
- No record carries a profile, a persona, mining progress or a velocity: the Go
  `CompanionUpdate` keeps `velocity`, `ground` and `mining` for the simulation,
  and the remote-player publication drops the survival, inventory and mining
  fields the private player publication owns. `event_people.rs` pins the
  absence by naming every field of every record in its seed literals, so an
  added field fails to compile.
- Constructor totality follows the crate convention: the despawns and the
  remote-player records are total because every part is an already-validated
  domain value, while the two companion records are fallible because the
  companion dimension and pitch rules are relations a type cannot express.
- Focused entry: `cargo test -p mornlea_domain --test event_people --locked`
  (29 cases: the six seed records, the zero tick, the depths dimension, the
  unbounded remote pitch, the checked identities, the empty, reversed and
  duplicate batches, the two above-wire-maximum batches, the inclusive pitch
  limits, the next float above the limit, the spaced companion name, and the
  absent profile, persona, mining and velocity fields).

## Hostile and passive mob observations (`src/event/mobs.rs`, `tests/event_mobs.rs`)

- `HostileSpawnRecord`/`HostileStateRecord` (with `Parts`), the
  `HostileSpawn`/`HostileState`/`HostileDespawn` batches, and the passive
  mirror set (`PassiveSpawnRecord`/`PassiveStateRecord`, the total
  `PassiveDespawnRecord::new`, `PassiveSpawn`/`PassiveState`/`PassiveDespawn`)
  are the visibility-derived mob publications an authoritative session sends
  about the hostiles and passive mobs one subscriber can see. Every record
  rule is the Go `protocol` mob validator's rule, so a record this crate
  admits is a record the protocol layer admits.
- Spawn records check the overworld dimension (`InvalidDimension`), then yaw
  finiteness (`NonFiniteRotation`, checked here because the record carries a
  raw `f32` yaw rather than a `LookAngles`), then health inside
  `1..=MAX_HEALTH` (`InvalidSurvivalValue`), in that order; state records
  check yaw then health. Identity and position/velocity finiteness are
  already enforced by `HostileId`/`PassiveId` and `FiniteVec3` before a
  record can be assembled. State records carry no dimension because a
  dimension change always goes through a despawn/spawn pair.
- The six batch constructors apply the shared cap → empty → strict-ID-order
  sequence (`BatchTooLarge`, `EmptyStateBatch`, `InvalidStateOrder`) with no
  copy or sort and admit a zero tick. The 64-record packet maxima of both
  families are transport budgets in `mornlea_protocol`, not domain rules: a
  65-record batch is still publishable and a protocol adapter splits it.
- `HostileKind` (`Nightwalker`/`BoneThrower`) and `PassiveDespawnReason`
  (`Vanished`/`Died`) are closed enums; the raw kind/grazing/reason byte
  conversions belong to evidence and protocol adapters, and grazing is stored
  as the domain `bool`. No record exposes cooldown, target, path, AI or
  capacity state; the hostile despawn carries identities only while the
  passive despawn carries the closed reason pair.
- Focused entry: `cargo test -p mornlea_domain --test event_mobs --locked`
  (18 cases: the two seed records with a zero tick, the triple-violation
  dimension-precedence rejections, the yaw-before-health rejections, the
  health boundaries, the no-dimension state fields with grazing, the
  vanished/died pair, the empty/reversed/duplicate batch rejections, the two
  65-record packet-separation batches, and the checked identity/vector
  gates).

## Projectile and item-drop observations (`src/event/objects.rs`, `tests/event_objects.rs`)

- `ProjectileSpawnRecord`/`ProjectileStateRecord` (with `Parts`), the
  `ProjectileSpawn`/`ProjectileState`/`ProjectileDespawn` batches,
  `ItemDrop`/`ItemDropParts` and the `ItemDropUpserts`/`ItemDropRemoves`
  batches are the object publications an authoritative session sends about
  the projectiles in flight and the dropped item stacks one subscriber can
  see. Every record rule is the Go `protocol` object validator's rule, so a
  record this crate admits is a record the protocol layer admits.
- The projectile record constructors are total (`new`) because every part is
  an already-checked domain value: the identity is a nonzero `ProjectileId`,
  the kind one of the two closed `ProjectileKind` variants, the dimension
  one of the two playable ones, and both vectors finite `FiniteVec3`s. All
  four kind × dimension combinations are publishable — the
  kind-by-dimension policy is an authority concern the wire and this domain
  do not enforce. The state record carries no kind, dimension or velocity
  because all three are fixed for the projectile's whole life; the spawn
  carries no yaw or health because a projectile is a point-like transient
  whose orientation the client derives from its velocity.
- `ItemDrop::try_new` is the one fallible record: the chunk-local block
  index has to stay below the chunk's 24 × 4096 cells, and the first value
  outside is rejected as `DomainError::InvalidBlockIndex` (defined in
  `src/identity.rs`) rather than clamped. `DropId`'s arbitrary raw dimension
  and the shared `ItemStack` registration/count/durability rules are
  consumed unchanged, so a drop cannot publish a slot value the inventory
  families reject. The drop record carries no world position and no pickup
  timer, despawn countdown or velocity: those are authority-side lifecycle
  quantities the publication never puts on the wire.
- The five batch constructors apply the shared cap → empty → strict-order
  sequence (`BatchTooLarge`, `EmptyStateBatch`, `InvalidStateOrder`) with no
  copy or sort and admit a zero tick. The projectile batches order by the
  typed numeric `ProjectileId`; the drop batches order by the derived
  `DropId` ordering (dimension, chunk column, slot, generation) without
  normalizing raw dimensions. The 128-record projectile and 32-record drop
  packet maxima are transport budgets in `mornlea_protocol`, not domain
  rules: a 129-record or 33-record batch is still publishable and a
  protocol adapter splits it.
- Focused entry: `cargo test -p mornlea_domain --test event_objects --locked`
  (16 cases: the four kind × dimension combinations, the spawn fields, the
  identity-and-position-only state record, the tick-and-ids-only despawn,
  the empty/reversed/duplicate batch rejections, the two above-wire-maximum
  packet-separation batches, the checked identity/vector gates, the raw
  negative dimension, the empty stack and index 98_303 boundary, the 98_304
  no-clamp rejection, the shared stack rules, the zero tick, and the absent
  authority-only fields).

## Chat event observations (`src/event/chat.rs`, `tests/event_chat.rs`)

- `ChatEvent` (with `ChatEventParts`) is the chat fact an authoritative
  session confirms for one player: the issuing player's checked identity and
  name plus the closed `ChatBody` union. `ChatEvent::try_new` rejects only a
  zero event id (`InvalidIdentity`) and owns the checked player fields and
  the body without normalization; the event id names the chat
  acknowledgment itself, not a `CommandEnvelope` sequence, because a chat
  command travels through its own FIFO with no sequence at all. The value
  carries no routing recipient, publish tick or raw reason byte.
- `ChatBody` is the closed semantic union with exactly seven variants
  (`Accepted`, `InvalidFormat`, `UnknownCompanion`, `QueueFull`,
  `NotFollowing`, `Task`, `Speech`) unfolding into the sixteen legal branch
  shapes the Go `protocol.ChatEvent` validator admits (the four rejections,
  the five plain task facts, the five failure reasons, speech and the
  accepted command). Every illegal cross-field combination — speech on a
  non-speech branch, a command on speech or malformed format, a leaked
  companion identity on malformed format or unknown companion — has no
  constructible state: the variant field lists are the rule.
- `TaskState` (`Started`/`Progress`/`Completed`/`TimedOut`/`Stopped`/
  `Failed(TaskFailure)`) mirrors the Go task fact kinds, and `TaskFailure`
  the closed `TaskFailReason` 16..=20 set that rides only inside the failed
  state. `CompanionSpeaker` is total from the checked `CompanionId` and
  `CompanionName`. Absence is the variant shape, never a zero identity: the
  malformed-format branch carries nothing and the unknown-companion branch
  only the target name. Raw invalid kind, reason and text combinations stay
  corpus-adapter concerns; no type here exposes an enum number or an
  `Unknown` member.
- Focused entry: `cargo test -p mornlea_domain --test event_chat --locked`
  (10 cases: the zero-id rejection, the player field preservation with the
  `u64::MAX` boundary, the five addressing branches, the five plain task
  states, the five failure reasons, the speech branch, the sixteen-branch
  exhaustive classification, the zero-identity absence proof, the
  command/speech slot separation counts, and the absent envelope fields).

## Event surface and routing (`src/event.rs`, `tests/event_surface.rs`)

- `Event` is the closed set of the thirty semantic publications this crate
  owns, declared in the frozen order
  `event_surface_constructs_exactly_30_semantic_variants` pins: every
  payload is a checked leaf value, no variant carries a packet ID, raw
  bytes, a digest, a handshake/login/rejection/keepalive/disconnect fact, a
  chunk-worker lifecycle message or a generated-chunk pointer, and there is
  no catch-all. The construction test's name match is exhaustive without a
  wildcard, so an unplanned variant is a compile failure rather than a
  silently accepted shape.
- `EventRecipient::Session(u64)` admits zero because session existence and
  broadcast policy are runtime concerns; `Broadcast` is an explicit shape
  and never a sentinel session. `RoutedEvent::new` stores the recipient and
  the event unchanged, adds no tick of its own, and exposes
  `recipient()`/`event()`
  (`routed_event_preserves_session_zero_and_event`,
  `routed_event_preserves_broadcast_and_event`).
- The crate publishes no digest helper: the retired digest-era public names
  are gone from `src/event.rs` and `src/lib.rs`, and
  `domain_public_api_has_no_digest_observation_exports` keeps them out.
- Focused entry: `cargo test -p mornlea_domain --test event_surface --locked`
  (4 cases: the retired-name source gate, the 30-variant construction in
  the declared order, and the two recipient-form preservation tests).

## Domain corpus dispatch (`tests/corpus_domain.rs`, `tests/corpus_domain/`)

- Integration test `corpus_domain.rs` loads `testdata/runtime-migration/contracts.json`
  and routes all 533 `mornlea_domain` cases into eleven closed topic modules:
  `identity_text` (31), `values` (99), `command_control` (28), `command_inventory` (54),
  `event_player` (47), `event_world` (38), `event_inventory` (33), `event_people` (46),
  `event_mobs` (68), `event_objects` (45), and `event_chat` (44).
- Every domain case is JSON-formatted and specifies operation `admit`. Single ownership
  is enforced across the closed partition; catch-all predicates are prohibited.
- `event_mobs.rs` owns the six hostile/passive mob rules (`hostile-spawn`,
  `hostile-state`, `hostile-despawn`, `passive-spawn`, `passive-state`,
  `passive-despawn`). Each executor classifies the raw batch in the Go
  validator's precedence — the raw count above the shared 4,096 semantic cap
  first, then an empty batch, then each record in submitted order, then the
  strictly-increasing identity rule — verifies the mapped `DomainError` through
  the checked constructors, and normalizes accepted fields from Rust getters.
  An unknown hostile kind, grazing or despawn-reason byte cannot enter a
  closed Rust type, so those branches stay classifier-only and no `Unknown`
  variant is invented; submitted record order is never sorted.
- `event_objects.rs` owns the five projectile and item-drop rules
  (`projectile-spawn`, `projectile-state`, `projectile-despawn`,
  `item-drop-upserts`, `item-drop-removes`) with the same classifier /
  constructor / normalizer discipline. The raw count above the shared 4,096
  semantic cap classifies first, then each record walks its Go field order
  (projectile identity, kind, dimension, finite pose and velocity; drop
  slot, generation, block index, then the shared stack precedence), then
  the strict batch order compares the full `DropId` key with the raw
  dimension first. An unknown projectile kind or dimension byte stays
  classifier-only, the raw drop dimension `-1` and `ItemStack::EMPTY`
  construct and publish back losslessly, and submitted record order is
  never sorted.
- `event_chat.rs` owns the sole chat rule (`chat`), closing the 533-case
  partition with 44 cases and 321 total `domain.event` executions. The
  executor parses every raw field, replays the Go `ChatEvent.Validate`
  precedence — the global event/player identity and name gates, the speech
  slot's kind exclusivity before kind dispatch, then inside the switch the
  reason, companion identity, companion name and command/speech text — and
  verifies each constructible culprit through the checked constructors. A
  zero event identity reaches `ChatEvent::try_new` around an otherwise
  valid synthetic event and must return `InvalidIdentity`, so the domain
  proof stays in the domain type. An admitted event publishes its exact
  semantic branch (`accepted`, the four rejection branches, the five task
  facts, `task-failed-<reason>`, `speech`) with only that branch's legal
  fields; a rejection retains every raw input including unknown numeric
  kind and reason values. Unknown kind, reserved reject-reason, out-of-
  domain failure-reason and illegal cross-field combinations stay
  classifier-only; no `Unknown` variant is invented.
- Nine semantic anti-copy mutations in `corpus_domain.rs`
  (`corpus_domain_rejects_mutated_*`) each clone one loaded `FrozenCase`,
  mutate exactly one field of its frozen expected normalized JSON, keep the
  independently executed actual value, and require `assert_domain_normalized`
  to panic inside `catch_unwind` while the unmutated pair passes first.
  Together they cover the semantic-drift classes the delta specification
  names across the mob, passive, projectile, item-drop and chat families:
  kind, health, grazing, despawn-reason, dimension and velocity bits, a
  block index, a record-order swap and a chat branch category.
- `domain.input/45/session-sequence-arrival` and
  `domain.input/45/stale-sequence-no-effect` are assigned to `external:runtime-authority`
  because their expectations carry authoritative admission, deduplication,
  discard and world observations. Pure domain execution still covers 533 cases.
- `support.rs` provides shared strict JSON parsing, primitive type readers, exact-array
  length bounds, float-token and UUID hex parsers, normalized outcome builders, and
  command wire-stripping comparison.

## Focused Verification

```bash
rustup run 1.97.1 cargo test --manifest-path packages/engine/Cargo.toml -p mornlea_domain --test resource_bounds --locked
rustup run 1.97.1 cargo test --manifest-path packages/engine/Cargo.toml -p mornlea_domain --test corpus_domain corpus_structure --locked
rustup run 1.97.1 cargo test --manifest-path packages/engine/Cargo.toml -p mornlea_domain --test corpus_domain support:: --locked
rustup run 1.97.1 cargo test --manifest-path packages/engine/Cargo.toml -p mornlea_domain --test command_order --locked
rustup run 1.97.1 cargo test --manifest-path packages/engine/Cargo.toml -p mornlea_domain --test runtime_contract --locked -- --list
rustup run 1.97.1 cargo test --manifest-path packages/engine/Cargo.toml -p mornlea_domain --test runtime_contract --locked
rustup run 1.97.1 cargo test --manifest-path packages/engine/Cargo.toml -p mornlea_domain --test identity_values --locked
rustup run 1.97.1 cargo test --manifest-path packages/engine/Cargo.toml -p mornlea_domain --test items_locations --locked
rustup run 1.97.1 cargo test --manifest-path packages/engine/Cargo.toml -p mornlea_domain --test command_control --locked
rustup run 1.97.1 cargo test --manifest-path packages/engine/Cargo.toml -p mornlea_domain --test command_inventory --locked
rustup run 1.97.1 cargo test --manifest-path packages/engine/Cargo.toml -p mornlea_domain --test event_player --locked
rustup run 1.97.1 cargo test --manifest-path packages/engine/Cargo.toml -p mornlea_domain --test event_inventory --locked
rustup run 1.97.1 cargo test --manifest-path packages/engine/Cargo.toml -p mornlea_domain --test event_world --locked
rustup run 1.97.1 cargo test --manifest-path packages/engine/Cargo.toml -p mornlea_domain --test event_people --locked
rustup run 1.97.1 cargo test --manifest-path packages/engine/Cargo.toml -p mornlea_domain --test event_mobs --locked
rustup run 1.97.1 cargo test --manifest-path packages/engine/Cargo.toml -p mornlea_domain --test event_objects --locked
rustup run 1.97.1 cargo test --manifest-path packages/engine/Cargo.toml -p mornlea_domain --test event_chat --locked
rustup run 1.97.1 cargo test --manifest-path packages/engine/Cargo.toml -p mornlea_domain --test event_surface --locked
```
