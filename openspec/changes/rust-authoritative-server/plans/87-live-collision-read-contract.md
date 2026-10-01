# Managed collision read contract

> Use superpowers:executing-plans and test-driven-development. tasks.md is the sole status source.

**Goal:** Land the shared collision-block read for four real native actor-grid producers. Managed world cells outside the world height are loaded AIR before horizontal readiness; mutation observations remain unavailable there. Sparse disabled fixtures retain their previous collision inputs. The executable contract and consumer doubles precede the separate four-producer integration; no actual falling/void recovery is accepted here.

**Prerequisites:** Accepted settled read84 source7e1c3c585128b67780e5a21f9a308e12420fccae/root8865b46f/accept00286679 and initial83 source834822714658fbbd65a6f29ab637f2c956e5d5fc/root9542e53f/acceptf9d52f31. Reset86 sourcef46c45ab647687e2c2d15580b288409baff9b88d/root6c29488b accepted/pushed1233a806d35ced97d2d5e7ac3b65bdd0b15ede4d after ROOT1,295actual+3docs/release35.18s/static/audit/strict128 and clean fresh1,107actual+3docs. ROOT owns serial state/guide integration. Use the ROOT planning reconciliation commit as dispatch baseline. Sealed Go source d042982d33bb1694d768b75b01c297bd02534a08 remains read-only.

## Verified source problem and semantic boundary

Go packages/server/sim/realm/state.go469–477 Dimension.BlockAt first returns AirID,true when Y<-64 or Y>=320, before any horizontal record lookup. Otherwise only ChunkReady supplies its current block; missing/loading/unloading is unavailable. packages/server/sim/entity/spawn.go74–81 dimensionCollisionSource.CollisionBoxes converts exactly that block/readiness, and shared/physics/step.go234 builds the real native grid. Accepted Rust actor_placement.rs64–79 already normalizes source out-of-height AIR for placement, while mutation observation deliberately returnsNone outside[-64,320). Current player_motion.rs868,companions.rs766,passives.rs1914,hostile_actors.rs1927 build their physical grids from observation and convertNone to unloaded CollisionCell. The native unknown-as-blocking policy then supplies a false boundary floor/ceiling even for managed worlds.

Do not change native unknown-as-blocking policy, F1 kernels, ABI, SourceBlockObservation/CAS or all legacy fixture data. Add one private semantic read; the future actual producer node replaces exactly those four grid sampling calls. Existing F1 CollisionGrid/CollisionCell contracts remain accepted, no new cross-language interface or version. Source projects/placements/path/projection/publication are separate consumers and remain unchanged.

## Exact ownership and API

Exactly TWO editable tracked crate paths under packages/engine/crates/mornlea_server: src/core/state.rs (one crate-private getter on AuthorityReadView and a new private live_collision_read_tests group); AGENTS.md (ownership/validation paragraph). All existing methods/fields/factory/tick/session/step/rules/other tests/registries/contracts/geometry/scan/reset/store/transport/native/Go/dependencies/versions/F1/seals/OpenSpec/status/ledger remain read-only. No new module/directory/trait/port/owner/enum or public API; crate guide remains directory owner.

Exact method:

```rust
pub(crate) fn live_collision_block(
    &self,
    dimension: Dimension,
    pos: mornlea_domain::BlockPos,
) -> Option<u16>;
```

Exact algorithm:

```rust
if self.acquisition.is_some() && !(-64..320).contains(&pos.y()) {
    return Some(0);
}
self.observation(dimension, pos).map(|observed| observed.block)
```

acquisitionSome means the actual enabled live managed world, matching the existing read-view factory and TickContext construction; no new flag or presentation mirror inference. Disabled manual/sparse fixtures retain observation-based lookup at every coordinate. Do not use radius/source-player enablement: companions/hostiles/passives consume the same live world even without a source player. Do not require a horizontal Ready key before the managed out-of-height AIR branch; that would contradict the source. In-height reads retain existing live availability gating, current accepted writes, dimensions and traced observation semantics. Outside managed height the getter does not fabricate a BlockObservation/generation/revision/CAS address or add a mutation trace record. Generic block(), observation(), available(), actor_placement and all22existing read fields stay byte-identical.

The added branch allocates nothing and cannot refuse; dimension/block position are already checked domain values. Delegated observation retains its existing allocation, work and trace-capacity refusal behavior when a recorder is attached. The added branch is constant work; delegated existing in-height read has its existing BTree/current compact lookup cost, not an invented O(1) whole lookup or global byte/work bound. Every result is already registered block or AIR. No new scan, clone, materialization, world I/O, fallback store, protocol frame or mutable owner. The current settled factory's lifetime/failure/phase/clock behavior remains exact and its compile-fail borrow doctest continues passing. A narrowly scoped temporary #[allow(dead_code)] on this getter is permitted until actual motion consumers land; no broader lint allowance. Explain why collision read differs from mutation observation in English.

## Frozen four-producer integration successor

One serial actual-consumer node, after acceptance of this contract SHA, owns ONLY rules/player_motion.rs,companions.rs,hostile_actors.rs,passives.rs grid sampling replacements, removal of this getter's dead-code allow, guide and their concrete tests. Each existing prism loop retains y/x/z order, existing4096-cell request/prism checks, Cell conversion and native PhysicsRequest/tuning/held controls. Replace observation.map(block) with live_collision_block; None remainsCollisionCell::default(),Some(block) uses existing collision_cell(block). No broad observation replacement: fluid, sneak, ray, mutation and terrain data remain unchanged unless a separately frozen source finding requires it. Enumerate every production NativePhysics/CollisionGrid constructor repository-wide before dispatch; current list is exactly four actor producers. Projectile/ray kernels are different interfaces and not claimed accepted by this actor-grid node.

Actual integration must demonstrate native downward travel through the lower height boundary and upward motion through the upper plane using the live world and real login/acquisition for players, plus analogous checked actual provider calls for all other actor kinds; missing in-height columns remain blocking and sparse disabled fixtures remain exact. Such tests use actual native F1 kernel output, not guessed poses, fake native results or direct private actor edits masquerading as actual login. ROOT freezes that successor's full recipe before dispatch. Only after its acceptance can actual source void recovery be qualified; unstick/safe/death/Snow/source clocks remain separate serial nodes.

## Concrete contract-first tests

Use private actual AuthorityState+TickContext fixtures and accepted ReadyChunk/acquisition ownership; manual successful prepared offers qualify THIS read owner only, not real disk/ChunkDriver integration. Full compact Chunk shape24sections/32drops/32furnaces/16chests, revision9/generation from real reserve_chunk_load/bind. Set section0 and section23 SingleStone1, all other sectionsAIR. KeyOverworld(0,0), optional Depths(0,0) with different block2. enable_live_chunks, replace_chunk_wants, reserve/bind/PreparedChunk/offer_acquired and actual advance_tick install Acquire exactly as existing state private acquisition fixtures. Canonical metadata/environment seeded normally. No actor/native/Godot process launched. New helpers remain inside the new test module; do not export/change old private helpers.

Baseline absent declaration/import is setup-only. Compile-ready inert getter delegating observation at all heights must produce behavioral RED on managed out-of-height expectedSome0 and both consumer doubles. Record distinct setup/RED/GREEN logs; do not count a compile failure as behavior.

1. Height/unknown-horizontal table: managed known key queryY-65→Some0,-64→Some1,319→Some1,320→Some0, plus i32MIN/MAX→Some0. Unknown horizontal key(9,-3):Y-64/319→None,Y-65/320/i32MIN/MAX→Some0. Explicit source ybound320, never319 as outside; no horizontal Ready requirement outsideheight. Repeated reads do not change next tick/metadata/current Ready generation/revision/body/cache.
2. Managed phase/dimension table: loading samekey in each dimension yields in-heightNone and outsideSome0; ReadyOverworld1 vs ReadyDepths2 yields distinct in-height block values; unknown otherkeyNone. replace wants to remove a Ready key makes it Unloading with body retained: inside becomesNone, outside remainsSome0. Verify source phase/retained body identity so the unavailable case is causal, not absent data. No reload or physical retirement acceptance.
3. Actual current mutation: after Ready install, build exclusive context, obtain observation at(8,319,8) block1, perform actual transaction.try_system(SystemRule::Support,vec![BlockWrite::try_new(old,2)]) and require getter2 BEFORE context drop and after retained commit/drop; generic observation shows2/current pending revision10. GetteroutsideSome0 but generic block/observation there stillNone before/after; cannot manufacture a write target. Use existing world::reset_ready_clones/reset_materializations then50bounded untraced reads, assert counters0; setup/snapshot clones are outside measurement. Check the state-visible outer resident ReadyChunk entry address, generation and revision, plus the existing readable current BlockObservation node reference after the actual write. Those observations stay unchanged across getter calls. Private ReadyChunk base/page Arc identities and capacities are inaccessible in this exact two-path scope and are explicitly not claimed; no world.rs accessor, unsafe layout inspection or scope expansion.
4. Disabled sparse fixture: insert existing checked BlockObservation for position(8,64,8) into residents.blocks using its existing insert API, no live_acq enablement and no full Ready body. Factory/context getter returns that exact block and missing neighborNone, all outside-height queriesNone exactly as old observation. Explicit source-player radius may be enabled only in a separate pristine fixture without acquisition: it DOES NOT switch this collision read mode and outside remainsNone. This witnesses that the mode is managed acquisition, not a player-specific flag.
5. Factory/context consumer agreement: same healthy live owner yields same boundary/in-height values through settled_read and TickContext::read in lexical separate loans. HealthyClosing borrow remains readable; existing failed/Closed factory guards are inherited unchanged and need no mirrored new test. Tick/absolute clock convention and pending ingress exclusions remain exact. No new field/reference/capture owner. Also attach existing with_observation_trace to a local RefCell<ObservationTrace>: managed outsideY320 getterSome0 leaves cells empty/overflowfalse; in-height known(8,319,8) getterSome1 records exactly its actual Some(BlockObservation), and in-height missing(144,319,-48) getterNone records exactly None. Repeated reads retain two entries. In a separate trace prefilled with512 distinct in-height keys, an additional new in-height query returnsNone and sets overflowtrue under the existing policy; subsequent managed outsideY320 still returnsSome0 without adding a trace entry or clearing overflow. Keep ordinary generic outside observation calls separate from this trace assertion because those calls deliberately record None.
6. GridBuilderDouble: a test-only function takes &AuthorityReadView, explicit dimension/origin/dimensions checked≤4cells and iterates y,x,z exactly. GetterSome converts using accepted crate::rules::player_motion::collision_cell; None uses CollisionCell::default. Construct actual checked CollisionGrid from these cells. Live unknown-horizontal prism origin(144,319,-48), dimensions[1,2,1] has cellY319loadedfalse andY320loadedtrue/used0; known key origin(8,319,8) hasY319loadedtrue/used1 andY320loadedtrue/used0. Disabled outside cells loadedfalse. This double compiles/executes the accepted native INPUT contract but does not execute actual actor motion or prove kernel output.
7. GeometryConsumerDouble: test-only PlacementWorld wrapper delegates ready_revision to actual view.ready_chunk_revision and block_at to live_collision_block; no new production trait. Accepted body_space on live unknown-horizontal pose[144.5,321.,-47.5] returns free/readytrue because every physical cell is outsideheight; pose[144.5,319.,-47.5] returns readyfalse from missing in-height cell. Known key body at[8.5,319.,8.5] sees solid collision (freefalse,readytrue); disabled outside pose waits. This consumes actual accepted geometry against the DOUBLE read adapter and is labeled contract-only, not SourcePlayer motion/restore/Ready producer integration.

Existing self.observation query calls preserve all its tracing/current-write details. Dummy wrapper constructors/helpers allocate only in test setup; production getter remains noncopying. Do not introduce a global allocator, unsafe values, mocked native motion, arbitrary path cap or fresh whole-body snapshots for every query.

## Exact execution recipe

From the isolated repository root, use the pinned environment and raw supervisor argv below. Run commands serially; each command's raw output goes to a distinct immutable scratch log. Discovery lists each selected set before its actual run; no zero-case success qualifies a set. ROOT owns all49activation and release acceptance, so worker persistence filters exclude activation.

```bash
source /workspace/.mornlea-env/env.sh
export CARGO_TARGET_DIR=/workspace/scratch/mornlea-live-collision-read-contract-target
export MORNLEA_AGENT_PYTHON=/workspace/mornlea/packages/agent/.venv/bin/python
export MORNLEA_PREVIOUS_PACKAGE=/tmp/mornlea-previous-package-final-3f732022/previous-runtime.json
export MORNLEA_PREVIOUS_SERVER_BIN=/tmp/mornlea-previous-package-final-3f732022/previous-server
make rust
cargo test --manifest-path packages/engine/Cargo.toml -p mornlea_server --lib --locked live_collision_read_tests -- --list
python3 /workspace/scratch/linux-activation-owned-supervisor.py cargo test --manifest-path packages/engine/Cargo.toml -p mornlea_server --lib --locked live_collision_read_tests
cargo test --manifest-path packages/engine/Cargo.toml -p mornlea_server --lib --locked -- --list
python3 /workspace/scratch/linux-activation-owned-supervisor.py cargo test --manifest-path packages/engine/Cargo.toml -p mornlea_server --lib --locked
```

For each topic in server_contract, server_replay, local_remote_parity, agent_process, discover using `cargo test --manifest-path packages/engine/Cargo.toml -p mornlea_server --test TOPIC --locked -- --list`, then run `python3 /workspace/scratch/linux-activation-owned-supervisor.py cargo test --manifest-path packages/engine/Cargo.toml -p mornlea_server --test TOPIC --locked`. For each exact filter in live_acquisition::, live_chunk_saves::, chunk_driver::, chunk_encoding::, chunk_retirement::, actor_projection::, source_player_restore::, use that same discovery/run pair with `--test persistence_failure --locked FILTER` before `-- --list`. These are shell recipe placeholders for the explicitly enumerated finite topic/filter sets, not an unspecified choice of suites.

```bash
python3 /workspace/scratch/linux-activation-owned-supervisor.py cargo test --manifest-path packages/engine/Cargo.toml -p mornlea_server --doc --locked
cargo clippy --manifest-path packages/engine/Cargo.toml -p mornlea_server --all-targets --locked -- -D warnings
cargo fmt --manifest-path packages/engine/Cargo.toml --all -- --check
git diff --check
go test ./packages/audit -run '^TestCodeCommentsExcludeTaskIDs$' -count=1
```

The Go helper runs only after serial make rust completes. Include the separate stronger ROOT165-ID/English/251-seal/exact-two-path/native/previous/protected-path/derived-consumer audit in the report; the Go scanner alone does not check every current OpenSpec ID. No source scanner refresh is authorized. Worker does not run ROOT OpenSpec status/acceptance or alter the original branch.

## Validation, source consumers and closure

Every Cargo test/clippy command includes --locked, original default threads/deadlines, private CARGO_TARGET_DIR. Implementation checkout make rust must complete serially before any Go helper, with canonical native hashes unchanged. source /workspace/.mornlea-env/env.sh; pinned MORNLEA_AGENT_PYTHON=/workspace/mornlea/packages/agent/.venv/bin/python and previous manifest /tmp/mornlea-previous-package-final-3f732022/previous-runtime.json plus previous-server. Helper-bearing lib/replay/Agent and all persistence topics execute rawCargoargv via /workspace/scratch/linux-activation-owned-supervisor.py, no global kill/reap/--help/test serialization/deadline extension. No full49activation/previous rebuild by worker; ROOT owns rebuilt release/allserver acceptance.

Run new lib live_collision_read_tests nonzero, full lib baseline298+newcases, server_contract275, server_replay443, default-thread local_remote_parity40, persistence_failure topics live_acquisition8/live_chunk_saves6/chunk_driver22/chunk_encoding2/chunk_retirement2/actor_projection1/source_player_restore4, actualAgent6/doc3; alltargetclippy-Dwarnings/workspacefmt/diff. Prefix distinctraw /workspace/scratch/live-collision-read-contract-*.log. Audit exactlyTWOtrackedpaths/onecrate-privategetter/noPublicAPI/unchangedallothermethods/22fields/factory/Englishrevisedcomments/latestROOTIDs/251source-oracle/native/previous/dependency/version/F1/seals. Repository-wide hashed/generated/embedded/source-scanned consumers: no Rust state implementation is a capability seal; current phase_order reads step.rs unchanged; full-corpus hashes sealedGo. English/taskID/identity/guide source scanners retain their gates. Any unexpected tracked refresh requires ROOT ownership/packet update before code change.

Commit feat(server): define managed collision block reads. Worker reports exactSHA/commands/counts/behaviorRED/setup distinction/source limits, no status/docs/ledger/push/cleanup. ROOT full first review and fresh independent exactSHA review plus applicable cumulative release/all49activation/doc/static/audit/strict128 before ONLY3.7l3mc closes. Record accepted contract SHA before four-producer node dispatch. Rollback removes unused getter/tests/guide paragraph; no world/wire/save/data migration or F1 refresh. Architecture skill: no change unless verified stable cross-task rule merits promotion; no history duplication.

ROOT readiness self-review: one existing borrowed read is shared by four separately reasoned physical producers, so compile-ready/double executable boundary precedes actual consumers. Scope/mode/sourcebounds/borrow/currentCAS/error-freevalue/work/field identity/late consumer DAG/files/test doubles/RED/limits/serial review are explicit. Independent source criticism must validate this packet before implementation dispatch; prior source and policy proof is not completed code.
