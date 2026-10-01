## Context

The existing Rust Godot extension dynamically loads a pilot Go client-core. Python already consumes typed semantic views. F1 and the accepted F2 protocol/session contract are prerequisites; this stage replaces client runtime ownership without turning pilot code into the final architecture.

The [target runtime interface map](../../../docs/runtime-interface-architecture.md) assigns C1 session, C2 presentation and G1 bridge/registry after F2 S2, with target V1 families consumed by later features. Each shared boundary requires its own compile-ready contract landing and accepted SHA before disjoint consumers start; the Godot registry and real integration have serial owners. Planned facade signatures and logical family names do not imply a present Rust client-core or numeric ABI descriptors.

## Goals / Non-Goals

Preserve typed presentation behavior while moving sessions, mirrors, prediction and frame production to Rust. Do not expand Go core behavior, migrate complete terrain/UI/actors/audio features, transfer tracked visual producers, delete the legacy renderer, or switch defaults.

## Decisions

### Rust core and adapter direction

Add `packages/engine/crates/mornlea_client_core/` with dependencies on F1 protocol/domain and `mornlea_engine`; it must not depend on the Godot adapter, server implementation, Python or GPU host. Existing `mornlea_godot` depends on the Rust core through a safe Rust-facing API and remains sole owner of Godot conversions, feature negotiation and boundary failure translation. Client and server may share contracts, never mutable authority.

Create the new crate guide and update `packages/engine/AGENTS.md`. Keep the old Go dynamic-library path as an explicitly selected pilot/rollback adapter while comparing offline; never automatically fall back after a Rust core failure. The old Go core ABI and `mornlea_client` ABI v19 are separate surfaces and remain until their independent retirement evidence exists.

### Semantic continuity

Move the meaning of the current Go `FrameSnapshot` and typed input/view families into Rust records; do not freeze Go internal layouts as the final contract. One session epoch and one confirmed frame revision cover every sampled view. Rust owns network decoding, mirrors, correction, mesh scheduling and capacity/revision policy. Godot Python applies typed views in bounded batches and owns presentation resources. Unsupported family negotiation fails closed.

Establish an inventory of frame/input/world/entity/UI/cue families under `testdata/runtime-migration/client/`, mapping each current source to Rust tests and required P8–P11 consumers. The [typed family and capacity packet](plans/02-family-schemas.md) fixes the ownership, provenance, identity, byte accounting and split-family assembly rules for the compile-ready contract. Publish versioned family contracts; leave new visual features disabled until their own changes pass. No production GDScript or Python raw ABI/wire access is introduced.

### Thread and lifecycle ownership

Keep bounded queues and immutable publications between session, preparation and main-thread application. Declare reset, reconnect, overflow, drop/upsert and shutdown semantics before implementation. The adapter releases Python feature references before destroying Rust resources; panic handling never unwinds into the host. Test repeated create/reset/destroy with queued work without a graphics device, then qualify the real headless Godot/Python lifecycle separately. The existing `make godot-smoke` validates startup/teardown markers but does not establish a Rust-backed session. Extend the lifecycle harness with explicit Rust producer/revision selection, live create/reset/reconnect/destroy and queued work; fail when only engine/Python startup passes or a stale/Go producer is selected. Rebuild the tested native artifacts and bind their identities to the report.

### Evidence and rejected alternatives

Use offline Go transcripts for parity and F2 integration for local/remote behavior. Screenshots do not prove prediction, ordering or cancellation. Recreating Python wire decoders or preserving a Go runtime fallback would retain the wrong owner. Calling Rust per cell instead of semantic batches violates the bounded bridge direction.

## Risks / Trade-offs

- Typed APIs can match shapes but differ in ordering → compare semantic observations across epochs and correction paths.
- Changing the native link can break packaging → record producer/bridge identity and defer exported release closure to P13; qualify source-tree native loading and failure behavior at the separate real-process gate; the current code-only batch proves only its allowed code-level checks.
- Client-core migration may expose visual differences → retain candidates under existing pilot rules and diagnose semantics; any tracked update waits for a separate feature handoff.

## Migration Plan

Accept F1 and F2 session contracts; inventory families; implement sessions and state, prediction, preparation/publication and lifecycle in separate tested nodes; adapt the Rust Godot bridge; run replay and real Rust-server integration. Publish F3 completion and family versions to P8–P12. Rollback selects the previous pilot/legacy release as a whole without extending it with new features. F3 does not retire the Bootstrap; P14 must first provide qualified native diagnostics.

## Validation and completion evidence

Implementation follows failing contract/replay tests, minimum implementation, then refactoring. Test targets in `tasks.md` are prospective until their owning task registers them. Use the actual Rust workspace (`--manifest-path packages/engine/Cargo.toml`) and named integration targets; inspect `-- --list` output and reject empty discovery. Do not substitute text searches or unrelated optional-build audits for prerequisite acceptance.

Record source SHA, corpus digest/coverage, command, discovered/executed tests, result, failure cases and rollback proof in `ledger.md`. Rust stage completion requires its full declared inventory, not only the first successful slice. Commit each independently verified task; if an inventory item exceeds one session, refine it into explicit capability tasks before implementation rather than checking off a broad placeholder. Planning validation proves artifact structure only.

At implementation closeout run formatting, `make rust-check`, `make dev-check` (including all six Go-module vet commands), `make test-race`, `go test ./packages/audit -count=1`, and `openspec validate --all --strict --no-interactive`. Add the change-specific replay, failure-injection and platform gates. No graphical foreground window may be started by automated tests.

## Post-numerical-closure planning decisions

The [node dependency and ownership register](plans/03-parallel-readiness.md) and [refined node decisions](plans/04-refined-nodes.md) are normative execution inputs alongside the original packets. Preserve prior scope and separate source-contract, real-provider and real-integration acceptance. Numerical completion does not supply complete F1 acceptance; use the independently owned [foundation successor](../rust-runtime-foundation-acceptance/proposal.md).

The controller compared wholesale migration resequencing, immediate parallel dispatch from prospective signatures, and bounded refinement of existing nodes. Bounded refinement preserves reviewed scope and existing node identities while making dependencies, shared edits and source-information limits decidable. Immediate dispatch remains blocked by missing accepted contract/provider SHAs. Broad shared files stay serial; only disjoint providers with accepted predecessors can overlap.

Supporting declarations and packet-to-owned-value mappings are fixed in [05-supporting-values](plans/05-supporting-values.md). Task observations have no invented task ID/generation; container revision is local attribution; absent actor associations stay absent. The checked drop inverse is private C2, preserving the full admitted i32 chunk range through checked i64 arithmetic and exact finite f64 coordinates, without narrowing or rejecting a source-valid identity.


## Approved code-only execution scope

The following decisions were approved in the written-spec review on 2026-10-01. The [concrete code-only handoff](plans/07-code-only-implementation-handoff.md) supplies execution detail without claiming implementation or closing deferred gates.

## Purpose and agreed scope

Deliver the next genuinely unstarted client integration: the complete standalone Rust semantic client core, its safe Rust-to-Godot code adapter, and the Python feature host migration. The selected approach is the complete scope, implemented in dependency-ordered, separately reviewable stages. The user approved this scope and the responsibility boundary in conversation. The user approved this written specification on 2026-10-01; the detailed implementation plan is a separate review artifact.

This is planning only. No product code, tests, engine runs, scenes, models, or renderer changes have been performed for this document. The final planning documents are to be committed and pushed on a new independent Mornlea documentation branch. The running Rust server task remains on its own branch and thread. The user's latest direction is to finish planning and leave ZCode out of this task. The final GLM5.3 handoff is portable and tool-neutral for the user to supply to their chosen implementation tool; no installation, login, external-agent submission, or development launch is included.

The future implementation batch includes:

- All C1 client responsibilities: client-side nonblocking transport admission and I/O queues, session/login, confirmed mirror, semantic input, prediction/correction, bounded CPU preparation, reset/reconnect/close
- All ten C2 semantic families and one atomic frame publisher
- G1 producer-scoped descriptors, safe core token ownership, checked argument/result conversion, and the Rust producer adapter code
- Python host ownership of one core step and one frame pull per tick, with symbolic family negotiation and dependency-ordered activation/rollback
- Automated Rust, replay, Python unit, source/static, and audit checks that do not launch Godot

The future implementation batch excludes:

- The existing Rust F2 server task, including service entry, chunk encoding/publishing/unloading, and its full server acceptance
- Opening or launching Godot in any mode, including editor, foreground, headless, smoke, or a process spawned indirectly by a test runner
- Scene edits, renderer/model/resource realization, visual baselines, screenshots, and P8–P11 feature implementation
- Default-client switching, release packaging, merging, deployment, or implicit Go fallback

Real Godot/Python process qualification, renderer/visual qualification, full real F2/C1/C2/G1 integration, and F3 stage closure remain deferred. They are not the completion criterion for the code-only GLM batch. Their absence must remain visible in evidence and task status.

The narrower login-to-terrain/player slice was not selected: it would give an earlier partial proof but require an explicit exception to all-family assembly readiness and leave the rest of the requested handoff incomplete. Contract-only work was not selected: it minimizes collision with moving server work but postpones the core-to-host path. The complete staged scope preserves the selected architecture and source-family inventory without pretending the deferred runtime gates passed.

## Source authority and existing state

The reviewed revision has no `mornlea_client_core` crate. The workspace contains the existing engine/client/Godot/domain/protocol/storage/server crates, and all 41 F3 tasks are unchecked. The target architecture is already selected in the repository; this document scopes its next implementation rather than proposing a replacement architecture.

Repository AGENTS require active planning to stay in the existing English OpenSpec change: `openspec/changes/rust-client-core/design.md` for decisions, `tasks.md` as the only progress list, `plans/` for execution details, and `ledger.md` for appended evidence. This file is the canonical design; linked execution packets remain in this active change. No duplicate active plan is published under `docs/superpowers`. Preserve correct prior decisions and the sole task status source.

Read the existing source packets as the exact contract, not as proof of implementation:

1. [C1 C2 contract and dispatch graph](https://github.com/chenyang-zz/mornlea/blob/5994fa26937f68669bbcc2ca2be9bf36bb45a0ef/openspec/changes/rust-client-core/plans/00-client-contract.md)
2. [Bounded worker packets](https://github.com/chenyang-zz/mornlea/blob/5994fa26937f68669bbcc2ca2be9bf36bb45a0ef/openspec/changes/rust-client-core/plans/01-client-slices.md)
3. [Typed family schemas](https://github.com/chenyang-zz/mornlea/blob/5994fa26937f68669bbcc2ca2be9bf36bb45a0ef/openspec/changes/rust-client-core/plans/02-family-schemas.md)
4. [Direct accepted predecessor register](https://github.com/chenyang-zz/mornlea/blob/5994fa26937f68669bbcc2ca2be9bf36bb45a0ef/openspec/changes/rust-client-core/plans/03-parallel-readiness.md)
5. [Refined nodes](https://github.com/chenyang-zz/mornlea/blob/5994fa26937f68669bbcc2ca2be9bf36bb45a0ef/openspec/changes/rust-client-core/plans/04-refined-nodes.md)
6. [Supporting owned values](https://github.com/chenyang-zz/mornlea/blob/5994fa26937f68669bbcc2ca2be9bf36bb45a0ef/openspec/changes/rust-client-core/plans/05-supporting-values.md)
7. [Exact G1 facade](https://github.com/chenyang-zz/mornlea/blob/5994fa26937f68669bbcc2ca2be9bf36bb45a0ef/openspec/changes/rust-client-core/plans/06-godot-facade.md)

The direct predecessor register controls readiness; refined child packets replace retired parent nodes. Supporting-value and exact facade packets complete or supersede loose older declarations. If current accepted source differs, the controller resolves the contract discrepancy and records a new accepted identity before dispatching affected consumers.

F1 has historical zero-gap acceptance at `d042982d33bb1694d768b75b01c297bd02534a08`, with 112 supported points and 1,434 cases. F2 common S2, Memory, and TCP are checked at the reviewed revision. Its ledger records joint local/remote S2 acceptance at `756937af` with 16 parity cases and 81 full crate cases. These are historical recorded results, not checks rerun for this plan. The F3 ledger's prose saying S2 is pending is stale. Implementation entry must re-bind the latest actually accepted common-plus-both-adapters S2 source/evidence; preliminary F3 work does not wait for full F2 closure. Final F3 node 3.4 does wait for F2 node 4.2.

The existing Memory/TCP implementations are server adapters. They do not establish that a client connector exists. The new client core still owns its connector port and client-side bounded I/O implementation.

Preserve protocol v45, player/chunk v9, metadata v6, companions v5, hostiles v2, passives v1, engine ABI v11, renderer client ABI v19, and scenario v23. Pilot core ABI 1.1 is a distinct explicit rollback surface. Semantic C2 major/minor and producer-scoped G1 descriptors are not wire or renderer ABI versions. Unsupported required major/minor fails before feature creation; no compatibility coercion silently changes identity, absent fields, or ownership.

## Responsibility boundaries

`mornlea_client_core` is an independent Rust crate depending on accepted `mornlea_protocol`, `mornlea_domain`, and `mornlea_engine` APIs. It must not import the server implementation, Godot, Python, GPU code, or the Go pilot. It owns authoritative client observations and their confirmed mirror, local input admission, reversible prediction, CPU preparation, semantic projections, and immutable publications.

`mornlea_godot` owns the G1 boundary: native capability negotiation, a private core arena, stale-token rejection, checked conversion, complete owned value copies, and lifetime/panic containment. It calls safe Rust C1/C2 directly in Rust mode. It does not derive protocol meaning, assign input sequences, duplicate container authority, or dynamically load Go in Rust mode. Old `session_*` methods remain only on the explicitly selected pilot path.

The Python host owns presentation dispatch. It collects semantic feature input, submits through the facade, performs exactly one `step` and one `pull_typed_frame` for each host tick, validates the whole successful frame and required descriptors, and distributes the same owned frame to features in dependency order. Features use `apply_typed_frame(frame)` only; they do not mutate the shared frame, drive the session, parse packet bytes, or retain borrowed native storage. Derived presentation resources remain feature-owned.

At the reviewed source, `feature_host.py` instead loops over feature `drive_session` methods and takes the first feature's `pull_typed_frame`. That exact ownership reversal is part of this migration. Preserve the existing required-feature reverse rollback and dependency-ordered activation, and optional-feature disable reasons. This batch does not enable the 14 reserved manifests or qualify actual feature resources.

## C1 transport session and lifecycle

The public safe client surface remains `new`, `connect`, `submit_input`, `step`, `snapshot`, `reset`, and `close`, with the existing `ClientEndpoint` operation signatures. `snapshot(epoch)` returns an immutable `Arc<PresentationFrame>`. Constructors and work budgets have checked private fields and read-only accessors.

`Endpoint` is `Memory { connector_id: NonZeroU64 }` or `Tcp(SocketAddr)`. Memory connectors resolve through an injected native registry, never a server dependency or Python callback. TCP uses a checked numeric IP/socket address, with no blocking DNS lookup. A connector has nonblocking `try_connect`, `poll`, `try_send`, and `close`; `TransportPoll` is `Pending`, `Connected`, one complete framed v45 packet, or `Closed(ClientError)`. A private ticket includes the nonzero launch generation. Old tickets are invalid after reset/close.

The TCP provider owns bounded worker/queue resources. Network reads, waits, Python, and GPU work never block `core.step`. Both client transport paths feed the same observation and outbound ownership model. `try_send` admits a complete framed command on success; `Capacity` or `Io` retains the entire outbound head for retry or terminal handling. Fragmented TCP input is assembled under the accepted protocol body limit before the normal decoder/admission path.

The core has one mutable state owner. Transport/preparation workers own only their bounded queue/job allocations and return complete immutable results; they cannot mutate the mirror, input admission owner, or published frame. Queue locks and native waits must not span decoding, projections, frame construction, or callbacks. A reset cancels by epoch/generation before replacement and releases stale results exactly once. Any shutdown/join work that would wait on external I/O stays outside the core's synchronous hot path. Deterministic tests control completion order rather than depending on sleeps or scheduler luck.

`connect` returns a nonzero epoch for pending admission. Only `step` advances the session through validated observations. The phases are `Disconnected`, `Connecting`, `Handshaking`, `Admitted`, and `Closing`; transitions obey the accepted state machine, with one terminal publication. The injected monotonic clock enforces the accepted 5-second hello and 10-second login policy unless re-bound accepted S2 changes it. Input receipts report local admission, never server success.

Reset invalidates queued input, transport tickets, prediction journals, preparation jobs/results, and native handle generations before the next epoch becomes visible. Old completions cannot enter the new mirror/frame. Close is idempotent. Consumers release before providers/native resources. A late callback or duplicate release is rejected or ignored with exactly-once ownership accounting.

## Input admission and confirmed state

The semantic input set is closed to these twenty actions: `PlayerInput`, `PlaceBlock`, `Resync`, `SelectHotbar`, `OpenContainer`, `TillSoil`, `BoneMeal`, `CollectWater`, `PlaceWater`, `MoveInventory`, `MoveCrafting`, `MoveContainer`, `CloseContainer`, `DropSelectedItem`, `TakeCraftingOutput`, `EquipArmor`, `MovePartial`, `QuickMove`, `DropStack`, and `Chat`. Reconcile the actual F1 public parts/getters and every accepted discriminant before freezing declarations. `KeepAliveReply` is session-generated only. Device scan codes and focus policy remain outside C1.

An `InputBatch` carries epoch plus zero to 128 checked actions. `validate_batch` is read-only. It checks phase, epoch, action fields, finite/ranged values, text, local view tokens, encoded sizes, journal demand, and queue demand. One admission owner atomically reserves and commits outbound records, journal entries, and contiguous local sequences. Validation, encoding, allocation, arithmetic, token, or capacity failure consumes no sequence and enqueues no part of a batch.

Empty semantic input is `Noop`. Chat is unsequenced and uses no prediction journal entry. Input/chat/input commits exactly two contiguous sequences plus one chat. Device-neutral input behavior remains the separate P11 adapter concern.

`ContainerToken` carries epoch, the unchanged accepted Furnace/Chest `ContainerRef`, and local confirmed-revision attribution. Crafting uses a separate `CraftingViewToken` with epoch/revision/size, with no fabricated workbench reference or generation. Required, absent, irrelevant, stale, closed, or mismatched tokens are checked for every action/region combination. Python never duplicates this authority.

One complete accepted authoritative observation updates a staging mirror, then atomically swaps it and increments local `ConfirmedRevision` once. Its optional source tick is separate. `ForgetChunks` has no tick to invent. Duplicate, backward, malformed, ambiguous, old-epoch, and old-generation observations preserve prior accepted state. Despawn/remove precedes identity reuse. Prediction starts from the confirmed player state, discards acknowledged entries, and replays remaining journal entries in sequence through the accepted numerical facade and frozen tolerance.

## C2 publication and provenance

All ten logical families start at major 1/minor 0: `session`, `input`, `terrain`, `actors`, `player-view`, `inventory-ui`, `world-ui`, `audio-cues`, `lifecycle`, and `diagnostics`. Exact fields, unions, source mapping, units, and bounds are the typed family and supporting-value packets linked above. There is no worker-local extension of their public schema.

Completeness means every accepted family contract/provider is implemented and every required descriptor is present, not that every family has a nonempty record on every tick. Empty records are legal only where the accepted schema permits them. Pending login cannot synthesize actor/world/player observations to fill a family. A later optional framed extension may be skipped only through its declared length and compatible minor rules; that does not relax G1's current exact-field dictionary validation.

One frame owns layout version, epoch, confirmed revision, local frame index, and its families. Every record has epoch/revision, optional source tick, and `Upsert`/`Remove`. The validator checks every child, identities, required families, duplicates, finite numbers, record counts, full owned byte totals, and compatibility before the publisher makes one `Arc` swap. A failure preserves the old frame and frame index. Local publication may advance frame index without increasing confirmed revision. At revision zero, only pending/session/lifecycle/diagnostics/local-input records allowed by the accepted contract may appear.

Actor identity remains a closed tagged union of Player/Drop/Hostile/Passive/Projectile/Companion identities. Missing source fields remain absent. Hostiles have yaw without fabricated pitch; passive state has no lure marker; CombatHit does not identify an attacker/projectile; companion task observations belong to ChatEvent-derived world UI. Task states are the actual Started/Progress/Completed/TimedOut/Stopped/Failed union, with no invented task ID, generation, percentage, Pending, or Running. Container revision remains local attribution, not a wire field.

Drop positions preserve the full accepted i32 chunk range and raw dimension. Compute `chunk * 16 + local` in i64 and convert directly to finite f64; do not narrow to i32 or reject a source-valid extreme coordinate. In particular, maximum i32 chunk x maps to 34,359,738,352. The detailed supporting-value rule supersedes older general prose suggesting rejection and the Go narrowing wrap. Block index 98,304 and above is invalid.

Audio provenance is `Confirmed { ObservationKey, optional actual event ID }`, `Predicted { input sequence }`, or `Local { local event sequence }`. Dedup is epoch/provenance-scoped. Do not invent a universal event ID. Only footsteps/snow use predicted cues; combat/place/world outcomes remain confirmed. Device absence does not alter semantic cue generation.

Terrain keys distinguish near sections from far LOD tiles, and visibility must match. Bounded preparation consumes owned Near/Far jobs, returns the complete original job on admission rejection, retains FIFO remainder, and rejects stale epoch/generation/content/seed results. The resource arena retains immutable geometry with `Arc`; safe Rust-only `prepared_resource(key)` returns geometry for later native realization. Python gets the key only. Far-ring scheduling, distance/x/z ordering, static precharge, and native numerical facade are source-defined. P8 resource realization remains deferred.

## Capacity and error policy

Existing source ceilings include 128 semantic input actions, 4,096 work items per step, and the protocol's 2 MiB body cap. Proposed 4,104 queue records, 8 MiB queue bytes, 4,096 pending preparation results, 64 MiB preparation bytes, and 8 MiB complete frame bytes are not measured facts or accepted defaults yet. The inventory stage measures every supported replay's high-water and complete owned bytes; the contract stage freezes compatible limits and cap-plus-one behavior. The existing maximum world batch is 4,325,408 bytes, so a 4 MiB complete frame cap is invalid.

Work budgets are item ceilings, not unmeasured millisecond guarantees. Zero-work steps retain accepted FIFO work; the 4,097 request rejects before dequeue. Backpressure cannot coalesce away input, removal, acknowledgments, or terminal state. Measure step cost and preparation high-water against fixed replay/fixture identity in external test evidence without adding undeclared diagnostics fields or claiming a new FPS target. Total-frame accounting and the maximum aggregate world batch are distinct from the per-packet 2 MiB protocol-body limit.

Byte accounting includes scalars, headers, union and optional tags, text, vectors, resource keys, queue-held and worker-held preparation allocations, and actual owned registry/parameter allocations. Shared Arc allocation ownership is counted once; separately allocated equal payloads are separate. Checked arithmetic must fail before partial state changes. Any supported replay exceeding a proposal requires one controller-owned versioned contract revision before consumers proceed.

Errors are closed to `InvalidInput`, `IncompatibleVersion`, `InvalidState`, `StaleEpoch`, `Capacity`, `Timeout`, `Disconnected`, `Io`, and `Internal`. A native panic maps to `Internal`. Overflow, stale work, and malformed input never permit a partial enqueue, partial mirror commit, or partial frame publication.

## G1 facade and host contract

The exact exported methods are `open_core`, `connect`, `submit_typed_input`, `step`, `pull_typed_frame`, `family_table`, `reset`, and `close`. Their complete arguments/results are the exact facade packet linked above. No silent alias is added to avoid migrating the host.

Every result is exactly `{ ok, value, error }`, with null error on success and null value on failure. Error details use the registered class/resource/limit/observed vocabulary. All u64 values, including epochs, revisions, generations, counts, and u64 maximum, use canonical checked decimal strings across G1. UUID values use 32 lowercase hex digits; digests use fixed-length lowercase hex. Floats are finite; missing/extra fields, unknown tags, incompatible layout, malformed identifiers, and incorrect option shape fail before a core call.

`CoreToken` is a nonzero u32 slot plus decimal-text u64 generation in a private bridge arena, never a pointer. A stale generation fails before access. Repeated close of the same issued token succeeds without another release. Submit decoding validates the complete batch, calls C1 exactly once on success, and calls it zero times on failure. Pulled Godot values are owned copies, with no retained Rust borrow.

Producer `rust-client-core` assigns numeric IDs 1–10 in the ten-family order above. The pilot's IDs 1–8 are a separate producer table, not the meanings of Rust IDs. Resolve symbolic logical names before feature instantiation and reject duplicate/unknown keys, duplicate IDs within a producer, wrong major, unsupported required minor, and missing mandatory family. Registry/catalog/bridge/host edits have one serial owner.

## Code-only verification and deferred qualification

The implementation plan must distinguish three kinds of evidence:

1. Contract evidence: compiling checked declarations and an executing deterministic consumer double. This does not accept a real provider.
2. Provider/code-integration evidence: real Rust core behavior against replay fixtures; actual C1 with the engine-independent adapter validation/copy/lifetime routines used by exported methods; and actual production FeatureHost logic with only its py4godot shell stubbed. This does not accept native Godot marshalling or an engine process.
3. Deferred runtime evidence: native Godot values/real bridge, embedded Python release behavior, rebuilt 100 actual process cycles, renderer/visual tests, and full F2/C1/C2/G1 integration.

Keep validation and owned intermediate conversion routines testable without Godot; exported methods must use those same routines. Do not replace them with dictionary-only test doubles or claim native marshalling passed. The exact Godot conversion layer is compile-checked here and runtime-qualified later.

The existing `feature_contract_check.py` and `bridge_host_check.py` are Godot Node scripts, so their execution is prohibited in this batch. Add an engine-free host unit harness rather than launching those scripts. It must exercise one step/pull, same-frame dispatch, failure retention, symbolic negotiation, required rollback, optional disable behavior, stale generation, reset, and idempotent close in the actual production host logic.

At the reviewed source, `make godot-project-check` invokes source-only `validate-project.sh`, and `make godot-capability-check` invokes a Python registry checker. Their callees must be re-inspected on the implementation SHA before execution. `make godot-python-check` invokes static/unit tooling without Godot, but requires the existing pinned embedded macOS Python runtime and locked environment. If absent, report that exact gate as blocked; do not build/start Godot or substitute another interpreter as proof of the same gate.

Rust/Go/audit commands also require a callee review for hidden engine launch or unrelated implementation. Discover every selected test filter and record nonzero cases. A compiling target, empty filter, stale binary, synthetic source, or Go-only transcript is not acceptance of a real Rust provider.

Fault injection must cover fragmented/truncated/oversized packets, phase/version rejection, deadline immediately before and at expiry, duplicate/out-of-order observations, transport Capacity/Io before send, failed encoding/allocation/checked arithmetic, queue and full-owned-byte cap plus one, stale tickets/tokens/results after reset, duplicate release, a caught native panic, mixed revisions, missing family, and required/optional feature activation failure. Assert unchanged prior frame/index/mirror/sequence/queue where each owner requires atomic rejection, plus exactly-once terminal/release and zero retained stale work. Preserve diagnostics producer/source/contract identity and owner-maintained bounded counters; saturation is reported as incomplete evidence rather than wrapping.

Use the actual read-only Go implementation only for semantics it really represents. Runtime correction oracles select `Predict`, not `Prediction`. Remote-player presentation Entity tests do not prove every actor mirror; companion/drop/hostile/passive/projectile cases need their actual mirror/transcript oracles. Audio Cue tests prove PCM synthesis, not provenance; provenance requires cue-selection/step transcripts and the real Rust family tests. Extreme drop geometry uses the checked mathematical source contract rather than reproducing Go's narrowing wrap. Freeze numerical tolerance from accepted F1, never a worker-selected epsilon.

For this code-only batch, dependent code work may proceed after its actual safe contract and code-level predecessor tests pass, while an engine-dependent qualification remains explicitly pending. That is a scoped staging decision, not permission to check off an original node whose full prescribed acceptance has not occurred. Record implemented code, allowed checks, blocked checks, and deferred checks separately. In particular, 3.3c, 3.4, and F3 full closeout remain open; 3.3b's real bridge acceptance is deferred.

## Ownership integration and handoff

The detailed implementation plan will preserve the repository's node ownership. One controller owns workspace/lockfiles, crate exports, test entrypoints, contracts/validator, family assemblers, fixture indexes, ledgers, registry, native bridge, host, catalogs, and shared runners. Disjoint providers edit only their packet files, using at most three isolated workers. Parallelism starts only after their direct accepted code-level prerequisites and frozen contract identity. Shared files and integrations are serial.

The order is inventory and accepted-prerequisite binding; compiling C1/C2 contracts; session/mirror/I/O and ready independent providers; prediction/preparation and complete family projections; serial family and full-frame assembly; pure-core lifecycle; producer descriptors and safe adapter; host migration and code-level release behavior. Far LOD preparation remains included. Partial login/terrain-only output cannot masquerade as the selected complete scope.

Each passing stage records source/contract identity, owned files, fixture digest, behavioral red and green, nonzero discovered/executed cases, integration identity, unchanged-state assertions, derived refresh, and rollback. A provider rollback touches its owned files only; a contract rollback includes its consumers. Actual default-client rollback and release switching remain separate P13/P14 work.

Before any future edits, GLM must inspect the actual checkout, root/relevant AGENTS and local skills, clean/dirty state, branch ancestry, existing Godot changes, and current accepted F1/S2 evidence. The reported unpushed Godot commits `1839e3ac` and `ff4bd261`, and bridge/catalog/World3D ownership concerns, are checkout-inspection inputs rather than accepted facts at this source. Do not overwrite, merge, or cherry-pick them by inference.

Final planning publication is documents only on a new independent branch based on `5994fa26937f68669bbcc2ca2be9bf36bb45a0ef`, with its exact base/commit/diff and remote existence verified. The running F2 branch `cursor/rust-authoritative-server-98e6` is never a documentation or implementation write target. It had advanced to `7104fa07979f14968be020089e55e3b61d9e0c3f` when observed at 2026-10-01 09:13 UTC; that observation does not establish a new accepted S2 identity. The implementation entry audit chooses the currently accepted F1/S2-compatible integration source and records any source/contract changes rather than treating the planning pin as permanently current.

No server branch edits, merge, code implementation, or default change occurs during planning. After the user reviews this specification and the implementation plan, the approved portable handoff targets GLM5.3 without assuming a particular coding app, login state, or automatic launch. A future executor must use an independent implementation workdir and branch based on the recorded accepted source, preserving the running F2 checkout and all unrelated changes. Keep the approved no-Godot, no-F2-write, no-auto-merge/default-switch constraints in the handoff. Include no credentials, tokens, or login secrets in any document or agent prompt.

Repository orchestration remains binding. At most three implementation/review workers may run concurrently. An OpenAI controller retains the shared-contract, dispatch, integration, and acceptance decisions when assigning GLM5.3 bounded implementation work. If GLM5.3 becomes the controller, it must use the repository's strict non-OpenAI subagent-driven workflow with independent implementation and review responsibilities; it cannot silently replace that with self-implementation/self-approval. If the chosen implementation tool cannot supply required isolation/review roles, the outer controller supplies those roles before dispatch, or reports the exact unsupported requirement. Neither model selection nor planning approval removes the gate or expands source ownership.
