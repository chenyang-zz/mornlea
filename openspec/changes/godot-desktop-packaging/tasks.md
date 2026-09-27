# godot-desktop-packaging implementation tasks

All nodes are pending. [Worker packets](plans/worker-packets.md) and [local supervision contract](plans/02-local-supervision.md) and [platform preparation packets](plans/03-platform-preparation.md) define exact readiness, ownership, interfaces, concrete red/green cases, commands, exclusions and rollback. New targets/scripts are prospective until their owning node lands. `ledger.md` binds source/package/fixture identity and the accepted contract SHA to nonzero test counts and scoped commits. A mock, plan validation or candidate capture cannot substitute for real integration or required approval.

See the [node-by-node dependency and ownership gate](plans/03-parallel-readiness.md) before dispatch. It refines the linked packets without duplicating status.

See the [cross-change dispatch map](../godot-default-client-switch/plans/00-cross-change-dispatch.md) for parallel lanes and serial gates.

## 1. Prerequisite and contract gate

- [ ] 1.1 Inventory exact release assets, dependencies, target and backup identities.
- [ ] 1.2 Register nonempty launcher/release harness with behavioral red.

## 2. Independent providers and serial integration

- [ ] 2.1a Implement generic supervised loopback Rust-server launch and lease coordination.
- [ ] 2.1b Implement Unix process-group teardown.
- [ ] 2.1c Implement Windows Job Object teardown.
- [ ] 2.2 Implement typed Godot launch progress and cancellation.
- [ ] 2.3 Implement relocatable package asset/export resolver.
- [ ] 2.3b1 Verify and prepare pinned Windows Godot editor.
- [ ] 2.3b2 Build and verify relocatable Windows embedded Python payload.
- [ ] 2.3b3 Build and verify Windows native Rust/GDExtension payload.
- [ ] 2.3c1 Verify and prepare pinned Linux Godot editor.
- [ ] 2.3c2 Build and verify relocatable Linux embedded Python payload.
- [ ] 2.3c3 Build and verify Linux native Rust/GDExtension payload.
- [ ] 2.4a Route shared build wrappers to accepted target-specific providers.
- [ ] 2.4b Integrate the Rust desktop launcher with actual supervisor and readiness providers.
- [ ] 2.4c Integrate strict package closure and identity-complete release reports.

## 3. Qualification, handoff or cutover

- [ ] 3.1 Qualify macOS package on macOS.
- [ ] 3.2 Qualify Windows package on Windows.
- [ ] 3.3 Qualify Linux package on Linux.
- [ ] 3.4 Prove save failure/recovery and complete previous-release restore.
- [ ] 3.5 Capture untracked candidate release evidence after P12 phase 2.

## 4. Closeout

- [ ] 4.1 Reconcile supported target/package coverage, guides and rollback.
- [ ] 4.2 Run complete implementation stage gates.
