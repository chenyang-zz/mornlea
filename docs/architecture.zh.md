---
doc_id: architecture
doc_revision: 2026-09-16.1
language: zh-CN
counterpart: architecture.md
---
# Mornlea 当前架构

> 本文档记录当前实现。所有新架构决策必须先阅读 [`architecture-target.md`](architecture-target.md)。下面的 Godot/Python pilot 是迁移期实现，不是最终语言或运行时拓扑。

## 迁移期 Godot pilot（不是终局架构）

迁移项目根目录为 `apps/mornlea-godot/`，按粗粒度 feature 组织，并使用仅支持 macOS、Windows、Linux 的 `platform/desktop` 目标。当前 pilot 使用内嵌 Python host、Go client-core 和 Rust Godot adapter。这些是迁移 seam：Python 保持为终局 Godot feature 语言，而 Go client-core 及其 c-shared 边界将由 Rust client-core 替代。Android、iOS、Web 和主机平台不在范围内。

## 1. 系统总览

Mornlea 由 Go 应用、独立 Python 伙伴 Agent 服务与两个 Rust `cdylib` 组成。Go 持有应用装配、世界与权威模拟、协议与传输、存储、客户端镜像，以及渲染的 CPU 布局和上传编排；Python 只运行 Planner、Dialogue 与 compact memory；Rust `mornlea_engine` 提供无窗口数值内核，Rust `mornlea_client` 提供 Darwin 窗口、事件和 GPU 后端。Go 与 Rust 只经既定 C ABI bridge 协作；Go 与 Python 只经 loopback HTTP/MCP 合同协作，不存在可切换的第二套权威实现。

## 2. 服务端权威与客户端镜像

服务端是世界、玩家、伙伴、库存和玩法状态的唯一权威。客户端提交输入意图，保存经过验证的权威镜像，并可为本地响应进行可纠正的预测；预测结果不能反向确认服务端结算。

权威状态变更在服务端 tick 内按固定阶段串行结算，再通过协议发布。客户端渲染和 UI 只消费镜像与本地呈现状态，不直接读取或修改权威模拟。

## 3. Memory/TCP 共用路径

普通本地游戏使用 Memory transport，远程游戏使用 TCP transport；两者复用同一 packet/codec 契约、登录状态机、会话装配和权威模拟。Memory 只改变传送介质，不提供绕过登录、输入校验或服务端裁决的同进程特权路径。

`packages/shared/network` 负责会话与传输编排：共享 stream 接口、Play endpoint 门面、登录状态机和 Memory transport，并以别名再导出对协议消息子包 `packages/shared/network/protocol`（packet/message/registry/snapshot 协议层）与编解码子包 `packages/shared/network/codec`（packet↔wire 编解码与帧封装）保持既有 `network.X` 消费面；`packages/shared/network/tcp` 负责 TCP listener、dial、stream 实现，且只承担 transport 职责。Host 与模拟层消费相同的已验证命令，因此本地和远程模式共享行为语义。

## 4. 独立伙伴 Agent 服务

伙伴链路的依赖和写权限固定如下：

```text
Go Host / Agent HTTP client ──loopback HTTP v1──> Python FastAPI / LangGraph
Python MCP SDK             ──loopback MCP v1───> Go frozen SnapshotRegistry
Go tick ──strict decode + current-world revalidation──> Task Runner ──> world writes
Python SQLite <──compact MemoryState CAS──> Go companions.ai v5 recovery mirror
```

Go 服务端仍唯一拥有伙伴身体、背包、任务/FIFO、generation、snapshot、最终计划校验与全部世界写入。Python 返回候选 Plan、Dialogue proposal 和 memory 结果；MCP 六工具只读 33×17×33 frozen terrain projection 或做纯候选校验，不能提交 `CompanionAction`。Go 不 shell-out、FFI 或嵌入 Python，也不负责启动/监护 Agent 进程。

Agent、MCP 或 provider 不可用时，权威世界、已有 Running 任务与 FIFO 继续推进；新规划以稳定 `PlannerUnavailable` 失败，Dialogue 跳过。任何失败都不引入 direct-model fallback，也不能把恢复镜像当成正常 Dialogue 提示来源。

Python 服务使用 Python 3.12、FastAPI、LangChain/LangGraph、单 Uvicorn worker 和单 SQLite writer。Planner 与 Dialogue 每次创建 transient graph，不保存 checkpoint。SQLite 只保存 namespace lease/fencing、每伙伴 epoch、summary/revision/operation 与 tombstone/CAS 元数据；不保存 plan、task、FIFO、snapshot、prompt、messages、persona、line 或 proposal。Python 是运行期摘要权威，Go `companions.ai` v5 镜像只用于数据库丢失或进程重启后的 reconcile。

Go 与 Python 各自限制全局 4 个 Planner/Dialogue run、同一伙伴 1 个在途且无等待队列；模型调用默认/硬上限 3/5，自主工具 4/8，总时限 30/60 秒。snapshot registry 以 caller deadline 加 5 秒 TTL 持有最多四份 immutable snapshot；取消后立即拒绝新 lookup，已取得 view 的 handler 在有界 loop、编码和 response commit checkpoint 观察取消后丢弃完整结果。权威 tick 只发布/接收有界不可变值，不执行 Agent HTTP、MCP、模型、SQLite 或 JSON 编码 I/O。

终态 Dialogue 先返回未提交 proposal。Go 在 tick 边界重验后建立 accepted reservation，之后 generation 变化不撤销该 reservation；只有 operation/epoch 对应的 CAS commit 确认成功，Go 才更新 v5 镜像并广播一次台词。active↔inactive 每次推进 memory epoch；active canonical-zero、active nonzero mirror 与 inactive tombstone 各有严格 replay identity，higher epoch 会 fence 全部旧结果。安全关服依次停止新聊天、取消并等待 worker、冻结队列/actor、保存最终 v5、flush 世界、release namespace，再关闭 MCP 与世界存储；保存或 flush 失败时保持组件可重试。

六个 Go 单元模块均由根 `go.work` 统一编排。

## 5. Go 包职责与 archcheck 依赖边界

- `packages/client/cmd/mornlea` 与 `packages/server/cmd/mornlea-server` 负责应用入口与资源生命周期装配。
- `packages/shared` 是共享域 Go 模块（独立 go.mod，go.work 成员），持有 server 与 client 双侧共用的领域包：`core`（公共领域类型与 native raycast）、`world`（区块与世界数据模型）、`physics`（玩家运动与碰撞）、`pathfind`（不可变快照寻路）、`companion`（伙伴身份与领域类型）、`network`（+protocol/codec/tcp，登录状态机与传输）、`worldgen`（seed 播种与区块回写）、`tuning`（Tunables 快照）、`profile`、`config`、`logging` 与 `nativeabi`（engine ABI 的唯一 Go bridge）。
- `packages/server/sim` 为仅含指导文档的目录，权威模拟由四个子包承载：`contract`（跨边界 DTO）、`realm`（世界维度与单 tick 事务）、`entity`（玩家/伙伴/夜行者与玩法结算）、`runtime`（Engine 与 Step 编排）；Tunables 快照位于 `packages/shared/tuning`。依赖方向与单次提交纪律见 `packages/server/sim/AGENTS.md` 与 `packages/audit`。
- `packages/server/storage` 持有世界、玩家、伙伴和夜行者数据的编码、迁移、恢复与磁盘生命周期，并以子包 `packages/server/storage/chunk`、`packages/server/storage/player`、`packages/server/storage/companion`、`packages/server/storage/hostile`、`packages/server/storage/region` 等细化实现，顶层保持外部消费面。
- `packages/server/server` 装配 Host、Server、登录、会话、权威 tick、发布和关服编排；通过 `packages/server/server/persistence` 委派存档生命周期，自身不持有保存队列、重试状态或 worker，实现只保留 `PersistenceStatus` 与 `ErrPlayerPersistenceBackpressure` 的兼容 re-export。
- `packages/server/server/persistence` 单独持有世界区块与 metadata、玩家、伙伴、夜行者四类存档的加载、观察、异步保存、重试、flush/close 与 worker 生命周期；生产代码仅依赖 `packages/shared/companion`、`packages/shared/core`、`packages/shared/physics`、`packages/server/sim/runtime`、`packages/server/storage`，不得反向导入 `packages/server/server` 或访问 Host/Server 私有状态，依赖方向以 `packages/audit` 为准。
- `packages/shared/pathfind` 持有不可变快照上的有界寻路且只依赖 `packages/shared/core`；`packages/shared/companion` 与 `packages/server/server` 消费它，但寻路不拥有玩法或世界访问。
- `packages/client` 是客户端域 Go 模块（独立 go.mod，go.work 成员）：`client` 持有客户端镜像、输入预测、消息接收、client ABI bridge 和渲染侧 CPU 编排；`render`（+hud）、`mesh`、`assets`、`lod`、`audio` 与 `packages/shared/worldgen`、`packages/server/fluid` 同属领域数据描述、CPU 编码和 Rust 调用编排，不拥有 GPU 后端或第二套数值生产实现；`packages/client/cmd/mornlea` 是图形客户端应用入口（app/benchmark 在进程内装配本地权威 Host，是该模块中唯一允许 import `packages/server` 的位置）。
- server 与 client 两模块间的两条豁免边由 `packages/audit` 源码守卫强制：server 生产文件禁 import client（Memory/TCP 集成测试的客户端镜像只出现在 `_test.go`），client 域库文件禁 import server（服务端装配只属于 `packages/client/cmd/mornlea`）。

内部包允许的直接依赖以 `packages/audit/audit/dependencies.py` 的 `allowed` 表为准。`packages/audit` 同时以 `TestSimAuthorityStateOwnershipStaysExplicit` 扫描 runtime 的包变量与全部 holder，把唯一 mutation/commit 绑定到 `StepWithTunables` 的真实调用路径，以 `TestAuthorityTickTunablesStayExplicit` 守住权威 tick 参数捕获和传递；其余门禁守住无 WebGPU Go 依赖、无图形专服闭包、伙伴 Agent 服务发布边界、Make/CI 门禁和长期版本基线；架构文档不复制会随包演进的依赖白名单。

## 6. `mornlea_engine` / engine ABI v11

`mornlea_engine` 是 mesh/light、collision、raycast、physics tick 积分、worldgen、LOD shell 与流体规则求值/重扫扫描的唯一生产实现；流体的队列、预算、游标与冲毁结算编排仍在 Go（`packages/server/fluid` 与 `packages/server/sim/realm` 经 `packages/shared/nativeabi` 调用流体 kernel）。该 crate 保持无窗口，不拥有权威世界状态，不执行文件或网络 I/O，也不承载伤害、库存、权限或 tick 编排等业务规则。

当前 engine C ABI 为 v11。Go 侧只有 `packages/shared/nativeabi` 可以接触该 ABI；领域包构造语义输入并解码结果。header、Rust FFI、Go bridge、ABI 版本和跨语言一致性检查必须成套演进，调用结束后任一侧都不得保留对方指针。

## 7. `mornlea_client` / client ABI v19

`mornlea_client` 持有 Darwin 窗口与事件采集、进程内 WKWebView 菜单层、GPU 资源、shader、render pass、窗口 surface 和离屏渲染。窗口型 UI（主菜单/设置/暂停/F3）由内嵌的 Vite + TypeScript + React 前端经 WKWebView 呈现，资产经 `mornlea://` scheme handler 从 Rust 内嵌字节供给；常显 HUD、背包/合成、人物页、工作台、箱子、熔炉及 tooltip 同样由前端呈现，生产帧不再准备或提交面板 GPU 实例。`-connect` 是交互联机入口，首次下行即挂载同一前端；只有 capture/benchmark 保持无窗口、无 WebView。Go 不导入 WebGPU 绑定，只通过 `packages/client/client` 提供的 client ABI bridge 使用窗口和 renderer 领域接口。

当前 client C ABI 为 v19，并与 engine ABI 独立演进。header、Rust FFI、`packages/client/client` bridge、版本和跨语言检查必须同步更新；失败或容量不足不能发布部分输出。

renderer 已拥有由 MRW1 原子更新的紧凑 `RenderWorld` 派生缓存，Go Mirror 仍是客户端逻辑状态的真相来源。该缓存入口目前只由 Rust/Go 测试驱动，尚未接入 `packages/client/cmd/mornlea/app`；Go 仍持有生产 mesh 调度、connectivity/visibility、逐 section upload 与 draw 输入，迁移这些职责属于后续 change。

## 8. 图形客户端与无图形专服 release unit

Darwin 图形客户端 `mornlea` 同时依赖同一次构建的 `mornlea_engine` 与 `mornlea_client`，不能跨构建混装 ABI 组件。无图形专服 `mornlea-server` 不依赖 `mornlea_client`、窗口或 GPU 栈，但权威物理和空间查询仍依赖同次构建的 `mornlea_engine`。

Linux 专服发布单元由 `mornlea-server` 与相邻的 `libmornlea_engine.so` 组成，并通过 `$ORIGIN` 加载。二者必须作为一个不可跨版本混装的 release unit 分发。

## 9. 并发、数据和热路径约束

- 跨 goroutine 发送成功后的消息及其 slice 视为不可变；后续修改必须复制。
- 权威 tick、渲染和网络热路径只执行有界工作，不阻塞磁盘、网络、模型调用或其他重 CPU 工作。
- 重工作通过有界队列、不可变快照或 worker 离开热路径，并在所有权清晰的边界汇合结果。
- 持久化并发边界：`packages/server/server/persistence` 的四类所有者各自以有界 channel 与固定数量 worker 隔离磁盘 I/O——`World` 由 `Options.SaveWorkers` 决定 worker 数（`saveJobs`/`saveCompletions` 容量为 `SaveWorkers*2`），`Players` 固定 2 worker（`playerSaveJobCapacity=16`/`playerSaveDoneCapacity=2`），`Companions` 与 `Hostiles` 各 1 worker（容量各 1）。权威 tick 仅执行有界、非阻塞的 `World.Observe`/`Drain`、`Players.Observe`/`Poll`、`Companions.Observe`/`Poll`、`Hostiles.Observe`/`Poll` 调度，绝不阻塞等待落盘；`SaveObserver` 仅在 `World` worker 的 `SaveBatch` 计时路径中调用，不在 tick 路径执行。`World.Flush` 与 `World.ShutdownContextError` 通过 `Options.EngineLocker`（根 `Server.stepMu`，子包独立构造时回退到私有 `sync.Mutex`）先于 `World.mu` 做短暂的 engine/state 变迁，随后立即释放两者再等待 channel/context；`Drain`/`Status` 仍保持调用方持有 tick 锁的既有契约。
- 协议、存档和 FFI 入口先校验类型、长度、计数、容量与版本，再分配、遍历或写输出。
- overflow、数据丢失、报告身份不完整和 I/O 错误必须显式失败，不能静默截断或吞错。

## 10. 行为规格、实现进度和历史设计入口

当前可观察行为以代码、测试和 [`openspec/specs/`](../openspec/specs/) 为准。正在实施的变更位于 [`openspec/changes/`](../openspec/changes/)，流程见 [`docs/openspec.md`](openspec.md)。

实现编年史见 [`docs/notes/progress.md`](notes/progress.md)。`docs/superpowers/` 与 [`openspec/changes/archive/`](../openspec/changes/archive/) 保存历史设计和 change 证据，不覆盖当前架构与行为主规格。完整入口见 [`docs/README.md`](README.md)。

## 11. 仓库目录导览

```text
.
├── packages/
│   ├── agent/
│   │   └── companion/      Python 3.12 FastAPI/LangGraph/SQLite 独立服务
│   ├── contracts/          独立最小 Go 模块（go.work workspace 成员）
│   │   └── companion-agent/    HTTP v1 与 MCP v1 的共享 manifest/schema/golden
│   ├── shared/             共享域 Go 模块（go.work workspace 成员）：server/client 双侧共用领域包
│   │   ├── core/           公共领域类型与 native raycast batch 驱动
│   │   ├── nativeabi/      engine C ABI 的唯一 Go bridge
│   │   ├── logging/        模块化日志
│   │   ├── physics/        玩家运动与碰撞
│   │   ├── pathfind/       不可变快照上的有界寻路
│   │   ├── world/          区块和世界数据模型
│   │   ├── worldgen/       worldgen seed→perm 播种、Rust 调用与区块回写
│   │   ├── companion/      独立伙伴身份、静态定义与身体类型
│   │   ├── network/        二进制协议、登录状态机与 Memory/TCP 传输（protocol/codec/tcp 子包）
│   │   ├── tuning/         Tunables 快照与校验（原 sim/tuning 上提）
│   │   ├── profile/        本机稳定玩家身份与档案
│   │   └── config/         共享 JSON 配置加载与校验
│   ├── client/             客户端域 Go 模块（go.work workspace 成员）
│   │   ├── client/          输入、相机、预测、窗口/client ABI 与客户端镜像
│   │   ├── render/          渲染 CPU 半部：布局、编码与上传调度（hud/ 子包）
│   │   ├── mesh/            区块网格生产 API（实现位于 Rust cdylib）
│   │   ├── lod/             远环 tile 调度与 CPU 编码
│   │   ├── audio/           Darwin 本地程序化提示音
│   │   ├── assets/          方块定义与程序化材质（packs/ 内嵌默认材质包）
│   │   └── cmd/
│   │       └── mornlea/     游戏客户端与内置服务端装配（app/capture/benchmark/devcapture 子包）
│   ├── server/             服务端域 Go 模块（go.work workspace 成员）
│   │   ├── cmd/
│   │   │   └── mornlea-server/  无图形 TCP 专用服务端
│   │   ├── sim/            权威模拟指导目录（生产见 contract/realm/entity/runtime 子包）
│   │   │   ├── contract/   跨边界 DTO
│   │   │   ├── realm/      世界维度、持久化与环境事务
│   │   │   ├── entity/     玩家/伙伴/夜行者与玩法结算
│   │   │   └── runtime/    Engine、订阅与 Step 编排
│   │   ├── fluid/          有界权威流体更新队列与流体 kernel 的 native 包装
│   │   ├── storage/        世界、区域文件与玩家状态持久化
│   │   └── server/         服务端 Host、Server、登录、会话、权威 tick、发布与关服编排
│   │       └── persistence/    四类存档（世界/玩家/伙伴/夜行者）加载、观察、异步保存、重试、flush 与 worker
│   ├── tools/              开发工具模块（go.work workspace 成员）
│   │   ├── perfcheck/      性能报告比较工具
│   │   ├── agent-board/    AI 工作者执行状态看板（web/ 为 React 前端）
│   │   ├── gfxspike/       Rust renderer 地形渲染验证程序
│   │   └── composite_grass_side/    草方块侧面材质合成工具
│   ├── audit/              独立 Python 审计单元：架构与治理门禁测试集（依赖方向、单元边界、身份与基线版本）
│   └── engine/
│       └── crates/
│           ├── mornlea_engine/  固定 Rust 1.97.1 cdylib：mesh/light/collision/raycast/physics/worldgen/lod/fluid
│           └── mornlea_client/  Darwin 窗口、事件循环、WebView 菜单层（frontend/ React 前端）与全部 GPU 渲染
├── scripts/agent-hooks/     已下线 Hook 的策略实现与 CI 测试
└── docs/                    设计、实施计划、性能记录与实现进度
```
