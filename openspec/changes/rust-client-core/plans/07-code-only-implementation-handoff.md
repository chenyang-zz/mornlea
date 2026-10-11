# Client core code integration implementation plan

> For agentic workers: use the project-selected Superpowers execution method. A non-OpenAI or unverified controller must use strict subagent-driven-development with independent implementation and review. An OpenAI controller follows the repository's provider-aware orchestration policy. This packet has numbered execution steps; `../tasks.md` is the only checkbox and completion-status source.

**Goal:** Implement the complete Rust semantic client core, safe Rust producer adapter code, and one-frame Python host plumbing, with engine-free automated evidence and explicitly deferred runtime qualification.

**Architecture:** One Rust owner handles C1 state and immutable C2 publications using accepted protocol/domain/engine contracts. G1 validates and copies owned typed values, and the Python host drives one core step/pull per tick. Contract landings and shared integrations are serial; ready providers have disjoint files and one accepted contract identity.

**Tech Stack:** Rust 1.97.1; Go 1.26 read-only compatibility oracles; existing Godot 4.7/godot-rust 0.5.5 code; pinned typed Python tooling. No Godot process executes in this batch.

**Spec:** The user approved written specification version 1 on 2026-10-01 with `通过`. Canonical decisions are incorporated into [`../design.md`](../design.md) and the delta specs. Read proposal, delta specs, design, tasks, then packets [00](00-client-contract.md), [01](01-client-slices.md), [02](02-family-schemas.md), [03](03-parallel-readiness.md), [04](04-refined-nodes.md), [05](05-supporting-values.md), [06](06-godot-facade.md), and this packet. The direct predecessor register and refined nodes supersede retired parent packets.

**Planning source:** `5994fa26937f68669bbcc2ca2be9bf36bb45a0ef`. The documentation branch is based on that revision. This is not a permanent implementation or accepted-contract SHA; the entry audit binds current accepted F1/S2 evidence before dispatch.

## Global constraints

- Complete scope means all ten C2 families and all twenty C1 actions, including far LOD preparation. Do not stop at the first login/terrain/player slice or label it complete.
- Preserve protocol v45; player/chunk v9; metadata v6; companions v5; hostiles v2; passives v1; engine ABI v11; renderer client ABI v19; scenario v23. Pilot core ABI 1.1 is a separate explicit rollback surface.
- C1 production dependencies are protocol/domain/engine only. No server implementation, Godot, Python, GPU, or Go core dependency enters the new core.
- Never edit the running F2 branch `cursor/rust-authoritative-server-98e6` or its workdir. Its service/chunk/full-server work remains separately owned. Use a clean independent implementation branch/worktree and preserve unrelated work.
- Do not launch Godot, including editor, headless, smoke, indirect harness launch, or foreground. Do not edit scenes/models/renderers, realize P8 resources, update visual baselines, switch defaults, merge, deploy, or auto-submit to another coding app.
- Memory/TCP server adapters are not client connectors. Implement the client-side nonblocking port/queues; preliminary F3 requires accepted S2, while full F3 3.4 still waits for F2 4.2.
- Twenty input actions, exact token rules, finite values, optional source fields, and provenance are closed contracts from 02/05. No worker-local public field, fabricated source fact, implicit Go fallback, or Python authority is permitted.
- Every u64 crossing G1 is canonical checked decimal text; UUIDs are 32 lowercase hex; digests are fixed-length lowercase hex. Core tokens are private nonzero slots plus generations, never pointers.
- Existing fixed ceilings: 128 input actions, 4096 message/mesh work items, 2 MiB protocol body. Proposed limits are measured at 1.1 and frozen at 1.2; 4 MiB is invalid for the 4,325,408-byte aggregate world batch.
- Whole-batch input, complete-observation mirror updates, and whole-frame publication are atomic. Reject before allocation/sequence advancement where the owner requires it; no truncation/coalescing of accepted input, removal, acknowledgment, or terminal work.
- At most three native implementation/review workers at once. A changed shared boundary must first compile and execute deterministic consumer success/failure examples on an accepted SHA. Shared exports/registries/catalogs/integration/derived artifacts have one serial controller.
- New comments are English and contain no applicable task ID. Commits are scoped, one English conventional line. Do not force-push, skip hooks/gates, overwrite user changes, or clean a dirty workdir to make acceptance easier.
- Contract, actual provider/code integration, and native runtime/full integration evidence are separate. A double or compile-only success cannot close a provider or deferred qualification node.
- The user requested planning only. This packet is a future implementation handoff, not authority to start implementation in the planning task.

## Review focus

1. Reset during a fragmented receive or retained outbound head: old tickets, queued data, callbacks and resources cannot enter the new epoch; pin it in 1.5/3.3a/3.3b.
2. Hostile-shaped or numerically valid-looking G1 input: bool-as-integer, leading-zero u64, overflow, extra fields and irrelevant tokens call C1 zero times; pin it in 3.1b.
3. Whole owned bytes, including headers/tags/text/shared allocation ownership: cap plus one preserves the visible frame and admission owner; pin it in 1.1/1.2/2.3/2.4.
4. Required feature failure after earlier dependencies activated: rollback is reverse order, releases consumers first, and does not publish a partial new frame; pin it in 3.2/3.3b.
5. A selected test name with zero actual cases or a stale/Go artifact: discovery/source identity fails acceptance rather than producing a green claim; pin it in entry audit and every closure record.

## Execution ownership and common test cycle

Abbreviations: `C` = `packages/engine/crates/mornlea_client_core`; `G` = `packages/engine/crates/mornlea_godot`. Expand every abbreviated path literally before dispatch. `C` is prospective until 1.2 lands.

Each node below consumes the exact declarations/records in the named packet and the accepted 1.2 contract SHA. A worker brief must contain that SHA, the current implementation baseline, its direct predecessor acceptance, editable/read-only files, and the case row. All other files are read-only. The controller appends evidence and owns status; no worker checks off tasks.

For each behavioral node, perform these five actions, in order:

1. Add the named table-driven assertion to its exact registered test module. Use checked fixtures/helper definitions from 1.2; an absent import or unregistered test is not the intended behavioral red.
2. Discover the filter and run it against the compiling baseline. Capture a nonzero case count and the specified wrong behavior, such as missing removal, sequence consumed on rejection, stale result admitted, or extra host step.
3. Implement only the prescribed provider/state transition in its owned source. Do not change the test oracle, numeric tolerance, shared contract, or another node's files to make it green.
4. Run the same discovered nonempty target, the named read-only Go oracle, and the affected consumer regression. Expected green is all declared assertions passing, with unchanged-state/release counts recorded. Review the final diff independently.
5. Integrate serially, refresh assigned derived consumers, rerun affected gates, append the node evidence, and commit only owned passing work. Record rollback as reverting that node's files; contract rollback includes all consumers. Do not begin a dependent node before the required evidence identity exists.

For node 1.1, the red is a missing source/measurement/prerequisite row rather than product behavior; never call it provider acceptance. Node 1.2 proves constructors/validator and doubles, not live I/O or real providers.

Common Rust commands, with literal target/filter substitutions shown in each node:

```sh
rustup run 1.97.1 cargo test --manifest-path packages/engine/Cargo.toml -p mornlea_client_core --test TARGET --locked FILTER -- --list
rustup run 1.97.1 cargo test --manifest-path packages/engine/Cargo.toml -p mornlea_client_core --test TARGET --locked FILTER
```

`TARGET` and `FILTER` are command-pattern notation only, not runnable placeholders in a dispatched brief. Every node supplies concrete values below. Discover Go selectors with the same package and `-list REGEX`, then run `-run REGEX -count=1`; a relevant zero selector blocks oracle acceptance. Node 1.1 maps each relevant Go test/transcript to its fixture before providers start. Full-package Go execution may supplement a selector but cannot conceal missing semantic oracle coverage.

The ledger record is `{node, baseline_sha, contract_sha, accepted_predecessors, editable_files, read_only_files, fixture_sha256, discovered/executed_cases, red_command/result, green_command/result, compatibility_command/result, failure_owner_unchanged, derived_refresh, independent_review, integration_sha, rollback, deferred_checks}`. Counters/timings in evidence are measured, not new public schema fields.

## Entry audit before node 1.1

Controller only; no implementation files are edited in this audit.

1. Read actual ancestor AGENTS, OpenSpec config and the project orchestration skill/checklist. Inspect `git status`, branch ancestry, in-flight source mtimes/ownership, current ledger routing, and untracked work. Do not adopt an orphan merely because it is old; validate its contract/red test and review it before use.
2. Bind F1 historical acceptance and the current jointly accepted common-plus-Memory-plus-TCP S2 source. Historical S2 `756937af` has 16 parity/81 crate cases; re-bind accepted evidence, not a moving branch head. F3 stale prose saying S2 pending is not a new block.
3. Inspect the reported unpushed Godot commits `1839e3ac`/`ff4bd261` and bridge/catalog/World3D ownership only in the actual selected checkout. Preserve them and report overlap; do not infer they exist in this planning source or overwrite them.
4. Enumerate all hashed/generated/embedded/source-scanned consumers of edited files. Assign the serial refresh owner and downstream gate. If a source-bound registry/hash consumer lies outside this packet's owned files, extend the controller packet before dispatch rather than editing it opportunistically.
5. Inspect all runner callees for hidden Godot launch. Verify toolchain availability. Root AGENTS requires `make rust` before a focused Go check on a clean Rust-related checkout; use the existing build route only after callee review, and report a real toolchain blocker without weakening the later gate.

Stop only affected work for an unresolved accepted-source mismatch, dirty overlap, missing exact required tool, or newly required authority. Continue independent authorized nodes. A failed required gate remains failed/open.

## Contract interfaces frozen by node 1.2

The exact public C1/C2/G1 declarations are 00/02/05/06; the following spells the consumed boundary once for every worker:

```rust
ClientCore::new(config: ClientConfig) -> Result<ClientCore, ClientError>
ClientEndpoint::connect(&mut self, endpoint: Endpoint, identity: ClientIdentity) -> Result<SessionEpoch, ClientError>
ClientEndpoint::submit_input(&mut self, epoch: SessionEpoch, input: InputBatch) -> Result<InputReceipt, ClientError>
ClientEndpoint::step(&mut self, epoch: SessionEpoch, work: ClientWorkBudget) -> Result<StepReport, ClientError>
ClientEndpoint::snapshot(&self, epoch: SessionEpoch) -> Result<Arc<PresentationFrame>, ClientError>
ClientEndpoint::reset(&mut self, epoch: SessionEpoch) -> Result<SessionEpoch, ClientError>
ClientEndpoint::close(&mut self) -> Result<(), ClientError>
InputTranslator::validate_batch(&InputBatch, &ConfirmedMirror, &ClientLimits) -> Result<ValidatedInputBatch, ClientError>
InputTranslator::commit(ValidatedInputBatch, &mut InputAdmissionState) -> Result<InputReceipt, ClientError>
PreparationPort::try_submit(PreparationJob) -> Result<PreparationTicket, RejectedPreparation>
PreparationPort::poll_ready() -> Option<PreparationResult>
PreparationPort::invalidate(SessionEpoch) -> InvalidationReport
ClientCore::prepared_resource(&self, &PreparedResourceKey) -> Result<Arc<PreparedGeometry>, ClientError>
```

`ClientConfig` keeps clock/connector injection native. The Godot checked value supplies registered capability IDs and finite timeout/limit fields, never a Python callback or socket/thread owner. `ClientCore::new` retains its one config argument; deterministic clock/transport substitution is configured through native checked configuration, not a different Python method.

The connector and clock signatures are exact 05 substitution ports: `now() -> Instant`, `try_connect(&Endpoint, &ClientIdentity) -> Result<TransportTicket, ClientError>`, `poll(TransportTicket) -> TransportPoll`, `try_send(TransportTicket, &[u8]) -> Result<(), ClientError>`, `close(TransportTicket) -> Result<(), ClientError>`. `Frame(Vec<u8>)` is one complete prefix-inclusive v45 frame. F1 exposes `ServerPacket` and `ProtocolCodec::decode_server`; reuse their actual accepted APIs and checked parts, not a new wire decoder.

Internal projection functions use one 1.2-owned `ProjectionView<'a>` immutable context with exact input roles:

| Borrowed input | Defining mutable owner | Exact content and lifetime |
| --- | --- | --- |
| `mirror: &ConfirmedMirror` and `observations: &[AcceptedObservation]` | 1.4 | Current confirmed state; bounded pending typed ServerPacket observations with actual ObservationKey/optional tick and prevalidated actor identity resolution. No second mirror or unbounded packet history |
| `input: &InputProjectionState` | 2.1 | Checked admitted/rejected input records, sequence/receipt metadata and pending local semantic UI cue-source events; no raw device input |
| `player: &PlayerProjectionState` | 2.2 | Checked confirmed/predicted pose, correction, finite look ray, movement and source MiningState; prompt2.7b5 uses this current ray and immutable mirror target lookup |
| `audio: &AudioProjectionState` | 2.8 | Immutable committed epoch-scoped dedup keys, pending predicted cancellations, and source-attributed cue candidates borrowed from1.4/2.1; no device handle |
| `lifecycle: &LifecycleProjectionState` | 1.3 for Open/terminal; 3.3a for Reset/Invalidate/local close | Bounded pending transition records and checked core-owned generation/resource order; no fabricated feature/native handle identity |
| `diagnostics: &DiagnosticProjectionState` | State owners supply counters;2.9 projects | Checked ProducerIdentity and immutable QueueCounters/ErrorClassCounters snapshot from input/I/O/preparation/publication/lifetime owners |
| epoch/revision/candidate frame index/limits | Core controller/publisher | Checked current frame identity and frozen ClientLimits; projections cannot change them |

Each state above is a private contract declared at1.2 and initially exercised by deterministic checked doubles. Its actual owner replaces that double before provider acceptance. AcceptedObservation owns its source key/tick/typed packet/resolved keys; its pending storage and local input/cue/transition metadata count toward measured family/frame-owned byte/count accounting. Shared Arc allocations count once. No successful commit may retain an uncharged event history.

Child headers use the coherent candidate frame epoch/revision, but the original source order is retained privately until assembly. Freeze `OrderedRecord<R> { order: ProjectionOrder, stable_key: StableRecordKey, record: R }`. `ProjectionOrder` is `Confirmed { observation: ObservationKey, record_ordinal: u32 }` or `AfterConfirmed { epoch: SessionEpoch, revision: ConfirmedRevision, local_sequence: u64, record_ordinal: u32 }`. The confirmed key is issued by1.4; record ordinal is the actual typed packet record index, with singleton topic ties resolved by the frozen stable topic key. AfterConfirmed is local/derived attribution issued by the native owner, never a fabricated server tick/event ID. Checked local sequence/ordinal exhaustion returns Capacity before mutation.

Sort confirmed entries by actual observation revision/ordinal, then packet record ordinal, then checked stable key. Local/derived entries sort after confirmed entries at their sampled revision, then by local sequence/record ordinal/stable key. StableRecordKey is the closed typed identity derived from the record: actor kind+dimension+actual ActorId; terrain dimension+TerrainKey; or inventory/world topic tag plus its actual container/event identity where present. Singleton topic ties use the existing union variant order. Never reconstruct order from absent/equal source_tick or from the rebased parent revision. Validate remove-before-reuse against that retained order; a malformed duplicate/ambiguous ordering rejects. Strip this envelope only when emitting the unchanged C2 record vector after serial family assembly.

Frozen private functions all take `&ProjectionView<'_>`: per-kind `project_remote_player`, `project_hostile`, `project_passive`, `project_projectile`, `project_companion`, `project_drop` return `Result<Vec<OrderedRecord<ActorRecord>>, ClientError>`; `project_inventory`, `project_container`, `project_crafting`, `project_furnace` return `Result<Vec<OrderedRecord<InventoryUiRecord>>, ClientError>`; `project_environment`, `project_survival`, `project_chat`, `project_task`, `project_prompt` return `Result<Vec<OrderedRecord<WorldUiRecord>>, ClientError>`. `project_player_view` returns `Result<Vec<PlayerViewRecord>, ClientError>`; `project_diagnostics` returns `Result<Vec<DiagnosticRecord>, ClientError>`; `project_audio` returns `Result<AudioProjection, ClientError>`, where `AudioProjection { records: Vec<AudioCueRecord>, proposed_dedup: AudioDedupDelta }` is a checked private publication proposal, not mutation of the committed audio state.

Serial private signatures: `assemble_actors(epoch: SessionEpoch, revision: ConfirmedRevision, parts: [Vec<OrderedRecord<ActorRecord>>; 6], limits: &ClientLimits) -> Result<Vec<ActorRecord>, ClientError>`; `assemble_inventory_ui` uses `[Vec<OrderedRecord<InventoryUiRecord>>; 4]` and returns `Result<Vec<InventoryUiRecord>, ClientError>`; `assemble_world_ui` uses `[Vec<OrderedRecord<WorldUiRecord>>; 5]` and returns `Result<Vec<WorldUiRecord>, ClientError>`. Parts order is the provider order named in each node; retained order keys, not parts order, determine interleaving. These functions remove envelopes after validation. `assemble_frame(current: &Arc<PresentationFrame>, candidate: PresentationFrame, limits: &ClientLimits) -> Result<Arc<PresentationFrame>, ClientError>` checks complete candidate/next index and returns a prepared immutable Arc without changing current.

2.4 alone owns `prepare_publication` and `commit_publication`. A checked PublicationReservation owns the validated new Arc, preallocated proposed audio dedup state, and consumption cursors for pending observation/input/local-cue/lifecycle records. Reservation checks all bytes/counts and allocates staging state before mutation. The final single-owner critical section has no callbacks, waits or new allocations: it commits staged owner swaps/cursors and then the visible Arc exactly once. Failure or panic before that section changes no visible frame/index, dedup or consumption cursor; there are no fallible operations inside the commit. If snapshot/validation/preallocation fails, events/removals/cancellations stay retry-owned. Reset separately invalidates and releases them by epoch. Projections are pure over immutable borrows. This is not a new external C1/C2 field or consumer acknowledgment API.

The exact private functions in `C/src/presentation/assembly.rs` are `prepare_publication(owners: &PublicationOwners<'_>, candidate: PresentationFrame, audio: AudioDedupDelta, consume: PublicationConsumption, limits: &ClientLimits) -> Result<PublicationReservation, ClientError>` and `commit_publication(owners: &mut PublicationOwners<'_>, reservation: PublicationReservation) -> Arc<PresentationFrame>`.1.2 declares the private types;2.4 implements the functions. `PublicationOwners<'a>` is the controller's exclusive borrowed bundle of visible Arc plus the1.4 observation queue,2.1 pending input/local-cue state,2.8 committed dedup/cancellation state and lifecycle pending state. It grants no authority over the confirmed mirror. `PublicationConsumption` has checked prefix counts/actual source keys for those queues, never arbitrary removal indices. Reservation stages replacement cursor/state values and the new Arc; commit contains only their infallible owner swaps/moves and returns the committed Arc. Test doubles cannot substitute for2.4's real implementation.

At 1.2 freeze the Rust representation of frame families as checked `FamilyFrame { key: FamilyKey, records: FamilyRecords }`, where the closed FamilyRecords variants wrap the ten corresponding typed record vectors in logical family order. `PresentationFrame.families` is the owned vector of those checked entries. The key/variant pairing constructor rejects mismatch. This spells the Rust representation of the existing C2/G1 key-and-records contract without adding any field, family, tag, or behavior to its external schema.

1.2 also defines the four test entrypoints and a deterministic `ReplayHarness` in registered test support. Its fixture-driven operations are connect/submit/step/snapshot/reset/close, clock advance, complete/fragmented receive, send Capacity/Io, delayed preparation completion, and read-only inspection of sequence/queues/release counts. It uses real checked constructors and the accepted frame validator; provider tests replace the contract double with the real provider. Assertions in the tables below are the test-helper input/expected-output contract, not invitation to invent missing behavior.

The input contract also declares private `LocalViewValidity` in2.1's admission owner. It is a bounded epoch-scoped tombstone for the locally closed external ContainerToken attribution, not authoritative inventory/container contents. `validate_batch` keeps its accepted three-argument read-only signature and validates current confirmed token/shape. Before reservation, `commit` simulates action order against a temporary copy of the owner's local validity overlay. CloseContainer marks the current external token closed; any later external-view action in that batch or a subsequent batch before confirmed close/reopen fails InvalidState. OpenContainer admission alone does not revive the old token. Invalid shape/token is InvalidInput, wrong epoch is StaleEpoch, wrong session phase is InvalidState, and a valid over-limit demand is Capacity; none mutates owners.

Only a successful whole input commit swaps the overlay with queue/journal/sequence/local-cue-source metadata. A rejected batch leaves all unchanged.1.4's accepted ContainerClosed removes the confirmed view; a later explicit confirmed reopened/replaced view supplies fresh attribution and permits overlay retirement. An unrelated world observation or update to a still-locally-closed view cannot reopen it. A rejected close command does not silently reopen the user's locally dismissed view. Reset clears the epoch overlay. Crafting still uses its separate token: CloseContainer never fabricates a crafting/workbench reference, and crafting validity changes only through its declared confirmed-view/reset lifecycle.2.1 emits bounded Local cue-source events only from accepted semantic UI actions identified by the1.1 source cue inventory, with a native local sequence; no new wire action/public ingress/device scan code is added.2.8 reads those events without consuming them until2.4 publication succeeds.

## Node packets

### 1.1 Bind prerequisites and measure the complete inventory

**Predecessors:** accepted F1 final and jointly accepted F2 S2. **Editable:** `openspec/changes/rust-client-core/ledger.md`, `testdata/runtime-migration/client/capability-inventory.json`. **Read-only:** F1/F2 source/evidence, all client Go source/oracle packets, C1/C2/G1 packets. **Output:** complete family/action/source/fixture/consumer table and numeric high-water/owned-byte proof.

Steps: bind accepted source/corpus identities; enumerate ten families/twenty actions and every supported packet variant; map real Go oracle cases and hashes; measure queue record/bytes, journal, CPU preparation job/result allocations, per-family records, aggregate frame bytes and pending semantic changes; compare proposals; freeze a compatible proposed contract revision for 1.2. Include 4,325,408-byte world aggregate, 128 actions, mixed input/chat/input, rare actors/tasks/containers, resets and LOD. Missing source/measurement or any supported over-limit row fails the inventory gate, rather than silently disabling the variant.

Run accepted F1/F2 commands from their recorded ledgers on the bound source and `go test ./packages/client/runtime ./packages/client/presentation -count=1`. Do not rerun or claim full F2 4.2. Expected result: every supported row has evidence; no invented capacity. Commit `docs(client-core): bind client contract inventory`; rollback inventory/ledger additions only.

### 1.2 Land checked contracts validator and deterministic consumers

**Predecessor:** 1.1. **Editable:** workspace `packages/engine/Cargo.toml`, `packages/engine/Cargo.lock`, `packages/engine/AGENTS.md`; new `C/Cargo.toml`, `C/AGENTS.md`, `C/src/{lib,contracts,input,prediction,preparation}.rs`, `C/src/session/mod.rs`, `C/src/presentation/{mod,frame,geometry}.rs`; four `C/tests/{session_replay,prediction_replay,presentation_contract,lifecycle_contract}.rs`; registered `C/tests/session_replay/contract_double.rs`, `C/tests/presentation_contract/preparation_port.rs`, and `C/tests/support/mod.rs`. The controller predeclares/registers every later file named below, including `preparation/lod.rs`; providers do not edit exports/test roots.

**Output:** exact 00/02/05 declarations, checked private constructors, ProjectionView/test support, complete F1 mapping table, frame validator, and one accepted contract SHA. Source-specific login/payload parts are compiled from real public getters; every G1 config/action example is registered, not guessed by bridge workers.

| Named contract case | Exact expected assertion |
| --- | --- |
| `contract_double::pending_and_first_observation` | Pending epoch nonzero/revision 0; first complete observation revision 1; no invented world state |
| `contract_double::whole_input_failure_atomic` | Invalid middle action/129th action/token/full journal leaves sequence, queue, journal, mirror unchanged |
| `contract_double::mixed_input_chat` | Empty is Noop; chat-only has no first sequence; input/chat/input has two contiguous sequences and one chat |
| `contract_double::frame_failure_atomic` | Mixed epoch/revision or cap plus one preserves old Arc/frame index; tickless ForgetChunks has None |
| `contract_double::ordered_projection_and_commit` | Interleaved equal/tickless observations retain private order across topic vectors; failed publication consumes no event/removal/dedup proposal |
| `contract_double::local_close_overlay` | Close+later external move rejects the whole batch; committed close blocks a later batch until confirmed close/reopen; mirror contents remain authoritative and unchanged |
| `contract_double::exact_source_mapping` | Every action/region; hostile yaw-only; absent associations; real task states; raw drop dimension; queued receipt is not confirmation |
| `geometry::checked_extreme_chunk` | i32::MAX chunk/index0 yields exact x 34359738352; index98304 InvalidInput; ordinary first/last block values match 05 |
| `preparation_port::rejected_job_retained` | Byte/count plus one returns the whole job, no ticket increment; FIFO/stale invalidation releases once |

First compile declarations, then use a deliberately wrong mixed-frame/non-atomic double for behavioral reds and require the real validator to reject it. Constructor/validator tests include major/minor compatibility, every numeric/text bound plus one, 4,325,408-byte aggregate, and measured limits. Do not label scaffolding compile failures as a behavioral red.

Run `rustup run 1.97.1 cargo metadata --manifest-path packages/engine/Cargo.toml --no-deps --format-version 1`; discover all four test targets with `-- --list`; run `session_replay contract_double`, `presentation_contract geometry`, and `presentation_contract preparation_port` using the concrete common Rust form. Expected: all named cases execute and pass; no network/native provider accepted. Commit `feat(client-core): land checked client contracts`; rollback the contract plus all consumers of that identity, never a worker-local signature workaround.

### 1.3 Login and session observations

**Predecessor:** 1.2. **Editable/test:** `C/src/session/login.rs`, `C/tests/session_replay/login.rs`. **Input/output:** connector/clock/checked identity → pending epoch and exact StepReport/session/terminal records, plus real lifecycle Open when a pending epoch is issued and terminal Close/Invalidate for actual core-owned resources. **Read-only:** accepted S2, Go runtime session and `cmd/mornlea-godot-core/step_test.go`.

Implement state-checked hello/login before play, monotonic deadlines, one terminal publication and one transport release. The lifecycle projection uses a checked local core generation and lists only actually core-owned resource keys; it invents no Python feature/native bridge handle. Table: Memory success admits only after complete login; wrong v45/duplicate login/wrong phase/truncated/2 MiB plus one rejects; disconnect during login terminates once; just-before 5s/10s remains pending and at deadline times out; late login after timeout cannot revive. Add `login::lifecycle_open_terminal`: one Open at revision0 and one ordered terminal lifecycle Close/Invalidate, with real nonempty lifecycle records before2.4. Run `session_replay login`; oracle `go test ./packages/client/runtime -run 'Session|Connect|Disconnect' -count=1`. Expected red: wrong transition/deadline or duplicate terminal; green: exact phase/epoch/release counts. Reset/local close conformance is still3.3a; do not accept those variants from a stub. Commit `feat(client-core): implement client login state`; rollback this provider.

### 1.4 Confirmed mirror and identity resolution

**Predecessor:** 1.2. **Editable/test:** `C/src/session/mirror.rs`, `C/tests/session_replay/mirror.rs`. **Input/output:** complete checked ServerPacket + epoch → one staged/validated mirror commit, current revision and immutable projection context. Only this owner increments confirmed revision.

Validate all packet identities and required prior dimension before changing state. Resolve dimensionless state/despawn to exactly one live typed identity; reject orphan/ambiguous/old generation. Preserve optional tick and actual source order. Table: snapshot→block change→forget; all actor spawn/update/despawn/reuse; duplicate/backward malformed observation unchanged; tickless forget increments once without tick; reset old epoch unchanged. Run `session_replay mirror`; oracle `go test ./packages/client/runtime -run 'Mirror|Chunk|Entity' -count=1` plus each actor mirror case mapped at 1.1. Expected red: partial mirror/revision, stale resurrection, invented dimension. Commit `feat(client-core): implement confirmed mirror`; rollback mirror only, with controller revalidation of consumers.

### 1.5 Client connectors and bounded I O

**Predecessor:** 1.2. **Editable/test:** `C/src/session/io.rs`, `C/tests/session_replay/io.rs`. **Input/output:** native registered Memory/TCP capability → complete framed packets and atomic send/receive queue ownership; exact 05 ticket/connector APIs. Session wiring is a controller-only edit in `session/mod.rs` after provider review.

Implement client-side numeric-IP TCP worker and bounded Memory queue capability without importing server authority. A frame assembler admits complete prefix/body only; work drains FIFO up to budget. Reserve records and bytes before enqueue; retain outbound head after Capacity/Io. Reset invalidates partial receive and old tickets. Table: zero/4096 work retains/drains exactly; 4097 rejects before dequeue; record/byte cap plus one unchanged both directions; fragmented prefix/body then complete frame; malformed/oversized/failed encode; Capacity then retry sends the same complete head once; reset during fragment cannot publish old packet; same Memory/TCP transcript yields same admitted observations. Run `session_replay io`; oracle `go test ./packages/client/cmd/mornlea-godot-core -run 'Step|Input' -count=1`. Loopback fixture peers may test actual client connector I/O, but are not real F2 authority/full integration acceptance. Commit `feat(client-core): add bounded client transport`.

### 2.1 Semantic input and local view tokens

**Predecessor:** 1.2. **Editable/test:** `C/src/input.rs`, `C/tests/prediction_replay/input.rs`. **Interfaces:** exact InputTranslator validate/commit and InputReceipt above, plus the private LocalViewValidity and pending local-cue-source owner defined at1.2. Validate confirmed shape read-only; simulate local ordered close validity in commit before allocation; encode/reserve tentative buffers; then one owner commits sequence/journal/queue/overlay/local cue-source metadata. Error precedence is the explicit contract above; no owner changes on rejection.

Table: all 20 action variants and every container/crafting region; missing/irrelevant/stale token; finite/ranged control/ray and NaN; empty/chat-only/mixed; 128/129; checked sequence exhaustion; full journal/outbound bytes; close+later external move in one batch rejects without tombstone mutation; committed close rejects a subsequent move before confirmed close/reopen even when the confirmed mirror still has the container; unrelated observation cannot reopen; confirmed close then new view allows its fresh token; rejected close does not silently reopen; reset clears overlay/sequence/local cue attribution. Run `prediction_replay input`; oracle `go test ./packages/client/client -run Input -count=1`. Expected red: consumed sequence/partial send/old view revived; green: exact receipt, overlay and unchanged authoritative mirror. Commit `feat(client-core): implement atomic typed input`.

### 2.2 Prediction and correction replay

**Predecessors:** 1.4, 2.1. **Editable/test:** `C/src/prediction.rs`, `C/tests/prediction_replay/correction.rs`. **Input/output:** immutable confirmed player + accepted journal → attributed predicted pose/correction through accepted numerical API.

Journal by epoch/sequence; acknowledge once, remove rejected intent, replay remaining sequence order from latest confirmed state. Table: three controls with second acknowledged and authoritative correction; duplicate ack unchanged; rejection removes pending prediction/cue; collision; reset/old correction cannot affect new epoch. Compare with independent frozen F1 numerical tolerance and Go transcript, never a newly selected epsilon. Run `prediction_replay correction`; oracle `go test ./packages/client/runtime -run 'Predict|Step' -count=1`. Commit `feat(client-core): implement correction replay`.

### 2.3 Bounded near preparation and resource arena

**Predecessor:** 1.4. **Editable/test:** `C/src/preparation.rs`, `C/tests/presentation_contract/preparation.rs`. **Interfaces:** exact accepted PreparationPort; owned Near/Far jobs/results and safe Rust-only prepared_resource lookup.

Workers own immutable input allocations, never mirror mutation. Static/actual count and byte charge includes queue-held and worker-held ownership; admit all or return full job. Drain FIFO at most meshes; retain geometry until no published frame references or reset; stale epoch/generation/content result releases once. Table: zero/4096/4097; frozen cap plus one/count/checked-byte overflow; shared Arc counted once versus distinct equal allocations; late forget/reset; newer result beats stale; unknown/stale resource lookup; duplicate completion/invalidate is not double release. Run `presentation_contract preparation`; oracle `go test ./packages/client/client -run 'Mesher|Backpressure' -count=1`. Commit `feat(client-core): bound preparation ownership`.

### 2.3b Far LOD preparation

**Predecessors:** 1.2, 1.4, 1.5. **Editable/test:** `C/src/preparation/lod.rs`, `C/tests/presentation_contract/lod.rs`. **Read-only:** queue owner, exports, accepted 05 LodConfig/TerrainKey/numerical facade. **Output:** real bounded far jobs/results; 2.5 integrates with the real queue after 2.3.

Use tile center arithmetic-shift chunk/4; Chebyshev ring lower floor(view_distance/4)+1, upper ceil(view_distance*far_multiplier/4); order distance then x/z; precharge static maximum before dispatch. Keep accepted remainder, remove out-of-ring, reject old seed/config generation. Checked view distance 2..64, multiplier 2..8, steps 2/4/8, source defaults 3/4; byte allowance frozen at 1.1/1.2. Cases `lod::no_near_overlap`, `negative_tile_and_radius_edges`, `disabled_has_no_jobs`, `budget_plus_one_retained`, `old_seed_or_epoch_is_stale`. Run `presentation_contract lod`; `go test ./packages/client/lod -count=1`; `go test ./packages/client/cmd/mornlea/app -run Lod -count=1`, with nonzero cases. Commit `feat(client-core): prepare far terrain tiles`.

### 2.5 Terrain family integration

**Predecessors:** 2.3, 2.3b. **Editable/test:** `C/src/presentation/family_terrain.rs`, `C/tests/presentation_contract/terrain.rs`. **Input/output:** real Near/Far preparation + confirmed mirror → checked TerrainRecord vector, full keys/resource references only.

Publish near Section and far LodTile with matching visibility, epoch/dimension/generation/content revision, material/light. Remove precedes full-key reuse; no GPU handle/Python geometry. Table: near/far coexist without overlap; fluid material; section replacement; stale mesh reset; full-range negative tile; 4096/4097 records and byte cap plus one preserve prior output. Run `presentation_contract terrain`; oracle `go test ./packages/client/mesh -run 'Visibility|Light|Native' -count=1`. Commit `feat(client-core): publish terrain semantics`. P8 realization remains excluded.

### Disjoint actor providers

All consume 1.4 plus the accepted 1.2 ProjectionView/ActorRecord/ActorId schema. Each exact private `project_KIND(&ProjectionView<'_>) -> Result<Vec<ActorRecord>, ClientError>` is defined at 1.2. Sources and other family files are read-only. Use the common five-step behavioral cycle for each separate row, with its own review/commit; these are not one family catch-all.

| Node | Editable source and exact test module | Concrete table and expected behavior | Rust filter and read-only oracle |
| --- | --- | --- | --- |
| 2.6a | `C/src/presentation/actors/remote_player.rs`; `C/tests/presentation_contract/actors_remote.rs` | Spawn/state/remove/reuse; typed UUID and actual dimension; no velocity; invalid transform/late state unchanged | `presentation_contract actors_remote`; `go test ./packages/client/presentation -run Entity -count=1` |
| 2.6c | `C/src/presentation/actors/hostile.rs`; `C/tests/presentation_contract/actors_hostile.rs` | Yaw-only spawn; health/velocity update; no pitch/attacker association from CombatHit; old epoch/reuse | `presentation_contract actors_hostile`; actual hostile mirror transcript mapped by 1.1; `go test ./packages/client/client -count=1` |
| 2.6d | `C/src/presentation/actors/passive.rs`; `C/tests/presentation_contract/actors_passive.rs` | Graze/health/remove; exact vanished/died reason; no lure/drop inference; late graze/invalid pose | `presentation_contract actors_passive`; actual passive mirror transcript; `go test ./packages/client/client -count=1` |
| 2.6e | `C/src/presentation/actors/projectile.rs`; `C/tests/presentation_contract/actors_projectile.rs` | Launch position/velocity/archetype; state position only; no invented hit/projectile link; duplicate despawn/old epoch | `presentation_contract actors_projectile`; actual projectile mirror transcript; `go test ./packages/client/client -count=1` |
| 2.6f1 | `C/src/presentation/actors/companion.rs`; `C/tests/presentation_contract/actors_companion.rs` | UUID/name/pose/reset; spawn/state/remove/reuse; no task/death fields; malformed/old epoch | `presentation_contract actors_companion`; actual companion mirror transcript; `go test ./packages/client/client -count=1` |
| 2.6f2 | `C/src/presentation/actors/drop.rs`; `C/tests/presentation_contract/actors_drop.rs` | Preserve raw dimension/chunk/slot/generation, stack; split/pickup/reuse; invalid count/index; exact extreme geometry via 1.2 helper | `presentation_contract actors_drop`; actual item-drop mirror transcript; `go test ./packages/client/client -count=1` |

The broad client package oracle does not by itself prove each actor: require the mapped relevant test/transcript/fixture row from 1.1 and nonempty real Rust cases. Commit each as `feat(client-core): project KIND semantics`; rollback its exact row only. No actor worker edits a family assembler or registry.

### 2.6g Assemble the complete actor family

**Predecessors:** 2.6a, 2.6c, 2.6d, 2.6e, 2.6f1, 2.6f2. **Editable/test:** `C/src/presentation/family_actors.rs`, `C/tests/presentation_contract/actors.rs`. **Interface:** `assemble_actors` combines the six checked vectors, validates kind/typed ID/dimension and source order, then full family count/owned bytes.

Table: all six kinds nonempty; equal underlying bytes/digits across different tagged IDs are distinct; interleaved player/hostile/drop observations with equal or absent tick keep their private source-key order after merging; remove before reused identity; duplicate invalid key and cap/byte plus one retain old output. Run `presentation_contract actors`; relevant actual mirror oracles plus `go test ./packages/client/presentation -run Entity -count=1`. Expected red: omitted kind/cross-kind collision/misordered reuse or topic-batched ordering. Commit `feat(client-core): assemble all actor families`.

### 2.6b Player view

**Predecessor:** 2.2. **Editable/test:** `C/src/presentation/family_player_view.rs`, `C/tests/presentation_contract/player_view.rs`. **Interface:** `project_player_view` emits confirmed pose, explicitly attributed optional prediction, checked ray/movement/correction and exact MiningState Idle/Active.

Table: unconfirmed pose never labeled confirmed; correction reason/sequence; zero/NaN/ranged ray; reset; authoritative mining progress is not reconstructed from elapsed local time. Run `presentation_contract player_view`; oracle `go test ./packages/client/runtime -run 'Predict|Step' -count=1`. Commit `feat(client-core): publish player view semantics`.

### Disjoint inventory UI providers

Each row depends directly on 1.4, consumes exact 02/05 checked domain unions/tokens, and returns checked InventoryUiRecord values through its 1.2-declared project function. No renderer/control/item authority. Each row uses its own five-step cycle/review/commit.

| Node | Editable source and test | Named table assertions | Rust filter and Go oracle |
| --- | --- | --- | --- |
| 2.7a1 | `C/src/presentation/inventory_ui/inventory.rs`; `C/tests/presentation_contract/inventory_core.rs` | First/last slot, nine hotbar/27 backpack, full checked stack, malformed count, rejected selection, reset; no unconfirmed debit | `presentation_contract inventory_core`; `go test ./packages/client/client -run Inventory -count=1` |
| 2.7a2 | `C/src/presentation/inventory_ui/container.rs`; `C/tests/presentation_contract/inventory_container.rs` | Chest open→move→close; local token/revision; stale revision/late reply/reset; no fabricated wire revision | `presentation_contract inventory_container`; `go test ./packages/client/client -run 'UI|Chest' -count=1` |
| 2.7a3 | `C/src/presentation/inventory_ui/crafting.rs`; `C/tests/presentation_contract/inventory_crafting.rs` | Personal/workbench checked size; valid/unavailable recipe; separate sequence rejection; revision change; pending action not consumed | `presentation_contract inventory_crafting`; `go test ./packages/client/client -run Craft -count=1` |
| 2.7a4 | `C/src/presentation/inventory_ui/furnace.rs`; `C/tests/presentation_contract/inventory_furnace.rs` | Input/fuel/output, progress below200/burn up to1600; progress→completion, exhausted fuel, closed/stale view | `presentation_contract inventory_furnace`; `go test ./packages/client/client -run Furnace -count=1` |

Commit each as `feat(client-core): project TOPIC ui`; rollback its row only. Use complete existing domain payloads; do not invent equipped-armor arrays or recipe output fields absent from source.

### 2.7a5 Assemble inventory UI

**Predecessors:** 2.7a1–2.7a4. **Editable/test:** `C/src/presentation/family_inventory_ui.rs`, `C/tests/presentation_contract/inventory_ui.rs`. **Interface:** `assemble_inventory_ui` merges one coherent revision, token/outcome union and close/reject-before-open ordering.

Table: accepted move/rejection remain distinct; stale/missing/irrelevant token; close then open interleaved with inventory/crafting/furnace at equal/absent ticks preserves actual source order from envelopes; mixed revision/count/bytes plus one old snapshot intact. Run `presentation_contract inventory_ui`; oracle `go test ./packages/client/client -run 'UI|Inventory' -count=1`. Commit `feat(client-core): assemble inventory ui`.

### Disjoint world UI providers

Rows 2.7b1–b4 depend on 1.4; 2.7b5 depends on 1.4 and 2.2. Each consumes the exact 1.2 project function/schema and immutable source attribution; every row has an independent five-step test cycle/review/commit.

| Node | Editable source and test | Named table assertions | Rust filter and Go oracle |
| --- | --- | --- | --- |
| 2.7b1 | `C/src/presentation/world_ui/environment.rs`; `C/tests/presentation_contract/world_environment.rs` | Day/weather/season/temperature boundary, restore, optional source tick preserved | `presentation_contract world_environment`; `go test ./packages/client/presentation -run Environment -count=1` |
| 2.7b2 | `C/src/presentation/world_ui/survival.rs`; `C/tests/presentation_contract/world_survival.rs` | Damage→death→respawn; health/hunger/armor up to20, oxygen up to300; stale update; no client authority | `presentation_contract world_survival`; `go test ./packages/client/presentation -run HUD -count=1` |
| 2.7b3 | `C/src/presentation/world_ui/chat.rs`; `C/tests/presentation_contract/world_chat.rs` | Valid UTF-8/actual sender ID; each accepted text cap plus one; duplicate event/reset; rejection separate | `presentation_contract world_chat`; `go test ./packages/client/client -run ChatEvents -count=1` |
| 2.7b4 | `C/src/presentation/world_ui/task.rs`; `C/tests/presentation_contract/world_task.rs` | Started→Progress→Completed plus TimedOut/Stopped/Failed; actual ObservationKey; no invented ID/generation/%/Pending/Running; duplicate/old epoch | `presentation_contract world_task`; `go test ./packages/client/client -run ChatEventsTaskLifecycle -count=1` |
| 2.7b5 | `C/src/presentation/world_ui/prompt.rs`; `C/tests/presentation_contract/world_prompt.rs` | Current checked target/revision/registered label, target change/removal, stale/overlong/no target; local derivation explicitly attributed | `presentation_contract world_prompt`; `go test ./packages/client/presentation -run 'Target|Prompt' -count=1` |

Commit each as `feat(client-core): project TOPIC view`; rollback its row. Go selectors with zero relevant cases require a corrected real source oracle in the controller inventory, not a false pass.

### 2.7b6 Assemble world UI

**Predecessors:** 2.7b1–2.7b5. **Editable/test:** `C/src/presentation/family_world_ui.rs`, `C/tests/presentation_contract/world_ui.rs`. **Interface:** `assemble_world_ui` combines checked environment/survival/chat/task/prompt records at one coherent frame revision without a second mirror.

Table: weather/task/chat together; interleaved topic observations with equal/absent tick retain actual source order after envelope merge; derived prompt follows the sampled confirmed revision without invented source event; mixed revision; stale prompt; full count/owned-byte plus one retains old frame. Run `presentation_contract world_ui`; oracle `go test ./packages/client/presentation -run 'HUD|Environment' -count=1`. Commit `feat(client-core): assemble world ui`.

### 2.8 Provenance-aware audio family

**Predecessors:** 1.4, 2.1, 2.2. **Editable/test:** `C/src/presentation/family_audio.rs`, `C/tests/presentation_contract/audio.rs`. **Interface:** `project_audio` returns exact typed AudioCueRecord values; epoch/provenance/cue-specific dedup uses real ObservationKey/event ID, input sequence or local sequence.

Table: confirmed ChatEvent with actual ID duplicate; confirmed combat/place without ID; predicted snow/footstep correction replay emits once; rejected input cancels pending cue; local UI click from a real admitted semantic UI action's native local event; reset clears epoch keys; absent device still generates cue; unknown cue/NaN/bad gain/pitch/cap plus one rejects. `audio::failed_publication_retains_dedup` builds a proposed cue/dedup delta then forces frame failure: no source event/cancellation is consumed, no dedup key commits, and a valid retry emits once before successful publication commits it. Do not correlate combat/projectile/attacker absent from source. Run `presentation_contract audio`; source cue-selection/step transcript mapped at 1.1 plus `go test ./packages/client/runtime -run Step -count=1`. `go test ./packages/client/audio -run Cue -count=1` is supplementary PCM evidence, never provenance acceptance. Commit `feat(client-core): publish cue provenance`.

### 2.9 Bounded diagnostics

**Predecessor:** 1.2. **Editable/test:** `C/src/presentation/family_diagnostics.rs`, `C/tests/presentation_contract/diagnostics.rs`. **Interface:** `project_diagnostics` returns accepted producer/source/contract identity, epoch/frame/tick correlation and owner-maintained counters only.

Table: queue saturation increments exactly once; reset epoch-scoped counters; unknown identity rejected; saturating counter reports incomplete evidence; oversized record leaves old publication. No raw packets/commands, or undeclared timing fields. Run `presentation_contract diagnostics`; oracle `go test ./packages/client/presentation -run Frame -count=1`. Commit `feat(client-core): publish bounded diagnostics`.

### 2.4 Serial whole-frame assembly

**Direct predecessors:** 1.3, 1.5, 2.5, 2.6g, 2.6b, 2.7a5, 2.7b6, 2.8, 2.9. Transitive mirror/input/preparation prerequisites are not optional. **Editable/test:** `C/src/presentation/assembly.rs`, `C/tests/presentation_contract/frame.rs`. **Read-only:** accepted `frame.rs` validator and all provider files. **Interface:** `assemble_frame` → one immutable validated Arc swap; `snapshot` remains safe after later failures.

Combine complete checked families at one current epoch/revision; preserve actual provenance, remove/reuse ordering, empty-family legality and revision-zero restrictions. Compute checked whole-owned size, reserve the PublicationReservation, and then atomically commit owner cursors/dedup plus one visible swap; increment frame index only on success. At this initial gate the real lifecycle provider is1.3's Open/terminal path. Reset/Invalidate/local-close conformance remains unaccepted until3.3a, and full lifecycle-family assembly must be revalidated immediately after3.3a beforeG1. No double closes that gap.

Table: every family present/nonempty across inventory cases, including real lifecycle Open/terminal; one wrong epoch/mixed revision/missing required/duplicate family/invalid number; family+byte cap plus one including world aggregate; interleaved equal/tickless source order; repeated local publication same confirmed revision; old Arc readable; failed preallocation/validation consumes no event/removal/cancellation/dedup key; retry commits once. Run full `presentation_contract` target (no filter); oracle `go test ./packages/client/presentation -run Frame -count=1`. Expected red: partial swap/frame index/event loss; green: one atomic publication transaction. Commit `feat(client-core): publish atomic semantic frames`.

### 3.3a Pure-core lifecycle

**Predecessor:** 2.4. **Editable/test:** `C/src/session/lifecycle.rs`, `C/tests/lifecycle_contract/lifecycle.rs`, and serial controller-owned `C/tests/presentation_contract/frame.rs` for the mandatory integrated lifecycle-family regression. **Read-only:** the accepted2.4 assembly implementation. **Interfaces:** exact reset/close, queue/journal/preparation invalidation and generation rules.

Cases `lifecycle::old_work_cannot_cross_reset`, `pending_login_reset`, `reconnect_after_terminal`, `one_hundred_cycles`; add reset during retained send/fragment and sequence/ticket/generation exhaustion typed failure. This owner completes real lifecycle Reset/Invalidate/local-close projections and clears pending publication/audio/overlay ownership before the next epoch. Add `frame::complete_lifecycle_family` through real core+2.4: Open→Invalidate/Reset→new Open→Close has exact nonempty records, generation/resource order and no stub/old-epoch survival. Deterministic clock/transport/preparation scheduling; no sleeps or Godot. Assert all consumers invalidated before new epoch, release exactly once, terminal once, repeated close succeeds, zero retained stale work after100 pure-core cycles. Run full `lifecycle_contract` and full `presentation_contract` after this node; oracle `go test ./packages/client/runtime -run 'Predict|Lifecycle|Step' -count=1`. This integrated revalidation closes only pure-core lifecycle-family conformance, not100 real Godot processes. Commit `feat(client-core): implement repeatable lifecycle`.

### 3.1a Rust producer descriptors

**Predecessor:** 2.4. **Editable:** `G/src/feature_negotiation.rs`, `G/tests/family_registry.rs`, `apps/mornlea-godot/catalog/capability_registry.tres`, `scripts/godot/capability_registry_check.py`. **Read-only:** accepted C2 records/limits, pilot table, bridge/host/features.

Freeze producer `rust-client-core`, IDs1..10 in session/input/terrain/actors/player-view/inventory-ui/world-ui/audio-cues/lifecycle/diagnostics order, major1/minor0. Descriptor limits come from accepted C2; pilot1..8 stays separate. Cases `family_registry::all_ten_keys`, `producer_scoped_ids`, `missing_and_version_fail_closed`; unknown/duplicate key/ID, wrong major, too-new required minor and symbolic audio/lifecycle. Run `rustup run 1.97.1 cargo test --manifest-path packages/engine/Cargo.toml -p mornlea_godot --locked family_registry`; `make godot-capability-check` after callee review. Commit table/catalog together as `feat(godot): register rust semantic families`; do not enable reserved features.

### 3.1b Safe Rust core adapter and exact owned facade

**Predecessors:** 3.1a, 3.3a. **Editable:** `G/src/{abi,client_core,bridge,lib}.rs`, `G/tests/rust_producer.rs`. **Input/output:** exact08-method argument/result table in06; actual C1/C2 through safe Rust, private CoreToken arena, owned checked boundary values. **Read-only:** host/catalog enabling, old renderer, every core/provider file.

Separate the engine-independent checked decoding/owned intermediate-value/call/lifetime routines from Godot marshalling within these owned files. Exported Godot methods use those same routines; a test-specific second implementation is prohibited. Compile the Godot marshalling code but do not execute engine-dependent Dictionary/Node operations. Rust mode's `open_core` creates the safe Rust core, never Go loader/CoreCalls; old `session_*` stays explicitly pilot-only.

Cases `rust_producer::rust_mode_does_not_load_go`, `whole_frame_failure_atomic`, `method_table_and_owned_values`: actual C1 plus the shared adapter routines; all20 actions/every container region; empty129/altered token; extra/missing field/unknown tag/bool integer/NaN; u64MAX canonical succeeds, MAX+1/leadingzero/sign fails; UUID/hash/result shape; stale slot/generation before lookup; missing/mixed family; Mining Idle/Active and near/far geometry key; rejected decoding makes zero core calls; successful submit exactly one; copy remains valid after Rust frame released; caught panic Internal; repeated close one release. Native Dictionary marshalling is explicitly deferred, not a passing double claim.

Run engine-free discovered Godot crate tests `rustup run 1.97.1 cargo test --manifest-path packages/engine/Cargo.toml -p mornlea_godot --locked rust_producer`; `make godot-capability-check`; `go test ./packages/audit -run 'Architecture|Godot' -count=1` after runner review. Any test that needs an engine is deferred and cannot accept that native behavior. Commit `feat(godot): adapt the safe rust client core`.

### 3.2 Host-owned session and single frame dispatch

**Predecessor:** code-verified 3.1b with accepted C1/C2/G1 schema; full native qualification remains pending. **Editable:** `apps/mornlea-godot/app/host/feature_host.py`, new `apps/mornlea-godot/tests/unit/test_rust_feature_host.py`, `apps/mornlea-godot/config/feature_catalog.tres` only for necessary symbolic/schema wiring; any source-hash consumer from entry audit is controller-owned. Existing Godot Node harnesses are read-only/deferred in this batch.

The native bridge node is acquired by existing scene path/identity cast, avoiding cross-Py4Godot native-object passing. In Rust mode the host owns token/epoch, one step/pull, full result/frame/descriptor validation and one same-frame fan-out via existing `apply_typed_frame`. Features only submit semantic input and apply output. Preserve pilot selection as an explicit complete path; do not silently alias old session names, add `apply_frame`, or change defaults. Reserved14 manifests remain disabled.

The new unit harness imports actual production host logic with py4godot module/Node shell stubs, fake feature resources and a scripted boundary result only; it does not replace host dispatch/rollback logic. Tests: two features cause exactly one bridge step/pull and share frame object identity; no feature drive_session/pull call; failed/incomplete/mixed/stale frame calls no apply and retains last frame/resources; logical audio-cues/lifecycle resolution; duplicate/unknown/version/missing mandatory; cycle; provider-before-consumer activation; required failure rolls back exact reverse trace; optional failure disables dependents with reason; close/reset stale callback.

Run engine-free `python3 -m unittest discover -s apps/mornlea-godot/tests/unit -p 'test_rust_feature_host.py' -v`; `make godot-project-check`; `make godot-capability-check`; pinned `make godot-python-check` only when its existing locked embedded Python is present. The unittest is a separate engine-free host check, never a substitute claim for the missing locked gate. Inspect final runner imports before running. Commit `feat(godot): make the host own rust frame dispatch`. Record real host/Python integration as deferred.

### 3.3b Code-level bridge release order

**Predecessors:** 3.3a, code-verified 3.2. **Editable/test:** `G/src/lifecycle.rs`, `G/tests/lifecycle_contract.rs`, `apps/mornlea-godot/tests/unit/test_rust_feature_host.py`; prospective real `apps/mornlea-godot/tests/scripts/rust_bridge_lifecycle_check.py` remains deferred/not needed for this batch. No P11/P13 ownership work.

Use the actual engine-independent adapter/host release routines and deterministic trace, not a duplicate test implementation. Cases `lifecycle_contract::consumers_before_core`, `late_callback_ignored`, `native_panic_is_internal`; add repeated close and reset with queued input/preparation. Assert consumers first, one native release, stale generation rejected, Internal shape preserves prior visible state, zero retained test-owned handles. Run `rustup run 1.97.1 cargo test --manifest-path packages/engine/Cargo.toml -p mornlea_godot --locked lifecycle_contract` and the engine-free host unittest. Do not run the real bridge harness. Commit `feat(godot): order rust bridge release ownership`; full source node acceptance remains pending wherever native qualification is prescribed.

## Dependency schedule and serial leases

1. Serial entry/1.1/1.2: bind real prerequisites, measure limits, land compiling contracts and test registrations. No provider dispatch before the accepted contract SHA.
2. After1.2: session1.3/mirror1.4/I/O1.5 remain controller-owned shared state work; ready input2.1/diagnostics2.9 can use isolated disjoint files. Do not parallelize edits in workspace/mod/test roots.
3. After1.4 plus each direct predecessor: bounded preparation/prediction, six actor providers, four inventory providers and five world providers may be dispatched individually with a maximum of three workers; the queue/registry/contract owner is never shared. Far2.3b obeys1.5 readiness; prompt/audio wait for prediction/input as listed.
4. Serial terrain2.5, actor2.6g, inventory2.7a5, world2.7b6 assemblies after all children. Player/audio/diagnostics ready independently. Only then serial2.4 whole-frame assembly.
5. After2.4, descriptors3.1a and pure lifecycle3.3a have distinct files but controller integrates/accepts each; adapter3.1b waits for both. Host3.2 and code release3.3b are serial.
6. Re-run code-only aggregate checks on the integrated SHA, record code completion versus still-pending native/F2 gates, and return the portable handoff/evidence.

The shared lease covers Cargo/workspace/exports/test roots, corpus indexes, ledgers, bridge/host/registry/catalogs, runners, manifests, Makefile and CI. Reread the merged source before refreshing derived consumers. Never cherry-pick another controller's F2 work or refresh its evidence in this branch.

## Failure matrix and recovery

| Failure | Owner and decisive assertion | Recovery and stop condition |
| --- | --- | --- |
| Malformed/truncated/wrong-phase/oversized packet | 1.3/1.5 normal decoder; no partial observation | Reject/typed terminal as accepted session policy; retained prior frame; one release |
| Stale epoch/ticket/callback/result | 1.4/2.3/3.3 owners; cannot cross reset | Count stale/release once; continue new epoch without old work |
| Valid input exceeds record/byte/journal/sequence bound | 2.1 admission owner; no sequence/queue mutation | Return Capacity; whole caller batch may be retried only after demand/space changes |
| Send Capacity/Io | 1.5 outbound owner; complete head retained | Retry same head or publish typed terminal by policy; never partial record or silent drop |
| Complete-frame validation/capacity error | 2.4/G1/host; old Arc/index and visible host resources unchanged | Retain required event/removal ownership; correct source/contract or fit accepted next work; no malformed frame reaches consumers |
| Optional/required feature activation error | 3.2; isolate optional or exact reverse required rollback | Stable reason; consumers release before providers; no default fallback |
| Source contract changed or proposed bound too small | Controller only | Pause affected consumers, reconcile design/spec/packets, accept a new contract SHA and revalidate; unaffected nodes continue |
| Missing tool/runtime or engine-dependent required check | Controller evidence | Record exact blocked/deferred gate, never substitute a fake pass; no Godot launch or scope expansion |
| Relevant test discovery is zero | Test/fixture owner | Fix actual selector/fixture mapping before claiming acceptance; compile or broad unrelated green does not close it |
| Dirty ownership overlap or stale parallel-controller artifacts | Controller entry/serial lease | Preserve files, inspect evidence and review before adoption; no broad staging, cleanup or overwrite |

No wall-clock latency/FPS guarantee is invented. Measure fixed replay step time, allocation/high-water, retained queue work and prepared bytes as external evidence. Budget0 retains FIFO work; budget4097 rejects before dequeue. Counter saturation is incomplete evidence. Avoid busy loops/scheduler-dependent sleeps in deterministic tests.

## Code-only completion and deferred nodes

This GLM batch is complete only when every planned core provider/family, safe adapter routine and host code path is implemented, the allowed nonempty checks pass on the integrated source, all relevant derived consumers are current, and the report separately lists every blocked/deferred qualification. Every source node whose full required acceptance was not run stays unchecked in tasks.md. Do not archive F3 or call the product ready.

Run, after callee inspection and tool availability:

```sh
rustup run 1.97.1 cargo fmt --manifest-path packages/engine/Cargo.toml --all --check
rustup run 1.97.1 cargo test --manifest-path packages/engine/Cargo.toml -p mornlea_client_core --test session_replay --locked
rustup run 1.97.1 cargo test --manifest-path packages/engine/Cargo.toml -p mornlea_client_core --test prediction_replay --locked
rustup run 1.97.1 cargo test --manifest-path packages/engine/Cargo.toml -p mornlea_client_core --test presentation_contract --locked
rustup run 1.97.1 cargo test --manifest-path packages/engine/Cargo.toml -p mornlea_client_core --test lifecycle_contract --locked
rustup run 1.97.1 cargo test --manifest-path packages/engine/Cargo.toml -p mornlea_godot --locked
python3 -m unittest discover -s apps/mornlea-godot/tests/unit -p 'test_rust_feature_host.py' -v
make godot-project-check
make godot-capability-check
make godot-python-check
make rust-check
make dev-check
make test-race
go test ./packages/audit -count=1
openspec validate --all --strict --no-interactive
```

If the full Godot crate test command contains an engine-dependent test at execution time, run the discovered engine-free targets and record the full command deferred; never execute the engine to make the aggregate green. If locked embedded Python is absent, that exact Python gate is blocked. The simple host unittest remains valid evidence of its own scope. All commands here are prospective, not runs performed by the planner.

Deferred original gates are preserved:

- 3.3b real native Godot/embedded Python bridge release harness and actual marshalling
- 3.3c rebuilt producer/artifact-bound100 isolated real headless Godot/Python create/login/reset/reconnect/destroy cycles, using future `--core rust --exercise-session` flags only after their owning node lands
- 3.4 real F2 Memory/TCP+C1/C2/G1 all-family integration, after3.3c and F2 4.2; malformed/overflow/save-failure/stale-agent failure injection and source/artifact identities
- 4.1 complete zero-gap real provider/integration inventory; 4.2 full stage gates on its accepted SHA
- P8–P14 real rendering/resources/features/visual/release/default switch and distinct release cycles

These gates are requirements for later F3/product closure, not unrequested work for this code-only batch.

## Portable GLM5.3 handoff prompt

Read the approved active OpenSpec change `openspec/changes/rust-client-core/` in order: proposal, both delta specs, design, tasks, packets00–07. Implement only the complete code-only client-core/adapter/host scope described in07. All ten semantic families and twenty typed input actions are required; do not substitute a first successful slice.

Use a new independent implementation workdir/branch. Inspect actual AGENTS/config/local skills, status, ownership and current accepted F1 plus jointly accepted S2 evidence. The planning source is5994fa26937f68669bbcc2ca2be9bf36bb45a0ef; bind the execution baseline/contract before dispatch. Do not edit or adopt the running F2 branch cursor/rust-authoritative-server-98e6, overwrite existing Godot work, merge, deploy, switch defaults, or launch Godot in any mode. Keep source/save/gameplay versions unchanged and preserve the explicit Go pilot path without fallback.

If you act as a non-OpenAI/unverified controller, follow strict subagent-driven-development with independent implementer/reviewer responsibilities. At most three workers run at once, each with isolated exact ownership and the same accepted shared-contract SHA. Land1.1/1.2 serially before providers; do not ask a worker to invent missing shared behavior. Escalate an actual contract mismatch with source and failing case, revise every affected packet centrally, and accept a new identity before dependent work proceeds.

Use the node's concrete behavioral red/green and named real oracle; discover nonzero tests. Implement only owned files, integrate serially, refresh assigned derived consumers and commit verified scoped nodes. Keep tasks.md as sole status and append ledger evidence. A double is not provider acceptance, safe adapter routine tests are not native marshalling acceptance, and the engine-free host harness is not real Godot/Python qualification.

Stop after complete code implementation and allowed automated evidence. Report integrated source/contract/fixture identities, each implemented node and actual commands/case counts, unchanged-state/release assertions, compatibility results, independent reviews, code-only completion, exact blocked/deferred gates, and scoped rollback. Preserve3.3c/3.4/full-F3/P8–P14 closure as pending. Do not install/sign in to a coding service, transmit credentials, submit externally, or start another app as part of this handoff.

## Planning review and publication

The plan is planning output only. Self-review must trace every approved requirement to its node/case, verify all type names/units and direct predecessor edges, ensure shared-file leases are exclusive and derived consumers are enumerated, and confirm no required field/variant/algorithm is left to provider invention. An independent review checks substantive contradictions; record findings/fixes separately from test evidence.

User review of this detailed plan precedes implementation. The already selected destination is a portable GLM5.3 handoff; do not reopen a coding-app choice or automatically start work. Publish approved final planning documents only on the independent documentation branch based on the fixed planning source, using English active OpenSpec files and unchanged task completion state. Verify exact diff, commit and remote branch; strict OpenSpec validation on the actual diff is required when available, otherwise disclose its exact blocker. No product acceptance is claimed by a documentation push.
