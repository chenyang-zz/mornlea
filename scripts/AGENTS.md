# 脚本指南

## 开发契约

Hook、gate、发布和 agent 自动化脚本改变的是仓库开发契约，而不只是本地便利命令。修改前应核对调用方、运行环境、失败语义和现有文档。

失败时修复根因；不得删除步骤、吞掉错误、放宽真实 overflow 或数据丢失门禁，也不得使用无 spec 绕过变量规避 Hook。

## Hook 生命周期

`scripts/agent-hooks/guard.mjs` 曾由 `.codex/hooks.json` 与 `.claude/settings.json` 挂接；两处 hook 配置现已移除，该实现当前仅由 CI 的 `node --test scripts/agent-hooks/guard.test.mjs` 覆盖。修改 `guard.mjs` 时以该测试为准，不得假设仍存在钩子调用方或生命周期差异。

## Gate 现状

`scripts/agents/gates.sh` 当前依次运行 gofmt 检查、逐 workspace 模块的 `go vet` 与 archcheck（`go test ./packages/audit -count=1`）、OpenSpec strict 校验、`make rust`，并在未设置 `GATES_SKIP_RACE=1` 时逐模块运行全量 race。它不包含 `make rust-check`，文档和输出不得宣称已经执行该门禁。

修改 shell 脚本时运行对应的 focused shell check；修改 Node 脚本时运行对应的 focused Node check。本指南只记录现状，不修改任何脚本。

## Previous runtime package

`rust-server-opt-in.sh prepare-previous` owns the sealed tracked-source export,
actual native/Go builds, streamed artifact identities, and atomic package
record. It never starts a runtime or opens a world. Failed partial packages
remain for inspection; activation and rollback consume packages separately.

Activation requires an explicit `--previous-manifest` package record as well as
independently checked previous binary/hash inputs. The package stays disjoint
from mutable run, world and backup trees. Every resume revalidates its exact
record, source, oracle, binary and native bindings before stopping a writer.
Rollback runs the actual sealed Go all-family verifier only after quiescence,
with a 60-second owned-child timeout, bounded strict report and independent
before/after world hashes. It verifies current compatible saves or the installed
backup before starting Go; failures retain diagnostic logs/reports and restored
artifacts for inspection. An already live previous writer is probed idempotently
without offline verification.

## Offline restore ownership (`rust-server-restore.py`)

`rust-server-restore.py` is the private offline restore consumer of
`rust-server-opt-in.sh`. Its original native flock remains held through bounded
streamed copy/hash work, atomic directory exchange, no-replace retirement and
all durable manifest checkpoints. Canonical, staged and retired roots share the
original lock inode; a separate copied stage lock must also be held before any
stale-stage deletion or lock rebinding. A live stage writer refuses restoration.

Linux uses libc `renameat2` with exchange/no-replace flags; Darwin uses
`renamex_np` with swap/exclusive flags. Unavailable or failed native operations
retain artifacts and refuse; there is no retire-before-install fallback.
Original directory identity and role hashes qualify crash continuation even
when the original and backup hashes agree. Legacy installed worlds with separate
lock lineages refuse rather than rebinding another writer's lock.

The private `_restore` boundary callback is test-only construction around actual
filesystem operations. The CLI never enables callbacks or environment fault
bypasses. The sealed Go verifier and previous runtime acquire their own native
leases after the helper releases its guard; this is not descriptor transfer.
`activation_restore_lease.rs` under the server persistence-failure suite pins
actual held-lease refusal, paused native boundaries, directory-role crash
continuation, secondary lock lifetime and typed failure artifacts. The
`uncertain_retirement_barrier_is_retried_before_installed_publication` case
qualifies retry barriers for a world parent distinct from the manifest parent.
Run the activation/verifier/package suites with explicit fixtures and an owned
Linux child supervisor. Focus the helper topic with:

```bash
cargo test --manifest-path packages/engine/Cargo.toml -p mornlea_server --test persistence_failure --locked activation::activation_restore_lease::
bash -n scripts/rust-server-opt-in.sh
```

Also compile Python into a disposable cache, run server all-target clippy and
check workspace formatting. These gates preserve the inherited fixture and
process ownership requirements.
