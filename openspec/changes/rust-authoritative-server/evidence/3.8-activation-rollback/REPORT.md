# Task 3.8: opt-in activation and rollback evidence

Source commit: `123ba12fecf02339101004115cda50f10f87e944` (`cursor/rust-authoritative-server-98e6` head when this evidence was produced).
Host: see `environment.txt` (Linux x86_64, rustc/cargo 1.97.1, go 1.26.0).

## Acceptance (from `plans/04-refined-nodes.md` Node 3.8)

1. Four real disposable workflows against a rebuilt Rust binary and the sealed previous Go package, with no foreground game:
   - `activation::actual_activate_stop_compatible_rollback`
   - `activation::actual_backup_restore`
   - `activation::interrupted_each_phase`
   - `activation::live_writer_and_bad_previous_identity`
2. `scripts/rust-server-opt-in.sh --self-test` runs those real workflows plus a separate dry-run inspection; dry-run alone does not qualify rollback.
3. Default startup stays on Go (self-test asserts this; `activation::default_startup_paths_untouched` also passes).
4. Phases, single-writer lock, failure codes and no automatic fallback are exercised by the four tests and the script (see test source and script).

## Result

| Criterion | Evidence | Exit |
|---|---|---|
| Four activation tests | `activation-tests.log` — 4 passed, 0 failed | 0 |
| `--self-test` (activate, compatible rollback, restore-backup rollback, dry-run, default startup) | `self-test.log` — "SELFTEST all disposable workflows passed" | 0 |
| Default startup untouched | `default-startup.log` — 1 passed | 0 |

No code or test changes were required: the acceptance tests and script already exist on this head. This PR only records verified evidence, ticks task 3.8 and adds the ledger entry.

## Fixtures

- Rust binary: `MORNLEA_RUST_SERVER_BIN` = release `mornlea-server` built from this checkout (`sha256` in `environment.txt`).
- Previous package: sealed `d042982d33bb1694d768b75b01c297bd02534a08` at `/tmp/ethan-c38-prev/previous-runtime.json` (server/verifier/native hashes match the package record; see `environment.txt`).
- `TMPDIR=/tmp/e38` for short Unix socket paths.

## How to regenerate

```sh
cd openspec/changes/rust-authoritative-server/evidence/3.8-activation-rollback
sha256sum -c MANIFEST.sha256
bash run-evidence.sh   # requires the three MORNLEA_* fixtures named in environment.txt
```

`MANIFEST.sha256` was written last. It lists the sha256 of every file in this directory except itself.
