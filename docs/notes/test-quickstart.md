---
doc_id: test-quickstart
doc_revision: 2026-10-13.1
language: en
counterpart: test-quickstart.zh.md
---
# Test quickstart for development

[中文版](test-quickstart.zh.md).

When test execution feels slow, identify where time is spent first: the repository test suite (495 Go test files across 33 packages) is large due to the "one topic per file" rule. Execution time is concentrated in heavy client command subpackages and `packages/server/server`—namely `packages/client/cmd/mornlea/capture` (golden scene frame capture), `packages/client/cmd/mornlea/benchmark` (real renderer scenes; previously combined with app in a single package taking ~4 minutes), and `packages/server/server` (long tick integration and capacity tests). For Rust, building the full wgpu stack initially takes minutes, while incremental `cargo test -p` takes ~5 seconds. Full race tests (`make test-race`, cycling across five modules) take ~4.5 minutes locally, and high host load can trigger load flakes on timing-sensitive tests.

## Tier discipline (T0–T3)

Development time should not be spent waiting on tests—select the tier corresponding to your current stage, and **do not skip tiers**:

| Tier | When | Command | Expected time |
|---|---|---|---|
| **T0 Edit cycle** | After every change | Targeted `-run` tests on modified package (see table below), without `-race`/`-count=1`; Rust uses `cargo test -p <crate>` | Seconds |
| **T1 Task closure** | After each task or fix | `make test-race-changed` (modified packages and reverse dependencies, always including archcheck; `RACE_BASE=ref` changes comparison baseline) | Seconds–minutes |
| **T2 Pre-push** | Before pushing a branch or opening a PR | `make dev-check` + `make test-race-short` | ~2–3 minutes |
| **T3 Pre-commit / risk domains** | Touching concurrent or timing-sensitive domains, or reproducing CI failures | `scripts/agents/gates.sh` (gofmt, vet, archcheck, OpenSpec, `make rust`, and full race unless skipped) | ~5 minutes |

Key rules:

- **Never use full race with `-count=1` as a quick check**: `-race -count=1` forces full cache invalidation, taking the full 4.5 minutes across five modules every run. `-race -count=1` is reserved for T1/T2/T3; T0 intentionally omits them to leverage test caching.
- **Do not duplicate full race runs locally and on CI**: CI `go-race` runs full race partitioned across client / server / rest (contracts+shared+tools). The standard workflow is green T2 → push → trust CI; local T3 is required only when (a) changes touch concurrent or timing semantics in `packages/server/sim`/`packages/server/server`/`packages/shared/network`/`packages/server/storage`/`packages/shared/physics`, or (b) reproducing CI failures locally. When `AGENT_MODE=merge` (direct push without PR), local T3 is mandatory.
- **Flake triage protocol**: Under high host load, when timeout or budget failures occur, **rerun the failing package alone** (`go test ./<pkg> -race -count=1`); if it passes independently, record it as a load flake (in ledger or PR notes), without entering a fix cycle or modifying production code. Only file a repair task when the same flake reproduces ≥2 times in a single day.
- **Verification evidence reuse**: Outputs already recorded in a change `ledger.md` under the same baseline SHA may be referenced directly without rerunning identical commands; reviews verify with focused test spot-checks (see `docs/development-process.md` stage 3).
- **Boundaries of T1 closure**: `race-changed.sh` traces reverse dependencies along production import edges, while test imports count as one level of direct dependency; residual blind spots are covered by T3 and CI. When iterative verification touches heavy tests in `packages/client/cmd/mornlea/app`, `packages/client/cmd/mornlea/benchmark`, or `packages/server/server`, append `-short` (the capture package has no `testing.Short()` guard, where `-short` is a no-op); the script warns when the set contains heavy packages `packages/client/cmd/mornlea/app`, `packages/client/cmd/mornlea/benchmark`, and `packages/server/server`.

## T0: Targeted tests

| Domain | Quick check command |
|---|---|
| Window / rendering / materials / WGSL | `cargo test -p mornlea_client --locked` + `go test ./packages/client/cmd/mornlea/app -run 'TestFitFramebuffer\|TestApplicationConnection'` |
| Protocol / network / storage | `go test ./packages/shared/network/... ./packages/server/storage/... ./packages/shared/core` |
| Server tick / companion / farming | `go test ./packages/server/server ./packages/server/sim/... -run 'keywords'` |
| Python companion Agent | `cd packages/agent/companion && uv run pytest tests/test_<topic>.py -q`; run `uv sync --locked` on first entry |
| Go/Python Agent contracts | `go test ./packages/server/server -run 'CrossLanguage|CompanionAgent.*Integration'` (full real-process entry points below) |
| Fluid queue / rule evaluation / rescan wrapper | `go test ./packages/server/fluid` (rescan differential gate is in `./packages/server/sim/realm`, see `packages/server/fluid/AGENTS.md`) |
| Assets / texture packs / provenance | `go test ./packages/client/assets` |
| Visual goldens | Unchanged expectation: `make visual-check`; intentional change: inspect images then `make visual-update`, followed by `make visual-check` |
| Dependency boundaries / documentation consistency | `make archcheck` |

Key rules:

- `go test ./pkg` without `-count=1` hits the cache and returns immediately for unchanged packages; `-race` and `-count=1` invalidate cache and are reserved for T1 and above.
- Rust incremental builds never wipe `target/`; the Makefile sets `CARGO_TARGET_DIR=packages/engine/target/cargo` by default for `make rust`, avoiding cross-worktree artifact reuse. Direct cargo calls do not read the Makefile, so set `CARGO_TARGET_DIR` explicitly when needed. Routine work uses `cargo test -p <crate> --locked`; keep `make rust-check` for pre-commit.

## T1: race-changed

```bash
make test-race-changed             # Run race on modified closure relative to origin/main
make test-race-changed RACE_BASE=<ref>   # Change comparison baseline (e.g. shared batch SHA)
scripts/agents/race-changed.sh --diff    # Print package set without running
```

Set = modified packages (committed diff ∪ staged ∪ unstaged ∪ untracked .go) ∪ production import reverse dependencies (transitive) ∪ test import direct dependencies. When the closure contains cdylib consumer packages (packages/shared nativeabi/core/physics, packages/client mesh/client/cmd/mornlea and app/capture/benchmark subpackages, and packages/server sim/server/cmd/mornlea-server), the script builds `make rust` on demand; pure Go leaf edits skip Rust build.

Python services do not enter the Go import closure: after modifying `packages/agent/companion`, T1 additionally runs `make companion-agent-check`, which executes locked sync, Ruff format/lint, `mypy src`, and all pytest suites. When modifying HTTP/MCP wire contracts, process lifecycle, or integration helpers, run `make companion-agent-integration`; this tests `tests/integration` and starts real Go/Python loopback child processes with the race detector.

## T2: Short mode and one-key quick check

Heavy tests in `packages/client/cmd/mornlea/app`, `packages/client/cmd/mornlea/benchmark`, and `packages/server/server` (off-screen renderer scenes taking several seconds, benchmark real renderer scenes, long tick integration and capacity tests) are skipped under `testing.Short()`; the capture package has no `testing.Short()` guard where `-short` is a no-op, and heavy golden captures run via `make visual-check` as an independent gate:

```bash
make test-race-short   # Five-module loop with -race -short, ~5x faster than full race
make dev-check         # gofmt + five-module vet + short tests + Rust fmt/clippy/unit tests
make companion-agent-check        # Agent Python full quality gate
make companion-agent-integration  # Real-process contract when modifying cross-language boundaries
```

`-short` is solely an iteration tool separating speed from correctness: CI and final gates run full suites **without** `-short`, and skipping tests never relaxes correctness gates. Rule of thumb: tests taking ≥ 1.5 seconds warrant `-short`; unit-level assertions are always retained.

## T3: Full gates

```bash
scripts/agents/gates.sh     # gofmt, vet, archcheck, OpenSpec, rust, and full race
make rust-check             # Run separately before final commit; gates.sh does not include this
make companion-agent-check
make companion-agent-integration
```

`scripts/agents/gates.sh` runs gofmt, vet, archcheck, OpenSpec, and `make rust`, followed by full race unless `GATES_SKIP_RACE=1` is set; it does not include `make rust-check`, `make companion-agent-check`, or `make companion-agent-integration`, which must be run explicitly when touching those domains. Both companion entry points use deterministic fake model and temporary SQLite databases bound to loopback only, without network or external providers, and do not launch or focus game windows. When visual output is expected unchanged, run `make visual-check`; when changed, confirm images first, run `make visual-update`, and rerun `make visual-check`.

Before final commit: `make rust-check` → `make test-race` → corresponding benchmark/golden/`perfcheck` (numeric values recorded only).
