# repository-code-organization Specification

## Purpose
为全仓 Go 文件提供可判定的职责审计与纯结构重组契约，确保文件或包边界变化不会改变任何既有外部行为、固定 artifact 或架构守卫语义。
## Requirements
### Requirement: 代码组织重构保持外部行为
系统 MUST 确保 Mornlea 身份切换不改变任何未获批准的外部行为、测试入口或固定 artifact。测试入口基线 MUST 是 Task 7 初始 HEAD 持久化的清单，该 HEAD 已包含 Tasks 4–6 新增的数据迁移和命令路由测试。

#### Scenario: 身份切换只改变获批测试入口
- **GIVEN** Task 7 初始 HEAD 已持久化全部 Test、Benchmark 与 Fuzz 入口
- **WHEN** 完成原子身份切换
- **THEN** MUST 仅有以下 6 项重命名：`TestMCGodHasNoGraphicsDependencies` → `TestMornleaServerHasNoGraphicsDependencies`、`TestMcgoUsesLoginStreamsInsteadOfAttachedServerEndpoints` → `TestMornleaUsesLoginStreamsInsteadOfAttachedServerEndpoints`、`TestMcgoBenchmarkTCPPathUsesTheSharedLoginStateMachine` → `TestMornleaBenchmarkTCPPathUsesTheSharedLoginStateMachine`、`TestMCGodProcess` → `TestMornleaServerProcess`、`TestMCGodProcessReleasesWorldLockAfterSIGTERM` → `TestMornleaServerProcessReleasesWorldLockAfterSIGTERM`、`TestMCGodProcessSaveFailureExitsNonzero` → `TestMornleaServerProcessSaveFailureExitsNonzero`
- **AND** MUST 仅新增 `TestMornleaCurrentIdentity`
- **AND** 其余 Test、Benchmark、Fuzz 入口与 Task 1 后冻结的 fixture、golden 和性能 baseline MUST 保持不变
- **AND** benchmark 与 `perfcheck` 的性能数值及既有阈值 MUST 只保存记录且不得改变退出状态
- **AND** 只有报告结构、身份/provenance、真实 overflow、数据丢失、I/O 错误和非数值命令失败 MUST 阻断

#### Scenario: Apple M2 固定主线的同环境视觉失败不掩盖身份漂移
- **GIVEN** `system_profiler SPHardwareDataType` 的精确 Chip 为 `Apple M2`，且原始 Task 1 `origin/main` 仅有 `materials-showcase` 最大通道差 1、26 个差异像素（0.0113%）与 `oak-grove` 最大通道差 47、10 个差异像素（0.0043%）两个精确已知失败
- **WHEN** 原始主线与 Mornlea 分支在各自隔离 HOME 下运行同一非更新 capture
- **THEN** 两边 10 个场景 PNG 与上述两个失败的 actual/diff MUST 逐字节一致
- **AND** 两边 MUST 仅有上述两个失败且摘要完全一致，其余 8 个场景 MUST 通过 tracked golden
- **AND** 此裁决 MUST NOT 修改或放宽 golden、阈值、capture 代码或其他视觉失败

#### Scenario: 非 Apple M2 固定主线同环境视觉结果不漂移
- **GIVEN** `system_profiler SPHardwareDataType` 的精确 Chip 不是 `Apple M2`
- **WHEN** 原始 Task 1 `origin/main` 与 Mornlea 分支在各自隔离 HOME 下运行同一非更新 capture
- **THEN** 两边 10 个场景 PNG MUST 逐字节一致，且两次 `visual-check` MUST 退出 0
- **AND** 两边都 MUST 不产生 `*-actual.png` 或 `*-diff.png`
- **AND** 此裁决 MUST NOT 修改或放宽 golden、阈值或 capture 代码

### Requirement: 架构守卫不依赖单一源文件位置
架构守卫 MUST 对完整职责文件集合执行原有检查，不得绑定单一固定源文件位置。

#### Scenario: 同一职责分布到多个文件
- **GIVEN** 一个包内职责被拆到多个生产 Go 文件
- **WHEN** 运行架构守卫
- **THEN** 守卫 MUST 扫描完整职责文件集合并继续拒绝旧路径

### Requirement: 全仓 Go 文件完成职责审计
基线中的全部生产和测试 Go 文件 MUST 获得且仅获得一种职责审计结论。

#### Scenario: 文件无需修改但职责单一
- **GIVEN** 基线中的任意生产或测试 Go 文件
- **WHEN** 完成其所属包的审计任务
- **THEN** 该文件 MUST 被判定为保留、同包拆分、提取新包或删除之一

#### Scenario: 主线同步后的完整审计
- **GIVEN** `37cdb3e` 中 `cmd/` 与 `internal/` 下的 412 个 Go 文件
- **WHEN** 完成 Task 20 的主线同步审计
- **THEN** 36 个同包拆分、2 个提取到唯一新包 `internal/render/hud`、0 个删除、374 个保留的结论 MUST 互斥且合计为 412
- **AND** 不得新增其他包、恢复拆分前旧大文件或以更新 golden/baseline 掩盖差异

### Requirement: TCP transport 具有单向包边界

仓库 MUST 将 TCP listener、dial、stream、deadline 和 socket 生命周期实现置于
`packages/shared/network/tcp`。`packages/shared/network/tcp` MUST 依赖
`packages/shared/network`，而 `packages/shared/network` MUST NOT 依赖
`packages/shared/network/tcp`。根包 MUST 保留登录和应用装配使用的共享
packet stream 接口。

#### Scenario: 依赖图接受 TCP 子包
- **GIVEN** 仓库包含 `packages/shared/network/tcp`
- **WHEN** 架构依赖检查枚举全部内部包
- **THEN** 它 MUST 接受 `packages/shared/network/tcp -> packages/shared/network`
- **AND** 它 MUST 拒绝从 `packages/shared/network` 到 `packages/shared/network/tcp` 的任何反向依赖

#### Scenario: TCP 构造器由子包消费
- **GIVEN** 应用或测试需要创建 TCP listener 或 dial TCP stream
- **WHEN** 它针对整理后的仓库编译
- **THEN** 它 MUST 经由 `packages/shared/network/tcp` 解析构造器
- **AND** 返回值 MUST 满足共享根包的 listener 或 packet stream 接口

### Requirement: Transport 包整理保持传输行为

仓库 MUST 在移动 TCP 实现文件时保持既有 Memory/TCP packet、登录、关闭、背压、
deadline、peer-address、校验和错误语义。该整理 MUST NOT 改变 wire bytes、协议
状态机转换或测试函数名。

#### Scenario: Memory 与 TCP 保持相同登录契约
- **GIVEN** 客户端和服务端使用 Memory transport 或 TCP transport
- **WHEN** 它们执行既有 handshake、登录和 Play packet 流
- **THEN** 两种 transport MUST 继续使用相同的登录状态机和 packet 校验契约
- **AND** packet 值和登录结果 MUST 保持不变

#### Scenario: 既有 transport 测试仍可寻址
- **GIVEN** TCP white-box 测试已迁入 TCP 子包
- **WHEN** 整理后的仓库运行包测试
- **THEN** 每个既有 TCP 测试函数和子测试标签 MUST 保持存在且未重命名
- **AND** Memory、codec、packet 和登录测试入口 MUST 保持存在

### Requirement: 客户端命令按功能域分包

仓库 MUST 将 `packages/client/cmd/mornlea` 组织为薄 `package main`（CLI
解析与装配入口）加 `app`、`capture`、`benchmark`、`devcapture` 四个
功能域子包。依赖方向 MUST 为：main → app/capture/benchmark/devcapture；
capture、benchmark 与 devcapture → app。app MUST NOT 导入 capture、
benchmark 或 devcapture；capture、benchmark 与 devcapture MUST NOT 相互
导入。

#### Scenario: 依赖图接受客户端子包并拒绝反向边

- **GIVEN** 仓库包含 `packages/client/cmd/mornlea/app`、
  `packages/client/cmd/mornlea/capture`、
  `packages/client/cmd/mornlea/benchmark` 与
  `packages/client/cmd/mornlea/devcapture`
- **WHEN** 架构依赖检查枚举全部包
- **THEN** MUST 接受 main → app、main → capture、main → benchmark、
  main → devcapture 与 capture/benchmark/devcapture → app
- **AND** MUST 拒绝 app → capture、app → benchmark、app → devcapture 与
  capture/benchmark/devcapture 之间任何方向的依赖边

#### Scenario: 跨包访问经由消费端接口

- **GIVEN** capture 或 benchmark 需要读写 app 域状态（菜单、设置、面板、
  相机、渲染器等）
- **WHEN** 其编译 against 迁移后的仓库
- **THEN** 所需能力 MUST 通过各自包内定义的接口表达，由 `app.Application`
  实现
- **AND** app 包 MUST NOT 为 capture/benchmark 导出全量内部字段

### Requirement: 客户端分包保持测试入口集合

迁移 MUST 保持全部既有测试入口可寻址：测试函数名与 `t.Run` 标签逐一不变，
三个子包 `go test -list` 入口并集 MUST 等于迁移前 `cmd/mornlea` 单包集合。
分包后 MUST 能对单个子包运行测试而不编译执行其他子包的重型测试。

#### Scenario: 入口并集与基线一致

- **GIVEN** 迁移前已持久化 `go test ./cmd/mornlea -list '.*'` 全量快照
- **WHEN** 分包完成并对 `./cmd/mornlea/...` 各包取 `-list` 并集
- **THEN** Test、Benchmark、Fuzz 入口集合 MUST 与快照一致
- **AND** 子测试标签 MUST 逐一不变

#### Scenario: app 层迭代不为重型测试付费

- **GIVEN** 开发者修改 app 包内输入或 HUD 逻辑
- **WHEN** 运行 `go test ./cmd/mornlea/app -race`
- **THEN** MUST NOT 编译或执行 capture golden 抓帧与 benchmark 真实 renderer
  场景测试
- **AND** `go test ./cmd/mornlea/capture` 与
  `go test ./cmd/mornlea/benchmark` MUST 可单独定点运行

### Requirement: golden 资产随 capture 子包迁移且视觉结果不变

视觉基线 MUST 统一位于仓库根 `testdata/visual-golden/`（`world/` 与
`ui/` 两个子目录），Go 世界场景基线由
`packages/client/cmd/mornlea/capture` 的 golden 目录常量引用，前端 UI
部件基线由 `packages/engine/crates/mornlea_client/frontend` 的视觉
harness 引用。基线 PNG 内容 MUST 逐字节不变，capture 子包 MUST NOT 再
持有自有 golden 目录。

#### Scenario: 视觉门禁在迁移后保持全绿

- **GIVEN** golden PNG 位于 `testdata/visual-golden` 且两侧引用路径已同步
- **WHEN** 运行 `make visual-check` 与 `make frontend-visual-check`
- **THEN** 全部场景 MUST 通过 tracked golden 且退出 0
- **AND** MUST NOT 产生 `*-actual.png` 或 `*-diff.png`
- **AND** MUST NOT 修改、放宽或重新生成任何 golden 图像

### Requirement: 架构守卫覆盖客户端子包子树

针对 `packages/client/cmd/mornlea` 生产源码的字符串级架构守卫（登录路径
守卫、benchmark TCP 路径守卫等）MUST 扫描该完整子树（含子包），不得因
源码迁入子包而丢失覆盖。

#### Scenario: 源码守卫随子包继续生效

- **GIVEN** 登录装配与 benchmark TCP 路径的生产源码已迁入子包
- **WHEN** 运行对应源码守卫
- **THEN** 守卫 MUST 继续要求 `network.NewMemoryStreamPair`、
  `network.LoginClient`、`networktcp.ListenTCP(` 等既有模式
- **AND** 守卫 MUST 继续拒绝 `server.NewEmbedded(`、`server.New(` 等违规模式

### Requirement: 存储世界按域拆分子包

仓库 MUST 将 `packages/server/storage` 组织为编排根包加
`storagedef`、`region`、`chunk`、`player`、`companion`、`hostile`、`passive` 七个
子包。根包 MUST 保留 `Store`/`WorldStore` 等接口与 disk/memory/world_files/
backup/metadata/chunk_keys 编排；`storagedef` MUST 只承载
`ErrCorrupt`/`ErrFutureVersion` 跨域哨兵；`region` MUST 承载 region 格式
原语（superblock/bank 编解码、扇区空间分配）与 `RegionKey`/`RegionFor`；
`chunk` MUST 承载 chunk 信封编解码、迁移、chunk 值类型与 region 记录层
容器（现 `*region` 及其读写、压缩、崩溃恢复）；`player`/`companion`/
`hostile`/`passive` MUST 各自承载对应实体的 codec、迁移、类型、域测试与版本化
bin fixture。

#### Scenario: 依赖图接受存储子包布局并拒绝反向边

- **GIVEN** 仓库包含 `packages/server/storage` 的全部七个子包
- **WHEN** 架构依赖检查枚举全部内部包
- **THEN** MUST 接受根包到 storagedef 叶子与六个域子包的依赖边
- **AND** MUST 接受 chunk → region、实体域 → storagedef、region → storagedef
  等已登记的消费边
- **AND** MUST 拒绝任何子包指向根包的反向依赖与子包之间未登记的相互依赖
  （chunk → region 除外）

#### Scenario: region 记录层随 chunk 包且 region 包只收格式原语

- **GIVEN** `region.go` 的读写路径直接调用 chunk 信封编解码并经手 chunk 值
  类型，属于 chunk 的记录层容器
- **WHEN** 针对拆分后的仓库编译
- **THEN** 记录层容器（现 `*region` 及其 open/load/save/sync/close/compact
  入口与文件注入钩子）MUST 位于 `chunk` 包，并由 chunk 包导出容器类型供根包
  缓存编排
- **AND** `region` 包 MUST 只承载格式原语与 `RegionKey`/`RegionFor`，MUST NOT
  承载 chunk 信封编解码或容器文件编排
- **AND** 根包 MUST 继续持有容器缓存与 `ChunkKeys` 编排

### Requirement: 存储子包依赖方向单向

存储子包的依赖方向 MUST 单向且由架构依赖检查登记：根包 →
region/chunk/player/companion/hostile/passive 六个域子包与 storagedef 叶子（经错误
别名消费哨兵）；chunk → {region, storagedef, shared/core, shared/world}；
player/companion/hostile/passive → {storagedef, shared/core, shared/world}；
region → {storagedef, shared/core}。companion 存储域 MUST 经既有
`shared/companion` 边访问伙伴领域类型，不得新增其他子包间依赖。

#### Scenario: archcheck 登记并强制存储依赖边

- **GIVEN** `packages/audit` 的依赖白名单已登记存储子包允许边
- **WHEN** 架构依赖检查枚举全部内部包
- **THEN** 实际存在的包间导入边 MUST 全部落在允许边集合内
- **AND** 向任何子包注入未登记依赖边（如 player → chunk、region → chunk）
  MUST 被拒绝

#### Scenario: companion 存储域沿用既有伙伴领域边

- **GIVEN** companion 存储域的 codec 引用伙伴领域类型
- **WHEN** 拆分完成并枚举依赖边
- **THEN** `packages/server/storage/companion` MUST 仅经既有的
  `packages/shared/companion` 边访问伙伴领域类型
- **AND** 该边方向与拆分前保持一致

### Requirement: 存储别名再导出保持消费面与格式语义

拆分 MUST 保持消费方生产代码零改动：根包 MUST 以类型别名与错误值别名再导出
全部迁出符号，既有 `storage.X` 引用 MUST 继续以同一名称与类型身份解析；
`ErrCorrupt`/`ErrFutureVersion` 别名 MUST 与 `storagedef` 定义为同一错误值；
消费方测试因版本化 bin fixture 随所属域包迁移所需的 fixture 相对路径更新
MUST NOT 视为消费方改动。拆分 MUST NOT 改变 schema 号、迁移表、错误消息、
原子替换路径或格式字节；版本化 bin fixture MUST 逐字节随所属域包迁移。
单元化搬迁时，消费方（`packages/server/server`、
`packages/client/cmd/mornlea` 的 app/benchmark、
`packages/server/cmd/mornlea-server` 等）MUST 仅发生 import path 改写，
不发生符号或逻辑改动。

#### Scenario: 消费方符号继续可寻址

- **GIVEN** 消费方以 `storage.X` 引用迁出符号
- **WHEN** 拆分与单元化搬迁完成并编译全仓
- **THEN** 全部既有 `storage.X` 符号 MUST 继续以同一名称与类型身份解析
- **AND** 消费方生产代码 MUST 仅出现 import path 改写
- **AND** 消费方测试仅因 fixture 随域包迁移更新的 fixture 相对路径 MUST NOT
  视为消费方改动

#### Scenario: 错误哨兵身份与 fixture 字节不变

- **GIVEN** 既有测试以 `errors.Is` 匹配 `ErrCorrupt`/`ErrFutureVersion` 并断言
  错误消息文本
- **WHEN** 拆分完成并运行存储测试
- **THEN** 哨兵别名 MUST 与 `storagedef` 定义为同一错误值且错误消息逐字节
  不变
- **AND** 全部版本化 bin fixture MUST 逐字节不变且继续被对应域测试加载

### Requirement: 存储分包保持测试入口集合

拆分 MUST 保持全部既有测试入口可寻址：七个包 `go test -list` 入口并集 MUST
等于拆分前 `packages/server/storage` 单包集合，测试函数名与 `t.Run` 标签
逐一不变。拆分后 MUST 能对单个域包定点运行测试而不编译执行其他域的测试。

#### Scenario: 入口并集与基线一致

- **GIVEN** 拆分前已持久化 `go test ./internal/storage -list '.*'` 全量快照
- **WHEN** 拆分与单元化完成并对 `./packages/server/storage/...` 各包取
  `-list` 并集
- **THEN** Test、Benchmark、Fuzz 入口集合 MUST 与快照一致
- **AND** 子测试标签 MUST 逐一不变

#### Scenario: 实体域迭代不为其他域付费

- **GIVEN** 开发者修改单个实体域的 codec 或迁移逻辑
- **WHEN** 运行 `go test ./packages/server/storage/<域> -race`
- **THEN** MUST NOT 编译或执行其他域包的测试
- **AND** 每个域包 MUST 可单独定点运行

### Requirement: 网络按域拆分子包

仓库 MUST 将 `packages/shared/network` 组织为编排根包加
`packages/shared/network/protocol`、`packages/shared/network/codec` 两个
子包；`packages/shared/network/tcp` MUST 保持现状不动。根包 MUST 保留会话
与传输编排（共享 packet stream 接口、登录状态机、Memory transport、
`ClientEndpoint`/`ServerEndpoint` 与 `ErrClosed`）；`protocol` MUST 承载
密封 `ClientPacket`/`ServerPacket` 接口、全部协议消息 DTO 与 `Validate`、
冻结包 ID 表 `registry` 与区块 wire DTO `snapshot`，并承载协议级校验
`ValidateDecodedClientWirePacket`（含 Handshake/Login 放行路径）；`codec`
MUST 承载 `Codec` 门面与双向分发、编码原语与帧封装。

#### Scenario: 依赖图接受网络子包布局并拒绝反向边

- **GIVEN** 仓库包含 `packages/shared/network` 的全部子包
- **WHEN** 架构依赖检查枚举全部内部包
- **THEN** MUST 接受根包到 protocol 与 codec 的依赖边及既有
  shared/core 边
- **AND** MUST 接受 codec → protocol、protocol → shared/companion、
  tcp → network 等已登记的消费边
- **AND** MUST 拒绝任何子包指向根包的反向依赖、protocol → codec 边与根包
  到 tcp 的依赖

#### Scenario: 密封接口与冻结包 ID 表同处 protocol 包

- **GIVEN** `ClientPacket`/`ServerPacket` 以 unexported marker 密封且全部
  message DTO 与 snapshot 实现 marker，`registry` 与
  `CommandRejected.Validate` 双向引用包 ID 映射
- **WHEN** 针对拆分后的仓库编译
- **THEN** 密封接口、全部 message DTO、`ChunkSnapshot` wire DTO 与冻结包
  ID 表 MUST 位于同一 `protocol` 包
- **AND** `ValidateDecodedClientWirePacket` MUST 位于 `protocol` 包且
  Handshake/Login 放行路径原样保留
- **AND** 根包 MUST 继续持有登录状态机、Memory transport 与共享 stream
  接口

### Requirement: 网络子包依赖方向单向

网络子包的依赖方向 MUST 单向且由架构依赖检查登记：根包 → {protocol,
codec, shared/core}；protocol → {shared/core, shared/companion}；
codec → {protocol, shared/core}；tcp → 根包（既有边不动）。根包的
companion 边 MUST 随 companion 消息文件移交 protocol 后移除；protocol 与
codec 之间 MUST 仅存在 codec → protocol 单向边；子包 MUST NOT 依赖根包。

#### Scenario: archcheck 增量登记并强制网络依赖边

- **GIVEN** `packages/audit` 的依赖白名单已按单元路径登记网络子包允许边
- **WHEN** 架构依赖检查枚举全部内部包
- **THEN** 实际存在的包间导入边 MUST 全部落在允许边集合内
- **AND** 向任何子包注入未登记依赖边（如 protocol → codec、codec → 根包）
  MUST 被拒绝

#### Scenario: companion 协议域沿用既有伙伴领域边

- **GIVEN** companion 消息文件的协议域代码引用伙伴领域类型
- **WHEN** 拆分完成并枚举依赖边
- **THEN** `packages/shared/network/protocol` MUST 仅经既有的
  `packages/shared/companion` 边访问伙伴领域类型
- **AND** 根包 MUST NOT 再保留 companion 边，该边方向与拆分前保持一致

### Requirement: 网络别名再导出保持消费面与协议语义

拆分 MUST 保持消费方生产代码零改动：根包 MUST 以类型别名、常量、错误与
var 函数别名再导出全部迁出符号，既有 `network.X` 引用 MUST 继续以同一
名称与类型/错误/常量身份解析；类型别名 MUST 保持方法集（`msg.Validate`
继续可用）；留守根包的会话与传输 API MUST 保持原生定义。拆分 MUST NOT
改变 wire 字节、包 ID、协议语义、校验规则、错误语义、Memory/TCP 行为或
`ProtocolVersion`；`chunk-snapshot-v1.bin` fixture MUST 逐字节随 codec 包
迁移。Memory 与 TCP transport MUST 继续使用相同的登录状态机和 packet
校验契约。单元化搬迁时，消费方（`packages/server/server`、
`packages/client/client`、`packages/server/sim`、
`packages/client/cmd/mornlea` 各子包、`packages/server/cmd/mornlea-server`
与 `packages/shared/network/tcp`）MUST 仅发生 import path 改写。

#### Scenario: 消费方符号继续可寻址

- **GIVEN** 消费方以 `network.X` 引用迁出符号
- **WHEN** 拆分与单元化搬迁完成并编译全仓
- **THEN** 全部既有 `network.X` 符号 MUST 继续以同一名称与类型/错误/常量
  身份解析
- **AND** 消费方生产代码与 tcp 生产代码 MUST 仅出现 import path 改写

#### Scenario: 协议语义与 wire 字节不变

- **GIVEN** 既有 golden、fuzz 与 `chunk-snapshot-v1.bin` fixture 测试断言
  wire 字节、包 ID 与错误语义
- **WHEN** 拆分完成并运行网络测试
- **THEN** wire 字节、包 ID、校验规则与错误语义 MUST 逐项不变
- **AND** fixture MUST 逐字节不变且继续被 codec 包测试加载
- **AND** Memory 与 TCP transport 的既有 handshake、登录与 Play packet
  流行为 MUST 保持不变

### Requirement: 网络分包保持测试入口集合

拆分 MUST 保持全部既有测试入口可寻址：根包与 protocol/codec/tcp 各包
`go test -list` 入口并集 MUST 等于拆分前根包与 tcp 集合，测试函数名与
`t.Run` 标签逐一不变。拆分后 MUST 能对单个子包定点运行测试而不编译执行
其他子包的测试。

#### Scenario: 入口并集与基线一致

- **GIVEN** 拆分前已持久化 `go test ./internal/network/... -list '.*'`
  全量快照（根包 164 + tcp 33 = 197 项）
- **WHEN** 拆分与单元化完成并对 `./packages/shared/network/...` 各包取
  `-list` 并集
- **THEN** 剥离快照的 `#` 分节行与空行后，Test、Benchmark、Fuzz 入口集合
  MUST 与快照逐名一致
- **AND** 子测试标签 MUST 逐一不变

#### Scenario: 单域迭代不为其他域付费

- **GIVEN** 开发者修改 protocol 或 codec 子包的协议消息或编解码逻辑
- **WHEN** 运行 `go test ./packages/shared/network/<子包> -race`
- **THEN** MUST NOT 编译或执行其他子包的测试
- **AND** 每个子包 MUST 可单独定点运行

### Requirement: Server persistence has a one-way package boundary

仓库 MUST 将世界区块与 metadata、玩家、伙伴和夜行者的存档加载、观察、异步
保存、重试、flush 及 worker 生命周期置于
`packages/server/server/persistence`。根 `packages/server/server` MUST
保留 Host、Server、权威 tick、登录、会话、发布和关服编排，并可以依赖该
子包；`packages/server/server/persistence` MUST NOT 反向依赖根包。

#### Scenario: 架构守卫接受唯一允许的父子依赖

- **GIVEN** 仓库包含 `packages/server/server` 与
  `packages/server/server/persistence`
- **WHEN** 架构依赖检查枚举全部内部生产包
- **THEN** MUST 接受 `packages/server/server -> packages/server/server/persistence`
- **AND** MUST 拒绝 `packages/server/server/persistence -> packages/server/server` 或任何未登记的内部依赖

#### Scenario: 持久化职责不留在根包

- **GIVEN** 世界、玩家、伙伴和夜行者都需要其既有存档生命周期
- **WHEN** 根 Server 或 Host 执行 tick、登录、登出或关服
- **THEN** 对应存档生命周期 MUST 由 `packages/server/server/persistence` 的单一所有者执行
- **AND** 根包 MUST NOT 保留第二套保存队列、重试状态或 worker 实现

### Requirement: Server persistence extraction preserves contracts

这次结构迁移 MUST 保持已有的根包调用面、存档载荷、时序和失败语义。它 MUST NOT
改变 autosave、retry、backpressure、channel 容量、worker 数量、flush/close 顺序、
协议、schema、Rust ABI 或 client ABI。

#### Scenario: 既有持久化工作流保持可观察行为

- **GIVEN** 玩家或世界、伙伴、夜行者存档处于保存、重试、背压或关服 flush 状态
- **WHEN** 迁移后的 Host 或 Server 运行相同工作流
- **THEN** MUST 产生与迁移前相同的成功、失败、重试、背压和关闭结果
- **AND** 已有 `server` 包公开 API 与错误哨兵 MUST 继续可由既有调用方使用

#### Scenario: 测试入口和子测试标签保持可寻址

- **GIVEN** 迁移前已保存 `internal/server` 的 Test、Benchmark、Fuzz 名称及被迁移测试的 `t.Run` 标签
- **WHEN** 迁移完成后分别枚举 `internal/server` 与 `internal/server/persistence`
- **THEN** 默认构建的两个包入口并集 MUST 等于不可变迁移前基线，加且仅加 `TestFlushFrozenFailureReleasesUnsentPendingJobsForLaterRetry`、`TestPersistentServerGoroutineMatcherIncludesWorldSaveWorker`、`TestShutdownFlushSerializesPublicEngineReads` 和 `TestShutdownWorkerTimeoutDrainsReadySaveFailure`
- **AND** `TestPublicPersistenceContracts` MUST 排除在默认构建并集外，并以其单独的 `persistence_contract` 命令验证
- **AND** 被迁移测试的 `t.Run` 标签 MUST 逐项保持不变

### Requirement: 权威模拟具有五个单向子包

（标题沿用拆分时五子包的历史命名；单元化后模拟子包为四个，调参值对象
上提共享层。）仓库 MUST 将权威模拟组织为 `packages/server/sim/contract`、
`packages/server/sim/realm`、`packages/server/sim/entity` 与
`packages/server/sim/runtime` 四个子包，调参值对象 MUST 位于
`packages/shared/tuning` 供模拟与配置双侧消费。根
`packages/server/sim` 不得保留生产 Go package、类型别名、转发函数或
兼容 facade。所有内部调用方 MUST 直接导入其所消费值或行为的所属子包。

`contract` MUST 不依赖 `realm`、`entity` 或 `runtime`；`realm` MUST
不依赖 `entity` 或 `runtime`；`entity` MAY 依赖 `contract` 与 `realm`，
但 `runtime` 是唯一允许同时编排其余三者与 `shared/tuning` 的模拟包。
所有直接依赖 MUST 由架构检查登记并验证。

#### Scenario: 架构检查接受约定方向并拒绝反向边
- **GIVEN** 仓库包含四个权威模拟子包与 `packages/shared/tuning`
- **WHEN** 架构依赖检查枚举全部内部生产包
- **THEN** 它 MUST 接受 `runtime` 对 `contract`、`realm`、`entity` 与
  `shared/tuning` 的编排依赖
- **AND** 它 MUST 拒绝 `contract` 对上层模拟包的依赖、`realm` 对
  `entity` 或 `runtime` 的依赖，以及 `entity` 对 `runtime` 的依赖

#### Scenario: 迁移后的调用方不经过根包兼容层
- **GIVEN** 既有 server、config 与客户端装配调用方已迁移
- **WHEN** 仓库编译其内部生产包
- **THEN** 它们 MUST 直接解析 `runtime.Engine`、`contract` 值或
  `shared/tuning` 值
- **AND** 不得存在可编译的生产 `packages/server/sim` facade、类型别名或
  转发 API

### Requirement: 子包边界保持单一权威提交路径

子包整理 MUST 保持服务端为世界与玩家状态的唯一权威。每个权威 tick 的区块写入
MUST 经同一个 realm-owned 事务收敛，并在一次提交中维持既有 revision、持久化、流体
入队与发布批次语义。`runtime` MUST 保持既有串行 tick 阶段顺序、goroutine 所有权、
有界工作和快照边界。每个实际推进模拟的 server tick MUST 在处理聊天、伙伴任务与
runtime 阶段前捕获一个不可变参数束；该参数束 MUST 分别包含一次读取所得的 simulation
tunables 与 physics tunables，并在当前 tick 的 server manager、runtime、entity、realm
和 physics 路径中按值复用，不得在阶段中再次读取任一全局活动快照。两组独立活动快照的
捕获不构成跨参数组事务，也不得为此引入 `tuning` 对 `physics` 的依赖。

#### Scenario: 同一权威输入保留结算与发布结果
- **GIVEN** 固定世界、权威输入、异步区块结果与 tunable 快照
- **WHEN** 子包整理后的运行时完成一个或多个权威 tick
- **THEN** 它 MUST 产生与整理前相同的接受或拒绝结果、状态发布、区块 revision、方块变更顺序和持久化请求
- **AND** 同一 tick 的相关库存、容器、掉落物与方块变更 MUST 继续原子提交或原子拒绝

#### Scenario: 模拟边界不改变持久化或线上契约
- **GIVEN** 子包整理前后的相同世界与玩家数据
- **WHEN** 它们经 Memory 或 TCP 登录并执行既有模拟路径
- **THEN** wire bytes、协议状态机、存档编码、schema、engine ABI、client ABI、benchmark scenario 与视觉 golden MUST 保持不变

#### Scenario: 单个权威 tick 复用同一参数束
- **GIVEN** server tick 入口已经分别捕获 simulation tunables 与 physics tunables
- **AND** 任一全局活动快照在该 tick 的后续阶段中发生更新
- **WHEN** server manager、runtime、entity、realm 与 physics 完成当前 tick
- **THEN** 当前 tick 的交互距离、眼高、实体运动、浸没和环境推进 MUST 全部使用入口捕获值
- **AND** 下一次实际推进模拟的 tick MAY 观察更新后的活动快照
- **AND** 暂停早退不得无意义地捕获参数，shutdown 的最终模拟 tick MUST 对其 manager 与 runtime 阶段复用同一参数束

### Requirement: 子包迁移保持测试入口

迁移前持久化的 `packages/server/sim` Test、Benchmark、Fuzz 入口与
`t.Run` 标签 MUST 在迁移后仍可从 `packages/server/sim/...` 子包集合中
逐项取得（`tuning` 相关入口随包落在 `packages/shared/tuning`）。白盒
测试 MUST 与其生产所有者一同迁移，不得为访问旧私有状态而增加生产导出
API。

#### Scenario: 子包测试入口并集等于迁移前清单
- **GIVEN** 迁移前已保存 `internal/sim` 的 Test、Benchmark、Fuzz 入口和
  `t.Run` 标签清单
- **WHEN** 迁移后枚举 `packages/server/sim/...` 与
  `packages/shared/tuning` 的测试入口
- **THEN** 子包入口并集与每个 `t.Run` 标签 MUST 与迁移前清单完全一致
- **AND** 每个子包 MUST 能独立运行其 focused race 测试而不依赖已删除的
  根 `sim` package

### Requirement: 寻路实现是独立内部包

仓库 MUST 将可复用的有界寻路值与算法作为独立包提供，并在提取时保持既有
寻路行为与测试入口不变。

#### Scenario: pathfind owns reusable pathfinding values and algorithms
- **GIVEN** the repository builds its internal packages
- **WHEN** callers construct a path grid or execute a path search
- **THEN** they MUST use `packages/shared/pathfind` values and functions
- **AND** `packages/shared/pathfind` MUST directly depend only on
  `packages/shared/core`

#### Scenario: pathfinding extraction preserves existing behavior
- **GIVEN** the pre-extraction companion and server test entry inventory
- **WHEN** the package extraction is complete
- **THEN** existing Test, Benchmark, Fuzz names and `t.Run` labels MUST remain available
- **AND** path results, revision validation, errors and bounded resource behavior MUST remain unchanged

### Requirement: 伙伴 Agent 服务具有单向分层边界

仓库 SHALL 在 `packages/agent` workspace 提供独立 Python 服务（成员 `harness/`、`extension-api/` 与未发布的 `app/gateway`），并把可发布的 Agent harness/domain 与 FastAPI app、模型适配、MCP adapter、memory storage 分离。依赖方向 MUST 是 app/CLI → harness/domain 与 storage adapter → domain；domain 和 graph factory MUST NOT依赖 FastAPI、Uvicorn、Go server 包或 Mornlea 世界实现。harness 内部 import MUST NOT 使用 `mornlea` 前缀（`harness.*` / `extension_api.*` / `app.*`）。Go 生产代码 MUST 只通过 Agent HTTP contract 与 MCP handler 交互，MUST NOT通过 Python FFI、shell 或嵌入解释器调用服务。

#### Scenario: graph harness 可脱离 HTTP 测试

- **GIVEN** 测试提供 fake model、fake MCP 与临时 memory adapter
- **WHEN** 直接构造并运行 Planner 或 Dialogue graph
- **THEN** 测试 MUST 不启动 FastAPI/Uvicorn、不连接真实 Go 世界，并得到与 HTTP app 相同的严格 domain 输出

#### Scenario: app 不成为 domain 依赖

- **GIVEN** 架构检查枚举 Python import 与 Go/Python 边界
- **WHEN** domain/graph factory 导入 FastAPI、Uvicorn 或 Go 通过 shell/FFI 调用 Python
- **THEN** 门禁 MUST 失败；app 到 harness 的单向组合与 Go HTTP/MCP 边界 MUST 被接受

#### Scenario: harness 永不反向依赖 app

- **GIVEN** workspace 内全部 Python 源码
- **WHEN** 运行 harness 边界门禁
- **THEN** `harness/**` 下任何文件 MUST NOT import `app`、`fastapi` 或 `uvicorn`，违反 MUST 硬失败

### Requirement: Python 依赖与验证入口可复现

伙伴 Agent 服务 MUST 使用 Python 3.12、`mcp>=1.28.1,<2` 和提交的 `packages/agent/uv.lock` 精确解析生产与开发依赖，且 SHALL 提供固定的 `mornlea-agent serve --config` 入口；`mornlea-companion-agent` 作为 deprecated 别名 MUST 继续可用一期并输出弃用提示。仓库 MUST 提供一个无网络漂移的 Agent 服务门禁，验证 locked sync、格式/静态检查、类型检查与测试；lock 与 manifest 不一致 MUST 硬失败。第一阶段 MUST 只支持单个 Uvicorn worker 与单个 SQLite writer，配置多 worker 或缺失持久数据库路径 MUST 被拒绝。

#### Scenario: clean checkout 可锁定安装与测试

- **GIVEN** 一个 clean checkout 与 Python 3.12/uv
- **WHEN** 运行仓库规定的 Agent service check
- **THEN** `uv sync --locked`、格式/静态检查、类型检查与 pytest MUST 使用提交 lock 完成，MUST NOT隐式改写依赖版本

#### Scenario: 多 worker 配置被拒绝

- **GIVEN** 服务配置请求两个 Uvicorn worker 或两个进程共享同一 SQLite
- **WHEN** `mornlea-agent serve` 验证配置
- **THEN** 启动 MUST 失败，MUST NOT以不安全多 writer 模式继续

#### Scenario: 旧入口别名兼容一期

- **GIVEN** 新 workspace 已生效且旧别名尚未删除
- **WHEN** 调用 `mornlea-companion-agent serve --config` 与 `MORNLEA_COMPANION_AGENT_PYTHON`
- **THEN** 服务 MUST 正常启动并输出弃用提示，其 HTTP/MCP 可观察行为 MUST 与 `mornlea-agent` 逐字节一致

### Requirement: Units are organized independently under packages

The repository MUST place all independent functional units under `packages/`: five Go modules (`packages/shared`, `packages/server`, `packages/client`, `packages/tools`, `packages/contracts`), the standalone Python audit unit `packages/audit`, the Rust workspace `packages/engine`, and the Agent service family `packages/agent` (a uv workspace with members `harness/`, `extension-api/`, unreleased `app/gateway`, and `tests/`). The top level MUST retain only global repository content: `go.work`, `Makefile`, `.github/`, `docs/`, `openspec/`, `scripts/`, `testdata/`, root-level Agent configuration template `config.example.yaml` (local `config.yaml` gitignored), and root-level instructions, license, and ignore files. The top level MUST NOT contain `internal/`, `cmd/`, `services/`, `web/`, `contracts/`, or `engine/`. Each unit MUST build and test independently within its own directory.

#### Scenario: Top level contains only global directories and files

- **GIVEN** the repository root after unit reorganization
- **WHEN** top-level directory entries are enumerated
- **THEN** MUST appear only global directories and files (`packages/`, `go.work`, `Makefile`, `.github/`, `docs/`, `openspec/`, `scripts/`, `testdata/`, root `config.example.yaml`, root README/LICENSE/AGENTS.md/CLAUDE.md/.gitignore, etc.)
- **AND** MUST NOT appear `internal/`, `cmd/`, `services/`, `web/`, `contracts/`, or `engine/`

#### Scenario: Units build and test independently

- **GIVEN** the workspace with local modules resolved via `go.work`, Rust cdylib built, and Python venv ready
- **WHEN** running `go build ./...` and `go test ./...` in `packages/server`, `packages/client`, and `packages/tools`, `cargo build` and `cargo test` in `packages/engine`, `uv run pytest` in `packages/agent`, and `uv run pytest packages/audit` in `packages/audit`
- **THEN** each unit MUST complete its build and tests within its own directory without requiring prior manual steps in other unit directories

#### Scenario: Agent service family is extensible

- **GIVEN** companion has been onboarded into `packages/agent` workspace
- **WHEN** adding another independent Agent service
- **THEN** it MUST onboard side-by-side at `packages/agent/<name>` or reuse the workspace shared harness, and MUST NOT break observable behavior of companion or other units

### Requirement: Two-tier enforcement of Go module boundaries

The repository MUST coordinate five Go modules (`packages/shared`, `packages/server`, `packages/client`, `packages/tools`, `packages/contracts`) directly under root `go.work`, and the root directory MUST NOT retain a `go.mod`. Cross-unit references MUST be declared via `go.mod` require directives, and only the following directions SHALL be permitted: `server -> {shared, contracts}` (production code), `client -> {shared, server}`, `tools -> {shared, server, client, contracts}`; `contracts` MUST NOT require any sibling unit. Architectural and governance gates are enforced by the standalone Python unit `packages/audit`. Two ruled exceptions exist: `server` MAY require `client` solely for test files (mirror-driven integration tests), while production code MUST NOT import `packages/client`, enforced by source guards in `packages/audit/audit/boundaries.py`; `client` requires `server` solely for `packages/client/cmd/mornlea` application entry points (in-process local authoritative Host assembly), while client domain library packages (client/render/mesh/lod/audio/assets) MUST NOT import `packages/server`, enforced by source guards.
`packages/audit` architectural gates MUST enumerate actual import edges and enforce the same direction contract: client domain libraries MUST NOT depend on server or tools (except `packages/client/cmd/mornlea`), server production packages MUST NOT depend on client or tools, and shared MUST NOT depend on server, client, or tools.

#### Scenario: Compiler rejects undeclared cross-unit imports

- **GIVEN** source in `packages/shared` imports a package from `packages/server`
- **WHEN** building `packages/shared` under the workspace
- **THEN** the build MUST fail due to undeclared require
- **AND** even if `go.mod` require is added, module boundary checks in `packages/audit` MUST fail because `shared -> server` violates direction contracts

#### Scenario: Server production code cannot import client

- **GIVEN** a non-`_test.go` file in `packages/server` imports a `packages/client` package
- **WHEN** architectural check scans server production source
- **THEN** the import MUST be rejected
- **AND** imports of `packages/client` in `_test.go` files MUST be permitted for mirror-driven integration tests

#### Scenario: Client domain libraries cannot import server

- **GIVEN** a file in `packages/client` domain libraries (outside `cmd/mornlea`) imports a `packages/server` package
- **WHEN** architectural check scans client domain library source
- **THEN** the import MUST be rejected (including `_test.go`)
- **AND** imports of `packages/server` in `packages/client/cmd/mornlea` application entry points MUST be permitted

#### Scenario: Server sibling require set is gated

- **GIVEN** the sibling require set in `packages/server/go.mod`
- **WHEN** architectural check parses direct sibling requires
- **THEN** server sibling requires MUST be exactly `{shared, contracts, client}`
- **AND** extra require edges MUST be rejected, and removal of the client test edge MUST be rejected

#### Scenario: Architecture check enumerates real cross-module import edges

- **GIVEN** allowed dependency allowlists in `packages/audit/audit/dependencies.py`
- **WHEN** architecture check enumerates production imports across modules under `packages/`
- **THEN** actual internal import edges MUST fall entirely within the allowed edge set
- **AND** injecting undeclared edges MUST be rejected with synthetic drift tests proving checker validity

#### Scenario: Audit unit does not couple back to audited units

- **GIVEN** `packages/audit` operates as a standalone Python unit observing audited units solely via `go list`, file inspection, and AST/regex scanners
- **WHEN** dependencies and imports of `packages/audit` are enumerated
- **THEN** it MUST NOT import any production package from shared, server, client, tools, or contracts

### Requirement: 单元迁移保持测试入口与产物集合

单元化迁移 MUST 保持全部既有测试入口可寻址：各单元 `go test -list` 的
Test、Benchmark、Fuzz 入口并集与迁移前全仓集合一致，`t.Run` 标签逐一
不变；迁移 MUST 以 `git mv` 保历史，golden PNG、版本化 bin fixture 与
dist 产物 MUST 逐字节不变；wire bytes、协议版本、存档 schema、engine
ABI、client ABI 与 benchmark scenario MUST 不变。

#### Scenario: 测试入口并集与迁移前一致

- **GIVEN** 迁移前已持久化全仓 `go test ./... -list '.*'` 快照
- **WHEN** 迁移后对各 Go 单元分别 `go test -list` 取并集
- **THEN** Test、Benchmark、Fuzz 入口集合 MUST 与快照一致
- **AND** `t.Run` 标签 MUST 逐一不变

#### Scenario: 视觉 golden 与 fixture 字节不变

- **GIVEN** `testdata/visual-golden` 的世界与 UI 基线、各包版本化 bin
  fixture 已随单元迁移
- **WHEN** 运行 `make visual-check` 与受影响包测试
- **THEN** 全部场景 MUST 通过 tracked golden 且退出 0
- **AND** MUST NOT 产生 `*-actual.png`/`*-diff.png`，MUST NOT 修改、放宽
  或重新生成任何基线

