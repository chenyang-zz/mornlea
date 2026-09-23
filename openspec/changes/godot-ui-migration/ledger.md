# Change ledger

## 2026-09-20 — target architecture reconciliation

- Baseline SHA inspected: `8d9cc122486097fb7d7788abdb523ecd13f5a75a`. This entry records planning only; all implementation tasks remain open.
- Reconciled proposal, behavioral delta, design, and tasks against `docs/architecture-target.md`, current code/test ownership, and the archived pilot decision. Kept current behavior distinct from the target.
- Execution: isolated delegated planning draft, limited to this change's existing artifacts and this new append-only ledger. The controller owns integration, foundation changes, documentation, and validation of the integrated tree. No runtime source or tracked baseline was edited.
- Prerequisites now require accepted foundation ledger evidence; prospective test/tool interfaces must be created and prove nonzero discovery. No text search or zero-test success may stand in for acceptance.
- Architecture skill: no change. This round applies existing verified target ownership and validation rules; it implements no new architectural behavior.
- Planning validation: pending the isolated strict OpenSpec validation recorded below. Implementation validation and producer/default cutover approval remain pending.

## 2026-09-20 — isolated planning validation

- `openspec validate godot-ui-migration --type change --strict --no-interactive`: passed.
- Relative Markdown link resolution against the draft and integration repository: passed.
- These are artifact checks only. Prospective runtime, release, visual-handoff, and full implementation gates have not run and no task has been marked complete.

## 2026-09-20 — integrated planning review and validation

- Integrated with F1–F3, P8–P14, the synchronized visual skill/reference, and bilingual target/visual documentation. This remains a planning-only change; every runtime task is pending.
- Shared review, orchestration decisions, exact validation commands and results are recorded in the [F1 integration ledger](../archive/2026-09-21-rust-runtime-foundation/ledger.md). OpenSpec strict validation passed 124 items; focused architecture/documentation/visual checks and skill validation passed.
- The full audit's existing protocol-documentation and English-comment-inventory failures reproduced on untouched baseline `8d9cc122486097fb7d7788abdb523ecd13f5a75a`; no exemptions or unrelated changes were introduced.
- Architecture skill: promoted the verified distinction between Rust semantic correctness and Godot/Python presentation evidence. Implementation-specific future facts stay in these plans and the visual handoff guide.
- No current canonical spec, runtime, tracked image, version or default entry changed; the user-owned `pr-submit` skill deletions remain excluded.

## 2026-09-23 — Godot mainline container worker plan

- Planning baseline: `0a75dec671e9f41e048637642014d7256f17dff1` (`main`), isolated worktree `/Users/chen/.codex/worktrees/godot-mainline-planning/mornlea`, branch `codex/godot-mainline-worker-plan`. The user's active `codex/rust-protocol-completion` checkout and its uncommitted changes were not edited. This entry records planning only; no C-node or P10 implementation task is complete.
- User ruling: prior backlog candidates were retired and irrelevant to the Godot migration. The selected mainline slice is container UI within this active change, with Rust-owned semantic state and Godot Control/Python presentation. The existing HUD pilot and disabled `menus`/`containers` resources were inspected as transition state, not as an accepted F3 Rust contract.
- Prerequisite ruling: F1's historical artifacts now reside in [the archive](../archive/2026-09-21-rust-runtime-foundation/ledger.md); archive location alone is not full F1 exit acceptance. F2/F3 accepted producer evidence and the `mornlea_client_core` crate remain absent at this baseline. C1–C6 are prospective; the controller must reconcile the packet against accepted F3 types and SHA before implementation dispatch.
- Controller design: [container-controls.md](plans/container-controls.md) freezes the view/intent fields, token/revision and error rules, legacy command map, bounded queue, scene lifecycle, generated icons, theme, gesture routing and fixture matrix. C1 and C2 establish shared interfaces; C3 icons, C4 personal/workbench and C5 chest/furnace have disjoint files and can run concurrently after C2; C6 is serial integration. A separate required candidate catalog fails closed, while the current default pilot catalog remains unchanged. The controller owns cross-change contracts, status and ledger integration.
- Execution selection retained from the user: future implementation Workers use `gpt-6-luna` at `max` reasoning, independent reviewers use `gpt-6-sol` at `high` reasoning, and the Superpowers subagent-driven workflow governs each node. No implementation Worker was dispatched during this planning turn. A read-only Sol reviewer found and prompted corrections for empty-slot parity, accepted local transitions, required candidate startup failure, fixed Control counts, silent-session revision, bounded close, cross-area/outside drag, visual tokens and DAG alignment. Its turn ended with a model-capacity error; do not treat that as a completed approval.
- Tooling: the pinned Godot 4.7.2 executable and `scripts/godot/godot.sh` are available. No Godot-specific MCP tool is installed in this session; the planned reproducible headless checks do not require one. Adding an editor MCP later cannot replace Rust semantic replay, qualified embedded Python or P12 visual evidence.
- Validation: strict OpenSpec and link checks are rerun after final review. Runtime, headless UI, asset-generator, visual handoff and full release gates have not run, because this change creates planning artifacts only.
- Architecture skill: no change. The Rust/Godot ownership boundary is already documented in the target architecture; no new implemented cross-task rule is verified here.

## 2026-09-23 — container plan review and artifact gates

- Independent `gpt-6-sol` read-only review identified contract gaps in empty-slot parity, local-success result/revision publication, required candidate startup, scene-node counts, bounded close, drag routing, visual tokens and C3's predecessor. The controller resolved them in the linked packet and F3's producer design/task. A final targeted Sol recheck confirmed that the router sink, personal-versus-container close and `grid[1]` fixture fields are internally consistent; the review is planning evidence, not runtime approval.
- `openspec validate godot-ui-migration --type change --strict --no-interactive`: passed. `openspec validate --all --strict --no-interactive`: 127 passed, 0 failed. `git diff --check`: passed. All local Markdown links in the edited proposal, designs, tasks, ledgers and packet resolve. Historical F1 ledger links were mechanically repointed to their archived location without changing their evidence text.
- The isolated worktree contains documentation and OpenSpec artifacts only. No `mornlea_client_core`, container scene, generated PNG, test target, default profile or visual baseline was produced in this planning round. Every implementation checkbox stays open pending accepted F1/F2/F3 evidence and then node-specific red/green gates.

## 2026-09-24 — candidate panel renders and visual execution gate

- The user requested rendered targets for every styled container panel before Worker implementation, unified primary/section headings, and a character/equipment entry on the personal inventory. Four candidate images were generated for user review. They remain unapproved concept images outside the repository; they are not Godot output, P12 evidence, or a new canonical visual producer. The controller found and corrected structural issues in the first inventory candidate (extra player row and missing 2×2 crafting cells), removed an unsupported chest bulk-action button, and revised the heading treatments. No visual Worker was dispatched.
- Visual node routing changes to independent `gpt-6-sol` Workers at `max` for C2/C4/C5/C6; nonvisual C1/C3 remain `gpt-6-luna` at `max`; independent reviews remain `gpt-6-sol` at `high`. Explicit user review of each relevant target is required before a visual Worker starts. The controller must store approved images in the change and reconcile numeric tokens, layout and tests before dispatch.
- The legacy “人物” tab shows a portrait plus health/hunger, while current Godot/Rust planning defines no character/equipment destination or navigation intent. The proposed “人物与装备” card is therefore pending a read-only character/equipment view and typed navigation contract. No active dead link may be implemented. The existing container slice remains a planning-only packet; prerequisite F1/F2/F3 acceptance and all implementation gates remain open. Architecture skill: no change.
- An independent Sol plan review found that the requested title consistency lacked a shared scene owner and consumption tests. The controller assigned both heading scenes and four original title icons to C2, then added shared-theme assertions in C2 and scene-instance checks in C4/C5. This resolves the enforceable shared-heading boundary; approved numeric metrics and the character destination still await user review and later contract reconciliation.
