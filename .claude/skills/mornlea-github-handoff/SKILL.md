---
name: mornlea-github-handoff
description: Use when starting an authorized Mornlea task, preparing delivery, receiving review feedback, changing a candidate source SHA, or publishing an acceptance verdict through GitHub or a local handoff fallback.
---

# Mornlea GitHub Handoff

Use one GitHub Issue per task as the external requirements, acceptance and interaction anchor; link implementation PRs to it. Bind every handoff and independent review to a full immutable source commit SHA. GitHub records mirror the approved task; OpenSpec `tasks.md` remains the plan/status authority for OpenSpec work. Reconcile divergence before continuing.

Read [the record contract](references/record.md) before producing or interpreting a handoff. It defines the JSON block, statuses, rounds and one fictional example. Keep a short human summary beside the block: outcome, exact candidate, failed criteria or blockers, next action and evidence links. This replaces manual report copying when authorized GitHub access is available.

## Roles and authority

Each developer, reviewer and coordinator acts only within their own user authorization. A reviewer assigned diagnosis/review remains read-only with respect to product code; return a repair prompt to the authorized developer. This skill grants no implementation, commit, push, merge, deployment, GitHub publication or recurring execution permission. Use available authorized GitHub tools; do not inspect secrets, install tooling, change credentials/permissions, webhooks, Actions or scheduled reviews. Do not presume Vera has an external API or a public Page write interface. External agents have no model/provider requirement under this protocol.

## Lifecycle

1. **Start:** identify the existing Issue before proposing a new one. Freeze the original acceptance criteria, task ID, base SHA and scope reference. Reuse the existing plan/ledger. If GitHub creation/publication is unauthorized or unavailable, prepare the same copyable record locally with the blocker; never claim a remote write succeeded.
2. **Deliver:** identify the committed candidate and verify external readability of that exact SHA and required evidence. Record actual test commands, working directory, exit codes and criterion coverage. A developer may publish `ready_for_review` only when all original criteria have supporting passing evidence; this is not acceptance. Local-only commits use `local` when remote visibility is the sole blocker; an uncommitted candidate, missing evidence, unavailable access or missing publication authority uses `blocked`, while retaining the actual `source_availability`.
3. **Review:** an independent reviewer reads requirements, current candidate/round and PR head, then inspects and validates the exact `source_sha` in isolation. Read-only review may generate disposable test artifacts, never repair source. Missing evidence/identity/access is `blocked`; an observed original-criterion failure is `changes_requested`; every original criterion independently verified is `accepted`. Cite review evidence and reviewer identity.
4. **Reconcile:** before publishing a verdict, reread current candidate, round, acceptance scope and remote head. After publishing, reread them and the saved record again. If identity or scope changed, retain the old-SHA evidence as historical, record `stale`, and request a fresh handoff. A comment is not an atomic lock: consumers must reread current identity before relying on acceptance.
5. **Revise:** every new source SHA invalidates previous approval for the current candidate, including small fixes. Increment the round for a new handoff (also for a deliberate re-review of the same SHA); retries reuse the round. Evidence-only commits may remain separate only after the reviewer verifies the diff contains no candidate source/acceptance changes and that their manifest names the reviewed source. Never substitute `evidence_sha` or a moving branch for `source_sha`.
6. **Close:** on failure, create/update the corresponding repair prompt with failed criterion IDs, exact SHA, reproduction, expected behavior and evidence. Only an authorized developer executes it. Remove only that active prompt after all original criteria pass independent review for the current identity; preserve its historical record and all evidence. A new successor task has its own acceptance criteria and does not silently become a prerequisite for the original task. Acceptance does not authorize merge or Issue closure.

## Retry and concurrency

Use the task + source SHA + round key in the record contract. Before retrying a timed-out write, read existing records: reuse a matching saved event, append a genuinely new transition, or report unknown publication as `blocked`. Never blindly duplicate an acceptance or overwrite another writer's newer state. Stop repeated mutation attempts after an uncertain retry; return a copyable record and reconciliation action.

Common mistakes: declaring an unpushed SHA externally readable; accepting from developer claims alone; preserving approval after a new source commit; deleting a repair prompt after partial success; treating a posted verdict as current without rereading identity.
