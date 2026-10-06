# Protocol contracts

`packages/engine/crates/mornlea_protocol` owns versioned framing, negotiation,
and packet-family codecs. It is a windowless rlib. Production code may depend
on `mornlea_domain` and exactly one compression dependency, `zstd`, and must
not depend on `mornlea_storage`, `mornlea_engine`, `mornlea_client`, or
`mornlea_godot`. Direction is enforced by `tests/runtime_contract.rs`
(`production_manifest_depends_only_on_domain` and
`domain_does_not_depend_on_protocol`), which pins the permitted set exactly
rather than checking that it omits a few names.

## Inventory freeze (`src/lib.rs`, `tests/runtime_contract.rs`)

- Eventual owner of every `protocol.*` inventory row, including framing.
- Registration tests fail if the frozen corpus drops protocol rows or if
  production dependencies reverse onto domain or reach storage/kernel/host
  crates.
- Packet families are ported one inventory row at a time with byte-preserving
  and malformed-input cases; this crate must not infer parity from a covered
  subset.

## Typed packet registry (`src/registry.rs`, `tests/protocol_registry.rs`)

- `Direction::{ClientToServer,ServerToClient}`, `State::{Handshake,Login,Play}`
  and `PacketKey { direction, state, id }` are the three-part key the Go
  `registry.go` freezes. Numeric IDs collide across direction and state by
  design — C/Play/4 is a keep alive reply while S/Play/4 is a command
  rejection — so the complete key, never the bare ID, identifies a family and
  a per-struct `PACKET_ID` cannot serve as the registry key.
- `ClientPacket` and `ServerPacket` are exhaustive enums over exactly the 59
  Go v45 keys: 23 client keys (C/Handshake/0, C/Login/0 and the 21 registered
  C/Play IDs) and 36 server keys (S/Handshake/0..1, S/Login/0..1 and the 32
  registered S/Play IDs). Framing stays outside both enums because the
  registry owns packet keys, not the frame envelope. There is no catch-all
  variant, no `Box<dyn Packet>` and no string-keyed module lookup, so a new
  packet is a new variant plus a new match arm and a missing arm is a compile
  error rather than a silent fallback.
- `decode_client(state, id, payload)` is a free function and
  `ProtocolCodec::decode_server(state, id, payload)` is a method on the owned
  snapshot context; both dispatch over the complete `(state, id)` pair. The
  two client negotiation variants hold the raw `InboundHello` /
  `InboundLoginStart` records rather than the strict outbound types, because a
  peer running another version has to receive the negotiated mismatch answer
  instead of a bare decode failure; every other variant holds its concrete
  packet type. `encode_client_into` / `ProtocolCodec::encode_server_into`
  obtain the key from the variant and delegate to the concrete `encode_into`,
  and because the output path carries no state argument a record cannot be
  encoded under a state it was not decoded under, so wrong-state encoding is
  unrepresentable rather than refused.
- Unregistered keys are `ProtocolError::UnknownPacket` before a DTO is
  published, including the retired C/Play/1, the unassigned C/Play/22 and the
  unassigned S/Play/32. The Go codec's 64 KiB small-payload ceiling
  (`MAX_SMALL_PAYLOAD_BYTES`) is applied by the dispatcher to every family
  except the compressed snapshot, whose own compressed and decoded ceilings
  bound it — the same split the Go decoder makes between its control decoder
  and its snapshot codec.
- The snapshot variant is the one family whose payload length is not
  precomputed, so `encode_server_into` routes it through the codec's owned
  compressor and `decode_server` through its owned decompressor; the
  snapshot-only `decode_snapshot` / `encode_snapshot_into` entry points stay
  in place for the corpus and the group suite.
- `InboundHello` and `InboundLoginStart` carry the crate's common fallible
  surface (`validate` → checked `encoded_len` → `encode_into` → allocating
  `encode`) so the client enum is encodable without a refusal branch. Their
  gates are total and their `encode_into` re-publishes the raw bytes the
  structural decoder admitted: the version rule and the canonical
  display-name rule belong to `validate_hello` / `admit_login` and to the
  outbound `ClientHello` / `LoginStart`, so a raw record restates no admission
  rule.
- The Go `registry.go` table is the read-only source of this map; the runtime
  never reads Go source or the test manifest. `tests/protocol_registry.rs`
  pins the map against one reviewed Go-produced payload per key, asserts the
  23/36 split and the 59-row closure by key and variant rather than by count,
  and pins the three table-tamper detections, the reserved and unassigned ID
  refusals, the complete-key resolution of the C/Handshake/0 versus
  S/Handshake/0 collision, and the old-version hello reaching `validate_hello`
  through the structural path.

## Semantic adapters (`src/semantic.rs`, `tests/protocol_semantic.rs`)

- `PlayIntent` is the client-side semantic surface:
  `Sequenced { sequence: u64, command: DOMAIN::Command }`,
  `Chat(DOMAIN::ChatIntent)` and `KeepAliveReply { token: u64 }`. Three
  `TryFrom` pairs cross the seam: `TryFrom<ClientPacket> for PlayIntent`,
  `TryFrom<ServerPacket> for DOMAIN::Event`, and the outbound
  `TryFrom<PlayIntent> for ClientPacket` / `TryFrom<DOMAIN::Event> for
  ServerPacket`. Errors are `ProtocolError` with explicit mappings, and there
  is no catch-all arm in any direction: a new registry variant or a new domain
  variant is a compile error rather than a silent fallthrough.
- Chat conversion calls checked `ChatIntent::try_new` after the unchanged
  canonical wire command gate; saved task text does not widen client admission.
- The client map is one intent per registered client key: the 19 sequenced
  Play commands map to exactly one `DOMAIN::Command` variant each (the domain
  `input/inventory.rs` payloads are the targets, including the `StackView
  {Inventory, Crafting, Container(ContainerRef)}` split the three
  view-addressed commands need), `ChatCommand` maps only to `Chat`,
  `KeepAliveReply` only to `KeepAliveReply`, and the two negotiation records
  are refused with `UnknownPacket`. The server map is one publication per
  registered server key: the 30 publication packets map to the same-named
  `DOMAIN::Event` variants (`ChatEvent` maps to `Event::Chat` through the
  bidirectional pair in `src/chat_event.rs`), and the six control families
  (`ServerHello`, `HandshakeReject`, `LoginSuccess`, `LoginReject`,
  `KeepAlive`, `Disconnect`) are refused with `UnknownPacket`.
- No conversion invents `session`, `tick`, `arrival_index`, a routing
  recipient or an authoritative result. `PlayIntent::Sequenced` and the
  outbound conversion are the only places the wire sequence is detached and
  reattached, because the domain payload families carry none;
  `DOMAIN::CommandEnvelope` is never constructed here — the envelope is the
  ordering layer's owner of intake metadata — and `RoutedEvent` recipients
  stay outside `ServerPacket`. The `TakeCraftingOutput` nonzero-sequence rule
  is not restated by the adapter: the packet gate refuses a zero sequence and
  the domain envelope refuses it independently, and the adapter carries the
  wire value verbatim for every command that admits it.
- Every inbound arm runs the record's own `validate` first, which is what
  applies the family's wire caps and field rules before any domain value is
  assembled; the checked domain constructors behind the match then restate no
  rule. Every outbound batch arm applies its wire record cap BEFORE any record
  is converted or a buffer reserved, so a domain-valid batch above a cap is
  refused as one packet (`InvalidRange`, the same variant the family gate
  answers a count above its bound with) instead of being truncated into a
  silently partial publication. The caps are 4 companion records
  (`MAX_COMPANION_STATES`), 7 remote-player records
  (`MAX_REMOTE_PLAYER_STATES`), 32 drops (`MAX_ITEM_DROP_BATCH`), 64 hostile
  and 64 passive records, 128 projectile records (`MAX_PROJECTILE_RECORDS`),
  and 4096 block changes / forget chunks (`MAX_BLOCK_CHANGES`,
  `MAX_FORGET_CHUNKS`); the submitted record order is preserved exactly.
- Raw wire exceptions go through the gates earlier nodes published rather
  than through restated rules: a container reference converts through
  `ContainerRef::to_domain_present` (so dimension `256` and `-1` fail rather
  than aliasing into the overworld), a reject reason through
  `reject_reason_from_wire` / `reject_reason_to_wire`, a chat event through
  the `src/chat_event.rs` pair, and chunk sections through the compact
  `TryFrom` conversions in `src/chunk_snapshot.rs` — never by expanding a
  section's 4096 cells. Domain rejections map onto the protocol vocabulary
  through the module-private `wire_error`, which keeps the identity, enum,
  text and float boundaries distinct.

## Framing (`src/frame.rs`, `src/varint.rs`, `src/bytes.rs`, `tests/runtime_contract.rs`, `tests/protocol_frame.rs`)

- `write_frame_into` / `read_frame_ref` are the caller-owned packet boundary.
  `read_frame_ref` returns a borrowed
  `FrameRef { packet_id, payload, consumed }` that aliases the caller's buffer
  and allocates nothing; `write_frame_into` publishes into a caller-owned
  slice and returns the bytes written. `write_frame` / `read_frame` stay the
  allocating compatibility wrappers and delegate to the same validated sizes,
  so the two entry points always agree byte for byte
  (`allocating_wrappers_match_the_caller_owned_paths`).
- The length is a canonical uvarint and does not include itself. Empty,
  oversized, truncated, overlong, and non-canonical length prefixes fail
  before a payload is published
  (`frame_read_rejects_invalid_lengths_before_payload`,
  `frame_read_rejects_truncated_and_noncanonical_packet_id`,
  `frame_write_enforces_maximum_payload`).
- Capacity is `MAX_FRAME_BYTES`; do not copy that number here. The body size
  is validated before the destination is tested, so an invalid frame size is
  reported as such even when the buffer is also short
  (`write_frame_into_reports_an_invalid_size_before_capacity`). A short
  destination is `OutputTooSmall { needed, available }` and leaves every
  destination byte unchanged, while a larger destination is written only in
  `dst[..length]` (`write_frame_into_leaves_a_short_destination_unchanged`,
  `write_frame_into_publishes_a_canonical_frame_into_an_exact_window`).
- The publication order is the crate-wide packet pattern: private
  `publish_packet(length, dst, write)` checks capacity, then hands
  `SliceWriter` a window exactly `length` bytes wide and verifies the final
  cursor in a debug assertion. `SliceWriter` (`src/bytes.rs`) is crate-private,
  never allocates, and performs no semantic validation, so a packet module's
  `validate` → checked `encoded_len` → capacity → publish chain cannot allocate
  or publish half a record.
- Canonical uvarint vectors are pinned by
  `canonical_uvarint_round_trips_and_rejects_malformed` and by
  `canonical_uvarint_lengths_and_malformed_vectors_are_pinned`: the boundary
  lengths are 1 for `0..127`, 2 for `128..16383`, 3 for `16384..2097151`, 4 for
  `2097152..268435455` and 5 otherwise; a fifth data byte above `0x0f` and a
  redundant zero group are refused.
- Coalesced frames consume only one record
  (`frame_round_trip_preserves_packet_id_and_payload`,
  `frame_ref_borrows_the_payload_from_the_caller_buffer`).
- The borrowed read costs no heap allocation after warm-up
  (`borrowed_frame_read_does_not_allocate`). The same test asserts that the
  allocating wrapper still allocates, so the counter cannot pass vacuously.
- `ProtocolError` publishes `UnknownPacket`,
  `OutputTooSmall { needed, available }` and `Allocation` beside the framing
  variants; no localized message text is part of the contract.

## Control packets (`src/server_hello.rs`, `src/handshake_reject.rs`, `src/login_success.rs`, `src/login_reject.rs`, `src/keep_alive.rs`, `src/keep_alive_reply.rs`, `src/disconnect.rs`, `tests/protocol_control.rs`)

- The seven non-gameplay control families share the common fallible surface in
  design §4: `validate(&self)` rechecks every public field on each call,
  checked `encoded_len(&self)` sizes the validated record, `encode_into(&self,
  dst)` publishes into a caller-owned buffer, `encode(&self)` is the allocating
  wrapper over it, and `decode(payload)` stays a bounded read plus `done()` plus
  validation. The infallible `encode`-returning-`Vec` signatures are gone, so a
  record mutated into an invalid value after construction is refused instead of
  silently published, and a short destination reports
  `OutputTooSmall { needed, available }` with every destination byte unchanged.
  An invalid value always wins over a short destination.
- The shared publication half lives in `server_hello.rs` as crate-private
  `publish_packet(length, dst, write)`: capacity check first, then a
  `SliceWriter` window exactly `length` bytes wide, then a debug assertion that
  the published length equals the validated one. The framing module keeps its
  own private copy because its record is the frame envelope rather than a packet
  payload. `SliceWriter` (`src/bytes.rs`) performs no semantic validation.
- The three message-carrying families (`HandshakeReject`, `LoginReject`,
  `Disconnect`) share one message reader in `handshake_reject.rs`:
  `read_control_message` checks the declared length against the 256-byte family
  bound and the bytes that remain before copying anything. A declared length the
  payload cannot complete reports `Truncated`, not the `InvalidString` the
  shared string primitive reports for the same bytes: the Go decoder answers
  that condition with the same sentinel as a malformed UTF-8 message, and the
  frozen corpus category for an incomplete payload is `truncated`. One owner
  keeps that boundary from drifting across the three families
  (`control_an_incomplete_message_payload_is_truncated` in
  `tests/protocol_control.rs`).
- Enum and string bounds are checked before size and capacity, in the Go
  validator's order: the reject code first, then the message bound. `u64`
  tokens and seeds stay little-endian, and the version a handshake reject
  answers with stays informational. `LoginSuccess::validate` is total because
  the identity is checked by `PlayerId` and the seed carries no rule.
- The corpus evidence is executed by `tests/protocol_corpus.rs` through the
  same surface: `protocol.server.{ServerHello, HandshakeReject, LoginSuccess,
  LoginReject, KeepAlive, Disconnect}` and `protocol.client.KeepAliveReply`
  each register a decode and an encode route under the `mornlea_protocol`
  consumer, produced by the real Go codec in
  `packages/tools/cmd/runtime-oracle/protocol_control_test.go`.

## Client control packets (`src/player_input.rs`, `src/place_block.rs`, `src/request_chunk_resync.rs`, `src/select_hotbar.rs`, `tests/protocol_client_control.rs`)

- The four Play client-to-server control families share the common fallible
  surface in design §4: `validate(&self)` rechecks every public field on each
  call, checked `encoded_len(&self)` sizes the validated record,
  `encode_into(&self, dst)` publishes into a caller-owned buffer,
  `encode(&self)` is the allocating wrapper over it, and `decode(payload)`
  stays a bounded read plus `done()` plus validation. The infallible
  `encode`-returning-`Vec` signatures are gone, so a record mutated into an
  invalid value after construction is refused instead of silently published,
  and a short destination reports `OutputTooSmall { needed, available }`
  with every destination byte unchanged. An invalid value always wins over a
  short destination.
- The shared publication half is the control group's crate-private
  `publish_packet(length, dst, write)` in `server_hello.rs`; `SliceWriter`
  (`src/bytes.rs`) performs no semantic validation and publishes the look
  angles as their exact IEEE-754 bits, so a `-0.0` yaw or pitch survives the
  round trip where an `f32 ==` comparison cannot tell the two zeros apart.
- The validation order follows the Go validators: `PlayerInput` checks the
  finite rotation through the domain `LookAngles` and leaves the move axes
  and the pitch unrestricted at the protocol boundary; `PlaceBlock` checks
  the finite rotation and then the domain `HotbarSlot` range; `SelectHotbar`
  checks the slot range; `RequestChunkResync::validate` is total because the
  dimension is the checked domain `Dimension` and the coordinates and the
  held revision carry no rule. The resync decoder matches the raw `i32`
  dimension against the known IDs instead of narrowing it to a `u8`, so a
  value such as `256` is an `InvalidEnum` rather than a reinterpreted
  dimension.
- The corpus evidence is executed by `tests/protocol_corpus.rs` through the
  same surface: `protocol.client.{PlayerInput, PlaceBlock,
  RequestChunkResync, SelectHotbar}` each register a decode and an encode
  route under the `mornlea_protocol` consumer, produced by the real Go codec
  in `packages/tools/cmd/runtime-oracle/protocol_client_control_test.go`. The
  look angles are published and requested as eight-digit hexadecimal bit
  strings, so the corpus carries the bit pattern rather than the number.

## Client hello (`src/client_hello.rs`, `src/admission.rs`, `tests/runtime_contract.rs`, `tests/protocol_admission.rs`)

- Handshake packet ID 0 payload is a canonical protocol-version uvarint.
- `ClientHello::new` / `decode` accept only `Identities::current().protocol`.
  Other versions are `UnsupportedVersion`; trailing bytes and truncated
  varints fail before publication
  (`client_hello_round_trip_preserves_current_version_bytes`,
  `client_hello_rejects_unknown_version_and_malformed_payload`). The strict
  `decode` is the outbound record's convenience path and is never the inbound
  one.
- The strict outbound record rechecks that version on `validate`,
  `encoded_len`, `encode_into`, and fallible `encode`, since its public field can
  be mutated after construction. An invalid version wins over destination
  capacity, and no refused call writes caller bytes.
- `ClientHello::decode_inbound` is the structural inbound path: it applies the
  canonical uvarint and full-consumption rules and keeps the peer's version in
  `InboundHello`, because a peer running another version has to receive the
  negotiated mismatch answer instead of a decode failure. Pure
  `validate_hello` maps any other version to
  `HandshakeRejection::VersionMismatch { server_version }`; no session,
  deadline or send lives in that decision.

## Server hello (`src/server_hello.rs`, `tests/runtime_contract.rs`)

- Handshake packet ID 0 payload is the same canonical protocol-version
  uvarint as ClientHello, on the server-to-client handshake ID space.
- `ServerHello::new` / `decode` accept only `Identities::current().protocol`.
  Other versions are `UnsupportedVersion`; trailing bytes and truncated
  varints fail before publication
  (`server_hello_round_trip_preserves_current_version_bytes`,
  `server_hello_rejects_unknown_version_and_malformed_payload`).

## Handshake reject (`src/handshake_reject.rs`, `src/bytes.rs`, `tests/runtime_contract.rs`)

- Handshake packet ID 1 payload is a canonical uvarint server protocol
  version, a one-byte reject code, and a length-prefixed UTF-8 message.
- Only `HANDSHAKE_VERSION_MISMATCH` (`1`) is a published code. The server
  version is informational and is not required to match the current
  protocol. Unknown codes are `InvalidEnum`; invalid UTF-8, oversized
  declared lengths, and oversized messages are `InvalidString`; trailing
  bytes fail before publication
  (`handshake_reject_round_trip_preserves_golden_bytes`,
  `handshake_reject_round_trip_preserves_empty_message`,
  `handshake_reject_rejects_unknown_code_and_malformed_payload`).
- `ByteEncoder` / `ByteDecoder` are crate-private payload primitives shared
  by later packet families. They are not a public codec surface.

## Login start (`src/login_start.rs`, `src/admission.rs`, `tests/runtime_contract.rs`, `tests/protocol_admission.rs`)

- Login packet ID 0 payload is a 16-byte UUIDv4, a length-prefixed display
  name, and a trailing view-distance byte.
- The identity is the checked domain `PlayerId`. The outbound record applies
  the raw 128-byte bound, then trims by the domain's pinned whitespace set, and
  then admits by the domain's canonical display-name rule (`1..=32` runes,
  `<=128` bytes, no control characters), so the login path shares one lexical
  rule with the other name carriers. View distance is the closed interval
  `2..=64`. Failures are `InvalidIdentity`, `InvalidString`, or `InvalidRange`
  before publication
  (`login_start_round_trip_preserves_golden_bytes`,
  `login_start_rejects_invalid_identity_name_range_and_malformed_payload`).
- The strict outbound encoder repeats the canonical name check before the
  distance check on every call. Its checked length and caller-owned buffer
  publication refuse mutated records with typed errors and never panic or
  partially write. Raw inbound records keep their separate admission path.
- `LoginStart::decode_inbound` is the structural inbound path: it checks the
  64 KiB small-payload ceiling (`MAX_SMALL_PAYLOAD_BYTES`) before any field,
  accepts a canonical length prefix and valid UTF-8 up to that ceiling, and
  keeps the raw identity, name and distance in `InboundLoginStart`. It does not
  apply the canonical name bound before the trim, so a raw name the Go driver
  trims into a canonical name survives decoding.
- Pure `admit_login` owns the inbound decision in the Go driver's order:
  identity bytes first, pinned trim plus canonical display name second, view
  distance third. An invalid identity or name is `LoginAdmissionError::InvalidIdentity`
  and an out-of-domain distance is `ProtocolViolation`, matching the frozen
  `LoginInvalidIdentity` / `LoginProtocolViolation` codes. `AdmittedLogin`
  owns the checked `PlayerId`, the canonical `DisplayName` and the declared
  distance unchanged; clamping is the admission point's decision.

## Inbound admission (`src/admission.rs`, `tests/protocol_admission.rs`)

- Splitting structural decoding from policy is what keeps an inbound record
  answerable: `decode_inbound` never applies a semantic rule, so the peer
  always learns why its record was refused rather than seeing a bare decode
  failure.
- `admission.rs` holds the pure decisions and nothing else: no session, no
  deadline, no timeout, no transport send, no listener. The later server
  chooses when to call `validate_hello` / `admit_login` and owns connection
  lifecycle (F2).
- `tests/protocol_admission.rs` and the package-local Go driver table in
  `packages/shared/network/protocol_admission_oracle_test.go` carry the same
  case identities, so a rejection-order change has to be made on both sides in
  the same change. An old-version hello and an over-long raw name have no
  two-way corpus case, because the Go outbound encoder refuses both.

## Login success (`src/login_success.rs`, `tests/runtime_contract.rs`)

- Login packet ID 0 payload is a 16-byte UUIDv4 followed by a little-endian
  `u64` world seed. Zero seeds are legal. Non-v4 identities, truncated
  payloads, and trailing bytes fail before publication
  (`login_success_round_trip_preserves_golden_bytes`,
  `login_success_rejects_invalid_identity_and_malformed_payload`).

## Login reject (`src/login_reject.rs`, `tests/runtime_contract.rs`)

- Login packet ID 1 payload is a one-byte reject code and a length-prefixed
  UTF-8 message (max 256 bytes/runes). Published codes are the closed
  interval `1..=7` copied from Go `LoginRejectCode`. Unknown codes are
  `InvalidEnum`; invalid UTF-8 and oversized declared lengths are
  `InvalidString`; trailing bytes fail before publication
  (`login_reject_round_trip_preserves_golden_bytes`,
  `login_reject_round_trip_preserves_empty_message_codes`,
  `login_reject_rejects_unknown_code_and_malformed_payload`).

## Disconnect (`src/disconnect.rs`, `tests/runtime_contract.rs`)

- Play packet ID 6 payload is a one-byte disconnect code and a
  length-prefixed UTF-8 message (max 256 bytes/runes). Published codes are
  the closed interval `1..=5` copied from Go `DisconnectCode`. Unknown
  codes are `InvalidEnum`; invalid UTF-8 and oversized declared lengths are
  `InvalidString`; trailing bytes fail before publication
  (`disconnect_round_trip_preserves_golden_bytes`,
  `disconnect_round_trip_preserves_empty_message_codes`,
  `disconnect_rejects_unknown_code_and_malformed_payload`).

## Keep alive (`src/keep_alive.rs`, `tests/runtime_contract.rs`)

- Play packet ID 5 payload is a little-endian `u64` token. Zero tokens are
  `InvalidRange`; truncated payloads and trailing bytes fail before
  publication (`keep_alive_round_trip_preserves_golden_bytes`,
  `keep_alive_rejects_zero_token_and_malformed_payload`).

## Keep alive reply (`src/keep_alive_reply.rs`, `tests/runtime_contract.rs`)

- Play packet ID 4 payload is a little-endian `u64` token. Zero tokens are
  `InvalidRange`; truncated payloads and trailing bytes fail before
  publication (`keep_alive_reply_round_trip_preserves_golden_bytes`,
  `keep_alive_reply_rejects_zero_token_and_malformed_payload`).

## Place block succeeded (`src/place_block_succeeded.rs`, `tests/runtime_contract.rs`, `tests/protocol_player_outcomes.rs`)

- Play packet ID 20 payload is a little-endian `u64` sequence. Zero
  sequences are legal. Truncated payloads and trailing bytes fail before
  publication (`place_block_succeeded_round_trip_preserves_golden_bytes`,
  `place_block_succeeded_rejects_malformed_payload_and_accepts_zero_sequence`).
- The family carries the player and private outcome group's common fallible
  surface with a total value gate: `validate(&self)` rechecks the record on
  every call, checked `encoded_len(&self)` sizes the fixed 8-byte stride,
  `encode_into(&self, dst)` publishes into a caller-owned buffer through the
  crate-private `publish_packet`, `encode(&self)` is the allocating wrapper
  over it, and `decode(payload)` stays a bounded read plus `done()`. The Go
  packet expresses no rule the wire could violate, so the gate admits every
  field state — including a `u64::MAX` sequence — and stays on the surface so
  a later field arrives with a refusal path already in place
  (`player_outcomes_place_block_succeeded_round_trips_through_the_fallible_surface`).
- The corpus evidence is executed by `tests/protocol_corpus.rs` through the
  same surface: `protocol.server.PlaceBlockSucceeded` registers a decode and
  an encode route under the `mornlea_protocol` consumer, produced by the real
  Go codec in
  `packages/tools/cmd/runtime-oracle/protocol_player_outcomes_test.go`.

## Command rejected (`src/command_rejected.rs`, `tests/runtime_contract.rs`, `tests/protocol_player_outcomes.rs`)

- Play packet ID 4 payload is a little-endian `u64` sequence followed by a
  one-byte reject reason. Published reasons are the closed interval
  `1..=15` copied from Go `CommandRejectReasonID`. Unknown IDs are
  `InvalidEnum`; trailing bytes fail before publication
  (`command_rejected_round_trip_preserves_golden_bytes`,
  `command_rejected_round_trip_preserves_frozen_reason_ids`,
  `command_rejected_rejects_unknown_reason_and_malformed_payload`).
- The family carries the player and private outcome group's common fallible
  surface, so a record mutated into an unregistered reason after construction
  is refused instead of silently published with the retired zero byte the Go
  encoder would have written for it
  (`player_outcomes_invalid_value_wins_over_short_capacity`).
- `reject_reason_to_wire(domain::RejectReason) -> Result<u8, ProtocolError>`
  and `reject_reason_from_wire(u8) -> Result<domain::RejectReason,
  ProtocolError>` are the explicit bidirectional translation the Go
  internal-enum/wire-enum split requires: the internal enum runs `0..14`
  while the wire enum runs `1..15`, so the wire value is never a discriminant
  cast. The outbound half is a closed match over the fifteen domain variants
  with no catch-all arm, so a new domain variant fails to compile here; the
  inbound half is a closed match over the fifteen published wire values and
  answers every other byte — including the retired zero and the first number
  above the interval — with `InvalidEnum`. The mapping is the same one the
  domain `RejectReason::wire_id` publishes, and the group test pins both
  directions against the Go table row by row
  (`player_outcomes_the_reject_reason_matrix_translates_both_ways`).
- The corpus evidence is executed by `tests/protocol_corpus.rs` through the
  same surface: `protocol.server.CommandRejected` registers a decode and an
  encode route under the `mornlea_protocol` consumer, produced by the real
  Go codec in
  `packages/tools/cmd/runtime-oracle/protocol_player_outcomes_test.go`.

## Select hotbar (`src/select_hotbar.rs`, `tests/runtime_contract.rs`, `tests/protocol_client_control.rs`)

- Play packet ID 5 payload is a little-endian `u64` sequence followed by a
  hotbar slot u8. Slot must be inside domain `HotbarSlot` (`0..=8`).
  Out-of-range slots are `InvalidRange`; trailing bytes fail before
  publication (`select_hotbar_round_trip_preserves_golden_bytes`,
  `select_hotbar_rejects_invalid_slot_and_malformed_payload`).
- The family carries the client control group's fallible surface, so a record
  mutated into slot 9 after construction is refused instead of silently
  published (`client_control_invalid_value_wins_over_short_capacity`).

## Simple inventory and crafting command packets (`src/move_inventory_stack.rs`, `src/move_crafting_stack.rs`, `src/close_container.rs`, `src/drop_selected_item.rs`, `src/equip_armor.rs`, `src/take_crafting_output.rs`, `tests/protocol_client_inventory.rs`)

- The six Play client-to-server inventory and crafting command families share
  the client control group's common fallible surface in design §4:
  `validate(&self)` rechecks every public field on each call, checked
  `encoded_len(&self)` sizes the validated record, `encode_into(&self, dst)`
  publishes into a caller-owned buffer, `encode(&self)` is the allocating
  wrapper over it, and `decode` stays a bounded read plus `done()` plus
  validation. The infallible `encode`-returning-`Vec` signatures are gone, so
  a record mutated into an invalid slot pair or a zero `TakeCraftingOutput`
  sequence after construction is refused instead of silently published, and a
  short destination reports `OutputTooSmall { needed, available }` with every
  destination byte unchanged. An invalid value always wins over a short
  destination.
- Each family keeps its own struct, packet ID and private field-writing
  closure over the crate-private `publish_packet` in `server_hello.rs`; no
  combined exported command payload type exists, so the packet keys
  (`MoveInventoryStack` C/Play/6, `MoveCraftingStack` C/Play/7,
  `CloseContainer` C/Play/10, `DropSelectedItem` C/Play/11,
  `TakeCraftingOutput` C/Play/15, `EquipArmor` C/Play/18) are never erased.
- Two payload shapes are in scope. The two move payloads are exactly 10 bytes
  — the sequence and the two slot bytes — with no moved item or count,
  because inventory contents and the moved count stay server-owned. The four
  sequence-only payloads are exactly 8 bytes, so an item count, a drop
  location and a target armor slot are all wire-level decisions the protocol
  never carries. The rule split follows the Go validators: the move families
  check range, then the same-slot relation, then the crafting
  both-in-inventory exclusion, and all report `InvalidRange`; the personal
  grid's size-dependent extension cells are an authority rule the protocol
  layer does not publish.
- The three sequence-only families (`CloseContainer`, `DropSelectedItem`,
  `EquipArmor`) have no invalid mutable value, so they carry the total
  `validate` pattern like `src/request_chunk_resync.rs`: the gate returns
  `Ok(())` for every field state, including a `u64::MAX` sequence, while the
  fallible surface stays uniform. `TakeCraftingOutput` is the fourth
  sequence-only family but does carry a rule — a zero sequence cannot take
  part in command acknowledgement — so its gate refuses it
  (`client_inventory_invalid_value_wins_over_short_capacity`).
- The corpus evidence is executed by `tests/protocol_corpus.rs` through the
  same surface: `protocol.client.{MoveInventoryStack, MoveCraftingStack,
  CloseContainer, DropSelectedItem, EquipArmor, TakeCraftingOutput}` each
  register a decode and an encode route under the `mornlea_protocol`
  consumer, produced by the real Go codec in
  `packages/tools/cmd/runtime-oracle/protocol_client_inventory_test.go`, with
  the sequence published and requested as a decimal and the two move slots as
  JSON numbers.

## Drop selected item (`src/drop_selected_item.rs`, `tests/runtime_contract.rs`, `tests/protocol_client_inventory.rs`)

- Play packet ID 11 payload is a little-endian `u64` sequence. Zero
  sequences are legal. The selected slot and drop position stay
  server-owned. Truncated payloads and trailing bytes fail before
  publication (`drop_selected_item_round_trip_preserves_golden_bytes`,
  `drop_selected_item_rejects_malformed_payload_and_accepts_zero_sequence`).
- The family carries the simple inventory and crafting command group's
  fallible surface with a total value gate, so no field mutation can make
  the record unpublishable and the surface stays uniform
  (`sequence_only_families_keep_a_total_value_gate`).

## Equip armor (`src/equip_armor.rs`, `tests/runtime_contract.rs`, `tests/protocol_client_inventory.rs`)

- Play packet ID 18 payload is a little-endian `u64` sequence, the same
  shape as DropSelectedItem. Zero sequences are legal. The selected item
  and destination armor slot stay server-owned. Truncated payloads and
  trailing bytes fail before publication
  (`equip_armor_round_trip_preserves_golden_bytes`,
  `equip_armor_rejects_malformed_payload_and_accepts_zero_sequence`).
- The family carries the simple inventory and crafting command group's
  fallible surface with a total value gate, so no field mutation can make
  the record unpublishable and the surface stays uniform
  (`sequence_only_families_keep_a_total_value_gate`).

## Take crafting output (`src/take_crafting_output.rs`, `tests/runtime_contract.rs`, `tests/protocol_client_inventory.rs`)

- Play packet ID 15 payload is a little-endian `u64` sequence. Zero
  sequences are `InvalidRange` because they cannot take part in command
  acknowledgement. Output contents stay server-owned. Truncated payloads
  and trailing bytes fail before publication
  (`take_crafting_output_round_trip_preserves_golden_bytes`,
  `take_crafting_output_rejects_zero_sequence_and_malformed_payload`).
- The family carries the simple inventory and crafting command group's
  fallible surface, so a record mutated into a zero sequence after
  construction is refused instead of silently published
  (`client_inventory_invalid_value_wins_over_short_capacity`).

## Move inventory stack (`src/move_inventory_stack.rs`, `tests/runtime_contract.rs`, `tests/protocol_client_inventory.rs`)

- Play packet ID 6 payload is a little-endian `u64` sequence plus source
  and target slot bytes. Slots must be distinct and inside
  `0..INVENTORY_SLOTS-1` (`36`, copied from Go `InventorySlots`). Same-slot
  and out-of-range pairs are `InvalidRange`; trailing bytes fail before
  publication (`move_inventory_stack_round_trip_preserves_golden_bytes`,
  `move_inventory_stack_rejects_invalid_slots_and_malformed_payload`).
- The family carries the simple inventory and crafting command group's
  fallible surface, so a record mutated into an out-of-range or same-slot
  pair after construction is refused instead of silently published
  (`client_inventory_invalid_value_wins_over_short_capacity`).

## Move crafting stack (`src/move_crafting_stack.rs`, `tests/runtime_contract.rs`, `tests/protocol_client_inventory.rs`)

- Play packet ID 7 payload is a little-endian `u64` sequence plus unified
  view slots. Grid is `0..CRAFTING_GRID_SLOTS-1`; inventory is
  `CRAFTING_GRID_SLOTS..GRID_CRAFTING_VIEW_SLOTS-1`. Same-slot, out-of-range,
  and inventory-to-inventory pairs are `InvalidRange`; trailing bytes fail
  before publication (`move_crafting_stack_round_trip_preserves_golden_bytes`,
  `move_crafting_stack_rejects_invalid_slots_and_malformed_payload`).
- The family carries the simple inventory and crafting command group's
  fallible surface, so a record mutated into an out-of-range, same-slot or
  both-in-inventory pair after construction is refused instead of silently
  published (`client_inventory_invalid_value_wins_over_short_capacity`).

## Close container (`src/close_container.rs`, `tests/runtime_contract.rs`, `tests/protocol_client_inventory.rs`)

- Play packet ID 10 payload is a little-endian `u64` sequence. Zero
  sequences are legal. The viewed container identity stays server-owned.
  Truncated payloads and trailing bytes fail before publication
  (`close_container_round_trip_preserves_golden_bytes`,
  `close_container_rejects_malformed_payload_and_accepts_zero_sequence`).
- The family carries the simple inventory and crafting command group's
  fallible surface with a total value gate, so no field mutation can make
  the record unpublishable and the surface stays uniform
  (`sequence_only_families_keep_a_total_value_gate`).

## Place block (`src/place_block.rs`, `src/bytes.rs`, `tests/runtime_contract.rs`, `tests/protocol_client_control.rs`)

- Play packet ID 2 payload is a little-endian `u64` sequence, two
  little-endian `f32` look angles, and a hotbar slot byte. Slot must be
  inside domain `HotbarSlot` (`0..=8`). Non-finite yaw/pitch are
  `InvalidFloat`; out-of-range slots are `InvalidRange`; trailing bytes
  fail before publication (`place_block_round_trip_preserves_golden_bytes`,
  `place_block_rejects_invalid_slot_non_finite_and_malformed_payload`).
- The family carries the client control group's fallible surface, so a record
  mutated into an out-of-range slot or a non-finite angle after construction
  is refused instead of silently published
  (`client_control_invalid_value_wins_over_short_capacity`).
- `ByteEncoder` / `ByteDecoder` `f32` helpers copy the Go primitive: NaN
  and Inf fail before a payload is published.

## Client ray action packets (`src/open_container.rs`, `src/till_soil.rs`, `src/bone_meal.rs`, `src/collect_water.rs`, `src/place_water.rs`, `tests/protocol_client_rays.rs`)

- The five Play client-to-server ray action families share the client control
  group's common fallible surface in design §4: `validate(&self)` rechecks the
  finite rotation on each call, checked `encoded_len(&self)` sizes the
  validated record, `encode_into(&self, dst)` publishes into a caller-owned
  buffer, `encode(&self)` is the allocating wrapper over it, and `decode`
  stays a bounded read plus `done()` plus validation. The infallible
  `encode`-returning-`Vec` signatures are gone, so a record mutated into a
  non-finite angle after construction is refused instead of silently
  published, and a short destination reports
  `OutputTooSmall { needed, available }` with every destination byte
  unchanged. An invalid value always wins over a short destination.
- Each family keeps its own struct, packet ID and private field-writing
  closure over the crate-private `publish_packet` in `server_hello.rs`; no
  combined exported ray payload type exists, so the packet keys
  (`OpenContainer` C/Play/8, `TillSoil` C/Play/13, `BoneMeal` C/Play/14,
  `CollectWater` C/Play/16, `PlaceWater` C/Play/17) are never erased. The
  payload is exactly 16 bytes — the sequence and the two look angles — with
  no target position, held item, container kind or result, because the
  ray-cast target and the world write stay server-owned.
- The look angles are published as their exact IEEE-754 bits, so a `-0.0` yaw
  survives the round trip where an `f32 ==` comparison cannot tell the two
  zeros apart (`client_ray_negative_zero_yaw_survives_the_round_trip`).
- The corpus evidence is executed by `tests/protocol_corpus.rs` through the
  same surface: `protocol.client.{OpenContainer, TillSoil, BoneMeal,
  CollectWater, PlaceWater}` each register a decode and an encode route under
  the `mornlea_protocol` consumer, produced by the real Go codec in
  `packages/tools/cmd/runtime-oracle/protocol_client_rays_test.go`, with the
  look angles published and requested as eight-digit hexadecimal bit strings.

## Open container (`src/open_container.rs`, `tests/runtime_contract.rs`, `tests/protocol_client_rays.rs`)

- Play packet ID 8 payload is a little-endian `u64` sequence followed by
  two little-endian `f32` look angles. The server ray-casts the
  authoritative world and decides whether the hit block is a furnace or a
  chest, so the client never declares a container kind. A zero sequence is
  legal. Non-finite yaw/pitch are `InvalidFloat`; truncated payloads and
  trailing bytes fail before publication
  (`open_container_round_trip_preserves_golden_bytes`,
  `open_container_rejects_non_finite_and_malformed_payload`).
- The family carries the client ray group's fallible surface, so a record
  mutated into a non-finite angle after construction is refused instead of
  silently published (`client_ray_invalid_value_wins_over_short_capacity`).

## Request chunk resync (`src/request_chunk_resync.rs`, `tests/runtime_contract.rs`, `tests/protocol_client_control.rs`)

- Play packet ID 3 payload is a little-endian `u64` sequence, the target
  dimension, two little-endian chunk coordinates, and the revision the
  client already holds. Only domain `Dimension::OVERWORLD` and
  `Dimension::DEPTHS` are known; any other dimension is `InvalidEnum`.
  Chunk state stays server-owned; negative coordinates are legal.
  Truncated payloads and trailing bytes fail before publication
  (`request_chunk_resync_round_trip_preserves_golden_bytes`,
  `request_chunk_resync_rejects_unknown_dimension_and_malformed_payload`).
- The decoder matches the raw wire `i32` against the two known dimension IDs
  instead of narrowing it to a `u8`, so a value such as `256` is an
  `InvalidEnum` rather than a reinterpreted dimension
  (`client_control_decode_rejects_the_pinned_invalid_values`), and
  `validate` stays total because the dimension is the checked domain value
  (`client_control_request_chunk_resync_has_no_invalid_value_to_mutate_into`).
- `ByteEncoder` / `ByteDecoder` `i32` helpers use two's-complement
  little-endian encoding, matching the Go primitive.

## Till soil (`src/till_soil.rs`, `tests/runtime_contract.rs`, `tests/protocol_client_rays.rs`)

- Play packet ID 13 payload is a little-endian `u64` sequence followed by
  two little-endian `f32` look angles, with no slot byte. The server
  validates that the ray-cast target is fluid-adjacent dirt and owns the
  resulting block write. A zero sequence is legal. Non-finite yaw/pitch
  are `InvalidFloat`; truncated payloads and trailing bytes fail before
  publication (`till_soil_round_trip_preserves_golden_bytes`,
  `till_soil_rejects_non_finite_and_malformed_payload`).
- The family carries the client ray group's fallible surface, so a record
  mutated into a non-finite angle after construction is refused instead of
  silently published (`client_ray_invalid_value_wins_over_short_capacity`).

## Bone meal (`src/bone_meal.rs`, `tests/runtime_contract.rs`, `tests/protocol_client_rays.rs`)

- Play packet ID 14 payload is a little-endian `u64` sequence followed by
  two little-endian `f32` look angles, with no slot byte. The server
  validates that the ray-cast target is a fertilizable plant block and
  owns the resulting block write. A zero sequence is legal. Non-finite
  yaw/pitch are `InvalidFloat`; truncated payloads and trailing bytes
  fail before publication (`bone_meal_round_trip_preserves_golden_bytes`,
  `bone_meal_rejects_non_finite_and_malformed_payload`).
- The family carries the client ray group's fallible surface, so a record
  mutated into a non-finite angle after construction is refused instead of
  silently published (`client_ray_invalid_value_wins_over_short_capacity`).

## Collect water (`src/collect_water.rs`, `tests/runtime_contract.rs`, `tests/protocol_client_rays.rs`)

- Play packet ID 16 payload is a little-endian `u64` sequence followed by
  two little-endian `f32` look angles, with no slot byte. The server
  validates that the ray-cast target is a water source the held bucket
  can collect from and owns the resulting item change. A zero sequence is
  legal. Non-finite yaw/pitch are `InvalidFloat`; truncated payloads and
  trailing bytes fail before publication
  (`collect_water_round_trip_preserves_golden_bytes`,
  `collect_water_rejects_non_finite_and_malformed_payload`).
- The family carries the client ray group's fallible surface, so a record
  mutated into a non-finite angle after construction is refused instead of
  silently published (`client_ray_invalid_value_wins_over_short_capacity`).

## Place water (`src/place_water.rs`, `tests/runtime_contract.rs`, `tests/protocol_client_rays.rs`)

- Play packet ID 17 payload is a little-endian `u64` sequence followed by
  two little-endian `f32` look angles, with no slot byte. The server
  validates that the held bucket is water-filled and owns the resulting
  block write. A zero sequence is legal. Non-finite yaw/pitch are
  `InvalidFloat`; truncated payloads and trailing bytes fail before
  publication (`place_water_round_trip_preserves_golden_bytes`,
  `place_water_rejects_non_finite_and_malformed_payload`).
- The family carries the client ray group's fallible surface, so a record
  mutated into a non-finite angle after construction is refused instead of
  silently published (`client_ray_invalid_value_wins_over_short_capacity`).

## Container reference (`src/container_ref.rs`, `tests/runtime_contract.rs`)

- `ContainerRef` is the shared 18-byte wire value that furnace and chest
  commands carry: a little-endian `i32` dimension, two chunk coordinates,
  a one-byte kind, a slot byte, and a little-endian `u32` generation.
  `write`/`read` keep furnaces and chests on one encoding, and `read`
  stores the raw `i32` dimension without narrowing it: both container arrays
  live in the overworld's fixed per-chunk storage, so a foreign dimension is
  an invalid real reference, never a value to reinterpret as a `u8`. The
  dimension-first conversion therefore refuses `256` and `-1` as
  `InvalidRange` instead of publishing a reference the authority rejects
  (`container_view_decode_refuses_a_foreign_raw_dimension`).
- `NONE` is the exact all-zero record, the one absent sentinel the inventory
  and crafting views carry. `to_domain_present` rejects it and every invalid
  real reference; `to_domain_optional` maps only the exact zero record to
  `None` and requires a checked present record for everything else. The
  checked value is the domain `ContainerRef`, which owns the per-kind slot
  bounds and the nonzero-generation rule; a foreign dimension, out-of-range
  slot, or zero generation is `InvalidRange`, and an unknown kind is
  `InvalidEnum`
  (`container_reference_present_conversion_rejects_foreign_dimensions`,
  `container_reference_absence_is_exactly_the_zero_record`).
- The packet families keep their own reference gates through the crate-private
  `validate_furnace` / `validate_chest` helpers, which run the kind check
  first and then the checked conversion, and through `validate_any`, which
  delegates straight to `to_domain_present` and therefore answers the
  dimension first. `MoveContainerStack`
  validates its reference through `validate_any`, and the three view-addressed
  commands through the private `validate_stack_view` in
  `src/move_stack_partial.rs`, which requires the exact `NONE` in the
  inventory and crafting views and runs `to_domain_present` in the container
  view. A case record therefore never combines an unknown kind with a nonzero
  dimension: Go's `validAnyContainerRef` answers the kind first
  (`invalid-enum`) while the neutral conversion answers the dimension first
  (`invalid-value`), so a doubly invalid reference would publish different
  categories on the two sides. Every corpus case and group-test negative
  carries exactly one violation.

## Move container stack (`src/move_container_stack.rs`, `tests/runtime_contract.rs`, `tests/protocol_client_stack_views.rs`)

- Play packet ID 9 payload is a `u64` sequence, an 18-byte container
  reference, and unified source and target slot bytes. Furnace unified
  slots are `0..FURNACE_VIEW_SLOTS-1` with `FURNACE_OUTPUT_SLOT` legal
  only as a source; chest unified slots are `0..CHEST_VIEW_SLOTS-1`.
  The family carries the stack-view group's common fallible surface
  (`validate` → checked `encoded_len` → capacity check → private
  `publish_packet`), so a record mutated into a malformed reference, a
  same-slot pair or the furnace output target after construction is refused
  instead of silently published, and a short destination reports
  `OutputTooSmall { needed, available }` with every destination byte
  unchanged.
- The value gate keeps the Go `MoveContainerStack.Validate` order: the
  reference through `validate_any` first, then the same-slot relation, then
  the per-kind range, then the furnace output-target exclusion. A foreign
  dimension, an unknown kind, a zero generation and a physical slot outside
  the per-chunk array are all refused before any slot rule, which is what
  keeps the frozen corpus categories aligned on both sides.
  Truncated payloads and trailing bytes fail before publication
  (`move_container_stack_round_trip_preserves_golden_bytes`,
  `move_container_stack_rejects_invalid_container_and_malformed_payload`,
  `client_stack_view_refuses_a_malformed_real_reference`).

## Move stack partial (`src/move_stack_partial.rs`, `tests/runtime_contract.rs`, `tests/protocol_client_stack_views.rs`)

- Play packet ID 19 payload is a `u64` sequence, an 18-byte container
  reference, a view byte, source and target bytes, and a single-item
  flag. The moved amount is derived by the server from the source stack,
  so the wire carries no count field. The three view domains are
  `STACK_VIEW_INVENTORY` (`0`), `STACK_VIEW_CRAFTING` (`1`), and
  `STACK_VIEW_CONTAINER` (`2`); the container view bounds the index by the
  referenced container kind, while the inventory and crafting views must
  carry the zero container reference.
- The family carries the stack-view group's common fallible surface and
  routes its gate through the private `validate_stack_view`, which runs the
  container view's checked reference conversion even though the packet layer
  publishes the raw bytes, so a malformed real reference is refused here
  instead of being handed to the authority. The returned
  `Option<mornlea_domain::ContainerRef>` is the checked identity the
  container view addresses; the two absent views return `None`.
- The static rule set deliberately stops where the Go validator stops: two
  crafting inventory-region indices and the furnace output slot as a target
  stay wire-valid here, because target-cell capacity, furnace slot item
  rules and the moved count are authority rules the protocol layer does not
  publish (`move_stack_partial_keeps_the_authority_only_rules_off_the_wire`).
  Same-slot and out-of-range pairs are `InvalidRange`; unknown views and
  kinds are `InvalidEnum`; a `single` flag outside 0/1 is `InvalidEnum`;
  truncated payloads and trailing bytes fail before publication
  (`move_stack_partial_round_trip_preserves_golden_bytes`,
  `move_stack_partial_rejects_invalid_view_and_malformed_payload`).

## Quick move stack (`src/move_stack_partial.rs`, `tests/runtime_contract.rs`, `tests/protocol_client_stack_views.rs`)

- Play packet ID 20 payload is the `MoveStackPartial` prefix without the
  target and single-item flag: a `u64` sequence, an 18-byte container
  reference, a view byte, and one source byte. The destination is a fixed
  deterministic contract the server derives, so the wire carries no target
  slot and there is no same-slot rejection.
- The family carries the same fallible surface and hands its single index to
  `validate_stack_view` as both ends, so the exact all-zero reference in the
  inventory and crafting views and a checked real reference in the container
  view are gates this family publishes rather than assumptions. Unknown views
  and kinds are `InvalidEnum`; truncated payloads and trailing bytes fail
  before publication
  (`quick_move_stack_round_trip_preserves_golden_bytes`,
  `quick_move_stack_rejects_invalid_view_and_malformed_payload`).

## Drop stack (`src/move_stack_partial.rs`, `tests/runtime_contract.rs`, `tests/protocol_client_stack_views.rs`)

- Play packet ID 21 payload is a `u64` sequence, an 18-byte container
  reference, a view byte, and a unified slot byte. The drop position is
  derived by the server from the authoritative player state, so the wire
  carries no coordinates. The static view, container-reference, and index
  bounds match the other view-addressed commands; unknown views and kinds
  are `InvalidEnum`; truncated payloads and trailing bytes fail before
  publication (`drop_stack_round_trip_preserves_golden_bytes`,
  `drop_stack_rejects_invalid_view_and_malformed_payload`).
- The canonical vector carries the exact all-zero reference, so the absent
  sentinel is round-tripped byte for byte rather than reconstructed, and any
  nonzero reference in that position is refused
  (`view_addressed_records_carry_the_exact_absent_sentinel`).

## Container and view-addressed stack command packets (`tests/protocol_client_stack_views.rs`)

- The four Play client-to-server stack command families share one common
  fallible surface in design §4: `validate(&self)` rechecks every public
  field on each call, checked `encoded_len(&self)` sizes the validated
  record, `encode_into(&self, dst)` publishes into a caller-owned buffer,
  `encode(&self)` is the allocating wrapper over it, and `decode` stays a
  bounded read plus `done()` plus validation. The infallible
  `encode`-returning-`Vec` signatures are gone, so a record mutated into an
  invalid reference view pair after construction is refused instead of
  silently published, and a short destination reports
  `OutputTooSmall { needed, available }` with every destination byte
  unchanged. An invalid value always wins over a short destination.
- Payload strides are fixed and pinned per shape: `MoveContainerStack`,
  `QuickMoveStack` and `DropStack` are 28 bytes, `MoveStackPartial` is 30.
  Each family keeps its own struct, packet ID and private field-writing
  closure over the crate-private `publish_packet` in `server_hello.rs`, and
  the three view-addressed records share the private `validate_stack_view`,
  which owns the view dispatch, the reference regime and the per-view index
  bounds once (`client_stack_view_records_round_trip_through_the_fallible_surface`,
  `client_stack_view_invalid_value_wins_over_short_capacity`).
- The corpus evidence is executed by `tests/protocol_corpus.rs` through the
  same surface: `protocol.client.{MoveContainerStack, MoveStackPartial,
  QuickMoveStack, DropStack}` each register a decode and an encode route
  under the `mornlea_protocol` consumer, produced by the real Go codec in
  `packages/tools/cmd/runtime-oracle/protocol_client_stack_views_test.go`.
  The container reference publishes and requests as a nested JSON object of
  plain integers (`dimension`, `chunk_x`, `chunk_z`, `kind`, `slot`,
  `generation`), the `sequence` as a decimal string in normalized fields and
  a decimal number in requests, and `view`/`from`/`to`/`slot` as JSON
  numbers with `single` as a JSON boolean.

## Chat command (`src/chat_command.rs`, `tests/runtime_contract.rs`, `tests/protocol_client_chat.rs`)

- Play packet ID 12 payload is a length-prefixed UTF-8 instruction bounded
  by `CHAT_COMMAND_TEXT_MAX_BYTES` (`1024`, shared with the planner
  instruction limit). The family carries the common fallible surface in
  design §4: `validate(&self)` rechecks the public `text` on each call,
  checked `encoded_len(&self)` sizes the validated record,
  `encode_into(&self, dst)` publishes into a caller-owned buffer,
  `encode(&self)` is the allocating wrapper over it, and `decode(payload)`
  stays a bounded read plus `done()` plus validation. An invalid value
  always wins over a short destination, so a record mutated into an
  untrimmed, empty, control-carrying or oversized text after construction is
  refused instead of silently published
  (`chat_command_round_trip_preserves_golden_bytes`,
  `chat_command_rejects_blank_control_and_malformed_payload`).
- The text rule is the domain `CommandText` rule: at least one byte, at most
  1024 bytes, no surrounding whitespace and no control character, with the
  pinned whitespace and control sets the domain owns. `valid_command_text`
  routes both the wire slot and the chat event's command restatement through
  it, so the wire, the domain and the Go `validateCommandText` share one
  admitted set. Text-bound failures are `InvalidString`; the pre-parse
  payload ceiling is `CHAT_COMMAND_MAX_WIRE_BYTES` (`1026`, the two-byte
  maximum prefix plus the text) and an oversized payload is
  `FrameTooLarge`.
- A declared string length the remaining payload cannot complete is an
  incomplete payload and reports `Truncated` rather than the `InvalidString`
  the shared string primitive reports for the same bytes, mirroring the
  control message reader in `src/handshake_reject.rs`: the Go
  `byteDecoder.string` answers that condition with the same sentinel as a
  malformed UTF-8 text, while the frozen corpus category for an incomplete
  payload is `truncated`. A noncanonical length prefix stays
  `NonCanonicalUvarint`; trailing bytes fail after the last field.
- The text is never interpreted on the wire: a leading `@` is not
  addressing, a leading `/` is not a warp, and the payload carries no
  session, sequence or FIFO field, so the exact-literal pins in
  `tests/protocol_client_chat.rs` are the purity contract.
- The corpus evidence is executed by `tests/protocol_corpus.rs` through the
  same surface: `protocol.client.ChatCommand` registers a decode and an
  encode route under the `mornlea_protocol` consumer, produced by the real
  Go codec in
  `packages/tools/cmd/runtime-oracle/protocol_client_chat_test.go`. The
  normalized field is `{"text": <string>}` with the text verbatim. The
  text-above-bound case is encode-only, because the payload ceiling answers
  a length the decoder cannot reach first, and the truncated case is a
  rejected proper prefix of the reviewed payload, which is the shape the
  boundary resolver resolves to the truncated category.

## Player input (`src/player_input.rs`, `tests/runtime_contract.rs`, `tests/protocol_client_control.rs`)

- Play packet ID 0 payload is the highest-frequency record: a `u64`
  sequence, two `i8` move axes, four action flags around two `f32` look
  angles, in field order sequence, move X, move Z, jump, yaw, pitch,
  mining, eating, sprinting, sneaking. Rotation must be finite; the
  domain type `mornlea_domain::LookAngles` is the single owner of that
  rule, so a non-finite yaw or pitch is `InvalidFloat`. A non-0/1 flag
  byte is `InvalidEnum`; truncated payloads and trailing bytes fail
  before publication (`player_input_round_trip_preserves_golden_bytes`,
  `player_input_rejects_non_finite_and_malformed_payload`).
- The family carries the client control group's fallible surface, so a record
  mutated into a non-finite rotation after construction is refused instead of
  silently published (`client_control_invalid_value_wins_over_short_capacity`).
  The move axes and the pitch stay unrestricted at the protocol boundary, and
  the angles are published as their exact IEEE-754 bits, so a `-0.0` yaw
  survives the round trip (`client_control_negative_zero_and_wide_values_keep_their_wire_shapes`).

## Combat hit (`src/combat_hit.rs`, `tests/runtime_contract.rs`, `tests/protocol_player_outcomes.rs`)

- Play packet ID 25 payload is a fixed 10-byte confirmation: a
  little-endian `u64` server tick, a damage byte, and a target kind byte.
  The server tick must be non-zero, damage must be inside `1..=MAX_HEALTH`
  (`20`, copied from Go `core.MaxHealth`), and the kind must be inside
  `COMBAT_TARGET_PLAYER..=COMBAT_TARGET_PASSIVE` (`1..=3`). Out-of-range
  values are `InvalidRange`; unknown kinds are `InvalidEnum`; truncated
  payloads and trailing bytes fail before publication
  (`combat_hit_round_trip_preserves_golden_bytes`,
  `combat_hit_rejects_invalid_range_and_malformed_payload`).
- The family carries the player and private outcome group's common fallible
  surface, and the gate keeps the Go validator's order — tick, then the
  damage range, then the kind — so both implementations refuse the same bytes
  with the same error variant and a record mutated into a zero tick, an
  out-of-range damage or an unknown kind after construction is refused instead
  of silently published
  (`player_outcomes_invalid_value_wins_over_short_capacity`).
- The corpus evidence is executed by `tests/protocol_corpus.rs` through the
  same surface: `protocol.server.CombatHit` registers a decode and an encode
  route under the `mornlea_protocol` consumer, produced by the real Go codec
  in
  `packages/tools/cmd/runtime-oracle/protocol_player_outcomes_test.go`.

## Remote player despawn (`src/remote_player_despawn.rs`, `tests/runtime_contract.rs`)

- Play packet ID 8 payload is the 16-byte UUIDv4 identity of a remote
  player the authoritative world removed. `PlayerId` stays the single
  identity gate: zero and non-v4 values are `InvalidIdentity`. Truncated
  payloads and trailing bytes fail before publication
  (`remote_player_despawn_round_trip_preserves_golden_bytes`,
  `remote_player_despawn_rejects_invalid_identity_and_malformed_payload`).
- The family carries the remote-player group's common fallible surface, and
  its gate is total because the identity is the checked domain `PlayerId`:
  a zero or non-UUIDv4 value cannot be constructed, so no field mutation
  after construction can make the record unpublishable and the gate restates
  no rule the domain type owns
  (`remote_players_despawn_round_trips_through_the_fallible_surface`).
- The corpus evidence is executed by `tests/protocol_corpus.rs` through the
  same surface: `protocol.server.RemotePlayerDespawn` registers a decode and
  an encode route under the `mornlea_protocol` consumer, produced by the real
  Go codec in
  `packages/tools/cmd/runtime-oracle/protocol_remote_players_test.go`. The
  Go validator's identity message is family-specific, so the zero-identity
  case classifies at the identity boundary both sides publish and the
  trailing-byte case keeps its own category.
- `ByteEncoder` / `ByteDecoder` `i8` helpers use two's-complement
  little-endian encoding, matching the Go primitive.
- `ByteEncoder` / `ByteDecoder` `boolean` helpers copy the Go primitive:
  only 0 and 1 are accepted, and anything else is `InvalidEnum`.

## Item stack (`src/item_stack.rs`, `tests/runtime_contract.rs`)

- `ItemStack` is the fixed 5-byte slot value every inventory-carrying family
  shares: `u16` item, `u8` count, `u16` durability. The empty stack is the
  zero value. The value is the domain's checked `ItemStack`, re-exported
  here; the registered item table the Go side consults for `ItemStack.Valid`
  — stack limits, tool and armor durability maxima, and the smelting
  input/output whitelists — lives once in `mornlea_domain`, and this module
  composes it instead of forking a second copy
  (`item_stack_rules_come_from_the_domain_tables`).
- Unregistered item numbers are `InvalidEnum`; a non-canonical empty stack, a
  zero or over-limit count, and a durability outside the item budget are
  `InvalidRange`. The item-number constants are frozen wire data and stay in
  this module; the furnace slot predicates (`valid_furnace_input`,
  `valid_furnace_output`) read the domain's smelting table rather than
  listing the products again.
- The wire codec helpers `read` and `write` are crate-private: decoding runs
  the domain rule and maps its rejection into the protocol error, and
  encoding writes an already-checked value.

## Record arrays and batch headers (`src/batch.rs`, `src/block.rs`)

- `read_fixed` / `write_fixed` encode fixed-count record arrays, and
  `ByteCountBatch` / `UvarintCountBatch` are the two batch headers the
  entity families share: a server tick plus a one-byte or canonical-uvarint
  record count. A zero count and a count above the family's fixed maximum are
  `InvalidRange`; a remaining length that is not exactly `count` records of
  the family stride is `Truncated` before any record is published.
- `require_minimum_records` is the budget check for the families whose Go
  decoder rejects a short payload but accepts a long one, leaving the
  remainder to the end-of-payload check. Using the exact-length rule there
  would report a padded batch as truncated rather than as trailing bytes,
  which is a different failure than the Go side publishes. The item drop and
  remote player state batches use it; the mob and companion batches use
  `require_records`.
- `src/block.rs` re-exports the domain's registered-block predicate and keeps
  the wire-level geometry: the world vertical span, the section layout, the
  chunk-ordered block index that sorted block-change batches compare, and
  `MAX_CHUNK_BLOCK_INDEX`, the exclusive upper bound an item drop's block
  index must stay below.
- The byte-count batch families carry one recorded latent class, which the
  projectile group resolved and the others keep: the Go decode path applies
  a fixed per-family wire ceiling before the family decoder, so an
  over-ceiling payload answers `capacity` there. The three projectile
  decoders close the class by applying the same ceiling as a pre-parse size
  check before a single byte is read — the node precedent
  `src/companion_despawn.rs` set — so `ProjectileDespawn/decode-over-ceiling`
  freezes one category on both sides and the ceiling fires before every
  count, length and record rule. The hostile and passive decoders still size
  by the count bound and the exact-remaining-length rule, so the same bytes
  answer `Truncated` there; no corpus case sits on that boundary, and their
  family-specific `*_MAX_WIRE_BYTES` constants mirror the Go declarations
  without gating the Rust decode. A later family wanting an over-ceiling
  corpus case follows the projectile ruling rather than freezing the
  divergence again.

## World delta packets (`src/block_changes.rs`, `src/forget_chunks.rs`, `tests/protocol_world_delta.rs`, `tests/protocol_corpus.rs`)

- The two server-to-client world delta families are the first variable-count
  batch families in this crate: both payloads are a fixed header, a canonical
  uvarint record count, and fixed-stride records (14 bytes per block change,
  8 bytes per chunk coordinate). Both carry the crate's common fallible
  surface in design §4 — `validate(&self)` rechecks every public field on
  each call, checked `encoded_len(&self)` sizes the validated record
  (header + varint length + stride × count, all checked arithmetic),
  `encode_into(&self, dst)` publishes into a caller-owned buffer through the
  crate-private `publish_packet`, `encode(&self)` is the allocating wrapper
  over it, and `decode(payload)` is a bounded read plus `done()` plus
  validation — so a record mutated into an invalid value after construction
  is refused instead of silently published and a short destination reports
  `OutputTooSmall { needed, available }` with every destination byte
  unchanged.
- The empty rules differ and the difference is the wire's own: a zero-change
  block batch stays legal as the revision barrier an item-only tick carries,
  while a zero-count forget batch is `InvalidRange` because it has no
  observable meaning. The decode side applies the count bound before the
  record-length rule on both families, mirroring the Go decode arms, so a
  count above the ceiling is a range violation even when the remaining
  payload is also short.
- The order rules differ too. A block-change batch must stay strictly
  increasing by the chunk-ordered block index `src/block.rs` publishes; a
  forget batch keeps its submitted wire order and is only checked for
  uniqueness, because the authority groups and sorts when it publishes and a
  replay has to observe the recorded sequence. The uniqueness check sorts a
  fallible reserved scratch copy (`reserve_unique_scratch`, a failed reserve
  is `Allocation`) rather than the batch itself, following the domain's
  forget-batch pattern.
- The block-change gate's variant split is the pinned ruling: an
  unregistered block is `InvalidEnum` and an out-of-span Y is
  `InvalidRange`, which are the two categories the Go validator publishes for
  the same bytes; the two checks were one collapsed branch before this
  surface landed. The revisions render as decimal strings and the coordinates
  and blocks as plain JSON numbers in the corpus, and the record arrays are
  published in wire order, never sorted.
- The 4096/4097 boundaries are group-test pins rather than corpus assets, so
  the frozen corpus stays small; `tests/protocol_world_delta.rs` constructs
  the maximum columns directly and pins the count bound firing before the
  record-length rule.
- The corpus evidence is executed by `tests/protocol_corpus.rs` through the
  same surface: `protocol.server.BlockChanges` and
  `protocol.server.ForgetChunks` each register a decode and an encode route
  under the `mornlea_protocol` consumer, produced by the real Go codec in
  `packages/tools/cmd/runtime-oracle/protocol_world_delta_test.go`.

## Block changes (`src/block_changes.rs`, `tests/runtime_contract.rs`, `tests/protocol_world_delta.rs`)

- Play packet ID 1 payload is the dimension, two chunk coordinates, the
  base and new revision, a canonical uvarint change count, and the
  fixed-stride changes. Zero changes stay legal as a revision barrier for an
  item-only tick; the count is bounded by `MAX_BLOCK_CHANGES` (`4096`).
- The revision transition must be exactly `base + 1` from a non-zero,
  non-saturated base; every change must name a registered block, stay inside
  the world span and the announced chunk, and keep the batch strictly
  increasing by chunk-ordered block index. Failures are `InvalidEnum`,
  `InvalidRange`, or `Truncated` before publication; trailing bytes fail
  after the last change
  (`block_changes_round_trip_preserves_golden_bytes`,
  `block_changes_rejects_invalid_revision_position_and_malformed_payload`,
  `world_delta_block_changes_round_trips_through_the_fallible_surface`,
  `world_delta_the_variant_split_is_pinned`).
- Validation checks all submitted fields before index order, preserving the
  error precedence of a later invalid block. It then compares adjacent
  indices without allocating a temporary vector on the encode path.

## Forget chunks (`src/forget_chunks.rs`, `tests/runtime_contract.rs`, `tests/protocol_world_delta.rs`)

- Play packet ID 2 payload is the dimension, a canonical uvarint chunk
  count, and the fixed-stride chunk coordinates. The count is bounded by
  `MAX_FORGET_CHUNKS` (`4096`), a zero count is `InvalidRange`, duplicate
  coordinates are `InvalidRange`, and a remaining length shorter than the
  declared records is `Truncated` before publication; trailing bytes fail
  after the last coordinate
  (`forget_chunks_round_trip_preserves_golden_bytes`,
  `forget_chunks_rejects_empty_duplicate_and_malformed_payload`,
  `world_delta_forget_chunks_round_trips_through_the_fallible_surface`,
  `world_delta_forget_chunks_preserves_the_unsorted_wire_order`).

## Companion identity (`src/entity_id.rs`)

- `CompanionId` is the checked domain 16-byte UUIDv4 companion identity the
  companion families share; zero and non-v4 values are `InvalidIdentity`.
  The domain publishes no zero companion identity, so the absent form a
  never-addressed chat event carries stays a wire-only raw value; see
  "Shared value rules"
  (`companion_absence_is_never_a_domain_identity`).
- `valid_companion_name` is the companion name rule: the canonical display
  name rule plus a rejection of Unicode whitespace, so a publishable companion
  name never contains an embedded space. `valid_display_name` is the plain
  canonical display-name rule, shared by the chat event and the remote player
  spawn. Both predicates route through the domain's checked text
  constructors, which own the byte, rune, whitespace and control bounds; a
  family never restates the rule.
- `valid_display_name` is a different rule from the login start's, which
  trims first, because the Go side applies `NormalizeDisplayName` differently
  in those two places.

## Companion despawn (`src/companion_despawn.rs`, `tests/runtime_contract.rs`)

- Play packet ID 19 payload is the 16-byte UUIDv4 companion identity the
  authoritative world removed. `InvalidIdentity` rejects zero and non-v4
  values; truncated payloads and trailing bytes fail before publication
  (`companion_despawn_round_trip_preserves_identity_bytes`,
  `companion_despawn_rejects_invalid_identity_and_malformed_payload`).
- The family carries the companion group's common fallible surface, and its
  gate is total because the identity is the checked domain `CompanionId`:
  no field mutation can make this record unpublishable and the gate restates
  no rule the domain type owns. `decode` applies the 16-byte fixed wire bound
  as a pre-allocation guard, matching the Go decoder's fixed-maximum check for
  this packet ID, so a payload above the stride reports `FrameTooLarge` before
  a single byte is read rather than the trailing-byte boundary
  (`companions_despawn_round_trips_through_the_fallible_surface`,
  `companions_the_fixed_ceilings_refuse_before_any_field_is_read`).
- The corpus evidence is executed by `tests/protocol_corpus.rs` through the
  same surface: `protocol.server.CompanionDespawn` registers a decode and an
  encode route under the `mornlea_protocol` consumer, produced by the real Go
  codec in
  `packages/tools/cmd/runtime-oracle/protocol_companions_test.go`. The
  zero-identity case classifies at the identity boundary, which is the one
  category the Go despawn message and `InvalidIdentity` share, and the
  trailing-byte case classifies at the capacity boundary because both
  implementations refuse the oversized payload before any field is read.

## Hostile despawn (`src/hostile_despawn.rs`, `tests/runtime_contract.rs`, `tests/protocol_hostiles.rs`)

- Play packet ID 24 payload is a `u64` server tick, a one-byte record count,
  and the fixed 8-byte hostile IDs. The count is bounded by
  `MAX_HOSTILE_RECORDS` (`64`), a zero count is `InvalidRange`, records must
  be strictly ascending, and a remaining length that is not exactly `count`
  records is `Truncated` before publication
  (`hostile_despawn_round_trip_preserves_batch_bytes`,
  `hostile_despawn_rejects_unsorted_zero_and_malformed_payload`).
- The identity is the checked domain `HostileId` re-exported from
  `src/hostile_id.rs`, which also owns the fixed eight-byte wire read and
  write the three hostile families share. Zero is the absent form of every
  entity family, so it is refused where the identity is read
  (`InvalidIdentity`) and cannot be constructed on the outbound surface at
  all, which is the boundary the Go `network: hostile despawn %d ID is zero`
  message names. The order comparison is over the typed identity, so no batch
  rule compares a reinterpreted byte string.
- The family carries the hostile group's common fallible surface
  (`validate(&self)` → private `valid` → checked `encoded_len` →
  `encode_into` through the crate-private `publish_packet` → allocating
  `encode`), and its gate is count-first: the batch count bound, then the
  strictly increasing identity order. A batch mutated into an empty or
  over-full record set or an unordered identity after construction is refused
  instead of silently published, and a short destination reports
  `OutputTooSmall { needed, available }` with every destination byte
  unchanged (`hostile_despawn_round_trips_through_the_fallible_surface`,
  `hostile_despawn_invalid_value_wins_over_short_capacity`,
  `hostile_mutated_public_fields_are_never_published`).
- The batch applies the exact-remaining-length rule rather than the
  minimum-records rule, because the Go decoder rejects a payload whose
  remaining length is not exactly `count` records before it reads one, so a
  padded payload answers at the truncation boundary on both sides
  (`hostile_decode_rejects_one_trailing_byte`).
- The corpus evidence is executed by `tests/protocol_corpus.rs` through the
  same surface: `protocol.server.HostileDespawn` registers a decode and an
  encode route under the `mornlea_protocol` consumer, produced by the real Go
  codec in
  `packages/tools/cmd/runtime-oracle/protocol_hostiles_test.go`. The frozen
  cases are the canonical vector pair, the 64-record ceiling, the zero
  identity, the duplicate and descending order refusals, the count-bound
  refusal, and the padding refusal the exact-length rule answers at the
  truncation category.

## Projectile despawn (`src/projectile_despawn.rs`, `tests/runtime_contract.rs`, `tests/protocol_projectiles.rs`)

- Play packet ID 31 payload is a `u64` server tick, a one-byte record count,
  and the fixed 8-byte projectile IDs. The count is bounded by
  `MAX_PROJECTILE_RECORDS` (`128`), a zero count is `InvalidRange`, records must
  be strictly ascending, and a remaining length that is not exactly `count`
  records is `Truncated` before publication
  (`projectile_despawn_round_trip_preserves_batch_bytes`,
  `projectile_despawn_rejects_unsorted_zero_and_malformed_payload`).
- The identity is the checked domain `ProjectileId` re-exported from
  `src/projectile_id.rs`, which also owns the fixed eight-byte wire read and
  write the three projectile families share. Zero is the absent form of every
  entity family, so it is refused where the identity is read
  (`InvalidIdentity`) and cannot be constructed on the outbound surface at
  all, which is the boundary the Go `network: projectile despawn %d ID is zero`
  message names. The order comparison is over the typed identity, so no batch
  rule compares a reinterpreted byte string.
- The family carries the projectile group's common fallible surface
  (`validate(&self)` → private `valid` → checked `encoded_len` →
  `encode_into` through the crate-private `publish_packet` → allocating
  `encode`), and its gate is count-first: the batch count bound, then the
  strictly increasing identity order. A batch mutated into an empty or
  over-full record set or an unordered identity after construction is refused
  instead of silently published, and a short destination reports
  `OutputTooSmall { needed, available }` with every destination byte
  unchanged (`projectile_despawn_round_trips_through_the_fallible_surface`,
  `projectile_despawn_invalid_value_wins_over_short_capacity`,
  `projectile_mutated_public_fields_are_never_published`).
- The batch applies the exact-remaining-length rule rather than the
  minimum-records rule, because the Go decoder rejects a payload whose
  remaining length is not exactly `count` records before it reads one, so a
  padded payload answers at the truncation boundary on both sides
  (`projectile_decode_rejects_one_trailing_byte`).
- The decoder applies `PROJECTILE_DESPAWN_MAX_WIRE_BYTES` — derived from the
  eight-byte stride and the 128-record bound — as a pre-parse size check
  before the header is read, mirroring the Go decode path's fixed maximum.
  An over-ceiling payload therefore answers `FrameTooLarge`, the capacity
  boundary, rather than the truncation boundary the length rule would
  report, and the ceiling precedes every count, length and record rule
  (`projectile_decode_refuses_an_over_ceiling_payload_before_any_read`).
- The corpus evidence is executed by `tests/protocol_corpus.rs` through the
  same surface: `protocol.server.ProjectileDespawn` registers a decode and an
  encode route under the `mornlea_protocol` consumer, produced by the real Go
  codec in
  `packages/tools/cmd/runtime-oracle/protocol_projectiles_test.go`. The
  frozen cases are the canonical vector pair, the 128-record ceiling, the
  zero identity, the duplicate and descending order refusals, the count-bound
  refusal, the padding refusal the exact-length rule answers at the
  truncation category, and the over-ceiling refusal the pre-parse ceiling
  answers at the capacity category.

## Passive despawn (`src/passive_despawn.rs`, `tests/runtime_contract.rs`, `tests/protocol_passives.rs`)

- Play packet ID 28 payload is a `u64` server tick, a one-byte record count,
  and the fixed 9-byte records of an ID plus the removal reason. The count is
  bounded by `MAX_PASSIVE_RECORDS` (`64`); the only published reasons are
  `PASSIVE_DESPAWN_VANISHED` (`0`) and `PASSIVE_DESPAWN_DIED` (`1`), anything
  else is `InvalidEnum`; records must be strictly ascending and non-zero
  (`passive_despawn_round_trip_preserves_batch_bytes`,
  `passive_despawn_rejects_unsorted_zero_reason_and_malformed_payload`).
- The identity is the checked domain `PassiveId` re-exported from
  `src/passive_id.rs`, which also owns the fixed eight-byte wire read and
  write the three passive families share. Zero is the absent form of every
  entity family, so it is refused where the identity is read
  (`InvalidIdentity`) and cannot be constructed on the outbound surface at
  all, which is the boundary the Go `network: passive despawn ID is zero`
  message names. The shim mirrors `src/hostile_id.rs` rather than
  generalizing it, because the two identities are distinct domain newtypes
  over the same bits and a generic module would erase that distinction.
- The family carries the passive group's common fallible surface
  (`validate(&self)` → private `valid` → checked `encoded_len` →
  `encode_into` through the crate-private `publish_packet` → allocating
  `encode`), and its gate is count-first: the batch count bound, then per
  record the closed reason match with the strictly increasing identity order.
  A batch mutated into an empty or over-full record set, an unordered
  identity or an unpublished reason after construction is refused instead of
  silently published, and a short destination reports
  `OutputTooSmall { needed, available }` with every destination byte
  unchanged (`passive_despawn_round_trips_through_the_fallible_surface`,
  `passive_despawn_invalid_value_wins_over_short_capacity`,
  `passive_mutated_public_fields_are_never_published`).
- The batch applies the exact-remaining-length rule rather than the
  minimum-records rule, because the Go decoder rejects a payload whose
  remaining length is not exactly `count` records before it reads one, so a
  padded payload answers at the truncation boundary on both sides
  (`passive_decode_rejects_one_trailing_byte`).
- The corpus evidence is executed by `tests/protocol_corpus.rs` through the
  same surface: `protocol.server.PassiveDespawn` registers a decode and an
  encode route under the `mornlea_protocol` consumer, produced by the real Go
  codec in
  `packages/tools/cmd/runtime-oracle/protocol_passives_test.go`. The frozen
  cases are the canonical vector pair, the zero identity, the closed reason
  pair with its encode twin, the duplicate and descending order refusals, the
  count-bound refusal, and the padding refusal the exact-length rule answers
  at the truncation category.

## Chest state (`src/chest_state.rs`, `tests/runtime_contract.rs`)

- Play packet ID 15 payload is the 18-byte chest container reference plus the
  fixed `CHEST_SLOTS` (`27`) item stacks. `read_fixed` decodes the fixed-count
  array, so a truncated payload fails before any slot is published; the chest
  reference must name a chest with a legal slot and generation
  (`chest_state_round_trip_preserves_slot_bytes`,
  `chest_state_rejects_wrong_reference_and_malformed_payload`).
- The family carries the inventory and container publication group's common
  fallible surface: `validate(&self)` rechecks the reference gate on each
  call, checked `encoded_len(&self)` sizes the fixed stride,
  `encode_into(&self, dst)` publishes into a caller-owned buffer through the
  crate-private `publish_packet`, `encode(&self)` is the allocating wrapper
  over it, and `decode(payload)` stays a bounded read plus `done()` plus
  validation. The domain `ItemStack` rule is the single slot-value gate — its
  private fields make an invalid slot value unconstructible on this surface —
  so the gate restates the Go `validChestRef` order (kind, then dimension,
  then slot, then generation) alone, and a record mutated into a malformed
  reference after construction is refused instead of silently published
  (`inventory_publication_chest_state_round_trips_through_the_fallible_surface`,
  `inventory_publication_invalid_value_wins_over_short_capacity`).
- The corpus evidence is executed by `tests/protocol_corpus.rs` through the
  same surface: `protocol.server.ChestState` registers a decode and an encode
  route under the `mornlea_protocol` consumer, produced by the real Go codec
  in
  `packages/tools/cmd/runtime-oracle/protocol_inventory_publication_test.go`.
  The reference publishes as the nested raw-integer object, and every stack
  renders as the `{item, count, durability}` object both directions share.

## Furnace state (`src/furnace_state.rs`, `tests/runtime_contract.rs`)

- Play packet ID 13 payload is the 18-byte furnace reference, the fixed
  input, fuel, and output stacks, a single progress byte, and a
  little-endian `u16` burn time. The reference must name a furnace
  (`validate_furnace`); the timers are bounded by `FURNACE_SMELT_TICKS` and
  `FURNACE_BURN_TICKS`; the input slot only accepts an empty stack or a
  registered smelting input, the fuel slot only an empty stack or coal, and
  the output slot only a fixed smelting product
  (`furnace_state_round_trip_preserves_golden_bytes`,
  `furnace_state_rejects_invalid_slots_timers_and_malformed_payload`).
- The family carries the inventory and container publication group's common
  fallible surface, and its gate keeps the Go `FurnaceState.Validate` order:
  the reference (`validFurnaceRef`: kind, then dimension, then slot, then
  generation), then the two timer bounds, then the three slot whitelists,
  which report `InvalidRange`. No timer-versus-stack consistency relation
  exists in the Go validator and none is invented here, so an idle furnace
  with zero progress, zero burn and empty whitelisted slots is publishable
  beside the active canonical vector
  (`inventory_publication_furnace_state_invents_no_timer_relation`).
- The corpus evidence is executed by `tests/protocol_corpus.rs` through the
  same surface: `protocol.server.FurnaceState` registers a decode and an
  encode route under the `mornlea_protocol` consumer, produced by the real
  Go codec in
  `packages/tools/cmd/runtime-oracle/protocol_inventory_publication_test.go`.

## Crafting state (`src/crafting_state.rs`, `tests/runtime_contract.rs`)

- Play packet ID 21 payload is the grid size, the fixed nine grid slots, and
  the derived output slot, so the encoding never takes a variable-length
  branch. The only published sizes are `CRAFTING_GRID_SIZE_PERSONAL` (`2`) and
  `CRAFTING_GRID_SIZE_WORKBENCH` (`3`); a personal grid may not carry residue
  beyond its own size (`crafting_state_round_trip_preserves_golden_bytes`,
  `crafting_state_rejects_unknown_size_residue_and_malformed_payload`).
- The family carries the inventory and container publication group's common
  fallible surface. Its gate keeps the Go `CraftingState.Validate` order —
  the size domain, then every grid slot with the personal-grid residue rule,
  then the output — and reports an unknown size as `InvalidRange`, which is
  the invalid-value boundary the Go validator's own size message publishes;
  the slots and the output are the domain's checked `ItemStack`
  (`inventory_publication_crafting_state_personal_residue_refuses_every_extension_slot`).
- The corpus evidence is executed by `tests/protocol_corpus.rs` through the
  same surface: `protocol.server.CraftingState` registers a decode and an
  encode route under the `mornlea_protocol` consumer, produced by the real
  Go codec in
  `packages/tools/cmd/runtime-oracle/protocol_inventory_publication_test.go`.

## Inventory state (`src/inventory_state.rs`, `tests/runtime_contract.rs`)

- Play packet ID 10 payload is the selected hotbar byte, the fixed nine
  hotbar slots, and the fixed `BACKPACK_SLOTS` (`27`) backpack slots, which
  makes the payload stride `INVENTORY_STATE_WIRE_BYTES` (`181`). The selected
  index is a domain `HotbarSlot`; every slot is a validated item stack
  (`inventory_state_round_trip_preserves_golden_bytes`,
  `inventory_state_rejects_unknown_selected_and_malformed_payload`).
- The family carries the inventory and container publication group's common
  fallible surface. Its gate keeps the Go `Inventory.Valid` order — the
  selected index first, every stack second — and the stacks are the domain's
  checked `ItemStack`, whose private fields make an invalid slot value
  unconstructible on this surface, so the index is the only rule the gate
  restates while the item-stack rule stays owned once
  (`inventory_publication_inventory_state_round_trips_through_the_fallible_surface`,
  `inventory_publication_the_stack_rule_boundaries_stay_pinned`).
- The corpus evidence is executed by `tests/protocol_corpus.rs` through the
  same surface: `protocol.server.InventoryState` registers a decode and an
  encode route under the `mornlea_protocol` consumer, produced by the real
  Go codec in
  `packages/tools/cmd/runtime-oracle/protocol_inventory_publication_test.go`.

## Container closed (`src/container_closed.rs`, `tests/runtime_contract.rs`)

- Play packet ID 14 payload is the 18-byte container reference whose view
  ended, shared by furnaces and chests. The reference gate is
  `validate_any`, which is the Go `validAnyContainerRef` rule: either known
  kind with its own slot bounds. The exact all-zero record is refused through
  its zero generation rather than treated as an absent container, because
  this family closes a real view and the absent form belongs to the inventory
  and crafting views alone.
- The module existed on disk but was absent from `src/lib.rs`, which is the
  compile red this node closed: `ContainerClosed` now compiles on the public
  surface beside `CloseContainer`, its client-to-server sibling. The family
  carries the inventory and container publication group's common fallible
  surface (`validate` → checked `encoded_len` → capacity check →
  `publish_packet` → allocating `encode` → bounded `decode`), so a record
  mutated into a malformed reference after construction is refused instead of
  silently published
  (`inventory_publication_container_closed_round_trips_through_the_fallible_surface`).
- The corpus evidence is executed by `tests/protocol_corpus.rs` through the
  same surface: `protocol.server.ContainerClosed` registers a decode and an
  encode route under the `mornlea_protocol` consumer, produced by the real
  Go codec in
  `packages/tools/cmd/runtime-oracle/protocol_inventory_publication_test.go`.

## Hostile spawn (`src/hostile_spawn.rs`, `tests/runtime_contract.rs`, `tests/protocol_hostiles.rs`)

- Play packet ID 22 payload is a `u64` server tick, a one-byte record count,
  and the fixed 30-byte spawn records: ID, dimension, position, yaw, health,
  and kind. The count is bounded by `HOSTILE_SPAWN_MAX_RECORDS` (`64`); IDs
  must be non-zero and strictly ascending; only the overworld dimension is
  published; health is `1..=MAX_HEALTH`; and the kind is
  `HOSTILE_KIND_NIGHTWALKER` or `HOSTILE_KIND_BONE_THROWER`
  (`hostile_spawn_round_trip_preserves_batch_bytes`,
  `hostile_spawn_rejects_invalid_records_and_malformed_payload`).
- The family carries the hostile group's common fallible surface, and its
  record gate keeps the Go `HostileSpawnRecord.validate` order: the
  dimension, the pose finiteness, the health span and the closed kind match.
  The identity is already checked by the domain `HostileId`, so the record
  gate restates no identity rule, and the batch gate adds the count bound and
  the strict identity order in the Go batch order. A record mutated into a
  foreign dimension, a non-finite pose, an out-of-range health or an unknown
  kind after construction is refused instead of silently published, and a
  short destination reports `OutputTooSmall { needed, available }` with every
  destination byte unchanged
  (`hostile_spawn_round_trips_through_the_fallible_surface`,
  `hostile_spawn_invalid_value_wins_over_short_capacity`,
  `hostile_kind_is_a_closed_match`, `hostile_health_span_is_one_to_twenty`).
- The dimension is the raw wire `i32` matched against the two known dimension
  IDs rather than narrowed to a `u8`, so a value such as `256` is an
  `InvalidEnum` instead of a reinterpreted dimension, and the negative-zero
  pose bits survive the round trip
  (`hostile_spawn_dimension_is_matched_against_the_known_ids`,
  `hostile_records_preserve_negative_zero_pose_bits`).
- The batch applies the exact-remaining-length rule, the count bound fires
  before it on both sides, and the 64-record ceiling is admitted on all three
  hostile families (`hostile_count_bound_fires_before_the_record_rule`,
  `hostile_batches_admit_the_full_record_ceiling`).
- The corpus evidence is executed by `tests/protocol_corpus.rs` through the
  same surface: `protocol.server.HostileSpawn` registers a decode and an
  encode route under the `mornlea_protocol` consumer, produced by the real Go
  codec in
  `packages/tools/cmd/runtime-oracle/protocol_hostiles_test.go`. The frozen
  cases are the canonical vector pair, the 64-record ceiling, the zero
  identity, the depths dimension, the two health boundaries, the kind refusal
  and its encode twin, the non-finite position, the descending order refusal,
  and the padding refusal at the truncation category.

## Hostile state (`src/hostile_state.rs`, `tests/runtime_contract.rs`, `tests/protocol_hostiles.rs`)

- Play packet ID 23 payload is a `u64` server tick, a one-byte record count,
  and the fixed 38-byte state records: ID, position, velocity, yaw, health,
  and kind. The dimension is not on the wire because a dimension change
  always goes through a despawn/spawn pair. The same count, ordering, health,
  and kind bounds as the spawn batch apply
  (`hostile_state_round_trip_preserves_batch_bytes`,
  `hostile_state_rejects_invalid_records_and_malformed_payload`).
- The family carries the hostile group's common fallible surface. Its record
  is the spawn record with the dimension exchanged for the velocity, so the
  gate order is the finiteness of the position, the velocity and the yaw as
  one message, then the health span, then the closed kind match, with the
  count bound and the strict identity order in the batch gate. The 8-byte
  difference between the two strides is the dimension-for-velocity exchange,
  which the group test pins directly
  (`hostile_state_round_trips_through_the_fallible_surface`,
  `hostile_state_invalid_value_wins_over_short_capacity`,
  `hostile_state_omits_dimension_and_spawn_omits_velocity`).
- The corpus evidence is executed by `tests/protocol_corpus.rs` through the
  same surface: `protocol.server.HostileState` registers a decode and an
  encode route under the `mornlea_protocol` consumer, produced by the real Go
  codec in
  `packages/tools/cmd/runtime-oracle/protocol_hostiles_test.go`. The frozen
  cases are the canonical vector pair, the 64-record ceiling, the zero
  identity, the non-finite velocity, the health-above boundary, the kind
  refusal, the duplicate identity refusal, the count-bound refusal, and the
  padding refusal at the truncation category.

## Player state (`src/player_state.rs`, `tests/runtime_contract.rs`)

- Play packet ID 3 payload is the fixed 93-byte body, survival, and
  world-time record. It is the only family that carries health, oxygen,
  hunger, the day phase offset, weather, season, in-season progress,
  temperature, and armor points, so those value ranges live in this module
  rather than in a shared value module no other family consumes.
- Out-of-range weather, season, and armor values are rejected outright
  instead of being clamped: a wire value outside the authoritative domain is
  a protocol violation. Season progress and temperature have no sub-range
  because they are a whole `u8` and an `i8`.
- The mining block is validated as one unit. An inactive block must be
  entirely empty so a client never has to guess whether a stale target still
  applies; an active block must report progress strictly below the
  requirement, so a completed swing is published as inactive
  (`player_state_round_trip_preserves_golden_bytes`,
  `player_state_rejects_out_of_range_fields_and_malformed_payload`).
- The family carries the player and private outcome group's common fallible
  surface: `validate(&self)` rechecks every public field on each call,
  checked `encoded_len(&self)` sizes the fixed 93-byte stride,
  `encode_into(&self, dst)` publishes into a caller-owned buffer through the
  crate-private `publish_packet`, `encode(&self)` is the allocating wrapper
  over it, and `decode(payload)` stays a bounded read plus `done()` plus
  validation. The dimension field is the checked domain value, so it carries
  no protocol check and the gate's remaining order is the Go validator's; a
  record mutated into an out-of-range scalar, a non-finite vector or a dirty
  mining block after construction is refused instead of silently published,
  and a short destination reports `OutputTooSmall { needed, available }` with
  every destination byte unchanged
  (`player_outcomes_player_state_round_trips_through_the_fallible_surface`,
  `player_outcomes_the_mining_union_is_validated_as_one_unit`).
- The temperature is the full `i8` range and the pitch is never clipped: the
  protocol layer publishes the exact bits the wire carries, so a `-0.0`
  angle and a full-range temperature survive the round trip
  (`player_outcomes_temperature_and_pitch_are_never_clipped`).
- The corpus evidence is executed by `tests/protocol_corpus.rs` through the
  same surface: `protocol.server.PlayerState` registers a decode and an
  encode route under the `mornlea_protocol` consumer, produced by the real
  Go codec in
  `packages/tools/cmd/runtime-oracle/protocol_player_outcomes_test.go`. The
  ticks and the input sequence render as decimal strings, the position,
  velocity and look angles as eight-digit lowercase-hexadecimal bit strings
  so a negative zero stays distinguishable, and the mining target as its
  ordered integer triple.

## Companion spawn (`src/companion_spawn.rs`, `tests/runtime_contract.rs`)

- Play packet ID 17 payload is the 16-byte companion identity, the
  length-prefixed name, the `u64` tick, the dimension, the position, and the
  yaw and pitch. Only the overworld dimension is published, and the pitch
  stays inside the vertical look range so a wrapped angle is rejected instead
  of being normalized on the client. `COMPANION_SPAWN_MAX_WIRE_BYTES` is the
  fixed payload ceiling
  (`companion_spawn_round_trip_preserves_golden_bytes`,
  `companion_spawn_rejects_invalid_identity_name_and_pose`).
- The family carries the companion group's common fallible surface, and its
  gate keeps the Go `CompanionSpawn.Validate` order: the companion name
  first, then the overworld-only dimension, then the finite pose with its
  inclusive half-turn pitch bound. Exactly ±pi/2 admits bit-exact and the next
  float above or below rejects, while the yaw stays unrestricted in range
  (`companions_spawn_round_trips_through_the_fallible_surface`,
  `companions_pitch_limit_is_inclusive_on_both_ends`).
- The corpus evidence is executed by `tests/protocol_corpus.rs` through the
  same surface: `protocol.server.CompanionSpawn` registers a decode and an
  encode route under the `mornlea_protocol` consumer, produced by the real Go
  codec in
  `packages/tools/cmd/runtime-oracle/protocol_companions_test.go`. The frozen
  cases are the boundaries where both implementations agree: the canonical
  vector pair, the embedded-space name (Go's folded validator message
  resolves as a value boundary, matching `InvalidString`), the pitch above the
  limit (`InvalidFloat`), and the NaN yaw the Go float primitive answers
  before the validator.
- **Latent cross-implementation boundary class:** the Go validator folds the
  identity, the name, the dimension and the pose into one predicate with the
  single message `network: invalid companion spawn`, so a zero or
  wrong-version identity and the depths dimension cannot publish distinct
  categories on the Go side. Those boundaries are pinned as Rust group-test
  assertions instead of corpus cases — `InvalidIdentity` for the identity and
  `InvalidEnum` for dimension 1, both on the decode path
  (`companions_absent_identity_is_unconstructible_and_decode_refuses_it`,
  `companions_spawn_latent_boundaries_are_pinned_here_not_in_the_corpus`).
  Unlike the identity, the dimension is a constructible domain value, so the
  outbound surface can name the depths and the packet gate refuses it there
  too; this is the same latent class the remote-player and furnace families
  record, and it is recorded here rather than resolved by weakening the Rust
  gates to match the Go message coarseness.

## Companion states (`src/companion_states.rs`, `tests/runtime_contract.rs`)

- Play packet ID 18 payload is a `u64` tick, a canonical uvarint count, and
  the fixed 41-byte records of identity, dimension, position, yaw, pitch, and
  reset. The count is bounded by `MAX_COMPANION_STATES` (`4`), which is the
  companion activity limit; identities are ordered by unsigned byte order
  through `strictly_increasing_ids`, and the name rule does not apply because
  this family carries no name
  (`companion_states_round_trip_preserves_batch_bytes`,
  `companion_states_rejects_unsorted_invalid_and_malformed_payload`).
- The family carries the companion group's common fallible surface, and its
  gate keeps the Go `CompanionStates.Validate` order: the count bound, then
  each record's overworld dimension and finite pose with its inclusive pitch
  bound, then the strictly increasing identity order. The batch applies the
  exact-remaining-length rule rather than the minimum-records rule the
  remote-player batch uses, because the Go decoder compares the remaining
  bytes against the declared count, so a payload with one extra byte reports
  the truncation boundary on both sides
  (`companions_states_round_trips_through_the_fallible_surface`,
  `companions_count_bound_fires_before_the_record_scan`,
  `companions_decode_rejects_one_trailing_byte`).
- The corpus evidence is executed by `tests/protocol_corpus.rs` through the
  same surface: `protocol.server.CompanionStates` registers a decode and an
  encode route under the `mornlea_protocol` consumer, produced by the real Go
  codec in
  `packages/tools/cmd/runtime-oracle/protocol_companions_test.go`. The frozen
  cases are the canonical vector pair, the full four-record boundary, the
  count-bound refusals, the two order refusals, the pitch above the limit,
  and the length refusal the exact-record rule answers.

## Drop identity (`src/drop_id.rs`, `src/item_drop_upserts.rs`, `src/item_drop_removes.rs`, `tests/runtime_contract.rs`)

- `DropId` is the stable identity of one authoritative drop: an `i32`
  dimension, two `i32` chunk coordinates, a `u8` slot, and a `u32`
  generation, in the 17-byte wire order. The value is the domain's checked
  `DropId`, re-exported here; the slot must fit the fixed per-chunk drop
  array (`32`) and the generation must be non-zero.
- The dimension is deliberately not validated. The Go `DropID.Valid` rule
  checks only the slot range and the generation, so a drop naming an unusual
  dimension is still a publishable identity and must not be rejected by a
  stricter Rust rule.
- `DropId` is a validated newtype, so an out-of-range slot or a zero
  generation cannot be constructed at all. The batch families therefore only
  assert the batch bounds and the identity order, and the identity rejections
  are asserted at `DropId::try_new`.
- The wire read maps the two identity rejections to `InvalidIdentity` rather
  than `InvalidRange`: the Go decoder answers a slot past the fixed array and
  a zero generation with `network: invalid item drop ID` (and `network: item
  drop remove %d: invalid ID`), which is the identity boundary the domain
  publication corpus already freezes for drop-ID slot and generation errors,
  so the protocol error vocabulary keeps one boundary for the same condition
  (`item_drops_the_identity_rejection_is_the_identity_boundary`).
- `MAX_ITEM_DROP_BATCH` (`32`) lives with the identity because both drop
  halves describe the same bounded drop set.

## Item drop upserts (`src/item_drop_upserts.rs`, `tests/runtime_contract.rs`, `tests/protocol_drops.rs`)

- Play packet ID 11 payload is a `u64` server tick, a canonical uvarint
  count, and the fixed 26-byte records of identity, `u32` block index, and
  the five-byte item stack. The block index must stay below
  `MAX_CHUNK_BLOCK_INDEX` and every stack goes through the shared `ItemStack`
  rule, so a drop cannot publish a slot value the inventory families reject
  (`item_drop_upserts_round_trip_preserves_batch_bytes`,
  `item_drop_upserts_rejects_invalid_records_and_malformed_payload`).
- The family carries the item drop group's common fallible surface:
  `validate(&self)` rechecks every public field on each call, checked
  `encoded_len(&self)` sizes the validated batch, `encode_into(&self, dst)`
  publishes into a caller-owned buffer through the crate-private
  `publish_packet`, `encode(&self)` is the allocating wrapper over it, and
  `decode(payload)` stays a bounded read plus `done()` plus validation. The
  gate order is the Go `ItemDropUpserts.Validate` order: the batch count
  bound, then per record the block-index bound and the shared stack rule,
  with the strict identity order checked against the previous record. A
  record mutated into an out-of-range block index, an invalid stack or an
  unordered identity after construction is refused instead of silently
  published, and a short destination reports
  `OutputTooSmall { needed, available }` with every destination byte
  unchanged (`item_drops_upserts_round_trips_through_the_fallible_surface`,
  `item_drops_invalid_value_wins_over_short_capacity`,
  `item_drops_mutated_public_fields_are_never_published`).
- The identity order compares the full `(dimension, chunk x, chunk z, slot,
  generation)` key with the RAW dimension first, exactly as the Go
  `core.DropID.Compare` does, so a −1-dimension record sorts before a
  0-dimension one and neither an unusual dimension nor the exact empty stack
  triple `(0,0,0)` is refused
  (`item_drops_identity_order_compares_the_raw_dimension_first`,
  `item_drops_the_stack_rule_preserves_the_valid_empty_triple`).
- The batch applies the minimum-records rule rather than the
  exact-remaining-length rule: the Go decoder rejects a payload shorter than
  the declared records before it allocates and applies its end-of-payload
  check afterwards, so a short payload reports `Truncated` and a padded one
  reports `TrailingBytes` on both sides
  (`item_drops_decode_rejects_one_trailing_byte`).
- The corpus evidence is executed by `tests/protocol_corpus.rs` through the
  same surface: `protocol.server.ItemDropUpserts` registers a decode and an
  encode route under the `mornlea_protocol` consumer, produced by the real Go
  codec in
  `packages/tools/cmd/runtime-oracle/protocol_drops_test.go`. The frozen
  cases are the canonical vector pair (the raw dimensions −1 and 256, block
  indexes 0 and 98303, one ordinary stack and one exact empty triple), the
  block-index boundary and its encode twin, the identity boundary (zero
  generation and slot 32, which the Go decoder answers with its invalid-drop-ID
  message), the count above the stack limit, the two order refusals, and the
  trailing-byte refusal the minimum-records rule answers.
- **Latent cross-implementation boundary class:** the Go validator folds an
  unregistered item number, a count above the item's stack limit and a
  durability violation into the single message `network: invalid item drop
  stack`, while the Rust rule answers an unregistered number with
  `InvalidEnum` and the count and durability boundaries with `InvalidRange`.
  The corpus freezes the count boundary, where both sides publish the value
  boundary, and the unregistered item number is pinned as a Rust group-test
  assertion instead of a corpus case
  (`item_drops_unregistered_item_number_is_pinned_here_not_in_the_corpus`);
  this is the same latent class the furnace and companion families record,
  and it is recorded here rather than resolved by weakening the Rust gates to
  match the Go message coarseness.

## Item drop removes (`src/item_drop_removes.rs`, `tests/runtime_contract.rs`, `tests/protocol_drops.rs`)

- Play packet ID 12 payload is a `u64` server tick, a canonical uvarint
  count, and the fixed 17-byte drop identities. The two drop halves are
  separate modules because each has its own wire entry point and packet ID,
  and they share the identity space, the batch ceiling, and the count header
  (`item_drop_removes_round_trip_preserves_batch_bytes`,
  `item_drop_removes_rejects_invalid_ids_and_malformed_payload`).
- The family carries the item drop group's common fallible surface, and its
  gate is count-first: the batch count bound, then the strictly increasing
  identity order over the same full key the upsert half compares. Identity
  validity is enforced where the identity is read, so the gate restates no
  rule the domain `DropId` owns, and a batch mutated into an empty or
  over-full record set or an unordered identity after construction is refused
  instead of silently published
  (`item_drops_removes_round_trips_through_the_fallible_surface`,
  `item_drops_invalid_value_wins_over_short_capacity`).
- The batch applies the same minimum-records rule and end-of-payload check as
  the upsert half, because the Go decoder validates the record budget before
  it allocates and reports the remainder as trailing bytes afterwards
  (`item_drops_decode_rejects_one_trailing_byte`).
- The corpus evidence is executed by `tests/protocol_corpus.rs` through the
  same surface: `protocol.server.ItemDropRemoves` registers a decode and an
  encode route under the `mornlea_protocol` consumer, produced by the real Go
  codec in
  `packages/tools/cmd/runtime-oracle/protocol_drops_test.go`. The frozen
  cases are the canonical vector pair, the one-record and full 32-record
  boundaries, the two count-bound refusals (the count bound fires before the
  remaining-bytes rule), the two order refusals including the
  cross-dimension descending twin, and the zero-generation identity
  refusal.

## Remote player spawn (`src/remote_player_spawn.rs`, `tests/runtime_contract.rs`)

- Play packet ID 7 payload is the 16-byte player identity, the
  length-prefixed display name, the `u64` server tick, the dimension, the
  position, and the yaw and pitch. Unlike a companion or a mob, a remote
  player may appear in either playable dimension because it mirrors a peer
  session. Its name rule is the plain canonical display name, not the
  companion rule that also rejects embedded whitespace, because a player
  display name may legitimately contain spaces
  (`remote_player_spawn_round_trip_preserves_golden_bytes`,
  `remote_player_spawn_rejects_invalid_identity_name_and_pose`).
- The family carries the remote-player group's common fallible surface, and
  its gate keeps the Go `RemotePlayerSpawn.Validate` order: the canonical
  display name first, then the pose finiteness. The pitch stays
  unrestricted — a mirrored peer pose is published as observed — so the
  companion vertical-look rule is deliberately not imported
  (`remote_players_spawn_round_trips_through_the_fallible_surface`,
  `remote_players_pitch_and_the_finite_range_stay_wire_valid`).
- The corpus evidence is executed by `tests/protocol_corpus.rs` through the
  same surface: `protocol.server.RemotePlayerSpawn` registers a decode and
  an encode route under the `mornlea_protocol` consumer, produced by the real
  Go codec in
  `packages/tools/cmd/runtime-oracle/protocol_remote_players_test.go`. The
  frozen cases are the boundaries where both implementations agree: the
  canonical vector pair, the padded name (Go's combined validator message
  resolves as a value boundary, matching `InvalidString`), and the non-finite
  pitch (the Go float primitive fires before the validator, matching
  `InvalidFloat`).
- **Latent cross-implementation boundary class:** the Go validator folds the
  identity, the name, the dimension and the finiteness into one predicate
  with the single message `network: invalid remote player spawn`, so the zero
  and wrong-version identity and the unknown dimension cannot publish
  distinct categories on the Go side. Those three boundaries are pinned as
  Rust group-test assertions instead of corpus cases — `InvalidIdentity` for
  the two identity forms and `InvalidEnum` for the dimension, both on the
  decode path — and the outbound surface cannot express either value at all
  because `PlayerId` and `Dimension` are checked domain newtypes
  (`remote_players_spawn_latent_boundaries_are_pinned_here_not_in_the_corpus`).
  This is the same latent class the furnace family records for its
  unregistered-item boundary, and it is recorded here rather than resolved by
  weakening the Rust gates to match the Go message coarseness.

## Remote player states (`src/remote_player_states.rs`, `tests/runtime_contract.rs`)

- Play packet ID 9 payload is a `u64` server tick, a canonical uvarint
  count, and the fixed 41-byte records of identity, dimension, position,
  yaw, pitch, and reset. The count is bounded by
  `MAX_REMOTE_PLAYER_STATES` (`7`), the peer-session budget, which is a
  different ceiling from the companion activity limit even though the record
  stride is the same.
- This is the only uvarint-count family that also carries a fixed wire
  ceiling, `REMOTE_PLAYER_STATES_MAX_WIRE_BYTES` (`296`), which the Go
  decoder applies before it allocates
  (`remote_player_states_round_trip_preserves_golden_bytes`,
  `remote_player_states_rejects_unsorted_invalid_and_malformed_payload`).
- The family carries the remote-player group's common fallible surface, and
  its gate keeps the Go `RemotePlayerStates.Validate` order: the count bound
  first, then each record's finiteness, then the strictly increasing
  identity order in the raw unsigned byte order the Go `bytes.Compare`
  applies. A public-field batch mutated into an empty or over-full record
  set, a duplicate or descending identity, or a non-finite pose after
  construction is refused instead of silently published, and the fixed wire
  ceiling refuses before any record is read while the count bound fires
  before the record-length rule
  (`remote_players_states_admit_the_full_batch_at_the_fixed_wire_ceiling`,
  `remote_players_states_the_fixed_ceiling_refuses_before_any_record_is_read`,
  `remote_players_invalid_value_wins_over_short_capacity`).
- The corpus evidence is executed by `tests/protocol_corpus.rs` through the
  same surface: `protocol.server.RemotePlayerStates` registers a decode and
  an encode route under the `mornlea_protocol` consumer, produced by the real
  Go codec in
  `packages/tools/cmd/runtime-oracle/protocol_remote_players_test.go`. Every
  boundary the Go decoder names is its own message — the count bound, the
  remaining-length check, the strict-order rule and the per-record combined
  predicate — so the count, duplicate, reversed, dimension and non-finite
  cases each classify at the boundary both sides publish, and the two
  boundary-admitting counts (one and seven records) freeze the count domain
  beside the fixed wire ceiling.

## Passive spawn (`src/passive_spawn.rs`, `tests/runtime_contract.rs`, `tests/protocol_passives.rs`)

- Play packet ID 26 payload is a `u64` server tick, a one-byte record count,
  and the fixed 29-byte spawn records of ID, dimension, position, yaw, and
  health. The record has the same field face as a hostile spawn record minus
  the kind byte, because a passive mob has no category to publish. The count
  is bounded by `MAX_PASSIVE_SPAWN_RECORDS` (`64`), which is the protocol
  budget the decoder accepts, not the smaller live capacity the authority
  converges on
  (`passive_spawn_round_trip_preserves_batch_bytes`,
  `passive_spawn_rejects_invalid_records_and_malformed_payload`).
- The family carries the passive group's common fallible surface, and its
  record gate keeps the Go `PassiveSpawnRecord.validate` order: the
  dimension, the pose finiteness and the health span. The identity is
  already checked by the domain `PassiveId`, so the record gate restates no
  identity rule, and the batch gate adds the count bound and the strict
  identity order in the Go batch order. A record mutated into a foreign
  dimension, a non-finite pose or an out-of-range health after construction
  is refused instead of silently published, and a short destination reports
  `OutputTooSmall { needed, available }` with every destination byte
  unchanged (`passive_spawn_round_trips_through_the_fallible_surface`,
  `passive_spawn_invalid_value_wins_over_short_capacity`,
  `passive_health_span_is_one_to_twenty`).
- The dimension is the raw wire `i32` matched against the two known dimension
  IDs rather than narrowed to a `u8`, so a value such as `256` is an
  `InvalidEnum` instead of a reinterpreted dimension, and the negative-zero
  pose bits survive the round trip
  (`passive_spawn_dimension_is_matched_against_the_known_ids`,
  `passive_records_preserve_negative_zero_pose_bits`).
- The batch applies the exact-remaining-length rule, the count bound fires
  before it on both sides, and the 64-record ceiling is admitted on all three
  passive families with the 32-actor live cap named in the group test as the
  authority-only concern it is (`passive_count_bound_fires_before_the_record_rule`,
  `passive_batches_admit_the_full_record_ceiling`).
- The corpus evidence is executed by `tests/protocol_corpus.rs` through the
  same surface: `protocol.server.PassiveSpawn` registers a decode and an
  encode route under the `mornlea_protocol` consumer, produced by the real Go
  codec in
  `packages/tools/cmd/runtime-oracle/protocol_passives_test.go`. The frozen
  cases are the canonical vector pair, the 64-record ceiling, the zero
  identity, the depths dimension, the two health boundaries with the
  health-above encode twin, the non-finite position, the descending order
  refusal, and the padding refusal at the truncation category.

## Passive state (`src/passive_state.rs`, `tests/runtime_contract.rs`, `tests/protocol_passives.rs`)

- Play packet ID 27 payload is a `u64` server tick, a one-byte record count,
  and the fixed 38-byte state records of ID, position, velocity, yaw, health,
  and the grazing bit. The dimension is not on the wire for the same reason
  as the hostile state batch. Grazing is a transient presentation observation
  that is never persisted, so it is validated as a 0/1 value rather than
  reinterpreted
  (`passive_state_round_trip_preserves_batch_bytes`,
  `passive_state_rejects_invalid_records_and_malformed_payload`).
- The family carries the passive group's common fallible surface. Its record
  is the spawn record with the dimension exchanged for the velocity and the
  grazing bit closing the record, so the gate order is the finiteness of the
  position, the velocity and the yaw as one message, then the health span,
  then the closed grazing match, with the count bound and the strict identity
  order in the batch gate. The 9-byte difference between the two strides is
  the dimension-for-velocity exchange plus the grazing bit, which the group
  test pins directly (`passive_state_round_trips_through_the_fallible_surface`,
  `passive_state_invalid_value_wins_over_short_capacity`,
  `passive_spawn_omits_kind_and_state_omits_dimension`,
  `passive_grazing_and_reason_are_closed_pairs`).
- The corpus evidence is executed by `tests/protocol_corpus.rs` through the
  same surface: `protocol.server.PassiveState` registers a decode and an
  encode route under the `mornlea_protocol` consumer, produced by the real Go
  codec in
  `packages/tools/cmd/runtime-oracle/protocol_passives_test.go`. The frozen
  cases are the canonical vector pair, the 64-record ceiling, the closed
  grazing pair with its grazing-two encode twin, the non-finite velocity, the
  zero identity, the duplicate identity refusal, the count-bound refusal, and
  the padding refusal at the truncation category.

## Projectile spawn (`src/projectile_spawn.rs`, `tests/runtime_contract.rs`, `tests/protocol_projectiles.rs`)

- Play packet ID 29 payload is a `u64` server tick, a one-byte record count,
  and the fixed 37-byte spawn records of ID, kind, dimension, position, and
  velocity. The record carries a kind byte because a projectile's behaviour
  differs by what fired it, but no yaw or health: a projectile is a
  point-like transient whose orientation the client derives from its
  velocity. Both playable dimensions are legal because a player's bow works
  in either one, while the kind-by-dimension policy is an authority concern
  this codec does not enforce
  (`projectile_spawn_round_trip_preserves_batch_bytes`,
  `projectile_spawn_rejects_invalid_records_and_malformed_payload`).
- The identity is the checked domain `ProjectileId` re-exported from
  `src/projectile_id.rs`, so the record gate restates no identity rule: the
  gate keeps the Go `ProjectileSpawnRecord.validate` order of the closed kind
  match and the pose-and-velocity finiteness, and the batch gate adds the
  count bound and the strict identity order in the Go batch order. The
  dimension is the checked domain value read from the raw wire `i32` matched
  against the two known IDs, so a value such as `256` is an `InvalidEnum`
  rather than a reinterpreted dimension.
- The kind and the dimension are independent on this wire: all four
  combinations admit, because a shard in the depths and an arrow in the
  overworld are both publishable records at this boundary. The narrower
  authority rule is deliberately not published here, and the group test pins
  the independence so a later change cannot quietly add the policy to the
  wire (`projectile_kind_and_dimension_are_independent_on_the_wire`,
  `projectile_kind_is_a_closed_match`,
  `projectile_spawn_dimension_is_matched_against_the_known_ids`).
- The family carries the projectile group's common fallible surface, and the
  batch applies the exact-remaining-length rule with the count bound firing
  before it on both sides, so a padded payload answers at the truncation
  boundary and a declared count above the ceiling answers at the count bound
  (`projectile_count_bound_fires_before_the_record_rule`,
  `projectile_batches_admit_the_full_record_ceiling`).
- The decoder applies `PROJECTILE_SPAWN_MAX_WIRE_BYTES` — derived from the
  37-byte stride and the 128-record bound — as a pre-parse size check before
  the header is read, so an over-ceiling payload answers `FrameTooLarge`, the
  capacity boundary the Go fixed maximum publishes
  (`projectile_decode_refuses_an_over_ceiling_payload_before_any_read`).
- The corpus evidence is executed by `tests/protocol_corpus.rs` through the
  same surface: `protocol.server.ProjectileSpawn` registers a decode and an
  encode route under the `mornlea_protocol` consumer, produced by the real Go
  codec in
  `packages/tools/cmd/runtime-oracle/protocol_projectiles_test.go`. The
  frozen cases are the canonical vector pair that carries both kinds across
  both dimensions, the 128-record ceiling, the zero identity, the closed kind
  pair with its encode twin, the dimension boundary, the non-finite position,
  the descending order, the count-bound refusal and the padding refusal the
  exact-length rule answers at the truncation category.

## Projectile state (`src/projectile_state.rs`, `tests/runtime_contract.rs`, `tests/protocol_projectiles.rs`)

- Play packet ID 30 payload is a `u64` server tick, a one-byte record count,
  and the fixed 20-byte state records of ID and position. This is the
  narrowest record in the crate: a projectile's kind, dimension, and velocity
  are fixed for its whole life, so the mirror records them at spawn and the
  state batch only moves the body
  (`projectile_state_round_trip_preserves_batch_bytes`,
  `projectile_state_rejects_invalid_records_and_malformed_payload`).
- The family carries the projectile group's common fallible surface, and its
  record gate restates no identity rule — the identity is the checked domain
  `ProjectileId` — and carries no kind, dimension or velocity gate, because
  this wire publishes none of the three and infers none of them. The batch
  gate adds the count bound and the strict identity order in the Go batch
  order, and the group test pins the stride against the spawn and despawn
  strides so a layout change on any of the three fails before it reaches a
  corpus case (`projectile_state_carries_only_identity_and_position`,
  `projectile_state_invalid_value_wins_over_short_capacity`).
- The decoder applies `PROJECTILE_STATE_MAX_WIRE_BYTES` — derived from the
  20-byte stride and the 128-record bound — as a pre-parse size check before
  the header is read, and the batch applies the exact-remaining-length rule
  with the count bound firing first, so the padded and over-count payloads
  answer the same boundaries the Go decoder publishes
  (`projectile_decode_refuses_an_over_ceiling_payload_before_any_read`,
  `projectile_batches_admit_the_full_record_ceiling`).
- The corpus evidence is executed by `tests/protocol_corpus.rs` through the
  same surface: `protocol.server.ProjectileState` registers a decode and an
  encode route under the `mornlea_protocol` consumer, produced by the real Go
  codec in
  `packages/tools/cmd/runtime-oracle/protocol_projectiles_test.go`. The
  frozen cases are the canonical vector pair, the 128-record ceiling, the
  zero identity, the non-finite position with its encode twin, the duplicate
  order, the count-bound refusal and the padding refusal the exact-length
  rule answers at the truncation category.

## Chat event (`src/chat_event.rs`, `tests/runtime_contract.rs`, `tests/protocol_chat_event.rs`)

- Play packet ID 16 payload is the event ID, the player identity and name,
  the companion identity and name, the kind, the reason, and one text slot.
- The text slot is reused by kind. A companion speech event carries a
  model-generated line bounded by `CHAT_SPEECH_TEXT_MAX_BYTES` (`256`), and
  every other kind restates the player's original command bounded by
  `CHAT_COMMAND_TEXT_MAX_BYTES` (`1024`). The decoder therefore reads the
  kind first and then decides which text to read, and the two fields are
  mutually exclusive on the wire as well as in validation.
- The combination rules are atomic. A speech field on any other kind, a
  command on a speech event, an empty or non-canonical name, an out-of-range
  kind, a reserved rejection reason, or a chat rejection reason on a failed
  task rejects the whole event before any field is applied.
- A format rejection must not leak a companion identity or the command,
  because a malformed command never addressed a companion; a queue-full or
  not-following rejection keeps the same identity and command requirements as
  an acceptance so the player can match the rejection to its command. The
  absent companion identity for those cases is `CompanionId::NONE`
  (`chat_event_round_trip_preserves_golden_bytes`,
  `chat_event_rejects_invalid_kind_combinations_and_malformed_payload`).
- The family carries the common fallible surface (`validate` → checked
  `encoded_len` → capacity check → private `publish_packet`), and its value
  gate publishes the frozen corpus categories: a zero event identity or an
  invalid player identity is `InvalidIdentity`, an unknown kind and the two
  reason-domain boundaries (the reserved reject reason 3 and a failure reason
  outside `16..=20`) are `InvalidEnum`, and every text boundary and illegal
  cross-field combination is `InvalidString`.
- The fixed payload ceiling `CHAT_EVENT_MAX_WIRE_BYTES` (`1328`) is applied
  before any parse, which is the `capacity` category the Go decoder's fixed
  maximum publishes. The ceiling is a guard rather than a record size: the
  widest admissible record is the 128-byte player name, a companion name
  inside its 32-rune bound and the maximum-length command, which reaches the
  bound exactly with three two-byte length prefixes.
- The three length-prefixed text slots read through the shared
  `read_bounded_text` reader, so a declared length the payload cannot
  complete reports `Truncated` rather than the `InvalidString` the shared
  string primitive reports for the same bytes; the Go producer resolves the
  same condition from its own derivation base.
- The bidirectional domain conversion is produced here for the domain
  evidence node: `TryFrom<ChatEvent> for DomainChatEvent` and its reverse
  map the closed `ChatBody` union (seven variants unfolding into the sixteen
  legal branch shapes) to and from the wire record. The raw zero companion
  identity maps to semantic absence in the two permitted rejection branches
  alone, and the reverse conversion is the only publisher of that raw form.
  No conversion fabricates a session recipient, a publish tick or a command
  sequence: those stay in `RoutedEvent` and the chat FIFO.
- The corpus evidence is executed by `tests/protocol_corpus.rs` through the
  same fallible surface; its cases are the twelve legal branch shapes with
  the accepted and speech encode pair, the reserved reject reason, the
  failed-reason domain, the two text-slot boundaries, the identity, name and
  event-identity gates and one trailing byte, produced by the real Go codec in
  `packages/tools/cmd/runtime-oracle/protocol_chat_event_test.go`. The two
  slot-exclusivity combinations are not constructible on the wire, because
  the payload carries one text slot and the decoder assigns it by kind; their
  corpus cases record the branch's own requirement at that slot, and the
  DTO-level exclusivity is pinned by the group test's mutated records.

## Chunk snapshot (`src/chunk_snapshot.rs`, `tests/runtime_contract.rs`)

- Play packet ID 0 payload is an eight-byte envelope — the declared decoded
  length, then the declared compressed length — followed by a zstd frame
  carrying the logical snapshot: dimension, chunk, revision, a section count,
  and one paletted container per section. This is the only family whose wire
  payload is compressed, and it is the only reason this crate is allowed the
  `zstd` dependency.
- `MAX_COMPRESSED_SNAPSHOT` and `MAX_DECODED_SNAPSHOT` are the two ceilings,
  and both are checked before the frame is touched, including when callers use
  the public one-shot helpers. Every declared palette and word count is
  checked against the bytes that remain before the matching
  buffer is allocated, so a corrupt count cannot become a large allocation.
- The zstd frame's declared history window is also bounded by the decoded
  2 MiB ceiling before either decompressor entry point. The owned context's
  `WindowLogMax` alone does not enforce this in libzstd's one-shot bulk path;
  `preflight_zstd_windows` scans every bounded frame and rejects an oversized
  window as `Integrity`. The Go decoder admits exactly 2 MiB and rejects a
  descriptor one eighth above it, as the paired fixture mutation tests pin.
- Section containers are a closed set of three kinds. A `Single` section
  carries one block ID, an `Indexed` section a palette plus 4- or 8-bit slots,
  and a `Direct` section 15-bit slots with no palette. A section must not carry
  a field belonging to another kind, every slot must resolve inside the palette
  or the registered block range, and a direct word must not carry bits above
  its 15-bit slot. Rejecting the combination instead of ignoring it keeps a
  partially described section from being silently reinterpreted.
- The section list is the whole ordered column: the count must be exactly
  `SECTIONS_PER_CHUNK` and each section's Y must equal its index. The decoder
  does not repair a missing or reordered section.
- Encoder output is intentionally not byte-identical to the Go encoder and
  must not be "fixed". The Go payload is built by
  `github.com/klauspost/compress/zstd`, a pure-Go zstd implementation whose
  compressed block payload differs from the reference libzstd bound here at
  every compression level, even though the frame header and the trailing
  xxhash-64 content checksum are byte-identical and the total frame length can
  match. A standalone Go program re-encodes the committed fixture exactly, so
  the divergence is exclusively a Rust-versus-Go encoder difference.
- Cross-implementation compatibility is defined at the logical/decode level:
  zstd frames are self-describing, so the Go decoder reads what this encoder
  writes and this decoder reads what the Go encoder wrote. Acceptance is
  semantic round-trip — exact decode of the committed fixture, lossless
  encode/decode, round-trip through the fixture, and rejection without
  implicit repair — never byte-identical re-encode. Do not add an assertion
  that a frame produced here equals a committed fixture's compressed bytes.
  The frame magic, the frame header descriptor, the four-byte content size, and
  the trailing content checksum are pinned separately because those parts are
  identical across the two implementations.
- `tests/testdata/rust-chunk-snapshot-v45.bin` is emitted by the owned Rust
  `ProtocolCodec` for the same golden snapshot as the Go fixture. The Rust
  snapshot suite pins the fixture to current encoder bytes; the Go
  runtime-oracle suite decodes both fixtures through the production Go codec,
  compares the complete packets, and rejects a flipped Rust frame checksum.
  The compressed frames may differ, so equality belongs to decoded values.
- Intermediate layers (`encode_logical_checked`, `decode_logical`,
  `decode_envelope`, `SnapshotEnvelope::decompress`, `compress_logical`) are
  public so contract tests can prove decode exactness byte for byte and
  rejection before allocation without reaching into private state. The
  envelope's fields remain private: callers inspect them through getters,
  and decompression rechecks both limits before it allocates
  (`chunk_snapshot_round_trip_preserves_golden_bytes`,
  `chunk_snapshot_round_trips_through_committed_fixture`,
  `chunk_snapshot_rejects_malformed_envelope_and_bounds`,
  `chunk_snapshot_rejects_malformed_logical_payload`).
- `src/codec.rs` owns the family's one zstd context pair. `ProtocolCodec::new`
  creates one compressor and one decompressor, `decode_snapshot` reuses the
  decoder context, and `encode_snapshot_into` reuses the compressor context;
  no global or static context exists, no background thread is started, and no
  call constructs a context of its own. `compress_logical` stays the one-shot
  frame builder the contract tests use to construct malformed payloads from
  mutated logical bytes; production compression runs through the owned context.
  The one-shot helper checks the worst-case scratch bound before constructing
  its zstd context. One call's scratch is bounded by the 2 MiB logical and
  1 MiB compressed ceilings, with `try_reserve` for the compressed scratch and
  `ProtocolError::Allocation` for an unreservable request; the infallible
  `encode`/`encode_logical` entry points are removed, so a record mutated after
  construction is refused instead of panicking inside the encoder
  (`tests/protocol_snapshot.rs`).
- The failure boundaries keep the envelope's checks and the frame's checks
  distinct. A declared compressed length the payload cannot back is
  `Truncated`, both envelope ceilings are `FrameTooLarge` before the frame is
  touched, a frame that decompresses to a different length than the envelope
  declares stays `Truncated`, and a length-complete frame the zstd layer
  rejects is `Integrity` — the compressed stream's own content-checksum
  failure, which the envelope checks could not catch. `tests/protocol_snapshot.rs`
  pins each boundary separately, and the corpus consumer publishes `Integrity`
  as the `integrity` category
  (`snapshot_compressed_layer_negatives_are_refused_at_their_boundary`).
- `SectionData` and the domain's `PalettedSection` are the same compact
  storage in two representations. `TryFrom<SectionData> for PalettedSection`
  moves the palette and packed words through the domain's checked
  constructors; `TryFrom<(PalettedSection, i32)> for SectionData` moves them
  back with the section index supplied separately, because the wire states
  each section's `Y` while the domain holds the column as a fixed array whose
  position is the index. Neither direction expands the 4096 cells into block
  IDs, sorts a palette, or recompresses slots, so the round trip preserves the
  exact palette and word order
  (`indexed_section_conversion_keeps_palette_and_word_order`,
  `single_and_direct_section_conversions_round_trip`).

## Shared value rules

- `mornlea_protocol` reuses `mornlea_domain::{PlayerId, CompanionId,
  ItemStack, DropId}` and their checked tables. The shim modules
  (`src/player_id.rs`, `src/entity_id.rs`, `src/item_stack.rs`,
  `src/drop_id.rs`) re-export the domain value and hold only the wire edge:
  the fixed-stride read/write helpers, the frozen item-number constants, and
  the domain-rejection-to-`ProtocolError` mapping. A packet family never
  restates an identity, item, or name rule.
- A wire `ContainerRef` keeps the exact v45 raw representation: the dimension
  is the wire `i32`, never narrowed, and the exact all-zero record is the one
  absent sentinel. `to_domain_present` rejects that record and every invalid
  real reference; `to_domain_optional` maps only the exact zero record to
  `None`. The domain `ContainerRef` owns the slot and generation bounds, so
  no container rule lives in this crate beside the per-kind array sizes the
  wire pins.
- Wire-only exceptions stay explicit and raw: the absent companion identity
  in the two permitted chat rejection branches, and armor-owned broken
  pieces, which are not ordinary `ItemStack` values and stay out of the
  inventory families. The chat event therefore carries its companion slot as
  raw 16 bytes and interprets them through `names_companion`, so the zero
  form never becomes a domain identity.
- The pinned whitespace set and the canonical text rules live in
  `mornlea_domain`. `mornlea_domain::trim_pinned_whitespace` borrows the
  trimmed remainder, and admission paths (the login start today) trim by it
  before constructing a domain text value, so one lexical rule decides every
  step and no `str::trim` Unicode-table shortcut enters the crate.
- Compact section storage is shared through the two checked conversions in
  `src/chunk_snapshot.rs`; see the chunk snapshot section for the move
  semantics.
- The crate-private `valid_bounded_text` (`src/chat_command.rs`) is the one
  local text rule left: only the chat event's speech slot still consumes it,
  and routing that slot through the domain `SpeechText` is a later node's
  change. The command slot itself routes through the domain `CommandText`, so
  no local copy of that rule remains.
- A family whose Go `Validate` checks fewer fields than the Rust newtype
  enforces is a parity break. Where the Go rule is narrower, as with
  `DropID.Valid` and the dimension, the Rust rule is narrowed to match rather
  than the Go rule being treated as incomplete.
- Per-packet validation behavior is owned by the packet nodes: this crate's
  shared-value layer removes duplicate rules and the dimension narrowing
  without strengthening or weakening any packet's own gates. The four
  stack-view families validate their reference through `validate_any` and the
  shared `validate_stack_view` gate, so a malformed real reference is refused
  at the packet boundary.

`tests/protocol_snapshot.rs` is the compressed family's group suite: the
committed Go fixture's logical payload is reproduced byte for byte and its frame
pins are checked against a Rust re-encoding, `ProtocolCodec` round-trips the
canonical mixed vector and every-single vector through one owned context, the
sections-mutation red is pinned as typed errors instead of panics, a short
destination is refused with every byte unchanged, an invalid value wins over a
short destination, and each compressed-layer boundary (both envelope ceilings,
a truncated frame, a checksum failure) is pinned at its own error variant. It
needs no corpus files, so it runs before the controller integrates the exported
candidates.

## Focused Verification

```bash
rustup run 1.97.1 cargo test --manifest-path packages/engine/Cargo.toml -p mornlea_protocol --test protocol_registry --locked -- --list
rustup run 1.97.1 cargo test --manifest-path packages/engine/Cargo.toml -p mornlea_protocol --test protocol_registry --locked
rustup run 1.97.1 cargo test --manifest-path packages/engine/Cargo.toml -p mornlea_protocol --test protocol_semantic --locked -- --list
rustup run 1.97.1 cargo test --manifest-path packages/engine/Cargo.toml -p mornlea_protocol --test protocol_semantic --locked
rustup run 1.97.1 cargo test --manifest-path packages/engine/Cargo.toml -p mornlea_domain --test event_surface --locked
rustup run 1.97.1 cargo test --manifest-path packages/engine/Cargo.toml -p mornlea_protocol --test protocol_values --locked -- --list
rustup run 1.97.1 cargo test --manifest-path packages/engine/Cargo.toml -p mornlea_protocol --test protocol_values --locked
rustup run 1.97.1 cargo test --manifest-path packages/engine/Cargo.toml -p mornlea_protocol --test protocol_admission --locked -- --list
rustup run 1.97.1 cargo test --manifest-path packages/engine/Cargo.toml -p mornlea_protocol --test protocol_admission --locked
rustup run 1.97.1 cargo test --manifest-path packages/engine/Cargo.toml -p mornlea_protocol --test runtime_contract --locked -- --list
rustup run 1.97.1 cargo test --manifest-path packages/engine/Cargo.toml -p mornlea_protocol --test runtime_contract --locked
rustup run 1.97.1 cargo test --manifest-path packages/engine/Cargo.toml -p mornlea_protocol --test protocol_frame --locked
rustup run 1.97.1 cargo test --manifest-path packages/engine/Cargo.toml -p mornlea_protocol --test protocol_control --locked
rustup run 1.97.1 cargo test --manifest-path packages/engine/Cargo.toml -p mornlea_protocol --test protocol_client_control --locked
rustup run 1.97.1 cargo test --manifest-path packages/engine/Cargo.toml -p mornlea_protocol --test protocol_client_rays --locked
rustup run 1.97.1 cargo test --manifest-path packages/engine/Cargo.toml -p mornlea_protocol --test protocol_client_inventory --locked
rustup run 1.97.1 cargo test --manifest-path packages/engine/Cargo.toml -p mornlea_protocol --test protocol_client_stack_views --locked
rustup run 1.97.1 cargo test --manifest-path packages/engine/Cargo.toml -p mornlea_protocol --test protocol_client_chat --locked
rustup run 1.97.1 cargo test --manifest-path packages/engine/Cargo.toml -p mornlea_protocol --test protocol_world_delta --locked
rustup run 1.97.1 cargo test --manifest-path packages/engine/Cargo.toml -p mornlea_protocol --test protocol_world_delta --locked -- --list
rustup run 1.97.1 cargo test --manifest-path packages/engine/Cargo.toml -p mornlea_protocol --test protocol_snapshot --locked
rustup run 1.97.1 cargo test --manifest-path packages/engine/Cargo.toml -p mornlea_protocol --test protocol_snapshot --locked -- --list
rustup run 1.97.1 cargo test --manifest-path packages/engine/Cargo.toml -p mornlea_protocol --test protocol_player_outcomes --locked
rustup run 1.97.1 cargo test --manifest-path packages/engine/Cargo.toml -p mornlea_protocol --test protocol_player_outcomes --locked -- --list
rustup run 1.97.1 cargo test --manifest-path packages/engine/Cargo.toml -p mornlea_protocol --test protocol_inventory_publication --locked
rustup run 1.97.1 cargo test --manifest-path packages/engine/Cargo.toml -p mornlea_protocol --test protocol_inventory_publication --locked -- --list
rustup run 1.97.1 cargo test --manifest-path packages/engine/Cargo.toml -p mornlea_protocol --test protocol_remote_players --locked
rustup run 1.97.1 cargo test --manifest-path packages/engine/Cargo.toml -p mornlea_protocol --test protocol_remote_players --locked -- --list
rustup run 1.97.1 cargo test --manifest-path packages/engine/Cargo.toml -p mornlea_protocol --test protocol_companions --locked
rustup run 1.97.1 cargo test --manifest-path packages/engine/Cargo.toml -p mornlea_protocol --test protocol_companions --locked -- --list
rustup run 1.97.1 cargo test --manifest-path packages/engine/Cargo.toml -p mornlea_protocol --test protocol_drops --locked
rustup run 1.97.1 cargo test --manifest-path packages/engine/Cargo.toml -p mornlea_protocol --test protocol_drops --locked -- --list
rustup run 1.97.1 cargo test --manifest-path packages/engine/Cargo.toml -p mornlea_protocol --test protocol_hostiles --locked
rustup run 1.97.1 cargo test --manifest-path packages/engine/Cargo.toml -p mornlea_protocol --test protocol_hostiles --locked -- --list
rustup run 1.97.1 cargo test --manifest-path packages/engine/Cargo.toml -p mornlea_protocol --test protocol_passives --locked
rustup run 1.97.1 cargo test --manifest-path packages/engine/Cargo.toml -p mornlea_protocol --test protocol_passives --locked -- --list
rustup run 1.97.1 cargo test --manifest-path packages/engine/Cargo.toml -p mornlea_protocol --test protocol_projectiles --locked
rustup run 1.97.1 cargo test --manifest-path packages/engine/Cargo.toml -p mornlea_protocol --test protocol_projectiles --locked -- --list
rustup run 1.97.1 cargo test --manifest-path packages/engine/Cargo.toml -p mornlea_protocol --test protocol_chat_event --locked
rustup run 1.97.1 cargo test --manifest-path packages/engine/Cargo.toml -p mornlea_protocol --test protocol_chat_event --locked -- --list
rustup run 1.97.1 cargo test --manifest-path packages/engine/Cargo.toml -p mornlea_protocol --test protocol_corpus --locked
```

`tests/protocol_registry.rs` pins the closed typed dispatch: one reviewed
Go-produced payload per key decodes and re-encodes through the real boundary,
the 23 client and 36 server keys are asserted by key and variant rather than by
count, the three table-tamper detections (dropped row, duplicated key, swapped
server IDs) report the offending key, and the reserved C/Play/1, the unassigned
C/Play/22 and S/Play/32, an unknown state under every ID, a login payload under
a Play ID and an over-ceiling payload are all refused at their own boundary.
The one family whose re-encoding is not byte-identical is the compressed
snapshot, whose acceptance is a semantic round trip through the owned context.
It needs no corpus files, so it runs before the controller integrates the
exported candidates.

`tests/protocol_semantic.rs` pins the adapter seam. It enumerates every
registered client and server key in a table, asserts the key each packet
publishes and the intent or event label the adapter answers with, and asserts
the executed counts (19 sequenced commands, one chat intent, one keep alive
reply and two negotiation refusals; 30 publications and six control
refusals). Every non-refused intent and event round-trips through the real
wire boundary (`encode_client_into` / `decode_client` and
`ProtocolCodec::encode_server_into` / `decode_server`) and back into an equal
semantic value. The suite also pins the 5-record companion rejection, every
wire cap boundary from the admitted count to the refused one (including the
4096-record world-delta batches and the domain work cap that refuses 4097
before the adapter is reached), a nonzero container reference with dimension
256 / −1 / 1 failing rather than aliasing into the overworld, the exact absent
companion identity mapping to variant-shaped absence while a companion-bearing
branch publishes the checked identity, six normalized-expectation mutations
(sequence, world dimension, actor health, record order, chat kind, chat and
command reject reason), and the absence of any `CommandEnvelope` construction
in the adapter source.

`tests/protocol_values.rs` pins the shared-value boundary: the raw container
dimension is kept raw through `read` and refused by the checked conversion
instead of being narrowed, the exact zero record is the
only absent reference, the item rules have the domain's single owner, the
compact section conversions preserve palette and word order, and the pinned
whitespace set is the only trim rule.

`tests/protocol_admission.rs` pins the inbound split: a version-44 hello
reaches `validate_hello` and answers with the negotiated version pair, a zero
identity wins over an out-of-domain distance, a raw name trim reduces to the
canonical name, and the structural boundaries (truncation, trailing bytes,
invalid UTF-8, the 64 KiB payload ceiling) reject inside `decode_inbound`. Its
case identities are shared with
`packages/shared/network/protocol_admission_oracle_test.go`.

`tests/protocol_control.rs` pins the control group's common surface: the
reviewed wire bytes of each family's canonical record round-trip through
`validate`/`encoded_len`/`encode_into`/`encode`, a short destination is refused
with `OutputTooSmall` and left untouched, an invalid value wins over a short
destination for every mutable field, every proper truncation of every canonical
payload rejects, one trailing byte rejects, and an incomplete message payload
reports `Truncated`. It needs no corpus files, so it runs before the
controller integrates the exported candidates.

`tests/protocol_client_control.rs` pins the four Play client-to-server control
records through the same surface: the reviewed wire bytes round-trip through
`validate`/`encoded_len`/`encode_into`/`encode`, a `-0.0` look angle keeps its
`0x80000000` bit pattern, the full `i8` axes and the unrestricted pitch are not
clamped, the resync family's total gate survives extreme legal field values,
and the resync dimension is refused by ID match rather than by a `u8` narrowing
cast. It also needs no corpus files.

`tests/protocol_client_rays.rs` pins the five Play client-to-server ray action
records through that same surface: the 16-byte reviewed wire literal round-trips
for every family, the `-0.0` yaw keeps its `0x80000000` bit pattern, a short or
invalid destination leaves every caller byte untouched, a mutated non-finite
angle wins over a short destination for each family, and every proper truncation
plus one trailing byte reject. It also needs no corpus files.

`tests/protocol_client_inventory.rs` pins the six simple inventory and crafting
command records through that same surface: the 10-byte move and 8-byte
sequence-only reviewed wire literals round-trip for every family, a short
destination leaves every caller byte untouched, a mutated slot pair or a mutated
zero `TakeCraftingOutput` sequence wins over a short destination, the three total
sequence-only families re-encode a `u64::MAX` sequence byte-exactly, and every
proper truncation plus one trailing byte reject. It also needs no corpus files.

`tests/protocol_client_stack_views.rs` pins the four container and
view-addressed stack records through that same surface: the reviewed wire
literals (28/30/28/28 bytes) round-trip for every family, the exact all-zero
reference is the only absent form the inventory and crafting views accept, a
malformed real reference (foreign dimension, unknown kind, zero generation,
out-of-range physical slot) refuses at the packet boundary, the
`MoveStackPartial` family keeps the authority-only crafting and
furnace-output rules off the wire, and every proper truncation plus one
trailing byte reject. It also needs no corpus files.

`tests/protocol_player_outcomes.rs` pins the four owner-private records
through that same fallible surface: the reviewed 93/9/8/10-byte wire literals
round-trip byte for byte with the negative-zero bits preserved, the reject
reason translates through the explicit closed matrix in both directions with
the interval boundaries refused, the temperature covers the full `i8` range
and the pitch is never clipped, the mining union refuses any inactive residue
and any completed or unstarted swing, a mutated public field wins over a short
destination for every family, every proper truncation plus one trailing byte
reject, and the packet IDs 3/4/20/25 stay pinned. It also needs no corpus
files, so it runs before the controller integrates the exported candidates.

`tests/protocol_inventory_publication.rs` pins the five inventory and
container publication records through that same surface: the reviewed
181/51/36/153/18-byte wire literals round-trip byte for byte with the exact
container-reference bytes preserved, the personal crafting grid refuses
residue in every extension cell while the workbench admits all nine, the
furnace gate invents no timer-versus-stack relation (an idle furnace beside
the active bounds both publish), the item-stack boundaries are pinned through
the decode path (full stack, durability bounds, the canonical empty triple,
and the unregistered item number's own enum boundary), the exact all-zero
container reference is refused through its zero generation, a mutated public
field wins over a short destination for every family, every proper truncation
plus one trailing byte reject, and the packet IDs 10/21/13/15/14 stay pinned.
It also needs no corpus files, so it runs before the controller integrates
the exported candidates.

`tests/protocol_remote_players.rs` pins the three remote-player publication
records through that same surface: the reviewed 54/16/91/296-byte wire
literals round-trip byte for byte with the negative-zero position components
and the wire-valid pitch 2.0 preserved, the full seven-record batch admits at
exactly the 296-byte fixed ceiling while one byte above it refuses
`FrameTooLarge` before any record is read and a count of eight refuses at the
count bound rather than the record-length rule, the identity order is the raw
unsigned byte order (a byte-ascending pair admits and its descending twin is
refused), the pitch carries no vertical-look rule, a mutated public field —
padded name, non-finite pose, duplicate or descending identity, empty or
over-full batch — wins over a short destination for every family, every
proper truncation plus one trailing byte reject, and the packet IDs 7/8/9
stay pinned. Its latent-boundary test pins the spawn's zero and wrong-version
identity and its unknown dimension at their Rust variants because the Go
validator's single message cannot separate them. It also needs no corpus
files, so it runs before the controller integrates the exported candidates.

`tests/protocol_companions.rs` pins the three companion publication records
through that same surface: the reviewed 53/16/50/173-byte wire literals
round-trip byte for byte with the negative-zero position components and the
boundary pitch preserved, the half-turn pitch limit is inclusive in both
directions (exactly ±pi/2 publishes its exact bits and the next float above
or below refuses) while the yaw keeps its full finite range, the full
four-record batch admits at exactly the fixed wire ceiling, the count bound
fires before the record scan and the exact-remaining-length rule answers a
payload whose declared count disagrees with the record bytes present, the
identity order is the raw unsigned byte order, the despawn's 16-byte fixed
bound refuses before any byte is read, a mutated public field — embedded-space
or padded name, foreign dimension, out-of-range or non-finite pose, duplicate
or descending identity, empty batch — wins over a short destination for every
family, every proper truncation plus one trailing byte reject, and the packet
IDs 17/18/19 stay pinned. Its latent-boundary test pins the spawn's zero and
wrong-version identity and the depths dimension at their Rust variants
because the Go validator's single message cannot separate them, and its
absent-identity test pins that the zero UUID is unconstructible on this
surface and refused on every decode path. It also needs no corpus files, so
it runs before the controller integrates the exported candidates.

`tests/protocol_drops.rs` pins the two item drop publication records through
that same surface: the reviewed 61-byte upsert and 26-byte remove literals
round-trip byte for byte with the raw dimensions −1 and 256 preserved
verbatim, the inclusive block-index boundary (98303 admits, 98304 refuses
without clamping), the exact empty stack triple and the non-canonical empty
refusals, the 32-record ceiling on both halves, the count bound before the
record rule, the identity order with the raw dimension first (including a
cross-dimension ordered pair admitted and its reverse refused), the
minimum-records batch rule (every proper truncation plus one trailing byte
reject), the identity boundary for a slot past the fixed array and a zero
generation, and the packet IDs 11/12. Its latent-boundary test pins the
unregistered item number at the Rust `InvalidEnum` variant because the Go
validator's single stack message cannot separate it from the count boundary,
and its mutation test quotes the silent publishes the previous surface
allowed: an out-of-range block index, an empty batch and a duplicate or
descending identity pair. It also needs no corpus files, so it runs before
the controller integrates the exported candidates.

`tests/protocol_hostiles.rs` pins the three hostile-mob publication records
through that same surface: the reviewed 69-byte spawn and 85-byte state
literals round-trip byte for byte with the negative-zero pose bits preserved,
the 25-byte despawn literal round-trips through the checked identity, the
record strides 30/38/8 with the dimension-for-velocity exchange that
distinguishes them, the closed kind match and the inclusive 1..=20 health
span, the raw-`i32` dimension match that refuses 256 as an enum violation,
the 64-record ceiling on all three families with the count bound firing
before the record rule, the exact-remaining-length batch rule (every proper
truncation plus one padded byte reject at the same boundary), and the packet
IDs 22/23/24. Its mutation test quotes the silent publishes the previous
surface allowed: a spawn record mutated into kind 2, the depths dimension,
health 21, a zero identity, a descending pair or an empty batch published
those values byte for byte, while a non-finite pose reached the primitive's
own refusal and became a panic at the previous encoder's `expect`. It also
needs no corpus files, so it runs before the controller integrates the
exported candidates.

`tests/protocol_passives.rs` pins the three passive-mob publication records
through that same surface: the reviewed 67-byte spawn, 85-byte state and
27-byte despawn literals round-trip byte for byte with the negative-zero pose
bits preserved, the record strides 29/38/9 with the dimension-for-velocity
exchange and the grazing bit that distinguish the two full-body records and
no kind byte on either, the closed grazing and reason pairs (0/1 admitted, 2
refused at the enum boundary on every entry point), the inclusive 1..=20
health span, the raw-`i32` dimension match that refuses 256 as an enum
violation, the 64-record ceiling on all three families with the 32-actor
live cap named in the test as the authority-only concern that never enters
the packet layer, the exact-remaining-length batch rule (every proper
truncation plus one padded byte reject at the same boundary), and the packet
IDs 26/27/28. Its mutation test quotes the silent publishes the previous
surface allowed: a spawn record mutated into the depths dimension, health 21
or a zero identity, a state record mutated into grazing 2 or a non-finite
velocity, a descending pair and an empty batch all published those values
byte for byte, with the health-21 spawn publishing `..., 0x15]` and the
grazing-2 state record publishing `0a, 02` in its health and grazing bytes.
It also needs no corpus files, so it runs before the controller integrates
the exported candidates.

`tests/protocol_projectiles.rs` pins the three projectile publication records
through that same surface: the reviewed 83-byte spawn, 49-byte state and
25-byte despawn literals round-trip byte for byte with the negative-zero
position and velocity bits preserved, the record strides 37/20/8 with the
kind-before-dimension order the spawn record carries and the bare identity
the state record reduces to, the closed kind pair (0/1 admitted, 2 refused at
the enum boundary on every entry point), the kind-by-dimension independence
(all four combinations admit on this wire, because a shard in the depths and
an arrow in the overworld are both publishable records and the narrower
authority rule never enters the packet layer), the raw-`i32` dimension match
that refuses 256 as an enum violation, the 128-record ceiling on all three
families with the derived `PROJECTILE_*_MAX_WIRE_BYTES` payload bounds
(9 + 128×37 / 9 + 128×20 / 9 + 128×8), the exact-remaining-length batch rule
(every proper truncation plus one padded byte reject at the same boundary),
the pre-parse wire ceiling (one byte above each derived bound answers
`FrameTooLarge` before any read, including over a simultaneously broken
count), and the packet IDs 29/30/31. Its mutation test quotes the silent
publishes the previous surface allowed: a spawn record mutated into kind 2
published `0x02` in the kind byte at offset 17, a zero-identity spawn and a
zero-identity despawn published `00` where the identity belongs, a duplicate
state pair published the same identity twice, and a non-finite component
reached the primitive's refusal and became a panic at the previous encoder's
`expect`. It also needs no corpus files, so it runs before the controller
integrates the exported candidates.

`tests/protocol_chat_event.rs` pins the chat event publication record through
that same surface: all sixteen legal branch shapes round-trip the reviewed wire
literal through `validate`/`encoded_len`/`encode_into`/`encode`, the record
stride is decomposed into its fixed fields and three length-prefixed slots,
the padded-prefix and sentinel checks hold on every branch, every proper
truncation of the accepted and speech payloads rejects at the truncation
boundary, one trailing byte rejects, and the wire ceiling refuses one byte
above `CHAT_EVENT_MAX_WIRE_BYTES` while admitting a maximum record at exactly
the bound. Its mutation test quotes the silent publishes the previous surface
allowed: a record mutated into kind 200 published `0xc8` in the kind byte at
offset 49, and a task branch's command mutated into 1025 bytes reached the
primitive's refusal and became a panic at the previous encoder's `expect`. The
suite also pins the bidirectional domain conversion: every branch maps to its
`ChatBody` member and back without rewriting the wire record, the absent zero
companion identity is admitted in the two rejection branches alone and refused
everywhere else, and the two text slots stay mutually exclusive on mutated
records. It also needs no corpus files, so it runs before the controller
integrates the exported candidates.

`tests/protocol_corpus.rs` executes the corpus cases this crate owns through
the real codec paths — `read_frame`/`write_frame` for framing,
`decode_inbound` plus the strict outbound encoders for the
`protocol.client.ClientHello` and `protocol.client.LoginStart` packet families,
and each later group's real packet surface — and compares the complete result
against the outcome the independent Go producer recorded. It loads the
`corpus_frame` selection and the `mornlea_protocol` selection the packet groups
register into; a packet family whose cases the controller has not integrated
yet fails as a missing corpus case rather than as an empty selection. The
shared loader retains each packet case's frozen direction/state/ID key. Valid
decode cases pass through public typed dispatch and establish all 59 family
keys; every concrete corpus comparison checks its case key against that family
before comparing bytes or fields. The frame family has no packet key, and
direction, state, or ID mutation must fail with unchanged payload bytes. The
preexisting `runtime_contract` framing test stays a separate regression suite.
The rejection categories the consumer derives from this crate's error variants
include the `integrity` boundary for a compressed stream the envelope checks
could not catch; for the one compressed family, the corpus digest is the
SHA-256 of the canonical logical payload rather than of the compressed bytes,
because the two implementations' encoders legitimately publish different
compressed blocks.

The suite also carries the protocol-only closure evidence for the complete
corpus. `protocol_corpus_protocol_family_set_is_closed` pins the executed
family set against the closed 60-family identity list — 59 packet families
beside the framing family — and requires every family's minimum of one valid
decode, one valid encode and one invalid or boundary case at packet version
45, with the packet and framing selections counted separately beside the
reviewed 177-packet and 3-frame minimums.
`protocol_corpus_every_group_mutation_fails_the_comparison` runs one reviewed
case per producer group through the real dispatch, asserts the unmutated
comparison passes first outside `catch_unwind`, and then fails the same
comparison with exactly one drifted expectation value (a bumped number or an
extended text), so every group's frozen expectation is proven load-bearing on
the Rust side as well. Both tests need no corpus files beyond the merged
manifest, so they run before the controller integrates any exported candidate.
