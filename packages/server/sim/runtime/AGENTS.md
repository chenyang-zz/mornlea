# runtime 包：权威编排

`packages/server/sim/runtime` 持有 `Engine`、inbox、订阅、时钟、阶段探针与固定 tick 编排，是唯一允许同时编排其余四个子包的权威入口。

## 职责

- `Engine` 只拥有并发 inbox、订阅、原子 tick/时间计数、阶段状态与编排 scratch，恰好组合一个 `*realm.State` 和一个 `*entity.State`；环境 scratch 属 realm，玩家/伙伴/夜行者及玩法状态属 entity，runtime 不设镜像集合。
- `StepWithTunables` 串行组合固定阶段：命令校验与交互收集 → 伙伴动作 → 区块 ingress → 玩家/伙伴物理与订阅收敛 → 夜行者推进 → 近战与死亡结算 → 伙伴/玩家世界交互 → 睡眠/掉落/熔炉 → 流体/耕地/作物推进 → 容器/挖掘 → 支撑复核 → 单次 `realm.Mutation.Commit` → tick/时间推进与全量发布。
- 持有阶段探针与订阅发布边界，阶段顺序由 `TestRuntimeStepPhaseOrder` 等定点钉住。
- 对外玩家/伙伴/夜行者生命周期和查询方法位于 `entity_delegate.go`，只做窄委派；`subscriptionState` 仅含序号、观察中心与 wanted 集合。

## 依赖方向

- 允许：`packages/shared/core`、`packages/shared/world`、`packages/shared/companion`、`packages/shared/physics`、`packages/server/sim/contract`、`packages/shared/tuning`、`packages/server/sim/realm`、`packages/server/sim/entity`。
- 禁止：被 `contract`/`tuning`/`realm`/`entity` 反向依赖；禁止依赖 `packages/server/server`/`packages/client/client` 具体 transport，`runtime` 只消费领域命令并产出权威结果。
- 方向由 `packages/audit` 强制，`simRequiredEdges` 要求 `runtime` 必须同时依赖 `contract`/`tuning`/`realm`/`entity`；缺失任一编排边即为漂移，`TestSimDependencyViolationsDetectDrift` 将报告「缺少必需依赖边」。

## 关键文件

- `engine.go`：`Engine` 结构、`NewEngine`、时钟与 inbox。
- `engine_step.go`：`StepWithTunables` 固定阶段顺序与单次 `Mutation` Commit。
- `engine_subscription.go`：订阅、发布与阶段探针。
- `entity_delegate.go`：玩家、伙伴、夜行者的窄生命周期与查询委派。
- `tick_tunables.go`：`TickTunables` 与两组独立活动快照的一次捕获。
- `command.go`/`persistence.go`/`world.go`：跨边界值、持久化与 realm 查询委派。
- `runtime_test.go`/`ownership_guard_test.go`：阶段、单 owner 与无镜像回归。
- `command_order_oracle_test.go` executes the real authority and compares its
  normalized results with frozen runtime-migration evidence. Ordinary tests
  require both successful ordering and observed stale-sequence rejection
  fixtures and never rewrite them. Explicit `RUNTIME_ORACLE_EXPORT_DIR` may
  export create-exclusive drafts outside the repository; publication and the
  source-bound manifest remain controller-owned.

## 编排纪律

- server 权威路径只调用 `StepWithTunables`；`Step` 是直接调用方的兼容 wrapper，会先调用 `ActiveTickTunables`。两者都保持串行，新增阶段或写者先核对 `engine_step.go` 的顺序约束、订阅收敛点与最终发布边界。
- 每个推进 tick 仅调用一次 `realm.State.NewMutation()`，全部阶段共享该值并在尾部 `Commit` 一次；不得在阶段中途另建 `Mutation` 或绕过 `Commit` 落盘。
- `ActiveTickTunables` 对 simulation/physics 活动快照各读取一次；二者独立而非跨组原子。server 正常 tick 与关服最终 tick 都把同一局部束传给 manager 和 `StepWithTunables`，权威路径不得再次读取全局快照或调用隐式 physics wrapper。
- 并发入口（`Enqueue`、`EnqueueCompanionAction`、`EnqueueHostileAction`、`SubmitAcquired`、`SubmitGenerated`）经有界 inbox 与稳定排序进入 tick，跨 goroutine 发送成功后的消息及其切片视为不可变。
- Treat global chunk readiness and per-session publication readiness as separate state. A session that newly wants an already-ready shared chunk still needs one `TickResult.Ready` wake-up even when the global wanted union is unchanged; stable subscriptions do not repeat it, and the server publication layer suppresses snapshots already sent to other sessions.
- Runtime-migration oracle tests may read and compare frozen corpus assets but
  must never rewrite tracked evidence. Draft exports require an explicit
  external directory and reject repository paths, symlinks and overwrites.
  Corpus publication is separately reviewed controller work, guarded
  repository-wide by `packages/audit`.
- 权威 tick、持久化与发布热路径不得执行无界工作或阻塞 CPU/磁盘/网络。

## 定点验证

- `go test ./packages/server/sim/runtime -race -count=1`
- 子树全量：`go test ./packages/server/sim/... -race -count=1`
- 依赖边界：`go test ./packages/audit -count=1`
