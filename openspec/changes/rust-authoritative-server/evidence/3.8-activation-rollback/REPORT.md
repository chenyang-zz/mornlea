# Task 3.8: opt-in activation and rollback evidence

Acceptance comes from `plans/04-refined-nodes.md` Node 3.8. Test output is recorded in [PR #22](https://github.com/chenyang-zz/mornlea/pull/22), not in this directory.

| Criterion | Check | Result |
|---|---|---|
| Four real disposable workflows against a rebuilt Rust binary and the sealed previous Go package, with no foreground game | `persistence_failure` tests `activation::actual_activate_stop_compatible_rollback`, `actual_backup_restore`, `interrupted_each_phase`, `live_writer_and_bad_previous_identity` | 4 passed |
| Activate, compatible rollback, restore-backup rollback and dry-run inspection (dry-run alone does not qualify) | `scripts/rust-server-opt-in.sh --self-test` | passed |
| Default startup stays on Go | `activation::default_startup_paths_untouched`, also asserted by the self-test | passed |

Phases, the single-writer lock, failure codes and the absence of automatic fallback are exercised by the tests and the script above. No product code or tests changed for this task.

Fixtures: `MORNLEA_RUST_SERVER_BIN` is a release `mornlea-server` built from the tested checkout; `MORNLEA_PREVIOUS_SERVER_BIN` and `MORNLEA_PREVIOUS_PACKAGE` name the sealed previous package from source `d042982d33bb1694d768b75b01c297bd02534a08`; `TMPDIR` is a short path so Unix socket paths fit.
