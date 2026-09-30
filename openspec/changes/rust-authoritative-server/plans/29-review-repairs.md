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

Baseline: 7ff06060. Prerequisite: accepted ActorRuntime and RuleEffect::Runtime declarations. Editable: src/rules/player_motion.rs, tests/server_replay/player_motion.rs, tests/server_replay/tick_state.rs under packages/engine/crates/mornlea_server. Read-only: src/core/contracts.rs, src/rules/player_survival.rs, src/rules/eating.rs, src/rules/projectiles.rs, src/core/login_seed.rs.

Deliverable: movement preserves all sibling runtime lanes while staging latest validated held controls. Input/output signatures remain `run(&mut TickContext, RuleCall) -> Result<PhaseReport, ServerError>`.

Write `motion_preserves_sibling_runtime` first: preload an Active player and Runtime with distinct nonzero cooldowns, oxygen, peak_y, hunger counters, eating and bow progress, and Player aux respawn/workbench; execute one PlayerMotion call and assert the complete record equals its preimage except controls. The baseline zeroes these fields. Also strengthen the real-login test to assert saturation stays nonzero after two live ticks.

Implement by cloning `ctx.read().runtime(actor)` when present and assigning `controls = held`. For the first motion with no runtime, initialize from the player body: saved saturation/exhaustion and respawn, current oxygen/height, neutral transient counters. Reuse the existing survival runtime initializer if exported; otherwise keep the exact fallback currently prescribed by that provider. Never overwrite sibling lanes based on pose alone.

Run `rustup run 1.97.1 cargo test --manifest-path packages/engine/Cargo.toml -p mornlea_server --test server_replay --locked player_motion` and the `tick_state` filter. Require nonempty discovery and red/green for the new cases. Run crate fmt/clippy before scoped commit `fix(server): preserve player runtime through movement`. Exclude contracts, persistence, transport and runtime composition. Controller integrates and may revert this unit independently.

## Environment and sleep continuity (3.9b)

Baseline: accepted previous repair SHA. Editable: src/core/state.rs and tests/server_replay/tick_state.rs; serial controller owns these files. Existing EnvironmentState, SleepState and resident staging remain the input/output records; no public boundary consumers change.

Write live-tick regressions first: seed world_time=0/weather_remaining=100, advance twice, require world_time=2/weather_remaining=98 and metadata snapshot matching committed environment. Seed sleep anchors plus one sleeping session into a resident snapshot; after a live tick with a second awake session, require retained anchors/eligibility. Baseline clock remains 1, weather resets, and sleep state disappears.

Carry sleep record/ordered sleeping sessions in ResidentTickState and its snapshot/for_tick round trip. At tick-start freeze clone committed environment when present, updating only executing tick/checked tunables; metadata initializes the first snapshot. Commit environment fields back into metadata with the same source conversion and a dirty revision only when durable fields change. Do not reset seed/difficulty or reinterpret display offsets. Existing fixture constructors remain neutral. Run `server_replay tick_state`, `server_replay sleep`, and `server_replay environment`; scoped commit `fix(server): retain climate and sleep across ticks`. Final production persistence remains a separate integration obligation.

## Terminal transport ownership (3.9c)

Baseline: controller-recorded repair predecessor. Editable: src/transport/common.rs, src/transport/tcp.rs, tests/local_remote_parity/common.rs, tests/local_remote_parity/tcp.rs. Existing TransportAuthority stays read-only. A separate repair packet will freeze terminal replay retention and its exact negative cases before implementation; this node is not dispatchable until that packet lands.

## Agent deadlines and ownership (3.9d)

Baseline: controller-recorded repair predecessor. Editable: src/agent/http.rs, src/agent/lease.rs, associated tests/server_contract/agent_http.rs and agent_lease.rs. Existing AgentHandle remains read-only. A separate repair packet will freeze absolute deadline/cancellation and bounded admission cases before implementation; this node is not dispatchable until that packet lands.

## Acceptance blockers outside the bounded fixes

Production endpoint/worker composition, chunk acquisition, durable player/world/entity snapshots, entity and terrain publication, actual Go-versus-Rust replay and rollback verification remain open under 3.7/3.8. Read-only source extraction is in progress; each deliverable must receive exact design and independently verifiable cases before dispatch. No placeholder or limitation may be checked off as complete. Full stage gates and architecture skill promotion are owned by 4.1/4.2.
