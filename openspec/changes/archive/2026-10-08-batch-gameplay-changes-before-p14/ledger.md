# Ledger: batch-gameplay-changes-before-p14

Baseline: `dev` at `0d7671d55898a91f5b799490a83026e3bda10092`, after PR #10 (archive of `require-controller-designed-worker-plans` and `align-delegation-concurrency-budget`, which also edit `development-governance` and root `AGENTS.md`), PR #11 and PR #12 merged.

Source: chen's decision via Vera and Lena's exception criteria, both 2026-10-08, as drafted by Owen. The requirement and scenario text follow Owen's draft. Lena's three blocking categories (hang, item loss, lost progress) are written in English to satisfy the OpenSpec language gate.

Ruling: the specification names the gameplay owner role rather than a person; Lena holds that role at the time of this change. No audit guard test is added because the rule is applied at review time and is not a mechanically checkable repository fact.

Validation evidence is recorded in the pull request validation section for the same source SHA.

Architecture skill: no change; this is a process rule with no new architectural decision.

## Review fixes, 2026-10-08

Owen's review of PR #15:

- End condition: the requirement, its end-of-rule scenario, the proposal and the root `AGENTS.md` pointer now name the real event: the Go real-time authority is retired and gameplay no longer needs to stay in parity with Go. P14 (`godot-default-client-switch`) is kept as a parenthetical, because `docs/notes/godot-client-pilot-baseline.md` defines P14 as the default-client switch plus retirement of the Go real-time runtime and client ABI. The requirement title changes from "until the P14 authority switch" to "until the Go real-time authority is retired". The change directory name keeps its original `before-p14` suffix.
- Owner: the specification keeps the "gameplay owner" role without a name; Lena is named only in the proposal and this ledger.
- Style: the four scenarios now use GIVEN/WHEN/THEN steps with SHALL, MUST, MUST NOT or MAY, matching the dominant style of `development-governance` (37 of the 39 pre-existing scenarios use GIVEN). The two pre-existing scenarios without GIVEN belong to other requirements and are unchanged.
- Because the change is archived in the same pull request, the archived delta and the canonical specification were updated together and carry identical requirement text.

Architecture skill: no change.
