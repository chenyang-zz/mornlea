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
