# Runtime oracle

`packages/tools/cmd/runtime-oracle` is the offline contract inventory and replay
oracle for OpenSpec change `rust-runtime-foundation`. It observes repository
sources and testdata only. Production code must not import live authority,
native ABI, network transports, or storage codecs; production files have an
empty allowed internal import set. Only `_test.go` files are permitted to import
designated offline codec, world, companion, and storage packages
(`shared/network/codec`, `shared/network/protocol`, `shared/core`,
`shared/world`, `shared/companion`, `shared/pathfind`, `shared/nativeabi`,
`server/storage/storagedef`, and
`server/storage/{chunk,player,companion,hostile,passive,region}`). Neither
production nor test files may import `server/server`, file stores, client/render,
or Agent process packages. These boundaries are enforced by `packages/audit`
`TestRuntimeOracleInternalDependencies` and `TestInternalDependenciesAreOneWay`.

## Inventory freeze (`inventory.go`, `discover.go`, `root.go`)

- `Discover` reads current protocol, save, kernel, and agent registries from
  files. It does not load production packages.
- `LoadInventory` reads the frozen corpus at `InventoryRelPath`
  (`testdata/runtime-migration/contracts.json`).
- `ReconcileWorking` permits in-progress manifests to contain zero-case families
  and returns sorted `Covered` and `Uncovered` coverage points without claiming
  complete acceptance; `ReconcileComplete` fails closed with an `*InventoryError`
  if any uncovered family/version point remains.
- Both reconciliation paths share a private validator that enforces identity,
  source provenance, case structural and cryptographic validity, case version
  membership in its family's `supported_versions`, expected outcome `kind: "ok"`
  plus `kind: "error"` (unless covered by a reviewed `NegativeCoverageExceptions`
  rationale), and exact family/version/operation membership in the closed
  `BaselineConsumerRegistry()`. A consumer registration binds its implementation
  kind to executable routes; empty names, invalid kinds, empty route sets, and
  known consumers used on unsupported routes fail closed.
- The baseline routes are `corpus_frame` → `protocol.frame/45/decode` and
  `protocol.frame/45/encode`; `mornlea_protocol` → the protocol packet routes
  (established with the framing routes it executes today and extended one
  producer group at a time; the inbound negotiation group adds
  `protocol.client.ClientHello/45/{decode,encode}` and
  `protocol.client.LoginStart/45/{decode,encode}`); `mornlea_domain` →
  `domain.identity_values/current/admit`,
  `domain.values/current/admit`, `domain.command_control/current/admit`,
  `domain.command_inventory/current/admit`, and `domain.event/1/admit`;
  `external:agent-contract` → `agent.http/v1/agent-contract` and
  `agent.mcp/v1/agent-contract`; `external:runtime-authority` →
  `domain.input/45/order`; and `mornlea_storage` → `save.player/1/decode`,
  `save.player/2/decode`, `save.player/3/decode`, `save.player/4/decode`,
  `save.player/5/decode`, `save.player/6/decode`, `save.player/7/decode`,
  `save.player/8/decode`, `save.player/9/decode`, `save.player/9/encode`,
  `save.region/1/decode`,
  `save.region/1/encode`, and `save.region/1/order`; `save.world-metadata/1/decode`
  through `save.world-metadata/6/decode` plus `save.world-metadata/6/encode`; and
  `save.hostile/1/decode`, `save.hostile/2/decode`, and `save.hostile/2/encode`; and
  `save.passive/1/decode` and `save.passive/1/encode`; and `save.chunk/1/decode`
  through `save.chunk/9/decode` plus `save.chunk/9/encode`; and
  `save.companion/1/decode` through `save.companion/4/decode`.
- Enforcement: `TestContractInventoryReconcilesFrozenCorpus`,
  `TestContractInventoryWorkingReportsZeroCaseFamilies`,
  `TestContractInventoryCompleteRejectsZeroCaseFamilies`,
  `TestContractInventoryRejectsUnknownConsumer`,
  `TestContractInventoryRejectsKnownConsumerOnUnsupportedRoute`,
  `TestContractInventoryRejectsInvalidConsumerRegistry`,
  `TestContractInventoryInputAssetBudgets`,
  `TestContractInventoryRejectsNonRegularAssets`,
  `TestLoadInventoryRejectsNonRegularFile`,
  `TestContractInventoryRejectsUnsupportedCaseVersion`,
  `TestContractInventoryWorkingAndCompleteCoverage`,
  `TestContractInventoryRejectsMissingFamily`,
  `TestContractInventoryRejectsVersionMismatch`,
  `TestContractInventoryRejectsMissingProvenanceSource`,
  `TestContractInventoryRejectsIncompleteIdentity`.
- When a new producer appends cases to a shared family, rerun the complete
  `runtime-oracle` package in that node and update every affected exact-total
  assertion together. A producer-filtered pass cannot detect stale totals in
  earlier producers.
- Producer changes that add Go comments also run audit
  `TestCommentBacktickIdentifiersExist` before node closure. A narrow package
  test does not check whether backticked names in comments are Go identifiers.

## Isolated replay (`trace.go`)

- `BuildTrace` assembles executed observations into a versioned `Trace` identity
  (source revision, contract versions, corpus digest, seed, ordered checkpoint
  inputs, tick schedule, and normalized observations). Trace assembly is pure
  and performs only structural validation without filesystem access.
- `TraceRequest` carries no work directory, repository root, or source revision.
  The harness owns the workspace.
- `NewTraceWorkspace` creates an exclusive temporary directory with
  `os.MkdirTemp("", "mornlea-runtime-oracle-")` and returns the cleanup that
  removes exactly that path. A workspace that would resolve inside the
  repository is rejected as a live-path write.
- `ValidateTraceAtRoot(root, trace, manifest)` validates a trace report against
  expected assets at an explicit corpus root. It loads every selected expected
  asset through bounded path, symlink, duplicate-key, and sha256 checks, and
  compares normalized outcomes. Every load error is recorded; none is skipped.
- `LoadTraceAtRoot(root, path, manifest)` loads a report from path, enforces
  duplicate-key and size budgets, and delegates to `ValidateTraceAtRoot`.
- `ExportTrace(root, target, trace, manifest)` publishes one report and stays
  separate from execution. It validates the trace against root first, resolves
  the target through its existing ancestor, rejects repository containment and
  symlink components, refuses a preexisting target, creates missing directories
  one component at a time, and stages the report in the target parent before
  publishing it with `os.Link`. The atomic publication success boundary is the
  successful link; staged-file cleanup is best-effort.
- Containment is judged on resolved paths, and only a `..` path element counts
  as leaving the repository: a component whose name merely starts with `..`
  (a sibling such as `..cache`) is a child of the repository, not an escape
  from it.
- Corpus assets are validated by the canonical inventory validator during
  reconciliation: every case's input, expected, and encoded asset must be
  reachable without a symlink component, stay inside its byte budget, and
  match its recorded digest. Manifests, provenance sources, inputs,
  expectations, and encoded assets must be regular files before reads or
  hashes. JSON inputs and expectations use the 256 KiB JSON budget; binary
  inputs and encoded assets use the 4 MiB binary budget.
- Incomplete source revision, missing contract identity, empty corpus digest,
  empty tick schedule, or missing observations fail closed. `LoadTraceAtRoot`
  rejects truncated bytes, non-object JSON, duplicate keys, and unsupported
  `schema_version`.
- Enforcement: `TestExecutedObservationMismatchFailsAgainstUnchangedExpected`,
  `TestTraceValidationRejectsMissingExpected`,
  `TestTraceValidationRejectsMalformedExpected`,
  `TestTraceValidationRejectsExpectedDigestMismatch`,
  `TestTraceValidationRejectsDifferentCorpusRoot`,
  `TestBuildTraceRejects*`, `TestTraceRejectsIncompleteIdentity`,
  `TestTraceRejectsLivePathWrites`, `TestTraceRejectsMalformedInput`,
  plus the isolation, path, output, and I/O regressions in
  `trace_isolation_test.go` (`TestTraceIsolation*`, `TestTracePath*`,
  `TestTraceOutput*`, `TestTraceIO*`).

## Independent operation runners (`runner_helpers_test.go`, `protocol_frame_test.go`)

`runner_helpers_test.go` declares `GoOperation`, the registries, `RunCases`,
and the isolated export helpers (`exportGeneratedAssets`,
`exportGeneratedAssetsFromEnvironment`).

- `GoOperation` and the `map[string]GoOperation` registry live in test code
  only. A producer receives the case specification and the case input bytes and
  returns the normalized outcome, its own encoded bytes, and an error. The
  recorded expected outcome is never handed to a producer, so an independent
  execution cannot be shaped by the evidence it is supposed to reproduce.
- `RunCases` keeps the historical single-operation-per-family contract: it
  binds each family to one operation name and delegates to the private
  `runCasesByRoute` loop. A family that publishes more than one operation, as
  the framing family does with `decode` and `encode`, is executed by
  `RunProtocolCases` instead.
- `RunProtocolCases(root, selection, routes)` in
  `protocol_corpus_helpers_test.go` is the protocol-only runner: it takes a
  closed `map[ConsumerRoute]GoOperation`, rejects an empty selection, a missing
  route map, an unregistered `{family, version, operation}` route before the
  case input is read, and a duplicate checkpoint. Every producer group that
  registers a packet family executes its cases through it, so a case can never
  claim coverage from its name alone.
- `runCasesByRoute` is the shared loop both runners use: it resolves the input
  under the input-format-specific corpus byte budget shared with
  reconciliation, proves the input digest matches the manifest, invokes the
  registered producer once per declared checkpoint, and derives every
  observation from the returned values. An unknown family, an unregistered
  operation, an operation that disagrees with its family's binding, a missing
  or duplicated checkpoint, or a tampered input digest is a hard error.
- Protocol producers use the real production codec. The framing producer calls
  `codec.ReadFrame` for the decode route and `codec.WriteFrame` for the encode
  route, reads the encoded frame back through `codec.ReadFrame` before
  publishing it, and classifies a rejection into one of the frozen execution
  contract categories (`invalid-varint` for a non-canonical length prefix,
  `capacity` for the writer's own size refusal); an unclassified failure is an
  error rather than an unlabelled rejection.
- The first packet producer group is `protocol_negotiation_test.go`
  (`protocol.client.ClientHello` and `protocol.client.LoginStart`, decode and
  encode). It executes `codec.DecodeClient`/`EncodeClient` with the case's own
  `PacketKey`, normalizes the semantic fields of the DTO the decoder returned
  (`display_name`, `player_id` as lowercase hexadecimal, `view_distance`,
  `protocol_version`), and reads every encoded payload back through the
  decoder before publishing it. Its cases are the ordinary structural ones
  only: an old-version hello and an over-long raw name are admitted by the Go
  inbound decoder and refused by the Go outbound encoder, so no case can carry
  them both ways. Those values are pinned instead by the package-local
  `packages/shared/network/protocol_admission_oracle_test.go` driver table and
  by the Rust `tests/protocol_admission.rs` suite, whose case identities match.
- The second packet producer group is `protocol_control_test.go`
  (`protocol.server.ServerHello`, `protocol.server.HandshakeReject`,
  `protocol.server.LoginSuccess`, `protocol.server.LoginReject`,
  `protocol.server.KeepAlive`, `protocol.client.KeepAliveReply` and
  `protocol.server.Disconnect`, decode and encode). It executes
  `codec.DecodeServer`/`EncodeServer` (and `DecodeClient`/`EncodeClient` for
  the client-to-server keep alive reply) with the case's own `PacketKey`,
  normalizes the DTO fields (`u64` tokens and seeds as decimal strings, small
  enums as declared integers, identities as lowercase hexadecimal), and reads
  every encoded payload back before publishing it. Its category table owns one
  boundary the Go sentinels cannot express: a declared message length the
  payload cannot complete is answered with the same `invalid string` error as a
  malformed UTF-8 message, so the boundary resolver classifies a rejected
  proper prefix of the case's reviewed payload as `truncated` and every other
  invalid-string rejection as `invalid-value`. The Rust control message reader
  reports the same boundary as its `Truncated` error, so both implementations
  publish one category.
- The third packet producer group is `protocol_client_control_test.go`
  (`protocol.client.PlayerInput`, `protocol.client.PlaceBlock`,
  `protocol.client.RequestChunkResync` and `protocol.client.SelectHotbar`,
  decode and encode). It executes `codec.DecodeClient`/`EncodeClient` in the
  play state with the case's own `PacketKey`, normalizes the DTO fields
  (`sequence` and `have_revision` as decimal strings, the `i8` axes, chunk
  coordinates and slot as declared integers, the four action flags as
  booleans, and the two look angles as eight-digit lowercase-hexadecimal bit
  strings), and reads every encoded payload back through the decoder before
  publishing it. The bit-string encoding is what carries a `-0.0` angle
  through the JSON encode requests unchanged. Its category table resolves the
  primitive and validator failures the Go codec names for these families:
  `invalid boolean` is the one invalid-enum boundary, while a non-finite
  angle, an out-of-range hotbar slot and an unknown resync dimension classify
  at their own boundaries, so the Rust consumer publishes the same category
  from its `ProtocolError` variants.
- The fourth packet producer group is `protocol_client_rays_test.go`
  (`protocol.client.OpenContainer`, `protocol.client.TillSoil`,
  `protocol.client.BoneMeal`, `protocol.client.CollectWater` and
  `protocol.client.PlaceWater`, decode and encode). It executes
  `codec.DecodeClient`/`EncodeClient` in the play state with the case's own
  `PacketKey`, normalizes the DTO fields (`sequence` as a decimal string and
  the two look angles as eight-digit lowercase-hexadecimal bit strings), and
  reads every encoded payload back through the decoder before publishing it.
  Every family's canonical vector is the same 16-byte payload — sequence 0,
  a `-0.0` yaw and a 1.5 pitch — so the packet key is the only thing that
  tells the five families apart. Its category table stays minimal: the f32
  primitive's `invalid float32` and each validator's `non-finite rotation`
  resolve to `invalid-value` on both directions, while `short input` and
  `trailing bytes` keep their own categories, so the Rust consumer publishes
  the same category from its `ProtocolError` variants. Provenance is
  `codec_client.go` plus the family's own message file, because
  `OpenContainer` lives in `message_container.go` and the other four in
  `message_command.go`.
- The fifth packet producer group is `protocol_client_inventory_test.go`
  (`protocol.client.MoveInventoryStack`, `protocol.client.MoveCraftingStack`,
  `protocol.client.CloseContainer`, `protocol.client.DropSelectedItem`,
  `protocol.client.EquipArmor` and `protocol.client.TakeCraftingOutput`,
  decode and encode). It executes `codec.DecodeClient`/`EncodeClient` in the
  play state with the case's own `PacketKey`, normalizes the DTO fields
  (`sequence` as a decimal string and the two move slots as JSON numbers),
  and reads every encoded payload back through the decoder before publishing
  it. Two payload shapes are in scope: the two move families carry a 10-byte
  sequence-plus-two-slots vector and the four sequence-only families the
  8-byte sequence alone, so no case's input carries an item count, a drop
  position or an equipped armor slot. The canonical `TakeCraftingOutput`
  vector carries sequence 1, because that family alone refuses a zero
  sequence. Its category table resolves the Go validators' slot-range,
  same-slot, both-in-inventory-region and zero-sequence rejections to
  `invalid-value` (all wrapped by `codecError`, so the shared substrings
  `slot is outside`, `source equals target`, `both ends in the inventory
  region` and `sequence is zero` match), while `short input` and `trailing
  bytes` keep their own categories, so the Rust consumer publishes the same
  category from its `ProtocolError` variants. Provenance is
  `codec_client.go` plus the family's own message file: `message_inventory.go`
  for the two move families and `TakeCraftingOutput`, `message_container.go`
  for `CloseContainer`, and `message_command.go` for `DropSelectedItem` and
  `EquipArmor`.
- The sixth packet producer group is `protocol_client_stack_views_test.go`
  (`protocol.client.MoveContainerStack`, `protocol.client.MoveStackPartial`,
  `protocol.client.QuickMoveStack` and `protocol.client.DropStack`, decode and
  encode). It executes `codec.DecodeClient`/`EncodeClient` in the play state
  with the case's own `PacketKey`, normalizes the DTO fields (the `sequence` as
  a decimal string, the 18-byte container reference as a nested JSON object of
  plain integers, and `view`/`from`/`to`/`slot` as JSON numbers with `single`
  as a JSON boolean), and reads every encoded payload back through the decoder
  before publishing it. Every negative carries exactly one violation, because
  Go's `validAnyContainerRef` checks the kind first (`invalid-enum`) while the
  Rust neutral conversion checks the dimension first (`invalid-value`), so a
  doubly invalid reference would publish different categories on the two
  sides. Its category table resolves the reference gates (`dimension is not
  overworld`, `generation is zero`, `slot is outside`, `carries a container
  ref`, `source equals target`, `cannot be a move target`) to `invalid-value`,
  the two enum boundaries (`unknown container kind`, `unknown stack split
  view`, the decoder's `invalid boolean`) to `invalid-enum`, and keeps `short
  input` and `trailing bytes` at their own categories, so the Rust consumer
  publishes the same category from its `ProtocolError` variants. Provenance is
  `codec_client.go` plus `message_container.go` for `MoveContainerStack`,
  `message_stack_splitting.go` added for `MoveStackPartial` and
  `QuickMoveStack`, and `message_drop_stack.go` added for `DropStack`, because
  the shared reference validators live in `message_container.go`.
- The seventh packet producer group is `protocol_client_chat_test.go`
  (`protocol.client.ChatCommand`, decode and encode). It executes
  `codec.DecodeClient`/`EncodeClient` in the play state with the case's own
  `PacketKey` and normalizes the single `text` field verbatim, including a
  leading mention prefix, because the codec performs no addressing. The
  family is the one variable-length client payload, so the category table
  resolves the pre-parse payload ceiling (`chat command payload exceeds`, the
  Go `ChatCommandMaxWireBytes`) to `capacity`, the noncanonical uvarint and
  the trailing-byte boundaries at their own categories, and the two text
  validator messages (`invalid chat command text`, `chat command contains
  control character`) to `invalid-value`. Its string boundary reuses the
  node 1.5 ruling: a declared length the payload cannot complete is answered
  by the Go string primitive with the same sentinel as a malformed UTF-8
  text, so the boundary resolver classifies a rejected proper prefix of the
  reviewed payload as `truncated` and every other invalid-string rejection as
  `invalid-value`, and the Rust reader reports `Truncated` for the same bytes.
  The text-above-bound case is encode-only, because the payload ceiling
  answers a length the decoder cannot reach first. Provenance is
  `codec_client.go` plus `message_companion.go`, which owns
  `validateCommandText` and the `companion.MaxPlanCommandBytes`-derived wire
  bound.
- The eighth packet producer group is `protocol_world_delta_test.go`
  (`protocol.server.BlockChanges` and `protocol.server.ForgetChunks`, decode
  and encode), the first server-to-client group of the plan. It executes
  `codec.DecodeServer`/`EncodeServer` in the play state with the case's own
  `PacketKey`, whose direction is `server-to-client` for both families, and
  normalizes `base_revision`/`new_revision` as decimal strings with
  `dimension`, the chunk coordinates, the change coordinates and the block
  numbers as plain JSON integers; the `changes` and `chunks` arrays publish
  in wire order, so the forget batch's submitted order survives the
  comparison rather than being sorted. Both families are the first
  variable-count batch payloads, so the category table resolves the decode
  arm's count bound (`packet count is outside 1..4096`, which fires before
  the arm's own record-length rule) and every validator range rejection to
  `invalid-value`, the two dimension messages (`block changes dimension is
  not overworld or depths`, `forget chunks dimension is not overworld or
  depths`) and an unregistered block to `invalid-enum`, and the arm's length
  rule (`packet count exceeds remaining payload`) plus `short input` to
  `truncated` while `trailing bytes` keeps its own category. The zero-change
  block batch stays legal as the revision barrier while a zero-count forget
  batch is refused, and the 4096/4097 boundaries are group-test pins in
  `tests/protocol_world_delta.rs` rather than corpus assets. Provenance is
  `codec_server.go` plus the family's own message file: `snapshot.go` for
  `BlockChanges` and `message_chunk.go` for `ForgetChunks`.
- The ninth packet producer group is `protocol_snapshot_test.go`
  (`protocol.server.ChunkSnapshot`, decode and encode), the one family whose
  wire payload is compressed. It executes `codec.DecodeServer`/`EncodeServer`
  in the play state with the case's own `PacketKey`, and normalizes
  `dimension`, the chunk coordinates, the section indices and the palette as
  plain JSON integers with `revision` as a decimal string and the packed
  words as fixed-width lowercase-hexadecimal strings; the section kinds
  (`single`, `indexed4`, `indexed8`, `direct`) imply the bits-per-slot, so the
  wire field is not restated. **Corpus digests are logical, never
  compressed**: the Rust and Go zstd encoders legitimately publish different
  compressed blocks, so for every encode case the producer encodes through the
  Go codec, decompresses its own output back to the canonical logical bytes
  with the zstd package this module already builds through `packages/shared`,
  and digests those; it returns the logical payload as its encoded output
  because the shared runner derives a case's recorded digest from the bytes
  its producer returns, and no corpus case ever records a digest of
  compressed bytes. The category table resolves the envelope's declared-length
  ceilings (`exceeds limit`) to `capacity`, the envelope's remaining-length
  check (`compressed length does not match snapshot envelope`) to `truncated`,
  and a length-complete frame the zstd layer rejects (`decompress snapshot`)
  to `integrity` — the first family to publish that category, which the
  envelope checks could not catch. The five logical-layer validator
  rejections (zero revision, section count, section Y, palette slot beyond
  the palette, direct high bits) resolve to `invalid-value`. The committed Go
  fixture is read verbatim as one decode case and is never rewritten, and this
  group's assets render compactly rather than with the committed two-space
  layout because the mixed vector's packed words would otherwise exceed the
  256 KiB JSON case budget. Provenance is `codec_server.go` plus
  `chunk_codec.go`, which owns the envelope and the logical layers, and
  `snapshot.go`, which owns the validators.
- `TestRustSnapshotDecodesInGo` reads the Rust-owned fixture from the protocol
  crate and the Go fixture, decodes both with the production `codec.DecodeServer`
  path, and compares complete `ChunkSnapshot` values. It also rejects a
  flipped zstd checksum and mutates the Go fixture's window descriptor to pin
  acceptance at 2 MiB and rejection just above it and at 128 MiB. The Rust
  snapshot suite pins that fixture to its current owned encoder and checks the
  same window boundary; compressed bytes are intentionally different.
- The tenth packet producer group is `protocol_player_outcomes_test.go`
  (`protocol.server.PlayerState`, `protocol.server.CommandRejected`,
  `protocol.server.PlaceBlockSucceeded` and `protocol.server.CombatHit`,
  decode and encode), the four owner-private records an authoritative tick
  addresses to the session that caused them. It executes
  `codec.DecodeServer`/`EncodeServer` in the play state with the case's own
  `PacketKey`, normalizes the ticks, sequences and revisions as decimal
  strings, the position, velocity and look angles as eight-digit
  lowercase-hexadecimal bit strings, the mining target as its ordered integer
  triple, and every closed enum as the declared integer it carries; the
  temperature renders as the signed integer the wire carries, because the
  full `i8` range is legal and never clipped. **The reject reason is carried
  as its frozen wire number, never as an internal enum cast**: the Go internal
  enum runs `0..14` while the wire enum runs `1..15`, so the producer requests
  and publishes the wire value and the Rust consumer owns the explicit
  bidirectional translation its group test pins row by row. The category table
  resolves the dimension, weather, season, reject-reason and combat-kind
  boundaries (`unknown command rejection reason`, `combat hit target kind`)
  to `invalid-enum`; the survival ranges, the day phase offset, the armor
  points, the mining-union messages, the combat tick and damage ranges and
  the float primitive's own non-finite rejection to `invalid-value`; the
  exact-length combat check plus `short input` to `truncated`; and `trailing
  bytes` to `trailing`. An unregistered wire reason never becomes a silent
  zero publication on the encode path, because the outbound validator runs
  before the reason byte is written. Provenance is `codec_server.go` plus the
  family's own message file: `message_player.go` for `PlayerState`,
  `CommandRejected` and `PlaceBlockSucceeded`, and `message_combat.go` for
  `CombatHit`.
- The eleventh packet producer group is `protocol_inventory_publication_test.go`
  (`protocol.server.InventoryState`, `protocol.server.CraftingState`,
  `protocol.server.FurnaceState`, `protocol.server.ChestState` and
  `protocol.server.ContainerClosed`, decode and encode), the item and
  container publications one player owns. It executes
  `codec.DecodeServer`/`EncodeServer` in the play state with the case's own
  `PacketKey`, normalizes the slot values as the `{item, count, durability}`
  object both directions share, the 18-byte container reference as the nested
  raw-integer object, and the selected index, size, timers and slot counts as
  the plain JSON integers the wire carries; the hotbar, backpack, grid and
  chest arrays publish in wire order. Its category table resolves the
  reference gates the way their Go order answers: the kind messages
  (`unknown container kind`, `furnace ref kind is not furnace`, `chest ref
  kind is not chest`) to `invalid-enum` and the dimension, slot and
  generation messages to `invalid-value`. The family validators answer every
  slot-rule violation with one shared message per family, so the table
  resolves those messages at the item-stack rule's own split, which is what
  the Rust consumer publishes for the same bytes: the chest slot message to
  `invalid-enum`, because the one stack-rule case that family registers
  carries an unregistered item number, and the inventory message to
  `invalid-value`, because that family's two stack-rule cases are the
  selected index and a durability violation. No crafting stack case is
  registered, so that family's slot messages stay outside the reviewed
  table. The container closure's exact all-zero record is refused through
  its zero generation (`validFurnaceRef` answers the kind first, and kind 0
  is a furnace, so the generation is the boundary that fires), which is the
  `invalid-value` category the Rust neutral conversion publishes for the
  same bytes. Provenance is `codec_server.go` plus the family's own message
  file: `message_inventory.go` for `InventoryState` and `CraftingState`,
  `message_container.go` for `FurnaceState`, `ChestState` and
  `ContainerClosed`.
- The twelfth packet producer group is `protocol_remote_players_test.go`
  (`protocol.server.RemotePlayerSpawn`, `protocol.server.RemotePlayerDespawn`
  and `protocol.server.RemotePlayerStates`, decode and encode), the peer
  session publications. It executes `codec.DecodeServer`/`EncodeServer` in the
  play state with the case's own `PacketKey`, normalizes the identity as the
  32-lowercase-hexadecimal text both directions share, the `server_tick` as a
  decimal string, the dimension as the plain wire integer, the ordered
  `position`/`yaw`/`pitch` as eight-digit lowercase-hexadecimal bit strings,
  the name verbatim and `reset` as a JSON boolean; the batch's records publish
  in wire order. Its category table resolves the boundaries the Go decoder
  and validators actually name: the float primitive's `invalid float32` and
  the validator messages to `invalid-value`, the batch's per-record combined
  predicate to `invalid-enum` (only the dimension mutation reaches it, because
  an infinite coordinate is answered by the float primitive first and a
  duplicate or descending identity by the order rule), the despawn's
  family-specific identity message to `invalid-identity`, the count bound and
  the remaining-length check to `invalid-value` and `truncated`, the 296-byte
  ceiling to `capacity`, and the structural pair to `truncated`/`trailing`.
  The spawn's identity and dimension boundaries are registered nowhere: the
  Go `RemotePlayerSpawn.Validate` folds identity, name, dimension and
  finiteness into one message, so a corpus case could not distinguish them,
  and they stay Rust group-test pins
  (`protocol.server.RemotePlayerSpawn` carries only the name and finiteness
  boundaries, which both sides publish as value violations). Provenance is
  `codec_server.go` plus `message_player.go` for all three families.
- The thirteenth packet producer group is `protocol_companions_test.go`
  (`protocol.server.CompanionSpawn`, `protocol.server.CompanionStates` and
  `protocol.server.CompanionDespawn`, decode and encode), the party
  publications. It executes `codec.DecodeServer`/`EncodeServer` in the play
  state with the case's own `PacketKey`, normalizes the identity as the
  32-lowercase-hexadecimal text, the `tick` as a decimal string, the dimension
  as the plain wire integer, the ordered `position`/`yaw`/`pitch` as
  eight-digit lowercase-hexadecimal bit strings, the name verbatim and `reset`
  as a JSON boolean; the batch's records publish in wire order. Its category
  table resolves the boundaries the Go decoder and validators actually name:
  the folded spawn and per-record messages (`invalid companion spawn`,
  `invalid companion state`) and the float primitive's `invalid float32` to
  `invalid-value`, the batch's count and order messages to `invalid-value`,
  the remaining-length message to `truncated` (this batch is the one whose Go
  decoder compares the remaining bytes against the declared count, so a
  trailing byte answers at that message rather than the trailing-byte
  boundary), the fixed payload ceilings to `capacity`, and the despawn's
  family-specific identity message to `invalid-identity`.
  The spawn's identity and dimension boundaries are registered nowhere: the Go
  folds identity, name, dimension and pose into one message, so a corpus case
  could not distinguish them, and they stay Rust group-test pins
  (`protocol.server.CompanionSpawn` carries the name, pitch-range and
  nonfinite-pose boundaries, which both sides publish as value violations).
  Provenance is `codec_server.go` plus `message_companion.go` and
  `companion_wire.go` for all three families.
- The fourteenth packet producer group is `protocol_drops_test.go`
  (`protocol.server.ItemDropUpserts` and `protocol.server.ItemDropRemoves`,
  decode and encode), the item-drop publications. It executes
  `codec.DecodeServer`/`EncodeServer` in the play state with the case's own
  `PacketKey`, normalizes the `server_tick` as a decimal string, each drop
  identity as the ordered five-field object
  `{dimension, chunk_x, chunk_z, slot, generation}` of plain integers where
  the dimension is the raw wire value kept verbatim, and each record's stack
  as the `{item, count, durability}` object beside its `block_index`; the
  records publish in wire order. Its category table resolves the boundaries
  the Go decoder and validators actually name: the per-record identity
  messages (`invalid item drop ID`, `item drop remove %d: invalid ID`) to
  `invalid-identity`, the block-index message, the batch count message, the
  two order messages and the folded stack message (`invalid item drop stack`)
  to `invalid-value`, `short input` to `truncated`, and the end-of-payload
  check to `trailing` because both drop decoders apply the minimum-records
  rule and leave the remainder to it (the companion batch, with its exact
  remaining-length rule, answers the same extra byte at the truncation
  boundary instead).
  The unregistered item number is registered nowhere: the Go folds it into
  the same stack message as the count boundary, so a corpus case could not
  distinguish them, and it stays a Rust group-test pin
  (`protocol.server.ItemDropUpserts` carries the count-above-stack-limit
  boundary, which both sides publish as a value violation). Provenance is
  `codec_server.go` plus `message_drop.go` and `codec_values.go` for both
  families.
- The fifteenth packet producer group is `protocol_hostiles_test.go`
  (`protocol.server.HostileSpawn`, `protocol.server.HostileState` and
  `protocol.server.HostileDespawn`, decode and encode), the hostile-mob
  publications. It executes `codec.DecodeServer`/`EncodeServer` in the play
  state with the case's own `PacketKey`, normalizes the `server_tick` and
  each record identity as decimal strings, the dimension as a plain integer,
  each vector as its ordered bit-string array, and the health and kind as
  JSON numbers; the records publish in wire order. Its category table
  resolves the boundaries the Go decoder and validators actually name, and
  every hostile boundary is distinct, so no boundary is latent: the batch
  count message and the strict-order message to `invalid-value`, the
  remaining-length check to `truncated` because all three decoders apply the
  exact-remaining-length rule before they read a record (the item drop batch,
  with its minimum-record budget, answers the same extra byte as `trailing`
  instead), the per-record identity message to `invalid-identity`, the
  dimension and kind messages to `invalid-enum`, and the pose and health
  messages to `invalid-value`. Provenance is `codec_server.go` plus
  `message_hostile.go` and `hostile_wire.go` for all three families.
- The sixteenth packet producer group is `protocol_passives_test.go`
  (`protocol.server.PassiveSpawn`, `protocol.server.PassiveState` and
  `protocol.server.PassiveDespawn`, decode and encode), the passive-mob
  publications. It executes `codec.DecodeServer`/`EncodeServer` in the play
  state with the case's own `PacketKey`, normalizes the `server_tick` and
  each record identity as decimal strings, the dimension as a plain integer,
  each vector as its ordered bit-string array, and the health, grazing and
  reason as JSON numbers; the records publish in wire order. Its category
  table resolves the boundaries the Go decoder and validators actually name:
  the batch count message and the strict-order message to `invalid-value`,
  the remaining-length check to `truncated` because all three decoders apply
  the exact-remaining-length rule before they read a record, the per-record
  identity message to `invalid-identity`, the dimension, grazing and reason
  messages to `invalid-enum` because each names a closed wire set, and the
  pose and health messages to `invalid-value`. The 64-record wire bound is
  the protocol budget every family admits, and the smaller live capacity the
  authority converges on is deliberately absent from this layer. Provenance
  is `codec_server.go` plus `message_passive.go` and `passive_wire.go` for
  all three families.
- The seventeenth packet producer group is `protocol_projectiles_test.go`
  (`protocol.server.ProjectileSpawn`, `protocol.server.ProjectileState` and
  `protocol.server.ProjectileDespawn`, decode and encode), the projectile
  publications. It executes `codec.DecodeServer`/`EncodeServer` in the play
  state with the case's own `PacketKey`, normalizes the `server_tick` and
  each record identity as decimal strings, the kind and the dimension as
  plain integers, and each vector as its ordered bit-string array; the
  records publish in wire order. Its category table resolves the boundaries
  the Go decoder and validators actually name: the pre-parse fixed payload
  maximum to `capacity`, because this group is the one that resolved the
  over-ceiling latent class the Rust decoders mirrored by adding the same
  pre-parse size check, the batch count message and the strict-order message
  to `invalid-value`, the remaining-length check to `truncated` because all
  three decoders apply the exact-remaining-length rule before they read a
  record, the per-record identity message to `invalid-identity`, and the
  kind, dimension and pose messages to `invalid-enum` and `invalid-value`
  respectively. The 128-record wire bound is the protocol budget every
  family admits, the kind and the dimension are independent on this wire, and
  the state record carries the identity and the position alone. Provenance is
  `codec_server.go` plus `message_projectile.go` and `codec_projectile.go`
  for all three families.
- The eighteenth packet producer group is `protocol_chat_event_test.go`
  (`protocol.server.ChatEvent`, decode and encode), the chat event union. It
  executes `codec.DecodeServer`/`EncodeServer` in the play state with the
  case's own `PacketKey` and normalizes the event identity as a decimal
  string, both identities as 32-lowercase-hexadecimal text, the names, command
  and speech verbatim, and the kind and reason as plain integers; the
  companion identity stays the raw wire form, so the two branches that never
  addressed a companion publish the exact zero bytes instead of a
  pre-validated identity. This is the one family whose text slot is reused by
  kind, so the decoder reads the kind before it decides which text to read and
  the payload carries exactly one slot. Its category table is a class table
  rather than a single category per message, because `ChatEvent.Validate`
  folds several relations into one message: the folded player-identity message
  publishes either `invalid-identity` (zero event identity, invalid player
  identity) or `invalid-value` (non-canonical player name), the failed-task
  message publishes either `invalid-enum` (the failure-reason domain) or
  `invalid-value` (the payload relations), the reserved reject reason and the
  unknown kind resolve to `invalid-enum`, every remaining combination message
  resolves to `invalid-value`, and the structural boundaries keep their own
  categories. The fixed payload ceiling resolves to `capacity`, and the string
  boundary reuses the node 1.5 ruling. The two slot-exclusivity combinations
  are not constructible on the wire, because one payload cannot carry both text
  fields, so their cases record the branch's own requirement at the slot and
  the DTO-level exclusivity is pinned by the Rust group test's mutated records.
  Provenance is `codec_server.go` plus `message_companion.go` and
  `companion_wire.go`.
- `validProducerIDs` is the closed exporter allowlist. It carries the 19
  protocol group producer IDs the v45 packet plan names
  (`runtime-oracle/protocol-negotiation`, `-control`, `-client-control`,
  `-client-rays`, `-client-inventory`, `-client-stack-views`, `-client-chat`,
  `-world-delta`, `-snapshot`, `-player-outcomes`, `-inventory-publication`,
  `-remote-players`, `-companions`, `-drops`, `-hostiles`, `-passives`,
  `-projectiles`, `-chat-event`) beside the existing frame and domain
  identities; an unrecognized ID is rejected before any directory is created.
- Every committed `*.expected.json` under `testdata/runtime-migration/cases/`
  publishes the frozen outcome vocabulary: `kind` is `ok` or `error`, and an
  `error` category is one of the structural, login admission, or storage values
  the execution contract names.
- Explicit fixture and asset export is gated by `RUNTIME_ORACLE_EXPORT_DIR`.
  There is no repository default: an unset variable exports nothing. When set,
  `exportGeneratedAssets` resolves the export root through its nearest existing
  ancestor, rejects repository containment and any symlink below that ancestor,
  and validates every asset path, duplicate, and file-as-parent collision before
  mutation. It creates export-root gaps, producer prefixes, and asset parents one
  component at a time with `Lstat`/`Mkdir`: existing components must be real
  directories, the final producer child (`<exportRoot>/<producerID>`) is created
  exclusively, and a preexisting final child is rejected. Containment is
  resolved again before fixed relative assets are opened with create-exclusive
  semantics, so exports cannot escape, follow a fixed-prefix symlink, or replace
  existing evidence.
  Production `main.go` reconciles and validates existing artifacts and has no
  trace-generation mode.
- Enforcement: `TestExportGeneratedAssetsRejectsRepositoryContainedRoots`,
  `TestExportGeneratedAssetsRejectsSymlinkedAncestor`,
  `TestExportGeneratedAssetsRejectsProducerPrefixSymlink`,
  `TestExportGeneratedAssetsRejectsNonDirectoryProducerPrefix`,
  `TestExportGeneratedAssetsRejectsEscapingRelativePath`,
  `TestExportGeneratedAssetsRejectsInvalidAssetSetBeforeCreation`,
  `TestExportGeneratedAssetsRejectsPreexistingProducerChild`,
  `TestExportGeneratedAssetsSuccessfulMultiProducerExport`,
  `TestProtocolOracleFrameIndependentOutcomes`,
  `TestCorpusOutcomeVocabularyMatchesExecutionContract`,
  `TestProtocolOracleFrameOutcomesDistinguishCases`,
  `TestProtocolOracleFrameRunnerRejects*`,
  `TestProtocolOracleFrameRunnerHandsProducerOnlyCaseAndInput`,
  `TestProtocolOracleFrameRunnerInvokesProducerOncePerCheckpoint`,
  `TestProtocolCorpusRoutesExecuteTwoOperationsForOneFamily`,
  `TestProtocolCorpusRunnerRejectsUnregisteredRoute`,
  `TestProtocolCorpusRunnerRejectsEmptySelection`,
  `TestProtocolCorpusRunnerRejectsMissingRoutes`,
  `TestProtocolCorpusRunnerRejectsDuplicateCheckpoints`,
  `TestProtocolCorpusFrameEncodeProducesCanonicalBytes`,
  `TestProtocolCorpusFrameEncodeRejectsOversizedPayload`,
  `TestProtocolCorpusFrameEncodeExpectedIDMutationFailsComparison`,
  `TestProtocolCorpusFrameCandidatesExportForReview`,
  `TestReadCaseInputUsesFormatBudget`,
  `TestProtocolOracleFrameExport*`,
  `TestProtocolNegotiationOracle*`.

## Protocol manifest candidate (`protocol_manifest_test.go`)

- `ProtocolSelection` is one packet producer group's exact reviewed
  registration: its producer ID, its case specifications, the repository-
  relative Go source paths its rules are read from, and the executable routes
  it claims. `mergeProtocolSelections(root, base, groups...)` clones the frozen
  manifest, rejects a duplicate or conflicting case, a case or route no
  consumer registration carries, a case route its own selection does not
  claim, an unrecognized producer ID, a non-protocol family and a missing
  provenance file, updates only the selected protocol families' case lists and
  source hashes, sorts case IDs and source paths, preserves `source_revision`,
  and reconciles the merged and the reloaded value against the production
  `Discover`/`ReconcileWorking` path.
- A candidate whose assets the controller has not integrated yet is accepted
  only through `verifyProtocolManifest`: the sole tolerated reconciliation
  failure is a problem naming an asset path of a case the base manifest does
  not register and that is absent from disk. A digest mismatch, a missing
  provenance source or a registration error keeps the file present or names a
  tracked value, so none of them can be classified as pending. The reviewed
  assets and the complete merged manifest are exported create-exclusively
  through `RUNTIME_ORACLE_EXPORT_DIR` under `runtime-oracle/protocol-frame`,
  reviewed by the controller, and then copied into the tracked corpus
  mechanically; the single global `source_revision` refresh stays the closure
  node's act.
- Enforcement: `TestProtocolCorpusManifestMergeRegistersFrameEncodeRoute`,
  `TestProtocolCorpusManifestMergePreservesUnrelatedFamilies`,
  `TestProtocolCorpusManifestMergeRejectsDuplicateAndConflictingCases`,
  `TestProtocolCorpusManifestMergeRejectsUnregisteredRoute`,
  `TestProtocolCorpusManifestMergeRejectsUnclaimedCaseRoute`,
  `TestProtocolNegotiationOracleManifestMergeRegistersPacketRoutes`,
  `TestProtocolNegotiationOracleRoutesExecuteEveryCase`,
  `TestProtocolNegotiationOracleRunnerRejectsUnregisteredRoute`,
  `TestProtocolNegotiationOracleCandidatesExportForReview`.

## Family evidence (`domain_*_test.go`, `agent_contract_test.go`)

Each executable corpus family in this package has exactly one producer file,
and each declares a family-scoped working manifest so a run never hands its
producer another family's cases: `protocol_frame_test.go` (framing), and
`domain_values_test.go`, `domain_identity_values_test.go`,
`domain_command_control_test.go`, `domain_command_inventory_test.go`,
`domain_event_player_test.go`, `domain_event_world_test.go`,
`domain_event_inventory_test.go`, `domain_event_people_test.go`,
`domain_event_mobs_test.go`, `domain_event_objects_test.go` and
`domain_event_chat_test.go` (the domain families). The world-event file also
carries the shared router arm (`runDomainEvent`) that the inventory, people,
mobs, objects and chat producers register their rule names into, because the
`domain.event` family is shared by seven producers and the rule name is the
only discriminator the manifest carries.

- `domain_event_mobs_test.go` executes the six hostile and passive mob
  publication rules (`hostile-spawn`, `hostile-state`, `hostile-despawn`,
  `passive-spawn`, `passive-state`, `passive-despawn`) through the Go
  `protocol` DTOs. Its 68 frozen cases live under
  `testdata/runtime-migration/cases/domain/event_mobs/` and are registered in
  the canonical manifest through the shared manifest-candidate helper below,
  raising the tracked corpus to 444 `mornlea_domain` cases and 232
  `domain.event` cases; the per-producer asset candidate still publishes
  external-only through `RUNTIME_ORACLE_EXPORT_DIR` under the producer ID
  `runtime-oracle/domain-event-mobs`.
  IDs and ticks are decimal strings in the frozen input so the full `u64`
  range stays lossless, grazing renders as a JSON Boolean in the normalized
  outcome, a rejected record retains the raw value of an unknown enum, and
  the 64-record wire batch caps stay transport budgets no case sits above.
  Rejection categories stay inside the frozen vocabulary: `invalid-identity`
  for a zero entity ID, `invalid-enum` for dimension, kind, grazing and
  reason, and `invalid-value` for everything else, with rule names shaped
  `<rule>.record_<index>.<field>`, `<rule>.count_range` and
  `<rule>.strictly_increasing_ids`.

- `domain_event_objects_test.go` executes the five projectile and item-drop
  publication rules (`projectile-spawn`, `projectile-state`,
  `projectile-despawn`, `item-drop-upserts`, `item-drop-removes`) through the
  Go `protocol` DTOs. Its 45 frozen cases live under
  `testdata/runtime-migration/cases/domain/event_objects/` and are registered
  in the canonical manifest through the shared manifest-candidate helper below,
  raising the tracked corpus to 489 `mornlea_domain` cases and 277
  `domain.event` cases; the per-producer asset candidate still publishes
  external-only through `RUNTIME_ORACLE_EXPORT_DIR` under the producer ID
  `runtime-oracle/domain-event-objects`.
  Projectile IDs and ticks are decimal strings in the frozen input so the full
  `u64` range stays lossless, a drop identity stays the object of its five
  ordered key fields with the raw i32 dimension first, and a carried stack
  stays the numeric `item/count/durability` object. A drop's raw dimension is
  deliberately unvalidated — the Go `DropID.Valid` rule checks only the slot
  range and the generation, so a negative raw dimension is an admitted
  boundary and the batch ordering compares the raw dimension first.
  Rejection categories stay inside the frozen vocabulary: `invalid-identity`
  for a zero projectile ID and for drop-ID slot/generation errors,
  `invalid-enum` for an unknown projectile kind or dimension, and
  `invalid-value` for block-index, stack, non-finite and aggregate errors,
  with rule names shaped `<rule>.record_<index>.<field>`,
  `item_drop_upserts.drop_<index>.<field>` and
  `item_drop_removes.id_<index>.<field>`, plus `<rule>.count_range` and
  `<rule>.strictly_increasing_ids`. The 128/32-record wire batch caps stay
  transport budgets no case sits above.

- `domain_event_chat_test.go` executes the sole closed-chat publication rule
  (`chat`) through the Go `protocol.ChatEvent` DTO, closing the Go evidence
  stage beside the mob (68 cases) and object (45 cases) producers with its
  44 frozen cases. They live under
  `testdata/runtime-migration/cases/domain/event_chat/` and are registered in
  the canonical manifest through the shared manifest-candidate helper below,
  closing the tracked corpus at 533 `mornlea_domain` cases and 321
  `domain.event` cases; the per-producer asset candidate still publishes
  external-only through `RUNTIME_ORACLE_EXPORT_DIR` under the producer ID
  `runtime-oracle/domain-event-chat`.
  The record is a semantic union rather than a flat payload: every raw input
  key is always present (a decimal-string event identity, the two
  32-lowercase-hex UUID identities, numeric kind and reason, and the two text
  slots), and an admitted event publishes its exact semantic branch
  (`accepted`, the four rejection branches, the five task-fact branches,
  `task-failed-<reason>` or `speech`) with only that branch's legal
  companion/name/command/speech data — the empty wire sentinels a branch
  forbids are not retained. The classifier mirrors the exact branch order of
  `ChatEvent.Validate`: global identity and name failures precede kind
  dispatch, a non-speech kind carrying speech fails before the switch, and
  inside the switch reason, companion identity/name, then command/speech text
  decide. Rejection categories stay inside the frozen vocabulary:
  `invalid-identity` for a zero event/player identity, `invalid-enum` for an
  unknown kind, the reserved reject reason 3 and failure reasons outside
  16..20, and `invalid-value` for every text-boundary (the 1,024/256 bounds
  and their plus-one rows, untrimmed or empty names, commands and speech) and
  every illegal cross-field combination (speech leak, command on speech,
  reason-not-none, missing companion, zero companion where illegal). Rule
  names begin `chat_event.` and name the failing field or combination:
  `chat_event.rejected.reason` for the reserved reason, `chat_event.task_failed.reason`
  for the failure-reason domain edges, `chat_event.kind` for an unknown kind,
  and `chat_event.<field>` for everything else. A rejection retains the raw
  semantic inputs, including an unknown numeric enum value.

- `agent_contract_test.go` is deliberately not an executable Agent producer. It
  validates manifest identity, case presence, golden coverage and the
  service-free ownership boundary for the explicit `external:agent-contract`
  consumer. Real Agent HTTP/MCP serialization execution remains package-local
  to `packages/shared/companion`; this package must not copy expected outcomes
  into `ExecutedObservation` values or publish a nominal Agent trace.

## Shared manifest candidate (`domain_event_manifest_test.go`)

- The corpus-registration chain grows the shared `domain.event` registration
  through one test-only helper: each producer exposes a selection
  (`domainEventMobsSelection` and successors) carrying only its exact reviewed
  `CaseSpec` values and provenance paths, and
  `mergeDomainEventSelections` clones the tracked manifest, rejects a
  duplicate or conflicting case ID in the base or any selection (a
  byte-identical re-registration is an idempotent no-op), sorts the complete
  top-level case list by ID, replaces the one `domain.event` family's case
  list with the sorted union of every top-level `domain.event` case, unions
  and re-hashes the family's provenance by repository-relative path,
  preserves `source_revision`, and reconciles the merged and the reloaded
  value against the production `Discover`/`ReconcileWorking` path. The one
  explicit `source_revision` refresh is the closing node's own act: the
  chat candidate test assigns the captured checkout SHA to the merged value
  before publication so the manifest and the Go `BaselineSourceRevision`
  constant stay identical.
- `writeDomainEventManifestCandidate` publishes one complete candidate as
  exactly `runtime-oracle/domain-event-manifest/contracts.json` below a fresh
  external `RUNTIME_ORACLE_EXPORT_DIR` (producer ID
  `runtime-oracle/domain-event-manifest`), delegating repository, symlink and
  freshness safety to the existing export helpers, reloading and reconciling
  the written bytes before returning the path. An empty export root writes
  nothing; ordinary test runs leave the tracked manifest and case directories
  unchanged, and the reviewed candidate is copied into
  `testdata/runtime-migration/contracts.json` mechanically, never assembled
  or edited by hand.
- Node 4.1 registered the 68 mob cases this way, raising the tracked corpus
  to 444 `mornlea_domain` cases and 232 `domain.event` cases, with the Rust
  `event_mobs` topic adapter in `mornlea_domain` owning all six rules.
- Node 4.2 registered the 45 object cases the same way: the tracked corpus
  reached 489 `mornlea_domain` cases and 277 `domain.event` cases with a
  30-path family provenance union, and the Rust `event_objects` topic
  adapter in `mornlea_domain` owns all five rules.
- Node 4.3 registered the 44 chat cases and closed the exact 533-case
  domain partition: the tracked corpus carries 533 `mornlea_domain` cases
  and 321 `domain.event` cases over the unchanged 30-path provenance union,
  the Rust `event_chat` topic adapter in `mornlea_domain` owns the chat
  rule, and the same node refreshed `source_revision` once to the captured
  checkout SHA `736af2f4b8bc3cbea4648733aa07e5ef9ce0e9d5`, which the Go
  `BaselineSourceRevision` constant carries identically. With the partition
  closed, every producer's re-merge gate now asserts the final 533/321
  totals rather than its node-era intermediate counts.
- Enforcement: `TestDomainEventManifestMergeRejectsDuplicateCase`,
  `TestDomainEventManifestMergeRejectsMissingFamilyCase`,
  `TestDomainEventManifestMergeSortsCasesSourcesAndFamilyCases`,
  `TestDomainEventManifestMergePreservesUnrelatedFamilies`,
  `TestDomainEventManifestCandidateRejectsRepositoryAndSymlinkTargets`,
  `TestDomainEventManifestCandidateReloadsAndReconciles`,
  `TestDomainEventMobsManifestCandidateRegistersEveryMobsCase`,
  `TestDomainEventObjectsManifestCandidateRegistersEveryObjectsCase`, and
  `TestDomainEventChatManifestCandidateClosesTheEventPartition`.

- Expected outcomes come from executing the real Go validator or codec, never
  from a hand-written value or a Rust result. Every test run compares generated
  bytes against the frozen corpus read-only; package-test flags or code paths
  capable of rewriting the tracked corpus are strictly prohibited.
- No file in this package may rewrite the frozen manifest: the CLI has no such
  flag and the test binary registers none, because both the discovery stub and
  any partial regeneration would silently gut the corpus. Manifest merges are
  controller-side and manual.
- Enforcement: `TestDomainOracle_<topic>` per producer,
  `TestAgentContractOracle*`, `TestCorpusOutcomeVocabularyMatchesExecutionContract`
  over every committed expectation, and `TestTestBinaryFlagsCannotRewriteTheFrozenCorpus`
  (which asserts that no flag containing `update` or mentioning tracked corpus
  rewrites can be registered in the test binary). The repository-wide
  `packages/audit` guards `TestCorpusTestFlagsCannotRewriteFrozenEvidence` and
  `TestCorpusWriterFlagGuardDetectsDrift` enumerate runtime-migration producer
  tests and reject update, rewrite, regeneration and write flags while leaving
  separately governed storage/protocol golden workflows outside this rule.

## Protocol coverage closure (`protocol_coverage_test.go`)

- `TestProtocolCorpusComplete` is the protocol-only zero-gap gate. It reads the
  closed route union from three independent sides — live registry discovery,
  the tracked manifest and `BaselineConsumerRegistry` — and requires the same
  60-family protocol set from each: 59 packet families beside the framing
  family, every family carrying both a decode and an encode route at the live
  protocol version. It then counts the frozen evidence per family: every
  family holds at least one valid decode, one valid encode and one invalid or
  boundary case, every protocol case carries exactly one zero checkpoint, and
  a packet family's case direction agrees with its `protocol.client.` or
  `protocol.server.` prefix.
- The same test runs `ReconcileWorking` under the closed union and requires
  every protocol family/version point to be covered while no protocol point
  stays uncovered; `ReconcileComplete` must still refuse with only the
  non-protocol uncovered points, so the protocol closure never claims complete
  F1. The tracked tree stays byte-identical through the run.
- `TestProtocolCorpusClosureMutationsFail` pins each drift class the closure
  names against a synthetic working manifest: a zero-case family, a duplicate
  case ID, a case with no registered route, a missing source hash, a
  mismatched digest, an unsupported case version, and a case no producer
  executes. `TestProtocolCorpusGroupKeysGateExecution` executes one reviewed
  case per producer group through its own route map and then refuses an
  altered key, a wrong direction and a wrong state, with the unmutated case
  reproducing the frozen expectation first so the refusals are attributable to
  the mutation.
- `TestProtocolCorpusNonProtocolEvidenceUnchanged` pins the reviewed exact
  totals — 534 domain, 154 agent and 436 protocol cases — and re-reconciles
  the complete manifest so any protocol-side rewrite of a domain or agent case
  fails.
- `TestProtocolCorpusClosureCandidateExport` stages the one-time source
  revision refresh as an external candidate through the reviewed
  create-exclusive exporter: an unset `RUNTIME_ORACLE_EXPORT_DIR` writes
  nothing, and a set variable publishes the complete manifest with the current
  `git rev-parse HEAD` revision. The tracked manifest and
  `BaselineSourceRevision` stay on their recorded value until the controller
  integrates the reviewed candidate.

## Focused Verification

```bash
go test ./packages/tools/cmd/runtime-oracle -run TestContractInventory -count=1
go test ./packages/tools/cmd/runtime-oracle -list TestContractInventory
go test ./packages/tools/cmd/runtime-oracle -run '^TestProtocolOracleFrame' -count=1
go test ./packages/tools/cmd/runtime-oracle -run '^TestProtocolInventoryPublicationOracle' -count=1
go test ./packages/tools/cmd/runtime-oracle -run '^TestProtocolRemotePlayersOracle' -count=1
go test ./packages/tools/cmd/runtime-oracle -run '^TestProtocolCompanionsOracle' -count=1
go test ./packages/tools/cmd/runtime-oracle -run '^TestProtocolDropsOracle' -count=1
go test ./packages/tools/cmd/runtime-oracle -run '^TestProtocolHostilesOracle' -count=1
go test ./packages/tools/cmd/runtime-oracle -run '^TestProtocolPassivesOracle' -count=1
go test ./packages/tools/cmd/runtime-oracle -run '^TestProtocolProjectilesOracle' -count=1
go test ./packages/tools/cmd/runtime-oracle -run '^TestProtocolChatEventOracle' -count=1
go test ./packages/tools/cmd/runtime-oracle -run '^TestProtocolCorpus' -count=1
go test ./packages/tools/cmd/runtime-oracle -run '^TestProtocolCorpusComplete' -count=1
go test ./packages/tools/cmd/runtime-oracle -run '^TestProtocolCorpus' -count=1
go test ./packages/tools/cmd/runtime-oracle -race -count=1
rustup run 1.97.1 cargo test --manifest-path packages/engine/Cargo.toml -p mornlea_protocol --test runtime_contract --locked corpus_frame
rustup run 1.97.1 cargo test --manifest-path packages/engine/Cargo.toml -p mornlea_protocol --test protocol_corpus --locked
```
