# entity 包：玩法结算与私有状态

`packages/server/sim/entity` 是玩家、伙伴、夜行者及其背包、容器、战斗、掉落与生命周期状态的唯一 owner，并持有全部实体玩法结算逻辑。

## 职责

- `State.sessions`、`State.companions` 与 `State.hostiles` 是实体权威集合；玩家背包/容器以及生成、重生、掉落、饥饿、氧气、战斗等状态只从这些集合可达，runtime 不复制或双写。
- tick 入口是 `State.BeginTick(TickInput, *realm.Mutation)`；`TickInput` 按值携带 `tuning.Tunables`、`physics.Tunables`、realm/时钟与只读 view，返回的短命 `TickContext` 暴露固定阶段所需的窄方法，不成为第二个 owner。
- 放置、挖掘、容器、合成、熔炉、掉落、战斗、饥饿、进食、睡眠、床/门交互等写入共享传入的 mutation；生产 entity 不读取任一全局 `ActiveTunables`，物理与浸没只调用显式 tunables API，也不导入 `runtime`。
- 通过 `contract` 返回值类型产出拒绝与成功结果，不直接发布网络或持久化。

## 依赖方向

- 允许：`packages/shared/core`、`packages/shared/world`、`packages/shared/companion`、`packages/shared/physics`、`packages/server/sim/contract`、`packages/server/sim/realm`、`packages/server/updates`（随机面 `Sampler` 纯函数哈希判定，变更 unified-block-updates-world-streaming 收敛）、`packages/shared/tuning`。
- 禁止：依赖 `packages/server/sim/runtime` 或 `packages/server/server`/`packages/client/client`/`packages/client/render`；禁止将 `runtime.Engine` 作为参数或返回值。
- 方向由 `packages/audit` 强制，合成测试注入 `entity → runtime` 必须被 `TestSimDependencyViolationsDetectDrift` 拒绝。

## 关键文件

- `engine.go`：`State`、实体权威集合与短命只读上下文。
- `api.go`：runtime 所需的窄生命周期与查询 API。
- `tick.go`：`TickInput`、`TickContext` 与阶段入口。
- `player.go`/`companion.go`/`hostile.go`/`actor.go`：各角色状态与推进。
- `crafting.go`/`container.go`/`furnace.go`/`mining.go`/`combat.go`/`drop.go`/`hunger.go`/`eating.go`/`sleep.go`/`bed.go`/`door.go`/`placement.go`/`world.go`：玩法结算实现。
- `helpers_test.go`/`stage_helpers_test.go`：包内白盒夹具与阶段 helper。
- `entity_test.go`/`gameplay_settlement_test.go`：所有权签名回归与结算定点。

## 结算纪律

- 状态只在成功路径提交，库存/耐久/产物/容器/方块相互依赖时先副本预演再同 tick 原子落地，拒绝不得留下部分结果。
- 世界写入一律经传入的 `*realm.Mutation`，调用前已由 `realm.State` 保证区块 Ready；世界读取优先经 `*realm.State` 的 `BlockAt`/`ReadyChunk`。
- 新增结算或写者时核对 simulation/physics 两组值都来自调用方同一个 tick bundle；两组值彼此独立，但当前 tick 内都不得重读全局。

## 定点验证

- `go test ./packages/server/sim/entity -race -count=1`
- 关联：`go test ./packages/shared/companion ./packages/shared/physics -race -count=1`
- 依赖边界：`make archcheck`
