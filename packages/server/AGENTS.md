# 服务端域 Go 模块指南

## 作用域

本文件补充仓库根 `AGENTS.md`，适用于 `packages/server/`（服务端域独立 Go 模块，
go.work 成员）。更具体的边界见各子目录 `AGENTS.md`：

- `packages/server/sim/AGENTS.md`（子树总纲；contract/realm/entity/runtime 各有局部指南）
- `packages/server/storage/AGENTS.md`（子树总纲；region/chunk/player/companion/hostile 各有局部指南）
- `packages/server/fluid/AGENTS.md`

## 包所有权

- `sim`（contract/realm/entity/runtime 四子包）持有服务端权威 tick、玩法结算和世界变更编排；根目录只保留指导文档，不含生产 Go Package。
- `storage` 持有世界、玩家、伙伴和夜行者的编码、迁移、恢复与磁盘生命周期。
- `server` 装配 Host、会话、权威模拟和持久化 worker；重 CPU、磁盘或网络工作不得阻塞权威 tick。
- `server/persistence` 单独持有四类存档的异步保存与 worker 生命周期，不反向依赖 `server` 根包或访问 Host/Server 私有状态。

## 依赖边界

- 模块生产代码只 require 兄弟单元 `packages/shared` 与 `packages/contracts`（`packages/audit/audit/boundaries.py` 强制）；包级依赖方向以 `packages/audit/audit/dependencies.py` 的 `allowed` 表为唯一真相，不要在指南中复制该表。新增包或依赖边必须先证明方向合理并同步架构门禁。
- go.mod 对 `packages/client` 的 require 是测试专用豁免边：server 的 Memory/TCP 集成测试以客户端镜像驱动会话（Go 的 require 无法按测试限定，模块层只能放行）；生产代码不得 import `packages/client` 的任何包，该禁令由 `packages/audit/audit/boundaries.py` 源码守卫强制，server 的兄弟 require 集恰为 {shared, contracts, client} 亦被精确断言钉住。
- engine C ABI 只能由 `packages/shared/nativeabi` 接触；领域包经既有 bridge 调用 Rust，不得直接引入 C ABI 或另建生产 fallback。

## 测试组织

测试与被测代码同目录并按关注点组织。共享 helper 的准入、命名和拆分规则遵循 `docs/test-organization.md`，不要新建平行 helper 中心。

## 定点验证

- 模拟子树示例：`go test ./packages/server/sim/... -race -count=1`；处理其他包时把路径替换为对应真实目录。
- 服务端装配：`go test ./packages/server/server -race -count=1`；存储子树：`go test ./packages/server/storage/... -race -count=1`。
- 依赖边界：`make archcheck`。
- 当前文档入口：`docs/notes/go-rust-division.md`、`docs/test-organization.md`。
