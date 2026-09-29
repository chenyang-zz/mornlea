# Frozen hostile melee input boundary

Controller node2.7c0 lands the compile-ready contract before the hostile action
producer and melee settlement consumer can be implemented independently. It
owns S/src/core/contracts.rs, S/tests/server_contract.rs and new
S/tests/server_contract/combat_input.rs plus guides/artifacts. Other providers
are read-only. S=packages/engine/crates/mornlea_server. The controller implements
this small contract directly; no producer/provider acceptance follows from its
doubles. No wire/save changes or runtime default change.

Add public HostileMeleeAttack with private attacker:HostileId and
 target:SessionKey. new(HostileId,SessionKey)->Self is total; getters attacker()
and target() return typed copies. It represents an already admitted internal
walker attack choice, never a client claim and never a saved UUID chase target.
Absence from a batch means no melee intent for that actor in this tick.

Add public HostileMeleeBatch with private tick:u64 and
entries:Vec<HostileMeleeAttack>. try_new(tick:u64,entries:&[HostileMeleeAttack])
->Result<Self,ServerError> first rejects len>64 as Capacity{RuleEffects,limit64,
observed:len}, then rejects duplicate attacker IDs as InvalidInput field
"hostile_melee" (bounded pairwise scan, before copying). It copies and sorts
unique entries by attacker ID; tick() and entries()->&[HostileMeleeAttack] expose
immutable observations. Empty is legal; tick0 and u64::MAX are legal scalar
instants. Derive Clone/Debug/PartialEq/Eq. Do not validate current liveness,
dimension, range or actor kind here; the actual combat consumer does that from
its frozen current authority. No truncation and no partial acceptance. The
producer first applies source earliest-valid action semantics and forms unique
entries; it cannot delegate duplicate policy to this boundary.

Test constructor REDs first with a compile-ready inert rejecting constructor:
empty accepted/tick retained;64 unique unordered entries accepted and sorted
without modifying caller;65 rejected with exact capacity;duplicate attacker
with same or different target rejected without modifying caller. Mint two
SessionKeys through real AuthorityState admission. Add callable producer and
consumer doubles that carry a target different from another live session and
an old/new tick unchanged; assert exact IDs/tick, not merely type-checking. These
doubles do not infer actor damage, validate target life or accept production.

Run pinned Rust1.97.1 server_contract combat_input, full server, clippy -D
warnings, fmt, strict OpenSpec/diff; independent review then scoped commit.
Main records accepted SHA in both subsequent packets and owns integration and
rollback/revalidation of affected consumers. Architecture skill:no change.

## Source findings that constrain following packets

Go server.go358-361 and hostile_manager.go166-204 freeze actions from pre-step
hostiles and Ready/live players before player physics. Target selection is due-
gated; runners resolve the selected target against this pre-physics snapshot.
Walker in horizontal1.8 submits its target even while cooling down; combat later
snapshots decremented cooldown and post-motion distance. The manager comment
suggesting otherwise loses to code at hostile_manager.go310-317.

Hurler tries an eye-to-eye unobstructed shot while projected shoot cooldown is0,
then uses >14 approach /6..14 hold /<6 retreat. Cooldown1 prevents manager shot
this tick even though consumption decrements it to0. Actions apply after spawn
and before hostile physics; new spawn has no pre-step action and skips physics.
Go runtime inbox bounds64 actions; first valid known ID wins, ranged suppresses
movement/melee, shot uses seed+authority tick+ID spread and cooldown40.

Existing Rust hostile motion recomputes nearest target from post-player-motion
view every tick, does not filter zero-health targets, and uses walker1.8 stop
for hurler too. Its current provider acceptance therefore excludes source-exact
attack decision timing and hurler strategy; new2.7a1 owns this reconciliation.
The immutable pre-physics snapshot and resulting action plan belong to the
single reducer, with motion/shot providers consuming it rather than silently
reselecting later. Native path/physics kernels remain the numerical owners.

Go deathDropChunks enumerates ALL loaded Ready chunks in the same dimension,
sorted by Chebyshev ring then stable chunk order; plan04's fixed0,1,2 truncation
was wrong and is removed. Player death first repacks all nine crafting cells,
then tries each36+4 source slot independently across those chunks; unplaceable
items remain for respawn. Hostile/passive loot is one whole batch at first fit,
all-full omits loot but still removes the actor. Hurler loot hashes world_time,
not tick (the source call, not its outdated comment, controls). These are death
packet requirements, not behavior implemented by this input contract.
