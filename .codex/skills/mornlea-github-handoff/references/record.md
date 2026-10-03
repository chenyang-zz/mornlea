# Handoff record contract (version 1)

Place one JSON object in a fenced `json` block following the exact marker
`<!-- mornlea-handoff:v1 -->` in an Issue/PR comment or a local Markdown record.
Keep the same shape in both transports. An authorized PR body still follows the
repository PR template and links this record and its task Issue.

## Fields and interpretation

| Field | Contract |
|---|---|
| `protocol_version` | Integer `1`; reject unsupported versions. |
| `task_id`, `repo` | Stable task ID and verified GitHub `owner/repository`, not a guessed module import path. |
| `issue`, `pr` | Verified URLs or `null`. A GitHub code delivery needs its task Issue and linked PR; locally proposed objects stay `null`. |
| `branch` | Observed branch name or `null`; informational, never an immutable identity. |
| `base_sha`, `source_sha` | Full 40-character lowercase Git commit IDs. Base is the agreed comparison base, source is the exact reviewed candidate. Unknown identity is `null` and blocks review. Dirty edits are not represented by HEAD. |
| `evidence_sha` | Full evidence commit ID, or `null` for unattached/uncommitted evidence or non-Git attachments. Never a substitute for source. |
| `observed_head_sha` | Last observed PR/remote branch head, or `null` if not observed. A different evidence-only head requires an explicit verified diff/manifest explanation in `summary`. |
| `source_availability` | `github`, `local`, `uncommitted`, or `unknown`; `github` requires a successful read of this exact commit through the recipient's authorized access. |
| `round` | Positive integer allocated by the task coordinator; increments at each new handoff/re-review, unchanged on retry. Coordinate allocation by rereading existing records; conflicting allocations block current acceptance. |
| `handoff_key` | Literal `repo/task_id/source_sha/round` joined by `/`. Use `unknown` in place of null source, which cannot be accepted. |
| `event_key` | `handoff_key/role/status`. A saved same-key event with equivalent payload is reused on retry. Conflicting payloads require reconciliation, not silent overwrite. |
| `role`, `actor`, `recorded_at` | `developer`, `reviewer` or `coordinator`; actual author identity and UTC ISO-8601 timestamp. Developer and accepting reviewer must be distinct actors independently validating the candidate. |
| `status` | One of the states below; it describes this immutable identity, not a blanket approval of the branch. |
| `acceptance_ref` | Immutable approved scope reference (file at full SHA or versioned Issue criteria). Any changed criteria require explicit scope reconciliation and new handoff. |
| `criteria` | Every original criterion: `id`, `result` (`pass`, `fail`, `missing`) and supporting `evidence` locations. No empty or skipped coverage counts as passing. |
| `tests` | Actual entries with `cwd`, `command`, integer `exit_code` or `null` if not run, `result` (`pass`, `fail`, `not_run`) and `evidence`. Zero exit alone does not establish criterion coverage. |
| `evidence` | Durable locations of logs/reports/artifacts, pinned to evidence commit SHA when Git-hosted. For attachments include checksum and manifest binding to source SHA. Local paths require stated recipient access; otherwise blocked for external review. |
| `repair_prompt`, `blockers`, `summary` | Active repair text/path or `null`; list of concrete blockers (empty only when none); concise human explanation. Keep historic failed prompts/evidence when removing an active prompt. |

## States

| Status | Use |
|---|---|
| `in_progress` | Task identified, development not handed off. |
| `local` | Committed candidate is only locally available and remote visibility is the sole blocker; not GitHub-ready. |
| `blocked` | Identity, authorization, access, evidence, coverage or publication outcome prevents a reliable next step. |
| `ready_for_review` | Developer supplied externally readable exact candidate and passing evidence for all original criteria. |
| `changes_requested` | Independent review observed an original acceptance failure; include repair prompt. |
| `accepted` | Independent review verified every original criterion at this current candidate/round and completed rereads. |
| `stale` | Reviewed identity/round/scope was superseded; verdict is historical and cannot accept the current candidate. |

If any blocker beyond local-only source visibility exists, use `blocked` even
when `source_availability` is `local`. Only an independent reviewer emits `accepted`. Local reviews may retain useful
findings, but cannot imply GitHub readiness or external acceptance. Tests and
criteria with missing evidence produce `blocked`, even if their result is claimed
as passing. Nonzero exits remain failures. Test commands requiring separately
authorized effects remain `not_run` with a blocker.

A consumer locates records by `handoff_key`, verifies authorship and required
fields, then reads the current task candidate/round/head before interpreting a
verdict. JSON syntax or a self-declared `reviewer` role alone is not evidence.
Evidence-only head advancement does not change source identity if its content
and provenance are verified; any source or scope change requires a new round.
Pre/post publication rereads detect observed races but cannot eliminate a later
commit. Preserve append-only history; mark superseded verdicts stale rather than
erasing evidence or rewriting another writer's record.

## Fictional example: local committed candidate

These IDs and results demonstrate format only; they are not real repository
objects, executed tests or a completed GitHub write. Replace them with observed
facts when using the contract. A ready/review verdict uses the same fields, but
needs real URLs, accessible evidence and the corresponding role/status checks.

<!-- mornlea-handoff:v1 -->
```json
{
  "protocol_version": 1,
  "task_id": "sample-doc-task",
  "repo": "chenyang-zz/mornlea",
  "issue": null,
  "pr": null,
  "branch": "docs/sample-handoff",
  "base_sha": "1111111111111111111111111111111111111111",
  "source_sha": "2222222222222222222222222222222222222222",
  "evidence_sha": null,
  "observed_head_sha": null,
  "source_availability": "local",
  "round": 1,
  "handoff_key": "chenyang-zz/mornlea/sample-doc-task/2222222222222222222222222222222222222222/1",
  "event_key": "chenyang-zz/mornlea/sample-doc-task/2222222222222222222222222222222222222222/1/developer/blocked",
  "role": "developer",
  "actor": "sample-developer",
  "recorded_at": "2026-10-03T12:00:00Z",
  "status": "blocked",
  "acceptance_ref": "AGENTS.md@1111111111111111111111111111111111111111:sample-doc-task",
  "criteria": [{"id": "C1", "result": "pass", "evidence": ["/tmp/sample-handoff/check.txt"]}],
  "tests": [{"cwd": "/tmp/sample-handoff", "command": "git diff --check", "exit_code": 0, "result": "pass", "evidence": "/tmp/sample-handoff/check.txt"}],
  "evidence": ["/tmp/sample-handoff/check.txt; manifest names source_sha; recipient access unverified"],
  "repair_prompt": null,
  "blockers": ["Candidate has not been pushed; GitHub publication is not authorized; external evidence access is unverified"],
  "summary": "Local format example only. Authorized developer must provide externally readable source and evidence before independent review."
}
```
