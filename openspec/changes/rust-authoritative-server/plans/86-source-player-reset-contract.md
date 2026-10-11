# Source player reset mapping contract

> Use superpowers:executing-plans and test-driven-development. tasks.md is the sole status source.

**Goal:** Land the shared checked reset mapping consumed by the separately reviewable active recovery and unified death nodes. The actual caller still owns crafting/drop settlement, scan restart, transient cleanup and authoritative staging. An executed local consumer double accepts this mapping only; it does not accept actual reset integration.

**Prerequisites:** Initial source registration83 source834822714658fbbd65a6f29ab637f2c956e5d5fc/root9542e53f is accepted at ROOTf9d52f31751aee4a8f8b95a5014989e882dc2763 after clean independent1,097actual+3docs and ROOT1,285actual+3docs/release19.97s/static/audit/strict128, pushed and remote exact-verified. Dispatch uses the ROOT reconciliation commit containing these accepted identities. Accepted scan81 sourcea365a178eb5d34e8c660f17ae593a5c6795c10cf/root7e1e8113b935513a0ba6d5eaaf41fb98f30242f8 supplies the exclusive completed Player scan and checked captured anchor/radius. No source83 gate remains pending; source reset implementation still requires the frozen packet below. Source authority is sealed Go d042982d33bb1694d768b75b01c297bd02534a08, packages/server/sim/entity/player.go:817 beginReset and death.go:35 settleDeaths. ROOT verified exact source, including preserved health/hunger on generic reset; those quantities are filled only by death settlement.

## Exact ownership and contract

Exactly FOUR editable tracked paths under packages/engine/crates/mornlea_server: new src/core/source_player_reset.rs; src/core/mod.rs ONE private module registration; src/core/pending_restore.rs ONE crate-private checked captured-anchor accessor and replacement of restart_player's duplicate initial kind/completed guard with its call, plus accessor tests in the existing private test module; AGENTS.md ownership paragraph. All authority/step/source_player_restore/rules/other tests/contracts/transport/store/Go/binary/native/versions/dependencies/F1/seals/OpenSpec/status/ledger are read-only. Unit mapping/double tests belong to the new module. No new directory invariant or public trait/port/value is warranted: this is a private pure mapping with two upcoming consumers.

Add exactly:

```rust
// PendingRestore, crate-private; never returns an anchor for an incomplete or Companion owner.
pub(crate) fn player_reset_anchor(&self) -> Result<ChunkPos, ServerError>;
// Private core module, crate-visible to future sibling consumers.
pub(crate) fn begin_reset(
    actor: &mut ActorRecord,
    runtime: &mut ActorRuntime,
    restore: &PendingRestore,
) -> Result<(), ServerError>;
```

The accessor checks kind==RestoreKind::Player AND completed before returning the captured self.anchor; either refusal is InvalidInput{field:"restore_restart"}, matching existing restart_player. It never reads metadata or recalculates columns. restart_player calls this accessor at its existing first validation point, retaining identical error precedence before candidates.len()>1, identical retained capacity1, restart algorithm and all existing behavior. No second anchor, mirrored scan state, new clone, public getter or whole scan transfer is added.

begin_reset accepts a matching Player actor/body/runtime/Player aux at lifecycleActive. Check in one initial condition: ActorKey::Player, ActorBody::Player, runtime.key==actor.key, ActorAux::Player and lifecycleActive. Any mismatch returns InvalidInput{field:"source_player_reset"}, before touching either value or consulting scan readiness. Then obtain restore.player_reset_anchor(), propagating restore_restart. Construct the source motion and checked survival before the first write. Impossible checked-value construction returns Internal{invariant:"source player reset"}, with both inputs unchanged. All fallible work precedes mutation; no later operation can refuse.

## Exact source field map and work

Source position is [(anchor.x() as f32)*16.0+0.5,321.0,(anchor.z() as f32)*16.0+0.5], exactly the source f32 arithmetic already qualified by83. Zero velocity and false on_ground. Actor lifecycle becomes Pending; current actor dimension and look remain unchanged. Rebuild SurvivalState with existing health, hunger, saturation_zero and armor_points and oxygen300. Never fill health/hunger or alter body.current/body.safe/body.inventory/body.armor/bed fields; durable stale body fields remain stale until the accepted projector applies live overlays.

Runtime overrides: controlsNone, resetfalse, attack_cooldown0, hurt_cooldown0, oxygen300, peak_y321, drown_ticks0, eatingNone, bowNone. Preserve key, has_view, burn_cooldown, exhaustion_milli, saturation_milli, since_damage_ticks, starvation_ticks, path and entire Player aux including live respawn/workbench. Runtime.path has public Vec fields and PlayerSave.display_name is an owned String: DO NOT clone any actor/runtime/body/path in this helper or claim arbitrary public values have fixed byte bounds. Mapping mutates only fixed scalar/Copy fields in place, leaves both existing path Vec allocations and body String allocation untouched, and performs O(1) work with zero allocation and no collection traversal, independent of their lengths/capacities. Caller preparation/compound staging has separate work responsibility. No scan allocation, geometry/world access, metadata lookup, unsafe input, thread, clock, publication or whole-map clone occurs.

Illustrative exact construction before mutation:

```rust
let anchor = restore.player_reset_anchor()?;
let position = FiniteVec3::try_new([
    (anchor.x() as f32) * 16.0 + 0.5,
    321.0,
    (anchor.z() as f32) * 16.0 + 0.5,
]).map_err(|_| ServerError::Internal { invariant: "source player reset" })?;
let velocity = FiniteVec3::try_new([0.0; 3])
    .map_err(|_| ServerError::Internal { invariant: "source player reset" })?;
let survival = SurvivalState::try_new(SurvivalStateParts {
    health: actor.survival.health(),
    oxygen: 300,
    hunger: actor.survival.hunger(),
    saturation_zero: actor.survival.saturation_zero(),
    armor_points: actor.survival.armor_points(),
}).map_err(|_| ServerError::Internal { invariant: "source player reset" })?;
let motion = MotionState::new(MotionStateParts { position, velocity, on_ground: false });
```

Apply the explicit override list above only after these constructors succeed. A narrowly scoped temporary #[allow(dead_code)] is permitted on begin_reset until actual recovery consumes it; no module-wide allowance or new dependency. Explain its private contract and future serial consumers in an English ownership comment. Crate guide names actual mapping qualification and leaves actual reset/death/subscription/save/runtime acceptance open.

## Frozen consumer responsibilities and serial order

First future consumer: after regen/starvation, eating and bow and after reset short-circuit, but before oxygen/native motion, source active recovery evaluates Y<-80 or source unstick failure. It prepares locally owned actor/runtime, invokes begin_reset using its existing completed SourcePlayerEntry, stages the pair atomically and calls existing restart_player(current_dimension,vec![]). Clear actual context mining/sleeping/suppression/own action charges through a serially owned context operation; reset the separately accepted persistent snow tracker once that owner is qualified (current step reconstructs FootprintSchedule per tick and does not establish this duty); do not erase the live aux bed/workbench or raw ACK. Preserve entry.ever_spawned and captured anchor/radius/columns. Recovery never fills health/hunger. These are future node responsibilities, not code editable here.

Second future consumer: actual late unified death completes source crafting repack and ready-ring partial-prefix inventory/armor drops, fills health20 and hunger20/saturation5000/exhaustion0/reset regen AND starvation_ticks0, validates optional LIVE runtime bed in current dimension, then invokes this SAME common mapping and restarts the SAME completed scan with zero/one validated bed candidate. It preserves ever_spawned and performs the same transient cleanup/staging; next tick's restoration decides activation. Prevent current legacy early death from preempting late source crafting/drop settlement through a separately frozen source-mode change. No death case is accepted merely by the local double below.

Actual success is separately source-qualified; first local mapper consumer can only claim common field mapping. ROOT serially owns their core/state/step/rules registrations and integration; no parallel shared-file editors.

## Concrete test-first cases

Use existing seed_player and player_survival::merged_runtime to build valid locally owned actor/runtime, or explicit checked constructors with the existing fixed PlayerSave shape. Get a checked completed scan by PendingRestore::try_new(Player,DEPTHS,anchor(-2,3),radius1,vec![RestoreCandidate{dimension:OVERWORLD,position:[8.5,65.,8.5],require_support:false}]) and advance it against a TEST DOUBLE PlacementWorld with key(Overworld,0,0)Ready, all blocksAIR/revision9. Candidate completes independently of anchor and preserves captured anchor(-2,3). Double bounded Ready methods are fixture providers, not actual world evidence.

1. Compile-ready inert begin_reset returning Ok() must fail behavioral assertions: Active becomes Pending at[-31.5,321.,48.5], zero velocity/falseground, oxygen300, resetfalse and neutral transient fields. Record absent import/setup failure separately; never count it as behavioral RED.
2. Recovery local consumer: active actor CURRENTdimOverworld, nonzero velocity/trueground, look(.1,.2), health7/hunger9/oxygen3/saturation_zerofalse/armor_points4; stale body.current in Depths at[100,64,100], body.safe distinct and stored bed fields distinct from live aux. Runtime controlsSome,resettrue,has_viewtrue,attack11/hurt12/burn13,oxygen3/peak90,exhaustion250/saturation9000/since_damage17/drown18/starvation19,eatingSome/bowSome,liveaux bedDepths(-1,2,0)/workbench(2,3,4). Invoke helper and assert EVERY override and EVERY preserved field. Selected inventory/crafting are external values and must remain unchanged in local consumer, raw ACK remains separate scalar3, ever_spawned remains separate true. These labels are DOUBLE/prepared-owner evidence only.
3. Death local consumer prepares the same pair but before common helper explicitly sets health20/hunger20/saturation_zerofalse/runtime.saturation5000/exhaustion0/since_damage0/starvation_ticks0. Invoke common helper; these caller-prepared full values AND starvation_ticks0 survive, oxygen fills and same reset map holds; recovery double keeps starvation_ticks19. This proves death mapping reuse, not crafting/drop/bed world validation.
4. Allocation-preservation table: PlayerSave.display_name String with capacity65536; runtime.path Some(PathState{generation7,target(1,2,3),revisions Vec with length9/spare65536,waypoints Vec length17/spare65536,cursor2,next_repath_tick900}). Record pointers/lengths/capacities, values and body safe/current BEFORE helper. Mapping must preserve all exactly with no clone/traversal; tests may clone snapshots OUTSIDE measured helper if needed, but not implement global allocator or allocate inside helper. This protects valid but arbitrarily capacious public inputs without inventing a Player path cap. Allocation claims come from source inspection and exact preserved pointers, not a global benchmark.
5. Identity/refusal table covers nonPlayerkey, wrongbody, runtimekey mismatch, nonPlayeraux, Pending/Respawning/Dead lifecycle. Deliberately combine a mismatched pair with an incomplete scan and require source_player_reset precedence. Each refusal preserves both full values. Incomplete Player scan and completed Companion scan with a valid Player pair yield restore_restart and unchanged pair. Companion constructor radius16 with oneAirReady candidate can complete against the same double. Getter refusal also preserves scan debug/state; getter success repeated returns captured anchor without changing ready progress/pending_keys/allocations. Existing restart tests qualify downstream restart guard/capacity/order, rerun them without changing expectations.
6. Negative anchors(-2,3)/(2,-1), metadata unrelated to actual anchor (helper has no metadata input), very large but CHECKED spawn anchor(134217723,-134217724)radius64: expected pose arithmetic uses the source f32 expression, not f64 multiplication before narrowing. Current dimension remains actor dimension even when the COMPLETED restore dimension differs: the common candidate completes the scan in Overworld (activate updates its dimension), so explicitly set actor.current dimensionDepths for this witness and require it remainsDepths. ConstructorDepths alone is not a differing-completed-dimension witness. Source declared positions are finite; no NaN actor injection or unsafe construction.

## Validation and closure

All Cargo commands explicitly --locked under source /workspace/.mornlea-env/env.sh and private CARGO_TARGET_DIR. Run new source_player_reset:: tests nonzero and existing pending_restore:: tests17 plus accessor cases; full lib/contracts/replay/default-thread parity, doc3 and acquisition8/save6/driver22/encoding2/retirement2/projection1/source_player_restore4/actualAgent6. Subprocess-owning lib/replay/Agent run via /workspace/scratch/linux-activation-owned-supervisor.py raw Cargo argv with pinned MORNLEA_AGENT_PYTHON=/workspace/mornlea/packages/agent/.venv/bin/python; no global process reap/test serialization/deadline extension. Build make rust serially BEFORE any Go helper if private implementation clean checkout requires it; pinned previous artifacts remain read-only. ROOT owns rebuilt release/all-server/all49activation acceptance; worker must not duplicate full activation or rebuild previous.

Commands: cargo test --manifest-path packages/engine/Cargo.toml -p mornlea_server --lib --locked; --test server_contract; --test server_replay; --test local_remote_parity; --test persistence_failure followed separately by live_acquisition::,live_chunk_saves::,chunk_driver::,chunk_encoding::,chunk_retirement::,actor_projection::,source_player_restore::; --test agent_process; --doc. EVERY test invocation retains --locked. cargo clippy --manifest-path packages/engine/Cargo.toml -p mornlea_server --all-targets --locked -- -D warnings; cargo fmt --manifest-path packages/engine/Cargo.toml --all -- --check; git diff --check. Keep raw RED/GREEN/setup logs distinct /workspace/scratch/source-player-reset-contract-*.log. Audit exact FOUR paths/no new public API/ONEprivate mod/current ROOT task-ID Englishcomments/251 source-oracle hashes/native/previous/dependencies/versions/F1 and repo-wide derived-consumer enumeration. Only existing pending_restore and new mapper tests should consume this private change; any other consumer requiring refresh returns to ROOT for scope update.

Commit feat(server): define source player reset mapping. Worker report sourceSHA/exactcount/commands/all failures/classifications/bounds; no docs/status/ledger/push/cleanup. ROOT fresh independent exactSHA review, integrate exact bytes, cumulative gates and strict128 before only3.7l3rc closes. ROOT records accepted contract SHA before either actual consumer dispatches. Rollback removes this unused helper/private registration/guide and restores the old equivalent restart guard/accessor. No accepted prior node is reopened without concrete evidence. Architecture skill review promotes only stable rules; otherwise no change.

ROOT readiness self-review: shared two-consumer mapping is explicit, no public adapter layer; all source fields/error ordering/allocations/file ownership/doubles/exclusions/serial consumers are defined. Independent source criticism in /workspace/scratch/source-player-reset-contract-readiness.log (original packetSHA52efd07558e94f8569e6874bda0cb3e44b932c1c9e021afc77b7f1d67fa25404) identified exactly the death starvation reset and completed-dimension fixture corrections above; ROOT independently verified hunger.go167–172 and PendingRestore::activate250–257 and reconciled them, with accepted83 prerequisite recorded. Implementation dispatch is now source-ready at this reconciliation commit; no worker chooses reset semantics or fills omitted source behavior.
