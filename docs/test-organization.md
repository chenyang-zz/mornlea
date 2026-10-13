---
doc_id: test-organization
doc_revision: 2026-09-16.1
language: en
counterpart: test-organization.zh.md
---
# Test-file organization standard

This document makes the two test-organization rules in `AGENTS.md` executable by defining decision criteria, splitting steps, helper placement, and acceptance checks. `AGENTS.md` states the principles; this document gives the procedure.

## Principles and boundaries

- **Zero behavior change**: splitting test files and moving helpers are structural changes only. Test function names and subtest labels from `t.Run` are entry points and remain unchanged. The canonical `repository-code-organization` OpenSpec requires the same property so existing `-run` filters remain compatible.
- Tests stay in the **same directory** as the code under test. `go test` recognizes only colocated `_test.go` files, and white-box assertions rely on package access. Do not create a centralized mirror under `tests/`.
- Reorganize one package directly under this document. A rollout across multiple packages is a cross-component change and requires an OpenSpec change.

## Directory and package form

- A new test file follows the package's existing form: use the package itself where white-box tests dominate and `foo_test` where external tests dominate. Existing tendencies include external black-box tests under `packages/shared/core`, `packages/shared/physics`, `packages/shared/world`, and `packages/client/mesh`; `packages/server/server` and the `contract`, `realm`, `entity`, and `runtime` subpackages under `packages/server/sim/...` tend toward white-box tests because assertions share the private state they cover.
- Do not switch between a white-box and external package merely to reorganize files. That changes visibility and exceeds a structural reorganization.

## Test-file naming

- Use `<topic>_test.go`. Derive the topic from the source filename under test (`rules.go` → `rules_test.go`) or from the property being proven.
- Prefix and suffix meanings are organizational only; `-run` and `-list` recognize function names:

| Name component | Meaning | Example |
|---|---|---|
| `property_` prefix | Proven property or decision-gate property suite | `property_rescan_test.go` |
| `_integration` suffix | Cross-component integration | `farming_integration_test.go` |
| `_restart` suffix | Restart or persistence recovery | `drop_restart_test.go` |
| `_e2e` suffix | End to end | `hunger_loop_e2e_test.go` |
| `_parity` suffix | Two-implementation comparison such as Memory/TCP or Go/Rust oracle | `parity_test.go` |
| `_oracle` suffix | Test-oracle comparison | `oracle_test.go` |
| `_fuzz` suffix | Fuzz entry point | `codec_fuzz_test.go` |
| `_bench`, `_benchmark`, or `_perf` suffix | Benchmark entry point | `bench_test.go` |
| `_external` suffix | View exclusive to the external `foo_test` package | `attached_external_test.go` |
| `_golden` suffix | Byte-for-byte golden assertion | `codec_golden_test.go` |

The prefix describes a proven property while a suffix describes a test kind. Do not stack the two meanings; for example, do not create `property_x_integration_test.go`.

## When to split

- **Mixed topics are the only hard criterion**. A file containing at least two unrelated topics is a candidate. Run `grep '^func Test' <file>` and cluster function-name prefixes; prefixes from different feature domains indicate mixing. A file is also mixed when its tests correspond to multiple source files with no shared property.
- Line count is only an inspection signal. Inspect files above roughly 800 lines for mixed topics, but do not split a large single-topic file. For example, `packages/server/sim/entity/mining_test.go` is more than one thousand lines but remains entirely about mining; the same rule applies in `packages/server/sim/runtime` and `packages/server/sim/realm`.
- Split when an otherwise-required change touches the file. Do not start a standalone task merely to split files; a bulk rollout requires OpenSpec.

## Splitting procedure

1. **Snapshot**: run `go test ./<pkg> -list '.*' | sort > /tmp/<pkg>-before.txt`.
2. **Decide helper placement**: for every declaration that is not a `Test`, `Benchmark`, or `Fuzz`, search its referencing test files with `grep -ln '<name>' <pkg>/*_test.go`. Move declarations used by more than one test file to the helper center; move a single-file private declaration with its sole consumer.
3. **Create target files and move verbatim**: create files following the naming rules. Move each test function together with its comments, private types, and constants. Do not merge, rewrite, or reorder statements.
4. **Repair self-referential comments**: if moving makes a reference such as “this file” false, name the file instead, for example “`queue_bounded_test.go` uses it as the oracle.” This is one of the few permitted comment edits.
5. **Add file-header comments**: every new file has English header comments explaining its responsibility, proven property or topic, and test-suite group. Distribute every sentence from the split file's old header among the new headers; no sentence may disappear entirely.
6. **Minimize imports**: retain only the imports each new file uses.
7. **Accept**: run the checklist below.
8. **Commit**: use one focused commit with a `test(<pkg>): ...` subject and state the split mapping.

## Helper-center rules

- Each package has at most one shared helper center. Extend an existing `*_helpers_test.go`; otherwise create `helpers_test.go`. Do not create a parallel center. The client command subpackages already have separate per-package centers: `packages/client/cmd/mornlea/app/app_test_helpers_test.go`, `packages/client/cmd/mornlea/capture/capture_test_helpers_test.go`, and `packages/client/cmd/mornlea/benchmark/benchmark_helpers_test.go`; extend those rather than adding another.
- A pure helper file containing no `Test`, `Benchmark`, or `Fuzz` function uses a `*_helpers_test.go` name, not an ordinary test filename.
- If the helper center exceeds roughly 500 lines or spans at least two unrelated domains such as test doubles, terrain fixtures, and white-box assertions, split it by domain, for example into `fixtures_test.go`.
- Helper comments use English and explain purpose. A helper shared across files identifies the scope of its consumers.

## Acceptance checklist for zero behavior change

- [ ] Every test function name is unchanged. Compare sorted `-list` snapshots as sets; declaration order naturally changes when files split and is not a regression.
- [ ] Every subtest label, the first argument to `t.Run`, is unchanged.
- [ ] Production files, `testdata/`, and goldens are unchanged; `git diff --stat` contains only `*_test.go`.
- [ ] No comment information is lost; every sentence from the split file's header appears in a new file.
- [ ] `gofmt -l <pkg>` prints nothing, `go vet ./<pkg>` passes, and `go test ./<pkg> -race -count=1` passes.
- [ ] After changing Go files, `make archcheck` passes.

## Examples

`packages/server/fluid` was the first pilot in August 2026, when it lived at `internal/fluid` in commit `fa12c56`:

| Before | After |
|---|---|
| `property_test.go` (805 lines: 340 lines of shared tools plus four properties) | `property_rescan_test.go` (168), `property_budget_test.go` (66), `property_order_test.go` (162), `property_converge_test.go` (148) |
| `memworld_test.go` (33 lines of pure helpers and no test function) | Merged into `helpers_test.go` (393 lines) |
| Cross-file helpers at the top of `queue_bounded_test.go` | `sortItems` and `queuedDueTick` moved to `helpers_test.go`; single-file `boundedPos` remained |

During the August 2026 client-command split, the `cmd/mornlea` package's 89 Go files moved through `git mv` into a thin main package plus `app/`, `capture/`, and `benchmark/` subpackages. Test function names and `t.Run` labels remained byte-identical, and the union of the three subpackages' `go test -list` entries matched the original package. The one-helper-center-per-package rule produced `app_test_helpers_test.go`, `capture_test_helpers_test.go`, and `benchmark_helpers_test.go`. Cross-package white-box assembly converged on the exported test assembly entry point in `packages/client/cmd/mornlea/app/testkit.go`. Capture goldens moved through `git mv` to `cmd/mornlea/capture/testdata/golden` and now live at `testdata/visual-golden/world`. `TestClientCommandSubpackageDependencyDirections` in `packages/audit` enforces subpackage dependency direction.

As a mixed-topic example, the former single-package `cmd/mornlea/app_input_test.go` was about 1,300 lines with 38 tests spanning prediction gates, mining overlay, hotbar placement, furnace UI, chest UI, crafting, and drop/eat/use-key behavior. In August 2026 it was split into `app_input_prediction_test.go`, `app_mining_overlay_test.go`, `app_hotbar_placement_test.go`, `app_furnace_ui_test.go`, `app_chest_ui_test.go`, `app_inventory_crafting_test.go`, and `app_use_key_test.go`, now all under `packages/client/cmd/mornlea/app/`. Shared message/mirror helpers moved to `app_test_helpers_test.go`. This is an identification example; new candidates still follow the criteria above.

## Rust mapping

The preceding criteria, procedure, and checklist use Go terminology. Rust crates `packages/engine/crates/mornlea_engine` and `packages/engine/crates/mornlea_client` apply the same principles: zero behavior change, topic-based splitting, and helper placement only after searching consumers.

### Placement and split form

- Tests remain in `#[cfg(test)]` submodules in the same crate and module tree as the code under test, either inline `mod tests` or sibling files. Do not create a centralized `tests/` integration directory. This mirrors the Go rule and preserves the current form.
- Split a mixed-topic giant inline `mod tests` into sibling files. First turn the source into a directory module, such as `greedy.rs` → `greedy/mod.rs`, then mount `#[cfg(test)] mod <topic>_tests;` at the module root. Move each test with its doc comments and private constants verbatim, and minimize each topic's imports; unused imports fail under clippy `-D warnings`. Existing examples are `packages/engine/crates/mornlea_engine/src/greedy/` from the August 2026 Rust pilot in commit `b2a6edb`, and `packages/engine/crates/mornlea_client/src/render/water_tests.rs`, which follows the same form as `render/plant_tests.rs` and is mounted from `render/mod.rs`.
- Conventionally, `#[cfg(test)] mod …;` declarations form one block at the end of `mod.rs`, as in `greedy/mod.rs`. The interleaving of test mounts and production `pub mod` declarations in client `render/mod.rs` is historical and not the pattern for new splits.

### Helper centers

- Follow existing precedents instead of creating parallel centers. There are two granularities:
  - Crate-wide sharing uses `#[cfg(test)] pub(crate) mod tests`, as in the engine crate's `src/input.rs`. Tests in `light.rs` and `ffi.rs` import `crate::input::tests::valid_input` and `crate::input::tests::ENTRY_BYTES` by name.
  - Sharing across multiple test files within one module uses `test_support.rs`, as in `src/greedy/test_support.rs`, mounted by `#[cfg(test)] mod test_support;`. Topic files use named imports such as `use super::test_support::{…}`.
- Search all consumers before placement. Only a helper used by more than one test module moves to a center; a single-module helper stays with its test file.
- Apply the same size signal as Go. Inspect and split by domain above roughly 500 lines or when at least two unrelated domains are present. Current engine `src/greedy/test_support.rs` is 63 lines and the `tests` module in `src/input.rs` is about 190 lines, well below that signal.
- Helper doc comments (`///`) use English and explain purpose. Cross-module helpers identify their consumer scope.

### Equivalent Rust acceptance checklist

- [ ] Every `#[test]` function name is byte-identical; Rust has no counterpart to a Go subtest label.
- [ ] The bare function-name set from `cargo test -p <crate> -- --list` is unchanged. Module prefixes naturally change when sibling files are mounted, which is expected and equivalent to Go `-list` set semantics.
- [ ] `cargo fmt --check` and `cargo clippy --workspace --all-targets -- -D warnings` pass, and `cargo test -p <crate> --locked` passes. Root `make rust-check` covers all three.
- [ ] cdylib exports are unchanged; run root `make rust`.

From `packages/engine/`, where `rust-toolchain.toml` pins Rust 1.97.1, extract a sorted bare-name snapshot by filtering the summary, removing the `: test` suffix, and selecting the final `::` component:

```bash
cargo test -p mornlea_engine -- --list \
  | grep ': test$' | sed 's/: test$//' | awk -F'::' '{print $NF}' | sort
```

### Notes

- Do not reorganize the test module that pins the C ABI contract by topic. The `#[cfg(test)] mod tests` in engine `src/ffi.rs` follows ABI exports rather than feature topics, so the topic-splitting procedure does not apply.
