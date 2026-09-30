# Whole-branch review evidence

This document preserves the independent review reports against baseline7ff06060. Reports describe baseline findings; current repair status is solely in tasks.md and ledger.md. Their temporary reproduction paths are evidence locations, not runtime dependencies. The first core pass intentionally consolidated integration findings; subsequent provider-local passes complete the code review.


# Independent F2 core/rules review

Baseline: `/Users/chen/work/mornlea-f2-server`, branch `cursor/rust-authoritative-server-98e6`, HEAD `7ff060609448538412c7d90e71e4bfd29f26ba1e`, versus dev `365a0339`. Read-only review; no repository edits or commits. Review was consolidated at controller request before exhaustive line-by-line review of every large rule module; the integration/ownership findings below are verified by code, targeted tests, and a standalone executable reproduction. Do not interpret absence of another provider-local finding as complete audit of that provider.

## Verdict

Current inventory and integration evidence does **not** support zero-gap F2 acceptance. The spec requires equivalence for every supported rule and actual authoritative state/events/save observations; current production assembly omits whole families and the executing reducer has continuity defects. Keeping the Go default does not waive opt-in server requirements. Ledger's final ruling that mob loading, chunk streaming, projection, and gameplay transport no longer block opt-in qualification is incompatible with accepting a complete Rust server; these can describe a control-plane qualification only.

## Verified findings (highest priority first)

### P1: player motion overwrites every sibling runtime lane

`packages/engine/crates/mornlea_server/src/rules/player_motion.rs:366-387` constructs a fresh `ActorRuntime` with zero saturation/exhaustion, since-damage/drowning/starvation timers, attack/hurt cooldowns, eating, bow, respawn/workbench, has_view and reset. `state.rs:2511-2515` replaces the whole keyed record. There is no reducer field merge despite the comment claiming it. The reducer runs survival/food/bow before motion, so those successful effects are immediately erased. Health regeneration and drowning/starvation counters cannot accumulate; hurt/attack protection resets every tick; food/bow cannot progress; a just-opened workbench loses its anchor next tick; saved respawn data vanishes.

Standalone reproduction after normal prepare/install(None)/activate/advance prints runtime `has_view=false, saturation_milli=0` for every login. The existing login test at `tests/server_replay/tick_state.rs:214-220` expressly excludes the saturation assertion and describes the loss as a pre-existing evolution; that is not a parity acceptance.

Required tests: full live reducer, saved nonzero saturation/exhaustion and cooldowns, >=32 eating ticks, bow hold/release, >80 underwater ticks, >=regen delay, multi-tick fall peak and retained workbench/respawn. Preserve all sibling lanes, not just fields immediately covered by one test.

### P1: next tick restores climate from stale metadata

`state.rs:1660-1674` rebuilds climate from `authority.metadata` every tick, replacing carried residents.environment. `environment.rs:103-151` advances the context only; `commit_residents` never updates metadata, nor is there any production metadata load/install setter. No code increments the persisted metadata sequence for a live climate update.

Executed reproduction, three full live ticks:
```
tick=0 climate_time=1 weather_remaining=66469 events=0
tick=1 climate_time=1 weather_remaining=66469 events=0
tick=2 climate_time=1 weather_remaining=66469 events=0
```
Time, season, weather and sleep offset cannot advance normally or resume from real loaded metadata. Required acceptance: real metadata time T/weather remaining R; after three ticks time T+3 and R-3; save/restart resumes those values; sleep offset survives next tick.

### P1: retired player actors remain Active forever and stop world progress after nine joins

`state.rs:466-477` changes session phase/outbox/occupied only. `step.rs:195-200` prunes viewers but commits all residents unchanged. `active_players` uses ActorRecord.lifecycle without consulting session lifecycle. Retired players remain motion/combat/drop/sleep/AI targets, can pick up items, and consume actor capacity. Session records themselves also remain in the map forever, making broadcast and duplicate-player scans unbounded over connection churn.

Executed prepare/install/activate/tick/retire loop 9 times: residents.actors grew 1..9. On ninth full tick `environment.world_time` became 0 because combat rejected >8 player actors and all later world phases silently stopped. Required acceptance: >8 sequential reconnects with at most 8 concurrent sessions, no retired actor participation, bounded tombstones/records, continuing world ticks; similarly prune terminal hostile/passive records and their runtime/mining state once terminal outputs project.

### P1: reducer errors are silently treated as successful ticks and accepted commands can disappear

`step.rs:168` ignores dispatch_rows error; `state.rs:378` always returns Ok(publication) and bumps tick. All drained commands receive applied sequence watermarks before admission (`step.rs:260-277`), so a provider failure stops remaining rows but commands cannot retry and no failure is observable. Concrete saturated path: 2049 accepted OpenContainer commands fit the 4096 command queue, but each defers once for container plus once for crafting; the 4096-effect deferred bag overflows during admission (`state.rs:2144`, `containers.rs:150`, `crafting.rs:1094`). Dispatch stops before any provider rows; all 2049 are already watermarked; advance_tick still succeeds. Production capacity errors and internal invariants must not silently lose accepted work.

Required acceptance: this concrete 2049-open transcript plus a later valid command; prove all acknowledged ownership is either executed, retained with original identity, or has the existing protocol refusal, and that a runtime failure cannot masquerade as success. No invented wire reason.

### P1: no-new-input tick clears retained held controls

`player_motion.rs:253-283` derives held exclusively from latest_deferred in this tick, ignoring existing runtime.controls. With no fresh packet it advances neutral input and writes controls=None. Go `packages/server/sim/entity/tick.go:121-135` updates player.input/miningHeld/eatingHeld only on a new valid input; invalid-input/reset/spawn are the explicit clear paths (`tick.go:104-106`, `player.go:829-835`, `spawn.go:204`). Ordinary ticks keep held controls.

Required acceptance: send one movement/primary/eating control, run several packet-free ticks, compare Go and Rust; then release and invalid-input packets clear according to the oracle. Distinguish absence of new input from an explicit invalid tombstone.

### P1: chunk acquisition does not install any production chunk

`step.rs:307` only calls world_acquisition::run. `rules/world_acquisition.rs:183-202` always returns a zero report. No production call to drain_chunks/apply_drained, no retained ChunkWants, no load/generation workers, and no Ready payload installation. `apply` marks a request consumed but discards its successful Chunk payload. Every Ready world used by gameplay tests was preloaded with harness APIs. Mailbox admitted chunk results consequently fill and never drain.

Required acceptance: real world loader/generator -> bounded result mailbox -> current want/generation validation -> off-tick prepared Ready -> install -> player collision/action -> chunk snapshot; include out-of-order, cancellation, recoverable load, failure/retry, movement subscription and unload. ChunkResult currently lacks revision/persisted revision/recovery/rewrite facts that LoadedValue::Chunk retains.

### P1: readiness and respawn lifecycle are unimplemented

Production runtime has_view writers are only false (`player_motion.rs:369`, survival/default/crafting paths); true occurs only in test code. Eating (`eating.rs:151`) and bow (`projectiles.rs:647`) suspend when !has_view. Login seeds actors directly Active without a Ready spawn check, ignoring Go pending-spawn/unstick/restore safety. Death produces ActorLifecycle::Respawning (`hostile_outcomes.rs:1108`, `player_survival.rs:589`), but no production code transitions Pending/Respawning back to Active. Such actors are permanently excluded from player physics, survival and interest.

Required acceptance: loaded safe/current location and world spawn/bed fallback, unready spawn stays pending, Ready arrival permits activation, death performs reset/loot once then respawns and accepts controls, no immediate re-login activation bypass.

### P1: door/bed interaction ingress is unreachable, and sleep state is not carried

AuthorityState.enqueue_interaction writes its private interactions at `state.rs:649`; TickContext.from_parts creates a separate empty vector at `state.rs:1733`; for_tick copies neither authority.interactions nor a drained interaction batch. `step.rs:299` collects bed entries from that empty context vector and later door loop reads it too. Human OpenContainer is admitted to chest/furnace and crafting paths only; neither derives and enqueues Door/Bed interaction. `Command::Resync` also has no reducer consumer. ResidentTickState (`state.rs:56-68`, resident_snapshot:1879) omits sleep_record and sleeping; each next context initializes them empty, so sleepers/bed records never survive a tick even if directly seeded.

Required acceptance: real v45 OpenContainer rays hit door and bed, session-bound validation and phase ordering match Go; two players enter sleep on different ticks, wakes/disconnect settle correctly, record and offset carry; existing resync produces actual snapshot/refusal.

### P1: production actions are not projected into client events

`step.rs:177-211` publishes only context.events. Rule source emits only tools placement-success, survival damage/reset, hostile outcomes combat confirmations and the projectile event arm; no assembled PlayerState/world mirror, chunk/block streams, remote-player, inventories/crafting/containers, mob/drop/projectile lifecycle or chat/rejection projection exists. `world_mutation::settle_place` does not emit placement-success, although tools does. The codec/Event types already cover all families; passing publication-port tests only proves encoding of manually supplied events.

Required acceptance: actual Memory and TCP clients log in, receive spawn/Ready chunks, make a visible successful and rejected action, see movement/crafting/container/actor/drop/projectile transitions, disconnect/despawn and resync; exact ordered existing-protocol frames match Go. Track subscriber visibility/generations for each lifecycle; no duplicate publication.

### P1: no live-state-to-save snapshot collection exists

`step.rs:200` commits resident maps only. No production caller of remember_dirty, no ActorPersistence implementation, no actor-set load or snapshot extraction/revision owner. SessionRecord.body remains the installed PlayerSave; PlayerMotion changes ActorRecord.motion/look without body.current/yaw/pitch; inventory changes InventoryRecord without body.inventory. Therefore copying ActorBody::Player would still lose movement, selection, items/crafting/armor. Other actor bodies/runtime fields likewise need exact source-specific save mapping. Metadata snapshot returns stale metadata and same sequence.

Required acceptance: actual command/tick -> production snapshot selector -> StoreScheduler/Mailbox/DiskStore -> completion -> restart -> actual login/residents, for players/chunks/companions/hostiles/passives/metadata. No manually injected OwnedSnapshot substitute; preserve recovery/rewrite and partial-ack errors.

### P1: Ready chunk durable revision never advances across production tick commits

`resident_snapshot` carries original ReadyChunk plus accumulated blocks and forever-dirty fixed slot copies. `ReadyChunk::snapshot` at `world.rs:135-144` returns base revision+1 when any overlay write/dirty exists. No commit-back materializes the overlay to a new base or clears per-tick dirty state. Two different future tick saves can therefore both be revision R+1 with different payloads; RegionIo at `store/region_io.rs:169-176` rejects equal revision/different payload as corruption. `ResidentTickState::ready_snapshot` additionally excludes carried overlay/drop/container edits entirely.

Required acceptance: alter one chunk on tick1 and tick2, save after each, verify distinct increasing logical revisions, unchanged tick stays non-dirty, reload second content; container/drop-only edits obey same once-per-chunk-per-tick barrier. This is a verified assembly issue; no production snapshot collector exists yet, so current shipped binary cannot reach these saves.

### P1: internal furnace interest discards dimension

`step.rs:638-645` enumerates ContainerRef from both dimensions, then view.container(reference) resolves Overworld only (`state.rs:1423-1425`). `furnaces::advance` also uses this Overworld getter and Container effect. A Depths furnace alone is skipped; a Depths furnace sharing an Overworld reference can tick the unrelated Overworld furnace. Wire refs are deliberately Overworld-only but internal world container methods already accept dimension.

Required acceptance: matching chunk/slot/generation furnace refs in two dimensions, only Depths interest; only that furnace advances and correct dimension chunk becomes dirty. The internal batch surface needs a dimension-bearing interest value; not a wire schema change.

### P2: snow travel tracker is discarded each tick

`step.rs:432` constructs fresh FootprintSchedule every tick, contradicting `crops.rs:223-225,334-339`; collectors accumulate horizontal travel at `crops.rs:427-446`. Typical movement less than 0.6/tick never reaches snow stride, and same-cell suppression forgets its cell. Trample pending cells can be ephemeral, but snow trackers must carry.

Required acceptance: several grounded 0.2-block live steps cross the 0.6 stride once, next same-cell crossing does not repeat, airborne intervals preserve accumulation; teardown prunes terminal actors.

### P1: late action exhaustion receipts vanish

PlayerPostPhysics consumes charges at `player_survival.rs:775-785`, before combat, tools and mining. Those later providers add ActionKind receipts (`hostile_outcomes` melee, tools till, mining completion). TickContext charges initializes empty and is absent from resident_snapshot, so end-of-tick receipts vanish and never reach the next consumer. Even after fixing Runtime clobber, melee/till/mining produce no exhaustion.

Required acceptance: actual successful action followed by next live tick changes hunger/exhaustion exactly once, failed action charges nothing, multi-actor receipts remain independent, death reset clears its own receipts according to source.

## Production closure map (evidence; controller must own design)

| Boundary | Current source facts | Missing concrete connection / decision evidence |
| --- | --- | --- |
| Metadata startup | DiskStore exposes real Metadata; AuthorityState::try_new creates defaults; no install API | Install verified loaded metadata/durable sequence before ticks; clock/climate commit+sequence policy |
| Player startup | StoredPlayer -> state.install -> login_seed is pure mapping | Source pending/Ready spawn/unstick/bed/safe restoration; retain admitted login view distance |
| Hostile/passive startup | LoadedValue has saves; rule bodies and actor keys exist | Exact saved-set-to-ActorRecord/ActorRuntime mapping; next-id continuity; lifecycle/dirty revision ownership |
| Companion startup | LoadedValue::Companions, CompanionBody, separate inventories/runtime exist | Real resident/task/policy ownership mapping; persisted ID/slot/settings/memory state; actual Agent route into these residents |
| Chunk startup | DiskBackend::load -> RecoveredChunk; Native generation kernels exist | Want union, generation/request ownership, bounded off-tick worker, cancellation/retry, PreparedReady install, subscription diff/unload and rescan enqueue |
| Chunk payload facts | ChunkResult::result is Result<Chunk, ServerError>, ReadyChunk needs generation+revision | Existing completion cannot express recovered/promoted logical revision, persisted_revision, needs_rewrite; accept an exact compile-ready result contract before consumers |
| Player save mapping | ActorRecord pose/look/survival + InventoryRecord + Runtime + PlayerSave safe/respawn/profile fields are distinct | Authoritative extraction of pose/look/slots/armor/hunger/exhaustion/respawn/safe while preserving source lifecycle save rules and revision |
| Chunk save mapping | ReadyChunk base + blocks + DropState + ContainerState | Materialized chunk, once-per-chunk tick revision and dirty touch, snapshot ownership/size estimate, durable ack/unload barrier |
| Actor save mapping | ActorBody durable payloads plus Runtime temporal state and active set | Source-exact saved hostiles/passives/companions, no dead/retired leak, set revision/ID continuation; no fixture-preload path |
| Durability coordination | StoreScheduler/Mailbox/DiskStore implemented | Live authority selects current snapshots; workers execute I/O; completions correlated; actor/metadata shutdown flush; no production FinalReducer/ActorPersistence impl |
| Input/interest | SessionRecord omits login.view_distance, active_keys/projectile scopes fixed radius2 | Preserve declared view distance, source subscription/visibility; current PlayerControl type carries control only, not view semantics |
| Internal interactions | AuthorityInteraction is typed, authority inbox write-only; context inbox always empty | Bounded drain + current session validation; human command ray produces exact Door/Bed kind; ordering and retained ownership |
| Internal furnace | ContainerRef lacks dimension, world_container API carries it | Dimension-bearing internal interest and WorldContainer effect, using accepted source semantics |
| Dynamic projections | Domain Event and ProtocolCodec already support full wire set | ChunkSnapshot/BlockChanges/ForgetChunks; PlayerState; RemotePlayer Spawn/States/Despawn; InventoryState/CraftingState; ChestState/FurnaceState/ContainerClosed; ItemDropUpserts/Removes; Companion/Hostile/Passive Spawn/State/Despawn; Projectile Spawn/State/Despawn; CommandRejected; Chat; placement success |
| Shutdown/final tick | Shutdown machine takes externally provided FinalReducer/ActorPersistence | Concrete once-only final gameplay reducer without publishing; exact snapshot capture of final mutations; bounded workers/store/Agent sequence |
| Failure evidence | ServerError and PhaseReport exist, TickPublication has no provider failure | Decide exact non-loss continuation/carry and failure observation without inventing v45 rejection reason; producer/consumer acceptance |

Shared-contract changes must be reviewed only where these consumers need them (not an invitation to invent a broad framework): chunk completion recovery/revision/prepared data; dimension-bearing internal furnace interest; actual live startup/snapshot/read boundaries and lifecycle ownership; provider fault outcome and intake disposition; projection's bounded immutable visibility snapshot. Resident private carry fixes (sleep, trackers, receipts, runtime preservation) need no public trait change by themselves.

## Why current integration evidence is insufficient

- `tests/server_replay/full_corpus.rs:149-206` checks Go source SHA ranges and fixture file existence. It never executes the named rust_test from each row.
- `full_corpus.rs:264-310` uses `state.admit` without install/activate, so no player actors exist; it submits only CloseContainer and compares two Rust runs to each other.
- Executed `full_corpus::logical_state_and_events_match_across_runs`: **2 ticks, 0 + 0 ordered events**, pass. No Go oracle, gameplay state or save checkpoints participate.
- `tests/local_remote_parity/integration.rs` uses test-only RealEndpoint and immediate loads; transcript action is CloseContainer. This proves adapter parity for that schedule, not complete gameplay.
- `tests/persistence_failure/integration.rs:205-230` manually injects snapshots of all six save families before DiskStore.write; restart confirms storage payloads. No live tick generated those payloads.
- `tick_state` tests preserve maps but deliberately allow saturation loss, omit climate progression and teardown, and use harness Ready seeding.
- Python/MCP tests use actual providers and service, but without real resident companion installation and effect checkpoints the live candidate admission alone is not gameplay acceptance.

## Validation and reproduction

Ran unchanged tests:
```
rustup run 1.97.1 cargo test --manifest-path packages/engine/Cargo.toml -p mornlea_server --test server_replay --locked tick_state -- --nocapture
# 8 passed, 306 filtered
rustup run 1.97.1 cargo test --manifest-path packages/engine/Cargo.toml -p mornlea_server --test server_replay --locked full_corpus::logical_state_and_events_match_across_runs -- --nocapture
# 1 passed, prints two ticks with zero events
```
A temporary standalone Rust executable linked current compiled crate libraries, called AuthorityState::try_new(full limits, seed7), advanced three empty ticks, then ran nine prepare/install(None)/activate/full-tick/retire cycles. No repository sources were edited. Repro output:
```
tick=0 climate_time=1 weather_remaining=66469 events=0
tick=1 climate_time=1 weather_remaining=66469 events=0
tick=2 climate_time=1 weather_remaining=66469 events=0
login=1 actors=1 has_view=false saturation=0 climate_time=1
...
login=8 actors=8 has_view=false saturation=0 climate_time=1
login=9 actors=9 has_view=false saturation=0 climate_time=0
```
Working tree remains clean. No claim of exhaustive provider-local algorithm audit; controller requested consolidation now. Architecture skill: no change.


# F2 independent IO/Agent/transport review

Reviewed baseline: `/Users/chen/work/mornlea-f2-server` HEAD `7ff060609448538412c7d90e71e4bfd29f26ba1e`, against requested dev `365a0339`. Read root/engine/server/store guides, OpenSpec config, proposal/spec/design/tasks and relevant continuation packets/ledger. No repository files edited. Main controller's later planning edits were preserved. This report does not certify full inventory acceptance; remaining production hydration and complete rule integration require the controller's separate review.

## P1: compaction uncertainty loses the parent durability barrier

`packages/engine/crates/mornlea_server/src/store/region_io.rs:340-353`, especially `if let Err(error) = renamed { reopen(); return Err(error) }`, and `sync_parent` failure followed by `reopen` without retained state. `at` invokes Rename After after the actual rename, so an after-hook error means the replacement already happened. Both the after-hook and failed parent-sync cases leave a reopened canonical descriptor with no pending parent barrier. Subsequent `save`/`sync` can acknowledge new bank revisions with only file fsync. A power loss can recover the old directory entry/inode and lose those newer acknowledged revisions. Creation has analogous behavior at 435-438: canonical creation can publish then return before parent durability; next open does not repair it.

Executed public-API repro saved in `/tmp/mornlea_review_io_repro.rs`, executable `/tmp/mornlea_review_io_repro`. It links the existing HEAD build `libmornlea_server-7ed6da85369368df.rlib` and corresponding storage rlib. No test/source edits. Output:

```
compact rename-after error=Err(Io { operation: Replace, kind: Other })
save after failed compaction while parent barrier fails: DiskWriteOutcome { committed: [(Chunk(...), 2)], error: None }
```

Algorithm: distinguish actual rename publication from before-hook failure, always attempt parent barrier once published even when after-hook fails; retain parent-uncertain state until directory fsync and fallible close both pass. Before any later durable ack (including already-known revision, save, sync, close), repair this barrier. On reopening an existing canonical file establish parent durability before acknowledging writes, to repair failed constructor publication. Do not erase uncertainty in refresh/reopen. Regression: save revision 1; compact with Rename After fault; then make every DirectorySync fail and save revision 2. Must have no committed revisions and an explicit directory error until barrier succeeds; retry then ack once. Repeat for DirectorySync Before and failed first creation/reopen. Existing compact_atomic_fault_boundaries checks old contents, not new save after uncertain replacement.

## P1: caller shutdown deadlines do not bound real storage work

`src/store/mailbox.rs:168-215` executes all busy worker slots synchronously; `flush:370-429` invokes this directly after checking deadline only before dispatch. `sync:432-444` and `close:446-458` ignore deadline. `src/store/scheduler.rs` flush likewise directly calls `drive_workers`. Real `DiskStore::{write,sync,close}` performs blocking filesystem calls and has no caller deadline/cancellation input. Slow writes/fsync therefore block shutdown past deadline; a late success can be returned after expiry. The comment claiming synchronous backend cannot block is invalid for DiskStore. A cancellability-sensitive native fsync must not be interrupted by dropping its descriptor, but that does not permit the caller to block past deadline.

Algorithm: put real disk owner in an actual owned bounded worker thread/channel with exclusive lease held there. Queue jobs, sync and close barriers with ownership identities. Caller waits only until remaining absolute deadline; timeout preserves queued/in-flight work, frozen authority and world lease for retry. A worker may continue non-cancellable fsync; late completions remain retained/charged until consumed and applied once. Shutdown close releases lease only after prior work/barriers and fallible close complete. Cap both jobs and bytes across active/completed ownership.

Regression: Condvar-gated backend `write`, `sync`, and `close`, released after 200ms; caller deadline 20ms must return Timeout within bounded tolerance (<100ms). While worker held, second world lease must fail. After gate release retry completes same phase once; no repeated final tick/sync, no lost/duplicate durable ack.

## P1: Agent RPC absolute deadline and cancellation are ineffective

`src/agent/http.rs:739-751` computes remaining duration once and sets per-operation socket timeouts. `read_http_response:798-866` repeatedly reads with that same timeout and no absolute-deadline checkpoint. Slow progress restarts OS timeout on every read, extending RPC arbitrarily. `AgentHttpWire::close:784-786` only flips a flag, not live socket shutdown. `src/agent/lease.rs:532-548` sets cancel, but business worker 485-518 never observes slot.cancel. `freeze:594-596` joins control worker unconditionally, despite no deadline in its API; `close:659-687` ignores its Deadline and joins before closing wire. Thus a dribbling control response can block freeze/close indefinitely.

Executed loopback public-API repro in same temp program: an HTTP peer sends one response-header byte every 10ms. A 40ms absolute deadline returned after **407ms**, `AgentUnavailable/503`; repeated progress could extend to header/body caps times duration. No Python/fake-provider ambiguity: actual TcpStream+AgentHttpWire.

Algorithm: recalculate remaining absolute time before connect/every read/write and checkpoint after each phase; cap timeout to remaining duration (or bounded slices when supporting cancellation). Track active socket owners and shutdown them on per-request cancellation/close. Cancellation must settle terminal ownership before capacity is released, not just mark a flag. freeze must fence outcomes immediately without unbounded join; quiesce/join should have explicit deadline-aware ownership/report and retain handles on timeout. Control/close cannot report success while RPC resources remain live.

Regression: 30-40ms deadline against 10ms dribbled headers/body, total server completion 200ms; RPC exits by deadline+tolerance. Start real control worker with stalled response, freeze/quiesce and close under 20ms caller bound; timeout preserves worker/lease, retry reaps after actual cancellation, no late lease install. A cancel of in-flight business RPC must interrupt active socket, settle once, and not install successful outcome.

## P1: unbounded business map/threads and completion retention

`src/agent/lease.rs:452-518` spawns detached thread per business request and inserts map entry without cap. `poll:521-530` returns clones forever; nothing removes map records anywhere, including close. Normal unique plan/dialogue/memory requests accumulate completed bodies indefinitely. PlanHost's four slots do not bound history, concurrent memory/cancel submissions, or direct provider calls. Cancellation is ineffective as above.

Algorithm: hard accepted bound across active and unconsumed completed requests; reserve before thread/spawn side effect; retained completion count/bytes stay charged. Introduce one-shot completion transfer or explicit retire acknowledgment, and remove settled slot only after consumer acceptance. Keep worker join/socket ownership until completed. Shared provider bounded executor preferred to detached thread per request. Request-ID replay prevention may use bounded/expiring tombstones, not whole response bodies forever.

Consumer adjustment needed: PlanHost::drain_outcomes currently polls repeatably and does not mark the successful slot outcome-queued (`host.rs:814-829`, dialogues 861-873). Multiple drain calls before install enqueue same result repeatedly, filling the four-entry queue; add queued/settled flag or ownership transfer before subsequent polls. Regression: repeatedly complete/drain/poll thousands of unique requests; retained allocations stay bounded, full cap+1 refuses before spawn, one outcome per slot despite repeated drain, later companion not starved.

## P1: unbounded MCP connection workers and early close completion

`src/agent/mcp.rs:2142-2149` spawns a detached thread for every accepted TCP peer before authentication; no capacity or owned join/socket registry. Partial unauthed headers retain one thread each; header timeout is per-byte read (2177 onward), allowing dribble extension. `McpService::close:2118-2127` joins only accept thread; done means accept exited while connection workers may still read/write, retain snapshot clones and sockets. Caller cannot prove resources released.

Algorithm: bound accepted connection slots/retained bytes before spawning and retain join + socket shutdown handles. Absolute request/header/body/write deadlines, not reset per byte. Close stops accept, cancels snapshots, shutdowns all owned sockets, waits with explicit deadline, and only emits clean done once every worker is reaped; timeout retains ownership for retry. Regression: cap+1 partial-header clients, ensure bounded admitted workers; close while clients dribble, all sockets EOF and workers reaped under deadline; no post-close tool publication.

## P1: unknown memory commit prevents every future shutdown attempt

`src/agent/memory.rs:448-478` removes failed commit from commits but retains its reservation. `MemoryFinalizer::begin_attempt:850-869` only cancels current RPCs/records fresh deadline. `drain:875-899` only polls; retained reservation alone always returns Timeout, with no resubmit/reconcile path and unused attempt_deadline. If a commit failed or outcome is unknown before shutdown, healthy future retries cannot make progress without an external gameplay caller, which frozen shutdown excludes.

Algorithm: explicit bounded finalization scheduler for retained reservations and pending delete/reconcile intents under fresh min(30s,caller) context. Reconcile unknown outcomes; if retry needed resubmit exact same operation/base/epoch/summary with fresh request identity. Retain uncertain ownership until exact remote confirmation; no new dialogue/operation. Regression failed-once commit then healthy wire: repeated shutdown attempts automatically reconcile/retry same operation and finish. Existing shutdown_reconcile_fresh_context merely unparks an already in-flight scripted commit.

## P1: completion applies to unrelated in-flight jobs

Cross-boundary issue communicated to core owner: `src/core/state.rs:835-860` drains all self.in_flight regardless of completion.ticket/submitted. Completion A returns every unrelated pending job B as retry/dirty, even though its worker still owns B. It also trusts committed key/revision without checking submitted list. Public minimum regression: select A and B in separate jobs; completion A leaves B's exact records in-flight, settles only A. Validate committed entries against actual request key/revision and ticket ownership, partial ack retains only A's failed remainder. This is separate from filesystem provider durability.

## P1: production MCP capability entropy is not cryptographic

`src/agent/snapshot.rs:837-855` derives bearer capability from SHA256(wall nanos,counter,PID,addresses). SHA256 does not add entropy to predictable process inputs. Go registry defaults to crypto/rand.Reader (`packages/shared/companion/snapshot_registry.go:126-131`, `newIdentity:385-396`). MCP bearer is sole snapshot authorization; use OS CSPRNG/getrandom platform provider. Preserve deterministic entropy injection only for tests and return explicit entropy failure before registration. Request IDs may be correlation-only; the capability must be unguessable. Algorithm acceptance should pin real OS random source and entropy-failure behavior rather than flaky uniqueness statistical tests.

## P1/P2: transport terminal resources never retire

`src/transport/common.rs:609-633` record_close marks phase/closed but retains inbound, codec and connection map forever; no public removal exists. open inserts new IDs after pending reservation released. Repeated malformed/timeout/local connections therefore grow history indefinitely; a maximum retained frame can remain per closed peer. Closed states' replay requirement needs bounded tombstones, not whole buffers. `src/transport/tcp.rs:224-230` EOF returns core ingest without dropping stream, and poll/flush_out don't automatically reap closed sockets after rejects drained. `TcpConn::drain_queue:94-119` ignores all write errors as temporary wait, so BrokenPipe/ConnectionReset never retire session and socket on send-only failure.

Algorithm: on close clear large buffers/codec and remove live connection after pending reject/control delivery; bounded terminal tombstone ledger for replay. TCP reap EOF immediately; malformed rejection retains only until bounded flush/drain deadline, then shutdown socket. drain_queue distinguishes WouldBlock/Interrupted from terminal/WriteZero and propagates fatal state; bound bytes/syscalls per flush, not drain every 512 queued maximal frame. Regression repeated open/malformed/close many times leaves bounded live/tombstone bytes, all EOF/reject sockets gone after close; reset during flush retires session and frees socket; high-volume readable peer cannot make one flush exceed declared byte budget.

## Previous Go binary fixture trust

Existing `/tmp/mornlea-f2-previous-server` SHA256 `282e0483ad0e6fe9df275957e51bc61afae769906e511f8f59dc37efd8fa0477` is a real Go1.26.0 Darwin/arm64 `github.com/channing771/mornlea/packages/server/cmd/mornlea-server` executable according to `go version -m`; it has **no vcs.revision/vcs.modified build provenance**. `/tmp/mornlea-previous-server` has different SHA256 `90b4cd74881f001b2720577c8b0124f94c3f179d676c484b26a70a063229daec` despite same size/time vicinity. Treat both as explicitly named disposable test fixtures only; cannot certify selected release/source identity or compatibility verifier acceptance from them. Rebuild the chosen previous runtime from a recorded approved source, record build/toolchain/native dependency identities and exact executable SHA; invocation/script hash consistency proves same binary, not its provenance. No previous binary launched in this review.


# Independent F2 acceptance / activation review

Reviewed baseline HEAD 7ff06060 versus dev 365a0339 in `/Users/chen/work/mornlea-f2-server`. Repository was clean at review start. Read-only repository review; executable reproductions used existing binaries and isolated temporary trees only. Parent has already begun reconciliation, so original status/ledger contradictions below refer to the review baseline.

## Blocking findings

1. **P1: executable is a lock/control stub, not the promised opt-in server.** `packages/engine/crates/mornlea_server/src/bin/mornlea-server.rs:3-12`, `229-233`: no ticks, no admitted sessions, game TCP port bound as reservation only, control status always final_tick=0. `ledger.md:1459-1464` downgraded mob disk seeding, chunk acquisition streaming, despawn projection and production transport to known limitations solely because default remains Go. This contradicts proposal:11-14, delta spec missing-supported-rule scenario, design capability inventory completion rule, and real integration packet 01:81. Default selection and opt-in completeness are separate gates. Parent has now withdrawn waiver/reopened 3.7; keep 3.8/4 pending until real assembly lands.

2. **P1: compatible rollback accepts corrupt unloaded player files.** `scripts/rust-server-opt-in.sh:1058-1065` checks only manifest schema constants and tree equality; `817-882` then starts previous Go and probes a random new player login/seed. No offline previous-runtime verifier is called although normative `plans/04-refined-nodes.md:236-240` explicitly requires it to read all existing files. Confirmed reproduction with existing `packages/engine/target/cargo/release/mornlea-server` and `/tmp/mornlea-f2-previous-server`: create Go world (seed42), stop Go, add `players/11111111-1111-4111-8111-111111111111.player` containing `corrupt unloaded player`, activate Rust, compatible rollback. Both commands exit0, rollback prints seed42 and phase PreviousRunning, corrupt player file remains. A startup/new-player login cannot qualify old players or distant unopened regions. Required regression: corrupt/future validly named player and region present before activation -> compatible rollback refuses before any previous process/tick; tree unchanged. The hash detects drift only after activation, not pre-existing incompatible data.

3. **P1: restore replaces the lock inode without retaining ownership.** `lock_probe` at `scripts/rust-server-opt-in.sh:204-223` acquires then immediately unlocks/closes. `stop_rust_owner:787-795` and `cmd_rollback:1044` prove FREE only momentarily. `restore_backup_world:954` renames world to retired, then `969` renames staged backup to world; backup carries a different world.lock inode. No held lock spans data verification/staging/renames/start. Deterministic race test: pause rollback before line954, acquire existing world/world.lock with actual Go or flock process, resume rollback; rename retires inode held by competing writer and installs free backup inode, previous Go then acquires new world.lock and serves while other writer is still live. This is precisely the unsafe path that an OS lock cannot protect when its containing directory is replaced. A stable external operation/ownership guard or a lock-preserving world replacement design is needed, with actual Go/Rust competing-owner tests and rollback phase invariants. Merely adding another FREE probe cannot fix this.

4. **P1/P2: unbounded serial control read prevents bounded shutdown.** `src/bin/mornlea-server.rs:305-312` handles one connection synchronously; `328-330` performs `read_line` into growing String without byte cap or socket read timeout. Reproduced with existing rebuilt release binary, valid v6 metadata fixture and minimal manifest: first Unix socket sends `{` without newline; second sends `{"op":"shutdown","deadline_ms":1}\n`; second times out at500ms and process remains live. Closing first client permits progress. A local stalled/oversized client can block all status/shutdown indefinitely and grow memory. Add bounded framing plus per-connection deadline, then real-process tests for withheld newline/oversized request and subsequent timely shutdown. `deadline_ms` is currently only validated, never used to bound work.

5. **P1: accepted 3.7 evidence does not execute full inventory or compare Go parity.** `tests/server_replay/full_corpus.rs:147-203` reads78 rows, recomputes Go source hashes and checks fixture file existence. It never invokes mapped rust_test nor parses expected results. `263-324` runs only two CloseContainer ticks twice in Rust and compares Rust with itself, not Go. `tests/local_remote_parity/integration.rs:70-117` uses ImmediateLoad always Loaded(None), explicitly labels RealEndpoint an executing endpoint double, and the transcript is limited CloseContainer sequencing. `tests/persistence_failure/integration.rs:206-...` manually remember_dirty/stage/write/ack/reopen snapshots; no tick->dirty->scheduler join, and mob families are empty. These are useful component checks but do not prove the complete real-provider/integration inventory required by 3.7/spec. Keep the acceptance node pending. Needed: source-bound expected logical state/events per row/branch; actual assembled endpoint over real save/chunk/Agent owners through Memory/TCP; tick-generated dirty/restart checks, with nonempty actor saves and subscriber events.

6. **P2: inventory still maps stale/nonexistent test names and wrong event evidence.** `testdata/runtime-migration/server/capability-inventory.json` contains78 rows but19 rows have rust_test entries whose final function name is absent from Rust test sources. Examples `drops::delay_40_radius_and_expire6000`, `drops::panel_full_capacity_atomic`, `hostile_outcomes::reservation_mutual_death`, `hostile_outcomes::full_drop_death_no_duplicate`, `session::sorted_9_9_8`, `passives::32_33_and_restore_transient`, `region_io::generation_ties` (actual function generation_ties_and_overflow). Some local_remote_parity::common entries are module filters rather than case mappings. Event rows map despawn/spawn/projection to provider tests which intentionally fabricate no despawn or merely check state: missing production event producers cannot be accepted by renaming mappings. Source hash test only requires mapping strings present. Closure needs exact executing names, positive/failure/boundary records and real integration references per supported entry.

## Additional actionable activation defects

- `scripts/rust-server-opt-in.sh:435-441` confines both paths below run root but only refuses equality. It permits backup nested inside world or world nested inside backup. backup_copy uses copytree source -> destination.tmp inside source; nested backup leads recursive self-copy, failure/large partial tree. Reject intersecting managed trees before filesystem work.
- `tree_hash:122-142` checks files for symlinks but ignores `_dirs`; os.walk doesn't descend directory symlinks, while backup_copy:278 follows them via symlinks=False. A symlink directory can escape disposable root, be omitted from source hash and then be copied into backup. Rust test helper collect_files rejects every symlink, so script and suite disagree. Reject symlink/special directories recursively before hash/copy; test world directory link outside root.
- Fresh adoption of pre-existing backup in activate:565-570 compares hashes without backup_verify; arbitrary identical directory lacking identity is accepted, then restore refuses missing identity. Require named identity before activation or explicitly create/qualify it under ownership.
- Previous restart uses append-only previous-server.log; `start_previous_owner:836-841` accepts first last-match immediately and never refreshes listen while trying login:850-860. An existing prior address can be chosen before new child writes its line, causing retries to target dead predecessor and fail a valid new startup. Reset log or pin current-start offset and refresh until current child's readiness. Test delayed previous exec after an earlier successful rollback.
- `manifest_set_field:181-195` fsyncs staged file but not parent after replace; binary store_manifest:184-193 does neither file nor parent fsync. Durable phase/resume claims exceed actual power/crash durability. Add checked atomic durable replace, preserving last proven phase on failure.
- tests/persistence_failure/activation.rs:566 default_startup_paths_untouched checks git status only, not committed diff vs approved baseline; a committed default-startup change passes. Self-test has same weakness. Scope audit should compare baseline or assert actual entry selection.
- Baseline proposal status said planning only; design review-correction paragraph still says all checkboxes pending; crate AGENTS contains chronological stubs/real integration later claims contradicted by completed nodes. Parent is updating proposal/status. Long489-line guide needs factual current responsibility, not accumulated worker chronology.

## Trustworthy previous-release verifier implementation surface

Implement a test binary in package `packages/server/storage` (not an online host). This enables actual unexported decodeMetadata at metadata.go:111 and existing bounded standalone read helpers. Acquire gofrs/flock on world.lock explicitly, after validating disposable path and requiring existing world.meta; do not call OpenDisk/openWorldFiles to establish read-only proof because world_files.go:31 creates world dir, prepares players dir and writes missing metadata. Use the selected previous release source/hash in verifier report; selected Rust codec acceptance cannot prove previous Go decoder compatibility.

Canonical files from disk.go:878-909:
- world.meta -> existing decodeMetadata
- players/<canonical UUID>.player -> player.Decode(wantID, bytes), disk.go LoadPlayer334/readPlayerFile910; bound max length and propagate read/close errors
- companions.ai -> companion.Decode, companion_codec.go202
- hostile_mobs.bin -> hostile.Decode, hostile_codec.go106
- passive_mobs.bin -> passive.Decode, passive_codec.go93
- dimensions/<dimension>/regions/r.<rx>.<rz>.region -> chunk.OpenRegion(ctx,path,region.RegionKey), chunk/region.go105; Bank() at201 supplies selected bank and entries; Load(ctx,core.ChunkKey) at207 uses actual payload migration/fallback reader. OpenRegion uses O_RDWR but performs no save/compaction/rewrite during load; for strict descriptor read-only qualification use existing injected hooks inside chunk tests or a narrow read-only region opening seam, without duplicating codec semantics. Region format APIs DecodeSuperblock/DecodeRegionBank are existing format readers; iterate every present entry, not merely spawn interest.

Verifier must recursively enumerate canonical sorted existing files; reject path aliases/specials/future schema/corruption/missing metadata; verify world tree before/after ignoring ownership diagnostics; report read_files>0/errors=[]/compatible=true with source_sha/executable_sha256/world_tree_sha256. Missing env ordinary test should decode a fixture, not claim deployment verification. Planned invocation uses MORNLEA_VERIFY_WORLD and MORNLEA_VERIFY_OUTPUT and TestRuntimeMigrationVerifyWorld; fixture is not implemented on baseline. The script needs an explicit verifier executable/hash binding, not PATH discovery.

## Fixture environment for full gates

- MORNLEA_AGENT_PYTHON=/absolute/path/to/packages/agent/.venv/bin/python (actual installed FastAPI/gateway/planner/model SDK/MCP fixture); required by agent_process/process.rs:228 and full_corpus.rs:513, failure not skip.
- MORNLEA_RUST_SERVER_BIN=/absolute/rebuilt/mornlea-server executable (activation.rs:67)
- MORNLEA_PREVIOUS_SERVER_BIN=/absolute/selected/previous-Go-server (activation.rs:75)
- CARGO_TARGET_DIR explicitly when desired; Makefile uses packages/engine/target/cargo but direct Cargo does not inherit.
- MORNLEA_REGION_TEST_PATH/POINT are internal subprocess crash fixture env set by parent tests, not global acceptance setup.
- Script self-test needs both server bin vars; planned new verifier will require explicit bound fixture too.

No full Rust gates were rerun by reviewer; parent owns stage validation. Two actual-process defect reproductions above succeeded. No repository edits or commits were made.
