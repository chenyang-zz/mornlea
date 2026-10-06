# Agent boundary

`src/agent/` owns independent loopback Agent requests and read-only MCP
planning capabilities. It owns neither authoritative world mutation nor the
Python process. Behavior is governed by the Rust authoritative server OpenSpec
change; the crate guide owns production dependency review.

## Directory map

- `http.rs`: closed-schema Agent HTTP, absolute request deadlines and cancellation.
- `lease.rs`: checked remote lease identity, freeze fences and release eligibility.
- `host.rs`: bounded task/dialogue/control requests and retained terminal cleanup.
- `memory.rs`: commit/reconcile ownership and retryable finalization;
  private `memory_authority.rs` gates outcomes against complete authoritative persistence.
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

`McpService::close_until` and its actual `McpLifecycle` implementation stop
admission before attempting registry cancellation and interrupting retained
socket I/O. The independent close-start mutex and registry core acquisition
both honor the caller's absolute deadline. Registry contention retains snapshot
records and leaves the start barrier retryable; worker timeout retains actual
joins for same-service retry. A held synchronous `PlanningTools` cannot be
killed. Only successful explicit close proves accept and connection joins retired;
compatibility
`close` is finite best-effort cleanup. Pending join diagnostics are bounded and
returned once after retirement. No world, store or Agent-process ownership
crosses this boundary.

## Snapshot close (`snapshot.rs`)

`SnapshotRegistry::close_until_shared` uses the same real monotonic deadline
passed by MCP. Core contention polls outside locks; poison reports
`Internal("snapshot close")`. The common private settlement also serves legacy
blocking `close_shared`. Every retained cancellation flag settles under the
actual core mutex before its closed bit can authorize a completed observation.
`REGISTRY_CAPACITY` bounds the records; drained snapshot payloads drop after
unlocking. Direct and cross-service callers share this completion boundary.

Private `close_tests` hold the actual core mutex behind entry/release channels,
exercise MCP deadline refusal and retry, and prove actual shared/direct close
cancellation and typed poison failure. They expose no production fault API.

## Helpers and regression evidence

Authoritative memory polls bind response request/client/namespace/companion
identity, then use latest ledger lifecycle metadata. Commit CAS precedes working
mirror replacement and complete reservation fulfillment; refusal retains the
proposal and retires only its HTTP attempt. Reconcile compares authoritative
active epoch and revision, accepts exact equality without a write, and passes
higher state through CAS before readiness. Inactive reconcile and delete replies
acknowledge an already durable tombstone; they do not create a local lifecycle
transition. Sticky or Closed authority refuses acceptance.

`AuthoritativeMemoryFinalizer` borrows the existing memory owner and requires the
shutdown machine's same authority through `MemoryFinalizer::drain_authority`.
It refuses authority-free drain. The trait default also refuses an enabled complete
companion owner, so a bare remote provider cannot bypass CAS in configured shutdown. Pending semantic work and cleanup joins remain
charged across fresh attempts; completion means ledger acceptance, while the
following aggregate flush/sync owns physical durability. Bare memory polls and
finalizers remain pure remote-provider controls. Configured callers must choose
the explicit authoritative variants; this adapter does not assemble a runtime
or publish the returned reservation's separately authorized dialogue effect.

The memory-authority contract descendant uses a scripted Agent, the companion
persistence descendant uses actual DiskStore with a scripted Agent, and the
Agent-process descendant uses actual Python HTTP with a prepared complete
authority ledger. Those independent cases do not claim combined physical
shutdown or configured executable integration.

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
cargo test --manifest-path packages/engine/Cargo.toml -p mornlea_server --lib --locked
cargo test --manifest-path packages/engine/Cargo.toml -p mornlea_server --test server_contract --locked agent_mcp::
cargo test --manifest-path packages/engine/Cargo.toml -p mornlea_server --test server_contract --locked agent_snapshot::
cargo test --manifest-path packages/engine/Cargo.toml -p mornlea_server --test agent_process --locked
cargo clippy --manifest-path packages/engine/Cargo.toml -p mornlea_server --all-targets --locked -- -D warnings
```

The actual Python cases require `MORNLEA_AGENT_PYTHON` naming the qualified
Agent interpreter. Their fixtures own their children; production never starts
Python. Use the repository validation tiers from `docs/notes/test-quickstart.md`.
