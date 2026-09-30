# Whole-branch review repairs

**Goal:** Correct verified ownership and lifecycle bugs without weakening complete F2 acceptance.
**Architecture:** Existing state owners retain their records across tick and I/O boundaries. The controller serially integrates any changed declarations; these private corrections require no new parallel shared contract.
**Tech stack:** pinned Rust workspace and existing Go/Python compatibility oracles.
**Spec:** ../specs/rust-authoritative-server/spec.md and ../design.md.

## Global constraints

Protocol v45 and all save schemas remain unchanged. No Go/Python fallback or second writer. Existing user work is preserved. New comments use English and contain no task IDs. All changed hashed/scanned consumers must be enumerated before closure. The controller owns task status, ledger and scoped commits.

## Review focus

Consecutive ticks must preserve sustained actions, climate progression and sleep eligibility. Terminal connections must release large buffers. Slow HTTP headers and bodies must share one absolute deadline. Acceptance must execute production providers and gameplay flows, not infer them from doubles or source identity.

## Player runtime preservation (3.9a)

Baseline: 7ff06060. Prerequisite: accepted ActorRuntime and RuleEffect::Runtime declarations. Editable: src/rules/player_motion.rs, src/rules/player_survival.rs (only visibility of merged_runtime), tests/server_replay/player_motion.rs, tests/server_replay/tick_state.rs under packages/engine/crates/mornlea_server. Read-only: src/core/contracts.rs, src/rules/eating.rs, src/rules/projectiles.rs, src/core/login_seed.rs.

Deliverable: movement preserves all sibling runtime lanes while staging latest validated held controls. Input/output signatures remain `run(&mut TickContext, RuleCall) -> Result<PhaseReport, ServerError>`.

Write `motion_preserves_sibling_runtime` first: preload an Active player and Runtime with distinct nonzero cooldowns, oxygen, peak_y, hunger counters, eating and bow progress, and Player aux respawn/workbench; execute one PlayerMotion call and assert the complete record equals its preimage except controls. The baseline zeroes these fields. Also strengthen the real-login test to assert saturation stays nonzero after two live ticks.

Implement by cloning `ctx.read().runtime(actor)` when present and assigning `controls = held`. Expose the existing survival initializer as `pub(crate) fn merged_runtime` and call it before mutation. It preserves staged runtime or initializes exact saved saturation/exhaustion and respawn defaults. No-new-input retains its `controls`; an invalid latest packet clears controls. The controls lane changes only when this tick contains an envelope. Never overwrite sibling lanes based on pose alone.

Run `rustup run 1.97.1 cargo test --manifest-path packages/engine/Cargo.toml -p mornlea_server --test server_replay --locked player_motion` and the `tick_state` filter. Require nonempty discovery and red/green for the new cases. Run crate fmt/clippy before scoped commit `fix(server): preserve player runtime through movement`. Exclude contracts, persistence, transport and runtime composition. Controller integrates and may revert this unit independently.

## Environment and sleep continuity (3.9b)

Baseline: accepted previous repair SHA. Editable: src/core/state.rs and tests/server_replay/tick_state.rs; serial controller owns these files. Existing EnvironmentState, SleepState and resident staging remain the input/output records; no public boundary consumers change.

Write live-tick regressions first: seed world_time=0/weather_remaining=100, advance twice, require world_time=2/weather_remaining=98. Seed sleep anchors plus one sleeping session into a resident snapshot; after a live tick with a second awake session, require retained anchors/eligibility. Baseline clock remains 1, weather resets, and sleep state disappears.

Carry `pub sleep_record: Option<SleepState>` and `pub sleeping: BTreeSet<SessionKey>` in ResidentTickState and its snapshot/for_tick round trip; Default stays a neutral None/empty set, for_tick retains its existing empty SleepState initializer when None. At tick-start freeze clone committed environment when present, updating only executing tick/checked tunables; metadata initializes the first snapshot. Do not reset seed/difficulty or reinterpret display offsets. Durable metadata assembly remains a separate serial node: current infallible reducer cannot safely report metadata revision exhaustion and completion can regress its revision; that fallible boundary must be accepted before metadata dirtying lands. Existing fixture constructors remain neutral. Run `server_replay tick_state`, `server_replay sleep`, and `server_replay environment`; scoped commit `fix(server): retain climate and sleep across ticks`. Final production persistence remains a separate integration obligation.

## Terminal transport ownership (3.9c)

Baseline: controller-recorded repair predecessor. Editable: src/transport/common.rs, src/transport/tcp.rs, tests/local_remote_parity/common.rs, tests/local_remote_parity/tcp.rs. Existing TransportAuthority stays read-only. A separate repair packet will freeze terminal replay retention and its exact negative cases before implementation; this node is not dispatchable until that packet lands.

## Agent deadlines and ownership (3.9d)

Baseline: controller-recorded repair predecessor. Editable: src/agent/http.rs, src/agent/lease.rs, associated tests/server_contract/agent_http.rs and agent_lease.rs. Existing AgentHandle remains read-only. A separate repair packet will freeze absolute deadline/cancellation and bounded admission cases before implementation; this node is not dispatchable until that packet lands.

## Acceptance blockers outside the bounded fixes

Production endpoint/worker composition, chunk acquisition, durable player/world/entity snapshots, entity and terrain publication, actual Go-versus-Rust replay and rollback verification remain open under 3.7/3.8. Initial independent evidence is retained in ../review.md; each deliverable must receive exact design and independently verifiable cases before dispatch. No placeholder or limitation may be checked off as complete. Full stage gates and architecture skill promotion are owned by 4.1/4.2.

## Region parent durability (3.9e)

Baseline 7ff06060; independent private owner. Editable: src/store/region_io.rs and tests/persistence_failure/region_io.rs only. Read-only DiskIo, IoCancellation and existing disk/atomic-file contracts. No shared type changes or version bump.

Write a regression: save revision1, compact with Rename After failure, then make DirectorySync fail and save revision2. Require error and empty committed; baseline acknowledges2. Repeat DirectorySync Before and first-file creation After/reopen. Use existing real RegionIo with narrowly injected DiskIo hooks, not a fake store.

Implement a retained `parent_uncertain: bool`: set before any actual rename publication; clear only after parent fsync AND fallible parent close succeed. Distinguish before-hook, actual rename, after-hook result; a published rename attempts parent barrier even if after-hook fails, returning the first original failure. Failed barrier remains pending across refresh/reopen. Establish parent barrier on reopening a canonical file too, so a failed constructor cannot lose the recovery obligation. Before durable acknowledgment (including an already-known revision), save/sync/close must repair pending barrier; load may remain read-only. Never acknowledge after a failed barrier, never erase the old bank allocation uncertainty.

Run `rustup run 1.97.1 cargo test --manifest-path packages/engine/Cargo.toml -p mornlea_server --test persistence_failure --locked region_io` and recovery filter. Red/green each new case; fmt/clippy. Controller owns integration and scoped `fix(server): retain region replacement durability barriers`; rollback only these two files. Exclude backend scheduler/threading, contracts, activation and state owner.

## Snapshot entropy (3.9f)

Editable: src/agent/snapshot.rs, tests/server_contract/agent_snapshot.rs, crate Cargo.toml and workspace Cargo.lock (only adding the already-locked getrandom0.3.4 dependency). Read-only SnapshotEntropy/registry declarations; no capability format change.

Add a source-bound regression that production SystemEntropy calls the OS RNG and has no time/PID/address-derived fallback, plus existing entropy-failure registration case (failure leaves zero records). Baseline source assertion fails. Preserve deterministic test entropy injection. Implement `getrandom::fill(out)` with explicit stable ServerError on failure; retain API `SystemEntropy::new/default` as a unit value. No uniqueness/statistical assertions. Compile against cached getrandom API; no toolchain/library upgrades. Run `server_contract agent_snapshot`, actual `agent_process` using explicit Python fixture, fmt/clippy. Controller scoped commit `fix(server): use system entropy for snapshot capabilities`; rollback dependency and provider as one unit.

## Bounded activation control reads (3.9h)

Editable: src/bin/mornlea-server.rs and tests/persistence_failure/activation.rs only. Baseline7ff06060. Existing flag/manifest/control JSON vocabulary stays unchanged. This repairs the control qualifier; it does not accept the absent gameplay runtime. Main owns script/verifier/runtime separately.

Write real-process regressions first: activate disposable world, one Unix client sends `{` without newline and stays open; another sends shutdown and must progress within500ms. Another sends4097 bytes without newline and receives `malformed` or `request_too_large`, followed by a successful status/shutdown. A third dribbles bytes every20ms for400ms; its connection must be retired within200ms plus scheduling tolerance. Missing fixtures fail, no skips.

Use a private `ControlSocket: Read + Write` trait with timeout setters, implemented for UnixStream and platform TcpStream. Each connection gets one absolute `Instant::now()+200ms` deadline. Read into bounded4097-byte storage, before every socket read set timeout to remaining absolute duration; EOF/no newline/oversize/timeouts refuse and return to accept loop. Set write timeout to remaining connection deadline before replies; do not retain unbounded BufReader String. Each connection handles one newline-framed request. Successful valid status/shutdown use the existing semantic handler. No parallel detached threads or foreground windows.

Run `MORNLEA_PREVIOUS_SERVER_BIN=/tmp/mornlea-f2-reviewed-previous-server rustup run 1.97.1 cargo test --manifest-path packages/engine/Cargo.toml -p mornlea_server --test persistence_failure --locked activation` (previous fixture is controller-rebuilt current compatible Go source; executable identity recorded before final release acceptance). Require RED on stalls/oversize, GREEN focused activation, owned fmt/clippy. Controller owns integration and scoped commit `fix(server): bound activation control requests`; rollback these two files only.


## HTTP absolute deadline (3.9d1)

Controller owns src/agent/http.rs and tests/server_contract/agent_http.rs only, after 3788d7aa. Existing AgentWire interface is unchanged. This repairs total RPC deadline and wire close; per-request cancellation and lease worker capacity remain 3.9d2.

Write real raw-loopback cases: drip response header bytes every20ms for400ms under an80ms RPC deadline; repeat valid complete headers with a slow JSON body. Both must return typed AgentRpc Timeout within250ms with one connection and no success. A blocked response with a2s deadline must exit within200ms after wire.close. Baseline exceeds deadline or ignores close. Server fixtures stop on BrokenPipe/ConnectionReset and are joined, without unbounded fixture waits.

Introduce a private RpcIo borrowing the wire and socket with the one caller deadline. Before every read/write attempt check closed and remaining clock duration; rearm socket timeout to min(remaining,20ms). TimedOut/WouldBlock loops recheck the absolute deadline/closed; other I/O errors stay unavailable. Partial writes explicitly advance the bounded outbound slice; zero write refuses. Connect uses remaining deadline and is never retried. Header/body parsing uses RpcIo::read, preserving current byte ceilings/schema/echo checks. Check the deadline and closed flag again before publishing a decoded response. No request-specific cancellation signature or extra worker is invented.

Run server_contract agent_http, actual agent_process with explicit Python interpreter, owned fmt and all-target clippy. Controller scoped commit fix(server): enforce absolute Agent HTTP deadlines. Rollback only these two files; no registry/lease/MCP/store overlap.


## Common terminal connection retention (3.9c1)

Baseline3c87d1da. Editable src/transport/common.rs and tests/local_remote_parity/common.rs only. TransportAuthority and TCP/Memory adapters stay read-only. Existing methods keep signatures; new read-only public `retained_connections(&self)->usize` reports the map size for ownership evidence.

Internal diagnostic policy: retain the newest64 terminal connections in closure order. Older handles return the existing unknown_connection terminal diagnostic. Closed connections retain only small queued handshake/control frames and final progress; they release inbound capacity and ProtocolCodec immediately. They never reserve prelogin capacity. Active/prelogin records are never evicted to make terminal room. Queued rejection remains drainable until its terminal diagnostic is evicted. This replaces the unbounded historical replay promise; internal Rust diagnostic retention is not a protocol/save change.

First write regression creating more than128 truncated/malformed peers serially: buffer a bounded incomplete frame, close, require retained_len=Some(0), live map<=64 (plus separately open active/prelogin records), latest poll/ack repeats exact terminal progress, oldest is evicted to existing unknown diagnostic, newest queued rejection still drains. Preserve existing cancellation/send-handoff once-only assertions. Baseline retains inbound bytes and all records. Update existing test that intentionally pinned retained refused prefix to require release instead; no source wire expectations weaken.

Change Connection.codec to Option<ProtocolCodec>; new wraps Some and live queue_packet obtains its mutable Some or stable Internal on invalid lifecycle. record_close releases reservation, replaces inbound with Vec::new (drops capacity), takes codec, records final progress, appends id once to closed_order VecDeque. While closed_order.len()>64 pop oldest and remove only its Closed record. Guard already-closed record against duplicate ledger insertion. Expose retained_connections solely as map count. No deadline, publication or endpoint changes. Run local_remote_parity common, full local_remote_parity, owned fmt/clippy; controller scoped commit fix(server): bound terminal connection retention. Isolated implementer owns these two files and controller integrates/rolls back them together.

## Save completion ownership (3.9i)

Controller direct owner: src/core/state.rs and tests/persistence_failure/integration.rs only. Existing SaveAuthority/SaveCompletion/AckReport signatures stay unchanged; scheduler doubles are read-only references. Baseline3c87d1da plus accepted continuity repair.

Write real AuthorityState regression: select Hostiles1 as jobA, Passives2 as jobB. A submitted/committed only Hostiles1 must ack1/release0/retryempty and retain B inflight; B later independently acks1. Add partial A with unrelated B, same-key newer revision, foreign committed tuple, missing committed without error, metadata ack cannot regress current revision. Baseline drains B and creates duplicated dirty retries. Use existing real-state snapshot helpers, not AuthorityDouble.

Correlate against the owned selected records by submitted key/revision and completion.snapshots exact identities; only those held records may settle. Metadata is the existing explicit exception: the scheduler submits metadata_snapshot directly without select, so structurally matched metadata snapshot/echo settles independently even without an authority in_flight token. Validate every committed tuple belongs to submitted owned snapshots; malformed membership reports Internal save completion identity and leaves all authority ownership intact. Submitted but uncommitted payload copies transfer into AckReport.retry; retain the authority in_flight accounting token and its immutable preimage until retry acknowledgment or return_dirty, and never clone into dirty. This is the scheduler's existing backoff contract and keeps a retry completion correlatable. Add explicit Internal incomplete save completion when omitted commits have no provider error. Preserve unrelated held records. Successful metadata acknowledgment must never overwrite metadata_sequence with an older revision (durable/current revision are distinct; no current revision mutation on ack). Existing Dirty population/urgency/metadata assembly remains a separate integration node.

Run persistence_failure integration, mailbox and scheduler filters, owned fmt/clippy; scoped fix(server): correlate save completion ownership. No ticket-map invention: scheduler already owns tickets and authority owns selected key/revision records. Main integrates and rollback is these two files.


## TCP socket retirement and send work (3.9c2)

Prerequisite accepted common terminal-retention SHA recorded in ledger. Editable src/transport/tcp.rs and tests/local_remote_parity/tcp.rs only. common.rs/Memory/admission/publication remain read-only. Existing transport signatures stay unchanged; add read-only `pub fn retained_sockets(&self)->usize` for real socket ownership evidence.

Terminal policy: EOF immediately drops/shuts socket after preserving core diagnostic. Data/poll yielding Closed performs one bounded best-effort flush of existing rejection/control frames, then drops the socket whether fully sent or blocked. No terminal grace worker or unbounded wait is introduced. Explicit close and fatal send failure retire via the same core close lane once. Core terminal diagnostic retention stays64.

Refactor private drain into a Write-testable helper over queue/head returning sent_core plus optional fatal io::Error. At most64 write attempts and1MiB bytes per call, slicing each write to the remaining byte budget; keep partial head offset for next call. WouldBlock stops normally, Interrupted consumes one attempt then retries, all other errors and WriteZero are fatal. Successful full Core frames count for ack exactly once; publication frames do not. flush_out acknowledges any fully sent Core frames then retires/drops on fatal; closed progress always drops after this best-effort drain. forward_outbox similarly handles fatal drain and acknowledges fully sent core frames before retirement, returning the actual I/O error. Its existing packet-id ownership API is excluded and remains a separate projection integration blocker.

Tests first: real peer sends partial hello then EOF, pump returns Closed and retained_sockets0; wrong-version reject still reaches an ordinary readable peer and socket retires without explicit close; poll hello expiry drops socket; private helper finite recorder drains exactly1MiB from a2MiB head and retains correct offset, Interrupted writer consumes at most64 attempts, BrokenPipe/WriteZero propagate fatal, preceding complete Core frame ack count survives a later error. Private helper declarations may land compile-only before behavioral RED; record separately. Existing actual reset/cancel/handoff tests remain green. No foreground game window.

Run local_remote_parity tcp and full parity, lib unit tests for helper, owned fmt/all-target clippy. Controller integrates and scoped commit fix(server): reap terminal TCP sockets and bound sends; rollback only these two files. No new shared contract or protocol/save change.


## Survival damage/fall/coordinate correction (3.9p1)

Baselinef01b81c7. Editable src/rules/player_survival.rs, src/rules/player_motion.rs (fluid_upper only), tests/server_replay/player_survival.rs, tests/server_replay/player_motion.rs only. Existing contract records/helper signatures stay unchanged. Read-only Go sim/entity/player.go applyDamage/applyFallDamage/StepPlayers, physics/collision.go and accepted motion/survival staging. Other sprint/sneak/snow/input/reset/bow-wrap discrepancies stay separate; do not silently bundle them.

Tests before code: post-physics landed-body-fluid with peak14/land10 preserves health20 and clears peak10; positive starvation/drowning/fall damage interrupts bow without arrow debit/fire, safe fall preserves bow; peak4.1/land0.1 source f32 subtraction yields floor4 and health19, versus current f64 subtraction yielding no damage. Real pre-step grounded+post-step grounded must not apply fall again, actual airborne→ground applies once; start-grounded/body-fluid resets peak before step then endpoint fluid suppresses landing damage. Both private fluid_upper paths at i32::MIN must refuse actor with unchanged overlay instead of panic; include boundary+near-boundary positive case.

Implementation exact: add bow Option<BowProgress> to private SurvivalWork, copy runtime.bow on load, clear it only after apply_damage positive guard, and project work.bow back in write_back. This keeps sibling lanes unchanged on damage-free calls. For post-physics peak: start from runtime.peak_y; when a pre_step snapshot exists, grounded or pre-body-fluid resets the baseline to pre_y. Endpoint body-fluid resets it to current_y before fall. Apply fall only when current grounded and pre snapshot is absent or airborne (harness fallback preserves existing direct-provider contract). Compute source f32 subtraction first, then convert that difference to f64 for floor; narrow nonnegative floor with Rust's bounded cast and saturating_sub(3), clamp at0, avoiding subtraction overflow without changing representable normal damage. Grounded/body-fluid final peak=current_y, otherwise max(baseline,current_y). Existing nonfinite peak refusal remains before effects.

Change both fluid_upper helpers to checked_ceil(maximum)?.checked_sub(1).ok_or(ServerError::InvalidInput{field:"actor"})?.max(lower). This explicit unrepresentable-prism refusal preserves geometry ownership and leaves no effects, avoiding a wrapped enormous scan. No Go protocol/save format changes. Update only comments naming changed ownership/semantics, English and no task IDs.

Run server_replay player_survival/player_motion and combined tick_state, owned fmt/clippy. Require nonempty discovery and concrete RED/GREEN for all new semantic cases. Controller integrates/scoped fix(server): align survival damage and fall boundaries; rollback these four files together. No state/step/contracts/Agent/storage edits.

## Player intake ownership (3.9p2)

Prerequisite accepted survival correction. Editable src/rules/player_motion.rs, tests/server_replay/player_motion.rs and tests/server_replay/projectiles.rs only. Existing ActorRuntime, ActorRecord and RuleEffect::Compound/Mining interfaces are read-only. No new shared type or protocol/save change. Controller serially owns intake semantics and integration; the reused isolated implementer owns these three files.

The source ApplyPlayerCommands commits held controls and normalized look during intake, before Eating/BowDraw. Preserve that order: after the existing active-actor and stale/duplicate gates, reserve the existing deferred envelope first. Load merged_runtime from the actor. For valid input, set runtime.controls=Some(control), update a cloned actor's look through LookAngles::try_new(normalize_yaw(yaw),pitch), and atomically stage Actor+Runtime. Preserve every other lane and pose; do not clear progress on a valid release, since BowDraw owns firing. For invalid latest input, set controls=None, eating=None, bow=None and atomically stage Runtime+Mining{actor,progress:None}, then return the existing InvalidInput control error. A tombstone is admitted control cleanup, not a world mutation or an ordinary refusal with unchanged transients. Stale/duplicate input remains a zero-effect report before any cleanup. Missing/non-active actor handling stays a separate lifecycle node; do not invent an absent actor.

Tests before code: intake valid primary immediately advances BowDraw before Motion; same-tick valid release fires one arrow using the newly normalized yaw/pitch without running Motion first; invalid move_x2 interrupts an existing20-tick bow, eating and mining before BowDraw, with ammunition/wear/projectiles unchanged; an invalid packet followed by a valid release in the same tick cannot resurrect the draw. Stale invalid and tied invalid packets cannot interrupt the earlier winner. Assert the complete runtime record for fresh/neutral/invalid input, revising the previous invalid preservation expectation only for controls/eating/bow and clearing mining. Add malformed-shape/no-session tests with unchanged state. Existing held continuation and finite/coordinate refusals stay green.

Run server_replay player_motion, projectiles, eating, tick_state and phase_order filters, owned fmt/all-target clippy. Require observed RED/GREEN for early control/look and rejected-release cases. Scoped fix(server): commit player controls before action advancement; rollback these three files only. No step/state/tunables/storage/Agent edits.

## Effective sprint and reset physics (3.9p3)

Prerequisite accepted intake SHA. Editable src/rules/player_motion.rs, player_survival.rs, src/core/step.rs and matching tests/server_replay/player_motion.rs, player_survival.rs, tick_state.rs only. Contract records remain read-only. Main freezes this private correction; no consumer may add a second raw-control owner.

Keep raw held controls unchanged by hunger/sneak gates. Oxygen writes held_control directly, and PostPhysics writes held directly; remove the private suppress_sprint_control constructor and unused imports. Motion derives its local sprinting as raw sprinting && record.survival.hunger()>=6 && !sneaking before sweep and NativePhysics. PostPhysics snapshots that same effective predicate from held and work.hunger BEFORE draining any charges, then applies jump/swim/sprint charges without reevaluating hunger after a debit. The existing source order is jump then swim then sprint; maintain it. Main will separately qualify late interaction charges and earn-time ordering; do not change those here.

After Eating/BowDraw in the reducer, skip oxygen/motion/post-physics when runtime.reset is true, matching source reset short-circuit; regen/eating/bow still run. Also make a direct Motion call with reset=true preserve exact actor pose/look and sibling runtime, retaining newly resolved raw controls without calling geometry/kernel. No teleport/unstick/spawn policy is introduced by this node.

RED cases: hunger5 with a fresh sprint packet produces ordinary walk rather than5.59; raw sprint survives hunger5 and resumes after staged hunger6 without a fresh packet; sneak suppresses sprint speed and exhaustion while retaining both intent bits; jump with hunger6/saturation0/exhaustion3970 adds50 then80, leaving hunger5/exhaustion100 rather than20. Active reset actor velocity/position remain exact and live tick leaves oxygen/peak/exhaustion untouched while regen still advances. Full-record comparisons must preserve sibling lanes and valid release semantics.

Run server_replay player_motion/player_survival/tick_state/phase_order, owned fmt/all-target clippy. Scoped fix(server): derive sprint gates without losing held intent. Controller integration/rollback covers the six files as one node. Exclude safe-location, below-world reset, pending/respawn activation and whole-tick failure policy.

## Sneak edge and snow physics entry (3.9p4)

Prerequisite effective-control repair. Editable src/rules/player_motion.rs and tests/server_replay/player_motion.rs only; NativePhysics, PhysicsTuning, collision_cell and AuthorityReadView declarations are read-only. No new numerical kernel or ABI.

Before sweep/kernel, derive local HeldStep. For grounded, dry, sneaking, non-jump, nonzero horizontal intent, compute direction with existing movement_target at speed1 and yaw sin/cos; probe position+direction*(HALF_WIDTH+0.05); side=[-direction.z,0,direction.x]*HALF_WIDTH. FootY=checked_floor(position.y-GROUND_PROBE). Sample at most two forward feet (probe+side then probe-side), using checked floors and the existing collision_cell mapping: an unavailable observation or any nonzero box count counts as support. If both loaded feet have no boxes, set only local move_x/move_z=0. Preserve raw held control and external velocity; no position clamp or knockback cancellation.

Then, only grounded with effective horizontal intent nonzero, sample floor(position) once. Loaded raw snow layer87/88 scales local tuning.walk_speed*=0.7;85/86, unavailable, airborne and no-input keep original. Use the same modified tuning in sweep and NativePhysics; no snow reduction on the immutable snapshot itself. Source helpers are packages/shared/physics/sneak_edge.go and snow_slowdown.go.

RED provider case: support only(0,0,0), grounded x0.8,y1,z0.5, move_x1/sneak/no jump cannot advance beyond edge; compare support/unloaded/jump/fluid/airborne, diagonal+yaw and carried external velocity cases against source helper expectations. Snow tier3/4 source walk4.3 produces3.01, tiers1/2 remain4.3; sprint target5.59 becomes3.913 with source f32 rounding. Airborne/no-input/unavailable and cross-dimension cases must remain unchanged. Coordinate refusal preflights before any staging.

Run server_replay player_motion/tick_state, owned fmt/all-target clippy. Controller scoped fix(server): restore sneak support and snow speed gates; rollback these two files together. Footprint accumulation and terrain mutation remain separate existing blockers.

## Tuning snapshot contract (3.9t0)

Controller direct owner: src/core/contracts.rs and tests/server_contract/world_read.rs only, baseline1c94164f. Read-only existing RuleTunables constructor/defaults and three provider consumers. Add getters returning stored values exactly: regen_delay_ticks()->u32, regen_interval_ticks()->u32, drown_interval_ticks()->u32, starvation_interval_ticks()->u32, regen_hunger_threshold()->u8, eating_ticks()->u16, furnace_burn_ticks()->u16, furnace_smelt_ticks()->u8. Constructor/ceilings/defaults do not change. These are internal immutable snapshot accessors, no ABI/wire/save impact.

Test the exact default tuple100/40/20/80/18/32/1600/200 and a non-default tuple11/7/3/5/17/9/91/23 through the real constructor; zero-valued retained lanes stay faithfully observable, without deciding provider fallback policy here. A missing-method compile failure is contract RED, not behavioral provider evidence. Implement minimal accessors and ownership comment; compile all consumers, run server_contract world_read and all-target clippy/fmt. Scoped fix(server): expose complete rule timing snapshots. Record accepted contract SHA before separate survival/eating/furnace consumer repair dispatch. No provider is accepted by these getter tests; main owns subsequent exact consumption packets.
