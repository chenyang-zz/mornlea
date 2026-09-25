# Change ledger

## 2026-09-20 — target architecture reconciliation

- Baseline SHA inspected: `8d9cc122486097fb7d7788abdb523ecd13f5a75a`. This entry records planning only; all implementation tasks remain open.
- Reconciled proposal, behavioral delta, design, and tasks against `docs/architecture-target.md`, current code/test ownership, and the archived pilot decision. Kept current behavior distinct from the target.
- Execution: isolated delegated planning draft, limited to this change's existing artifacts and this new append-only ledger. The controller owns integration, foundation changes, documentation, and validation of the integrated tree. No runtime source or tracked baseline was edited.
- Prerequisites now require accepted foundation ledger evidence; prospective test/tool interfaces must be created and prove nonzero discovery. No text search or zero-test success may stand in for acceptance.
- Architecture skill: no change. This round applies existing verified target ownership and validation rules; it implements no new architectural behavior.
- Planning validation: pending the isolated strict OpenSpec validation recorded below. Implementation validation and producer/default cutover approval remain pending.

## 2026-09-20 — isolated planning validation

- `openspec validate godot-desktop-audio --type change --strict --no-interactive`: passed.
- Relative Markdown link resolution against the draft and integration repository: passed.
- These are artifact checks only. Prospective runtime, release, visual-handoff, and full implementation gates have not run and no task has been marked complete.

## 2026-09-20 — integrated planning review and validation

- Integrated with F1–F3, P8–P14, the synchronized visual skill/reference, and bilingual target/visual documentation. This remains a planning-only change; every runtime task is pending.
- Shared review, orchestration decisions, exact validation commands and results are recorded in the [F1 integration ledger](../rust-runtime-foundation/ledger.md). OpenSpec strict validation passed 124 items; focused architecture/documentation/visual checks and skill validation passed.
- The full audit's existing protocol-documentation and English-comment-inventory failures reproduced on untouched baseline `8d9cc122486097fb7d7788abdb523ecd13f5a75a`; no exemptions or unrelated changes were introduced.
- Architecture skill: promoted the verified distinction between Rust semantic correctness and Godot/Python presentation evidence. Implementation-specific future facts stay in these plans and the visual handoff guide.
- No current canonical spec, runtime, tracked image, version or default entry changed; the user-owned `pr-submit` skill deletions remain excluded.

## 2026-09-25 — interface-first parallel worker decomposition

- Scope: planning artifacts only; every implementation checkbox remains open, and no new runtime, producer, image, package, default entry or version is accepted.
- Controller ruling: Cue mapping, input and device lifecycle have disjoint files, with playback after cue mapping and one serial assembly. Target reports remain independent; no-device semantic outcome persists.
- Orchestration: the controller authored and integrated the decisions after three bounded read-only repository evidence reviews of archived packet precedents, server/client seams and Godot/host seams. Shared declarations, registries, catalogs, generated reports and final integration have one serial editor; at most three disjoint implementation workers may run concurrently after accepted contract SHAs.
- Dispatch readiness: future packets require their named prerequisite implementation SHA, compiling contract, nonempty behavioral double and measured supported-case limits. A plan file alone never qualifies a worker or a runtime stage.
- Validation: `openspec validate --all --strict --no-interactive` passed 128/128; `git diff --check` passed; all 137 newly decomposed task IDs have one packet and all new relative Markdown links resolve; `go test ./packages/audit -run 'Architecture|Godot|VisualBaselineRouting' -count=1` passed. Runtime/provider/release tests remain pending because the planned crates, test targets and scripts are not implemented.
- Architecture skill: no change. The interface-first decision rule is already in the synchronized project skill; this round records downstream task execution detail, not a newly verified code/test-backed cross-task rule.
