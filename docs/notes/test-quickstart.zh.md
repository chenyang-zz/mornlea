---
doc_id: test-quickstart
doc_revision: 2026-10-13.1
language: zh-CN
counterpart: test-quickstart.md
---
# 开发期测试快启

[English version](test-quickstart.md).

测试很慢时先分清楚「慢在哪」：仓库测试总量大（495 个 Go 测试文件、33 个包）是「一文件一主题」策略的常态；实测时间里大部分集中在客户端命令的重型子包与 `packages/server/server`——`packages/client/cmd/mornlea/capture`（golden 场景抓帧）、`packages/client/cmd/mornlea/benchmark`（真实 renderer 场景；分包前两者与 app 同住单包实测约 4 分钟）与 `packages/server/server`（长 tick 集成/容量测试）。Rust 侧首次编译 wgpu 全家桶是分钟级，增量 `cargo test -p` 约 5 秒。全量 race（`make test-race`，五模块循环）本机约 4.5 分钟，且高负载会诱发时敏测试的负载 flake。

## 分层纪律（T0–T3）

大部分开发时间不该花在等测试上——按改动所处阶段选层级，**不要跳级**：

| 层级 | 时机 | 命令 | 预期耗时 |
|---|---|---|---|
| **T0 编辑循环** | 每次改动后 | 改动包定点 `-run` 测试（见下表），不带 `-race`/`-count=1`；Rust 用 `cargo test -p <crate>` | 秒级 |
| **T1 任务闭环** | 每个 Task/修复完成 | `make test-race-changed`（改动包及其反向依赖，恒含 archcheck；`RACE_BASE=ref` 换基线） | 秒级–分钟级 |
| **T2 推送前** | 分支完成、开 PR 前 | `make dev-check` + `make test-race-short` | ~2–3 分钟 |
| **T3 提交前/风险域** | 触碰并发时敏域，或需本地复现 CI 失败 | `scripts/agents/gates.sh`（gofmt/vet/archcheck/OpenSpec/`make rust`，未跳过时含 full race） | ~5 分钟 |

要点：

- **禁止把全量 race 加 `-count=1` 当快检**：`-race -count=1` 强制全量失效缓存，五模块循环每次都是完整 4.5 分钟起步。`-race -count=1` 是 T1/T2/T3 的专属旗标；T0 恰恰靠省略它们吃测试缓存。
- **本地与 CI 不双跑全量 race**：CI 的 `go-race` 已按 client / server / rest（contracts+shared+tools）三分片并行执行全量。默认路径是 T2 绿 → 推送 → 信任 CI；本地 T3 只在 (a) 改动涉及 `packages/server/sim`/`packages/server/server`/`packages/shared/network`/`packages/server/storage`/`packages/shared/physics` 的并发或时序语义，或 (b) CI 红了需要本地复现时执行。`AGENT_MODE=merge`（不经 PR 直推）时 T3 本地必跑。
- **flake 分诊协议**：高负载下出现「等待预算/超时」类失败时，先对**失败包单独重跑**（`go test ./<pkg> -race -count=1`）；单独通过即记录为负载 flake（写进 ledger 或 PR 备注），不进修复循环、不为此改生产代码。同一 flake 一天内重现 ≥2 次才立待修任务（参照 E-11 模式）。
- **验证证据可继承**：同一基线 SHA 下已记入 change `ledger.md` 的验证输出直接引用，不重跑同等命令；评审复核用 focused 测试抽查（见 `docs/development-process.md` 阶段 3）。
- **T1 闭包的边界**：`race-changed.sh` 的反向依赖沿生产 import 边传递，测试 import 只算一层直接依赖（否则触碰 `packages/shared/core` 这类底座包时闭包近似全仓）；残余盲区由 T3 与 CI 兜底。迭代验证涉及 `packages/client/cmd/mornlea/app`、`packages/client/cmd/mornlea/benchmark` 或 `packages/server/server` 的重型测试时可对这几个包追加 `-short`（capture 包没有 `testing.Short()` 守卫，`-short` 对它是空操作）；脚本的「集合含重型包」提示识别 `packages/client/cmd/mornlea/app`、`packages/client/cmd/mornlea/benchmark` 与 `packages/server/server`。

## T0：定点测试

| 改动域 | 快检命令 |
|---|---|
| 窗口 / 渲染 / 材质 / WGSL | `cargo test -p mornlea_client --locked` + `go test ./packages/client/cmd/mornlea/app -run 'TestFitFramebuffer\|TestApplicationConnection'` |
| 协议 / 网络 / 存档 | `go test ./packages/shared/network/... ./packages/server/storage/... ./packages/shared/core` |
| 服务端 tick / 伙伴 / 农业 | `go test ./packages/server/server ./packages/server/sim/... -run 'keywords'` |
| Python 伙伴 Agent | `cd packages/agent/companion && uv run pytest tests/test_<topic>.py -q`；首次进入先 `uv sync --locked` |
| Go/Python Agent 合同 | `go test ./packages/server/server -run 'CrossLanguage|CompanionAgent.*Integration'`（完整真进程入口见下文） |
| 流体队列 / 规则求值 / 重扫包装 | `go test ./packages/server/fluid`（重扫差分门禁在 `./packages/server/sim/realm`，见 `packages/server/fluid/AGENTS.md`） |
| 资产 / 材质包 / provenance | `go test ./packages/client/assets` |
| 视觉 golden | 预期不变：`make visual-check`；预期变化：逐图确认后 `make visual-update`，再运行 `make visual-check` |
| 依赖边界 / 文档一致性 | `make archcheck` |

要点：

- `go test ./pkg` 不带 `-count=1` 时未改动包命中缓存秒回；`-race` 与 `-count=1` 会强制失效缓存，只留给 T1 及以上。
- Rust 增量构建从不清 `target/`；Makefile 为 `make rust` 默认设置当前 worktree 的 `CARGO_TARGET_DIR=packages/engine/target/cargo`，避免跨 worktree 复用产物。直接调用 cargo 不读取 Makefile，需要该目录或其他目标时须显式设置 `CARGO_TARGET_DIR`；CI 也显式设置，传给 `make rust` 的值可覆盖默认值。日常用 `cargo test -p <crate> --locked`，`make rust-check` 留到提交前。

## T1：race-changed

```bash
make test-race-changed             # 相对 origin/main 的改动闭包跑 race
make test-race-changed RACE_BASE=<ref>   # 换比较基线（如批次共享 SHA）
scripts/agents/race-changed.sh --diff    # 只打印包集合不运行（核对闭包用）
```

集合 = 改动包（已提交 diff ∪ 暂存 ∪ 未暂存 ∪ 未跟踪的 .go）∪ 生产 import 反向依赖（传递）∪ 测试 import 直接依赖。闭包含 cdylib 消费包（packages/shared 的 nativeabi/core/physics、packages/client 模块的 mesh/client/cmd/mornlea 及其 app/capture/benchmark 子包，与 packages/server 模块的 sim/server/cmd/mornlea-server）时脚本先按需 `make rust`；纯 Go 叶子改动不构建 Rust。

Python 服务不进入 Go import 闭包：修改 `packages/agent/companion` 后，T1 另运行 `make companion-agent-check`，它执行 locked sync、Ruff format/lint、`mypy src` 与全部 pytest。修改 HTTP/MCP wire、进程生命周期或 integration helper 时，再运行 `make companion-agent-integration`；该入口会检查 `tests/integration` 并以 race detector 启动真实 Go/Python loopback 子进程。

## T2：短模式与一键快检

`packages/client/cmd/mornlea/app`、`packages/client/cmd/mornlea/benchmark` 与 `packages/server/server` 中的重型测试（每个超过数秒的离屏 renderer 场景、benchmark 真实 renderer 场景、长 tick 集成/容量测试）在 `testing.Short()` 时跳过；capture 包没有 `testing.Short()` 守卫，`-short` 对它是空操作，重型 golden 抓帧走 `make visual-check` 的独立门禁而非测试二进制：

```bash
make test-race-short   # 五模块循环 -race -short，实测比全量快约 5 倍
make dev-check         # gofmt + 五模块 vet + 短测试 + Rust fmt/clippy/单测
make companion-agent-check        # Agent Python 全量质量门禁
make companion-agent-integration  # 改动跨语言边界时运行真进程合同
```

`-short` 只是把「快」与「正确性」分离的迭代工具：CI 与最终门禁**不带** `-short` 全量运行，跳过不放松任何正确性门禁。判断标准：单个测试耗时 ≥ 1.5 秒才值得加；单元级断言永远保留。

## T3：全量门禁

```bash
scripts/agents/gates.sh     # gofmt/vet/archcheck/OpenSpec/rust/全量 race
make rust-check             # 完整提交前单独运行；gates.sh 当前不包含此项
make companion-agent-check
make companion-agent-integration
```

`scripts/agents/gates.sh` 当前依次执行 gofmt、vet、archcheck、OpenSpec、`make rust`，并在未设置 `GATES_SKIP_RACE=1` 时执行 full race；它不包含 `make rust-check`、`make companion-agent-check` 或 `make companion-agent-integration`，触碰对应领域时必须显式补跑。伙伴两条入口都使用 deterministic fake model 与临时 SQLite，只绑定 loopback，不访问 provider/DNS/外网，也不会启动或聚焦游戏窗口。预期视觉不变时运行 `make visual-check`；预期视觉变化时先逐图确认，再运行 `make visual-update`，随后重新运行 `make visual-check`。

完整提交前：`make rust-check` → `make test-race` → 对应 benchmark/golden/`perfcheck`（性能数值只记录）。
