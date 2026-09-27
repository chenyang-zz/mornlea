---
doc_id: runtime-interface-architecture
doc_revision: 2026-09-27.3
language: en
counterpart: runtime-interface-architecture.zh.md
status: target-not-current
---
# Target runtime interface architecture

This is the target interface map for work after the Rust/Godot migration. It is a design contract for planning and review, not a claim that the Rust server, Rust client core, or the listed presentation families already exist. [中文版](runtime-interface-architecture.zh.md). The [target architecture](architecture-target.md) decides language ownership; the [interface-first guide](interface-first-parallel-development.md) decides when a declaration must compile and be accepted before independent implementation starts. Current behavior remains defined by code, tests, and canonical OpenSpec specifications.

## 1. Dependency and authority map

```text
Godot scenes / devices / embedded Python
            │ typed, owned values; no raw pointer retained
            ▼
mornlea_godot (Rust GDExtension; one bridge and family registry)
            │ semantic input batches / immutable presentation frames
            ▼
mornlea_client_core (planned; session, mirror, prediction, frame builder)
            │ v45 protocol frames through Memory or TCP
            ▼
mornlea_server (planned; sole authority and tick owner)
      ├─────┼─────────────┐
      ▼     ▼             ▼
  domain / protocol   storage I/O workers ── versioned save codecs
      │
      └── numerical kernel

Independent Python Agent ── versioned loopback HTTP/MCP ──► server candidate intake
```

`mornlea_domain`, `mornlea_protocol`, and `mornlea_storage` already contain validated Rust value and codec surfaces. `mornlea_server` and `mornlea_client_core` are planned crates. The current `mornlea_godot` calls a Go pilot core; its ABI table is a migration surface, not proof of the target producer. The standalone `mornlea_client` renderer ABI is separate and must never be mistaken for the planned client core. Go remains an offline compatibility oracle and current product path until the staged cutover.

The dependency graph is acyclic: domain has no runtime dependency; protocol depends on domain; storage codecs depend on validated values; kernel owns numerical algorithms; server and client core depend downward on these contracts; Godot adapter depends on client core; Python depends only on typed bridge values. The client core never imports server or storage. Neither Python runtime imports the other.

## 2. Contract register and landing order

Each row names the **one owner** of a boundary. `Existing` means source and tests are present today; `target` means a future task must land the named compile-ready surface and its success/failure double before dependent providers start. A family is enabled only after real producer, real consumer, and integration gates pass on an accepted SHA.

| ID | Contract owner / public surface | Consumers | State and predecessor |
| --- | --- | --- | --- |
| D0 | `mornlea_domain`: checked IDs, locations, values, closed `Command`, `CommandEnvelope`, `Event`, `RoutedEvent` | protocol, server, client core, replay | Existing; extend only with a reviewed behavioral change. |
| P0 | `mornlea_protocol`: v45 packet registry, framing, admission, semantic conversion | server, client core, transport tests | Existing; wire schema stays v45 unless separately changed. |
| S0 | `mornlea_storage`: versioned record codecs and migrations | server persistence worker, offline tooling | Existing codec surface; I/O owner is target. |
| K0 | `mornlea_engine` safe numerical facade and pathfinding | server, client core, preparation workers | Accepted numerical implementation `9b843bbc`; [complete F1 seal](../openspec/changes/rust-runtime-foundation-acceptance/acceptance.json) binds source `d042982d`. No client or server duplicate algorithm. |
| S1 | `mornlea_server::core`: ingress, ordered tick, routed observations | transport adapters, replay, persistence | Target after complete F1; first F2 shared landing. |
| S2 | `mornlea_server::transport`: Memory/TCP framing and session admission | local play, LAN, client tests | Target after S1; one core path. |
| S3 | `mornlea_server::persistence`: async requests, acknowledgments, recovery | server core, activation tooling | Target after S1/S0; one writable world lease. |
| S4 | `mornlea_server::agent`: bounded candidate/result adapter | server core, Agent service | Target after S1; existing HTTP/MCP contract remains independent. |
| C1 | `mornlea_client_core::session`: state, mirror, prediction, correction | Godot adapter, headless replay | Target after F1 and accepted S2 session contract. |
| C2 | `mornlea_client_core::presentation`: immutable frame and bounded preparation | Godot adapter, feature families | Target after C1; one epoch/revision per publication. |
| G1 | `mornlea_godot`: typed bridge, ABI/family negotiation, lifecycle | embedded Python host | Target Rust-core producer after C1/C2; existing Go ABI is pilot only. |
| V1 | terrain, actors, UI, audio, input and lifecycle semantic families | Godot/Python features P8–P11 | Target after C2/G1; individually versioned and disableable. |
| T1 | replay, diagnostics, assets and desktop activation manifests | P12/P13/P14 tooling | Target after the owning runtime contracts; no second authority. |

`S1` and `C1` are *facades*, not blanket plugin interfaces for every gameplay rule. Within an owner, a helper private to one task stays private. A new family changes the register and publishes a small contract packet; it does not widen an untyped `Any` or string-message channel.

## 3. Common identity, versions and value rules

- `PlayerId`, entity IDs, positions, dimensions, finite vectors, display names and bounded text come from `mornlea_domain`; callers use its checked constructors. Do not clone their validation in Python, server transport or Godot.
- `tick: u64` is the authority's simulation tick. `session: u64` is allocated by the server session owner and is distinct from player identity. `sequence: u64` is the client command sequence. `arrival_index: u64` is assigned by server ingress for an admitted tick. Only server ingress may construct `CommandEnvelope`; protocol conversion never fabricates those fields. The existing `order_commands` rule and stale-sequence policy are the ordering oracle.
- `session_epoch: u64` identifies one client-core connection generation; it increases on reset/reconnect and never identifies a server tick. `confirmed_revision: u64` identifies the last fully applied authoritative observation in that epoch. A presentation publication carries both and cannot combine records from different pairs. Revision assignment belongs to the client core; Godot merely checks it.
- Wire compatibility is the protocol version and packet registry, currently v45. Save compatibility is the individual versioned schema, currently player v9, chunk v9, world v6, companions v5, hostile v2, passive v1. Bridge ABI major/minor and each semantic family major/minor are **separate** identities. A bridge minor may add a family or skippable field; changing layout or meaning of an existing required field needs its family major. Save, wire, and ABI numbers never stand in for one another.
- All sizes are explicit: counts are records, lengths are bytes, positions use domain coordinates, time budgets use integer nanoseconds at the API edge, and frame/tick identities are unsigned integers. Reject non-finite numbers, invalid UTF-8, duplicate identities, unknown required families, and arithmetic overflow before publication. No implicit clamping, truncation, lossy conversion, or sentinel ID.

## 4. Failure and publication rules shared by every boundary

The stable failure classes are `InvalidInput`, `IncompatibleVersion`, `InvalidState`, `StaleEpoch`, `StaleRevision`, `Capacity`, `Timeout`, `Cancelled`, `Disconnected`, `Unavailable`, `Io`, and `Internal`. Each boundary maps these to its existing typed error or status code; this list is a semantic taxonomy, **not** a new shared enum or a change to the v45 wire. Preserve the more specific existing `DomainError`, `ProtocolError`, and ABI statuses. No boundary reports success after discarding accepted data.

Validation order is: identity/version and lifecycle; declared count and byte bound; complete field validation; epoch/revision and ordering; required capacity reservation; then state mutation or publication. A failed input batch has no partial effect. For an outbound batch too large for one wire packet, the owner either partitions it into independently valid, ordered complete packets with an explicit resume cursor or rejects it; the protocol adapter never silently truncates a domain event. A full queue returns `Capacity` with a retry rule specific to the contract. Irreversible command outcomes and durable acknowledgments cannot be dropped to keep a renderer responsive. Render-only superseded snapshots may be coalesced only within one epoch, while removals and release actions remain ordered.

No tick, render, or network hot path blocks on file, socket, Agent, GPU, or Python work. Queue sizes, per-call record/byte ceilings, work units, and shutdown deadlines must be part of the compile-ready contract packet for that owner. Existing ceilings are authoritative where code already pins them: protocol frame body `2 MiB`, domain semantic batch `4096` records, pilot bridge input `128` events, pilot world batch `4096` operations, and pilot per-step message/mesh maximum `4096` each. The Godot host currently requests `64` messages and `32` meshes per step; these are **pilot defaults**, not silently inherited Rust-core service guarantees. New server queue and deadline numbers require F2 measured parity evidence before freeze; no guessed global capacity is established by this document.

An immutable publication is owned by its producer until transfer. The receiver either owns a copy or a ref-counted immutable Rust value. Python receives an owned Godot/Python representation; it never retains Rust memory. A bridge pull validates the complete batch before replacing the previously visible snapshot. Cancellation is idempotent, invalidates queued work by epoch or request ID, and cannot cancel an acknowledged durable write without an explicit failure result.

## 5. Authoritative server interfaces

The F2 contract landing defines a safe Rust API with these operations and outcomes; private concrete types are chosen there. The signatures below specify the public *shape* and data ownership and must be reconciled with accepted F1 types before coding:

```rust
ServerCore::open(config: ServerConfig, store: StoreHandle, agent: AgentHandle)
    -> Result<ServerCore, ServerOpenError>;
ServerCore::admit(login: AdmittedLogin, transport: TransportKind)
    -> Result<SessionKey, AdmissionError>;
ServerCore::submit(session: SessionKey, intent: PlayIntent)
    -> Result<SubmissionReceipt, IntakeError>;
ServerCore::submit_companion(candidate: CompanionActionEnvelope)
    -> Result<CompanionReceipt, IntakeError>;
ServerCore::advance_tick(work: TickBudget)
    -> Result<TickPublication, TickError>;
ServerCore::close_session(session: SessionKey, reason: CloseReason)
    -> Result<(), SessionError>;
ServerCore::shutdown(deadline: Deadline)
    -> Result<ShutdownReport, ShutdownError>;
```

`SessionKey` is an opaque server-issued handle; `AdmittedLogin` and `PlayIntent` are existing protocol types. `submit` validates the live session, phase and queue capacity. For `PlayIntent::Sequenced` it also validates sequence and records arrival order; its `SubmissionReceipt::QueuedForTick` means *queued*, never “world mutation succeeded.” `PlayIntent::Chat` follows its own validated text/routing path. `PlayIntent::KeepAliveReply` returns `SubmissionReceipt::ControlAccepted` through session control and never becomes a domain command. `advance_tick` is the only world mutation point: it drains the admitted bounded set, constructs existing domain command envelopes, orders them, executes authoritative rules, and produces a `TickPublication` containing routed domain events, control-plane responses, persistence requests and a work/overflow report. It does not call network, disk, Agent or Python. `EventRecipient::Broadcast` is expanded by the server's session owner, never by domain or protocol. Control-plane hello/login/keepalive/disconnect stays outside `Event`.

`TransportKind::{Memory,Tcp}` changes only byte transport and supervision. Both adapters call the same frame decoder, `validate_hello`, `admit_login`, semantic conversion, `submit`, and outbound encoder. They may not invoke a direct local mutation API. A named `SessionKey` is retired once; stale, duplicate, wrong-phase and post-disconnect messages are refused with stable outcomes. The server bounds pending logins, active sessions, inbound/outbound frames, and slow receivers; it never pauses the tick for one client. Login and handshake deadlines have the current Go path as compatibility evidence (currently 10 s and 5 s respectively); F2 pins exact phase behavior in its contract test.

`StoreHandle` is an async worker mailbox over `mornlea_storage` codecs: `submit(SaveRequest) -> Result<SaveTicket, CapacityOrState>`, `poll(SaveTicket) -> Pending | Durable | Failed(IoError)`, `flush(deadline) -> Result<FlushReport, IoOrTimeout>`. A durable result is emitted only after the required write/commit boundary; a failed read never creates a blank world. Startup acquires an exclusive writable-world lease before reading mutable state. Crash recovery, schema checks, backups and rollback are tested against disposable copies; an incompatible schema or competing writer is a hard failure.

`AgentHandle` issues bounded asynchronous requests over the existing versioned loopback service contract. It returns candidate data with request ID and source tick; timeout/cancel/service errors have no world effect. The current `CommandEnvelope` is bound to a human session and does not represent a companion candidate. S1 therefore admits candidates through a separately typed, sessionless companion ingress carrying companion identity and request provenance. At `advance_tick`, the server checks current authority, permissions, target, range, resources and ordering before the candidate joins the same validated world-mutation pipeline as human input. It does not forge a human session or sequence. Dialogue or summary text is presentation data, not a command bypass.

## 6. Client-core and typed bridge interfaces

The F3 contract landing provides one headless client core. Its public operations are:

```rust
ClientCore::new(config: ClientConfig) -> Result<ClientCore, ClientError>;
ClientCore::connect(endpoint: Endpoint, identity: ClientIdentity)
    -> Result<SessionEpoch, ClientError>;
ClientCore::submit_input(epoch: SessionEpoch, batch: InputBatch)
    -> Result<InputReceipt, ClientError>;
ClientCore::step(epoch: SessionEpoch, work: ClientWorkBudget)
    -> Result<StepReport, ClientError>;
ClientCore::snapshot(epoch: SessionEpoch)
    -> Result<Arc<PresentationFrame>, ClientError>;
ClientCore::reset(epoch: SessionEpoch) -> Result<SessionEpoch, ClientError>;
ClientCore::close() -> Result<(), ClientError>;
```

`connect` starts a nonblocking session attempt and returns its epoch; actual login success or failure arrives through `step`/session observations. The core owns login, packet processing, confirmed mirrors, pending-input journal, reversible prediction, correction/replay, semantic cue selection and preparation scheduling. It applies one complete validated server observation before increasing `confirmed_revision`; stale, duplicate or out-of-order observations follow the protocol/session replay contract and never leak a mixed frame. `InputReceipt` means accepted into the bounded local queue, not server acceptance. A correction discards and replays only the current epoch's pending inputs. Reset invalidates old work, resources and handles before publishing the next epoch. Headless replay must exercise the same path as Godot.

`PresentationFrame` is immutable and contains `{layout_major, layout_minor, session_epoch, confirmed_revision, frame_index, families}`. `frame_index` increases for client-side presentation publications within a confirmed revision; it is not a server tick. Every family payload repeats the epoch and revision in its validated header or is owned as a child of this frame with no independent clock. The frame builder checks every family, total record/byte quota, removal/upsert order and required capacity before the one atomic swap. A device being unavailable does not change the semantic outcome. GPU mesh bytes and resource handles are never exposed as domain or wire values.

`mornlea_godot` is the sole native adapter. It owns `open_core`, `connect`, `submit_typed_input`, `step`, `pull_typed_frame`, `family_table`, `reset`, and `close` as versioned Godot-callable methods. It validates ABI/layout and every count/length before copying to owned Godot values; a panic or invalid handle maps to a stable boundary error. Only it manages native handle lifetime. The embedded Python host negotiates required families before activation, maps frame views to scenes/resources, and can disable one feature without destroying the session. Python never receives packet bytes, save records, raw C symbols, borrowed pointers or mutable Rust buffers.

## 7. Semantic family catalog

This table is the **target logical bridge catalog**. The existing pilot ABI has numeric family IDs `1..8` for identity, connection, input, step, world, frame, status and environment. New numeric ABI IDs and binary layouts are assigned only by the exclusive F3 registry landing after C2; logical names below are stable planning keys, not claims that the pilot registry implements them. A manifest must resolve each logical key to an actual descriptor `{numeric_id, major, minor, record_limit, record_bytes}` and refuse a missing required mapping. This explicitly resolves the current mismatch where symbolic `audio-cues@1.0` and `lifecycle@1.0` requirements cannot be negotiated by a numeric-only host table.

| Logical family | Producer → consumer | Minimum semantic payload and ordering | Feature owner |
| --- | --- | --- | --- |
| `session@1` | client core → host | connection phase, epoch, player identity, terminal reason; transitions are monotonic per epoch | F3/P13 |
| `input@1` | host → client core | ordered typed semantic actions with checked container tokens, action/ray intent, local sequence and receipt; device mapping remains in presentation, whole-batch rejection | F3/P10 |
| `terrain@1` | client core → world feature | dimension, typed near-section/far-tile key and visibility, content revision, generation, typed mesh/light/material references, upserts and removals | P8 |
| `actors@1` | client core → actor features | stable kind/ID, dimension, transform, motion, animation and effect intent, spawn/update/despawn order | P9 |
| `player-view@1` | client core → player feature | confirmed/predicted pose attribution, look target, movement state, correction marker, checked mining state | F3/P9 |
| `inventory-ui@1` | client core → UI feature | selected slot, item stacks, accepted container reference and local confirmed-revision token, crafting/furnace results, rejection and close outcomes | P10 |
| `world-ui@1` | client core → UI feature | time, weather, survival state, chat/task state, interaction prompts, stable display text | P10 |
| `audio-cues@1` | client core → audio feature | cue ID, confirmed observation/optional event identity or predicted input/local event sequence, position or non-spatial flag, category, playback parameters and one-shot dedup key | P11 |
| `lifecycle@1` | client core/bridge → host | epoch open/close/reset, feature activation/release generation and resource invalidation order | F3/P8–P11 |
| `diagnostics@1` | client core/bridge → host | typed queue high-water, rejected-work reason, producer identity, contract version, frame/tick correlation | P12 |

Terrain removals precede reuse of a section key; a late mesh completion is discarded if epoch, chunk generation or content revision no longer matches. Actor despawns and UI container closes cannot be omitted when a later upsert or opening is shown. Audio cue dedup is scoped by epoch and source provenance: actual event identity where present, otherwise the confirmed observation key, predicted input sequence or local event sequence; missing audio hardware suppresses playback only, not the cue/outcome record. Device, texture, scene and audio-resource lifetime belongs to the Godot main-thread owner and is released on reset/feature disable. UI never reconstructs inventory truth from optimistic animation.

For each family landing, the owner publishes an exact field schema with units, valid ranges, ordering, count/byte caps, optional/required fields, compatibility examples, and one invalid/stale/overflow test. A planned family remains disabled until that packet exists and its producer and Godot consumer pass real integration. This is a controlled extension point, not permission to ship an untyped or incomplete family.

The Python feature-host lifecycle already has a structural pilot seam: `validate_feature(feature_id)`, `bind_host(bridge_path)`, `activate_feature(epoch)`, `reset_feature(epoch)` and `deactivate_feature()`. The target host preserves these lifecycle meanings: validate and negotiate all required families before instantiation; activate dependencies before consumers; apply at most one fully validated typed frame per host step; reset providers before consumers; deactivate consumers before providers. Only one feature drives the client session per host step. Optional feature failure disables that feature and its dependents; required feature failure rolls back the activation in reverse order. Feature methods may drive input and apply a frame, but may not own a second network session, mirror, tick, or bridge handle. The F3/G1 landing pins exact Godot-callable method names and return records against this structural contract.

### Functional coverage across the catalog

This matrix prevents a later task from treating a gameplay category as an unowned extension. The existing closed domain command/event sets and v45 registry, rather than this table's wording, determine the current supported payload variants.

| Function group | Authoritative input and result owner | Client/presentation projection |
| --- | --- | --- |
| Login, identity, permissions, disconnect, keepalive | P0 admission plus S1/S2 session control; control packets remain outside `Event` | C1 `session@1`, G1 lifecycle |
| World seed, chunk generation, blocks, lighting, visibility and resync | K0 deterministic algorithms; S1 world state and `ChunkSnapshot`/`BlockChanges`/`ForgetChunks` routing; S3 save | C1 mirror, C2 `terrain@1`, P8 resources |
| Physics, collision, raycast, movement and camera target | K0 numerical work; S1 validates `PlayerControl` and publishes `PlayerState` | C1 prediction/correction, `player-view@1`, P9 scene camera |
| Time, season, weather, temperature, fluid and survival | S1 rules over K0 kernels; `WorldState`/`PlayerState` and block changes | C2 `world-ui@1`, `terrain@1`, `audio-cues@1` |
| Mining, placement, farming, doors, torches and block updates | S1 validates command/resources; publishes world delta and rejection/success | C1 confirmed outcomes, P8 terrain and P10 interaction UI |
| Hotbar, armor, item stacks, crafting, furnaces, chests and containers | S1 owns conservation and permissions; `InventoryState`, `CraftingState`, `FurnaceState`, `ChestState`, `ContainerClosed` | C2 `inventory-ui@1`, P10 Godot Control |
| Drops, remote players, hostiles, passives, projectiles and combat | S1 owns spawn/state/despawn and hit outcomes through closed domain events | C1 identity/order, C2 `actors@1`, P9 scenes/effects |
| Companions, dialogue, tasks and candidate actions | Independent Agent suggests; S4/S1 revalidate and publish companion/chat/task outcomes | C2 `actors@1`/`world-ui@1`/`audio-cues@1` |
| Persistence, migration and recovery | S0 codecs; S3 exclusive I/O and durable acknowledgment | No client save API; P13 activation/rollback diagnostics only |
| Input devices, UI, sound, assets and resource release | C1 validates semantic intent; Godot/Python owns device and visual/audio resources | G1 families and `AssetManifest`; no authoritative device callback |
| Replay, visual evidence, diagnostics and release | T1 offline tools and manifests consume owner outputs | P12/P13/P14 gates; never a second online authority |

## 8. Tooling, assets and deployment seams

- Replay records reference the exact protocol/save/contract identity, seed, ordered input schedule, tick checkpoints and expected authoritative events/state hashes. Tools compare independent offline runs and report uncovered families. They never become a second online writer.
- Diagnostics are bounded observations, not a command bus. Metrics and trace records include owner, epoch or tick correlation and overflow counts; reports with missing identity, truncated cases or I/O errors fail acceptance.
- `AssetManifest` is a versioned **build/distribution contract**, separate from the bridge family table. It records licensed asset identity, content hash, resource kind, compatible material/cue mapping and target platform. Assets are built or synchronized offline from authorized sources; runtime family payloads carry stable references and hashes, not arbitrary paths or file bytes. Missing required assets fail feature activation with a typed reason. Godot releases GPU/audio resources on its owning thread.
- A desktop launcher supervises one Rust server child and one Godot client, uses loopback TCP for local play, pins artifact/contract identity, detects child exit and closes it on teardown. The Memory adapter remains a required F2/F3 parity surface. Activation obtains exclusive world ownership; rollback stops the writer and verifies save compatibility or restores a named backup before previous-runtime start. No automatic protocol/save downgrade or Go fallback is part of the target.

## 9. Parallel implementation protocol and acceptance

```text
accepted D0/P0/S0 + completed K0 (F1)
        │
        ├─ S1 shared server core contract ─► F2 providers S2/S3/S4 ─► serial F2 integration
        │                                    │
        └────────────────────────────────────┴─► C1 shared client contract
                                                  └─► C2 frame contract
                                                       └─► G1 exclusive bridge/registry landing
                                                            ├─ P8 terrain
                                                            ├─ P9 actors
                                                            ├─ P10 UI
                                                            └─ P11 audio
                                                                 └─ P12/P13 acceptance ─► P14
```

Before two tasks consume a new boundary, its owner lands compiling declarations, validated fixtures and a consumer double that executes success and typed failure. The ledger records accepted SHA, exact commands and nonzero tests. Provider tasks then own disjoint files; the registry/adapter and final real producer–consumer test have a serial editor. F2/F3 stage plans remain blocked by their stated prerequisites; this map does not waive them. Every task brief cites the relevant row and frozen family schema, declares editable/read-only paths, source baselines, expected failure/success, limit/error tests and rollback. A discovered mismatch returns to the controller for a revised packet and accepted SHA; workers do not silently broaden a public type.

Acceptance is three separate results: contract/double, real provider, and real integration. For D0/P0/S0 use existing focused tests and compatibility fixtures; for S1–S4 include Memory/TCP parity, deterministic replay, saturation, cancellation, Agent timeout and persistence failure; for C1/C2/G1 include headless correction/replay, mixed-epoch rejection, full-batch bridge validation, feature negotiation and repeated teardown; for P8–P11 include semantic, device/headless and visual evidence where relevant; for P13/P14 include exclusive activation, exported artifact identity and rollback. A screenshot, passing mock, compile-only run or planning checkbox alone is never runtime acceptance.

## 10. Change control and known gaps

This map intentionally freezes **ownership, direction, semantic categories, identity and failure policy** before implementation. Exact Rust declarations, new queue numbers, binary family layouts and numeric IDs become binding only through their specific compile-ready landing and recorded SHA. This distinction avoids claiming an unbuilt API is already callable while still letting later task design use one global interface map.

Verified foundation update: the safe native facade and pathfinding are accepted at `9b843bbc`. The [final acceptance seal](../openspec/changes/rust-runtime-foundation-acceptance/acceptance.json) binds source `d042982d`,112 supported points,1434 cases and zero gaps. Its two `domain.input/45` authority rows retain the real external Go producer; F2 still owns Rust authoritative effects. Remaining gaps: F2 server crate and measured queue/deadline contract; F3 client-core crate and Rust-core Godot producer; target semantic family schemas/registry; symbolic-to-numeric host negotiation for audio/lifecycle; complete real integration evidence. Each gap has one owner above. A future feature missing from this table starts with an OpenSpec delta naming its authority, producer/consumer, semantic family or private surface, version effect, limits, tests and contract landing. It does not fork the existing server, client mirror or host bridge.
