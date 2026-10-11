# F3 C1/C2/G1 contract and dispatch graph

The [target interface map](../../../../docs/runtime-interface-architecture.md), F3 delta spec and `design.md` control this plan. F1 D0/P0/S0/K0 zero-gap acceptance and the accepted F2 S2 session contract SHA are entry gates. Final real local/remote integration waits for F2 complete parity. The archived [protocol](../../archive/2026-09-24-rust-protocol-completion/plans/03-server.md) and [storage](../../archive/2026-09-25-rust-storage-codec-closure/plans/05-dispatch-slices.md) packets are format precedents, not current implementation proof. `tasks.md` alone tracks status. The new `mornlea_client_core` crate does not yet exist.

## Compile-ready C1 landing

The controller alone edits `packages/engine/Cargo.toml`, `Cargo.lock`, `mornlea_client_core/src/lib.rs`, `src/contracts.rs`, `src/session/mod.rs`, `src/presentation/mod.rs`, `src/presentation/frame.rs`, crate guides and test entrypoints. Node 1.2 first lands these compiling declarations, the frame validator and a deterministic consumer double; later workers edit only disjoint module/test files in [01-client-slices.md](01-client-slices.md). The [field and provenance packet](02-family-schemas.md) is part of that landing. On acceptance, `ledger.md` records the exact contract SHA. `ClientCore` consumes validated `mornlea_protocol` observations and emits owned immutable publications. It does not import the server implementation, Godot, Python, or GPU code.

```rust
pub struct SessionEpoch(NonZeroU64);
pub struct ConfirmedRevision(u64); // zero until the first complete server observation
pub struct ClientLimits {
    queued_input_events: usize, // default and maximum 128
    inbound_observations: usize,// proposed maximum 4104 queued records
    inbound_bytes: usize,       // proposed maximum 8 MiB encoded ownership
    outbound_commands: usize,   // proposed maximum 4104 queued records
    outbound_bytes: usize,      // proposed maximum 8 MiB encoded ownership
    prediction_journal: usize,  // current pilot ceiling 256 entries
    message_work: usize,        // maximum 4096 per step
    mesh_work: usize,           // maximum 4096 per step
    preparation_results: usize,// proposed maximum 4096
    preparation_bytes: usize,  // proposed maximum 64 MiB job/result ownership
    family_records: usize,     // target maximum 4096 per family per frame
    frame_bytes: usize,        // proposed maximum 8 MiB owned payload
}
pub enum InputReceipt { Noop, Queued { epoch: SessionEpoch, first_sequence: Option<u64>, sequenced_count: u16, chat_count: u16 } }
pub struct ClientWorkBudget { messages: u16, meshes: u16 }
pub trait ClientEndpoint {
    fn connect(&mut self, endpoint: Endpoint, identity: ClientIdentity) -> Result<SessionEpoch, ClientError>;
    fn submit_input(&mut self, epoch: SessionEpoch, input: InputBatch) -> Result<InputReceipt, ClientError>;
    fn step(&mut self, epoch: SessionEpoch, work: ClientWorkBudget) -> Result<StepReport, ClientError>;
    fn snapshot(&self, epoch: SessionEpoch) -> Result<Arc<PresentationFrame>, ClientError>;
    fn reset(&mut self, epoch: SessionEpoch) -> Result<SessionEpoch, ClientError>;
    fn close(&mut self) -> Result<(), ClientError>;
}
```

The 128 input and 4096 per-step work caps are existing pilot ceilings; all queue-byte, 4104 record, 4096 pending-preparation and 8 MiB frame values are proposed target bounds, not claimed pilot guarantees. The Go pilot allows a 4,325,408-byte maximum world batch, so the earlier 4 MiB frame proposal is invalid as a parity bound. Node 1.1 measures high-water and complete owned byte accounting on every supported recorded run; node 1.2 accepts only limits that contain every supported run and have a +1 rejection with no partial publication. A supported replay needing more returns to the controller for one versioned revision of all consumer packets. `ClientLimits::try_new` and `ClientWorkBudget::try_new` check private fields, arithmetic and injected over-limit values before state change. `ClientError` is a closed class set: `InvalidInput`, `IncompatibleVersion`, `InvalidState`, `StaleEpoch`, `Capacity`, `Timeout`, `Disconnected`, `Io`, `Internal`. Validation precedes allocation or sequence advancement; errors never publish a partial frame. `connect` is nonblocking: its epoch identifies a pending attempt, and login succeeds/fails only through `step` with injected monotonic hello/login deadlines. Input receipts attest local admission, not server success. No implicit Go fallback exists.

`PresentationFrame` owns `{layout_major: u16=1, layout_minor: u16=0, session_epoch, confirmed_revision, frame_index: u64, families}`. The frame validator belongs to 1.2; the real assembler and atomic swap belong to 2.4. A frame is atomically swapped only after all family children pass epoch/revision, count/byte and required-family checks. Confirmed revision is zero for a pending connection before any server observation and increases once per complete authoritative observation; only session/lifecycle/diagnostics/local input records may appear at zero. Frame index may increase for local presentation between observations. Old-epoch completions are discarded; duplicate/backward observations never regress a frame. Each family record uses one shared header `{epoch, revision, source_tick: Option<u64>, operation: Upsert|Remove}` because valid packets such as `ForgetChunks` have no source tick; the mirror assigns `ConfirmedRevision` and transport order without inventing one. Remove precedes reuse of the same stable ID. An `Arc` is immutable after publish; the Godot adapter copies into owned host values before Rust memory is released. Preparation may perform bounded CPU work off the main thread; no session, render, disk or network hot path waits on GPU/Python.

## Family field and version packet

The table is the family inventory; [02-family-schemas.md](02-family-schemas.md) defines typed records, units, provenance, identity, byte accounting, validation and compatibility examples. All IDs preserve the accepted F1 typed identity (including UUID-backed player/companion IDs) and are scoped by epoch and dimension where applicable; no generic integer coercion is allowed; all positions are finite world-block `f64` values and rotations finite radians. Lengths and counts are checked before allocation. `String` fields are valid UTF-8 with the existing F1 domain text bounds. Optional fields are explicit tagged values, never magic zeros. Every family starts at semantic major 1, minor 0. The existing pilot numeric IDs 1..8 remain a separate producer table. The exclusive G1 registry owner assigns new numeric descriptors and checks for collisions; feature workers use logical keys, never hardcoded numeric IDs.

| Logical family | Required fields and order | Producer node | Consumer change |
| --- | --- | --- | --- |
| `session@1` | phase, epoch, player ID when admitted, terminal reason; phase is monotonic within epoch | 1.3 | P13 |
| `input@1` | local sequence, semantic action and receipt; whole batch validates before enqueue; device mapping stays in P11 | 2.1 | P10/P11 |
| `terrain@1` | dimension/near-section or far-tile key, visibility, content revision, generation, material class, light/mesh reference and `Remove|Upsert`; removal first | 2.5 | P8 |
| `actors@1` | kind, stable ID, dimension, transform, motion, animation/effect intent, `Remove|Upsert`; despawn first | 2.6a–2.6g | P9 |
| `player-view@1` | confirmed/predicted pose attribution, look target, movement state, correction marker, checked MiningState | 2.2 | P9 |
| `inventory-ui@1` | selected slot, stack list, container token/revision, crafting/furnace outcome, rejection/close | 2.7a1–2.7a5 | P10 |
| `world-ui@1` | time/weather, survival state, chat/task state, prompt and bounded display text | 2.7b1–2.7b6 | P10 |
| `audio-cues@1` | typed confirmed/predicted/local provenance, cue ID, epoch-scoped dedup key, category, optional position, playback parameters | 2.8 | P11 |
| `lifecycle@1` | epoch open/close/reset, feature activation generation, resource invalidation order | 3.3 | P8–P11 |
| `diagnostics@1` | bounded queue high-water, rejected-work class, producer/contract identity, frame/tick correlation | 2.9 | P12 |

Each split family provider owns one `src/presentation/{actors,inventory_ui,world_ui}/<topic>.rs` and its matching file-per-topic test; a serial family assembler owns `family_actors.rs`, `family_inventory_ui.rs` or `family_world_ui.rs`. Simple families retain one `family_*.rs` and one matching test. No provider edits the registry, feature host or another family file. `terrain@1` and `actors@1` are output semantic records; P8/P9 own resource realization and visual completeness separately. The provider must deliver all supported variants in its F3 inventory; a currently unsupported variant remains disabled with an explicit reason. No consumer may infer save or wire data from a family record.

## G1 exclusive bridge and host owner

After C1/C2 providers pass, one controller edits `mornlea_godot` bridge exports, `feature_negotiation.rs`, `abi.rs`, `apps/mornlea-godot/app/host/feature_host.py`, `apps/mornlea-godot/catalog/capability_registry.tres`, `apps/mornlea-godot/config/feature_catalog.tres`, `scripts/godot/capability_registry_check.py`, and audit assumptions. It assigns the new Rust-producer numeric descriptors in one table, resolves symbolic manifest keys to descriptors `{numeric_id, major, minor, record_limit, record_bytes}`, and rejects missing/duplicate/major mismatch/too-new minor before a feature is instantiated. The host calls the one bridge's `open_core`, `connect`, `submit_typed_input`, `step`, `pull_typed_frame`, `family_table`, `reset`, `close`. A failed pull changes no visible frame; a panic is caught at the native boundary and becomes `Internal` with no borrowed pointer escaping. The host drives one session per step, validates required families before scene creation, activates providers before consumers, and releases consumers before providers. The pilot Go path is explicitly selectable for rollback but never automatically selected on Rust failure.

## Parallel and serial gates

```text
F1 complete + F2 accepted S2 → F3 1.1 inventory → F3 1.2 C1/C2 compile-ready contract and validator
                                                   ├─ C1 session/input/prediction/preparation workers
                                                   ├─ C2 terrain/actors/UI/audio family workers
                                                   └─ offline transcript/parity cases
                                → F3 2.4 serial frame assembler → F3 3.1 serial G1 registry/adapter
                                → F3 3.2 host activation → F3 3.3 lifecycle → F3 3.4 real integration (requires F2 complete)
                                → F3 closeout → P8/P9/P10/P11 real feature work
```

P12 evidence tool schema can be developed against the F3 diagnostics contract in parallel with feature implementations after C2/G1 land. P13 launcher/release waits for F2/F3 real integration; P14 default switch waits for all required P8–P13 gates and two distinct release cycles. A compile-ready double is not a real provider or real integration. A feature may write candidate evidence before P12 handoff, but tracked image/producer changes require the existing per-case human review and approval rule.

## Refinement precedence

Read [direct accepted predecessors](03-parallel-readiness.md) and [refined node decisions](04-refined-nodes.md) with this packet. Split parent IDs are retained here only as historical grouping; their child packets own execution. The dependency register controls readiness, and refined source mappings control absent fields. No worker infers a missing signature, capacity, source fact or shared-file edit.

## Current code-only execution refinement

The approved [code-only handoff](07-code-only-implementation-handoff.md) supplements private implementation contracts, file/test ownership and interim evidence without changing the external C1/C2/G1 method/family schemas or weakening full F3 gates. Freeze its source-order envelope, immutable projection inputs, publication transaction, and local view-validity overlay at 1.2 before dispatch. Actual Godot processes are excluded from this batch. Read 07 with 03/04; tasks.md alone carries status.
