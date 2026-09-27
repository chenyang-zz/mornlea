# godot-default-client-switch implementation tasks

All nodes are pending. [Worker packets](plans/worker-packets.md) and [two-cycle evidence schema](plans/02-cycle-schema.md) define exact readiness, ownership, interfaces, concrete red/green cases, commands, exclusions and rollback. New targets/scripts are prospective until their owning node lands. `ledger.md` binds source/package/fixture identity and the accepted contract SHA to nonzero test counts and scoped commits. A mock, plan validation or candidate capture cannot substitute for real integration or required approval.

See the [node-by-node dependency and ownership gate](plans/03-parallel-readiness.md) before dispatch. It refines the linked packets without duplicating status.

See the [cross-change dispatch map](plans/00-cross-change-dispatch.md) for parallel lanes and serial gates.

## 1. Prerequisite and contract gate

- [ ] 1.1 Bind accepted F1–F3/P8–P13 and enumerate product/ABI consumers.
- [ ] 1.2 Land strict cycle-report contract and behavioral cutover harness.
- [ ] 1.3 Land red-first product audit replacements while preserving pilot checks.
## 2. Independent providers and serial integration

- [ ] 2.1 Implement native source/export setup diagnostics.
- [ ] 2.2 Implement transitive product closure and mixed-runtime rejection.
- [ ] 2.2b Integrate pre-import native diagnostics into the opt-in candidate launcher.
- [ ] 2.3a Build clean committed release candidate A.
- [ ] 2.3b Run complete cycle-one target, feature and hard-error tests.
- [ ] 2.3c Restore the previous release and seal cycle one.
- [ ] 2.4a Build distinct subsequent release candidate B.
- [ ] 2.4b Repeat complete cycle-two target, feature and hard-error tests.
- [ ] 2.4c Restore the previous release and seal cycle two.
## 3. Qualification, handoff or cutover

- [ ] 3.1 Apply explicitly approved default switch after both cycles.
- [ ] 3.2 Retire Bootstrap and pilot Go core from selected product closure.
- [ ] 3.3 Retire independently inventoried old renderer ABI consumers.
- [ ] 3.4 Retire independently inventoried Go real-time product edges.
## 4. Closeout

- [ ] 4.1 Reconcile D0–T1 release, retirement, guide and rollback evidence.
- [ ] 4.2 Run final complete stage gates and restore verification.
