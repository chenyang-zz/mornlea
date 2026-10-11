# Local dev integration handoff

The authorized local merge is complete at `470692d13f3cfe704a3c95d57ba714fe1853fd3d`. Its reviewed tree is
`d93cf3a0b37f2def4fec6a5215e55f206fbb0947`, with ordered dev/feature parents `bee580f76eab34618ce40a0fb942f5955b199d10` and `11f8affdefcf886a2ddbda028516294ca73abc17`.
The independent [source review](dev-merge-review.md) reports no remaining
Critical/Important integration findings. Full F3 qualification remains open.

The following copyable record is blocked for external delivery only. No Issue,
PR, push or external acceptance was produced. Raw log access is local; publish
source and durable evidence only under separately authorized delivery scope.
These existing change artifacts inherit root guidance and introduce no directory
ownership or lifecycle boundary.

<!-- mornlea-handoff:v1 -->
```json
{
  "protocol_version": 1,
  "task_id": "rust-client-core",
  "repo": "chenyang-zz/mornlea",
  "issue": null,
  "pr": null,
  "branch": "dev",
  "base_sha": "bee580f76eab34618ce40a0fb942f5955b199d10",
  "source_sha": "470692d13f3cfe704a3c95d57ba714fe1853fd3d",
  "evidence_sha": null,
  "observed_head_sha": "bee580f76eab34618ce40a0fb942f5955b199d10",
  "source_availability": "local",
  "round": 1,
  "handoff_key": "chenyang-zz/mornlea/rust-client-core/470692d13f3cfe704a3c95d57ba714fe1853fd3d/1",
  "event_key": "chenyang-zz/mornlea/rust-client-core/470692d13f3cfe704a3c95d57ba714fe1853fd3d/1/coordinator/blocked",
  "role": "coordinator",
  "actor": "Codex /root",
  "recorded_at": "2026-10-11T03:11:33Z",
  "status": "blocked",
  "acceptance_ref": "openspec/changes/rust-client-core/ledger.md@470692d13f3cfe704a3c95d57ba714fe1853fd3d:2026-10-11 local dev integration; openspec/changes/rust-client-core/plans/07-code-only-implementation-handoff.md@11f8affdefcf886a2ddbda028516294ca73abc17",
  "criteria": [
    {
      "id": "local-integration-source",
      "result": "pass",
      "evidence": [
        "openspec/changes/rust-client-core/dev-merge-review.md; independently verified source, tree and ordered parents; local Git evidence",
        "470692d13f3cfe704a3c95d57ba714fe1853fd3d: ordered merge parents bee580f76eab34618ce40a0fb942f5955b199d10, 11f8affdefcf886a2ddbda028516294ca73abc17; exact reviewed tree d93cf3a0b37f2def4fec6a5215e55f206fbb0947"
      ]
    },
    {
      "id": "local-integration-validation",
      "result": "pass",
      "evidence": [
        "/private/tmp/mornlea-client-core-merge-focused.log; sha256=39f9d7c22267b8bcf9df8b3f671d535ff28d17b206df0c1a35d79cf316033195; bytes=43538; local recipient access only; 532 passed; zero failed/ignored",
        "/private/tmp/mornlea-client-core-merge-rust-fixed.log; sha256=9e1d467720fce1198678de18860d98a9db1b4488a73ce427d8253a9deb6a5654; bytes=529; local recipient access only; release native build and established dylib deployment passed",
        "/private/tmp/mornlea-client-core-merge-godot-python-final.log; sha256=65349f05fdd16fa1a0f13c358ab7f692d7bcc7bd0723851c43ae1255a632e2da; bytes=562; local recipient access only; locked boundary units, Ruff and mypy passed; 54 source files",
        "/private/tmp/mornlea-client-core-merge-godot-static.log; sha256=60f826c65d5817e3b20e053a3a771e5193535696a07df2d4ba6866d9760d1990; bytes=210; local recipient access only; project closure, 14 disabled capabilities and 10 producer families passed",
        "/private/tmp/mornlea-client-core-merge-agent-lock.log; sha256=d8b3c8bac4a8f973dad4cb840565dfa1d74ea8260270ea4f4c7587ed6753248b; bytes=56; local recipient access only; qualified Agent environment synchronized",
        "/private/tmp/mornlea-client-core-merge-openspec.log; sha256=2f748f8fb4f3f06733fc54bca5bab90bcc3c08e3e4e8884ed7019009a83da2a8; bytes=4037; local recipient access only; 127 passed; zero failed",
        "/private/tmp/mornlea-client-core-merge-dev-check.log; sha256=40d33dd1960d04b3ae92b888ad515ddf0f5ddc9e33d26ec879288b66aa6b239f; bytes=376942; local recipient access only; six-module vet/short and Rust fmt/Clippy/workspace passed; 85 Rust summaries, 4522 passed, zero failed/ignored",
        "/private/tmp/mornlea-client-core-merge-race.log; sha256=875c973040aa9861d53c71928a8209c96fdc3ad7d4c9e3284f55cfbcc35b3884; bytes=4828; local recipient access only; all six Go modules passed race; unchanged package cache results retained",
        "/private/tmp/mornlea-client-core-merge-audit.log; sha256=be47e178333b1d1cf4e8a1967667278cfe803d7766cc3d6386c4e83c685f87e4; bytes=60; local recipient access only; uncached architecture audit passed in 152.155s",
        "/private/tmp/mornlea-client-core-merge-host-final.log; sha256=c17df77b442f50598c3c473d5105f885dedaa7b0dbe438ef9fba1f0bf3af76e3; bytes=3785; local recipient access only; 23 engine-free host cases passed after source commit"
      ]
    },
    {
      "id": "local-integration-scope",
      "result": "pass",
      "evidence": [
        "openspec/changes/rust-client-core/dev-merge-review.md; independently verified source, tree and ordered parents; local Git evidence",
        "openspec/changes/rust-client-core/tasks.md@470692d13f3cfe704a3c95d57ba714fe1853fd3d: all 41 feature checkbox rows preserved; existing runtime and contract versions retained"
      ]
    }
  ],
  "tests": [
    {
      "cwd": "/Users/chen/work/mornlea",
      "command": "CARGO_TARGET_DIR=/Users/chen/work/mornlea/packages/engine/target/cargo rustup run 1.97.1 cargo test --manifest-path packages/engine/Cargo.toml -p mornlea_client_core -p mornlea_godot --locked",
      "exit_code": 0,
      "result": "pass",
      "evidence": "/private/tmp/mornlea-client-core-merge-focused.log; sha256=39f9d7c22267b8bcf9df8b3f671d535ff28d17b206df0c1a35d79cf316033195; bytes=43538; local recipient access only; 532 passed; zero failed/ignored"
    },
    {
      "cwd": "/Users/chen/work/mornlea",
      "command": "make rust",
      "exit_code": 0,
      "result": "pass",
      "evidence": "/private/tmp/mornlea-client-core-merge-rust-fixed.log; sha256=9e1d467720fce1198678de18860d98a9db1b4488a73ce427d8253a9deb6a5654; bytes=529; local recipient access only; release native build and established dylib deployment passed"
    },
    {
      "cwd": "/Users/chen/work/mornlea",
      "command": "make godot-python-check",
      "exit_code": 0,
      "result": "pass",
      "evidence": "/private/tmp/mornlea-client-core-merge-godot-python-final.log; sha256=65349f05fdd16fa1a0f13c358ab7f692d7bcc7bd0723851c43ae1255a632e2da; bytes=562; local recipient access only; locked boundary units, Ruff and mypy passed; 54 source files"
    },
    {
      "cwd": "/Users/chen/work/mornlea",
      "command": "make godot-project-check godot-capability-check",
      "exit_code": 0,
      "result": "pass",
      "evidence": "/private/tmp/mornlea-client-core-merge-godot-static.log; sha256=60f826c65d5817e3b20e053a3a771e5193535696a07df2d4ba6866d9760d1990; bytes=210; local recipient access only; project closure, 14 disabled capabilities and 10 producer families passed"
    },
    {
      "cwd": "/Users/chen/work/mornlea",
      "command": "uv sync --locked --project packages/agent",
      "exit_code": 0,
      "result": "pass",
      "evidence": "/private/tmp/mornlea-client-core-merge-agent-lock.log; sha256=d8b3c8bac4a8f973dad4cb840565dfa1d74ea8260270ea4f4c7587ed6753248b; bytes=56; local recipient access only; qualified Agent environment synchronized"
    },
    {
      "cwd": "/Users/chen/work/mornlea",
      "command": "openspec validate --all --strict --no-interactive",
      "exit_code": 0,
      "result": "pass",
      "evidence": "/private/tmp/mornlea-client-core-merge-openspec.log; sha256=2f748f8fb4f3f06733fc54bca5bab90bcc3c08e3e4e8884ed7019009a83da2a8; bytes=4037; local recipient access only; 127 passed; zero failed"
    },
    {
      "cwd": "/Users/chen/work/mornlea",
      "command": "make dev-check",
      "exit_code": 0,
      "result": "pass",
      "evidence": "/private/tmp/mornlea-client-core-merge-dev-check.log; sha256=40d33dd1960d04b3ae92b888ad515ddf0f5ddc9e33d26ec879288b66aa6b239f; bytes=376942; local recipient access only; six-module vet/short and Rust fmt/Clippy/workspace passed; 85 Rust summaries, 4522 passed, zero failed/ignored"
    },
    {
      "cwd": "/Users/chen/work/mornlea",
      "command": "make test-race",
      "exit_code": 0,
      "result": "pass",
      "evidence": "/private/tmp/mornlea-client-core-merge-race.log; sha256=875c973040aa9861d53c71928a8209c96fdc3ad7d4c9e3284f55cfbcc35b3884; bytes=4828; local recipient access only; all six Go modules passed race; unchanged package cache results retained"
    },
    {
      "cwd": "/Users/chen/work/mornlea",
      "command": "go test ./packages/audit -count=1",
      "exit_code": 0,
      "result": "pass",
      "evidence": "/private/tmp/mornlea-client-core-merge-audit.log; sha256=be47e178333b1d1cf4e8a1967667278cfe803d7766cc3d6386c4e83c685f87e4; bytes=60; local recipient access only; uncached architecture audit passed in 152.155s"
    },
    {
      "cwd": "/Users/chen/work/mornlea",
      "command": "apps/mornlea-godot/.venv/bin/python -m unittest discover -s apps/mornlea-godot/tests/unit -p test_rust_feature_host.py -v",
      "exit_code": 0,
      "result": "pass",
      "evidence": "/private/tmp/mornlea-client-core-merge-host-final.log; sha256=c17df77b442f50598c3c473d5105f885dedaa7b0dbe438ef9fba1f0bf3af76e3; bytes=3785; local recipient access only; 23 engine-free host cases passed after source commit"
    }
  ],
  "evidence": [
    "openspec/changes/rust-client-core/dev-merge-review.md; independently verified source, tree and ordered parents; local Git evidence",
    "This record binds command outputs and checksums to the reviewed source. Raw logs are local temporary attachments; external recipient access and retention are unverified."
  ],
  "repair_prompt": null,
  "blockers": [
    "Merged source has not been pushed; GitHub publication is outside the current local-merge authorization.",
    "No verified linked task Issue/PR was found; external evidence access is unverified."
  ],
  "summary": "Authorized local dev integration is complete and independently reviewed. Blocked describes external delivery only. Criteria cover this scoped integration, not full F3 acceptance. Subsequent closeout evidence is separate from the source commit and must pass an evidence-only diff check. No archive, default switch or real Godot qualification is claimed."
}
```

## Execution binding and environment

The aggregate gates ran on the frozen reviewed merge tree before its normal
merge commit. Earlier focused gates exercised identical product source before
final ledger documentation was appended; the locked host suite was rerun after
the source commit. The evidence-only follow-up changes documentation only and
does not replace the source identity above. Reuse of unchanged Go test cache
results is visible in the logs; the separate audit used `-count=1`.

For `make dev-check`, `make test-race` and the uncached audit, the controller used:

```text
TMPDIR=/private/tmp
PATH=/Users/chen/work/mornlea/packages/agent/.venv/bin:$PATH
MORNLEA_AGENT_PYTHON=/Users/chen/work/mornlea/packages/agent/.venv/bin/python
MORNLEA_COMPANION_AGENT_PYTHON=/Users/chen/work/mornlea/packages/agent/.venv/bin/python
MORNLEA_PREVIOUS_SERVER_BIN=/private/var/folders/99/_lwvdq2d6db3qj6ngjklqf0h0000gn/T/mornlea-rust-quality.IU8wPG/previous-server
MORNLEA_PREVIOUS_PACKAGE=/private/var/folders/99/_lwvdq2d6db3qj6ngjklqf0h0000gn/T/mornlea-rust-quality.IU8wPG/previous-runtime.json
```

The sealed prior-runtime package binds
`d042982d33bb1694d768b75b01c297bd02534a08`; executable, verifier, Go oracle and
native dependency hashes were checked against its manifest before reuse.
Canonical temporary paths preserve the verifier's symlink-ancestor refusal.
Clean incoming dev `make rust` passed before focused Go checks:
`/private/tmp/mornlea-client-core-baseline-rust.log; sha256=e46b5b753c5784a91c55a90215052bb769c29a2d46f12ee7af054ca9a877f6ff; bytes=431; local recipient access only`.

## Preserved failure history

These failures belong to earlier integration states, before the checked chat
constructor and Python static-check repairs. Their final passing replacements
are recorded above; failed exits are not described as passing.

- `make rust`: exit 2, removed ChatIntent::new constructor. `/private/tmp/mornlea-client-core-merge-rust.log; sha256=1a585ba91eb8e97ffc517c5d6d473d41a468914755d225aefcdcf722334aa094; bytes=1362; local recipient access only`.
- `CARGO_TARGET_DIR=/Users/chen/work/mornlea/packages/engine/target/cargo rustup run 1.97.1 cargo test --manifest-path packages/engine/Cargo.toml -p mornlea_godot --test rust_producer --locked`: exit 101, existing-target compile red before production repair. `/private/tmp/mornlea-client-core-merge-chat-red.log; sha256=b68bc5de4c2f0543f162a0ae16f2d0eb0d0e4c2b4b283f413be7f16ce6654b54; bytes=1368; local recipient access only`.
- `make godot-python-check`: exit 2, formatter failures. `/private/tmp/mornlea-client-core-merge-godot-python.log; sha256=a194a585ebdf2ab806a154465bf6eddb55e4df680f0864fb8ac8c883483e2ddf; bytes=4238; local recipient access only`.
- `make godot-python-check`: exit 2, 15 mypy errors after formatting. `/private/tmp/mornlea-client-core-merge-godot-python-fixed.log; sha256=62fb9d830623df2e2d19f8ec677bf3a8e1655609eb0232f5a57afb2e35ada17e; bytes=2325; local recipient access only`.
- `make godot-python-check`: exit 2, Ruff B010 on temporary fixture setattr form. `/private/tmp/mornlea-client-core-merge-godot-python-green.log; sha256=65a65a1487ada246a30fe6a4a8c27c7e64872fc05ab0c481d46f515d249e0daa; bytes=7727; local recipient access only`.

## Qualification still pending

The original prompt projection, native Dictionary marshalling/release, real
embedded Python and 100 process cycles, real F2 Memory/TCP/all-family integration,
production connector registration and full F3 closeout remain with their planned
owners. The existing terminate/connector-close error ordering and bounded audio
allocation qualifications remain recorded. No Godot process or game window ran.
Protocol/save/ABI versions and the production default are preserved. Architecture
skill: no change.
