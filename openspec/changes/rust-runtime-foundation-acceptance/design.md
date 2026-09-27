## Context

Read the [proposal](proposal.md) and [acceptance specification](specs/rust-runtime-foundation-acceptance/spec.md). Numerical implementation `9b843bbc9d43678d8ef62c0f0996a120cd11d4ca` is accepted; archive metadata is at `974458f0`. Read-only diagnosis at that baseline executes the existing zero-case rejection test successfully while logging `uncovered point domain.input version 45`.

## Goals / Non-Goals

Seal the foundation's full discovered coverage set without changing real-time behavior. Keep the existing external `domain.input/45/session-sequence-arrival` case and its real Go authority observations. This does not establish Rust server behavior, start F2, alter a production algorithm, or rewrite an archived change.

## Decisions

### Honest rejection evidence

Add `domain.input/45/stale-sequence-no-effect` under the existing `order` route and `external:runtime-authority` consumer. Its input names one live session, sequence zero, arrival zero and `select_hotbar` slot seven at tick seven. The real engine starts with last admitted sequence zero and selected slot zero. Capture before/after inventory and chunk revision; execute `Engine.Step`; require selection zero, no admitted command, one discarded command, last sequence zero, zero placement successes and unchanged inventory/world revision. Normalize this observed rejection as `kind:error`, `category:stale-sequence`; retain observed fields. There is no new negative-coverage exception.

The existing successful fixture is byte-identical. Pure Rust `order_commands` continues to prove ordering, duplicate-arrival failure, short-scratch atomicity and reuse, without pretending to apply world effects. F2 later proves the authoritative effects against the retained external reference. This separation avoids a circular demand to implement F2 before accepting F1 contract evidence.

### Existing interfaces and serial ownership

Use the existing `ReconcileComplete(root string, inventory Inventory, discovered []Family, live Identities, consumers ConsumerRegistry, negativeExceptions NegativeCoverageExceptions) (CoverageReport, error)`. The new mandatory test requires `err == nil`, `len(report.Uncovered) == 0`, nonempty `Covered`, and set equality with every discovered supported family/version. Keep the existing synthetic rejection tests. The CLI remains flag-free and read-only.

One controller owns manifest publication, source-revision identity, route registry, test registrations, ledger and downstream prerequisite update. The producer task owns only its test file and source-bound draft fixtures. No new shared production boundary qualifies for an interface landing: existing envelope/order/reconciliation APIs are stable. The manifest is a serial shared boundary. Provider work finishes before its publication and full acceptance; this small critical chain is deliberately serial.

### Source and derived evidence

Input family bindings include the existing protocol and engine-step sources plus `packages/server/sim/runtime/command_order_oracle_test.go`, `packages/engine/crates/mornlea_domain/src/input/order.rs`, `packages/engine/crates/mornlea_domain/tests/command_order.rs`, and the changed Rust corpus exclusion checks if their expected partition count changes. Hash actual files with the required `sha256:` prefix. Corpus merge preserves every baseline case byte/hash, unions only the new case and sources, sorts IDs, reloads, and reconciles. Update exact partition-count assertions in the named Go/Rust corpus tests by baseline count plus one external case; Rust domain case count stays unchanged. Producer expected bytes come from real Go execution. Candidate export uses the existing create-exclusive external-directory protocol; normal test runs never write tracked testdata.

## Risks / Trade-offs

- A stale command is silently discarded rather than receiving a wire rejection → record the observed discard as offline error classification; do not invent a wire packet.
- A passing generic test can hide non-kernel gaps → the new mandatory test fails on any complete-reconciliation error.
- F1 external authority evidence could be overclaimed → ledger explicitly states that F2 real Rust authority parity remains unaccepted.
- Source refresh can invalidate unrelated evidence → compare unaffected fixture bytes and run every real corpus consumer on the integrated baseline.

## Migration Plan

Run the mandatory failing gate, add the real rejection producer, integrate its reviewed corpus candidate, execute all existing consumers, then seal prerequisite evidence for F2/F3. The controller records the machine-readable seal beside the ledger and refreshes the downstream readiness/index and paired target-interface prerequisite facts; their runtime tasks stay pending. Every verified node has one scoped commit. Rollback reverts new test/fixture/source-binding changes together and clears the new seal; accepted numerical implementation and archived records remain intact. Full stage gates include formatting, `make rust`, `make rust-check`, `make dev-check`, `make test-race`, audit and strict OpenSpec validation. Timing values stay informational.
