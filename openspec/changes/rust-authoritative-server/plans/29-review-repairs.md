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

Production endpoint/worker composition, chunk acquisition, durable player/world/entity snapshots, entity and terrain publication, actual Go-versus-Rust replay and rollback verification remain open under 3.7/3.8. Read-only source extraction is in progress; each deliverable must receive exact design and independently verifiable cases before dispatch. No placeholder or limitation may be checked off as complete. Full stage gates and architecture skill promotion are owned by 4.1/4.2.

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
