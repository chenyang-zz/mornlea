# Ledger: batch-gameplay-changes-before-p14

Baseline: `dev` at `0d7671d55898a91f5b799490a83026e3bda10092`, after PR #10 (archive of `require-controller-designed-worker-plans` and `align-delegation-concurrency-budget`, which also edit `development-governance` and root `AGENTS.md`), PR #11 and PR #12 merged.

Source: chen's decision via Vera and Lena's exception criteria, both 2026-10-08, as drafted by Owen. The requirement and scenario text follow Owen's draft. Lena's three blocking categories (hang, item loss, lost progress) are written in English to satisfy the OpenSpec language gate.

Ruling: the specification names the gameplay owner role rather than a person; Lena holds that role at the time of this change. No audit guard test is added because the rule is applied at review time and is not a mechanically checkable repository fact.

Validation evidence is recorded in the pull request validation section for the same source SHA.

Architecture skill: no change; this is a process rule with no new architectural decision.
