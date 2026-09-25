# Target runtime interface architecture ledger

## Baseline

- Repository at start: clean `dev` checkout, ahead of `origin/dev` by ten commits; no pre-existing worktree changes.
- Target selected by user: Rust core plus Godot/embedded Python presentation.
- Source hierarchy: current code/tests and canonical specs for present behavior; `docs/architecture-target.md` for migration target; F2/F3/P8–P13 active changes are plans, not implementation evidence.
- Existing Rust surfaces reviewed: `mornlea_domain` command/event identities, `mornlea_protocol` admission/framing/semantic registry, `mornlea_storage` codecs, and pilot `mornlea_godot` ABI/negotiation. Planned `mornlea_server` and `mornlea_client_core` crates are absent.

## Decisions

- Main controller owns the catalog and integration ruling. Two isolated read-only evidence explorations inspected server and client/host seams; neither edited files.
- The map freezes ownership, direction, semantic categories and failure policy. New numeric ABI IDs, full feature schemas and unimplemented queue numbers remain owned by their compile-ready landings, avoiding a false current-implementation claim.
- Architecture skill: no change. This is target design; no new verified current-code rule was discovered for skill promotion.

## Validation

- Baseline source SHA: `29fd863e2b05d7c552b1ab0a4e613ba91bc4b3cb`; this change alters documentation and planning artifacts only. Its final commit SHA is recorded by Git after commit.
- `go test ./packages/audit -run 'TestDocumentationManifest|TestCompletedDocumentationPairsAreSynchronized|TestDocumentationPairValidation|TestDocumentationLinks' -count=1`: passed after the final bilingual catalog edit; nonempty Go package suite.
- `go test ./packages/audit -count=1`: passed (`139.967s`) on this documentation unit before the last catalog clarification; the focused documentation suite above was rerun after that clarification. No source code or audit test changed.
- `openspec validate target-runtime-interface-architecture --type change --strict --no-interactive`: passed after the final catalog edit.
- `openspec validate --all --strict --no-interactive`: 128 passed, zero failed. Its last run preceded a catalog wording/coverage clarification, which did not change OpenSpec artifacts; the focused change validator was rerun afterward.
- `git diff --check`: passed after final edits. `docs/documentation-manifest.json` also parsed as JSON.
- Review result: D0/P0/S0 are existing; K0/S1–S4/C1–C2/G1/V1/T1 are target-only. The dependency direction is acyclic, the server remains the sole world writer, client epoch/revision has one owner, and the audio/lifecycle symbolic-to-numeric pilot gap is explicit. The catalog covers world, simulation, interactions, inventory, actors, companion, persistence, presentation, assets, diagnostics and release. No runtime producer, new ABI ID or new save/wire version was claimed or introduced.
- The catalog is an accepted design baseline only. Compile-ready shared contracts, real providers, and real integration remain separate gates in their owning F1–P14 changes.
