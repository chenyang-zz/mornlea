# Branch closeout evidence

Production candidate: `e4fd8b770ff501ed8db832fa715a10c73cf27fb4`.
Comparison baseline: `20ea8a71a33c42fafb8975b7705410e98f89fdf3`.

This round validates the merged library/activation work, repairs CI prerequisites and endpoint parity, and reconciles remaining tasks. It does not accept the complete gameplay server or authorize push, merge, cutover or archive. The existing serial worktree is used; root `dev` and its unrelated changes are preserved.

The CI runner deploys current native libraries, installs the locked Agent, prepares the sealed previous runtime through its existing owner with qualified Python, and runs the unchanged workspace gate. Go and Rust now reject prefixed/nested bracketed hosts independently of Go patch version; valid loopback addresses and HTTP request behavior remain covered. Protocol, save and ABI versions are unchanged. Scoped independent reviews and exact command results are in the [ledger](../../ledger.md#branch-closeout-ci-prerequisites-2026-10-10).

## Final local validation

All product gates exited zero on the unchanged production candidate. Closing edits only reconcile planning/evidence; the final strict OpenSpec check covers those edits separately. Short canonical `TMPDIR=/private/tmp` avoids macOS symlink ancestors and its Unix socket path limit. The default long macOS temporary root produced a 104-byte socket refusal; the same-length OS reproduction and the complete short-root rerun are retained. No restore assertions or test deadlines were weakened.

| Command | Result |
| --- | --- |
| `TMPDIR=/private/tmp make ci-rust-quality` | Native deployment, actual sealed-package preparation, fmt, Clippy and 76 Rust summaries: 4142 passing, zero failed/ignored. |
| `TMPDIR=/private/tmp make test-race` with the qualified Agent interpreter | All six Go modules pass; final audit passes. |
| `TMPDIR=/private/tmp make dev-check` with the qualified Agent/Python PATH and validated previous-package variables | Formatting, six-module vet/short tests and the complete workspace Rust gate pass. |
| `TMPDIR=/private/tmp make companion-agent-integration` | Locked install, Ruff, mypy and real server integration pass. Companion-only selector is empty and is not counted as provider acceptance. |
| `GOTOOLCHAIN=go1.26.9 go test ./packages/shared/companion ./packages/shared/config -count=1` | Both complete suites pass; default Go 1.26.0 suites also pass. |
| `openspec validate --all --strict --no-interactive` | 128 passed, zero failed. |

Raw logs remain local under `/tmp/mornlea-rust-server-closeout-final-{rust-quality,race,dev-check,agent-integration,openspec}.log`. The manifest records their source binding and digests. `dev-check` reuses the actual sealed package prepared by the earlier CI attempt at `mornlea-rust-quality.IU8wPG`; executable, verifier, current oracle and native dependency hashes were verified before use. Failed attempts remain historical evidence and are not presented as passing gates. Independent CI and endpoint scoped reviews both bind the final immutable production SHA.

## Remaining functional acceptance

| Task | Verified remaining boundary |
| --- | --- |
| 4.1 | `src/bin/mornlea-server.rs` owns activation/control and reserves the game port; it runs no ticks or gameplay sessions. Adopt accepted source acquisition/encoding, player/mob persistence, runtime configuration and trusted-observer behavior in the gameplay host. |
| 4.1 | Configured companion startup and planning/current-world projections are accepted standalone capabilities. The production planner loop and combined real Agent + durable-store shutdown remain open. `PlanHost::install` drains every queued companion against one companion-specific `CurrentWorld`; resolve that interface before dispatching planner-host integration. |
| 4.1 | Automatic fluid rescans and adoption of the complete Go-configured work ranges remain unqualified; the accepted shared per-pass rescan accounting remains accepted. |
| 4.3 | Implement the production clock loop and its four declared overload/action-order tests. |
| 4.5 | Synchronize and verify paired Go/Rust fixes on `dev` and this branch before integration/archive. This round's endpoint guard is currently branch-only. |
| 4.7 | Consolidate day arc/effective day phase in its own PR after the declared predecessors. |
| 4.8 | Complete multi-step companion execution, task deadlines and tick-boundary path grids. |
| 4.9 | Port non-Overworld companion restore, motion, publication and persistence; current typed refusal preserves stored bytes. |

`tasks.md` remains the sole status source. Earlier activation/rollback acceptance and existing library/provider receipts are preserved; they do not imply a gameplay cutover. This evidence leaf inherits the active change's ownership and validation guidance and introduces no independent source boundary.
