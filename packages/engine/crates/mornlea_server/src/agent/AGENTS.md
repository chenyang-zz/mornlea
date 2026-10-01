# Agent boundary

`src/agent/` owns independent loopback Agent requests and read-only MCP
planning capabilities. It owns neither authoritative world mutation nor the
Python process. Behavior is governed by the Rust authoritative server OpenSpec
change; the crate guide owns production dependency review.

## Directory map

- `http.rs`: closed-schema Agent HTTP, absolute request deadlines and cancellation.
- `lease.rs`: checked remote lease identity, freeze fences and release eligibility.
- `host.rs`: bounded task/dialogue/control requests and retained terminal cleanup.
- `memory.rs`: commit/reconcile ownership and retryable finalization.
- `snapshot.rs`: immutable snapshot capabilities, expiry and shared cancellation.
- `mcp.rs`: frozen HTTP tools, bounded connection admission and actual MCP close.
- `mod.rs`: topic registration; integration composition remains with callers.

## MCP ownership (`mcp.rs`)

Admission must charge every spawned connection to `ConnectionOwner` before
releasing the owner mutex. `MAX_MCP_CONNECTIONS` bounds retained workers;
overflow closes its socket without tool dispatch or a protocol response.
Finished owners retire on accept-loop iterations and explicit close, with
finished-only joins and diagnostic settlement under the corresponding owner
mutex. Ordinary response EOF must not depend on a later admission.

`McpService::close_until` and its actual `McpLifecycle` implementation freeze
capabilities, stop admission and interrupt retained socket I/O. The independent
close-start mutex serializes complete registry cancellation; contention polls
the caller's absolute deadline. Timeout retains actual joins for same-service
retry. A held synchronous `PlanningTools` cannot be killed. Only successful
explicit close proves accept and connection joins retired; compatibility
`close` is finite best-effort cleanup. Pending join diagnostics are bounded and
returned once after retirement. No world, store or Agent-process ownership
crosses this boundary.

## Helpers and regression evidence

`tests/server_contract/agent_mcp.rs` owns the MCP snapshot and raw HTTP harness;
its `mcp_lifecycle.rs` child owns held-tool/client helpers. The real loopback
cases `legacy_close_waits_for_actual_held_tool`,
`seventeenth_connection_is_closed_without_tool_dispatch`,
`actual_lifecycle_timeout_retains_tool_then_retries`,
`actual_tool_panic_is_reported_once_then_retired` and
`concurrent_close_attempts_retain_and_retry_same_service` pin ownership,
deadlines, diagnostic retirement and same-service retry. Snapshot cases remain
in `agent_snapshot.rs`; actual Python integration is in `tests/agent_process/`.

## Focused verification

From the repository root with the pinned Rust toolchain and explicit target:

```bash
cargo test --manifest-path packages/engine/Cargo.toml -p mornlea_server --test server_contract --locked agent_mcp::
cargo test --manifest-path packages/engine/Cargo.toml -p mornlea_server --test server_contract --locked agent_snapshot::
cargo test --manifest-path packages/engine/Cargo.toml -p mornlea_server --test agent_process --locked
cargo clippy --manifest-path packages/engine/Cargo.toml -p mornlea_server --all-targets --locked -- -D warnings
```

The actual Python cases require `MORNLEA_AGENT_PYTHON` naming the qualified
Agent interpreter. Their fixtures own their children; production never starts
Python. Use the repository validation tiers from `docs/notes/test-quickstart.md`.
