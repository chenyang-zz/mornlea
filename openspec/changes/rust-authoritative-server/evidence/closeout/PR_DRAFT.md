Title: `feat(server): add rust authority library and guarded activation`

This is a local metadata draft for existing PR #5. No GitHub edit occurred.

## Summary

- Add the bounded Rust authority library, replay/provider evidence, source acquisition/publication/persistence, companion adapters and guarded single-authority activation/rollback.
- Prepare native, Go, locked Python Agent and sealed previous-runtime fixtures for required Rust CI; align loopback endpoint validation across Go patches. Protocol, save and ABI versions are unchanged.
- Retain the recorded F2 gates for gameplay-host composition, planner/shutdown integration, clock overload, complete fluid configuration, paired fixes, companion task execution and non-Overworld companions.

## Validation

- `TMPDIR=/private/tmp make ci-rust-quality` — PASS; 76 Rust summaries, 4142 passing, zero failed/ignored.
- `TMPDIR=/private/tmp make test-race` — PASS; all six Go modules, including audit; qualified Agent interpreter supplied.
- `TMPDIR=/private/tmp make dev-check` — PASS; six-module vet/short plus full Rust; qualified Agent and validated previous-runtime fixtures supplied.
- `TMPDIR=/private/tmp make companion-agent-integration` — PASS; actual server integration is nonempty.
- `GOTOOLCHAIN=go1.26.9 go test ./packages/shared/companion ./packages/shared/config -count=1` — PASS; Go 1.26.0 also passes.
- `openspec validate --all --strict --no-interactive` — PASS; 128 results.
