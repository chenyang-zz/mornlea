# 权威模拟

本目录仅保留指导文档，不含生产 Go Package；权威模拟由四个子包承载，单向依赖，见下表与 `packages/audit`。`Tunables` 快照已上提共享层 `packages/shared/tuning`（纯值对象叶子，见该目录 `AGENTS.md`），不再属于本子树。根 `packages/server/sim` 不得保留生产 Go 文件、类型别名或转发函数，调用方必须直接导入所属子包。

## Directory Map

```
packages/server/sim/
├── AGENTS.md                # 子树总纲：目录地图、依赖方向、Mutation 与定点验证
├── contract/                # 跨边界纯值 DTO（指南见 contract/AGENTS.md）
│   ├── contract.go          # Command/Reject/ChunkChangeBatch/TickResult 等值类型
│   └── contract_test.go
├── realm/                   # 世界维度、区块生命周期与单 tick Mutation 事务（指南见 realm/AGENTS.md）
│   ├── state.go             # State/Dimension 权威世界状态
│   ├── mutation.go          # Mutation 单 tick 事务与 Commit
│   ├── persistence.go       # revision/持久化与卸载
│   ├── environment.go       # 流体/耕地/作物等环境推进
│   └── *_test.go            # mutation/state/persistence/environment 白盒测试
├── entity/                  # 玩家/伙伴/夜行者与玩法结算（指南见 entity/AGENTS.md）
│   ├── engine.go            # State 与实体权威集合
│   ├── api.go / tick.go     # 窄生命周期 API 与 TickInput/TickContext
│   ├── player.go / companion.go / hostile.go / actor.go
│   ├── crafting.go / container.go / furnace.go / mining.go / combat.go / drop.go / hunger.go / eating.go / sleep.go
│   ├── placement.go / world.go / engine_changes.go
│   └── *_test.go
└── runtime/                 # Engine、inbox、订阅与固定 tick 编排（指南见 runtime/AGENTS.md）
    ├── engine.go            # runtime 状态及唯一 realm/entity 组合
    ├── engine_step.go       # StepWithTunables 阶段顺序与单次 Mutation Commit
    ├── engine_subscription.go # 订阅/发布与阶段探针
    ├── entity_delegate.go   # 生命周期与查询的窄委派
    ├── tick_tunables.go     # 单 tick simulation/physics 参数束
    ├── engine_run.go / persistence.go
    └── *_test.go
```

## 子包所有权

| 子包 | 持有状态与职责 |
| --- | --- |
| `contract` | 跨边界命令、拒绝、区块 ingress 与 tick 输出等纯值 DTO；`packages/server/server` 与 `packages/shared/network` 消费该层值类型 |
| `realm` | 世界维度、区块生命周期、持久化 revision、流体/耕地/作物等环境状态与单 tick `realm.Mutation` 事务 |
| `entity` | 玩家、伙伴、夜行者、背包、容器、合成、战斗、掉落、睡眠与生命周期的唯一 owner；`BeginTick` 经 `TickInput` 按值接收 simulation/physics 参数及同一 `*realm.Mutation` |
| `runtime` | 只持 inbox、订阅、时钟、阶段探针与编排 scratch，恰好组合一个 `*realm.State` 和一个 `*entity.State`；公开生命周期/查询方法仅窄委派，固定 tick 编排是唯一跨子包权威入口 |

`contract` 不依赖 `realm`/`entity`/`runtime`；`realm` 不依赖 `entity`/`runtime`；`entity` 可依赖 `contract`/`realm` 与共享层 `packages/shared/tuning`；`runtime` 编排全部三者并装配 tuning 快照。`runtime.subscriptionState` 只保存命令序号、观察中心与 wanted 集合，不是实体会话镜像。`packages/server/sim` 成为仅含指导文档的目录，所有生产状态与行为分属四个子包。

## Mutation 与单次提交

- `realm.State` 拥有维度记录、队列、持久化 revision 与环境 scratch，由权威 tick 单写者独占，不设内部锁。
- 每个推进 tick 由 `runtime` 在同一个 `realm.State` 上打开唯一 `*realm.Mutation`；方块读取经该 `realm.State`，全部写入汇入该 mutation。`entity.State.BeginTick(TickInput, mutation)` 只在短命 `TickContext` 中借用二者，不复制世界或事务 owner。
- `Mutation.Record`/`Touch` 收集 `pendingChunkChanges`，`Commit` 在 runtime tick 尾部一次性推进 revision、压缩 section 并产出 `ChunkChangeBatch` 发布批次；不得另设平行通道或二次提交。
- 相同输入在同一 tick 内按区块与索引的确定性排序提交，保证 revision、持久化请求与发布批次在重放时一致。

## 单 tick 参数束

- `runtime.TickTunables` 按值携带一个 `tuning.Tunables` 与一个 `physics.Tunables`。两组活动快照彼此独立，各读取一次不构成跨组原子事务，`tuning` 不得因此依赖 `physics`。
- `packages/server/server` 的实际推进路径在 lifecycle/pause 早退后、聊天与伙伴任务前调用一次 `ActiveTickTunables`，将同一局部值传给 manager 与 `Engine.StepWithTunables`；关服最终推进同样只捕获并复用一束。`Engine.Step` 仅是直接调用方的兼容捕获 wrapper。
- runtime 从 simulation 值投影一次 `realm.EnvironmentConfig`，entity 显式接收两组值；权威 entity/runtime/server 路径不得在 tick 中重读任一 `ActiveTunables`，也不得调用隐式 `physics.Step`/`physics.SubmersionFlags` wrapper。

## 结算与事务规则

- 状态只在成功路径提交，相互依赖时先副本预演再同 tick 原子落地。
- 方块写入经 `realm.Mutation` 汇入当前 tick，由 `Commit` 统一推进 revision 与发布批次，不另设平行通道。
- 每 tick 工作必须有界且保持确定性顺序，磁盘/网络/模型调用经有界队列或快照离开热路径。
- `Engine.StepWithTunables` 串行组合固定阶段，新增阶段或写者先核对 `engine_step.go` 的顺序约束、订阅收敛点与最终发布边界。

## 依赖方向

子包依赖以 `packages/audit/audit/dependencies.py` 的 `allowed` 表为唯一真相；本包不得依赖 `packages/client/client`、`packages/client/render` 或具体 network transport，模拟只消费领域命令并产出权威结果。依赖方向单向且由 `packages/audit` 强制（契约见 `openspec/specs/repository-code-organization`）：

- 接受：`runtime` → `contract`/`realm`/`entity` 与 `packages/shared/tuning`；`entity` → `contract`/`realm` 与 `packages/shared/tuning`；`realm` → `core`/`fluid`/`world`；`contract` → `core`/`world`/`companion`/`physics`（`core`/`world`/`companion`/`physics` 均已迁入 `packages/shared`）。
- 拒绝：`contract` 依赖 `tuning`/`realm`/`entity`/`runtime`；`realm` 依赖 `contract`/`tuning`/`entity`/`runtime`；`entity` 依赖 `runtime`；子树出现未登记的新包；`runtime` 缺少对三个兄弟子包与 `packages/shared/tuning` 的必需编排边。
- 强制点：`TestInternalDependenciesAreOneWay` 以 `go list` 覆盖全仓内部包完整白名单；`TestSimSubpackageDependencyDirections` 与 `TestSimDependencyViolationsDetectDrift` 守住真实/合成依赖边；`TestSimAuthorityStateOwnershipStaysExplicit` 扫描 runtime 包变量及全部 holder，锁定 runtime/entity owner、窄订阅状态，并把唯一 mutation/commit 绑定到 `StepWithTunables` 的真实调用路径；`TestAuthorityTickTunablesStayExplicit` 守住活动快照捕获与显式传递。新增 owner 字段、子包或依赖边必须同步对应门禁。

## 定点验证与入口

按子包定点（分层纪律见 `docs/notes/test-quickstart.md`；涉 Rust 侧先 `make rust`）：

| 改动域 | 命令 |
| --- | --- |
| 子树全量 | `go test ./packages/server/sim/... -race -count=1` |
| `contract` 值类型 | `go test ./packages/server/sim/contract -race -count=1` |
| `tuning` 快照 | `go test ./packages/shared/tuning -race -count=1` |
| `realm` 事务与环境 | `go test ./packages/server/sim/realm -race -count=1` |
| `entity` 结算 | `go test ./packages/server/sim/entity -race -count=1` |
| `runtime` 编排 | `go test ./packages/server/sim/runtime -race -count=1` |
| 依赖边界 | `make archcheck` |

- 当前文档入口：`docs/notes/go-rust-division.md`、`docs/test-organization.md`。
- 子树根的 `packages/server/sim/AGENTS.md` 是目录地图与边界总纲，子包细节见各自 `AGENTS.md`；修改任一子包的行为、导出面或测试入口必须同步对应指南。

## 子包指南

- `contract/AGENTS.md`：值类型所有权与跨边界 DTO 纪律。
- `packages/shared/tuning/AGENTS.md`：Tunables 校验与快照边界（已随包上提共享层）。
- `realm/AGENTS.md`：State/Mutation 所有权与单次提交。
- `entity/AGENTS.md`：唯一实体 owner、`TickInput` 与 `*realm.Mutation` 注入。
- `runtime/AGENTS.md`：Engine 组合、`StepWithTunables` 顺序与发布边界。
