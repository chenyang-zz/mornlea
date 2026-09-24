## ADDED Requirements

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
