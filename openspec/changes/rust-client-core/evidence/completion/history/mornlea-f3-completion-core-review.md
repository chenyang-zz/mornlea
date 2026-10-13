What this change does: F3 moves client transport/session, confirmed observations, input admission, prediction, CPU terrain preparation and ten semantic presentation families into one Rust core. The repaired live composition and TCP retirement run, but this exact candidate still publishes stale authoritative values and exceeds fixed retained actor limits.

Verdict: fix findings 1 and 2 before merge or full F3 acceptance. This is an independent read-only core review, not native/provider acceptance.

## Identity and scope

- Reviewer: independent OpenAI Codex subagent `/root/review_core`.
- Exact source: `24f57d73c1e1c2132aa5ad3055767fec2628ca35`.
- Original batch base: `7624e51a7e15e7cdf2c46a24db340ef9c962eacf`; resolved and verified ancestor with exit 0.
- Accepted C1/C2 landing: `c31a7cdc36f7a5c2c75739acd7f1dc8f4cbf797e`, recorded in the change ledger. Later repairs are part of the candidate, not assumed accepted from historical reviews.
- Detached clean worktree: `/private/tmp/mornlea-f3-completion-core-review`. No tracked product/test/planning file changed. Final HEAD and clean status rechecked after tests.
- Read root and engine/core scoped guides, OpenSpec config, proposal, both delta specs, design/tasks, qualification packet17, C1/C2 packets00/02/07, inventory/ledger and historical full review as context.
- Inventoried the whole core source/test tree and compared the full batch scope, not only the last TCP commit. Traced contracts/export/config; input validate/reserve/commit/ack/reject; login and public endpoint; Memory/TCP workers and close/bind/drop ordering; mirror/world staging and source attribution; lifecycle invalidation; prediction/replay/targets/snow; preparation queue/native/near/LOD; complete ten-family composition, family assemblers, frame validation/accounting and resource retry/retirement. Reviewed actor, world UI and inventory projection interfaces and their source mappings. Inspected registered test targets, the live/mirror/I/O/lifecycle/prediction/terrain/publication regressions and all executed names/results. Generated registry literals were checked through their owner and actual native tests rather than manually verifying every literal. This is not a claim of exhaustive line-by-line reading of 61,255 added source/test lines.
- Traced the real consumer in `mornlea_godot/src/client_core.rs`: CoreArena stores actual ClientCore behind ClientEndpoint; connect/submit/step/snapshot/reset/close use that same owner; pull renders a complete owned frame; failed close keeps its slot/token. Native Godot conversion and Python host implementation are assigned to other reviewers/controller.
- Assumed expected load: supported F3 inventory and normal repeated frames in one epoch, including recorded fixed actor mirror limits. Fault probes use otherwise protocol-valid stale packets or cap+one admission; no malformed wire is required.

## Must fix

1. **Stale authoritative state becomes a fresh presentation** (`mornlea_client_core/src/session/mirror.rs:171`, `:354`; `src/session/login.rs:982`; `src/presentation/actors/hostile.rs:160`; `src/presentation/world_ui/survival.rs:91`; `src/presentation/world_ui/environment.rs:88`).

   **What this is:** MirrorProvider issues the next local revision for a decoded packet and queues it for every projection. Its stale check compares only the client-assigned ObservationKey revision, not the authoritative source tick.

   **Problem:** Through real public ClientCore and bounded Memory/codec login, spawn hostile1 at tick10, update it to x12 at tick12, then send its valid tick11 state at x11. The next frame is revision3 with a tick11 Upsert/x11. A separate public-endpoint probe sends PlayerState tick12/health20 then tick11/health1: prediction retains tick12 while WorldUi publishes health1 and environment with source_tick11 under revision2. Equal ticks have the same missing ingress guard. Go remote/companion state owners reject stale/equal whole batches, hostile/passive/projectile owners skip stale/equal updates, and the Go predictor preserves survival/environment on stale/equal PlayerState. Delta spec explicitly forbids stale data becoming a newer frame.

   **Fix:** Enforce the accepted source-freshness policy in the existing single confirmed admission owner before publication, so all consumers see the same admitted observations. Retain the actual necessary per-identity/player tick facts across queue consumption. Do not make projections or Python into new tick authorities. The controller must reconcile exact mixed-packet policy against source parity; this review does not choose one.

   **If we skip it:** Actors can move back to an old position; local health/weather/season/armor can revert while the player pose remains newer. The whole-frame shape validator cannot detect the semantic regression.

   **Reproduction:** `/private/tmp/mornlea-f3-completion-core-probe/tests/probe.rs`, tests `backward_actor_tick_does_not_publish_an_older_pose` and `backward_player_tick_does_not_replace_survival`. Both fail on the candidate. Final locked log below retains exact records.

2. **Consumed actor observations leave an unbounded retained mirror** (`mornlea_client_core/src/session/mod.rs:77`; `src/session/mirror.rs:327`, `:348`, `:379`, `:410`, `:443`).

   **What this is:** The observation queue has count/byte limits, but ActorConfirmed keeps live vectors independently until despawn/reset. Spawn/upsert admission clones and appends to those vectors with no retained actor limit.

   **Problem:** Commit one valid unique hostile spawn, consume that observation, repeat65 times: the mirror holds65 hostiles with zero pending observations. Recorded fixed hostile capacity is64. The same cap+one probe confirms companion5 versus4, passive33 versus32, projectile129 versus128, drop801 versus800. Every complete observation commits successfully; counts continue to grow after consumption, increasing clone/search work. The per-frame4096 bound and inbound8192 bound do not constrain that lifetime state.

   **Fix:** Preflight retained live-count demand, including every member of a spawn/upsert batch, in the existing MirrorProvider staged commit. The controller explicitly ruled that overflow returns typed Capacity for the whole observation with prior mirror/revision/queue unchanged; existing public limits/API need not be expanded. Preserve replacement, removal and reuse semantics. Remote-player7 is a measured batch/server-interest ceiling rather than an explicit legacy live-map check; resolve its precise retained policy from its source contract before treating it as equivalent to the five directly fixed caps.

   **If we skip it:** Protocol-valid successive observations bypass every live actor cap, and retained memory and synchronous staging work grow throughout an epoch. The claimed supported bounded mirror/compatibility inventory is false.

   **Reproduction:** Same external probe, `fixed_hostile_mirror_ceiling_does_not_grow_after_consumption` and `other_fixed_actor_ceiling_plus_one_preserves_the_mirror`; both fail. The additional table asserts the controller's chosen whole-Capacity atomicity and logs every over-cap retained count.

## Shared root and six-actor source trace

Call path is `ClientCore::step -> LoginSession::step -> handle_frame -> stage_observation -> MirrorProvider::commit -> stage_event -> ActorConfirmed`, then `LoginSession::publish -> publication::complete -> project_<kind> -> assemble_actors -> prepare_publication/commit_publication`. No actor source tick survives in ActorConfirmed: five vectors contain `(typed id, Dimension)` and drops contain only DropId. AcceptedObservation carries an optional tick, but publication consumes it. Every actor projection is pure over retained observations and copies that optional tick; none owns persistent freshness.

| Kind | Current Go stale/equal source policy | Recorded retained ceiling and qualification |
|---|---|---|
| Remote player | `remote_players.go:168` rejects entire states batch at `ServerTick <= player.lastTick`; spawn initializes tick through remoteActor | 7 is renderer/batch maximum and eight-player server interest derivation; Go live map has no direct fixed-cap guard |
| Companion | `companions.go:70` rejects entire states batch at `Tick <= state.lastTick` | 4; direct whole-spawn error at `companions.go:44` |
| Hostile | `hostiles.go:93` skips stale/equal state per identity | 64; direct stable-ignore at `hostiles.go:65` |
| Passive | `passives.go:129` skips stale/equal state per identity | 32 includes live plus dying legacy presentation; cap check `passives.go:96` |
| Projectile | `projectiles.go:93` skips stale/equal state per identity | 128; direct stable-ignore at `projectiles.go:64` |
| Drop | `drops.go:58` accepts upsert in actual observation order and stores UpsertTick; no stale/equal tick rejection | 800; entire upsert preflight at `drops.go:66` |

Drop backward-tick acceptance is source-defined and documented in Rust `actors/drop.rs:71`; do not invent a universal six-kind tick filter. Typed identity/epoch/reuse checks remain necessary. Removing an actor must release its associated live freshness metadata; reset must invalidate all prior epochs. This report identifies existing owner/caller surfaces rather than designing new shared APIs.

PlayerState has a partial tick owner already: `LiveSession.player_source` and PredictionReplay.last_server_tick protect prediction. They do not prevent the accepted observation from reaching environment/survival projectors. Go `predictor_reconcile.go:20`, `predictor_reconcile_test.go:79`, and stale-or-equal weather/season/armor tests demonstrate the compatibility behavior.

## Commands and evidence

All Cargo runs are headless. The review never launched Godot, a game window, or the full machine matrix. No focused Go command was run, so `make rust` was not substituted or skipped as a Go prerequisite. All later probe compilation used `CARGO_BUILD_JOBS=2` after the controller's host-pressure instruction; the initial crate suite had already finished compiling.

| cwd | Actual command | Exit/result |
|---|---|---|
| root workspace | `git rev-parse 24f57d73c 7624e51a; git status --short` | 0; full identities, clean initial source |
| root workspace | `git worktree add --detach /private/tmp/mornlea-f3-completion-core-review 24f57d73c1e1c2132aa5ad3055767fec2628ca35` | 0; own isolated worktree |
| detached worktree | `CARGO_TARGET_DIR=/private/tmp/mornlea-f3-completion-core-cargo rustup run 1.97.1 cargo test --manifest-path packages/engine/Cargo.toml -p mornlea_client_core --locked` | 0; 424 passing tests: unit48, lifecycle10, prediction48, presentation232, session86. Doc tests discovered0 and are not claimed as additional coverage |
| detached worktree | `git merge-base --is-ancestor 7624e51a7e15e7cdf2c46a24db340ef9c962eacf HEAD` | 0 |
| external probe | `CARGO_BUILD_JOBS=2 CARGO_TARGET_DIR=/private/tmp/mornlea-f3-completion-core-cargo rustup run 1.97.1 cargo test --manifest-path /private/tmp/mornlea-f3-completion-core-probe/Cargo.toml --offline -- --nocapture` | 101; initial2 assertion failures, not compile/setup failure |
| external probe | same command with `--offline --locked -- --nocapture` | 101; subsequent3 assertion failures; final run4 assertion failures after five-kind retained-cap table |
| detached worktree | `git status --short; git rev-parse HEAD` | 0; clean and exact source unchanged |

Logs: `/private/tmp/mornlea-f3-completion-core-tests.log`; initial `/private/tmp/mornlea-f3-completion-core-probe.log`; third-case `/private/tmp/mornlea-f3-completion-core-probe-locked.log`; final `/private/tmp/mornlea-f3-completion-core-probe-final.log`. Disposable probe package includes Cargo.toml, Cargo.lock and tests/probe.rs; path dependencies name the exact detached candidate.

Final SHA256 bindings:

- Probe source: `88d908d4a18062157cc6f9d1f6781641f36e19c8e9b5de03c131453c567baaf7`.
- Final red log: `d779211991ee9be92a9f1a4831422d6833d42e39ebddc7d0e348baac69770919`.
- Unmodified green suite log: `26314267a5b73d91c8a01e39b53bd8685a9380f903aef9e7a30cead077b05997`.

Some exploratory source reads used nonexistent glob/path names and emitted local shell/rg/sed diagnostics. They were followed by corrected exact-path reads; none is a product gate or passing evidence. No test failure or nonzero probe result is relabeled success.

## Seven original criteria, scoped coverage

| Original criterion | This review result | Evidence/limit |
|---|---|---|
| Client state has one Rust owner | Pass for structural core ownership; semantic repairs required | Actual ClientCore/LoginSession/MirrorProvider composition and CoreArena caller trace;424 core tests |
| Frame publication is atomic and bounded | Fail | Findings1/2; staged visible Arc and terrain retry work otherwise have focused green tests, but semantic freshness and retained bounds fail |
| Reset and bridge lifecycle release ownership | Core portion passes; native portion not reviewed here | Actual lifecycle/I/O tests include retained worker socket EOF, late-bind refusal, foreign/released tickets, repeat close/reset/stale epochs; real native100 belongs to controller |
| Rust core parity is independent of rendering | Fail on source parity; full real-provider qualification not rerun here |424 renderer-free tests execute, but concrete stale/fixed-cap behavior diverges; Memory/TCP F2 integration assigned elsewhere |
| Migration preserves typed host isolation | Core dependency/caller seam passes; host/native portion not reviewed here | No server/Godot/Python/GPU dependency in production core; typed single endpoint/frame consumer traced; Python tests/native marshalling assigned elsewhere |
| Cross-runtime boundaries publish only bounded atomic snapshots | C1/C2 structural checks pass; overall criterion cannot pass while findings remain | Whole input/frame reservation, immutable Arc, stale epoch/version/count/byte tests; stale semantic observation still passes upstream before G1 |
| A Rust-mode host tick distributes one complete semantic frame | Core ten-family composition passes; actual host fanout not reviewed here | Live endpoint and publication::complete tests; native/Python host fanout is other scope |

These rows cannot be collapsed into blanket accepted status. Source change requires new immutable review identity and affected regressions. Controller owns integration, chosen compatibility policy, repair scope, final native/provider/matrix gates and rollback. No merge/publication was performed by this reviewer.

Not checked: native Godot/Python processes, native100, full F2 provider integration, full Rust/Go/audit/OpenSpec stage gates, other operating systems and remote source/evidence availability.
Risk: two confirmed original-scope defects remain at this SHA; passing424 core tests and prior historical reviews do not accept them.
