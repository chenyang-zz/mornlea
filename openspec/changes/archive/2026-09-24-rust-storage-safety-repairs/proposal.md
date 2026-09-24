## Why

The completed Rust protocol contract does not make Rust save values safe to publish. Legacy companion queues lose their owner ID, constructed companion saves can clone beyond their record limit, and chunk encoders can publish active containers that the decoder rejects. The storage crate also repeats current domain validation rules.

## What Changes

- Preserve each legacy companion queue's containing body identity and its task, FIFO and summary content.
- Reject oversized or mismatched companion aggregates before proportional copying or sorting.
- Apply the complete chunk aggregate validator to every public encoder, including logical and historical-schema entry points.
- Make Rust domain constructors the single authority for current identity, ordinary stack and compact section rules while retaining raw save DTOs and the player armor fidelity exception.
- Record the already completed region-bank work as a prerequisite, not another implementation task.

## Capabilities

### New Capabilities

None.

### Modified Capabilities

- `rust-runtime-foundation`: add bounded and complete Rust save-value admission before a later server can consume storage records.

## Impact

- Affected code: `packages/engine/crates/mornlea_storage/src/{companion,chunk,identity,items,lib}.rs`, focused Rust contract tests, crate guidance, and read-only Go storage authorities.
- Compatibility: protocol v45, all seven save version sets, region format v1, engine ABI v11, client ABI v19 and benchmark scenario v23 stay fixed. No live save is opened or rewritten.
- Concurrency and performance: no online loop changes; safe Rust callers receive bounded rejection before oversized clone/sort/compression. Current Go runtime and default entry remain unchanged.
- User outcome: malformed constructed saves cannot be emitted as apparently valid Rust bytes, and legacy queue ownership survives offline decoding.
- Non-goals: disk I/O, recovery, format-wide corpus closure, new gameplay, F1 acceptance or F2 server work.
- Rollback: revert the scoped Rust and test-only commits; the Go authority and source fixtures remain untouched.
