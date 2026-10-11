# Comprehensive review repair implementation plan

**Goal:** Repair the confirmed acceptance defects in the existing F3 code and verify the actual installed endpoint rather than disconnected provider substitutes.

**Architecture:** Preserve C1/C2/G1 and protocol v45. The existing session owns transport and composes the existing mirror, admission, prediction, preparation and publication providers. The bridge and host validate complete owned semantic values. No server implementation dependency, fallback or default switch is introduced.

**Tech stack:** Rust 1.97.1, the existing native numerical facade, typed Python and engine-free tests.

**Spec:** `../design.md`, both delta specifications, packets 00/02/05/06/07. `../tasks.md` is the sole status store. Execute with the installed Superpowers testing, debugging and verification skills; the controller retains integration responsibility.

## Scope and review authority

The user requested repairs to the comprehensive review of immutable source `b383673e383dcf1f6177998854e4752a28304a2b`. The controller used brainstorming to compare isolated symptom fixes, scope reduction, and composition of accepted owners. Composition is selected because the approved code scope includes the complete client. Isolated fixes cannot repair the actual call graph; scope reduction would weaken acceptance. Existing provider APIs remain the starting contract. New private staging surfaces are landed and tested before the serial integration consumer uses them.

No Godot process, scene, visual baseline, renderer realization, server implementation write, remote publication or default switch is authorized by this repair packet. Original native/process/server gates stay open without their actual evidence. The prior review's tick-zero question is not a new parity regression; first authority is handled explicitly when composing prediction. The native-release checkbox is reopened immediately because its original qualification never ran.

## Review focus and ownership

Cover queue exhaustion after transport dequeue, failed replay then same-tick retry, failed full-frame publication after terrain completion, spawn and despawn on separate published frames, and malformed/stale native results before host fan-out. Each concrete test belongs to the node below. Tests exercise real owners; transport and py4godot shells alone may be substituted.

The controller edits roots/exports, contract surfaces, final endpoint integration, fixture indexes, status, ledger and guides. Isolated workers own the disjoint provider files specified below. Workers report contract conflicts before changing shared behavior. Each coherent verified node has its own scoped English commit. The controller reviews and cherry-picks verified commits; a rollback reverts that node and its dependent consumers, preserving unrelated changes.

All new source comments are English and contain no OpenSpec task IDs. No dependency/compiler/lockfile/version change is required. Derived consumers include Rust `include_str!`/source-boundary tests, Godot capability catalog checks, Python boundary/type checks, and the six-module architecture audit. Refresh only an explicitly failing derived artifact with its existing generator; never hand-edit tracked asset or fixture digests.

## Boundary repair: R13–R15

**Editable:** `packages/engine/crates/mornlea_godot/src/abi.rs`, native conversion portions of `src/bridge.rs`, existing Rust boundary tests, `packages/engine/crates/mornlea_client_core/src/presentation/mod.rs` counter getters only, `apps/mornlea-godot/app/host/feature_host.py`, `apps/mornlea-godot/tests/` host tests. Read-only: plan06, capability registry resource/catalog, endpoint arena.

Preserve `{ok,value,error}` exactly, null error on success/null value on failure. Render u8/u16/u32 as bounded integers, actual u64 as canonical decimal text. Preserve nested companion identity/name and confirmed cue `observation`; render queue/error counter dictionaries through read-only typed getters. Regression compares actual output for survival, drops, chat, audio and diagnostics with frozen fields/types, including maximum u64.

Host retains and validates the descriptor table declared in the existing capability registry: exact fields, logical versions, IDs 1–10, record bound 4096 and byte declaration 0. It validates exact result envelopes, supported frame layout, active epoch, nondecreasing frame index/revision, unique family keys and every child's matching header before dispatch. Repeated pulls of one unchanged frame remain legal. No concrete feature family literals are added to app/host. Replace the scripted step fixture with the actual declared report fields. Replay the seven accepted-bad host probes as rejection tests and assert prior frame/dispatch count unchanged.

Reject input actions above 128 before allocating or decoding them. Native input conversion checks thread first and enforces a shared schema-derived budget: depth 9, 3843 value nodes, arrays 128, dictionaries 12 entries, string leaves 1024 bytes and keys 32 bytes. Reject unknown extra/deep fields within bounded traversal. Conversion and owner calls cannot unwind into Godot. Test every ceiling and a middle invalid action with zero endpoint calls. These are input budgets, not output-frame budgets.

Commands: `rustup run 1.97.1 cargo test --manifest-path packages/engine/Cargo.toml -p mornlea_godot --locked`; `python3 -m unittest discover -s apps/mornlea-godot/tests/unit -v`. `make godot-python-check`, `make godot-project-check`, `make godot-capability-check` after callee inspection. No native/process acceptance claim.

## Session repair: R2/R3/R11/R12

**Editable:** core `src/session/{login,lifecycle,mirror,mod}.rs` and registered `tests/session_replay/` / `tests/lifecycle_contract/` modules. Read-only: input/preparation/prediction, existing packet codec and accepted connector implementations.

Use `LifecycleOwner` as the single monotonic epoch/ticket release authority in `LoginSession`; preserve current endpoint signatures. Extend its terminal operation to accept the actual `CloseReason`, with existing local `close` delegating. Build records before release; a release refusal must leave a retryable owner, or publish a consistent terminal failure without retaining a permanently nonterminal frame. Repeated close succeeds, terminal reconnect retires the previous generation, reset publishes the dying `Reset`/`Invalidate` before fresh `Open`; old snapshots remain immutable. Add `LifecycleOwner::next_epoch() -> Result<SessionEpoch, ClientError>` as checked read-only build preflight, then let `open` alone issue it. Add `terminate(epoch, reason, connector, owners, preparation) -> Result<Vec<LifecycleRecord>, ClientError>`; existing close delegates with LocalClose. Rebase lifecycle records onto the actual confirmed revision. Reset prepares and validates the dying publication internally, invalidates owners, consumes it before opening the next generation, and retains no old-epoch record in the fresh frame. The fixed reset facade returns only the fresh epoch: host consumers release old references before that operation; this repair does not invent cross-epoch child headers or claim the dying frame is externally pulled after reset. Real LoginSession regressions replace lifecycle test handovers that hide the installed provider.

Validate both requested messages/meshes against selected config before dequeue. A decoded valid observation that cannot fit after transport dequeue terminates with typed Capacity, without a successful silent drop. Mirror admission charges actual retained packet allocations and observation metadata, with checked arithmetic, until publication consumption/invalidation; count and byte cap-plus-one reject before mirror mutation. Use protocol/domain owned sizes, not a lossy compressed-length approximation. Crafting has one current grid: replace all prior size/revision attributions; stale workbench input rejects with sequence/queue/journal unchanged.

Red cases: repeat close/reconnect/reset on the same real endpoint; two valid ForgetChunks under count 1; 32778-byte ForgetChunks under bytes 1; messages 2 with selected cap 1; Workbench rev1 then Personal rev2 rejects old token; connector release refusal then retry. Green preserves prior phase/frame for invalid input and reports terminal overflow explicitly.

Release refinement: for the exact ticket issued to the active lifecycle owner, close InvalidState is accepted as already released only when the same connector's nonblocking poll confirms Closed(InvalidState). Other errors or a still-live poll retain retry ownership. This closes the confirmed missing-transport terminal stall without swallowing arbitrary release failures or changing the transport port's stale-ticket classification.

Commands: core `cargo test --test session_replay --locked` and `--test lifecycle_contract --locked`, using the workspace manifest/toolchain. Commit lifecycle, capacity and crafting as coherent nodes with measured red/green evidence.

## Projection and preparation repair: R4–R8/R10/R12

**Editable:** core `src/presentation/family_actors.rs`, `src/prediction.rs`, `src/preparation.rs`, `src/presentation/family_terrain.rs`, `src/presentation/assembly.rs`, existing corresponding registered tests. Shared presentation types/exports only after controller ruling; audio projection/history is controller-owned later.

Actor removes consume the accepted mirror provider's resolved attribution; do not reconstruct live identity solely from this frame's delta list. Test spawn→assembly→consume→later despawn, plus same-batch identity reuse.

Prediction stages a cloned candidate owner and completes replay/ray derivation before replacement. Apply the same rule to rejection replay. A finite correction outside the supplied grid must fail without changing tick/base/journal; the same source tick with a covering grid must then succeed. No invented epsilon.

Preparation retains every full resource key until explicit successful-publication retirement. Count/charge simultaneous old and new geometry. Distinguish fresh job_id from duplicate delivery while rejecting stale epoch/generation/content; same-content re-entry with a new job ID completes. Prune high-water metadata only when no pending/completed/retained owner remains; no unbounded history or stale resurrection. Admission summaries commit only after `try_submit` succeeds and name exact admitted keys. Tests reuse the review's real queue/provider repro cases.

Terrain stage is a private preparation/commit transaction: preparation may advance bounded workers and retain retry-owned results, but cannot change the prior published reference set or forget resources referenced by that set. A staged record/reference/retirement value is committed only after complete-frame validation. Existing `publish` may remain a convenience wrapper for family-only provider tests; the actual core uses staged operations. Land the compile-ready staging value and consumer tests before its provider/serial integration acceptance; Frozen signatures: `prepare(view, queue, selection, center, params, budget) -> Result<TerrainPublication, ClientError>`, `TerrainPublication::records() -> &[TerrainRecord]`, and `commit(&mut self, queue: &mut PreparationQueue, publication: TerrainPublication) -> ()`. `TerrainPublication` privately owns prebuilt next references, exact retirements and records. Its single-owner commit only moves state and explicitly releases exact keys. The existing `publish` arguments remain unchanged and delegate to prepare/commit. The serial core is the only production consumer and cannot commit an unchecked whole-frame proposal. A full-frame failure from another family preserves all old terrain keys/resources and keeps new completions retryable. Commit performs no fallible allocation or callback.

Whole-publication admission requires all ten families, exact next frame index, same current epoch and nondecreasing confirmed revision. New epochs enter only via lifecycle replacement, not arbitrary candidate publication. Keep existing partial-family byte validation usable by local providers. Red cases: complete old-epoch frame, complete regressed-revision frame, zero-family frame; green same-revision next frame.

Commands: core `cargo test --test presentation_contract --locked`, `--test prediction_replay --locked`; discover nonempty regression filters. Independent commits for actors, correction, preparation, terrain staging and full-frame gate. Controller integrates each accepted SHA before consuming changed surfaces.

## Serial CPU and composed endpoint repair: R1/R9/R16/R12

**Owner:** controller after accepted provider repairs. **Editable:** core contracts/lib, input/preparation, session/login and mirror world ownership, presentation audio/prompt and shared types; Godot core arena and native connector injection. Corresponding registered real endpoint/preparation/audio tests and scoped guide updates. No overlap with worker edits until their commits are integrated.

Replace successful empty near/far geometry with the existing `NativeMesh::mesh` / `NativeLod::build`, using reusable bounded scratch and conservative destination bounds. Map kernel errors to closed client classes; geometry is retained only after successful complete build and capacity accounting. Compare a nonempty near section and a far tile to the actual native facade; real lookup returns the exact arena Arc and reset makes its key stale. No GPU code or fake successful geometry.

`ClientCore` becomes the safe public endpoint owner, preserving configuration access and existing endpoint operations, delegating to one composed session. CoreArena installs this real endpoint; no independent configuration-only core can masquerade as resource owner. Memory resolves only injected native capabilities; numeric TCP uses the accepted nonblocking TcpConnector with no DNS. The complete installed endpoint test performs hello/login, typed input, KeepAlive reply, multiple observations, complete publication, reset and close.

Extend the existing confirmed world owner to retain bounded immutable decoded chunk content, apply exact block deltas and forget payloads with their revision. Prediction and near-mesh input read this same owner; no separate mutable mirror or second decoder. Input admission gains crate-private successful-send head removal, acknowledgment and publication-consumption operations; refusal preserves FIFO, bytes, sequence and journal. KeepAlive is session-generated only. Each accepted play event receives the mirror provider's current next revision, never one shared staged revision. Publish all ten families with schema-legal empty families before authority exists; never fabricate gameplay records to fill them. A complete-frame rejection retains events/input/lifecycle/terrain references.

Snow cue facts come from real prediction advance: loaded foot block, snow material, ground bit, displacement and once-per-frame stride selection. First sample, stationary, off-snow, airborne and teleport remain silent. Do not emit a cue for every pending journal entry or correction replay. Shared facts are landed with concrete source tests before the audio projection consumes them. Audio dedup history remains entire-epoch: bounded staged history rejects Capacity before mutation rather than silently evicting old confirmed IDs. Predicted cancellation metadata retires only when its source is no longer live.

The confirmed block→registered-name path also permits the existing prompt node to be completed if the recorded ray target is current; it does not authorize new UI behavior. If a source conflict arises, reconcile the exact accepted contract and affected packet before code.

## Verification and handoff

After focused green nodes, independently review the exact integrated candidate and rerun the review regressions against real installed owners. Run `make rust`, `make dev-check`, `make test-race`, uncached `go test ./packages/audit -count=1`, the allowed Godot source/Python checks, and `openspec validate --all --strict --no-interactive`. Record commands/exits/nonempty counts and SHA in the ledger. Reconcile reopened original tasks against their actual criterion coverage; code repair does not mark native/process/server gates complete. Architecture skill: promote only stable verified rules, otherwise record no change. Local handoff remains copyable and does not imply push, external readability or acceptance.
