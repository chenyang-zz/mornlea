## Context

The Go server remains the only production authority. The archived protocol and region successors have passed their own gates, while F1 storage and numerical coverage remain incomplete. The Rust storage crate already decodes all seven save families, but its constructed-value entry points differ from the Go save validators. See the proposal and the source-checked failure list in this change's ledger.

## Goals / Non-Goals

**Goals:** close three reproducible admission defects; make domain rules authoritative for current storage conversion; keep the public save DTO format and every version/byte contract stable.

**Non-Goals:** new schemas, disk workers, runtime restoration, live save migration, a second online authority, corpus closure or the Rust server.

## Decisions

### Preserve format DTOs and add checked domain conversion

Storage owns raw on-disk representations, including absent identity sentinels, historical triples and player armor. Domain owns current semantic validity. Preserve the public Rust save DTO field layouts in this change. In `identity.rs`, delegate UUIDv4 validation to `mornlea_domain::PlayerId::try_from_bytes`; add a checked companion-ID conversion for body ownership. In `items.rs`, keep the raw `ItemStack { item:u16, count:u8, durability:u16 }` and fixed `Inventory` shape but delegate ordinary validation, stack limit and durability to `mornlea_domain::ItemStack::try_new`, `item_stack_limit` and `durability_max`. Keep the armor array raw. In `chunk.rs`, map one bounded `ContainerSnapshot` to `mornlea_domain::PalettedSection` using its checked single/indexed/direct constructors; retain the original compact palette/words and use `block_at` for container association. Expose checked conversions at the crate root for future Rust server use:

| Producer | Exact result | Failure mapping |
| --- | --- | --- |
| `checked_player_id(raw: PlayerId)` | `StorageResult<mornlea_domain::PlayerId>` | invalid bytes to `StorageError::Corrupt` |
| `checked_companion_id(raw: PlayerId)` | `StorageResult<mornlea_domain::CompanionId>` | invalid bytes to `StorageError::Corrupt` |
| `checked_item_stack(raw: ItemStack)` | `StorageResult<mornlea_domain::ItemStack>` | invalid item/count/durability to `StorageError::Corrupt` |
| `checked_section(raw: &ContainerSnapshot)` | `StorageResult<mornlea_domain::PalettedSection>` | invalid kind/palette/words/block to `StorageError::Corrupt` |

These functions read caller values without retaining references. A later runtime maps DTOs to semantic values before publication; storage never silently repairs an invalid ordinary field. Replacing every format DTO with a domain type now would erase raw historical/armor states or force a cross-family public API rewrite. Keeping duplicate rule tables would allow save and protocol admission to drift.

### Companion legacy identity and bounded v5 preflight

Immediately after `decode_queue_sections`, set the queue ID from that same decoded body, then retain only nonempty queues. Do not use index-based post-hoc matching. In `canonical_v5_parts`, check revision, namespace, body count <=64, lifecycle count equality and queue count <=4 before any clone/sort. Validate each body/lifecycle/queue, enforce unique IDs and the exact lifecycle/body set and active membership, then sort only bounded indices or values. Preserve Go's validation order for the observable error category: revision, namespace, body count, lifecycle set, per-record validity, active count, queues, file length. Rejected output is an error with no byte vector. The existing 1,024-byte command, 5,000-step plan, 16-entry FIFO, 2,048-byte summary and 393,904-byte file ceilings remain independent.

### One complete chunk validator for every entry point

`validate_chunk` already checks section shape and values plus active container block kind, index and duplicates on decode, but its shared path must also include the existing drop-slot validation before replacing the encoder's partial loop. Make the full `validate_save` invoke this complete aggregate validator after key/revision checks. Make `encode_logical` invoke the same validator after schema admission and key/revision admission, before `ByteWriter` allocation. `encode_at_schema` and `encode` then inherit the same verdict before compression. Keep the diagnostic historical encoder fallible and prohibit a current writer from treating it as schema downgrade permission. Validation order is schema, key/revision, fixed shape, section values, drop/furnace/chest values, then active association. The exact normal wire/logical bytes remain unchanged.

### Evidence and ownership

Rust focused tests live in new topic integration files under `mornlea_storage/tests/`; the existing `runtime_contract.rs` remains a downstream consumer. Go storage packages and committed binary fixtures are read-only compatibility authorities. No test writes tracked `testdata/runtime-migration` assets. The region-bank implementation from archived `rust-region-format-completion` is a satisfied predecessor, not an editable node. Workers own only the files in their packet; the controller integrates shared exports, guides, task status and ledger serially.

## Risks / Trade-offs

- A domain constructor may reject raw historical data earlier than Go migration does -> apply it only after historical decode/migration and retain the raw DTO until then.
- Reusing section conversion may allocate bounded compact arrays -> check 24-section shape and per-section lengths before conversion; do not expand 98,304 cells.
- A test can prove only a Rust round trip -> compare negative boundaries and preserved fields with the cited Go implementation/fixtures.
- Shared exports and the monolithic existing Rust test can cause overlap -> serialize their edits and run the full storage target after every interface node.

## Migration Plan

Run the nodes in `tasks.md` dependency order; keep source fixtures and Go production untouched. Each behavior node starts with a failing Rust regression, then the minimum repair and a focused gate, then a scoped commit. Rollback reverses that node's Rust/test commit. Completion of this change enables `rust-storage-codec-closure`; it does not accept F1 or permit F2. Close with Rust format/check, six-module Go race/vet gates, audit and strict OpenSpec validation at one result SHA.
