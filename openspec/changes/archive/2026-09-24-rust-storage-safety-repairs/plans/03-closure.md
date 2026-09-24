# Safety closure

<a id="node-4-1"></a>
## Node 4.1 — integrated acceptance

**Prerequisites:** 1.1–3.3; region-format archive evidence remains read-only. **Deliverable:** a reviewable result SHA with every safety requirement, downstream consumer and gate recorded. **Editable:** this change's `tasks.md` and `ledger.md`, `packages/engine/crates/mornlea_storage/AGENTS.md` only if the verified public boundaries require it. The controller owns this node; implementation workers do not mark themselves complete.

**Review matrix:** legacy v2/v3/v4 owner and FIFO; 64/65 and 4/5 companion boundaries; current/logical/schema chunk rejection; identity/item/section domain parity; raw armor and unchanged v1..v9/current format fixtures. Verify no tracked `testdata/runtime-migration` file changed, no production Go source changed, and region fixed-shape/caller-buffer behavior still passes. Search every new public export consumer in `packages/engine` and run those targets. Record each command's nonzero discovered/executed count, SHA and result; a failed gate leaves the node open.

**Commands:** `rustup run 1.97.1 cargo fmt --manifest-path packages/engine/Cargo.toml --all --check`; `rustup run 1.97.1 cargo test --manifest-path packages/engine/Cargo.toml -p mornlea_storage --tests --locked -- --list`; `rustup run 1.97.1 cargo test --manifest-path packages/engine/Cargo.toml -p mornlea_storage --tests --locked`; `make rust-check`; `make dev-check`; `make test-race`; `go test ./packages/audit -count=1`; `openspec validate --all --strict --no-interactive`; `git diff --check`; `git diff --exit-code <pre-safety-sha>..HEAD -- testdata/runtime-migration packages/server/storage`. The last command compares committed work with the recorded pre-safety baseline, not just uncommitted files. Run full gates serially to avoid test timing interference. **Commit:** `docs(openspec): record storage safety acceptance`. **Rollback:** revert the scoped safety commits; no live data rollback.

**Integration ruling:** only after all commands pass may `rust-storage-codec-closure` implementation begin. F1 remains incomplete until codec, numerical and final corpus gates pass. End-of-round architecture review records either a verified promotion or `Architecture skill: no change` in the ledger.
