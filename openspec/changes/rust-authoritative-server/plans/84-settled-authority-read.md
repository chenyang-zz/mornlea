# Settled authority read implementation plan

> Use superpowers:executing-plans and test-driven-development. tasks.md is the sole status source.

**Goal:** Borrow the healthy committed authority in constant construction work for actual actor-save projection and later Agent/publication consumers, without cloning residents or exposing unprocessed ingress as settled state.

**Architecture:** Reuse the existing public AuthorityReadView exactly. Add one healthy authority factory; no new port, copied snapshot, resident world mirror, owner, history or tick counter. Source-read-only evidence /workspace/scratch/settled-read-clock-source-trace.log enumerates every field/constructor/caller. Existing shared view and accepted actor projection79 source b26784b5750f9ffc7a1fb9128fdde51ecfdce8fd integrated849b2949aa1b13d481265a6ff90f344f7addc01a are prerequisites. Accepted pending restore source a365a178eb5d34e8c660f17ae593a5c6795c10cf, integrated repair7e1e8113b935513a0ba6d5eaaf41fb98f30242f8, is the serial prerequisite; root cumulative source baseline49589f1480c522fa71888dd3aa48ed2c8964fc69 passes1,258 actual cases and2doctests. Dispatch only after its acceptance/push because state/guide integration is root-owned. No accepted81 policy is changed. Root supplies all choices below.

## Exact files and API

Exactly THREE editable tracked paths under packages/engine/crates/mornlea_server: src/core/state.rs (factory/getter, private tests, one existing hard-failure test assertion); AGENTS.md (ownership guide); tests/persistence_failure/actor_projection.rs (use borrowed factory in the existing actual login/native tick/save/reopen case). No mod/export/test registry change. All step/rules/other tests/Agent/transport/store/Go/F1/native/source/oracle/version/dependency files read-only. No derived source/hash scanner consumes these changed Rust implementations; the existing phase-order source file stays byte-identical. Root integration and rollback own these3paths serially.

```rust
impl AuthorityState {
    pub fn settled_read(&self) -> Result<AuthorityReadView<'_>, ServerError>;
}
```

The existing AuthorityReadView methods/signatures remain unchanged. world() deliberately remainsNone for a settled view: the WorldState mirror is context-local and includes presentation fields; committed EnvironmentState is the actual climate owner. world_time() now selects existing explicit world.world_time_ticks first, then environment.world_time, then metadata.world_time_ticks. This preserves fixture World overrides and gives current absolute time to a real healthy settled/early-tick consumer. No current production .world_time() consumer exists, so this is boundary qualification, not a demonstrated existing gameplay clock repair.

Exact construction:

```rust
static SETTLED_PRE_STEP: BTreeMap<ActorKey, MotionState> = BTreeMap::new();
// Within settled_read, first:
if let Some(error) = self.tick_failure { return Err(error); }
if self.phase == ServerPhase::Closed {
    return Err(ServerError::InvalidState { phase: self.phase });
}
Ok(AuthorityReadView {
    acquisition: self.acquisition.enabled().then_some(&self.acquisition),
    tick: self.next_tick,
    world: None,
    commands: &[], companions: &[], interactions: &[],
    inventories: &self.residents.inventories,
    blocks: &self.residents.blocks,
    ready: &self.residents.ready,
    actors: &self.residents.actors,
    runtimes: &self.residents.runtimes,
    mining: &self.residents.mining,
    pre_step: &SETTLED_PRE_STEP,
    environment: self.residents.environment.as_ref(),
    containers: &self.residents.containers,
    container_chunks: &self.residents.container_chunks,
    viewers: &self.views,
    drops: &self.residents.drops,
    projectiles: &self.residents.projectiles,
    damage_intents: &[],
    metadata: &self.metadata,
    observation_trace: None,
})
```

Getter expression: self.world.map(|world| world.world_time_ticks()).or_else(|| self.environment.map(|environment| environment.world_time)).unwrap_or(self.metadata.world_time_ticks). No clock mutation, invented last-completed counter or global scan.

## Health, clocks and borrowing

Healthy Running and Closing can read, including the interval after successful final reduction and before close. Closed refuses. First retained tick failure takes precedence even if a separately injected test sets Closed; partial resident data after failure cannot create a new healthy publication/save/Agent basis. Do not hide that error or resurrect the authority. Old immutable obligations remain unaffected.

.tick() retains its existing endpoint convention: fresh0; while a context executes0; after successful advance1. On a settled view it names the NEXT executable endpoint index, not the most recent publication's executing index. Current Rust wire first-tick0 versus Go source first-tick1 is separately unqualified; this packet does not change any wire/process/hashing tick or saturating policy. .world_time() is independent absolute persisted climate time. Actual EnvironmentEnd advances environment; the ephemeral World mirror does not survive resident return. Direct reduce_tick does not bump the endpoint; final run_final does on success. Tests name the driver used. Failure may leave advanced resident environment with unchanged endpoint; factory refusal fences that state.

An immutable factory borrow excludes a simultaneous exclusive TickContext/advance_tick loan by Rust borrowing. Add ONE compile_fail doctest to the factory showing a borrowed view used after authority.advance_tick(full) in the same scope (E0502). Exact imports: mornlea_server::contracts::{ServerLimits,TickBudget}; mornlea_server::state::AuthorityState; construct limits(8,4096,512,64,64,1_048_576),seed42; let view=authority.settled_read().unwrap(); authority.advance_tick(TickBudget::full()).unwrap(); assert_eq!(view.tick(),0). No unsafe/interior mutable state/unsafe trait assumptions.

## Structural bounds and test-first oracles

Factory constructs22 fields in constant work, no allocation, iteration, reference count bump, Ready clone or materialization. Borrowed methods keep their existing semantics: enumeration explicitly allocates when called, all retained actors may include retired history, sparse cells do not manufacture Ready and managed Unloading is unavailable. This is no global history bound, eligibility selector, actor persistence owner, snapshot constructor or full runtime qualification.

Use one new private state.rs test topic. Callable inert factory may return Internal{invariant:"settled read unavailable"}; preserve declarations for compiled substantive RED. Keep original getter for causal clock RED. Exact cases:

1. cold_metadata_clock: checked restored metadata world_time1200/seed42, no environment; healthy settled tick0/worldNone/world_time1200, spawn anchor exact and all ephemeral lanes empty. Original early-harness view with no explicit World/Environment also reads metadata1200 rather than0 after the getter change. Pure initialization/metadata qualification only.
2. committed_borrow_identity_and_current_write: fresh authority, actual for_tick/full context; preload ONE real Ready key(Overworld,0,0),generation1,revision9,24SingleAir sections,32drop/32furnace/16chest default slots as accepted80; prepare/install/activate one ordinary session before the context. Test login has UUID bytes[0]=51,[6]=0x40,[8]=0x80, other bytes0; build checked domain PlayerId, LoginStart(name Settled,view8), decode_inbound and admit_login. Inside the context, canonical_player(playerID,Settled) with current.position=[8.5,64,8.5] supplies seed_player(session,&save); context.stage_login installs its actor/inventory. freeze_environment0, derive actual player_survival::merged_runtime(&context.read(),&actor), and context.stage(RuleEffect::Runtime(runtime)). No nonexistent public runtime initializer. A new actual observation at(8,63,8), BlockWrite::try_new(observed,2), transaction.try_system(SystemRule::Support,vec![write]) accepts; commit_carried/drop. BEFORE factory capture each resident map address, actor/projectile slice pointer and capacities, Ready generation/revision/block/cache and actor fields; reset ready_clones/materializations AFTER setup. View borrows exactly every matching resident/authority field by ptr::eq (including fixed states/environment/metadata/viewers); tick0,block2,Readyrevision10,height63, inventory/runtime same addresses; ephemeral pre_step/commands/companion/interactions/damage empty, traceNone, worldNone. Assert zero Ready clones/materializations and identical owner addresses/scalars after view. Repeat factory8 times. This is actual committed current-read provider work; no source activation/store ACK claim. No sparse writes over Ready or fabricated cache.
3. actual_clock_progress_and_final: restored1200; actual for_tick freeze0 begins worldNone, environment1200 and getter1200; drop without commit returns the newly frozen Some(environment1200) into residents; only original sleep absence has special restoration. actual advance_tick(full) publication0/endpoint1, env/worldtime1201; settled tick1/time1201/worldNone; second advance publication1/endpoint2/time1202. begin_close then run_final(&mut AuthoritativeFinalReducer) returns executing2, endpoint3/time1203 with no new delivery (record outbox prefix count before final; final cannot append); healthy Closing factory succeeds. No player/session required; this clock-only case has no receiver delivery witness, while accepted existing actual-final contract cases retain their receiver checks. No mock final reducer.
4. fixture_world_override: harness/freeze_environment0 with restored1200 and explicit WorldState::try_new(WorldStateParts{day_phase_offset:0,world_time_ticks:999,weather:Weather::Clear,season:Season::Spring,season_progress:0,temperature:11}).unwrap() constructed locally; a private sibling fixture helper is not callable => getter999 even environment1200; clear context.world privately toNone =>1200; stage actual EnvironmentEnd through environment::run(RuleCall phaseEnvironmentEnd,None args) =>1201 while read.tick remains0. This tests real getter precedence, not equality of fixture world and environment.
5. phases_and_first_error: healthy freshRunning and begin_close Closing succeed; explicitly injected terminal Closed refuses InvalidState Closed. Separate authority fail_tick(Internal{invariant:"settled read test failure"}) then fail_tick(Disconnected) retains first; factory returns original error, no reads/state mutations; injected Closed still original firsterror. Labels identify injected terminal/firsterror states, not real shutdown proof.
6. pending_ingress_is_not_settled: ordinary admitted session, submit CloseContainer before any tick; authority queue nonempty, factory commands/companion/interactions/damage/preStep empty and original queue identity/content unchanged. Distinct per-context data cannot leak through factory. Only the command lane needs actual ingress to establish causality; no fake companion queue proof is needed.

In existing failed_tick_tests::failed_facts add assert_eq!(a.settled_read().err(),Some(error)); existing capacity/unwind/postcommit delivery/final evidence then executes the real new health fence. No other existing test outcomes or fixtures changed.

## Actual producer-consumer integration

In tests/persistence_failure/actor_projection.rs existing actual case, replace both state.residents() captures with factory borrows. The PRE borrow lives in a block returning only position:[f32;3]; inspect initial .1/.2look and selected0 through borrowed actor/inventory. Drop that borrow before transport input/advance. POST borrow encloses all original projection assertions and returns only the checked PlayerSave; actor via view.actor, inventory via view.inventory, runtime via view.runtime. Assert view.world_time equals its environment and endpoint tick equals state.next_tick without conflating publication tick. The external integration target cannot access cfg(test) pub(crate) Ready counters. No visibility/export is added: zero clone/materialization counters belong solely to private state case2; the actual integration proves borrowed producer/consumer field flow through the factory and existing projection. Source body remains stale; live native horizontal motion, sequence2, selected5, every original field/respawn/source inequality, exact submitted/completed ticket/revisions, storeclose and full reopen/needs_rewritefalse assertions remain unchanged. Remove only the old cloned snapshot comment; add concise English ownership/borrow purpose comments without task IDs. No transport/admission/chunk-driving/deadline/native/world fixtures change. Actual fixture failure on inert factory is separate behavioral RED; do not count absent-method/import failures.

## Validation and closure

Scope3paths; fresh compiled private behavior/clock RED plus existing actual integration RED, then green6private+all priorlib, contracts275/replay442/default-thread parity40, acquisition8/save6/driver22/encoding2/retirement2/actor_projection1, doc3, all-targetClippy-Dwarnings/workspacefmt/diff. Exact counts grow after accepted81; enumerate actual output rather than retaining pre81lib counts. Source /workspace/.mornlea-env/env.sh and explicit private CARGO_TARGET_DIR; sequential make rust COMPLETE before Gohelpers; pinned Python/previous package and owned-child supervisor exactly as81. Do not rerun all activation49/previous-package build in worker. Current ROOT task-ID/new-English-comment/scope/source/native/F1/oracle/previous/dependency/version audit mandatory.

Raw /workspace/scratch/settled-authority-read-*.log; exact source/SHA root first review, fresh isolated independent review and cumulative actual rebuilt release acceptance before only3.7r4 closes. Commit feat(server): expose healthy settled authority reads. Rollback only factory/getter/tests/guide, no external data shape. This accepted shared boundary will be named by later Agent/current-world/publisher/actor-target consumers; their providers/integration remain separate. Architecture skill round-end review follows evidence; current expectation no change.
