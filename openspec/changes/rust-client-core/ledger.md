# rust-client-core ledger

## 2026-09-20 — target architecture planning synchronization

- Baseline: `8d9cc122486097fb7d7788abdb523ecd13f5a75a`.
- User scope: reconcile migration planning with the final architecture and improve the visual-baseline skill. This entry records planning only; no runtime task is complete.
- Ruling: add a concrete foundation change — later Godot feature plans referenced an unproposed Rust prerequisite — prevent proposal existence from being mistaken for implementation acceptance.
- Ownership: controller owns F1–F3 plans, target links and visual skills/docs; a fresh isolated agent audits and rewrites the seven later plans; a separate read-only agent forward-tests the complex visual skill. Isolation keeps multi-file discovery and review traces outside the controller's editing context.
- Directory guidance: planning artifacts and skill resources inherit root guidance; no runtime directory is created. Implementation tasks create scoped guides beside new architectural crates.
- Existing unrelated work: deleted `.codex/skills/pr-submit/SKILL.md` and `.claude/skills/pr-submit/SKILL.md` are user-owned and excluded from this change.
- Validation: pending planning integration; prospective Cargo suites have not run and do not yet exist.
- Architecture skill: review at round close; current code and canonical contracts remain the current-behavior authority.

## 2026-09-20 — integrated planning review and validation

- Integrated with F1–F3, P8–P14, the synchronized visual skill/reference, and bilingual target/visual documentation. This remains a planning-only change; every runtime task is pending.
- Shared review, orchestration decisions, exact validation commands and results are recorded in the [F1 integration ledger](../rust-runtime-foundation/ledger.md). OpenSpec strict validation passed 124 items; focused architecture/documentation/visual checks and skill validation passed.
- The full audit's existing protocol-documentation and English-comment-inventory failures reproduced on untouched baseline `8d9cc122486097fb7d7788abdb523ecd13f5a75a`; no exemptions or unrelated changes were introduced.
- Architecture skill: promoted the verified distinction between Rust semantic correctness and Godot/Python presentation evidence. Implementation-specific future facts stay in these plans and the visual handoff guide.
- No current canonical spec, runtime, tracked image, version or default entry changed; the user-owned `pr-submit` skill deletions remain excluded.

## 2026-09-25 — interface-first parallel worker decomposition

- Scope: planning artifacts only; every implementation checkbox remains open, and no new runtime, producer, image, package, default entry or version is accepted.
- Controller ruling: C1/C2 compiling contract and measured input/frame limits precede disjoint session and semantic-family providers; the atomic frame builder, G1 numeric registry, Godot host and real integration have serial controller owners. Pilot IDs 1..8 remain a separate producer table.
- Orchestration: the controller authored and integrated the decisions after three bounded read-only repository evidence reviews of archived packet precedents, server/client seams and Godot/host seams. Shared declarations, registries, catalogs, generated reports and final integration have one serial editor; at most three disjoint implementation workers may run concurrently after accepted contract SHAs.
- Dispatch readiness: future packets require their named prerequisite implementation SHA, compiling contract, nonempty behavioral double and measured supported-case limits. A plan file alone never qualifies a worker or a runtime stage.
- Validation: `openspec validate --all --strict --no-interactive` passed 128/128; `git diff --check` passed; all 137 newly decomposed task IDs have one packet and all new relative Markdown links resolve; `go test ./packages/audit -run 'Architecture|Godot|VisualBaselineRouting' -count=1` passed. Runtime/provider/release tests remain pending because the planned crates, test targets and scripts are not implemented.
- Architecture skill: no change. The interface-first decision rule is already in the synchronized project skill; this round records downstream task execution detail, not a newly verified code/test-backed cross-task rule.
