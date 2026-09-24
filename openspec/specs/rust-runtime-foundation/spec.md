# rust-runtime-foundation Specification

## Purpose

Define the reviewed offline evidence substrate and bounded shared Rust domain
contracts that later server, client, protocol, storage and numerical migration
changes can consume without changing the current online authority.

## Requirements

### Requirement: Working evidence and complete acceptance are distinct

The foundation SHALL provide an in-progress validation mode that validates every
present family and case while allowing explicitly uncovered families, and a
separate complete-acceptance mode that MUST reject every supported family or
version without executable coverage. Every case consumer identity MUST resolve
through a closed registry; a nonempty arbitrary string MUST NOT count as a
callable consumer.

#### Scenario: An in-progress manifest names future work

- **WHEN** a valid inventory contains a registered family with no cases and is
  checked in working mode
- **THEN** the present identities, sources and cases MUST be validated
- **AND** the uncovered family MUST remain explicitly visible rather than being
  counted as complete coverage

#### Scenario: Complete acceptance sees a zero-case family

- **WHEN** the same inventory is checked in complete-acceptance mode
- **THEN** validation MUST fail with the uncovered family and supported version
  identified

#### Scenario: A case names an unknown consumer

- **WHEN** a case names a consumer outside the closed callable registry
- **THEN** both working and complete validation MUST reject it

### Requirement: Trace observations come from executed operations

An offline trace SHALL contain observations produced by executing the selected
registered operations. The trace builder MUST NOT manufacture observations by
copying committed expected values. Validation MUST bind the trace to an explicit
corpus root and MUST fail on root resolution, file reads, digests, JSON shape or
normalized-value comparison errors.

#### Scenario: Behavior changes while expected files remain unchanged

- **WHEN** one selected operation returns a changed normalized scalar while the
  manifest, sources and expected files remain present
- **THEN** trace comparison MUST report the behavioral mismatch

#### Scenario: Expected evidence cannot be loaded

- **WHEN** an expected asset is missing, malformed, has a wrong digest or is
  resolved from a different corpus root
- **THEN** trace validation MUST fail before accepting the observation

### Requirement: Corpus generation never rewrites tracked evidence

Fixture producers SHALL write only into harness-owned temporary storage or a
fresh external export directory that passes the same containment, symlink and
no-replace checks as trace publication. Tests and commands MUST NOT rename,
replace or rewrite a tracked manifest or case asset. Committing generated output
remains a separately reviewed controller operation.

#### Scenario: A producer is asked to export into the repository

- **WHEN** a fixture producer receives an export path inside the repository or
  through a symbolic-link ancestor
- **THEN** it MUST reject the path without modifying tracked evidence

#### Scenario: A test checks symbolic-link rejection

- **WHEN** a test needs a malformed repository-shaped corpus
- **THEN** it MUST construct that corpus under harness-owned temporary storage
  rather than replacing a tracked fixture

### Requirement: Rust and Go enforce the same corpus integrity

For the shared manifest and case-asset subset it consumes, the Rust corpus
consumer SHALL enforce schema and case budgets, unique family/case identity,
family membership and case lists, supported case versions, ID prefixes, closed
operation/input-format/consumer values, nonempty decimal checkpoints,
repository-relative contained paths, every-component symbolic-link rejection,
regular-file and byte budgets, duplicate-key rejection and content digests
before returning a case. Live registry discovery, current identity comparison
and source-provenance comparison remain Go reconciliation responsibilities and
MUST NOT be claimed by the Rust loader.

#### Scenario: Corpus metadata is ambiguous

- **WHEN** a manifest contains a duplicate JSON key, unknown input format,
  unknown consumer or a case path with a symbolic-link ancestor
- **THEN** the Rust consumer MUST reject it before executing the case

### Requirement: Implemented domain cases execute independently in Rust

Every frozen case whose registered consumer is `mornlea_domain` SHALL execute
through the corresponding Rust domain constructor and compare its normalized
semantic outcome with the Go-produced expectation. Command expectations MAY
project out the Go wire bytes, because protocol encoding belongs to a successor
consumer, but every remaining field and every rejection MUST compare exactly.
The authoritative `domain.input` engine-step case MUST be registered to a
closed external Go consumer rather than misrepresented as executable by the
domain-only Rust orderer. Rust command ordering remains required through its
focused behavioral suite. Ownership MUST be bound by case consumer and family
identity, and a test filter that discovers zero cases MUST NOT qualify as
evidence.

#### Scenario: All baseline domain cases agree

- **WHEN** the Rust-owned baseline domain corpus is executed at its recorded identity
- **THEN** every selected case MUST run exactly once through Rust
- **AND** the normalized accepted value or typed rejection MUST equal the
  independently produced Go semantic result

#### Scenario: One domain outcome is mutated

- **WHEN** one executed domain value differs from the frozen Go expectation
- **THEN** the domain corpus gate MUST fail even though ownership strings and
  source files remain unchanged

### Requirement: Shared domain values preserve current semantics

The foundation SHALL preserve the current validated identity, text, item,
location, command and implemented publication semantics. Sequenced commands
MUST order by tick, session, sequence and arrival without a command-kind tie
breaker; chat MUST remain a separate unsequenced intent. Domain publication
records MUST preserve their validated source order and fields without wire IDs,
save migration policy or online authority state.

#### Scenario: Equal-sequence commands and chat arrive together

- **WHEN** two commands from one session share a sequence but have distinct
  arrival positions and chat arrives through its separate intake
- **THEN** the earlier command arrival MUST win the command ordering tie
- **AND** chat MUST retain no fabricated command sequence

#### Scenario: Implemented publication values are constructed

- **WHEN** player, world, inventory, remote-player or companion records are
  converted into shared domain values
- **THEN** their validated semantic fields and order MUST be preserved
- **AND** no packet ID, digest-only surrogate or authority-only state may replace
  those values

### Requirement: Domain construction is bounded before proportional work

Public domain validation SHALL enforce byte or record budgets before
proportional scanning, sorting or copying. Text validation MUST reject a display
name above 128 UTF-8 bytes before scalar iteration. Variable semantic batches
MUST reject more than 4096 records; smaller protocol packet limits remain
separate. Fallible temporary allocation MUST return a typed failure without
partial publication.

#### Scenario: An oversized semantic batch is submitted

- **WHEN** a safe Rust caller submits 4097 block changes, forgotten chunks,
  remote-player states or companion states
- **THEN** construction MUST return the bounded-batch failure before scanning,
  sorting or copying the records

#### Scenario: Bounded uniqueness scratch cannot be allocated

- **WHEN** uniqueness validation cannot reserve its bounded temporary storage
- **THEN** construction MUST return a typed allocation failure
- **AND** no partially validated value may be published

### Requirement: Baseline validation is headless and dependency-safe

The baseline SHALL run without a graphical host, audio device, embedded Python
runtime, live network authority or live save access. Foundation production
dependency direction MUST remain domain with no dependencies and protocol or
storage depending only on domain plus their approved compression dependency.

#### Scenario: Baseline gates run in a headless environment

- **WHEN** the evidence and domain validation suites run without presentation
  runtimes or a live server
- **THEN** they MUST execute their behavior and dependency checks successfully
  without changing the default Go runtime

### Requirement: Remaining publication families use checked semantic values

The Rust domain SHALL provide checked values for hostile mobs, passive mobs,
projectiles, item drops and chat publications. Accepted values MUST preserve
every semantic field and the validated record order of the current Go source
contract without carrying packet identifiers, wire padding, persistence policy,
authority-only state or presentation state. Hostile and passive spawns MUST be
overworld values; projectile spawns MUST accept both supported dimensions and
MUST NOT impose an authority-only kind-by-dimension rule. Item drops MUST retain
the raw dimension semantics of `DropId`, MUST accept the valid empty
`ItemStack`, and MUST reject a block index outside the 98,304-cell chunk. Chat
MUST be a closed tagged union in which invalid kind, reason, identity, command
and speech combinations are unrepresentable after construction.

#### Scenario: A valid remaining publication is constructed

- **WHEN** a Go-valid hostile, passive, projectile, item-drop or chat
  publication is converted into its Rust domain value
- **THEN** every semantic field and record position MUST be preserved
- **AND** the result MUST contain no packet ID, opaque wire bytes, digest-only
  surrogate or authority-only state

#### Scenario: Projectile policy stays with the authority

- **WHEN** a projectile spawn contains either supported projectile kind in
  either the overworld or depths
- **THEN** the domain constructor MUST accept the combination when its identity,
  position and velocity are otherwise valid
- **AND** any narrower kind-by-dimension rule MUST remain outside the shared
  semantic value

#### Scenario: A wire-shaped chat combination is semantically illegal

- **WHEN** a chat source carries an unknown kind or reason, a missing required
  identity, command text on speech, speech text on a non-speech event, or a
  task-failure reason on another event
- **THEN** conversion MUST reject the complete event before publication
### Requirement: The semantic event surface is exhaustive and wire-independent

The production domain SHALL expose an exhaustive `Event` enum with exactly
these 30 variants: `ChunkSnapshot`, `BlockChanges`, `ForgetChunks`,
`PlayerState`, `CommandRejected`, `RemotePlayerSpawn`,
`RemotePlayerDespawn`, `RemotePlayerStates`, `InventoryState`,
`ItemDropUpserts`, `ItemDropRemoves`, `FurnaceState`, `ContainerClosed`,
`ChestState`, `Chat`, `CompanionSpawn`, `CompanionStates`,
`CompanionDespawn`, `PlaceBlockSucceeded`, `CraftingState`, `HostileSpawn`,
`HostileState`, `HostileDespawn`, `CombatHit`, `PassiveSpawn`, `PassiveState`,
`PassiveDespawn`, `ProjectileSpawn`, `ProjectileState` and
`ProjectileDespawn`. Publication routing SHALL be a separate `RoutedEvent`
envelope with `EventRecipient::Session(u64)` and
`EventRecipient::Broadcast`. The surface MUST exclude transport lifecycle
messages and runtime worker lifecycle messages, and MUST NOT provide an opaque,
digest or catch-all variant.

#### Scenario: Every semantic publication is represented

- **WHEN** the event-surface contract suite enumerates the public `Event` union
- **THEN** each of the 30 named semantic variants MUST be constructed and
  observed exactly once
- **AND** the contained value MUST be the corresponding checked domain value

#### Scenario: Routing does not alter semantic values

- **WHEN** a checked event is addressed to one runtime session or broadcast
- **THEN** the recipient MUST be carried only by `RoutedEvent`
- **AND** the domain MUST NOT validate session existence, subscription state or
  visibility policy

#### Scenario: A lifecycle message reaches the domain boundary

- **WHEN** a handshake, login, rejection, keepalive, disconnect, chunk-worker
  acquisition, generation, readiness or resynchronization message is considered
- **THEN** it MUST remain outside `Event`
- **AND** it MUST NOT be hidden behind a generic byte or digest variant
### Requirement: Remaining semantic batches are bounded and atomic

Every hostile, passive, projectile and item-drop semantic batch SHALL own its
records, SHALL contain between 1 and 4,096 records, and SHALL preserve strictly
increasing typed identity order. The 4,096-record semantic cap MUST be checked
before any per-record scan, copy or sort. A duplicate, reversed, invalid or
oversized record MUST reject the complete batch without publishing a partial
value. Protocol packet maxima such as 32, 64 and 128 records MUST remain
protocol-owned and MUST NOT narrow the domain batch type.

#### Scenario: An oversized remaining batch is submitted

- **WHEN** a safe Rust caller submits 4,097 hostile, passive, projectile or
  item-drop records
- **THEN** construction MUST return the bounded-batch failure before inspecting
  an individual record
- **AND** no partial batch may be published

#### Scenario: Record identity order is not canonical

- **WHEN** a nonempty batch contains a duplicate identity or a later record
  whose typed identity is lower than its predecessor
- **THEN** construction MUST reject the complete batch without reordering it

#### Scenario: A domain-valid batch exceeds one packet limit

- **WHEN** a batch contains no more than 4,096 valid records but exceeds its
  protocol packet maximum
- **THEN** the domain value MAY be constructed
- **AND** the protocol adapter MUST remain responsible for rejecting or
  partitioning it according to the separately versioned wire contract
### Requirement: Remaining event evidence executes independently in Go and Rust

The frozen runtime-migration corpus SHALL obtain expected remaining-event
outcomes from current package-local Go validators, and the Rust consumer SHALL
execute the corresponding checked constructors and exhaustive event union
independently. Wire codec evidence remains owned by the later protocol
successor. Every selected case MUST execute exactly once;
zero-case discovery MUST fail qualification. The evidence SHALL cover every
legal chat branch, boundary and rejection cases for every new record family,
strict ordering, semantic-versus-wire limits, and mutations of individual
semantic fields. Producers and tests MUST write only to harness-owned temporary
or external export storage and MUST NOT rewrite tracked corpus evidence.

#### Scenario: Go and Rust agree on the remaining families

- **WHEN** the recorded remaining-event corpus is executed at its bound source
  identity
- **THEN** each selected Go producer and Rust consumer case MUST run exactly
  once
- **AND** every normalized accepted value or typed rejection MUST match

#### Scenario: One semantic field is mutated

- **WHEN** one expected kind, health, grazing state, despawn reason, dimension,
  velocity, block index, identity order or chat branch differs from the value
  produced by Rust
- **THEN** the event evidence gate MUST fail even if every file and ownership
  identifier remains present

#### Scenario: A corpus generator is aimed at tracked evidence

- **WHEN** the remaining-event producer receives an output path inside the
  repository or through a symbolic-link ancestor
- **THEN** it MUST reject the request without changing a tracked manifest or
  case asset

### Requirement: Rust region banks preserve the fixed v1 format

The Rust region contract SHALL accept exactly 1,024 bank slots and SHALL NOT silently add, discard or index past slots supplied by a safe caller. It SHALL preserve the Go v1 superblock and bank geometry, signed region key bits, little-endian fields, CRC32C coverage and canonical zero padding. It MUST reject invalid entries before publishing a bank.

#### Scenario: Valid fixed bank

- **GIVEN** a region key with negative dimension and coordinates, a committed bank with 1,024 slots and a valid occupied extent starting at sector 15
- **WHEN** Rust encodes and decodes the region bank
- **THEN** the result MUST contain exactly the original 1,024 entries and the Go v1 byte layout

#### Scenario: Wrong slot cardinality

- **GIVEN** a safe caller supplies 1,023 or 1,025 entries for a new bank
- **WHEN** the bank is constructed for encoding
- **THEN** construction MUST fail before any bank bytes are published

#### Scenario: Invalid extent or reserved bytes

- **GIVEN** an occupied extent before sector 15, an overlapping or overflowing extent, or a bank with nonzero reserved or padding bytes and a resealed checksum
- **WHEN** Rust validates the bank
- **THEN** it MUST reject the complete bank as corrupt

### Requirement: Rust region encoding is atomic in caller buffers

The Rust region contract SHALL provide caller-buffer encoding for the 4,096-byte superblock and the 28,672-byte bank. A valid call MUST write exactly the format length and preserve any destination tail. An invalid bank MUST be reported before destination capacity, and every failed call MUST leave the entire destination unchanged.

#### Scenario: Short destination

- **GIVEN** a valid bank and a 28,671-byte destination filled with canary bytes
- **WHEN** Rust attempts caller-buffer encoding
- **THEN** it MUST report the required and available lengths and MUST leave every canary byte unchanged

#### Scenario: Invalid bank and short destination

- **GIVEN** an invalid occupied entry and a destination shorter than 28,672 bytes
- **WHEN** Rust attempts caller-buffer encoding
- **THEN** it MUST report the invalid bank first and MUST leave the destination unchanged

#### Scenario: Oversized destination

- **GIVEN** a valid superblock or bank and a destination longer than its format length
- **WHEN** Rust encodes into that destination
- **THEN** it MUST return the exact bytes written and MUST leave the remaining destination bytes unchanged

### Requirement: Region recovery selects only a valid committed bank

Rust region decoding SHALL distinguish unsupported future versions from corrupt records, reject truncated or trailing records, and enforce occupied extents against the declared file size. Recovery MUST select the newest valid nonzero-generation bank, prefer bank A on identical generation and content, and reject divergent equal-generation banks or two invalid banks.

#### Scenario: One committed bank survives corruption

- **GIVEN** bank A is corrupt and bank B is valid with nonzero generation
- **WHEN** Rust selects a region bank
- **THEN** it MUST select bank B without repairing or publishing bank A

#### Scenario: Equal generation conflict

- **GIVEN** two valid banks with the same nonzero generation but different entries
- **WHEN** Rust selects a region bank
- **THEN** it MUST reject the conflict instead of choosing a bank by position

#### Scenario: Future format version

- **GIVEN** a region superblock or bank whose version exceeds v1
- **WHEN** Rust decodes it
- **THEN** it MUST report a future-version failure rather than treating the record as a supported format

### Requirement: Rust protocol covers every current packet key

The Rust protocol SHALL recognize exactly the current v45 direction, state and packet-ID combinations, preserve every accepted field and wire value, and reject unknown combinations, reserved IDs, malformed payloads and trailing bytes. Equivalent non-compressed packets MUST retain their Go wire bytes; a compressed chunk snapshot MUST retain its complete logical contents and cross-decoder interoperability even when two zstd encoders emit different compressed blocks. A packet identifier MUST NOT become a domain command or event field.

#### Scenario: Complete bidirectional packet coverage

- **GIVEN** the frozen current v45 registry and one valid Go-produced payload for each registered key
- **WHEN** the Rust protocol decodes and re-encodes each key in its declared direction and state
- **THEN** every packet MUST preserve all fields and the expected wire or logical snapshot result
- **AND** the number of executed keys MUST equal the independently discovered registry count

#### Scenario: Wrong state or reserved key

- **GIVEN** a valid Play payload presented under Login, a reversed direction, or reserved client Play ID 1
- **WHEN** Rust dispatches the record
- **THEN** it MUST reject the entire record before publishing a typed packet

### Requirement: Inbound negotiation separates structure from admission

Inbound handshake and login decoding SHALL preserve structurally valid values long enough to return the established version or login rejection. Identity, canonical display name and view-distance admission MUST follow the current decision order. Outbound records MUST remain fully validated before encoding. The protocol contract MUST leave session creation, deadlines, subscription distance clamping and transport ownership to the server.

#### Scenario: Old client version

- **GIVEN** a structurally valid handshake declaring an older protocol version
- **WHEN** Rust evaluates negotiation
- **THEN** the result MUST be a version mismatch carrying current version 45, rather than an unclassified decode error or an admitted session

#### Scenario: Multiple invalid login fields

- **GIVEN** a structurally valid login with an invalid identity and a view distance outside 2..=64
- **WHEN** Rust evaluates admission
- **THEN** invalid identity MUST win, and no admitted identity MUST be published

#### Scenario: Normalized inbound name

- **GIVEN** a valid raw UTF-8 login name with trim-only surrounding whitespace and a valid canonical result
- **WHEN** Rust evaluates admission
- **THEN** it MUST retain the canonical name and the declared in-range distance without silently clamping either

### Requirement: Protocol failures are bounded and atomic

Every frame and packet operation SHALL check canonical encoding, declared lengths, semantic values, count and decompression limits before proportional allocation or publication. Frame bodies MUST stay within 2 MiB, small packet payloads within 64 KiB, compressed snapshots within 1 MiB and decoded snapshots within 2 MiB. An invalid input, destination capacity failure or checked reservation failure MUST return a stable failure without a partial packet, changed caller output buffer or unbounded work. A coalesced input MAY contain later frames but a single read MUST consume only the first.

#### Scenario: Invalid value with a short destination

- **GIVEN** a mutable packet with an invalid field and an output buffer shorter than its valid encoding
- **WHEN** Rust attempts to encode it
- **THEN** field validation MUST fail before capacity reporting, and the output buffer MUST remain unchanged

#### Scenario: Declared oversized batch or snapshot

- **GIVEN** a declared record count or decompressed length above its packet-specific ceiling
- **WHEN** Rust decodes the payload
- **THEN** it MUST reject before allocating or scanning the declared content and MUST publish no partial record

#### Scenario: Two frames in one input

- **GIVEN** two canonical frames concatenated in one byte slice
- **WHEN** Rust reads one frame
- **THEN** it MUST return only the first frame and its exact consumed byte count, leaving the second available to the caller

### Requirement: Wire-to-semantic conversion preserves ownership

The Rust protocol SHALL convert validated Play client packets to the existing Rust semantic input types and validated server publication packets to the existing Rust semantic event types without inventing authority metadata, losing fields, changing record order or interpreting absence as a valid identity. Transport lifecycle packets MUST remain outside semantic commands and events. Protocol batch limits MUST remain distinct from the domain's larger semantic work bound.

#### Scenario: Valid semantic conversion

- **GIVEN** a valid command or publication with ordered records and all declared fields
- **WHEN** Rust converts it between wire and semantic values
- **THEN** the semantic result MUST retain every value and order, and a reverse conversion MUST reproduce an equivalent wire record

#### Scenario: Domain-valid value exceeds one wire batch

- **GIVEN** a valid semantic batch above its packet-specific count ceiling but within the domain's 4,096-record work cap
- **WHEN** Rust requests one wire packet
- **THEN** the conversion MUST fail explicitly rather than truncate, silently partition or weaken the domain type

#### Scenario: Transport control value

- **GIVEN** a handshake, login, keepalive or disconnect packet
- **WHEN** Rust requests a gameplay command or publication
- **THEN** it MUST reject that conversion without creating a placeholder semantic value

### Requirement: Protocol parity evidence is executable and source-bound

The protocol qualification SHALL execute independent Go producer behavior and Rust consumer behavior for every current packet family and declared operation. It MUST include valid, invalid and boundary observations, compare normalized outcomes rather than localized error prose, reject an absent or zero-case family, detect field or dispatch mutations, and bind all reviewed corpus assets to their source and content identities. Producers MUST write candidates only to a harness-owned temporary tree or an external export destination and MUST NOT rewrite tracked evidence.

#### Scenario: One packet family is missing

- **GIVEN** a frozen v45 family with no executed Rust case or a missing registered key
- **WHEN** protocol acceptance runs
- **THEN** coverage MUST identify the exact family/version/operation and remain incomplete

#### Scenario: One field or key changes

- **GIVEN** a deliberate mutation to one expected packet field, record order, direction, state or packet ID
- **WHEN** the Rust consumer compares its actual result with the source-bound case
- **THEN** qualification MUST fail even if the number of files and cases is unchanged

#### Scenario: Export targets tracked evidence

- **GIVEN** a producer export path inside the repository or through a symbolic-link ancestor
- **WHEN** it attempts to publish a candidate
- **THEN** it MUST refuse without modifying any tracked corpus asset

### Requirement: Legacy companion queues retain their owner

The Rust storage decoder SHALL preserve each nonempty v2, v3 and v4 companion task queue under the identity of its containing body. It SHALL preserve the queue's ordered task, FIFO and legacy summary fields and MUST NOT invent a queue for a body with no queue.

#### Scenario: Two legacy bodies have different work

- **GIVEN** a valid legacy aggregate with two distinct bodies and different nonempty queues
- **WHEN** Rust decodes the aggregate
- **THEN** each queue SHALL carry its own body's identity and complete ordered content
- **AND** the normalized result SHALL match the Go decoder

#### Scenario: A body has no legacy work

- **GIVEN** a valid legacy body with no current task, FIFO or summary
- **WHEN** Rust decodes it
- **THEN** no queue SHALL be synthesized for that body

### Requirement: Companion aggregate admission is bounded

A constructed v5 Rust companion save SHALL admit at most 64 bodies and matching lifecycles, at most four active bodies, and queues belonging only to active bodies. Count, membership, uniqueness, task, text and file-size failures MUST reject the whole save without partial output. An oversized body count MUST be rejected before cloning, sorting or scanning individual bodies.

#### Scenario: Body count boundary

- **GIVEN** otherwise valid saves with 64 and 65 inactive bodies respectively
- **WHEN** Rust encodes each save
- **THEN** the 64-body save SHALL be accepted and the 65-body save SHALL fail before proportional work

#### Scenario: Active and queue membership boundary

- **GIVEN** a fifth active body, a missing or duplicate lifecycle, or an orphan or inactive queue
- **WHEN** Rust validates the save
- **THEN** it MUST reject the complete save as corrupt without bytes

#### Scenario: Task and physical limits

- **GIVEN** a task command above 1,024 bytes, a plan above 5,000 steps, a FIFO above 16 entries, or a summary above 2,048 bytes
- **WHEN** Rust attempts encoding
- **THEN** it MUST reject before publishing bytes
- **AND** a save at every legal structural maximum MUST remain at or below 393,904 bytes

#### Scenario: Oversized lifecycle text is rejected before copying

- **GIVEN** an otherwise valid active lifecycle with a summary above 2,048 bytes
- **WHEN** Rust attempts encoding
- **THEN** it MUST reject the save without copying the oversized summary or publishing bytes

### Requirement: Every chunk encoder applies aggregate validity

Every public Rust chunk encoder SHALL reject an active furnace or chest whose block index is outside 0..98,303, whose block is the wrong kind, or whose kind-specific active index duplicates another slot. It SHALL validate the fixed 24-section, 32-drop, 32-furnace and 16-chest shapes and every section and slot before compression or output publication. A rejected encoder MUST NOT silently produce bytes a Rust or Go decoder would reject.

#### Scenario: Active container points at air

- **GIVEN** a structurally valid chunk with an active furnace or chest on an air block
- **WHEN** current, historical-schema or logical Rust encoding is requested
- **THEN** each entry point MUST reject the complete chunk before publishing bytes

#### Scenario: Boundary index and duplicate

- **GIVEN** a matching block at index 98,303, an active slot at index 98,304, or two active same-kind slots sharing one index
- **WHEN** Rust validates the chunk
- **THEN** only the first case SHALL be accepted

#### Scenario: Historical output preserves representable state

- **GIVEN** a valid current chunk containing a drop, furnace, chest, or durability field omitted by a requested older schema
- **WHEN** Rust requests historical envelope or logical encoding
- **THEN** it MUST reject rather than silently discard or change that state
- **AND** a raw pre-v5 logical chunk with a valid legacy multi-item tool drop SHALL remain exactly reserializable at its declared schema

### Requirement: Current save values use one domain rule source

Rust storage SHALL apply the Rust domain's current UUIDv4, ordinary item-stack, registered block and compact-section rules at the save-to-domain boundary. Format DTOs MAY retain raw historical bytes needed for migration. The codec MUST preserve player armor's raw 20-byte fidelity, including item numbers or durability that ordinary stacks reject, and MUST keep format validation separate from gameplay restoration.

#### Scenario: Ordinary stack and raw armor differ

- **GIVEN** an ordinary stack with unknown item 66 or invalid tool durability and a player armor slot with raw triple (4242,65,999)
- **WHEN** the current save is validated
- **THEN** the ordinary stack MUST be rejected by the shared domain rule and the armor bytes MUST be preserved without ordinary-stack admission

#### Scenario: Compact section remains compact

- **GIVEN** a valid indexed section with a nontrivial palette and packed words
- **WHEN** storage validates and converts it for a domain consumer
- **THEN** it SHALL preserve palette order and packed word bits without expanding or reordering cells
