# Committed tick tunables retention

Goal: preserve the checked RuleTunables held by the existing EnvironmentState through the real advance_tick. This maps the original spec's supported outcome and tick-start freeze; it does not claim configuration-file loading, startup assembly, or completion of overall 4.1/4.2.

Reuse the sole environment record and the exclusive TickContext borrow. Only remove the unconditional tunables reset in freeze_environment; when an environment is missing, the metadata/default initialization, next_tick, and the existing climate advancement remain unchanged. No new API, global configuration owner, or validation rules are needed.

The real local Claude first appends, at the end of the existing source_player_restore test file, two actual tick regressions plus a default control, leaving the main implementation untouched. Both tick runs retain all 19 checked fields; an actual farmland exhaustion with threshold2000, sat500, ex3999 plus a source cost5 must produce hunger19/sat0/ex4, and the native block, publication, Memory decode, and the next quiet tick are verified. After the Host verifies a genuine RED, Claude makes the minimal fix; Codex only formats, runs gates, and performs independent review.

The author may only change freeze_environment in core/state.rs, the test append above, and the environment-ownership note in server AGENTS. The controller maintains the English plan, tasks, and ledger. The sole task list tasks.md is updated only after full Rust, release, related Go, and OpenSpec, plus an independent review of the exact SHA; the other original gaps remain explicitly incomplete. The existing climate, Snow, lifecycle, and mutation guards are unchanged.
