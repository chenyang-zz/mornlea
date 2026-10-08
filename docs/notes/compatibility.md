---
doc_id: compatibility-guide
doc_revision: 2026-10-08.1
language: en
counterpart: compatibility.zh.md
---
# Compatibility and upgrades

[中文版](compatibility.zh.md). This document expands the README "compatibility and upgrades" section. It defines the version contracts for the wire protocol and for world, player, companion, and animal saves, the backup and rollback discipline around upgrades, and benchmark report compatibility rules. Every statement follows current code and `openspec/specs/`.

Current version numbers are maintained only in the "Current version matrix" below; other explanatory documents link here instead of restating versions (the [LAN server guide](lan-server.md) and [player manual](gameplay.md) are legacy documents awaiting bilingual migration and switch to this link when they migrate). `TestCompatibilityMatrixMatchesSource` in `packages/audit` compares the matrix with the source constants and fails when they disagree.

## Current version matrix

| Contract | Current version | Authoritative source constant |
|---|---|---|
| protocol | v45 | `packages/shared/network/protocol/packet.go` `ProtocolVersion` |
| player schema | v9 | `packages/server/storage/player/player_codec.go` `CurrentSchema`; Rust `mornlea_storage/src/player.rs` `CURRENT_SCHEMA` |
| chunk schema | v9 | `packages/server/storage/chunk/chunk_codec.go` `currentChunkSchema`; Rust `mornlea_storage/src/chunk.rs` `CURRENT_SCHEMA` |
| world metadata | v6 | `packages/server/storage/metadata.go` `currentMetadataVersion` |
| `companions.ai` schema | v5 | `packages/server/storage/companion/companion_codec.go` `CurrentSchema`; Rust `mornlea_storage/src/companion.rs` `CURRENT_SCHEMA` |
| `hostile_mobs` schema | v2 | `packages/server/storage/hostile/hostile_codec.go` `CurrentSchema`; Rust `mornlea_storage/src/hostile.rs` `CURRENT_SCHEMA` |
| `passive_mobs` schema | v1 | `packages/server/storage/passive/passive_codec.go` `CurrentSchema`; Rust `mornlea_storage/src/passive.rs` `CURRENT_SCHEMA` |
| engine ABI | v11 | `packages/engine/include/mornlea_engine.h` `MORNLEA_ENGINE_ABI_VERSION`; Rust `mornlea_engine/src/ffi.rs` `ABI_VERSION` |
| client ABI | v19 | `packages/engine/include/mornlea_client.h` `MORNLEA_CLIENT_ABI_VERSION`; Rust `mornlea_client/src/ffi.rs` `CLIENT_ABI_VERSION` |
| benchmark scenario | v23 | `packages/client/cmd/mornlea/benchmark/benchmark.go` `scenarioVersion` |

The matrix shares its source with the root `AGENTS.md` baseline sentence and the version matrix in `openspec/config.yaml`; `TestBaselineVersionsMatchCode` guards those two. `hostile_mobs.bin`, `passive_mobs.bin`, and `companions.ai` also carry an outer envelope version (currently 1 for each) in a separate constant. The matrix records the record-layout schema version; do not conflate the two.

## Wire protocol

- The current protocol version is in the matrix above (defined by `ProtocolVersion` in `packages/shared/network/protocol` and re-exported as an alias by the root package `packages/shared/network`). Every mismatched version is rejected deterministically during the handshake, before Play; there is no version negotiation or downgrade decoding.
- Per-version semantics are owned by the header comment in `packages/shared/network/protocol/packet.go`; this document no longer restates each version.
- Clients only declare input intent and the server performs all authoritative resolution. `SaturationZero` is a transient hint bit that is never saved; saturation and exhaustion values are server-only quantities with no wire field. `DayPhaseOffset` is persisted only through world metadata, never in player saves.

### Historical: protocol v26–v34 per-version notes (text as of protocol v34)

- The wire protocol was v34 (defined by `ProtocolVersion` in `packages/shared/network/protocol`, re-exported by the root package `packages/shared/network`); every mismatched version was rejected during the handshake before Play, with no negotiation or downgrade decoding; earlier per-version semantics are in the header comment of `packages/shared/network/protocol/packet.go`.
- Recent versions only appended to existing packet tails or added messages; from v34 the `PassiveState` record stride changed from 37 to 38 as the one exception, other existing length limits were unchanged, and no `RejectReason` was added:
  - v26 added Play S→C ID 20 `PlaceBlockSucceeded(sequence)`, sent only to the placing session as a successful-placement acknowledgement;
  - v27 added Play C→S ID 14 `BoneMeal`, shaped like `TillSoil` (sequence + facing), with the target cell chosen by the authoritative ray;
  - v28 appended a 1-byte `Sprinting` intent bit after `Eating` at the tail of `PlayerInput`;
  - v29 appended a 1-byte `SaturationZero` hint bit after `Hunger` and before `WorldTimeTicks` in `PlayerState`;
  - v30 added Play S→C IDs 22/23/24 for night-walker messages `HostileSpawn`/`HostileState`/`HostileDespawn`: each is a `ServerTick` u64, a count u8, and up to 64 records in strictly ascending ID order (spawn carries ID/dimension/position/facing/health, state carries ID/position/velocity/facing/health, despawn carries only the ID), published per session view subscription;
  - v31 appended a 2-byte `DayPhaseOffset` display phase offset after `SaturationZero` and before `WorldTimeTicks` in `PlayerState` (u16, range `0..23999`, out-of-range values rejected by validation and codecs); the display phase is `(WorldTimeTicks + DayPhaseOffset) % 24000`, and the offset only shifts presentation without rewriting absolute time;
- v32 appended Play S→C ID 25 `CombatHit(ServerTick, Damage, TargetKind)`; the fixed 10-byte payload goes privately to the attacking player session only, never to the target, observers, trusted observers, or hostile attack targets;
- v33 appended Play S→C IDs 26/27/28 for passive cow messages `PassiveSpawn`/`PassiveState`/`PassiveDespawn`, shaped like the hostile messages and published per session view subscription;
- v34 appended a 1-byte `Grazing` flag after `Health` in the `PassiveState` record (u8, only 0/1 valid; stride 37 to 38); grazing is a transient presentation bit and is never saved;
- Clients only declared intent; `SaturationZero` was transient; `DayPhaseOffset` was persisted only through world metadata v3, never in player saves.

## Engine and client ABI

- The numerical engine ABI version is in the matrix above. A Go binary and `libmornlea_engine` from the same build form one release unit that must not be mixed across versions; `packages/shared/nativeabi`, the C header, and the Rust dynamic library must be replaced together, with no legacy fallback. Per-version changes are in the comment at `ABI_VERSION` in `packages/engine/crates/mornlea_engine/src/ffi.rs`.
- The graphical client ABI version is in the matrix above. `mornlea` and `libmornlea_client.dylib` from the same build form one release unit that must not be mixed across versions; the headless dedicated server does not link the client library and is unaffected by client ABI changes. The argument-free identity export `mornlea_client_abi_version()` reports only the actual library identity; it takes no expected version and performs no negotiation or client status return. Per-version changes are in the comment at `CLIENT_ABI_VERSION` in `packages/engine/crates/mornlea_client/src/ffi.rs`.
- The protocol, save schemas, and benchmark scenario do not change with either ABI; existing world and player saves load normally on a new client.

### Historical: engine ABI v10 and client ABI v12–v14 notes

- The engine ABI was v10; v10 fixed the worldgen request to the `MGW1` layout 3 fifteen-material contract (v9 had added batched fluid evaluation and rescan entries on the v8 mesh registry surface).
- The client ABI was v14 and the identity export reported 14; selected-main v13's 28 versioned exports and v14's 29 versioned exports returned `ABI_VERSION` on mismatch before reading semantic input or changing window, renderer, capture, or UI/cache state.
- The v14-only `mornlea_client_render_apply_world_updates` did not exist in the selected-main v13 library; a v14 bridge requiring that MRW1 symbol had to fail at link, load, or bind time against a v13 library.
- v13 introduced window composite capture (two-step capacity query, compact top-down BGRA8 output, and the Go `Window.Capture` bridge); v14 kept it along with the single-outstanding capture pump after poll and before render.
- v12 introduced the in-process WKWebView menu bridge: the `render_upload_ui_font` export and frame TLV tag 9 UI segment (layout v1–v4 codecs) were retired (`INVALID_ARGUMENT` on use), `ui_push_state` was added for downstream JSON state, and `render_drain_ui_events` kept its signature with a versioned JSON event envelope (0 bytes for an empty queue); these surfaces remained in v13 and v14.

## Save versions

- The current world metadata version is in the matrix above. Supported older versions migrate on load and are written as the current version at the next normal autosave or shutdown; a program that only knows older versions must deterministically reject future metadata and must not overwrite the original file. Per-version layouts are documented in the comments of `packages/server/storage/metadata.go`.
- The current player save schema is in the matrix above. Supported older versions load through the existing migration chain and are rewritten as the current version at the next normal save; future versions must be rejected deterministically and must not be overwritten. Per-version layouts are documented in the comments of `packages/server/storage/player` and `mornlea_storage/src/player.rs`.
- Chunk saves stay at schema v9: v1..v8 load through the existing migration chain, where v8→v9 is an identity migration that does not backfill water into old chunks; the accepted cost is a wet/dry seam between old and new chunks. Future versions must be rejected deterministically and must not be overwritten.
- Night walkers are written separately to `hostile_mobs.bin` in the world root; the current writer emits schema v2 (one trailing kind byte per record): a fixed header plus up to 64 fixed-length records, with CRC and per-field validation over the whole file; any corruption, out-of-range value, or invalid record rejects the whole file and is treated as no valid save (a missing file is an empty set, and a failed restore never overwrites the old file with an empty set). Schema v1 files are accepted read-only (kind 0, upgraded at the next normal save), and future versions must be rejected deterministically.
- Passive animals are written separately to `passive_mobs.bin` in the world directory at schema v1: magic `PMST`, a fixed 32-byte header plus up to 32 fixed 72-byte records in strictly ascending ID order, with CRC-32C over the identity fields and payload; any length mismatch is corruption. Runtime facts such as the flee timer and birth chunk are not saved and are re-derived on restore. The file has no earlier versions, and future versions must be rejected deterministically.
- Companion state is written separately to `companions.ai` schema v5 in the world root: v1..v4 migrate read-only and the encoder writes only v5, with a physical ceiling of 393,904 bytes; active and inactive body records total at most 64; active records may persist the current task, FIFO, and recovery mirror, inactive records keep only a deactivation tombstone, and names and the effective persona always come from the current configuration.
- Column height maps, sky light, and static block light are derived only from the authoritative block mirror; they are not written to chunk, player, or companion saves and do not enter network payloads. The procedural sky consumes only authoritative world time.

### Historical: world metadata v3, player schema v8, and `hostile_mobs` schema v1 notes

- World metadata v3 appended an 8-byte `DayPhaseOffset` (u64, range `0..23999`, normalized on load) to the v2 payload; v1/v2 worlds migrated on load with offset `0` and were written as v3 at the next save.
- Player schema v8 appended a fixed 17-byte respawn tail after hunger state (present flag, bed-foot cell 3×f32, dimension u32); v1..v7 loaded through the migration chain (v4 filled full health, v6 filled the three-layer hunger state with new-player defaults, v7 migrated to "no respawn point").
- `hostile_mobs.bin` schema v1 had no earlier versions.

## Backup and rollback

- Before upgrading, shut down normally, wait for player, companion, and world storage to flush, back up the complete world directory, and only then start the new program.
- To roll back, stop the server first and restore the complete pre-upgrade backup. There is no promise to downgrade any current matrix version of metadata, chunks, player saves, `hostile_mobs.bin`, `passive_mobs.bin`, `companions.ai`, or new items; never let an old program open an upgraded directory and keep writing.
- After an abnormal exit, player, companion, and chunk files are each atomic, but there is no cross-file transaction between them.

## Embedded default texture pack

- The embedded default pack is a Pastelcraft subset (MIT, by XradicalD, version `Pastelcraft 1.21.11 [R21]`; source, rename mapping, and per-file hashes are in `ATTRIBUTION.md`/`PROVENANCE.json` under `packages/client/assets/packs/pastelcraft/`), replacing the earlier Pixel Perfection subset (CC BY-SA 4.0). The beef icon still uses OpenGameArt `16x16 Food` (CC0), and cow hide and head remain procedural pixels.
- Slot names (`textures/<name>.png`), the `pack.json` contract (`format=1`), the 16×16/≤64KiB whole-pack rejection semantics, and procedural fallback for missing slots are unchanged; user-directory overrides still apply per slot name.
- `openspec/specs` needed no change for the reskin: the `texture-pack-loading` behavioral contract is unchanged, and only the pixel content of the default pack changed (synchronized when the owning change's delta spec took effect and was archived).

## Benchmark report compatibility

- The current benchmark producer scenario version is in the matrix above. The fixed input remains seven remote players, zero companions, and no injected chat, in a test world without water or farming blocks. A scenario version change records a change to the measured process itself (HUD fixed upload layout and reserved-surface worst case, per-frame instance prefix bytes, HUD atlas columns, stable block and mesh registry, worldgen ABI, authoritative tick workload, and similar); performance numbers are comparable only within one scenario version. Per-version reasons are in the comment at `scenarioVersion` in `packages/client/cmd/mornlea/benchmark/benchmark.go` and in the [performance baseline](perf-baseline.md).
- Reports from older scenario versions remain readable for same-version comparison. Cross-scenario comparison accepts only the single explicit migration allowed by `packages/tools/perfcheck/compare.go`, currently `--allow-scenario-upgrade 22:23`; earlier migration authorizations are retired and kept only as archive evidence.
- Cross-transport comparison requires both sides to have the same scenario version and `git_commit`; otherwise it is rejected.
- Performance numbers are recorded only and do not change the exit status; report structure, source identity, real overflow, data loss, and I/O errors remain hard failures.

### Historical: scenario v22 notes

- The benchmark producer was scenario v22 (from v22 the fixed world deterministically adds natural short grass above qualifying grass blocks).
- Cross-scenario comparison accepted only the explicit `--allow-scenario-upgrade 21:22`; the earlier `20:21` had retired when the producer moved to scenario v22.
