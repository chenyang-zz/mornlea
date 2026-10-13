# contract 包：跨边界纯值 DTO

`packages/server/sim/contract` 持有权威模拟跨包传递的不可变值类型，不持有权威状态或行为逻辑。

## 职责

- 定义 `Command`、`RejectReason`、`Rejection`、`GeneratedChunk`/`AcquiredChunk`、`BlockChange`/`ChunkChangeBatch`、`TickResult`、`PlayerUpdate`/`CompanionUpdate`、`InventoryUpdate`/`FurnaceUpdate`/`ChestUpdate`/`CraftingUpdate`、`HostileMob`/`HostileAction`、`CompanionAction` 等跨边界 DTO。
- 全部字段为值语义，跨 goroutine 发送成功后视为不可变；不暴露可变世界引用。
- 仅使用 `packages/shared/core`、`packages/shared/world`、`packages/shared/companion`、`packages/shared/physics` 的值类型，不感知 `realm`/`entity`/`runtime`。

## 依赖方向

- 允许：`packages/shared/core`、`packages/shared/world`、`packages/shared/companion`、`packages/shared/physics`。
- 禁止：依赖 `packages/server/sim/realm`、`packages/server/sim/entity`、`packages/server/sim/runtime`、`packages/shared/tuning` 或其他模拟子包；禁止依赖 `packages/server/server`/`packages/client/client`/`packages/client/render`。
- 方向由 `packages/audit` 的 `allowed` 与 `simAllowedEdges` 强制，校验入口 `TestInternalDependenciesAreOneWay`、`TestSimSubpackageDependencyDirections`。

## 关键文件

- `contract.go`：全部 DTO 定义。
- `contract_test.go`：值形状与序列化回归。

## 定点验证

- `go test ./packages/server/sim/contract -race -count=1`
- 依赖边界：`make archcheck`
