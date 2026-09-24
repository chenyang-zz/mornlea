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
