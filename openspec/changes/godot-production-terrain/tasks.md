# godot-production-terrain implementation tasks

All nodes are pending. [Worker packets](plans/worker-packets.md) define exact readiness, files, interfaces, cases, commands, exclusions and rollback. New tests/scripts are prospective until their owning node lands. `ledger.md` records accepted prerequisite SHA, fixture identity, behavioral red/green, nonzero discovery, scoped commits and integration evidence. Shared registries, catalogs and real integration have one serial controller owner.

See the [node-by-node dependency and ownership gate](plans/03-parallel-readiness.md) before dispatch. It refines the linked packets without duplicating status.

See the [cross-change dispatch map](../godot-default-client-switch/plans/00-cross-change-dispatch.md) for parallel lanes and serial gates.

## 1. Prerequisite and contract gate

- [ ] 1.1 Inventory every supported terrain/material/LOD case and freeze bounds.
- [ ] 1.2 Register nonempty Rust/Godot terrain harness and behavioral red.
## 2. Parallel capability slices

- [ ] 2.1 Implement typed native bulk conversion.
- [ ] 2.2 Implement bounded Godot resource pool.
- [ ] 2.3 Implement stale-safe preparation completion.
- [ ] 2.4 Implement near-terrain Python scene consumer.
- [ ] 2.5a Implement independent terrain LOD consumer.
- [ ] 2.5b Implement material, light and fog shaders.
## 3. Serial assembly and evidence

- [ ] 3.1 Assemble real F2/F3/G1 terrain replay and enable qualified profile.
- [ ] 3.2 Capture untracked candidate visual evidence after P12 phase 2.
## 4. Closeout

- [ ] 4.1 Reconcile coverage, guides and rollback.
- [ ] 4.2 Run complete implementation stage gates.
