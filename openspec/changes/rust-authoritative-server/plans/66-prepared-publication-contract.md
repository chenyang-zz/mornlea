# Checked prepared frames and qualified publication receipts

**Goal:** Land one compile-ready immutable encoded-frame and explicit enqueue-result contract before actual outbox, transport and encoder consumers use it.

**Architecture:** Existing publication.rs owns the checked off-tick frame factory and an opaque Arc containing canonical bytes plus their actual protocol PacketKey. A narrow PreparedPublicationPort transfers those owners and reports Queued versus Closed; existing PublicationPort remains unchanged. Contract doubles prove caller semantics, not actual authority or transport delivery. Later actual outbox/adapters and background encoder consume this accepted SHA serially. Node3.7p2c, after accepted packet64 extraction and before real publication successors.

Baseline dispatch waits root acceptance of packet64 originalabedad35 plus owned-Go-helper repair/review/full actual suite. Source world/cache/capture, protocol v45 codecs, existing512-frame source outbox and canonical framing are read-only. Packet65 restore edits consume no publication boundary. Crate guide ownership remains serial with packet64. Root owns all actual lifecycle/producer/consumer integration, bounds, ordering, source publication and default executable; this contract never closes full publication/runtime.

## Exact API and representation

Add public items in existing core::publication (no new architectural directory):

```rust
#[derive(Clone)]
pub struct PreparedFrame(/* private Arc<PreparedFrameInner> */);
impl PreparedFrame {
    pub fn encode(codec: &mut mornlea_protocol::ProtocolCodec,
                  packet: &mornlea_protocol::ServerPacket) -> Result<Self, ServerError>;
    pub fn packet_key(&self) -> mornlea_protocol::PacketKey;
    pub fn byte_len(&self) -> usize;
    pub fn as_bytes(&self) -> &[u8];
    pub(crate) fn into_legacy_bytes(self) -> Vec<u8>;
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EnqueueOutcome { Queued, Closed }
pub trait PreparedPublicationPort {
    fn enqueue_prepared(&mut self, session: SessionKey, frame: PreparedFrame)
        -> Result<EnqueueOutcome, ServerError>;
    fn take_prepared_outbox(&mut self, session: SessionKey,
                           max_frames: usize, max_bytes: usize)
        -> Result<Vec<PreparedFrame>, ServerError>;
}
```

PreparedFrameInner owns exactly PacketKey and Vec<u8>. No decoded DTO, chunk capture, authority, codec or mutable bytes are retained. Clone shares the Arc in O(1); as_bytes is an immutable borrow and Send+Sync follows this representation. Debug reports only PacketKey and length, never a whole frame. Do not implement body-scanning equality merely for tests; compare canonical bytes off tick and pointer sharing explicitly. No public raw-byte constructor or unchecked key forging.

encode is explicitly off tick. It calls the existing state::encode_packet after changing ONLY that private function's visibility to pub(crate); its algorithm, callers and exact canonical bytes stay unchanged. Retain packet.key with the returned frame only after successful protocol validation/encoding/framing. Full frame length is at most MAX_FRAME_BYTES+5; malformed payloads retain existing InvalidInput("packet") and never publish a partial owner. Handshake/Login frames may be encoded (the canonical factory is general); enqueue_prepared accepts only ServerToClient/Play and rejects other keys as InvalidInput("packet") before queue mutation. No compressed-byte parity or actual enqueue claim. This contract deliberately reuses the accepted encoder; later off-tick worker owns its codec and extraction.

into_legacy_bytes is crate-private and documented as an off-tick compatibility observation: unwrap a unique Arc and transfer its Vec, otherwise clone the bytes. No actual prepared publication/transport path may call it. It does not create a second queue, modify framing or make an on-tick clone exemption.

## Receipt and reader semantics

The future provider will append only to an exact Active current session with an open outbox. Queued means that exact frame owner was appended to its one FIFO. An already closed/nonactive retained session returns Closed without mutation; saturation closes/retires that receiver with SlowReceiver, discards the overflowing frame and returns Closed. An unknown key returns existing StaleSession. No success inferred from legacy publish()->Ok: source mirror revision may advance only on Queued. Other recipients remain unaffected. A public queued record is not an acknowledgment of socket flush, peer decode, simulation or disk durability.

take_prepared_outbox shares legacy whole-frame budgets/order: max_frames0 takes none; otherwise first whole frame may exceed max_bytes (including zero), later first nonfit stops without skip. Frames transfer whole with no byte clone/encoding. Unknown session -> StaleSession; closed/retired receivers may drain previously queued frames. Queue capacity/membership/retirement remain the actual provider's ownership, never this type/factory.

Exactly FIVE editable crate paths: src/core/publication.rs; src/core/state.rs (encode_packet visibility ONLY); AGENTS.md; tests/server_contract.rs (one registration); new tests/server_contract/prepared_publication.rs. No contracts.rs/transport/world/acquisition/store/Agent/executable/Go/protocol/domain/fixtures/OpenSpec changes. Guide states the checked off-tick frame/explicit receipt boundary and that doubles do not accept the actual outbox. Existing source publication tests and encoder bytes remain unchanged.

## Test-first and validation

1. Missing types/methods are declaration RED only. Add a two-session, capacity2, VecDeque-backed contract double implementing the exact new trait. Frozen double algorithm follows the receipt/reader table above; it represents only queued owners and phase/open flags. Use checked SessionKey values, never raw production access. Execute Frame factory from actual validated CommandRejected packets, two FIFO sequences, one closed/full recipient, peer isolation, non-Play refusal, unknown session refusal, count0/bytes0/first oversized/exact later-fit cases. Mirror-side test updates its local revision ONLY on Queued and remains unchanged on Closed/error. Label all this consumer contract evidence.
2. Actual existing protocol factory proof: encode canonical CommandRejected and LoginSuccess packets; compare complete bytes/key with independently invoked existing protocol codec+write_frame. Decode through read_frame_ref and the actual family decoder; require consumed==frame.len and original identity/value. An actual checked domain air ChunkSnapshot converted through ServerPacket encodes/decodes with the real ProtocolCodec (no manual compressed bytes). Malformed mutable protocol snapshot refuses with InvalidInput("packet") and no owner. The source extraction provider is already accepted separately; this test does not replace its Go evidence.
3. Keep a cloned PreparedFrame after dropping original packet/codec and moving clones through the contract double; actual byte pointer remains shared, key/bytes remain immutable. Compile-time Send+Sync assertion and an owned thread transfer show the frame owns no codec or authority borrow. into_legacy_bytes tests unique transfer and explicit shared compatibility copy off tick; normal double admission/drain uses only frame ownership, never that conversion.
4. Existing actual publication/localremote/replay fixtures remain unchanged and pass: visibility-only state edit cannot alter framing or queue behavior. Added/revised comments English and free of every current OpenSpec task ID. Scope enumeration verifies no production trait implementation or consumer secretly landed here.

Gates: clean make rust; pinned env/isolated target/explicit actual Agent Python; nonempty new server_contract topic plus existing publication/transport-session topics; full lib/contracts/replay/localremote, existing save/acquisition topics; all-target clippy-Dwarnings/workspacefmt/diff/scope/comment-ID. Logs /workspace/scratch/prepared-publication-contract-*.log. Scoped feat(server): define prepared publication delivery contracts, no push/OpenSpec/status. Root independent exact-SHA contract/factory/source review plus cumulative applicable gates before only3.7p2c closes; future actual outbox/adapters/encoder require accepted contract SHA and separate real evidence. Architecture skill: no change.

Readiness: root chooses the exact opaque owner/type/module/factory/trait, protocol key validation, maximum frame bound, immutable/off-tick/legacy conversion semantics, receipt and first-whole-frame reader rules, concrete double versus actual codec evidence, five editable paths and serial provider integration. No provider invents a receipt, changes an existing trait or consumes a prospective adapter. Dispatch waits accepted serial predecessor; no broad node status is inferred.
