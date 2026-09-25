# godot-desktop-packaging implementation tasks

All nodes are pending. [Worker packets](plans/worker-packets.md) define exact readiness, ownership, interfaces, concrete red/green cases, commands, exclusions and rollback. New targets/scripts are prospective until their owning node lands. `ledger.md` binds source/package/fixture identity and the accepted contract SHA to nonzero test counts and scoped commits. A mock, plan validation or candidate capture cannot substitute for real integration or required approval.

See the [cross-change dispatch map](../godot-default-client-switch/plans/00-cross-change-dispatch.md) for parallel lanes and serial gates.

## 1. Prerequisite and contract gate

- [ ] 1.1 Inventory exact release assets, dependencies, target and backup identities.
- [ ] 1.2 Register nonempty launcher/release harness with behavioral red.
## 2. Independent providers and serial integration

- [ ] 2.1 Implement supervised Rust server child over loopback TCP.
- [ ] 2.2 Implement typed Godot launch progress and cancellation.
- [ ] 2.3 Implement relocatable package asset/export resolver.
- [ ] 2.4 Integrate strict desktop release closure harness.
## 3. Qualification, handoff or cutover

- [ ] 3.1 Qualify macOS package on macOS.
- [ ] 3.2 Qualify Windows package on Windows.
- [ ] 3.3 Qualify Linux package on Linux.
- [ ] 3.4 Prove save failure/recovery and complete previous-release restore.
- [ ] 3.5 Capture untracked candidate release evidence after P12 phase 2.
## 4. Closeout

- [ ] 4.1 Reconcile supported target/package coverage, guides and rollback.
- [ ] 4.2 Run complete implementation stage gates.
