# tuning 包：模拟可调参数

`packages/shared/tuning` 持有 `Tunables`、默认值、钳制与原子活动快照，是配置与模拟之间的唯一参数源。

## 职责

- 定义 `Tunables` 及其 `DefaultTunables`、`SetTunables`、`ActiveTunables`；字段与 `packages/shared/config` 的键名逐字对应，钳制逻辑自洽（`RegenIntervalTicks >=1`、`SpawnRadius` 区间等）。
- 活跃 simulation 快照以 `atomic.Pointer` 整体替换；并发读写必须只观察完整旧值或完整新值，由 `TestConcurrentSetAndActiveTunablesReturnWholeSnapshots` 钉住。
- server 实际推进 tick 在 lifecycle/pause 早退后由 `runtime.ActiveTickTunables` 捕获一次，直接 `Engine.Step` 调用则由兼容 wrapper 捕获。参数束还独立读取一次 `physics.Tunables`；两组值在当前 tick 内共同复用，但不构成跨组原子事务，`tuning` 仍不得依赖 `physics`。
- 不持有世界或玩法状态，仅依赖 `packages/shared/core` 常量。

## 依赖方向

- 允许：`packages/shared/core`。
- 禁止：依赖 `packages/server/sim/contract`/`realm`/`entity`/`runtime` 或 `packages/shared/world`/`packages/shared/physics` 等上层状态；禁止反向依赖。
- 方向由 `packages/audit` 强制，`tuning` 为叶子，任何对其上层包的依赖均为反向边。

## 关键文件

- `tunables.go`：默认值、钳制与原子快照。
- `tunables_test.go`：默认值回归、钳制与快照并发可见性。

## 定点验证

- `go test ./packages/shared/tuning -race -count=1`
- 关联：`go test ./packages/shared/config -race -count=1`（配置装配）
- 依赖边界：`make archcheck`
