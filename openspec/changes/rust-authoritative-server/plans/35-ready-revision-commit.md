# Ready chunk revision commit prerequisite

Controller serially owns this shared-state prerequisite of 3.9q7b. Baseline19430b0e has successful full server tests but carries compact Ready bases at their original revision. Existing per-cell overlay counters are CAS tokens, not chunk revision identities. Editable source is src/core/world.rs, src/core/state.rs and src/core/container_store.rs under packages/engine/crates/mornlea_server; add private state unit tests in state.rs. Other source, wire/save schemas, storage, providers and Go oracle inputs remain read-only. Controller owns integration, acceptance, records and rollback of these three files.

## Frozen algorithm and boundary

1. ReadyChunk retains immutable Arc<Chunk>, height cache and a private current-tick block-dirty flag. A successful changed write marks the flag; equal replacements do not. Existing preflight rejects changes when durable revision is u64::MAX. No materialization, storage decode or world I/O occurs at tick commit.
2. ReadyChunk's pending revision equals committed revision plus exactly one when block dirty OR associated drop dirty OR associated container dirty. Existing checked preflight makes overflow unreachable. Explicit replay/save materialization merges all carried block overlays and current physical drop/container slots, but computes revision from these current-tick flags, never from the presence of older carried overlays.
3. resident_snapshot clones the bounded net overlay. On its cloned Ready records, finalize pending revision once and clear block dirty; clear cloned drop dirty and reset cloned ContainerState dirty and its observation revision to the committed chunk revision. Original working context remains unchanged, so repeated snapshot extraction is stable. This is the serial reducer's existing full replacement commit boundary.
4. ResidentTickState.ready_snapshot explicitly materializes carried overlays and slots at the already committed revision. It must match the actual live logical content rather than return the initial compact base. Off-tick consumers own the materialization cost.
5. AuthorityReadView::ready_chunk_revision(key: ChunkKey) -> Option<u64> returns the exact Ready pending revision, including this tick's accepted block/drop/container work. Missing/nonReady chunks return None; sparse observations cannot establish readiness. The existing ready_chunk boolean remains available. Hostile provider consumes this query only after this compile-ready prerequisite is accepted.

## Test-first acceptance

Use private TickContext::for_tick and actual AuthorityState.advance_tick boundaries, not repeated harness contexts as cross-tick evidence. First add behavioral tests using existing resident_snapshot/ready_snapshot to show old code fails: changed blocks and containers disappear from durable resident snapshots and two later dirty ticks do not advance beyond the initial revision. Then add query tests with the implementation.

- Two writes in one tick increment once; next no-op live tick does not increment despite retained overlays. A later tick write increments again; actual materialized payload contains all previous writes. Reconstruct snapshots through ReadyChunk::try_new and exact block observations, avoiding copied packed-storage decoding.
- Equal replacement leaves revision and payload unchanged. Drop/container writes share one increment with blocks; resetting dirty flags prevents another increment next tick. Container observation revisions follow the committed chunk revision and later patches still pass exact preimages.
- Independent dimensions/chunks remain independent; sparse-only and absent keys return None. u64::MAX changed writes refuse before any cell/drop/container effects; equal block writes remain admitted. Defensive compound failure restores the new block-dirty state as part of the existing Ready map rollback.
- Existing compact hydration, fixture replay hashes, container/drop and mutation suites remain green. Enumerate ready_snapshot/snapshot_state and PathState consumers; no generated Rust artifact or hashed Go input needs regeneration.

## Gates

Source /workspace/.mornlea-env/env.sh. Run focused library Ready-commit tests, server_replay tick_state/containers/drops and server_contract mutation, then provider replay with explicit Python fixture when needed. Format only owned files; server all-target clippy --locked -- -D warnings and git diff --check. Record exact RED/GREEN and commit fix(server): commit Ready chunk revisions across live ticks before provider dispatch. Completion of this prerequisite does not close hostile revision reuse or production integration. Architecture skill: no change until acceptance.
