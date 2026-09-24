# Controller review, inventory closure, and stage gates

Packets for tasks **4.1** and **4.2**. Controller-owned; not delegated as a single implementation worker.

## Node 4.1: Zero-gap inventory and architecture promotion

**Prerequisites:** tasks 3.1–3.2 accepted.

**Deliverable:** `inventory.json` fully populated; every `server.authority.*` family has executable cases; `ledger.md` lists case counts and implementation SHA.

**Procedure:**

1. Diff `testdata/runtime-migration/server/inventory.json` against registered Rust routes and manifest case IDs.
2. Run full `server_*` and `local_remote_parity` suites; record discovered vs executed counts.
3. Review `packages/engine/AGENTS.md` and `mornlea_server/AGENTS.md`; promote only verified cross-task rules into both architecture skills or record `Architecture skill: no change`.
4. `go test ./packages/audit -count=1` after any new dependency edges.

**Validation:** mechanical inventory script output stored in ledger (family count, case count, zero missing routes).

---

## Node 4.2: Stage gates and rollback evidence

**Prerequisites:** node 4.1.

**Commands (all must pass at one SHA):**

```bash
rustup run 1.97.1 cargo fmt --manifest-path packages/engine/Cargo.toml --all --check
make rust-check
make dev-check
make test-race
go test ./packages/audit -count=1
npx --yes @fission-ai/openspec@1.7.0 validate --all --strict --no-interactive
```

Record rollback drill result from node 3.2 in `ledger.md`. Failures are blockers, not exclusions. Sync specs only after gates pass.

**Publication:** use `openspec-sync-specs` then archive only when the user explicitly requests archive; planning completion does not archive F2.
