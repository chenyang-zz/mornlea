# Task 3.7 rerun: the seven open inventory rows

Source commit: `faede5650b213b04eb9d160e9816bdead0738fbd` (fresh `cursor/rust-authoritative-server-98e6` head on 2026-10-08; it moved from `091657a0` only through docs-only archive commits, with no change under `packages/`, `scripts/` or the `Makefile`). After PR #8 (`0af287b4`) changed `packages/engine`, the row tests were re-checked at `173ec439` and still pass.
Host: Linux x86_64, rustc/cargo 1.97.1, go 1.26.0 (`environment.txt`).

## Result

- All seven rows that the earlier review left as open implementation gaps (rows with `test: null` in the old precise bindings) now have fresh evidence: the production producer location, Rust tests that assert content, recipient and order, the Memory/TCP parity test, and the matching Go tests. Every command exited 0 on the source commit above.
- The native Loom review was not rerun. chen removed it as a gate on 2026-10-08, so task 3.7 stays **ticked in place**. See "Loom review" below and the F2 ledger entry "Task 3.7 rerun evidence (2026-10-08)".

## How it was produced

`run-rerun.sh` runs every command from a clean checkout. Each log starts with the exact command, the source commit and a UTC start time, and ends with `# exit: <code>`. Rust tests run with `--exact`, so a passing count equal to the number of names shows that every named test exists and ran. Go runs with `-race -count=1 -v`.

Production call chain (`call-chain.txt`): `AuthorityState::advance_tick` -> `step::reduce_tick` -> `step.rs:410 project_source_publication` -> prepare (`publication_project.rs:267 emit_despawns`, `:268 emit_forgets`) and finish_tail (`:344 emit_companions`, `:353 emit_remotes`, `:372 emit_chat`).

## Per-row evidence

| Row | Producer (`producer.txt`) | Rust lib | Rust replay | Parity | Go (`go.log`) |
|---|---|---|---|---|---|
| `event.domain.chat` | `publication_project.rs:471 emit_chat`, `:485 Event::Chat` | 1/1 | 5/5 | 1/1 | 4 PASS |
| `event.domain.companion-spawn` | `:1150 emit_companions`, `:1199 Event::CompanionSpawn` | 4/4 | 3/3 | 1/1 | 4 PASS |
| `event.domain.companion-despawn` | `:929 emit_despawns`, `:950 Event::CompanionDespawn` | 2/2 | 2/2 | 1/1 | 1 PASS |
| `event.domain.forget-chunks` | `:958 emit_forgets`, `:979 Event::ForgetChunks` | 1/1 | 1/1 | 1/1 | 2 PASS |
| `event.domain.remote-player-spawn` | `:1249 emit_remotes`, `:1289 Event::RemotePlayerSpawn` | 3/3 | 3/3 | 1/1 | 5 PASS |
| `event.domain.remote-player-despawn` | `:929 emit_despawns`, `:944 Event::RemotePlayerDespawn` | 3/3 | 4/4 | 1/1 | 3 PASS |
| `event.domain.remote-player-states` | `:1335 RemotePlayerStates::try_new`, `:1341 Event::RemotePlayerStates` | 2/2 | 2/2 | 1/1 | 2 PASS |

Each row's directory holds `producer.txt`, `rust-lib.log`, `rust-replay.log`, `rust-parity.log` and `go.log`. Test names are in the command line at the top of each log and in `run-rerun.sh`.

What the tests show, per row:

- **chat**: accepted companion chat is broadcast in channel order and malformed or unknown chat rejects only the sender (replay `projection_chat_*`, Go `TestAcceptedCompanionChatBroadcastsInChannelOrder`, `TestMalformedOrUnknownCompanionChatRejectsOnlySender`). The command addresses the exact configured companion at the tick boundary. Memory and TCP publish identical bytes (`chat_publications_match_across_adapters`, `TestCompanionChatMemoryTCPParity`).
- **companion-spawn**: spawn waits for the foot-chunk snapshot. States are sorted and a new spawn skips its own tick's state. A reset is captured once, batches split within the wire caps, and companion capacity is independent of the 8-player capacity.
- **companion-despawn**: despawn on interest exit is emitted once per family, with departure/arrival ordering and a preflight retry.
- **forget-chunks**: snapshot, then block changes, then forget, all delivered to the target session only (`TestSessionRegistrySnapshotsDeltasAndForgetStayWithTarget`), in player-state publication order.
- **remote-player-spawn**: the interest matrix, silent join and leave outside interest, a negative foot chunk uses the floor, publication order, and a same-UUID generation replacement despawns before it spawns.
- **remote-player-despawn**: despawn preflight retry, same-UUID replacement, unavailable-observer preflight, and a reconnect replaces the visible incarnation.
- **remote-player-states**: sorted states where a new spawn skips its own tick, and eight sessions publish seven states with a 296-byte payload.

## Supplemental: Agent integration

`supplemental/agent-integration.log` runs `agent_process` (7/7) and `server_replay full_corpus::` (3/3) against the locked local agent venv (`uv sync --locked` in `packages/agent`, with `MORNLEA_AGENT_PYTHON` and `PYTHONPATH` set as in the log header). These failed on this host earlier only because the fixture interpreter was missing.

## Loom review (not rerun)

The 3.7 acceptance line records "native Loom reviewer remains timed_out/NO VERDICT". That reviewer is the frozen Loom controller (`89a86e9` in the ledger) driving a local Claude CLI through CCSwitch on the owner's Mac; it is not in this repository. chen removed the Loom review gate on 2026-10-08, so it was not rerun and no Loom verdict is claimed.

The Go integration harness in `packages/server/server` can intermittently report a SeasonProgress mismatch, because `replayResult()` zeroes `WorldTimeTicks` but not the season fields derived from it while the test server runs on a real-time ticker. This is a test harness issue, not authority logic, and it does not affect the Rust results. It did not occur in these runs, and it is being fixed separately against dev in PR #18.

## Verify

```sh
cd openspec/changes/rust-authoritative-server/evidence/3.7-rerun
sha256sum -c MANIFEST.sha256          # integrity of every recorded file
bash run-rerun.sh                     # regenerate the per-row logs on a checkout
```

`MANIFEST.sha256` was written last. It lists the sha256 of every file in this directory, with paths relative to it, and excludes itself.
