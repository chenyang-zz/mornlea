# Engine Test Support

`packages/engine/tests` provides shared test-only support for offline evidence and corpus verification across the foundation crates (`mornlea_domain`, `mornlea_protocol`, `mornlea_storage`).

## Invariants

- Code in this directory is test-only support. It must NEVER be added to production `[dependencies]` in any crate manifest.
- Only offline evidence and corpus fixture loading belong here. No production authority, network transports, or live game simulation may be imported.
- All file access is bounded by the harness budgets (`MAX_MANIFEST_BYTES` <= 4 MiB, `MAX_CASE_JSON_BYTES` <= 256 KiB, `MAX_BINARY_BYTES` <= 4 MiB, `MAX_CASES` <= 8192).
- Root paths supplied to `try_load_cases_from_root` are canonicalized once; relative manifest and asset paths must be lexically clean (no absolute paths, no backslashes, no `.` or `..` components).
- Every path component below the canonical root is inspected with `symlink_metadata`; symbolic links at any directory or leaf component are strictly rejected.
- Leaves must be regular files and are rechecked for containment after canonicalization.
- JSON files (manifest, JSON inputs, and expected outcomes) are decoded strictly via a custom Serde visitor that recursively rejects duplicate object keys and trailing content (`decode_strict_json`).
- Manifest validation enforces schema version 2, unique family and case IDs, case family membership, exact equality between each family's declared case list and registered cases, case version presence in family `supported_versions`, the `<family>/<version>/<label>` ID prefix format, the closed Go operation vocabulary, and nonempty decimal-u64 checkpoints.
- Case formats are restricted to `InputFormat::Binary` or `InputFormat::Json`; case consumers are restricted to the closed `CorpusConsumer` registry (`corpus_frame`, `mornlea_domain`, `mornlea_protocol`, `mornlea_storage`, `mornlea_engine`, `external:agent-contract`, `external:runtime-authority`). `mornlea_protocol` is the packet consumer the v45 packet groups register into; the framing cases stay with the separate `corpus_frame` consumer. `mornlea_storage` is the save consumer the storage evidence nodes register into once reviewed `save.*` cases land in the manifest. `mornlea_engine` is the numerical consumer the native closure registers its eleven `kernel.*` routes into: ten binary ABI families dispatched to the actual exported C symbols from the test target plus the JSON pathfinding route dispatched to the public typed pathfinder.
- SHA-256 content digests are verified for all input, expected, and encoded assets before returning loaded `FrozenCase` records.
- The Rust loader does not validate the manifest's source-provenance hashes. A Rust-only corpus pass proves asset integrity and consumer behavior, not source provenance; pair it with the Go `ReconcileWorking` inventory gate when qualifying a frozen corpus against current sources.

## Engine numerical consumer

- The `mornlea_engine` corpus routes execute in
  `mornlea_engine/tests/numerical_migration.rs` through the shared
  `runtime_corpus` loader: binary cases validate the frozen argument
  vocabulary (`abi_version`, byte `output_capacity` or mesh `u64` slots with
  `scratch_capacity`, `normal`/`short` buffer variant), initialize every
  output and scratch arena to `0xa5`, invoke the named exported symbol, and
  normalize status, whole-arena digests, success-only used-prefix digests and
  ordered float bit strings exactly as the Go producer does. An unknown
  route, argument, variant or category fails before any comparison.
- `kernel.pathfind` cases validate `operation_kind` (`grid` or `search`),
  expand Y-fast `blocks_rle` only after the checked shape and count fit
  131072 cells, build the owned grid snapshot through the public typed API,
  and normalize sorted decimal-string revisions with ordered waypoints on
  success. A copied loaded input must not mutate the owned grid. Both
  languages share the same JSON fixtures while their search implementations
  stay independent.
- Fixture provenance follows the manifest: input, expected and encoded assets
  are digest-checked on load; source-provenance hashes stay the Go inventory
  gate's job. The dispatch test asserts every loaded case executes exactly
  once with at least one `ok`, one `error` and one boundary observation per
  route, rejects unregistered routes and invalid arguments synthetically, and
  proves a mutated float bit and a mutated waypoint both fail comparison.

## Focused Verification

```bash
rustup run 1.97.1 cargo test --manifest-path packages/engine/Cargo.toml -p mornlea_domain --test corpus_loader --locked
rustup run 1.97.1 cargo test --manifest-path packages/engine/Cargo.toml -p mornlea_protocol --test runtime_contract corpus_frame --locked
rustup run 1.97.1 cargo test --manifest-path packages/engine/Cargo.toml -p mornlea_protocol --test protocol_corpus --locked
rustup run 1.97.1 cargo test --manifest-path packages/engine/Cargo.toml -p mornlea_engine --test numerical_migration --locked engine_kernel
rustup run 1.97.1 cargo test --manifest-path packages/engine/Cargo.toml -p mornlea_engine --test native_contract --locked
rustup run 1.97.1 cargo clippy --manifest-path packages/engine/Cargo.toml -p mornlea_domain -p mornlea_protocol --all-targets --locked -- -D warnings
rustup run 1.97.1 cargo fmt --manifest-path packages/engine/Cargo.toml --all --check
go test ./packages/tools/cmd/runtime-oracle -run '^TestContractInventoryReconcilesFrozenCorpus$' -count=1
git diff --exit-code -- testdata/runtime-migration
```
