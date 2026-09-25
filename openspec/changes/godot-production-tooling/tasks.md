# godot-production-tooling implementation tasks

All nodes are pending. [Worker packets](plans/worker-packets.md) define exact readiness, ownership, interfaces, concrete red/green cases, commands, exclusions and rollback. New targets/scripts are prospective until their owning node lands. `ledger.md` binds source/package/fixture identity and the accepted contract SHA to nonzero test counts and scoped commits. A mock, plan validation or candidate capture cannot substitute for real integration or required approval.

See the [cross-change dispatch map](../godot-default-client-switch/plans/00-cross-change-dispatch.md) for parallel lanes and serial gates.

## 1. Prerequisite and contract gate

- [ ] 1.1 Inventory canonical producers and release exclusions.
- [ ] 1.2 Land the tooling-only strict run/report schema and behavioral double.
## 2. Independent providers and serial integration

- [ ] 2.1 Implement producer registry and required-case validation.
- [ ] 2.2 Implement semantic replay and no-focus candidate capture.
- [ ] 2.3 Implement strict artifact/report comparison.
- [ ] 2.4 Integrate ownership-aware visual-regression dispatcher; accept phase-2 contract.
## 3. Qualification, handoff or cutover

- [ ] 3.1 Implement transactional per-case reviewed handoff.
- [ ] 3.2 Transfer only explicitly approved cases and prove rollback.
- [ ] 3.3 Implement informational benchmark report collector.
- [ ] 3.4a Implement import resource checks.
- [ ] 3.4b Implement developer capture checks.
- [ ] 3.4c Integrate tooling CI without premature required-entry promotion.
## 4. Closeout

- [ ] 4.1 Reconcile producer/adapter coverage, guides and rollback.
- [ ] 4.2 Run complete implementation stage gates.
