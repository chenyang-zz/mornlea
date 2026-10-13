---
doc_id: agent-guidance-style
doc_revision: 2026-09-19.1
language: en
counterpart: agents-md-style.zh.md
---
# AGENTS.md content and presentation standard

This document defines the writing and presentation details for directory-scoped `AGENTS.md` and `CLAUDE.md` files: section structure, content criteria, and self-review steps. The rules in `docs/AGENTS.md` are principles; this document makes them executable. If they conflict, `docs/AGENTS.md` controls. The first complete precedent is the `cmd/mornlea` subtree (August 2026: one overview plus `app/`, `capture/`, and `benchmark/` package guides). The structural reference is the layered guide for `backend/packages/harness` in the deer-flow repository; its planning record under `docs/superpowers/` is historical evidence.

## Scope and layering

- Put a rule at its nearest scope. A constraint for one package belongs in that package's `AGENTS.md`; cross-package maps, directions, and entry-point differences belong in the subtree overview. Ancestor guidance accumulates, so a child file adds rules and does not restate its parent.
- Treat a directory as important when it is the repository root, a top-level module or package, a subtree root with an independent ownership, dependency, lifecycle, or validation boundary, or a directory that coordinates multiple packages, entry points, or asset classes. Each such directory should have a concise `AGENTS.md` that states its purpose, directory map, boundaries, entry points, and focused validation.
- Create or update the local `AGENTS.md` in the same change when an important directory is created, reorganized, or materially reassigned. If no independent invariant exists, inherit the parent and do not add a guide merely for symmetry.
- Keep each guide tied to decidable, scope-specific invariants rather than turning it into a general tutorial.
- Thin-import `CLAUDE.md` files exist only at the repository root and subtree roots such as `cmd/mornlea/CLAUDE.md`. Their bytes are identical and their paths are registered in `packages/audit` as `claudeImportDocs`, guarded by `TestClaudeImportsAgentGuidance`. Edit only `AGENTS.md`; do not copy its body into `CLAUDE.md`. Package directories do not contain `CLAUDE.md`: agents already read subtree and parent guidance along the ancestor chain, and nested copies only create synchronization work.

## Overview-document skeleton for a subtree root

Use these sections in order. Domain-appropriate names are allowed, but their responsibilities remain fixed:

1. **Opening scope paragraph**: state in one sentence that the file is the overview for the subtree, identify where package guides live, and explain the thin-import role of the subtree `CLAUDE.md`. See the opening of `cmd/mornlea/AGENTS.md`.
2. **Directory Map**: an ASCII tree with one `#` role comment per line. Package lines identify the corresponding `<pkg>/AGENTS.md`; asset directories such as `testdata/golden/` identify their source or destination. Verify the tree against an actual file listing before writing.
3. **Dependency Direction**: list allowed and forbidden edges together. Each direction names the enforcing test or archcheck assertion and the registration point for a new package, such as `clientCommandAllowedEdges`.
4. **Entry/Mode Differences**, when applicable: use a table for each mode's trigger and assembly behavior. List mode-independent shared boundaries separately and identify their implementing package.
5. **Documentation Sync Policy**: state which changes require which synchronized updates and explicitly say that root documents do not copy package details.
6. **Focused Verification**: provide a command table by change domain. Refer to `docs/notes/test-quickstart.md` for tiering discipline instead of copying those rules.

## Package-document skeleton

1. **Opening scope paragraph**: state what the package does, where its OpenSpec behavior specification lives, which `docs/notes/` discipline applies when relevant, and one sentence for dependency direction plus its enforcing test.
2. **Invariant sections**: one topic per section, with exact paths embedded in a heading of the form `## Topic (`<pkg>/file.go`, `<pkg>/file2.go`)`. Every invariant must be decidable and state the constraint, its basis, and its enforcement point. The enforcement point is a concrete test function or archcheck assertion; if none exists, name the review or gate that provides the fallback.
3. **Helper center and regression tests**: identify the package's sole `*_helpers_test.go` and list the regression entry points that pin behavior: real test function names plus one parenthetical sentence describing the proven property.
4. **Focused Verification**: list package-focused commands and related Make targets.

## Content criteria

- **Write invariants, not tutorials**: do not introduce a technology or provide beginner narrative. State only constraints that can be violated and the evidence used to decide them.
- **Make rules decidable**: every constraint must be checkable against code or tests. “Keep it efficient” and “watch performance” are invalid; “a missing baseline must not be created silently and requires an explicit update request” is valid.
- **Do not copy drifting values**: resolutions, capacities, thresholds, scene lists, and timeout budgets are owned by code constants and property tests. Guides name the constant and test but do not repeat the numeric value.
- **State current facts only**: change narratives, evolution history, and rejected alternatives belong in OpenSpec artifacts or `docs/superpowers/`, not in guidance.
- **Use exact paths**: verify that every file, test function, and command named in headings or prose exists. Backticked identifiers must exist. `TestCommentBacktickIdentifiersExist` covers `.go` comments; documentation references require the same standard during review.
- **Use English normative prose**: `AGENTS.md` is machine governance and uses English. Preserve exact identifiers, commands, and paths.
- **Qualify measurements by date**: a volatile count or duration uses an “observed YYYY-MM” qualifier or refers to the code/test source instead.
- **Exclude task identifiers**: do not use identifiers matching `[A-F]-[0-9]{2}`. Trace provenance through a change name or OpenSpec artifact.

## Duplication control

- State a constraint authoritatively in one location. Cross-package boundaries live in the overview and package documents link to them; package details do not flow back into the overview. Navigation pages link to authoritative prose rather than copying it.
- A rule duplicated from a parent `AGENTS.md`, even with different wording, is a drift source. Replace it with a pointer or remove it.

## Self-review checklist

- [ ] Sections follow the skeleton and every path embedded in a heading exists.
- [ ] Named test functions exist according to `go test <pkg> -list` or a source search; every command runs from the repository root.
- [ ] Numeric values either carry a dated measurement qualifier or defer to code/tests.
- [ ] Parent and child guides do not duplicate rules; cross-package constraints occur only in the overview.
- [ ] `CLAUDE.md` exists only at repository and subtree roots, is byte-identical to the template, and is registered in `claudeImportDocs`; package directories contain no `CLAUDE.md`.
- [ ] `make archcheck` passes for thin imports, backticks, and dependency guards.
- [ ] No task identifier appears; normative prose is English and identifiers keep exact spelling.

## Examples

- Subtree overview: `cmd/mornlea/AGENTS.md`, with Directory Map, Dependency Direction, Entry Modes, Documentation Sync Policy, and Focused Verification sections.
- Package guide: `cmd/mornlea/capture/AGENTS.md`, covering golden discipline, scene table, consumer interface, helper center and regression tests, and Focused Verification. `app/` and `benchmark/` follow the same form.
- For cross-package constraints such as consumer-interface discipline and test-assembly entry-point guards, see the export-surface discipline and `testkit.go` sections of `cmd/mornlea/app/AGENTS.md`.
