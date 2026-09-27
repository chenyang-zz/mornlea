# F3 supporting owned values and source mapping

This completes the supporting declarations used by C1/C2 in F3 1.2. Types below are prospective; the controller lands compiling definitions and validated examples before workers consume them. Accepted `mornlea_domain` values are reused through public getters/parts, not reimplemented from wire bytes. Rust producer descriptors and host APIs remain the serial 3.1a/3.1b boundary. All fields are owned; constructors validate before state mutation.

## Source meaning that must survive the migration

- `ContainerRef` is the accepted Overworld-only `{chunk,kind,slot,generation}` value; it has no wire dimension or chest revision. `ContainerToken.confirmed_revision` is local mirror attribution.
- `WorldState` owns day phase offset below 24000, `world_time_ticks:u64`, accepted weather/season, `season_progress:u8`, `temperature:i8`. `SurvivalState` owns health/hunger/armor points up to 20, oxygen up to 300 and saturation-zero. Reuse these checked domain values.
- `ChatBody::Task` owns companion speaker, command text and `TaskState::{Started,Progress,Completed,TimedOut,Stopped,Failed(TaskFailure)}`. There is no task ID, task generation, progress percentage or Pending/Running wire state. Display observations in accepted order; do not fabricate correlation.
- Inventory has nine hotbar and 27 backpack slots; crafting has nine slots and an output with personal/workbench size; furnace has checked input/fuel/output, progress below 200 and burn ticks up to 1600; chest has 27 items. Those values do not identify a recipe result or equipped armor array. Command rejection and placement success are distinct outcomes with their actual sequence.
- Prompt is mirror-derived presentation of a current checked ray target and registered display name, not an authoritative prompt packet. Predicted pose remains explicitly attributed.

## Exact supporting records

The following signatures and fields are frozen planning decisions. Private constructors enforce constraints; no provider adds a new public field or enum variant.

```rust
pub struct SectionKey { chunk: ChunkPos, section: u8 }
pub struct LightSummary { sky: u8, block: u8 }
pub struct PreparedResourceKey {
    epoch: SessionEpoch, dimension: Dimension,
    key: TerrainKey, generation: u64,
    content_revision: u64, job_id: NonZeroU64,
}
pub struct Pose { position: [f64; 3], yaw: f64, pitch: f64 }
pub struct FiniteRay { origin: [f64; 3], look: LookAngles, reach: f32 }
pub struct MovementIntent { control: Option<PlayerControl>, on_ground: bool }
pub enum CorrectionReason { AuthoritativeReset, AcknowledgedReplay, RejectedInput }
pub struct Correction { last_input_sequence: u64, reason: CorrectionReason }
pub enum UiOutcome {
    Rejected { sequence: u64, reason: RejectReason },
    PlacementAccepted { sequence: u64 },
}
pub type EnvironmentView = WorldState;
pub type SurvivalView = SurvivalState;
pub struct TaskView {
    observation: ObservationKey, companion: CompanionSpeaker,
    command: CommandText, state: TaskState,
}
pub struct PromptView { target: BlockPos, label: BoundedText }
pub enum InventoryUiView {
    Inventory(InventoryState), Crafting(CraftingState), Furnace(FurnaceState),
    Chest(ChestState), Closed(ContainerRef), Rejected(CommandRejection),
}
pub enum WorldUiView {
    Environment(WorldState), Survival(SurvivalState), Chat(ChatEvent),
    Task(TaskView), Prompt(Option<PromptView>),
}
pub enum TextKind { Name, Command, Speech, Target, Control }
pub struct BoundedText { bytes: String, kind: TextKind }
pub struct CueId(u16);
pub struct FiniteUnit(f32);
pub struct FinitePositive(f32);
pub enum ResourceKey {
    InputJournal, PreparationQueue, PresentationFrames, BridgeHandles, Feature(u64),
}
pub struct QueueCounters {
    input: u64, inbound_records: u64, inbound_bytes: u64,
    outbound_records: u64, outbound_bytes: u64,
    journal: u64, preparation: u64,
}
pub struct ErrorClassCounters {
    invalid: u64, capacity: u64, stale: u64,
    disconnected: u64, io: u64, internal: u64,
}
pub struct ProducerIdentity { source_sha: [u8; 20], contract_sha: [u8; 32] }
```

`SectionKey::try_new(ChunkPos,u8)` requires section below 24. `LightSummary::try_new(u8,u8)` requires both at most 15. `Pose::try_new([f64;3],f64,f64)` requires finite values and valid accepted look angles; conversion from source f32 is exact. `FiniteRay::try_new([f64;3],LookAngles,f32)` requires finite origin and positive finite reach at most the accepted authority interaction distance; constructor tests bind that distance to its actual source, never a worker-chosen range. `BoundedText::try_new(String,TextKind)` checks UTF-8 bytes/scalars: Name at most 32 scalars/128 bytes; Command 1024 bytes; Speech 256 bytes; Target 64 bytes matching the current pilot target-name capacity. Control admits the exact protocol control-message set: any valid UTF-8 up to 256 bytes and 256 scalars, including empty, padded and control-containing values. Empty Target means a cleared local target; other kinds delegate trimming/control/emptiness rules to accepted domain constructors. `CueId::try_new(u16)` admits only the inventory's registered cue IDs. `FiniteUnit::try_new(f32)` admits finite 0..=1; `FinitePositive::try_new(f32)` admits finite values greater than zero. Counters use checked/saturating reporting increments with saturation explicitly reported as incomplete evidence, never silent wrap.

`InventoryUiRecord` is `{header,view:InventoryUiView,token:Option<ContainerToken>,outcome:Option<UiOutcome>}`. `WorldUiRecord` is `{header,view:WorldUiView}`. Their closed variants replace loose optional-field combinations in the earlier family table. `PlayerViewRecord.correction` uses `Option<Correction>` and `mining:MiningState` reuses the accepted checked Idle/Active union from PlayerState; no elapsed local time reconstructs authoritative progress. Terrain uses accepted `Dimension`, not an undefined `DimensionId`. Diagnostic record uses `producer:ProducerIdentity`, `frame_index:u64`, the two counter records and its header. `ClientIntentKind` has one unit variant for each of the twenty accepted semantic actions including Chat, in the exact C1 `ClientIntent` order; conversion is exhaustive and the contract test rejects registry drift. Close reasons are the local closed enum `LocalClose|RemoteDisconnect(BoundedText)|LoginRejected(BoundedText)|Timeout|Capacity|Internal`; source disconnect/login-reject text uses Control and preserves accepted text byte-for-byte. Checked structs have private fields and same-named read-only getters; `try_new` or a validated-parts constructor is their only creation path. Boundary validation rechecks all invariants before publication, even for test/deserialized parts. No fake command confirmation is implied by a local receipt.

## C1 substitution and atomic owners

`ClientIdentity` owns the accepted login identity and checked requested view distance; the exact protocol `LoginStart` value is read-only inside it. `Endpoint` is `Memory {connector_id:NonZeroU64}` or `Tcp(SocketAddr)`; Memory connector IDs resolve only through an injected connector registry, never a server dependency. `ClientConfig` owns checked limits, `hello_timeout:Duration` (5 s), `login_timeout:Duration` (10 s) and the connector registry. `ClientCore::new(ClientConfig)` rejects absent Memory connector IDs before creating an epoch. The runtime-injected `MonotonicClock` trait exposes `now() -> Instant`; a connector exposes `try_connect(&Endpoint,&ClientIdentity)->Result<TransportTicket,ClientError>`, `poll(TransportTicket)->TransportPoll`, `try_send(TransportTicket,&[u8])->Result<(),ClientError>`, `close(TransportTicket)->Result<(),ClientError>`. `TransportTicket` privately contains connector ID and nonzero launch generation; generation exhaustion is a typed error and old tickets are rejected after close/reset; `TransportPoll` is `Pending|Connected|Frame(Vec<u8>)|Closed(ClientError)`. Frame is exactly one complete framed v45 packet, including its length prefix, already bounded by the accepted 2 MiB body cap; the core still runs the normal decoder/admission path. `try_send` borrows queued framed bytes and copies them into the provider-owned bounded queue only on success; Capacity/Io retains the entire core queue head for retry or terminal reporting, never a partial record. Production clock uses std::time::Instant and tests inject a deterministic implementation of the same trait. These operations never block the core step; actual TCP provider owns its bounded worker and queues. C1 supplies deterministic transport/clock doubles in 1.2; runtime never falls back to them.

`StepReport` owns `{epoch,confirmed_revision,frame_index,processed_messages:u16,processed_meshes:u16,pending_input:usize,pending_inbound:usize,pending_preparation:usize,terminal:Option<CloseReason>}`. The pending counts describe owned accepted work, not dropped records.

`ConfirmedMirror` privately owns epoch/revision plus separate world, actor, inventory and world-UI confirmed values. Only 1.4 commits a validated complete observation and increments revision. Projections read immutable borrows. `ValidatedInputBatch` privately owns `{epoch,actions:Vec<InputAction>,required_encoded_bytes:usize,sequenced_count:u16,chat_count:u16,required_journal:usize}` after the read-only validator. `InputAdmissionState` is one mutable single-owner borrow of outbound records, journal and `next_sequence:u64`; `commit` checks all capacities and checked contiguous sequence range before any mutation. Tentative sequence allocation then validates/encodes all actions into owned reserved temporary buffers before atomically committing sequence, journal and queue; failure rolls back the reservations and publishes nothing. No projection or Python Control obtains either mutable owner.

## Checked drop location owned by C2

The accepted domain currently has no inverse chunk-index helper. Land the private C2 helper `drop_position(id:DropId,index:u32)->Result<[f64;3],ClientError>` in `src/presentation/geometry.rs` during 1.2; actor workers read it. Reject index at or above 98304. Compute section=index/4096, local=index%4096, x=local&15, z=(local/16)&15, y=-64+section*16+local/256. Compute world x/z in i64 as `chunk*16+local`; convert checked i64 world coordinates directly to f64; the full admitted i32 chunk range times 16 remains exactly representable in f64, so it must not be narrowed to i32. Preserve the DropId raw dimension in ActorDimension::DropRaw; no world normalization.

Examples: chunk(-1,2),index0→[-16,-64,32]; same chunk,index98303→[-1,319,47]; index98304→InvalidInput with previous frame unchanged. Extreme chunk x=i32::MAX produces exact finite coordinate34359738352 and preserves the accepted actor identity. This source-valid mathematical interpretation replaces Go's narrowing wrap; the explicit F3 delta scenario pins it. Ordinary i32-world-coordinate cases compare against `packages/shared/world/chunk.go:BlockPosFromChunkIndex`; extreme cases never reject a protocol-valid identity or claim a narrowed Go coordinate as parity. Tests `geometry::first_last_block`, `geometry::invalid_index`, `geometry::checked_extreme_chunk` compile in the contract landing before actor projection dispatch.

## Terrain geometry and LOD publication

`TerrainKey` is the closed union `Section(SectionKey)|LodTile(TilePos)`; TilePos owns checked i32 x/z in 64-block tile units, not chunk units. `TerrainVisibility` is Near|Far and must match that key tag. TerrainRecord and PreparedResourceKey use `key:TerrainKey` instead of an unconditional section; the resource also pins epoch/dimension/generation/content revision/job ID. A far tile is not published under a fake section identity. Every ordered removal names the full key before reuse.

C2 freezes checked `LodConfig {enabled:bool,view_distance:u8,far_multiplier:u8,step:LodStep,bytes_per_frame:u32}` during 1.2: source view-distance2..64, far multiplier2..8, discrete accepted LodStep2/4/8, and measured existing per-frame byte allowance. Defaults source far multiplier3/step4; loading normalization remains the source policy, not a runtime worker invention. Near resources use section meshes. Far resources use accepted `mornlea_engine::native` LodRequest/LodScratch/NativeLod checked facade and the seed/registry, never Python generation. Center tile is chunk coordinate arithmetic-shift2; far ring is Chebyshev `floor(view_distance/4)+1 <= distance <= ceil(view_distance*far_multiplier/4)`, so it never covers the near disk. Order pending tiles by distance then x/z, charge static maximum bytes before dispatch, retain accepted results, reject stale epoch/seed generation and remove outside-ring tiles. Content revision for procedural far geometry is its checked seed/config generation, explicitly distinct from authoritative near chunk revision. Tests bind Go scheduler/LOD oracle for negative tiles, enabled/disabled, radius edges and no near overlap. P8 reads key/visibility/resource only; it never derives the ring or reads packet/worldgen state.

## Additional required contract examples

In 1.2 execute Started→Progress→Completed task observations without an invented ID/percentage; inventory rejection remains a separate sequence-bound observation; container token revision is local; a source without task generation cannot be rejected on invented generation. Verify a missing actor field stays None and a passive despawn Died does not identify any separate drop. Pin invalid text, each numeric +1, queued receipt versus authoritative outcome, unknown cue and renderer-independent owned frame validation. These cases augment the existing mixed-frame/input/capacity examples and the producer packets. A differing accepted source declaration returns to the controller for a revised packet and new SHA.

## Shared bounded preparation port owned by 1.2

The controller declares this port in `src/contracts.rs` and its success/failure double in `tests/presentation_contract/preparation_port.rs`, registering it serially. Exact owned jobs are `PreparationJob {key:PreparedResourceKey,payload:PreparationPayload}`; payload is `Near(OwnedMeshView)|Far(OwnedLodRequest)`. OwnedMeshView has `blocks:Box<[u16;110592]>`, `heights_present:[bool;9]`, `heights:Box<[[i16;256];9]>`, `section_origin_y:i32`, `registry:Arc<MeshRegistry>` and maps to the accepted borrowed native MeshView only while a worker owns it. OwnedLodRequest has `params:Arc<WorldgenParams>,tile:[i32;2],step:LodStep` and maps to accepted LodRequest during the call. Near requires TerrainKey::Section and matching origin; Far requires TerrainKey::LodTile and matching tile. A checked constructor enforces these pairings and native admitted ranges.

`PreparationTicket` is private NonZeroU64 tied to key epoch/job generation. `PreparedGeometry` is `Near(Vec<MeshQuad>)|Far(Vec<LodQuad>)`; native bounds are enforced before publishing it. `PreparationResult {ticket,key,outcome:Result<Arc<PreparedGeometry>,ClientError>}` retains its exact request identity on success/error. `RejectedPreparation {error:ClientError,job:PreparationJob}` returns the complete owned job on failed admission. `PreparationPort::try_submit(PreparationJob)->Result<PreparationTicket,RejectedPreparation>`, `poll_ready()->Option<PreparationResult>` and `invalidate(epoch:SessionEpoch)->InvalidationReport` never block; InvalidationReport is `{jobs_cancelled:u32,results_stale:u32,bytes_released:u64}`. The real queue owner is node2.3; far provider2.3b uses the accepted deterministic port double, then2.5 integrates both real providers. Providers cannot edit the queue or module exports concurrently.

Capacity is both accepted pending-count and `ClientLimits.preparation_bytes` across queued and worker-held jobs/results. 64 MiB is proposed until1.1 proves supported cases fit; a byte+1, count+1 or checked arithmetic overflow returns the original job with no ticket increment/partial enqueue. Count each shared Arc allocation once while the owner holds it; separately allocated equal payloads count separately. Static charge includes arrays, enum/header/vector payload and actual registry/parameter ownership. A finished geometry is retained in the core's prepared-resource arena until its frame references are removed/reset; published Arc values are immutable. Core exposes safe Rust-only `prepared_resource(&PreparedResourceKey)->Result<Arc<PreparedGeometry>,ClientError>` for G1/P8 native realization; Python receives the typed key only, never this native storage. Late completion is counted stale and released once. `poll_ready` transfers one FIFO result and debits queue ownership; `step` alone controls at most work.meshes drains. Port tests exercise Near/Far matching, returned job on full queue, byte/count+1, old epoch, duplicate ticket, FIFO remainder and zero retained work after invalidation. P8 native bridge acceptance executes the real resource lookup/realization, not a fake key.
