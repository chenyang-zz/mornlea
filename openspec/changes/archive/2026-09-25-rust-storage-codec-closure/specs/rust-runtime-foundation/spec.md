## ADDED Requirements

### Requirement: Every supported save version executes against Go evidence

The Rust storage consumer SHALL independently execute decode and applicable migration for chunk schemas 1..9, player schemas 1..9, companion schemas 1..5, hostile schemas 1..2, passive schema 1, region format 1 and world metadata versions 1..6. It SHALL compare normalized values and current noncompressed bytes with outcomes produced by the current Go codec. Chunk compression MAY differ bytewise only when both implementations decode each other's output to the same complete logical chunk. Missing, zero-case, duplicate, unexecuted, unbound or stale cases MUST fail qualification.

#### Scenario: Historical save comparison

- **GIVEN** one source-bound case for each supported historical version and its current Go decode/migration observation
- **WHEN** the Rust consumer executes the frozen input
- **THEN** every admitted field, migration default, identity, record order and rewrite flag MUST match its Go observation

#### Scenario: Corpus case is mutated or omitted

- **GIVEN** a changed input digest, expected scalar, case route, supported version or missing family case
- **WHEN** storage acceptance reconciles and executes the corpus
- **THEN** it MUST fail rather than count the manifest name as behavioral coverage

### Requirement: Noncompressed save writers publish atomically

The Rust player, world-metadata, hostile, passive and companion codecs SHALL provide exact encoded lengths and caller-buffer encoding for current-schema saves. All semantic validity, checked lengths, canonical ordering and format maxima MUST be established before writing. A short destination SHALL report required and available lengths; an invalid value MUST take precedence over destination capacity. Every failed call MUST leave the entire destination unchanged, and a successful call MUST preserve any unused destination tail.

#### Scenario: Short caller buffer

- **GIVEN** a valid current save of encoded length N and an N-1 byte destination filled with canaries
- **WHEN** caller-buffer encoding runs
- **THEN** it MUST return the output-too-small error with N and N-1 and leave every canary unchanged

#### Scenario: Invalid save and short buffer

- **GIVEN** an invalid current save and a short destination
- **WHEN** caller-buffer encoding runs
- **THEN** it MUST report corruption before capacity and leave the destination unchanged

### Requirement: Metadata format preserves raw values

The Rust world-metadata codec SHALL preserve the Go v6 format's signed seed and spawn dimension, full-width phase and duration values, every weather byte, two-dimension table and difficulty admission. It SHALL apply documented defaults when reading v1..v5 and MUST NOT apply runtime weather or phase normalization inside the codec.

#### Scenario: Unknown weather byte

- **GIVEN** valid v6 metadata with weather 7 or 255, spawn dimension -3 and maximum day-phase offset
- **WHEN** Go and Rust encode and decode it
- **THEN** both MUST preserve those raw values and bytes while difficulty 3 remains rejected

### Requirement: Chunk compression remains bounded and complete

The Rust chunk codec SHALL validate the complete aggregate before encoding, enforce a logical payload at most 2 MiB and a compressed frame at most 1 MiB, and decode within the same limits without publishing partial chunks. A reusable codec SHALL own its scratch exclusively and recover cleanly after a failed operation. It MUST preserve all nine supported chunk migrations and cross-decoder logical equivalence.

#### Scenario: Reused codec after malformed frame

- **GIVEN** a codec instance that receives an oversized declaration, truncated frame or corrupt checksum
- **WHEN** it next handles a valid chunk
- **THEN** the first call MUST fail without a partial value and the next MUST match a fresh codec's full result

### Requirement: Companion history is not silently invented

The Rust companion codec SHALL decode v1..v5 with complete body, queue, task and lifecycle fields. A v1..v4 decoded record without a valid v5 namespace and lifecycle set MUST NOT be silently encoded as v5; any bootstrap or identity-generation policy remains a separate server decision.

#### Scenario: Legacy companion decode

- **GIVEN** valid v2, v3 or v4 bytes with tasks, FIFO and legacy summary
- **WHEN** Rust decodes them
- **THEN** it MUST preserve the owner and ordered content and identify the source schema
- **AND** it MUST NOT invent a v5 namespace, lifecycle or tombstone to claim current re-encode parity

### Requirement: Region corpus preserves committed-bank selection

The Rust region consumer SHALL compare exact v1 superblock/bank bytes and selection with the Go codec, including signed keys, standby banks, zero-length occupied payloads, CRC, extents and divergent equal-generation corruption. The fixed 1,024-slot representation and caller-buffer behavior already required by the canonical region requirements remain in force.

#### Scenario: Equal-generation conflict

- **GIVEN** two individually valid committed banks with equal generations and different entries
- **WHEN** Rust and Go select a bank
- **THEN** both MUST reject the region rather than choose one silently
