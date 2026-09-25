## Why

After storage safety repairs, Rust can still lack complete independent evidence for every supported save version. The seven save families currently have zero executed cases in the frozen runtime-migration corpus, and most Rust writers lack bounded caller-buffer APIs. Storage must close before the integrated F1 contract gate and Rust server work.

## What Changes

- Execute Go-produced, Rust-consumed decode, encode, migration and failure cases for all seven save families and all supported versions.
- Add atomic caller-buffer writers to player, metadata, hostile, passive and companion codecs; add a reusable bounded chunk compression context.
- Preserve raw metadata weather bytes and all documented historical defaults.
- Reuse the completed region-bank format implementation and add independent corpus/selection evidence for it.
- Require closed source-bound case coverage and mutation checks before declaring storage complete.

## Capabilities

### New Capabilities

None.

### Modified Capabilities

- `rust-runtime-foundation`: add executable complete Rust save-codec parity and bounded output contracts.

## Impact

- Affected: `packages/engine/crates/mornlea_storage/`, Go test-only producers in `packages/tools/cmd/runtime-oracle/` and package-local metadata/chunk tests, `testdata/runtime-migration/`, corpus consumers and scoped guides.
- Compatibility: chunk/player v9, metadata v6, companions v5, hostile v2, passive v1, region v1, protocol v45 and ABIs remain unchanged. No live save or production Go writer changes.
- Concurrency/performance: reusable chunk scratch belongs to one caller and is never shared concurrently; noncompressed writers validate and size before writing. Timing and allocation measurements are informational, while overflow, data loss and I/O errors fail.
- User outcome: an offline Rust server prerequisite can reject malformed or future data and reproduce Go save observations without a dual writer.
- Non-goals: disk scheduling, atomic rename, crash recovery, online authority, F1 numerical work, default startup or gameplay behavior.
- Rollback: revert the isolated Rust codec and test-only corpus commits; source fixtures and live saves remain untouched.
