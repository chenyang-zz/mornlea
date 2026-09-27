# godot-ui-migration implementation tasks

All nodes are pending. [Worker packets](plans/worker-packets.md) define exact readiness, files, interfaces, cases, commands, exclusions and rollback. New tests/scripts are prospective until their owning node lands. `ledger.md` records accepted prerequisite SHA, fixture identity, behavioral red/green, nonzero discovery, scoped commits and integration evidence. Shared registries, catalogs and real integration have one serial controller owner.

See the [node-by-node dependency and ownership gate](plans/03-parallel-readiness.md) before dispatch. It refines the linked packets without duplicating status.

See the [cross-change dispatch map](../godot-default-client-switch/plans/00-cross-change-dispatch.md) for parallel lanes and serial gates.

## 1. Prerequisite and contract gate

- [ ] 1.1 Inventory all supported UI surfaces, intents and states.
- [ ] 1.2 Register nonempty Rust/Godot UI harness with behavioral red.
## 2. Parallel capability slices

- [ ] 2.1 Adapt UI Controls to accepted F3 typed input and token validation.
- [ ] 2.2 Implement menus, focus and loading Controls.
- [ ] 2.3 Implement HUD and survival Controls.
- [ ] 2.4a Implement inventory Control.
- [ ] 2.4b Implement chest Control.
- [ ] 2.4c Implement workbench Control.
- [ ] 2.4d Implement furnace Control.
- [ ] 2.4e Assemble container Controls.
- [ ] 2.5a Implement chat/task Controls.
- [ ] 2.5b1 Implement validated local presentation settings Controls.
- [ ] 2.5b2 Implement read-only diagnostics/debug Controls.
## 3. Serial assembly and evidence

- [ ] 3.1 Assemble UI root and real F2/F3/G1 intent/view parity.
- [ ] 3.2 Capture untracked candidate UI evidence after P12 phase 2.
## 4. Closeout

- [ ] 4.1 Reconcile UI action coverage, guides and rollback.
- [ ] 4.2 Run complete implementation stage gates.
