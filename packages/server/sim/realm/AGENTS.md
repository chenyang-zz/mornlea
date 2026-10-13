# realm 包：世界维度与单 tick 事务

`packages/server/sim/realm` 持有权威世界维度、区块生命周期、持久化 revision 与环境推进的全部状态，是方块写入的唯一事务所有者。

## 职责

- `State`/`Dimension` 拥有维度记录、revision、持久化与环境 scratch，由权威 tick 单写者独占。
- `Mutation` 收集单 tick 内全部 `pendingChunkChanges`，`Record`/`Touch` 汇入变更，`Commit` 一次性推进 revision、压缩 section 并产出有序 `ChunkChangeBatch`。
- 环境推进（流体、耕地湿度、作物、树苗生长、干耕地退化、树苗/火把/床支撑复核）仅通过同一 `*Mutation` 写入，预算、重扫与确定性排序由本包持有，不另设平行通道。
- 环境配置由 runtime 从当前 tick 的 simulation 值投影后按值传入；realm 不读取全局 tunables，也不感知独立的 physics 快照。

## 依赖方向

- 允许：`packages/shared/core`、`packages/shared/world`、`packages/shared/worldgen`、`packages/server/fluid`。
- 树苗生长的树形几何来自 `packages/shared/worldgen` 的 engine ABI 桥（生产数值只在 Rust），realm 只做空间校验、跨区块原子写入与变更登记。
- 禁止：依赖 `packages/server/sim/contract`/`tuning`/`entity`/`runtime` 或 `packages/server/server`/`packages/client/client`；`realm` 不反向依赖上层结算或编排。
- 方向由 `packages/audit` 强制，校验 `TestSimSubpackageDependencyDirections` 的真实树扫描与 `TestSimDependencyViolationsDetectDrift` 的合成 `realm → entity/runtime` 注入。

## 关键文件

- `state.go`：`State`/`Dimension`、`ChunkRecord` 生命周期与 `BlockAt`/`SetBlock`。
- `mutation.go`：`Mutation`、`pendingChunkChanges`、排序与单次 `Commit`。
- `persistence.go`：revision、脏检查、飞行中保存与卸载。
- `environment.go`：流体/耕地/作物等环境推进与预算。
- `*_test.go`：`mutation_test.go`/`persistence_test.go`/`environment_test.go`/`support_test.go` 定点覆盖事务与环境。

## Mutation 纪律

- 每 tick 仅打开一次 `State.NewMutation()`，全部写入者接收同一值；`Commit` 仅调用一次，提交前 `ChangedBlocks` 可读快照。
- 方块写入必须先经 `Dimension.SetBlock` 再 `Mutation.Record`，或直接由 `Mutation` 承载的确定性路径；不得绕过 `Mutation` 直接落盘。
- `Commit` 按 `core.ChunkKey` 与区块内 `uint32` 索引全序输出，保证重放一致与 revision 唯一。

## 定点验证

- `go test ./packages/server/sim/realm -race -count=1`
- 依赖边界：`make archcheck`
