# Comprehensive-review code-repair handoff

The comprehensive-review code repairs are complete on `dev` at source `8e561c4122e179bd17d4bbf1cb2a730822fd4cd2`. The live endpoint, provider and G1/Python boundary reviewers each verified that exact candidate independently. Their acceptance is limited to their assigned code scopes; original full F3 acceptance remains pending.

The actual core composes one session, confirmed world, input/prediction, bounded native preparation and all ten families. Failed publication keeps previous resources and retry ownership. Actor removal, crafting attribution and audio source/provenance are repaired. The final review additionally found and resolved failed-far retry classification and incomplete host payload admission. No remaining reproduced code defect was reported by the final reviewers.

The original review remains preserved as round 2 evidence. This coordinator allocates round 3 to the final candidate. Source and logs are local; no Issue/PR write or remote push occurred. Existing OpenSpec artifacts inherit root guidance and introduce no new directory boundary.

<!-- mornlea-handoff:v1 -->
```json
{
  "protocol_version": 1,
  "task_id": "rust-client-core",
  "repo": "chenyang-zz/mornlea",
  "issue": null,
  "pr": null,
  "branch": "dev",
  "base_sha": "b383673e383dcf1f6177998854e4752a28304a2b",
  "source_sha": "8e561c4122e179bd17d4bbf1cb2a730822fd4cd2",
  "evidence_sha": null,
  "observed_head_sha": "bee580f76eab34618ce40a0fb942f5955b199d10",
  "source_availability": "local",
  "round": 3,
  "handoff_key": "chenyang-zz/mornlea/rust-client-core/8e561c4122e179bd17d4bbf1cb2a730822fd4cd2/3",
  "event_key": "chenyang-zz/mornlea/rust-client-core/8e561c4122e179bd17d4bbf1cb2a730822fd4cd2/3/coordinator/blocked",
  "role": "coordinator",
  "actor": "Codex /root coordinating three independent reviewers",
  "recorded_at": "2026-10-11T09:53:31Z",
  "status": "blocked",
  "acceptance_ref": "Original seven criteria: /private/tmp/mornlea-f3-full-review.md (round 2); both delta specifications/design/tasks/plans07@b383673e383dcf1f6177998854e4752a28304a2b; repair plan08@8e561c4122e179bd17d4bbf1cb2a730822fd4cd2",
  "criteria": [
    {
      "id": "Client state has one Rust owner",
      "result": "pass",
      "evidence": [
        "/private/tmp/mornlea-f3-live-final-review.md",
        "/private/tmp/mornlea-f3-provider-final-review.md"
      ]
    },
    {
      "id": "Frame publication is atomic and bounded",
      "result": "pass",
      "evidence": [
        "/private/tmp/mornlea-f3-live-final-review.md",
        "/private/tmp/mornlea-f3-provider-final-review.md"
      ]
    },
    {
      "id": "Reset and bridge lifecycle release ownership",
      "result": "missing",
      "evidence": [
        "/private/tmp/mornlea-f3-live-final-review.md",
        "/private/tmp/mornlea-f3-boundary-final-review.md",
        "tasks.md:3.3b; actual native release/marshalling not run"
      ]
    },
    {
      "id": "Rust core parity is independent of rendering",
      "result": "missing",
      "evidence": [
        "/private/tmp/mornlea-f3-live-final-review.md",
        "/private/tmp/mornlea-f3-provider-final-review.md",
        "tasks.md:3.3c,3.4; native100 and real F2 integration not run"
      ]
    },
    {
      "id": "Migration preserves typed host isolation",
      "result": "pass",
      "evidence": [
        "/private/tmp/mornlea-f3-boundary-final-review.md",
        "/private/tmp/mornlea-f3-provider-final-review.md"
      ]
    },
    {
      "id": "Cross-runtime boundaries publish only bounded atomic snapshots",
      "result": "pass",
      "evidence": [
        "/private/tmp/mornlea-f3-live-final-review.md",
        "/private/tmp/mornlea-f3-provider-final-review.md",
        "/private/tmp/mornlea-f3-boundary-final-review.md"
      ]
    },
    {
      "id": "A Rust-mode host tick distributes one complete semantic frame",
      "result": "pass",
      "evidence": [
        "/private/tmp/mornlea-f3-live-final-review.md",
        "/private/tmp/mornlea-f3-boundary-final-review.md"
      ]
    },
    {
      "id": "5.1",
      "result": "pass",
      "evidence": [
        "/private/tmp/mornlea-f3-boundary-final-review.md"
      ]
    },
    {
      "id": "5.2",
      "result": "pass",
      "evidence": [
        "/private/tmp/mornlea-f3-live-final-review.md"
      ]
    },
    {
      "id": "5.3",
      "result": "pass",
      "evidence": [
        "/private/tmp/mornlea-f3-provider-final-review.md",
        "/private/tmp/mornlea-f3-live-final-review.md"
      ]
    },
    {
      "id": "5.4",
      "result": "pass",
      "evidence": [
        "/private/tmp/mornlea-f3-provider-final-review.md",
        "/private/tmp/mornlea-f3-live-final-review.md"
      ]
    },
    {
      "id": "5.5",
      "result": "pass",
      "evidence": [
        "/private/tmp/mornlea-f3-provider-final-review.md",
        "/private/tmp/mornlea-f3-live-final-review.md"
      ]
    },
    {
      "id": "5.6",
      "result": "pass",
      "evidence": [
        "/private/tmp/mornlea-f3-provider-final-review.md",
        "/private/tmp/mornlea-f3-live-final-review.md"
      ]
    },
    {
      "id": "5.7",
      "result": "pass",
      "evidence": [
        "/private/tmp/mornlea-f3-provider-final-review.md",
        "/private/tmp/mornlea-f3-live-final-review.md"
      ]
    },
    {
      "id": "5.8",
      "result": "pass",
      "evidence": [
        "/private/tmp/mornlea-f3-live-final-review.md"
      ]
    },
    {
      "id": "5.9",
      "result": "pass",
      "evidence": [
        "/private/tmp/mornlea-f3-final-dev-check-qualified.log",
        "/private/tmp/mornlea-f3-final-test-race.log",
        "/private/tmp/mornlea-f3-final-audit.log",
        "/private/tmp/mornlea-f3-final-openspec.log",
        "/private/tmp/mornlea-f3-live-final-review.md",
        "/private/tmp/mornlea-f3-boundary-final-review.md",
        "/private/tmp/mornlea-f3-provider-final-review.md",
        "tasks.md: five original qualification nodes remain pending"
      ]
    }
  ],
  "tests": [
    {
      "cwd": "/Users/chen/work/mornlea",
      "command": "make rust",
      "exit_code": 0,
      "result": "pass",
      "evidence": "/private/tmp/mornlea-f3-final-rust.log"
    },
    {
      "cwd": "/Users/chen/work/mornlea",
      "command": "TMPDIR=/private/tmp PATH=/Users/chen/work/mornlea/packages/agent/.venv/bin:$PATH MORNLEA_AGENT_PYTHON=/Users/chen/work/mornlea/packages/agent/.venv/bin/python MORNLEA_COMPANION_AGENT_PYTHON=/Users/chen/work/mornlea/packages/agent/.venv/bin/python MORNLEA_PREVIOUS_SERVER_BIN=/private/var/folders/99/_lwvdq2d6db3qj6ngjklqf0h0000gn/T/mornlea-rust-quality.IU8wPG/previous-server MORNLEA_PREVIOUS_PACKAGE=/private/var/folders/99/_lwvdq2d6db3qj6ngjklqf0h0000gn/T/mornlea-rust-quality.IU8wPG/previous-runtime.json make dev-check",
      "exit_code": 0,
      "result": "pass",
      "evidence": "/private/tmp/mornlea-f3-final-dev-check-qualified.log"
    },
    {
      "cwd": "/Users/chen/work/mornlea",
      "command": "TMPDIR=/private/tmp PATH=/Users/chen/work/mornlea/packages/agent/.venv/bin:$PATH MORNLEA_AGENT_PYTHON=/Users/chen/work/mornlea/packages/agent/.venv/bin/python MORNLEA_COMPANION_AGENT_PYTHON=/Users/chen/work/mornlea/packages/agent/.venv/bin/python MORNLEA_PREVIOUS_SERVER_BIN=/private/var/folders/99/_lwvdq2d6db3qj6ngjklqf0h0000gn/T/mornlea-rust-quality.IU8wPG/previous-server MORNLEA_PREVIOUS_PACKAGE=/private/var/folders/99/_lwvdq2d6db3qj6ngjklqf0h0000gn/T/mornlea-rust-quality.IU8wPG/previous-runtime.json make test-race",
      "exit_code": 0,
      "result": "pass",
      "evidence": "/private/tmp/mornlea-f3-final-test-race.log"
    },
    {
      "cwd": "/Users/chen/work/mornlea",
      "command": "TMPDIR=/private/tmp go test ./packages/audit -count=1",
      "exit_code": 0,
      "result": "pass",
      "evidence": "/private/tmp/mornlea-f3-final-audit.log"
    },
    {
      "cwd": "/Users/chen/work/mornlea",
      "command": "openspec validate --all --strict --no-interactive",
      "exit_code": 0,
      "result": "pass",
      "evidence": "/private/tmp/mornlea-f3-final-openspec.log"
    },
    {
      "cwd": "/private/tmp/mornlea-boundary-final-8e561c4",
      "command": "/Users/chen/work/mornlea/apps/mornlea-godot/.venv/bin/python -B -m unittest discover -s apps/mornlea-godot/tests/unit -v",
      "exit_code": 0,
      "result": "pass",
      "evidence": "/private/tmp/mornlea-boundary-final-python-pinned.log; independently executed at the exact candidate, 37 tests"
    },
    {
      "cwd": "/Users/chen/work/mornlea",
      "command": "make godot-python-check godot-project-check godot-capability-check",
      "exit_code": 0,
      "result": "pass",
      "evidence": "/private/tmp/mornlea-f3-host-payload-main-gates.log; carried checked-source bytes from 1d350c120671246342585b276f3dbc6a68c60265"
    },
    {
      "cwd": "/Users/chen/work/mornlea",
      "command": "TMPDIR=/private/tmp go test ./packages/server/server -race -count=1 -run ^TestCompanionManagerPlaceAtomicConsumeAndDepletion$",
      "exit_code": 0,
      "result": "pass",
      "evidence": "/private/tmp/mornlea-f3-final-companion-retry.log"
    },
    {
      "cwd": "/Users/chen/work/mornlea",
      "command": "TMPDIR=/private/tmp PATH=/Users/chen/work/mornlea/packages/agent/.venv/bin:$PATH MORNLEA_AGENT_PYTHON=/Users/chen/work/mornlea/packages/agent/.venv/bin/python MORNLEA_COMPANION_AGENT_PYTHON=/Users/chen/work/mornlea/packages/agent/.venv/bin/python MORNLEA_PREVIOUS_SERVER_BIN=/private/var/folders/99/_lwvdq2d6db3qj6ngjklqf0h0000gn/T/mornlea-rust-quality.IU8wPG/previous-server MORNLEA_PREVIOUS_PACKAGE=/private/var/folders/99/_lwvdq2d6db3qj6ngjklqf0h0000gn/T/mornlea-rust-quality.IU8wPG/previous-runtime.json rustup run 1.97.1 cargo test --manifest-path packages/engine/Cargo.toml -p mornlea_server --locked --test persistence_failure activation",
      "exit_code": 0,
      "result": "pass",
      "evidence": "/private/tmp/mornlea-f3-final-python-path-retry.log"
    },
    {
      "cwd": "/Users/chen/work/mornlea",
      "command": "TMPDIR=/private/tmp MORNLEA_AGENT_PYTHON=/Users/chen/work/mornlea/packages/agent/.venv/bin/python MORNLEA_COMPANION_AGENT_PYTHON=/Users/chen/work/mornlea/packages/agent/.venv/bin/python MORNLEA_PREVIOUS_SERVER_BIN=/private/var/folders/99/_lwvdq2d6db3qj6ngjklqf0h0000gn/T/mornlea-rust-quality.IU8wPG/previous-server MORNLEA_PREVIOUS_PACKAGE=/private/var/folders/99/_lwvdq2d6db3qj6ngjklqf0h0000gn/T/mornlea-rust-quality.IU8wPG/previous-runtime.json make dev-check",
      "exit_code": 2,
      "result": "fail",
      "evidence": "/private/tmp/mornlea-f3-final-dev-check.log; historical companion test failure"
    },
    {
      "cwd": "/Users/chen/work/mornlea",
      "command": "TMPDIR=/private/tmp MORNLEA_AGENT_PYTHON=/Users/chen/work/mornlea/packages/agent/.venv/bin/python MORNLEA_COMPANION_AGENT_PYTHON=/Users/chen/work/mornlea/packages/agent/.venv/bin/python MORNLEA_PREVIOUS_SERVER_BIN=/private/var/folders/99/_lwvdq2d6db3qj6ngjklqf0h0000gn/T/mornlea-rust-quality.IU8wPG/previous-server MORNLEA_PREVIOUS_PACKAGE=/private/var/folders/99/_lwvdq2d6db3qj6ngjklqf0h0000gn/T/mornlea-rust-quality.IU8wPG/previous-runtime.json make dev-check",
      "exit_code": 2,
      "result": "fail",
      "evidence": "/private/tmp/mornlea-f3-final-dev-check-retry.log; historical 23 Python-version failures"
    },
    {
      "cwd": "/Users/chen/work/mornlea",
      "command": "openspec validate --all --strict --no-interactive",
      "exit_code": 0,
      "result": "pass",
      "evidence": "/private/tmp/mornlea-f3-final-evidence-openspec.log; status/evidence-only working tree after reconciliation"
    }
  ],
  "evidence": [
    "Local evidence manifest below binds the final source and distinguishes historical/predecessor artifacts. Raw logs/reports have checksums and byte counts; external recipient access and retention are unverified."
  ],
  "repair_prompt": "Original review repair request retained at /private/tmp/mornlea-f3-full-review.md#repair-request; remaining qualification is tasks.md:3.3b,3.3c,3.4,4.1,4.2",
  "blockers": [
    "Original native Godot/Python release and marshalling, native100, real F2 all-family integration and dependent full closeout have not run.",
    "Candidate and raw evidence are local only; GitHub publication is not authorized; linked Issue/PR and external recipient access are unverified."
  ],
  "summary": "All nine authorized code-repair nodes pass final exact-source scoped independent reviews and code gates. The seven original review criteria are retained: native lifecycle release and full real-server parity remain missing. This coordinator record is blocked for full F3 qualification and external delivery; it does not emit blanket acceptance. Subsequent status/evidence-only documentation preserves product source and acceptance definitions."
}
```

## Execution environment and qualification

The final full `make dev-check` and `make test-race` use the already qualified environment below. Canonical temporary paths preserve the storage verifier's symlink refusal. Python 3.12 is required by existing restoration/package scripts; both their executable overrides and command lookup must select it.

```text
TMPDIR=/private/tmp
PATH=/Users/chen/work/mornlea/packages/agent/.venv/bin:$PATH
MORNLEA_AGENT_PYTHON=/Users/chen/work/mornlea/packages/agent/.venv/bin/python
MORNLEA_COMPANION_AGENT_PYTHON=/Users/chen/work/mornlea/packages/agent/.venv/bin/python
MORNLEA_PREVIOUS_SERVER_BIN=/private/var/folders/99/_lwvdq2d6db3qj6ngjklqf0h0000gn/T/mornlea-rust-quality.IU8wPG/previous-server
MORNLEA_PREVIOUS_PACKAGE=/private/var/folders/99/_lwvdq2d6db3qj6ngjklqf0h0000gn/T/mornlea-rust-quality.IU8wPG/previous-runtime.json
```

The sealed previous-runtime fixture binds `d042982d33bb1694d768b75b01c297bd02534a08`; its source, executable, verifier, Go oracle and native dependency hashes were verified before reuse. The locked host tooling/project/capability run was performed after `1d350c120671246342585b276f3dbc6a68c60265`; checked host, scripts and dependency bytes are unchanged at the final candidate. Its evidence is explicitly carried, while the final host unit suite and independent boundary review were freshly executed at the final source.

| Pending original node | Missing prescribed evidence |
| --- | --- |
| 3.3b | Actual native Godot/embedded Python marshalling and release ordering |
| 3.3c | 100 rebuilt, artifact-bound real Godot/Python session cycles |
| 3.4 | Real F2 Memory/TCP, C1/C2 and G1 integration against all families |
| 4.1 | Complete zero-gap real-provider/integration reconciliation |
| 4.2 | Full stage closeout on the accepted integrated runtime source; current code aggregates pass separately |

`tasks.md` remains the status authority. Code-repair completion closes neither these nodes nor the product/default-switch work. No Godot process or actual F3-to-F2 integration was run. The coordinator's blocked handoff describes missing full qualification and external delivery, not an unresolved observed code defect.

## Preserved failure history and source maintenance

- The first final `make dev-check` returned 2 in `TestCompanionManagerPlaceAtomicConsumeAndDepletion/DepletionKeepsFirstPlacement`. Its isolated race retry returned 0, followed by a complete passing Go short stage. No server implementation changed.
- The next `make dev-check` returned 2: its Rust restoration group had 23 failures after system Python 3.9 was selected because the controller omitted the recorded Python PATH. Under qualified Python 3.12, the focused group passed 58 tests, followed by all 416 restoration cases in the complete passing workspace run. Failed exits remain recorded; no test was changed or skipped.
- Earlier pre-final aggregate attempts encountered symlinked temporary paths, the projectile timing case and stale numerical-source digests. Canonical-path storage/projectile retries passed; their history remains in the local repair logs and earlier ledger entries. These attempts are not current passing evidence.
- The numerical manifest changed exactly twelve existing source-digest leaves: eleven references to `native/mod.rs` and one to `native/worldgen.rs`. Identities, source paths, frozen `source_revision`, all 1434 cases and expected assets/values remain unchanged. Actual file digests and the structural-only diff were independently verified. Protocol/save/schema/ABI versions and dependencies are unchanged.
- A source-range whitespace check found a trailing blank line in the historical ledger. Appending this evidence removes that blank line; the final documentation diff must pass `git diff --check`.

Source identity remains the reviewed `8e561c4122e179bd17d4bbf1cb2a730822fd4cd2`. The follow-up changes status/evidence only; it must contain no product source or acceptance-definition changes and receive an independent diff check. A future source change requires a fresh source-bound review. Integration and rollback remain controller-owned; preserve all existing unrelated work and worker checkouts. Architecture skill: no change.

## Local evidence manifest

```json
{
  "source_sha": "8e561c4122e179bd17d4bbf1cb2a730822fd4cd2",
  "recipient_access": "local workspace only; external access/retention unverified",
  "artifacts": [
    {
      "path": "/private/tmp/mornlea-f3-full-review.md",
      "bytes": 25630,
      "sha256": "c34611ef07eefb805ff86d1d5a2ba35497208d08102dddf061839be544d8692f",
      "binding": "Original review, source b383673e383dcf1f6177998854e4752a28304a2b, round 2"
    },
    {
      "path": "/private/tmp/mornlea-f3-live-final-review.md",
      "bytes": 6193,
      "sha256": "4deecd850e71aaf75fe8fa4bd9a5d7581ba7688ed5ccf4166f94fb04dab41276",
      "binding": "Exact final candidate review; explicitly carried unchanged predecessor results"
    },
    {
      "path": "/private/tmp/mornlea-f3-boundary-final-review.md",
      "bytes": 8996,
      "sha256": "2002af7e947689f668615544741759c6260128a3f4a0b3a9da058304cc528c90",
      "binding": "Exact final candidate review; pinned interpreter and four independent refusal probes"
    },
    {
      "path": "/private/tmp/mornlea-f3-provider-final-review.md",
      "bytes": 4984,
      "sha256": "6629673b845b653a31371902279ffe4d670464bc8cecd8324182a21ebb4e7a54",
      "binding": "Exact final candidate review; carried unchanged providers plus fresh real far regression"
    },
    {
      "path": "/private/tmp/mornlea-f3-live-final-probe.log",
      "bytes": 1662,
      "sha256": "a0e49bad42c395d2ae3ebd47f6a64fd5314ed45fec6cec48c229c7cbc50066fc",
      "binding": "Exact final candidate; 17 actual public-endpoint cases"
    },
    {
      "path": "/private/tmp/mornlea-boundary-final-host-probe.log",
      "bytes": 276,
      "sha256": "51fe68edc090aa75737cae9657f9a7575903082334f902358efdacd0e44d3991",
      "binding": "Exact final candidate; four independent host refusal/preservation assertions"
    },
    {
      "path": "/private/tmp/mornlea-boundary-final-python-pinned.log",
      "bytes": 6209,
      "sha256": "8d3474be0c4e7de6ee89ea79f7ce1864d68afb2327c6104f3ff141707ee402f2",
      "binding": "Exact final candidate; 37 supported-Python host unit tests"
    },
    {
      "path": "/private/tmp/mornlea-boundary-final-rust.log",
      "bytes": 4904,
      "sha256": "9b675d86bda089d87433df2287e31471ddce1c1e7ace9369a214433141469721",
      "binding": "Exact final candidate; 40 Rust adapter tests"
    },
    {
      "path": "/private/tmp/mornlea-f3-provider-final-provenance.json",
      "bytes": 3660,
      "sha256": "1f89bf0dc474935fb08024ae0cf731874340e6c7b2bc8ca0d826798cd9af78a9",
      "binding": "Exact final candidate; independent structural and source digest verification"
    },
    {
      "path": "/private/tmp/mornlea-f3-provider-final-regression.log",
      "bytes": 1325,
      "sha256": "19582f6036a49ae31ef711bc75ace0d8ad337d7980606e13b3479cf224a381a2",
      "binding": "Exact final candidate; actual failed-native-far regression"
    },
    {
      "path": "/private/tmp/mornlea-f3-final-rust.log",
      "bytes": 639,
      "sha256": "7f2a796b1cd35b1ba4116fd0b201cb4602aa6fe108c2bfbb6f8d92f28edecc9e",
      "binding": "Final candidate release native build"
    },
    {
      "path": "/private/tmp/mornlea-f3-final-host-unit.log",
      "bytes": 6209,
      "sha256": "1ca5de2097da972a969732d89c4452011fb89840548ddb54cff911bc0d2b80f9",
      "binding": "Final candidate; 37 host unit tests"
    },
    {
      "path": "/private/tmp/mornlea-f3-final-audit.log",
      "bytes": 59,
      "sha256": "6d734d2971f8a8ae2c8abf5b9592bfbdc5545096b15aafaf6f81d277106a1ff4",
      "binding": "Final candidate; uncached audit"
    },
    {
      "path": "/private/tmp/mornlea-f3-final-openspec.log",
      "bytes": 4037,
      "sha256": "2f748f8fb4f3f06733fc54bca5bab90bcc3c08e3e4e8884ed7019009a83da2a8",
      "binding": "Final candidate before status/evidence follow-up; 127 items"
    },
    {
      "path": "/private/tmp/mornlea-f3-host-payload-main-gates.log",
      "bytes": 772,
      "sha256": "637ab7411195f9b429f415dc7137c7759a6e72d3447db8bf68acb4023ea4ff0b",
      "binding": "Locked Python/project/capability checks after host merge; same checked source bytes at final candidate"
    },
    {
      "path": "/private/tmp/mornlea-f3-provenance-refresh.log",
      "bytes": 78,
      "sha256": "6d62cc090142c0a1ed76c656aee571fba0f076aba5e7ccb823c18d9dbcacf252",
      "binding": "Refreshed manifest tree matching final candidate; full runtime-oracle package"
    },
    {
      "path": "/private/tmp/mornlea-f3-final-dev-check.log",
      "bytes": 4207,
      "sha256": "8ba504a785ca4add6f64f64db505caf2519d4c9d8ce33321949029c3b6925406",
      "binding": "Final candidate historical failed attempt; server companion event test"
    },
    {
      "path": "/private/tmp/mornlea-f3-final-companion-retry.log",
      "bytes": 66,
      "sha256": "ef81b51b66fc0120ae035ae4440e964eba4ec9a010a97aca707006461449fc03",
      "binding": "Final candidate; isolated companion race retry passed"
    },
    {
      "path": "/private/tmp/mornlea-f3-final-dev-check-retry.log",
      "bytes": 306143,
      "sha256": "628fa0f5618c7469048b30cd53789a97a81868c85550a33fc10d103064aa7c73",
      "binding": "Final candidate historical failed attempt; missing Python PATH, 23 restoration failures"
    },
    {
      "path": "/private/tmp/mornlea-f3-final-python-path-retry.log",
      "bytes": 6313,
      "sha256": "f35b8ceb529a0ec99ea1c51e8719ee3328bb46843a094cf0fb99f7b59401bc25",
      "binding": "Final candidate; qualified Python PATH retry, 58 tests passed"
    },
    {
      "path": "/private/tmp/mornlea-f3-live-candidate-review.md",
      "bytes": 9742,
      "sha256": "4999c25b416aa17fc11fb82ba670676a8f3315c14c1755f5bb29af4cf3ac1fa9",
      "binding": "Superseded source 6dc06b85330e2598525289dc68381b7c9422b9a7; historical scope review"
    },
    {
      "path": "/private/tmp/mornlea-f3-boundary-candidate-review.md",
      "bytes": 8920,
      "sha256": "064b7f025fe628047a9e196d2158a6776a17d316c5ee20bef0288b143e246aa4",
      "binding": "Superseded source 6dc06b85330e2598525289dc68381b7c9422b9a7; historical R14 changes request"
    },
    {
      "path": "/private/tmp/mornlea-f3-provider-candidate-review.md",
      "bytes": 11250,
      "sha256": "9afca565d352e6b8ada00acba6d12fc7cdee85bac5a489e5d378a52ff04c8628",
      "binding": "Superseded source 6dc06b85330e2598525289dc68381b7c9422b9a7; historical far changes request"
    },
    {
      "path": "/private/tmp/mornlea-f3-provider-rereview.md",
      "bytes": 5964,
      "sha256": "4f6c93c2d7d1fbadbfdba2314e02be332d8d6d3329ca4cd321783c6d9e32c8d0",
      "binding": "Scoped accepted far source 4ae0c0f8e232df63a03dfa9d90a972dbe1e54bed"
    },
    {
      "path": "/private/tmp/mornlea-f3-final-dev-check-qualified.log",
      "bytes": 386228,
      "sha256": "3e30445ebde5ecdd24ea9abe3d849c0e9fc6945899f62f6981342bc4e5842f35",
      "binding": "Exact final candidate; complete qualified dev-check passed, 85 Rust summaries / 4640 tests"
    },
    {
      "path": "/private/tmp/mornlea-f3-final-test-race.log",
      "bytes": 4828,
      "sha256": "8ee269f9b58aee5921ace7b88661413cc8b4c1c52c279ebef57f6125b0d17064",
      "binding": "Exact final candidate; complete six-module race gate passed"
    },
    {
      "path": "/private/tmp/mornlea-f3-final-evidence-openspec.log",
      "bytes": 4037,
      "sha256": "2f748f8fb4f3f06733fc54bca5bab90bcc3c08e3e4e8884ed7019009a83da2a8",
      "binding": "Strict OpenSpec after status/evidence reconciliation; product source unchanged"
    }
  ]
}
```
