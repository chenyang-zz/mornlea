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
- Case formats are restricted to `InputFormat::Binary` or `InputFormat::Json`; case consumers are restricted to the closed `CorpusConsumer` registry (`corpus_frame`, `mornlea_domain`, `mornlea_protocol`, `mornlea_storage`, `external:agent-contract`, `external:runtime-authority`). `mornlea_protocol` is the packet consumer the v45 packet groups register into; the framing cases stay with the separate `corpus_frame` consumer. `mornlea_storage` is the save consumer the storage evidence nodes register into once reviewed `save.*` cases land in the manifest.
- SHA-256 content digests are verified for all input, expected, and encoded assets before returning loaded `FrozenCase` records.
- The Rust loader does not validate the manifest's source-provenance hashes. A Rust-only corpus pass proves asset integrity and consumer behavior, not source provenance; pair it with the Go `ReconcileWorking` inventory gate when qualifying a frozen corpus against current sources.

## Focused Verification

```bash
rustup run 1.97.1 cargo test --manifest-path packages/engine/Cargo.toml -p mornlea_domain --test corpus_loader --locked
rustup run 1.97.1 cargo test --manifest-path packages/engine/Cargo.toml -p mornlea_protocol --test runtime_contract corpus_frame --locked
rustup run 1.97.1 cargo test --manifest-path packages/engine/Cargo.toml -p mornlea_protocol --test protocol_corpus --locked
rustup run 1.97.1 cargo clippy --manifest-path packages/engine/Cargo.toml -p mornlea_domain -p mornlea_protocol --all-targets --locked -- -D warnings
rustup run 1.97.1 cargo fmt --manifest-path packages/engine/Cargo.toml --all --check
go test ./packages/tools/cmd/runtime-oracle -run '^TestContractInventoryReconcilesFrozenCorpus$' -count=1
git diff --exit-code -- testdata/runtime-migration
```
