# Final F1 acceptance implementation plan

Planning only; all nodes remain pending. [Exact packets and evidence](plans/00-acceptance.md) define editable/read-only files, test-first steps, commands and rollback. This change closes the remaining `domain.input/45` rejection gap and seals F1, while F2 still owns real Rust authority parity. Only this file tracks status.

## 1. Real rejection and corpus integrity

- [ ] 1.1 Add mandatory complete-acceptance and mutation tests in runtime-oracle; capture the actual domain.input/45 failure.
- [ ] 1.2 Execute the real Go authority's stale-sequence no-effect case and export source-bound rejection evidence.
- [ ] 1.3 Integrate the reviewed corpus/source-binding candidate and reconcile exact affected consumer partitions.

## 2. Integrated acceptance and downstream seal

- [ ] 2.1 Execute every required Rust/Go corpus consumer and full stage gates on the integrated source.
- [ ] 2.2 Seal zero-gap foundation evidence and bind F2/F3 prerequisites without starting their runtime implementation.
