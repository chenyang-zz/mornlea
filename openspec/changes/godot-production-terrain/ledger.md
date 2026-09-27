# Change ledger

## 2026-09-20 — target architecture reconciliation

- Baseline SHA inspected: `8d9cc122486097fb7d7788abdb523ecd13f5a75a`. This entry records planning only; all implementation tasks remain open.
- Reconciled proposal, behavioral delta, design, and tasks against `docs/architecture-target.md`, current code/test ownership, and the archived pilot decision. Kept current behavior distinct from the target.
- Execution: isolated delegated planning draft, limited to this change's existing artifacts and this new append-only ledger. The controller owns integration, foundation changes, documentation, and validation of the integrated tree. No runtime source or tracked baseline was edited.
- Prerequisites now require accepted foundation ledger evidence; prospective test/tool interfaces must be created and prove nonzero discovery. No text search or zero-test success may stand in for acceptance.
- Architecture skill: no change. This round applies existing verified target ownership and validation rules; it implements no new architectural behavior.
- Planning validation: pending the isolated strict OpenSpec validation recorded below. Implementation validation and producer/default cutover approval remain pending.

## 2026-09-20 — isolated planning validation

- `openspec validate godot-production-terrain --type change --strict --no-interactive`: passed.
- Relative Markdown link resolution against the draft and integration repository: passed.
- These are artifact checks only. Prospective runtime, release, visual-handoff, and full implementation gates have not run and no task has been marked complete.

## 2026-09-20 — integrated planning review and validation

- Integrated with F1–F3, P8–P14, the synchronized visual skill/reference, and bilingual target/visual documentation. This remains a planning-only change; every runtime task is pending.
- Shared review, orchestration decisions, exact validation commands and results are recorded in the [F1 integration ledger](../archive/2026-09-21-rust-runtime-foundation/ledger.md). OpenSpec strict validation passed 124 items; focused architecture/documentation/visual checks and skill validation passed.
- The full audit's existing protocol-documentation and English-comment-inventory failures reproduced on untouched baseline `8d9cc122486097fb7d7788abdb523ecd13f5a75a`; no exemptions or unrelated changes were introduced.
- Architecture skill: promoted the verified distinction between Rust semantic correctness and Godot/Python presentation evidence. Implementation-specific future facts stay in these plans and the visual handoff guide.
- No current canonical spec, runtime, tracked image, version or default entry changed; the user-owned `pr-submit` skill deletions remain excluded.

## 2026-09-25 — interface-first parallel worker decomposition

- Scope: planning artifacts only; every implementation checkbox remains open, and no new runtime, producer, image, package, default entry or version is accepted.
- Controller ruling: Terrain conversion, resource pool, completion, near scene, LOD and shaders have disjoint files; the catalog and real producer-to-Godot replay are serial. Candidate visual evidence waits for P12 phase 2.
- Orchestration: the controller authored and integrated the decisions after three bounded read-only repository evidence reviews of archived packet precedents, server/client seams and Godot/host seams. Shared declarations, registries, catalogs, generated reports and final integration have one serial editor; at most three disjoint implementation workers may run concurrently after accepted contract SHAs.
- Dispatch readiness: future packets require their named prerequisite implementation SHA, compiling contract, nonempty behavioral double and measured supported-case limits. A plan file alone never qualifies a worker or a runtime stage.
- Validation: `openspec validate --all --strict --no-interactive` passed 128/128; `git diff --check` passed; all 137 newly decomposed task IDs have one packet and all new relative Markdown links resolve; `go test ./packages/audit -run 'Architecture|Godot|VisualBaselineRouting' -count=1` passed. Runtime/provider/release tests remain pending because the planned crates, test targets and scripts are not implemented.
- Architecture skill: no change. The interface-first decision rule is already in the synchronized project skill; this round records downstream task execution detail, not a newly verified code/test-backed cross-task rule.

## 2026-09-25 — second-pass worker-plan qualification

- Baseline: `4f0545df5c9ac584bfce9be0c5f1fa331c9b20f4`; this is planning only and every implementation checkbox remains open.
- Controller ruling: The live resource-pool cap is measured independently of per-frame family records. Candidate evidence uses the registered `terrain` feature key and remains untracked until P12 handoff.
- Orchestration: three bounded read-only subagent reviews independently audited server, client and Godot packets; the controller reconciled their findings against current F1 code, corrected producer/consumer contracts and retained one serial editor for shared declarations and integration. No implementation worker was dispatched.
- Validation: strict OpenSpec 128/128; all 185 revised task IDs have exactly one detailed packet; all 38 checked planning files have resolving relative Markdown links; all 80 parsed Go `-run` oracles select at least one existing test; `git diff --check` passed. These are planning checks, not prospective Rust/Godot provider or actual-host release acceptance.
- Architecture skill: no change. These are future implementation details and source-specific corrections, not newly verified cross-task code rules.

## Planning refinement, 2026-09-27

User requested more detailed downstream requirements and parallel execution using Superpowers. Controller read the installed brainstorming/writing-plans skills and retained final architecture/decomposition ownership. Three fresh GPT-6 Sol evidence agents separately inspected F2/F1, F3 and P8–P14 without editing; isolation kept source discovery out of the design context and prevented shared-file writes. This round edits planning only. Baseline `974458f0`; pre-existing `.claude/skills/mornlea-architecture/SKILL.md` and `.commandcode/` remain user-owned and excluded.

The new direct-predecessor register covers every task exactly once, and split-node packets replace their old parent tasks. At dispatch use accepted implementation SHAs and nonzero real tests, not this planning commit or an OpenSpec artifact-complete result. Complete F1 still rejects `domain.input/45`; its successor preserves real external authority ownership and adds honest rejection plus mandatory zero-gap acceptance. No runtime task was closed and no version, visual owner, default entry or release changed. Architecture skill: no change; these are prospective contracts and task-specific corrections, and the existing user-owned skill edit is preserved.


### Planning review and validation

Three independent read-only reviewers checked authority/source evidence, client contracts and Godot/release dependencies. Controller resolved stale-sequence vocabulary and zero-boundary probes; package-local deterministic persistence measurement with exact encoded-byte ownership; container versus crafting-view tokens; control-text/mining projection; transport retention; pure-core lifecycle before bridge activation; exact G1 owned facade; bounded near/far preparation/resource quotas; target preparation predecessors; and native diagnostics before either release cycle. All newly shared surfaces remain prospective until their contract owner lands compiling examples and accepted implementation SHAs. No implementation worker was dispatched.

Planning checks: `python3 /private/tmp/mornlea_check_planning.py` passed 204 pending nodes, all nine exact predecessor tables, an acyclic full cross-change DAG, 93 Markdown files and 284 resolving relative links. `openspec validate --all --strict --no-interactive` passed128/128. Focused documentation links/semantic claims, paired revisions, manifest classification, OpenSpec language/debt and baseline-version audit passed seven named tests with nonzero execution. `git diff --check` passed. These are planning validation results, not runtime provider, zero-gap foundation or actual-host release acceptance. Architecture skill: no change; stable verified cross-task rules were already recorded and no runtime architecture was implemented here.
