# Storage contracts

`packages/engine/crates/mornlea_storage` owns versioned save records and
supported migration codecs, including standalone entity families. It is a
windowless rlib. Production code may depend only on `mornlea_domain` and must
not depend on `mornlea_protocol`, `mornlea_engine`, `mornlea_client`, or
`mornlea_godot`. Direction is enforced by `tests/runtime_contract.rs`
(`production_manifest_depends_only_on_domain` and
`domain_does_not_depend_on_storage`).

## Inventory freeze (`src/lib.rs`, `tests/runtime_contract.rs`)

- Eventual owner of every `save.*` inventory row.
- Registration tests fail if the frozen corpus drops a save family or if
  production dependencies reverse onto domain or reach protocol/kernel/host
  crates.
- Save families are ported with current-schema round-trips, supported
  migrations, and corrupt/partial rejection; this crate must not repair
  invalid records.

## Shared primitives (`src/bytes.rs`, `src/crc32c.rs`, `src/error.rs`)

- `ByteReader`/`ByteWriter` are the single little-endian byte layer for every
  family; fixed-width integers match the on-disk layout exactly.
- `SliceWriter` is the caller-buffer cursor used only after a length preflight
  proves the reserved prefix fits.
- `crc32c`/`crc32c_join` are the Castagnoli CRC-32C used by every envelope.
  They hash header slices plus payload without materializing the
  concatenation, and are public so contract tests can reseal a mutated
  fixture.
- `StorageError::Corrupt` and `StorageError::FutureVersion` keep the two Go
  storage sentinels distinct: both reject, neither repairs.

## Ported families

Each family is one module re-exported from `src/lib.rs`, and each is verified
against the committed Go binary fixture where one exists (byte-for-byte
re-encode equality). `save.chunk` is the single exception: its acceptance is
semantic round-trip rather than re-encode equality, for the reason recorded in
the next section.

| Family | Module | Current schema | Notes |
| --- | --- | --- | --- |
| `save.chunk` | `src/chunk.rs` | v9 | `CHNK` envelope over a zstd frame carrying an `MCGC` logical payload; 24 section snapshots plus fixed drop/furnace/chest arrays; v1..v9 migrate to one normalized result |
| `save.passive` | `src/passive.rs` | v1 | 32-byte header + fixed 72-byte records, 30-byte zero reserved tail, canonical ascending-ID order |
| `save.hostile` | `src/hostile.rs` | v2 | v1 records lack the trailing `kind` byte and migrate to nightcrawler; re-encode keeps each v1 record as the v2 prefix |
| `save.region` | `src/region.rs` | v1 | fixed 4096-byte superblock plus two 28672-byte banks; newest valid committed generation wins, identical ties select bank A and divergent ties fail |
| `save.world-metadata` | `src/world_metadata.rs` | v6 | v1..v6 are pure tail appends; a legacy file keeps its bytes and reads missing tails as documented defaults |
| `save.companion` | `src/companion.rs` | v5 | v1..v4 stay read-only migration input; v5 adds a 16-byte agent namespace plus per-record lifecycle mirrors and tombstones |
| `save.player` | `src/player.rs` | v9 | every schema is a tail append; decoding peels fixed tails off the end so older files keep their layout |
| player identity | `src/identity.rs` | — | `PlayerId` UUIDv4 wrapper shared by the entity families |
| item rules | `src/items.rs` | — | Wire item IDs and fixed slot counts. Ordinary stack limits and durability delegate to `mornlea_domain`; `checked_item_stack` rejects any other triple. Player armor stays a raw triple |

## `save.passive` output boundary (`src/passive.rs`)

- `passive_mobs_encoded_len` and `encode_passive_mobs_into` write the exact v1
  aggregate length into a caller buffer and preserve any tail. A short buffer
  returns `StorageError::OutputTooSmall` without writing; invalid input reports
  corruption before capacity and likewise leaves the entire buffer unchanged.

## `save.hostile` output boundary (`src/hostile.rs`)

- `hostile_mobs_encoded_len` and `encode_hostile_mobs_into` write the exact v2
  aggregate length into a caller buffer and preserve any tail. A short buffer
  returns `StorageError::OutputTooSmall` without writing; invalid input reports
  corruption before capacity and likewise leaves the entire buffer unchanged.
  An absent target is encoded only when its player id is already zero.

## `save.region` output boundary (`src/region.rs`)

- `RegionBank.entries` is a boxed array of exactly 1024 slots. Use
  `RegionBank::try_from_entries` when taking ownership of caller entries; it
  rejects wrong cardinality and noncanonical entries before encoding. Direct
  entry mutation remains possible, so every encoder validates again.
- `encode_superblock_into` and `encode_region_bank_into` write the exact v1
  length into a caller buffer and preserve any tail. A short buffer returns
  `StorageError::OutputTooSmall` without writing; an invalid bank reports
  corruption before capacity and likewise leaves the entire buffer unchanged.
  Owned-array encoders retain their existing v1 bytes through these APIs.
- The Go region codec remains the read-only format authority during migration.
  Rust decoding rejects invalid extents, reserved bytes and padding without
  repair; a zero-generation bank is standby and cannot be selected as committed.

## `save.world-metadata` output boundary (`src/world_metadata.rs`)

- `encode_world_metadata_into` and `world_metadata_encoded_len` write the exact
  v6 record length into a caller buffer and preserve any tail. A short buffer
  returns `StorageError::OutputTooSmall` without writing; invalid input reports
  corruption before capacity and likewise leaves the entire buffer unchanged.
  The codec preserves raw weather bytes; difficulty validation stays on the
  wire path.

## `save.player` output boundary (`src/player.rs`)

- `encode_into` and `player_encoded_len` write the exact v9 record length into a
  caller buffer and preserve any tail. A short buffer returns
  `StorageError::OutputTooSmall` without writing; invalid input reports
  corruption before capacity and likewise leaves the entire buffer unchanged.

## `save.chunk` compression boundary (`src/chunk.rs`)

- The Rust encoder intentionally emits different compressed bytes than the Go
  encoder for the same logical chunk. This was verified and ruled on, not left
  unresolved: the Go envelope is built by
  `github.com/klauspost/compress/zstd`, a pure-Go implementation whose
  compressed block payload differs from the reference libzstd bound here at
  every compression level, while the frame header and the trailing content
  checksum are byte-identical and the total frame length can match. A
  standalone Go program re-encodes all nine committed fixtures exactly, so the
  divergence is exclusively a Rust-versus-Go encoder difference, and no crate
  available here binds klauspost.
- Cross-implementation compatibility is therefore defined at the logical/decode
  level, because zstd frames are self-describing. Acceptance for this family is
  semantic round-trip: exact decode of every committed fixture, lossless
  encode/decode, supported-version migration convergence, and rejection without
  implicit repair.
- Do not add an assertion that a frame produced here equals a committed
  fixture's compressed bytes, and do not "fix" the encoder to chase one. The
  frame header and the trailing content checksum are pinned instead, so a real
  regression in those specific fields is still caught
  (`chunk_frame_header_and_content_checksum_match_the_reference_frame`).
- `zstd` is the only non-domain production dependency this crate may have. It
  is pinned exactly by `production_manifest_depends_only_on_domain`.
- Encoder settings mirror the Go side: one worker and the content checksum
  enabled, with the exact logical length pledged so the frame carries its
  content size. The decoder honours the 2 MiB decoded ceiling and passes an
  exact-capacity destination buffer, matching the Go `DecodeAll` call. The
  compressed ceiling is `region::MAX_COMPRESSED_CHUNK`, reused rather than
  redeclared.
- The codec keeps the Go layer split, and the intermediate layers are public so
  contract tests can prove decode exactness byte for byte and migration
  convergence without reaching into private state, following the precedent set
  by `crc32c`. `encode`/`decode` are the whole envelope, `decode_envelope` is
  the header plus frame, `encode_logical`/`decode_logical` are the `MCGC`
  payload, `chunk_logical_len` preflights the exact logical byte length for a
  current value at a target schema, and `encode_at_schema` is the encoder at
  any supported schema.
- `Chunk` deliberately carries no position. The Go `world.Chunk` carries its
  own `Pos`, so the Go encoder rejects a save whose chunk position disagrees
  with the requested key; here the key on `ChunkSave` is the single source of
  truth, so that disagreement cannot be constructed.

## Focused Verification

```bash
rustup run 1.97.1 cargo test --manifest-path packages/engine/Cargo.toml -p mornlea_storage --test runtime_contract --locked -- --list
rustup run 1.97.1 cargo test --manifest-path packages/engine/Cargo.toml -p mornlea_storage --test runtime_contract region_ --locked
rustup run 1.97.1 cargo test --manifest-path packages/engine/Cargo.toml -p mornlea_storage --test runtime_contract --locked
rustup run 1.97.1 cargo test --manifest-path packages/engine/Cargo.toml -p mornlea_storage --lib --locked
rustup run 1.97.1 cargo fmt --manifest-path packages/engine/Cargo.toml -p mornlea_storage -- --check
```
