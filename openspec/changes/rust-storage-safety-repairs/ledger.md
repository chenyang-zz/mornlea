# Planning ledger — Rust storage safety repairs

## Scope and baseline

- Planning baseline: `d929eb9560bb310b249f443e48c404aee1f6ff6e` on `dev`; initial worktree was clean. The user confirmed that the next work should begin with the remaining F1 storage safety and format successors. No production code or save asset was edited while preparing this change.
- Prerequisites: archived `rust-protocol-completion` and `rust-region-format-completion`; current protocol v45, player/chunk v9, metadata v6, companion v5, hostile v2, passive/region v1, engine ABI v11 and client ABI v19 remain fixed.
- Source order used: current code/tests, `openspec/specs/`, target architecture, then historical foundation plans. The current Go server remains authoritative; this change does not close F1 or start F2.

## Evidence and rulings

- Legacy companion v2–v4 decoding parses queue sections without assigning the containing body's ID. Node 1.1 pins distinct nonempty owners and all ordered queue fields, rather than relying on count tests.
- The current companion v5 encode path clones/sorts before checking all aggregate bounds. Go's maximum legal 64-body/four-active shape reaches exactly 393,904 bytes. A valid larger witness cannot be constructed, so nodes 1.2 and codec 4.4 test the exact accepted maximum plus independently invalid counts/text/queue fields and their precedence.
- The current chunk encode validator lacks active container association, while decode checks it. The shared `validate_chunk` also lacked the drop-slot loop; node 2.1 adds that loop before replacing the encoder path. Node 2.2 closes the public logical/historical bypass.
- Storage's current UUID/item/section rules duplicate domain logic. Keep raw format DTOs because historical sentinels and the player armor triple must remain byte-faithful; expose four checked conversions for a later Rust server. Before domain section construction, retain raw mode shape/residue checks; the domain constructor owns the semantic per-cell scan, so a chunk does not scan all cells twice.
- Nodes 3.1–3.3 are rule-source refactors. Their initial red case may be a missing checked API, but acceptance also requires full boundary matrices and an explicit duplicate-rule/call-path audit. No Worker may claim success from a compiling wrapper alone.

## Orchestration

The main controller used Superpowers brainstorming and writing-plans with the project implementation-orchestration and architecture skills. Read-only agents independently audited current code, historical storage scope and worker readiness; they did not edit files. The controller resolved contract conflicts and owns shared exports, task status, integration and rollback. The eight checkbox nodes are serialized around `companion.rs`, `chunk.rs` and `lib.rs`; each has an exact file set, expected red/green result, focused commands and scoped commit. Workers report a conflict with evidence instead of inventing validation or migration policy. Architecture skill: no change; planning has not produced a newly verified cross-task rule.

**Requirement coverage:** legacy queue owner → 1.1; bounded companion admission → 1.2; complete chunk encoder admission → 2.1/2.2; one domain rule source with raw armor/compact residue → 3.1–3.3; integrated downstream and rollback evidence → 4.1. The task DAG is serial where source ownership overlaps and has no backward dependency on codec closure.

## Planning verification

- `openspec validate rust-storage-safety-repairs --strict --no-interactive`: valid.
- `openspec validate --all --strict --no-interactive`: 128 passed, 0 failed, including both new storage changes.
- Task-link/anchor and whitespace scan: 8/8 linked nodes resolve uniquely; no trailing whitespace, tab or placeholder. Local Markdown link scan: 8 files, 0 broken links.
- `git diff --check`: exit 0 before staging; staged diff is checked again before the planning commit.

No runtime gate is claimed by this planning change. At implementation closure, node 4.1 replaces this section with actual command output, discovered/executed counts, accepted result SHA, independent review and rollback evidence.

## Implementation readiness

- Fetched `origin/dev` at `107e4db90b91d69e19e943ed4f8fb0c66209e6b3`; local `dev` matched and the worktree was clean. Implementation branch: `cursor/rust-storage-safety-repairs-90bb`. Pre-safety SHA for the node 4.1 consumer diff is `107e4db90b91d69e19e943ed4f8fb0c66209e6b3`.
- Isolation: this cloud checkout is the isolated workspace (`GIT_DIR` equals `GIT_COMMON`). No nested git worktree was created. Status stays in `tasks.md` plus this ledger; no `.superpowers/sdd` progress store.
- Execution shape: strict subagent-driven development. Implementer role is `superpowers-implementer`; review role is `superpowers-reviewer`. Nodes run serially because `companion.rs`, `chunk.rs` and `lib.rs` overlap. At most one implementer edits the tree at a time.
- Readiness: yes to packet completeness, requirement trace (1.1–4.1), signature agreement, acyclic serial order, settled rejection policy, real negative boundaries, independent node rejection, and preservation of the clean tree. Pre-flight found no plan contradiction that blocks node 1.1. `openspec` CLI is absent in this environment and will be installed at node 4.1 from the documented `@fission-ai/openspec@1.7.0` pin.
- Architecture skill: no change.

## Node 1.1

- Project subagents `superpowers-implementer` and `superpowers-reviewer` are not dispatchable in this session (Task rejects that subagent type). The user directed the controller to continue and run the node commands directly. The controller implemented node 1.1 in place and did not substitute a generic subagent.
- RED: `rustup run 1.97.1 cargo test --manifest-path packages/engine/Cargo.toml -p mornlea_storage --test safety_legacy_queue --locked` discovered 1 test and failed because `companions-v2.bin` retained a queue whose `id` was all zero while command, plan, and FIFO already matched the Go fixture.
- GREEN: the same command passed 1/1 after assigning `queue.id = body.id` before the nonempty predicate in `decode_legacy_payload`. `runtime_contract companion_` passed 6/6. `make rust` exited 0. `go test ./packages/server/storage/companion -count=1` passed.
- Review: controller self-review. The v2/v3/v4 fixtures now compare full ordered queue content, and the two v4 queues keep distinct body IDs. No fixture or Go source changed.

## Node 1.2

- RED: `safety_companion_bounds` `sixty_five_bodies_report_count_before_an_invalid_body` failed with `companion record: 0: unsupported companion dimension 9` instead of a count error, showing the body scan ran before the 64-body gate.
- GREEN: the same test target passed 6/6 after `canonical_v5_parts` checks body count, lifecycle-set length, and queue count before cloning. The maximum legal aggregate encodes to exactly 393,904 bytes. `runtime_contract companion_` passed 6/6. `go test ./packages/server/storage/companion -count=1` passed.
- Review: controller self-review. Count precedence is pinned by an invalid first body inside a 65-body request. No fixture or Go source changed.

## Node 2.1

- RED: `safety_chunk_aggregate` `current_` encoded an active furnace on air (`Ok` bytes) because `validate_save` stopped at slot shape.
- GREEN: `current_` passed 1/1 after `validate_chunk` gained the drop-slot loop and `validate_save` delegates to it after key and revision checks. `runtime_contract chunk_` passed 10/10. `go test ./packages/server/storage/chunk -count=1` passed.
- Ruling: `runtime_contract.rs` was listed read-only, but `chunk_decode_rejects_container_slots_pointing_at_the_wrong_block` required the encoder to publish the invalid aggregate. That expectation is the bypass this node closes, so the test now expects `StorageError::Corrupt` from `encode_chunk`. No fixture or Go source changed.

## Node 2.2

- RED: `safety_chunk_aggregate` `logical_` accepted schema 1 bytes for an active furnace on air.
- GREEN: `logical_` and the full `safety_chunk_aggregate` target passed 2/2 after `encode_logical` checks dimension, nonzero revision, and `validate_chunk` before allocating a writer. `runtime_contract chunk_` passed 10/10.
- Review: controller self-review. Schemas 1 through 9 reject the same invalid aggregate, including schemas that omit furnace or chest fields. No fixture or Go source changed.

## Node 3.1

- RED: `safety_domain_values` `identity_` failed to compile because `checked_player_id` and `checked_companion_id` were absent. The same test already asserted canonical bytes, zero/wrong-version/wrong-variant rejection, and storage `is_valid` equality with `mornlea_domain::PlayerId::try_from_bytes`.
- GREEN: `identity_` passed 1/1. Full `runtime_contract` passed 68/68. `cargo clippy -p mornlea_storage --all-targets --locked -- -D warnings` passed after replacing two redundant test closures.
- Audit: removed the version-nibble and variant-mask checks from `PlayerId::is_valid` in `src/identity.rs`. Those bits now exist only in `mornlea_domain::identity::is_uuid_v4`. Storage calls `PlayerId::try_from_bytes` and `CompanionId::try_from_bytes`. The all-zero hostile target stays a raw format sentinel and is not passed through `checked_player_id`.

## Node 3.2

- RED: `safety_domain_values` `stack_` failed to compile because `checked_item_stack` was absent. The test already covered the empty triple, item 1 counts 64/65, item 66, item 10 durability 0/1/131/132, nondurable durability 1, the 0..=66 boundary matrix, raw armor `(4242,65,999)`, and `u16::MAX` armor exhaustion.
- GREEN: `stack_` passed 1/1. `runtime_contract` filters `player_` 11/11, `companion_` 6/6, and `chunk_` 10/10 passed. `cargo test -p mornlea_domain --locked` passed. Storage clippy with `-D warnings` passed.
- Audit: removed the per-item stack-limit match (IDs 1..=65 split into stack-64 and stack-1 arms) and the durability match (pickaxes 131/250, swords 59/131/250, bow 120, hoes 131/250, armor 165/240/225/195) from `src/items.rs`. `item_stack_limit` and `item_max_durability` delegate to `mornlea_domain`. Retained constants are furnace wire fields (`ITEM_NONE`, coal 5, raw iron 6, ingot 7, sand 18, glass 23, brick 24, clay 27, raw beef 53, cooked beef 54), `ITEM_ID_MAX` 66, `MAX_STACK_COUNT`, and the fixed hotbar/backpack slot counts. `ITEM_STONE` and `ITEM_STONE_PICKAXE` remain `cfg(test)` names for chunk slot tests. No smelting match table remains in `items.rs`. Player armor is not passed through `checked_item_stack`.

## Node 3.3

- RED: the in-progress `checked_section` path failed to compile because `validate_chunk` matched `StorageError` without importing it. `section_checked_conversions_preserve_compact_views_and_reject_residue` was already the behavioral oracle: single air and block 89, indexed 4-bit and 8-bit compact views, direct 15-bit words, and `Corrupt` for block 90, empty/duplicate/oversized palettes, wrong word counts, out-of-range slots, high direct bits, and single/indexed/direct residue.
- GREEN: `safety_domain_values section_` passed 1/1. `safety_chunk_aggregate` passed 2/2. `runtime_contract chunk_` passed 10/10. `cargo test -p mornlea_domain --locked` passed 272 tests across 18 suites plus empty doc-tests. `cargo clippy -p mornlea_storage --all-targets --locked -- -D warnings` exited 0. `cargo fmt -p mornlea_storage -- --check` exited 0.
- Call path: `reject_section_residue` runs once before `PalettedSection::single`, `indexed`, or `direct`. Registered blocks, duplicate palette entries, palette indexes, and direct high bits stay in those constructors. `validate_chunk` converts each of the 24 section snapshots once and reads active furnace and chest blocks only through `block_at`. No 98,304-cell buffer is allocated. The local `container_block` scan and the unused `BLOCK_ID_MAX` copy were removed. `read_packed` remains `cfg(test)` for the word-layout unit test. Palette order is copied as stored.
