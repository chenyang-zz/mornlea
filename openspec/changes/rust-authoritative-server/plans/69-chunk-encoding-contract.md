# Checked off-tick chunk encoding ownership contract

**Goal:** Land one callable contract for an immutable captured chunk becoming a canonical prepared network frame on a bounded CPU owner. Preserve exact capture identity and the source section-only payload charge for later correlation and source publication budgets.

**Architecture:** New core/chunk_encoding.rs owns checked values and the narrow CPU request port. The actual worker and source publisher are distinct successors consuming this boundary, so accept the compile-ready contract first. Existing ChunkSaveView, PreparedFrame, canonical codec and WorkerLifecycle remain their accepted owners. No actual CPU thread, publication planner, subscription map or executable is introduced here. Node3.7p2ec precedes3.7p2e and future source publisher.

Root uses installed brainstorming/writing-plans and project orchestration readiness. Compared synchronous network_snapshot/encoding on tick, reusing the disk owner and an independent bounded CPU lane: tick violates CPU bounds; disk reuse serializes unrelated durability and has no matching port; select a separate1..2-owner,8-request lane in packet70. This packet lands its contract only. Read-only source evidence /workspace/scratch/chunk-publication-source-trace.log identifies source snapshot/delta receipt rules and current producer gaps; none of those gaps is accepted by this declaration. Existing save capture admits Ready or Unloading; this CPU port encodes a supplied immutable capture and does not decide network eligibility. Source publisher must separately qualify Ready/wanted/generation/capture relevance.

Prerequisites: accepted packet64 network extraction abedad35/5193a2ce integrated57a097f9; packet66 frame contract22d6351d integratede2e1a66a; packet67 actual prepared queue330386b1 integratede1c2e35d, root1120 actual cases pass. Source acceptance baselinea76b769d. Crate guide registration is serial: dispatch only after packet68 source/guide acceptance and root records that baseline here; TCP does not change any consumed encoding type. No prospective provider is callable.

## Exact files and interfaces

Exactly SIX editable crate paths: new src/core/chunk_encoding.rs (types/factory/private factory cases); src/core/mod.rs (one public module registration); src/core/contracts.rs (ONLY add Resource::ChunkEncodes and Operation::Encode variants); AGENTS.md (boundary paragraph); tests/server_contract.rs (one topic registration); new tests/server_contract/chunk_encoding.rs (public contract/factory/consumer cases). World/publication/state/acquisition/generation_worker/transport/store/protocol/domain/Go/executable/dependencies/versioned artifacts and all existing tests are read-only. No new important directory; inherited crate guide owns this module.

Define these exact public values and operations in core/chunk_encoding.rs:

```rust
pub const MAX_CHUNK_ENCODE_REQUESTS: usize = 8;
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct ChunkEncodeRequestId(std::num::NonZeroU64);
impl ChunkEncodeRequestId {
    pub fn try_new(raw: u64) -> Result<Self, ServerError>;
    pub fn get(self) -> u64;
}
#[derive(Debug)]
pub struct EncodedChunkSnapshot {
    capture: ChunkSaveView,
    section_payload_bytes: usize,
    frame: PreparedFrame,
}
impl EncodedChunkSnapshot {
    pub fn encode(capture: ChunkSaveView, codec: &mut ProtocolCodec)
        -> Result<Self, ServerError>;
    pub fn capture(&self) -> &ChunkSaveView;
    pub fn section_payload_bytes(&self) -> usize;
    pub fn frame(&self) -> &PreparedFrame;
    pub fn into_frame(self) -> PreparedFrame;
}
pub enum ChunkEncodePoll {
    Pending,
    Ready(EncodedChunkSnapshot),
    Failed(ServerError),
}
pub trait ChunkEncodePort {
    fn start_encode(&mut self, capture: ChunkSaveView)
        -> Result<ChunkEncodeRequestId, ServerError>;
    fn poll_encode(&mut self, request: ChunkEncodeRequestId) -> ChunkEncodePoll;
    fn cancel_encode(&mut self, request: ChunkEncodeRequestId) -> Result<(), ServerError>;
}
```

All fields are private and there is no unchecked/raw encoded-result constructor, body copy getter or equality-by-revision cache key. Request IDs are a distinct nominal type from disk/generation IDs; zero refuses InvalidInput{field:"chunk_encode_request"}. Factory/getters retain one exact supplied capture owner and one actual prepared frame. Result is not Clone; consumers may explicitly Arc-clone a borrowed frame to multiple receivers. into_frame moves that frame and drops capture ownership; body pointer remains identical. Poll uses whole values; allow a narrowly local large_enum_variant lint only if actual clippy requires it, matching the existing whole-record rationale.

## Frozen factory and request semantics

Factory is explicitly OFF TICK: call capture.network_snapshot()?; convert the checked domain snapshot through ServerPacket::try_from(Event::ChunkSnapshot(snapshot)), mapping conversion refusal to InvalidInput{field:"packet"}; call accepted PreparedFrame::encode(codec,&packet)?; construct the opaque result with the SAME supplied capture and returned section charge. No disk materialize, recompression cache, cloned body, save estimate substitution, runtime authority borrow or retained decoded packet/codec. Existing network error field chunk_network_snapshot and packet errors propagate unchanged. Source section charge differs from frame length, logical packet length and persistence estimate; air charge48.

Port documentation fixes provider obligations before packet70:

1. At most8 queued+started+held-complete requests total across1..2 OS owners, one started job per owner. Caller start/poll/cancel only moves bounded owners; no CPU expansion/packing/framing/codec allocation, disk/authority/socket lock or wait on caller.
2. Admission precedence: stopping/closed -> InvalidState Closing/Closed; capture generation0 OR revision0 -> InvalidInput{field:"chunk_network_snapshot"}; capacity8 -> Capacity{resource:ChunkEncodes,limit:8,observed:9}; checked monotonic ID exhaustion -> Internal{invariant:"chunk encode request space"}. IDs are consumed only on successful admission. Other invalid capture facts fail on the actual CPU factory and are returned as Failed, not a successful frame.
3. A successful start retains supplied capture until whole Ready/Failed transfer or safe cancellation collection. IDs are never recycled. Unknown/consumed/cancelled poll is Pending, matching current CPU-port convention; callers correlate their own records before polling.
4. Unknown cancellation is idempotent Ok. Queued or held-complete cancellation drops that record immediately. Started cancellation suppresses delivery but stays charged until the real reply is collected, including capture/frame ownership. A later current capture with equal key/generation/revision is not interchangeable: compare ChunkSaveView identity.
5. Concrete provider additionally implements existing WorkerLifecycle. stop_new fences admissions; cancel removes idle/completed and marks started; wait(deadline) drains real queued/started work but may retain held completed records; close cancels, drains, disconnects and joins all actual owners before reporting success. Timeout retains handles/ownership for retry. Drop disconnects and claims no join or quiescence. No force cancellation of CPU code or invented completion.

The contract double is labeled separately; it cannot establish these actual thread, cancellation-charge or join claims. Root packet70 fixes the implementation and causal real-worker tests.

## Concrete test-first proofs

Missing declarations are compilation RED only. New contract topic uses the public authority fixture from existing persistence_failure/chunk_view.rs, copied locally without changing that file: AuthorityState with limits(8,4096,512,64,64,1MiB); PreparedChunk::try_new(key,7,RecoveredChunk{air24sections/fixed32drops32furnaces16chests,revision5,persisted5,rewritefalse,recoveredfalse}); TickContext::harness preload the moved Ready base, resident_snapshot, drop context, commit_residents; capture_chunk_snapshot Autosave and extract SaveValue::ChunkView. Label it explicit preparation/capture fixture, not real disk acquisition or a runtime publisher.

- ID0 typed refusal;1/MAX survive exact get/ordering and cannot be passed as ChunkRequestId.
- Actual checked factory air/key(-2,-3)/gen7/rev5 yields section charge48 and real ServerToClient/Play/ChunkSnapshot key. Decode actual framed bytes with read_frame_ref and ProtocolCodec and compare the actual snapshot packet generated from that capture. Frame length is separately observed; no compressed-byte equality claim against Go.
- Independently capture twice without changes: equal numeric identity, distinct capture tokens. Factory retains exactly the first token and refuses to let a consumer accept it as the second. Retained old output remains exact after source authority/codec drop; clone frame then into_frame preserves pointer/key/bytes and drops only metadata ownership.
- A bounded executing ChunkEncodePort double owns a VecDeque of at most8 records and explicit Pending/Ready/Failed outcomes. A consumer helper starts with a retained capture clone, polls by its ID, compares result.capture() to the retained token BEFORE accepting its frame, and reports Internal{invariant:"chunk encode correlation"} on a deliberately substituted equal-revision different token. It demonstrates pending, exact ready, typed failure, cancellation/unknown Pending, full8 Capacity then freed slot. All successful Ready values are actual factory values created off tick by the double harness; no raw opaque construction. This is callable contract/double evidence only.
- Consumer receipt example takes an accepted real factory frame, calls a small labeled publication double with Queued/Closed and advances local mirror revision only on Queued. Do not edit/reimplement actual AuthorityState provider or claim an actual source publisher. Existing real queue contracts run separately.

Private cases in the new production module may use existing pub(crate) ReadyChunk::capture/set_block/mark_blocks_dirty/finish_tick. Pin untouched Indexed4 with unused palette[2,0,1] retaining its actual palette/words, then set first cell1 and finish once to verify actual changed first-appearance packing and old capture independence. A private unknown32767 edit invalidates only its capture; factory returns existing chunk_network_snapshot error, never a packet. Existing world reset_materializations/materializations may prove no disk expansion; no new production observation getter or behavior-changing bypass. Do not mutate private world fields or broaden file scope to manufacture zero identities; existing world tests already cover them.

## Gates and root ownership

Pinned env/isolated target/baseline make rust, explicit actual Python. Nonzero new public/private contract factory cases; existing prepared_publication/prepared_delivery and network_view tests; full lib/contracts/replay/localremote plus exact live_acquisition::/live_chunk_saves::/chunk_driver:: persistence topics. All-target clippy-Dwarnings, workspacefmt, diff/six-path/current task-ID comment audit. Cargo rebuilds registration and any exhaustive Resource/Operation consumers; new enum variants are server-only, not a protocol/schema/generated refresh. Enumerate existing exhaustive uses before edits and return a verified external consumer conflict to root instead of expanding scope. Go sealed sources unchanged. Logs /workspace/scratch/chunk-encoding-contract-*.log.

Scoped commit feat(server): define checked chunk encoding ownership, no push/OpenSpec/status edits. Independent exact-SHA factory/identity/error/double review and root applicable integrated gates before only3.7p2ec closes; then record accepted SHA in packet70. Actual worker/source publisher/Ready filtering/mirror/runtime/global caps remain open. Root owns serial guide integration and rollback. Architecture skill: no change.

Readiness: exact opaque values/request type, nominal ID/error/capacity/lifecycle semantics, off-tick factory and source charge, immutable correlation, six paths, private versus public fixture/double proof and derived-consumer gates are settled. Packet70 and later source planner share this new boundary and cannot dispatch before its accepted compile-ready SHA. No worker decides publication eligibility, scheduling, overflow policy or runtime behavior.
