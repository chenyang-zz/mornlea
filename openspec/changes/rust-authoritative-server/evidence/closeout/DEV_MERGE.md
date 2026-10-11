# Local dev integration evidence

Merge source: `aab9b0f0c998924124567bdbe2d22003e1a55b78`. Ordered parents: incoming dev `84cd626d6a6f233608bbc63245f1aa09c1f951ee`, local closeout candidate `e9bf930e0a3868ed57a30837b6833bb9b8c38c8c`. Exact tree: `ef48de84206f2ad70724f623f4f5fbdf7d495401`.

Both local branch refs were synchronized to this merge at the source checkpoint. Subsequent closing commits contain only planning/evidence, with source identity retained. Independent [paired-source review](DEV_MERGE_REVIEW.md) preserves the endpoint and planning repairs, the dev replay correction and corpus digests. Task 4.5's local synchronization condition is complete. Hosted PR synchronization and full F2 acceptance remain open.

Final local gates pass: native build/deployment, full race across six Go modules, six-module vet/short plus Rust fmt/Clippy and 76 summaries / 4142 passing / zero failed or ignored, actual Agent/server integration, and strict OpenSpec (127/0 on the merged source). The [manifest](DEV_MERGE_MANIFEST.json) names exact commands, environments, source bindings and local raw-log hashes. The first source-tree gates began on the pending merge tree; serial dev-check ran on the clean immutable merge commit.

The first parallel dev-check failed `TestCompanionManagerMineApproachesBeforeMining` with event kinds `[1 6]`, expected `[1 3 5]`. All 20 focused runs and the complete serial gate subsequently passed. Root cause remains undetermined; no source repair was made and the failed log is retained. Final passing results do not turn that first attempt into a passing gate.

User-requested cleanup removed project AOCI indexes/runtime assets, its sole-server MCP file and injected guidance/ignore additions. Both original user stashes remain. The shared global CLI, out-of-repository safety backup and historical ledger mentions remain. No AOCI setup or maintenance is required after this explicit removal.

The existing feature worktree is retained for open gameplay composition, clocks, fluid configuration, companion planning/execution and non-Overworld work. No push, remote PR/Issue write, deployment, cutover or archive occurred. This evidence leaf inherits the active change's guidance. Architecture skill: no change.

Strict OpenSpec validation also passes after the closing task/ledger/evidence edits: 127 passed, zero failed. Its separately hashed raw log is recorded as `openspec-close` in the manifest.

The following new local integration record preserves earlier CI/endpoint handoff history. Its evidence is unattached at authoring time; it is blocked for external delivery by publication authority, task anchoring and recipient access, independently of passing local gates.

<!-- mornlea-handoff:v1 -->
```json
{
  "protocol_version": 1,
  "task_id": "rust-authoritative-server-dev-merge",
  "repo": "chenyang-zz/mornlea",
  "issue": null,
  "pr": "https://github.com/chenyang-zz/mornlea/pull/5",
  "branch": "dev",
  "base_sha": "84cd626d6a6f233608bbc63245f1aa09c1f951ee",
  "source_sha": "aab9b0f0c998924124567bdbe2d22003e1a55b78",
  "evidence_sha": null,
  "observed_head_sha": null,
  "source_availability": "local",
  "round": 1,
  "handoff_key": "chenyang-zz/mornlea/rust-authoritative-server-dev-merge/aab9b0f0c998924124567bdbe2d22003e1a55b78/1",
  "event_key": "chenyang-zz/mornlea/rust-authoritative-server-dev-merge/aab9b0f0c998924124567bdbe2d22003e1a55b78/1/coordinator/blocked",
  "role": "coordinator",
  "actor": "Codex /root",
  "recorded_at": "2026-10-11T01:32:23Z",
  "status": "blocked",
  "acceptance_ref": "openspec/changes/rust-authoritative-server/tasks.md@e9bf930e0a3868ed57a30837b6833bb9b8c38c8c:4.2,4.5; user-authorized project AOCI removal (2026-10-11)",
  "criteria": [
    {
      "id": "local-merge-and-paired-fix-preservation",
      "result": "pass",
      "evidence": [
        "DEV_MERGE_REVIEW.md; immutable merge aab9b0f0c998924124567bdbe2d22003e1a55b78",
        "DEV_MERGE_MANIFEST.json#ordered_parents"
      ]
    },
    {
      "id": "required-final-local-stage-gates",
      "result": "pass",
      "evidence": [
        "DEV_MERGE_MANIFEST.json#gates; first failed attempt retained in historical_support"
      ]
    },
    {
      "id": "project-aoci-removal-and-user-work-preservation",
      "result": "pass",
      "evidence": [
        "DEV_MERGE_MANIFEST.json#aoci_cleanup; local proof and original stash identities"
      ]
    }
  ],
  "tests": [
    {
      "cwd": "/Users/chen/work/mornlea",
      "command": "make rust",
      "exit_code": 0,
      "result": "pass",
      "evidence": "DEV_MERGE_MANIFEST.json#native; raw /tmp/mornlea-dev-merge-native.log"
    },
    {
      "cwd": "/Users/chen/work/mornlea",
      "command": "make test-race",
      "exit_code": 0,
      "result": "pass",
      "evidence": "DEV_MERGE_MANIFEST.json#race; raw /tmp/mornlea-dev-merge-race.log"
    },
    {
      "cwd": "/Users/chen/work/mornlea",
      "command": "make dev-check",
      "exit_code": 0,
      "result": "pass",
      "evidence": "DEV_MERGE_MANIFEST.json#dev-check-serial; raw /tmp/mornlea-dev-merge-dev-check-serial.log"
    },
    {
      "cwd": "/Users/chen/work/mornlea",
      "command": "make companion-agent-integration",
      "exit_code": 0,
      "result": "pass",
      "evidence": "DEV_MERGE_MANIFEST.json#agent-integration; raw /tmp/mornlea-dev-merge-agent-integration.log"
    },
    {
      "cwd": "/Users/chen/work/mornlea",
      "command": "openspec validate --all --strict --no-interactive",
      "exit_code": 0,
      "result": "pass",
      "evidence": "DEV_MERGE_MANIFEST.json#openspec-before-close; raw /tmp/mornlea-dev-merge-openspec.log"
    },
    {
      "cwd": "/Users/chen/work/mornlea",
      "command": "openspec validate --all --strict --no-interactive",
      "exit_code": 0,
      "result": "pass",
      "evidence": "DEV_MERGE_MANIFEST.json#openspec-close; raw /tmp/mornlea-dev-merge-openspec-close.log"
    }
  ],
  "evidence": [
    "DEV_MERGE_MANIFEST.json; source-bound log checksums; local access only",
    "DEV_MERGE_REVIEW.md; copied unchanged from isolated reviewer report"
  ],
  "repair_prompt": null,
  "blockers": [
    "Exact merge and closing evidence are local; push and GitHub publication are outside current authorization.",
    "No task Issue was identified; external recipient access to raw logs and hosted CI are unverified."
  ],
  "summary": "Local dev merge and project AOCI removal are complete. All final product gates pass at the unchanged merge source. First parallel dev-check failed one companion test; 20 focused runs and the serial complete gate passed, with no root cause or source fix established. Independent review proves paired-source preservation only. Remaining gameplay tasks and F2 archive stay open; no external acceptance is claimed."
}
```
