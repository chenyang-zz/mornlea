---
doc_id: runtime-interface-architecture
doc_revision: 2026-09-25.1
language: zh-CN
counterpart: runtime-interface-architecture.md
status: target-not-current
---
# 目标运行时接口架构

本文是 Rust/Godot 迁移后续开发的目标接口总图，属于设计与评审契约，**不表示** Rust 服务器、Rust 客户端核心或下文语义家族已经实现。[English](runtime-interface-architecture.md)。[目标架构](architecture-target.zh.md)决定语言所有权；[接口先行规范](interface-first-parallel-development.zh.md)决定何时必须先落地可编译且经过验收的声明，才能启动独立任务。当前行为仍以代码、测试和正式 OpenSpec 规范为准。

## 1. 依赖与权威关系

```text
Godot 场景 / 设备 / 嵌入式 Python
            │ 有类型的自有数据；不保留原生指针
            ▼
mornlea_godot（Rust GDExtension；唯一桥接层与家族注册表）
            │ 语义输入批次 / 不可变展示帧
            ▼
mornlea_client_core（规划中；会话、镜像、预测、帧构建）
            │ v45 协议帧，承载于 Memory 或 TCP
            ▼
mornlea_server（规划中；唯一权威与 tick 所有者）
      ├─────┼─────────────┐
      ▼     ▼             ▼
  领域 / 协议       存储 I/O 工作线程 ── 带版本的存档编解码器
      │
      └── 数值内核

独立 Python Agent ── 带版本的回环 HTTP/MCP ──► 服务器候选动作入口
```

`mornlea_domain`、`mornlea_protocol` 和 `mornlea_storage` 已有经过验证的 Rust 值类型和编解码接口。`mornlea_server` 与 `mornlea_client_core` 是规划中的 crate。当前 `mornlea_godot` 调用 Go 试点核心；它的 ABI 表只是迁移接口，不能证明目标生产者已存在。独立的 `mornlea_client` 渲染器 ABI 也不是规划中的客户端核心。分阶段切换完成前，Go 仍是当前产品路径和离线兼容性参照。

依赖图必须无环：domain 不依赖运行时；protocol 依赖 domain；storage 编解码器使用已校验值；kernel 拥有数值算法；server 和 client core 只向下依赖；Godot 适配器依赖 client core；Python 仅使用桥接层给出的有类型数据。client core 不导入 server 或 storage；两个 Python 环境互不导入。

## 2. 契约登记表与落地顺序

每行指定一个边界的**唯一所有者**。`现有`表示当前已有代码与测试；`目标`表示未来任务必须先交付可编译接口及成功/失败测试替身，然后依赖方才可并行开发。一个家族只有在真实生产者、真实消费者和集成三道门均基于已接受 SHA 通过后才能启用。

| ID | 契约所有者 / 公共接口 | 消费者 | 状态及前置条件 |
| --- | --- | --- | --- |
| D0 | `mornlea_domain`：已校验 ID、坐标、值、封闭的 `Command`、`CommandEnvelope`、`Event`、`RoutedEvent` | protocol、server、client core、回放 | 现有；扩展须经过行为变更评审。 |
| P0 | `mornlea_protocol`：v45 报文注册、分帧、准入、语义转换 | server、client core、传输测试 | 现有；除非另行批准，线协议保持 v45。 |
| S0 | `mornlea_storage`：带版本的记录编解码和迁移 | server 存储工作线程、离线工具 | 编解码接口现有；I/O 所有者仍是目标。 |
| K0 | `mornlea_engine` 安全数值门面和寻路 | server、client core、准备线程 | 目标 F1 收口；不得复制数值算法。 |
| S1 | `mornlea_server::core`：入口、排序 tick、路由观察结果 | 传输适配器、回放、存储 | 完整 F1 后的目标接口；F2 首个共享契约。 |
| S2 | `mornlea_server::transport`：Memory/TCP 分帧与会话准入 | 本地游玩、局域网、客户端测试 | S1 后的目标接口；共用核心路径。 |
| S3 | `mornlea_server::persistence`：异步请求、确认与恢复 | server core、激活工具 | S1/S0 后的目标接口；世界只允许一个可写租约。 |
| S4 | `mornlea_server::agent`：有界候选结果适配器 | server core、Agent 服务 | S1 后的目标接口；现有 HTTP/MCP 合同保持独立。 |
| C1 | `mornlea_client_core::session`：状态、镜像、预测、纠正 | Godot 适配器、无界面回放 | F1 和已接受的 S2 会话契约后的目标接口。 |
| C2 | `mornlea_client_core::presentation`：不可变帧与有界准备 | Godot 适配器、功能家族 | C1 后的目标接口；每次发布统一 epoch/revision。 |
| G1 | `mornlea_godot`：有类型桥接、ABI/家族协商、生命周期 | 嵌入式 Python 宿主 | C1/C2 后改由 Rust 核心生产；当前 Go ABI 仅属试点。 |
| V1 | 地形、角色、UI、音频、输入、生命周期语义家族 | Godot/Python 功能 P8–P11 | C2/G1 后的目标接口；逐家族版本化与禁用。 |
| T1 | 回放、诊断、资产与桌面激活清单 | P12/P13/P14 工具 | 所属运行时契约之后的目标接口；不得形成第二权威。 |

`S1` 和 `C1` 是边界门面，不是所有玩法规则的通用插件接口。仅供一个任务使用的辅助函数保持私有。新家族应更新登记表、交付小型契约包，不扩展无类型 `Any` 或字符串消息通道。

## 3. 统一身份、版本和值规则

- `PlayerId`、实体 ID、位置、维度、有限向量、显示名和有界文本来自 `mornlea_domain`，调用方使用其校验构造器。Python、服务器传输层及 Godot 不重复实现规则。
- `tick: u64` 是权威模拟 tick；`session: u64` 由服务器会话所有者分配，区别于玩家身份；`sequence: u64` 是客户端命令序号；`arrival_index: u64` 由服务器入口对所属 tick 分配。只有服务器入口构建 `CommandEnvelope`；协议转换不得编造这些字段。现有 `order_commands` 及过期序号策略是排序依据。
- `session_epoch: u64` 标识一次客户端连接代际，重置或重连时递增，不能当作服务器 tick；`confirmed_revision: u64` 标识该代际最后完整应用的权威观察结果。展示发布同时携带二者，不得混合不同配对的记录。revision 由 client core 分配，Godot 仅检查。
- 线协议兼容性以协议版本与报文注册表为准，目前是 v45。存档兼容性分别以带版本的 schema 为准，目前 player v9、chunk v9、world v6、companions v5、hostile v2、passive v1。桥接 ABI 主/次版本与各语义家族主/次版本是**不同身份**。桥接次版本可以新增家族或可跳过字段；现有必需字段的布局或语义改变须提升家族主版本。线协议、存档和 ABI 版本不能相互替代。
- 所有量纲显式约定：count 是记录数，length 是字节数，位置使用领域坐标，API 边缘的时间预算使用整数纳秒，帧/tick 身份是无符号整数。发布前拒绝非有限数、非法 UTF-8、重复身份、未知必需家族和算术溢出；不得隐式钳制、截断、丢精度转换或使用哨兵 ID。

## 4. 所有边界共用的失败与发布规则

稳定的语义失败类别为 `InvalidInput`、`IncompatibleVersion`、`InvalidState`、`StaleEpoch`、`StaleRevision`、`Capacity`、`Timeout`、`Cancelled`、`Disconnected`、`Unavailable`、`Io` 和 `Internal`。各边界映射到自己现有的类型错误或状态码；此列表是语义分类，**不是**新增共享枚举，也不改变 v45 线协议。保留更具体的 `DomainError`、`ProtocolError` 和 ABI 状态码。任何边界丢弃已接受数据后都不得报告成功。

校验顺序为：身份/版本和生命周期；声明的记录数与字节上限；全部字段；epoch/revision 与顺序；必需容量预留；然后才允许修改状态或发布。输入批次失败不得产生部分效果。输出批次超过单个线报文上限时，所有者可以用明确的续传游标切成有序、各自完整有效的报文，或整体拒绝；协议适配器不得悄悄截断领域事件。队列已满返回 `Capacity`，重试规则由具体契约说明。不可逆命令结果和持久化确认不能为了渲染流畅而丢弃。仅渲染用的旧快照可在同一 epoch 内合并；移除和资源释放操作仍须保持顺序。

权威 tick、渲染和网络热路径均不等待文件、套接字、Agent、GPU 或 Python 工作。各所有者的可编译契约包必须明确队列大小、每次调用的记录/字节上限、工作单位和关停时限。已有上限以代码为准：协议帧体 `2 MiB`、领域语义批次 `4096` 条、试点桥接输入 `128` 个事件、试点世界批次 `4096` 次操作、试点单步消息与网格最大值均为 `4096`。当前 Godot 宿主每步请求 `64` 条消息、`32` 个网格；这是**试点默认值**，不得默认为 Rust 核心服务保证。新服务器队列与时限需 F2 的测量及兼容性证据后冻结，本文不猜测全局容量。

不可变发布在交接前由生产者持有；接收方获得自有副本或引用计数的不可变 Rust 值。Python 得到自有的 Godot/Python 表示，不保留 Rust 内存。桥接 pull 完整校验批次后才替换上次可见快照。取消操作幂等，通过 epoch 或请求 ID 使待处理工作失效；已确认的持久写入不能在没有显式失败结果的情况下被取消。

## 5. 权威服务器接口

F2 契约落地须提供安全 Rust API，包含以下操作和结果；私有具体类型届时确定。以下签名规定公共**形状**和所有权，在编码前还须与已接受 F1 类型对齐：

```rust
ServerCore::open(config: ServerConfig, store: StoreHandle, agent: AgentHandle)
    -> Result<ServerCore, ServerOpenError>;
ServerCore::admit(login: AdmittedLogin, transport: TransportKind)
    -> Result<SessionKey, AdmissionError>;
ServerCore::submit(session: SessionKey, intent: PlayIntent)
    -> Result<SubmissionReceipt, IntakeError>;
ServerCore::submit_companion(candidate: CompanionActionEnvelope)
    -> Result<CompanionReceipt, IntakeError>;
ServerCore::advance_tick(work: TickBudget)
    -> Result<TickPublication, TickError>;
ServerCore::close_session(session: SessionKey, reason: CloseReason)
    -> Result<(), SessionError>;
ServerCore::shutdown(deadline: Deadline)
    -> Result<ShutdownReport, ShutdownError>;
```

`SessionKey` 是服务器签发的不透明句柄；`AdmittedLogin` 和 `PlayIntent` 是已有协议类型。`submit` 校验会话存活、阶段及队列容量。针对 `PlayIntent::Sequenced`，还要校验序号并记录到达顺序；`SubmissionReceipt::QueuedForTick` 只表示**已入队**，不表示世界修改成功。`PlayIntent::Chat` 走独立的文本校验和路由路径。`PlayIntent::KeepAliveReply` 通过会话控制返回 `SubmissionReceipt::ControlAccepted`，不得成为领域命令。`advance_tick` 是唯一世界修改点：排空本轮有界输入集、构建已有领域命令信封、排序、执行权威规则，并产生 `TickPublication`，包含路由领域事件、控制面响应、存储请求和工作量/溢出报告。它不调用网络、磁盘、Agent 或 Python。`EventRecipient::Broadcast` 由服务器会话所有者展开，domain/protocol 不负责。hello、login、keepalive、disconnect 等控制事实不属于 `Event`。

`TransportKind::{Memory,Tcp}` 只改变字节传输与监督方式。两种适配器调用相同的分帧解码器、`validate_hello`、`admit_login`、语义转换、`submit` 和输出编码器；不得调用本地直改世界的接口。`SessionKey` 只能退役一次；旧会话、重复、错误阶段或断开后的消息均有稳定拒绝结果。服务器限制待登录数、活动会话、入站/出站帧及慢接收端；不因单个客户端暂停 tick。目前 Go 路径的登录/握手时限分别是 10 秒和 5 秒，可作为兼容性证据；F2 须在契约测试中固定准确的阶段行为。

`StoreHandle` 是建立在 `mornlea_storage` 编解码器之上的异步工作邮箱：`submit(SaveRequest) -> Result<SaveTicket, CapacityOrState>`、`poll(SaveTicket) -> Pending | Durable | Failed(IoError)`、`flush(deadline) -> Result<FlushReport, IoOrTimeout>`。达到要求的写入/提交边界后才返回 durable；读取失败不得生成空白世界。启动时读取可变状态前先取得独占可写世界租约。崩溃恢复、schema 检查、备份和回滚必须在一次性副本上测试；不兼容 schema 或竞争写入是硬失败。

`AgentHandle` 使用现有带版本的回环服务契约发起有界异步请求。结果包含候选数据、请求 ID 和来源 tick；超时、取消或服务错误都没有世界效果。当前 `CommandEnvelope` 绑定人类会话，不能表达伙伴候选动作。因此 S1 使用单独的、无会话的强类型伙伴入口，携带伙伴身份和请求来源。`advance_tick` 重新检查当前权威、权限、目标、距离、资源及顺序后，再让候选动作进入与人类输入相同的已验证世界变更流程；它不会伪造人类会话或序号。对话或摘要文字属于展示数据，不能绕过命令校验。

## 6. 客户端核心与有类型桥接接口

F3 契约落地提供唯一的无界面客户端核心，公共操作如下：

```rust
ClientCore::new(config: ClientConfig) -> Result<ClientCore, ClientError>;
ClientCore::connect(endpoint: Endpoint, identity: ClientIdentity)
    -> Result<SessionEpoch, ClientError>;
ClientCore::submit_input(epoch: SessionEpoch, batch: InputBatch)
    -> Result<InputReceipt, ClientError>;
ClientCore::step(epoch: SessionEpoch, work: ClientWorkBudget)
    -> Result<StepReport, ClientError>;
ClientCore::snapshot(epoch: SessionEpoch)
    -> Result<Arc<PresentationFrame>, ClientError>;
ClientCore::reset(epoch: SessionEpoch) -> Result<SessionEpoch, ClientError>;
ClientCore::close() -> Result<(), ClientError>;
```

`connect` 启动非阻塞会话尝试并返回其 epoch；真正的登录成功或失败通过 `step`/会话观察结果出现。核心拥有登录、报文处理、确认镜像、待确认输入日志、可逆预测、纠正/重放、语义声音提示选择以及准备工作调度。应用一份完整且已校验的服务器观察结果之后才能递增 `confirmed_revision`；过期、重复或乱序观察结果遵循协议/会话回放契约，不能产生混合帧。`InputReceipt` 只表示进入有界本地队列，不表示服务器接受。纠正仅丢弃和重放当前 epoch 的待确认输入。重置在发布新 epoch 前使旧工作、资源和句柄失效。无界面回放与 Godot 使用同一路径。

`PresentationFrame` 不可变，包含 `{layout_major, layout_minor, session_epoch, confirmed_revision, frame_index, families}`。`frame_index` 在同一确认 revision 内随客户端展示发布递增，不是服务器 tick。每个家族载荷要么在已校验头中重复 epoch/revision，要么作为此帧子对象且无独立时钟。帧构建器在一次原子替换前检查全部家族、总记录/字节配额、移除/更新顺序和所需容量。设备不可用不改变语义结果。GPU 网格字节和资源句柄不能作为领域或线协议值暴露。

`mornlea_godot` 是唯一原生适配器，向 Godot 提供带版本的 `open_core`、`connect`、`submit_typed_input`、`step`、`pull_typed_frame`、`family_table`、`reset`、`close`。在复制为 Godot 自有值前校验 ABI/布局及所有 count/length；panic 或非法句柄映射为稳定边界错误。只有它管理原生句柄生命周期。嵌入式 Python 宿主在激活前协商必需家族，把帧视图映射到场景/资源，并能在不销毁会话的情况下禁用某个功能。Python 不接收报文字节、存档记录、原始 C 符号、借用指针或可变 Rust 缓冲区。

## 7. 语义家族目录

此表是**目标逻辑桥接目录**。现有试点 ABI 以数字 ID `1..8` 表示 identity、connection、input、step、world、frame、status、environment。新数字 ABI ID 和二进制布局须由 C2 后独占编辑的 F3 注册表契约落地任务分配；下方逻辑名称是稳定规划键，并不表示试点注册表已实现它们。清单必须把每个逻辑键解析为真实描述符 `{numeric_id, major, minor, record_limit, record_bytes}`，必需映射缺失则拒绝。这明确处理了当前 `audio-cues@1.0` 与 `lifecycle@1.0` 使用符号名称、而宿主仅能按数字协商的矛盾。

| 逻辑家族 | 生产者 → 消费者 | 最小语义载荷及顺序 | 功能所有者 |
| --- | --- | --- | --- |
| `session@1` | client core → 宿主 | 连接阶段、epoch、玩家身份、终止原因；每代际状态单调 | F3/P13 |
| `input@1` | 宿主 → client core | 有序设备事件、动作状态、指针/射线意图、本地序号及回执；整批拒绝 | F3/P10 |
| `terrain@1` | client core → 世界功能 | 维度、chunk/section 键、内容 revision、代数、有类型网格/光照/材质引用、更新与移除 | P8 |
| `actors@1` | client core → 角色功能 | 稳定种类/ID、维度、变换、运动、动画/特效意图、生成/更新/消失顺序 | P9 |
| `player-view@1` | client core → 玩家功能 | 已确认/预测姿态来源、注视目标、运动状态、纠正标记 | F3/P9 |
| `inventory-ui@1` | client core → UI 功能 | 选中槽、物品堆、容器引用/revision、制作/熔炉结果、拒绝与关闭结果 | P10 |
| `world-ui@1` | client core → UI 功能 | 时间、天气、生存状态、聊天/任务状态、交互提示、稳定显示文本 | P10 |
| `audio-cues@1` | client core → 音频功能 | 提示 ID、源/事件 ID、位置或非空间标记、类别、播放参数、一次性去重键 | P11 |
| `lifecycle@1` | client core/桥接 → 宿主 | epoch 开/关/重置、功能激活/释放代数、资源失效顺序 | F3/P8–P11 |
| `diagnostics@1` | client core/桥接 → 宿主 | 有类型的队列峰值、拒绝原因、生产者身份、契约版本、帧/tick 关联 | P12 |

地形移除必须早于相同 section 键的复用；晚到的网格结果若 epoch、chunk 代数或内容 revision 不匹配则丢弃。出现后续更新或打开之前，角色消失和 UI 容器关闭不得省略。音频去重按 epoch 与权威事件身份限定；没有音频设备只停止播放，不抹去提示/结果记录。设备、纹理、场景和音频资源由 Godot 主线程所有者管理，重置或禁用功能时释放。UI 不得根据乐观动画重建真实库存。

每个家族落地时，所有者发布精确字段 schema：量纲、合法区间、顺序、记录/字节上限、可选/必需字段、兼容性示例，以及非法、过期、溢出测试。未具备该契约包及真实生产者/Godot 消费者集成结果的规划家族保持禁用。这是受控扩展点，不允许交付无类型或不完整的家族。

Python 功能宿主目前已有一套结构性试点接口：`validate_feature(feature_id)`、`bind_host(bridge_path)`、`activate_feature(epoch)`、`reset_feature(epoch)` 和 `deactivate_feature()`。目标宿主保留其生命周期含义：实例化前校验并协商全部必需家族；先激活依赖、再激活消费者；每个宿主步只应用一份完整校验的有类型帧；重置时先处理提供者、再处理消费者；停用时先处理消费者、再处理提供者。每步只能有一个功能驱动客户端会话。可选功能失败会禁用它及其依赖方；必需功能失败按逆序回滚激活。功能方法可以驱动输入和应用帧，但不能持有第二个网络会话、镜像、tick 或桥接句柄。F3/G1 落地须依据这份结构契约固定准确的 Godot 可调用方法名和返回记录。

### 功能覆盖矩阵

此矩阵防止后续任务把某类玩法当作没有所有者的扩展。当前实际支持的载荷变体由现有封闭领域命令/事件集和 v45 注册表决定，而非本表的概括文字。

| 功能组 | 权威输入与结果所有者 | 客户端/展示投影 |
| --- | --- | --- |
| 登录、身份、权限、断开、保活 | P0 准入加 S1/S2 会话控制；控制报文不属于 `Event` | C1 `session@1`、G1 生命周期 |
| 世界种子、区块生成、方块、光照、可见性与重同步 | K0 确定性算法；S1 世界状态及 `ChunkSnapshot`/`BlockChanges`/`ForgetChunks` 路由；S3 存档 | C1 镜像、C2 `terrain@1`、P8 资源 |
| 物理、碰撞、射线、移动与镜头目标 | K0 数值工作；S1 校验 `PlayerControl` 并发布 `PlayerState` | C1 预测/纠正、`player-view@1`、P9 场景镜头 |
| 时间、季节、天气、温度、流体与生存 | S1 在 K0 内核上执行规则；`WorldState`/`PlayerState` 和方块变化 | C2 `world-ui@1`、`terrain@1`、`audio-cues@1` |
| 挖掘、放置、耕种、门、火把与方块更新 | S1 校验命令/资源，发布世界变化及拒绝/成功 | C1 确认结果、P8 地形和 P10 交互 UI |
| 快捷栏、护甲、物品堆、合成、熔炉、箱子与容器 | S1 掌管守恒与权限；`InventoryState`、`CraftingState`、`FurnaceState`、`ChestState`、`ContainerClosed` | C2 `inventory-ui@1`、P10 Godot Control |
| 掉落物、远端玩家、敌对/被动生物、抛射物与战斗 | S1 通过封闭领域事件掌管生成/状态/消失及命中结果 | C1 身份/顺序、C2 `actors@1`、P9 场景/特效 |
| 伙伴、对话、任务与候选动作 | 独立 Agent 给出建议；S4/S1 重校验并发布伙伴/聊天/任务结果 | C2 `actors@1`/`world-ui@1`/`audio-cues@1` |
| 持久化、迁移与恢复 | S0 编解码器；S3 独占 I/O 与持久确认 | 客户端无存档 API；仅 P13 激活/回滚诊断 |
| 输入设备、UI、音频、资产与资源释放 | C1 校验语义意图；Godot/Python 拥有设备及视觉/音频资源 | G1 家族和 `AssetManifest`；没有权威设备回调 |
| 回放、视觉证据、诊断与发布 | T1 离线工具和清单消费所有者结果 | P12/P13/P14 门禁；不得成为第二在线权威 |

## 8. 工具、资产与部署边界

- 回放记录引用准确的协议/存档/契约身份、种子、有序输入日程、tick 检查点和预期权威事件/状态哈希。工具比较两个独立离线运行并报告未覆盖家族，不作为第二个在线写入者。
- 诊断是有界观察结果，不是命令总线。指标和追踪记录包含所有者、epoch 或 tick 关联以及溢出计数；身份缺失、案例截断或 I/O 错误的报告不能通过验收。
- `AssetManifest` 是带版本的**构建/分发契约**，不属于桥接家族表。它记录合法资产身份、内容哈希、资源种类、兼容材质/声音映射和目标平台。资产从获授权来源离线构建或同步；运行时家族载荷只携带稳定引用与哈希，不携带任意路径或文件字节。必需资产缺失时功能激活以有类型原因失败。Godot 在自身线程释放 GPU/音频资源。
- 桌面启动器监督一个 Rust 服务器子进程和一个 Godot 客户端；本地游玩使用回环 TCP；固定产物/契约身份、检测子进程退出并在收尾时关闭它。Memory 适配器仍是 F2/F3 的必需一致性测试面。激活时取得世界独占所有权；回滚须先停止写入者，并验证存档兼容性或从具名备份恢复后再启动上一运行时。目标架构不包含自动协议/存档降级或 Go 回退。

## 9. 并行开发协议与验收

```text
已接受 D0/P0/S0 + 完成 K0（F1）
        │
        ├─ S1 服务器核心共享契约 ─► F2 提供者 S2/S3/S4 ─► 串行 F2 集成
        │                                    │
        └────────────────────────────────────┴─► C1 客户端共享契约
                                                  └─► C2 帧契约
                                                       └─► G1 独占桥接/注册表落地
                                                            ├─ P8 地形
                                                            ├─ P9 角色
                                                            ├─ P10 UI
                                                            └─ P11 音频
                                                                 └─ P12/P13 验收 ─► P14
```

两个任务消费新边界前，所有者先落地可编译声明、已校验夹具，以及能执行成功和有类型失败路径的消费者替身。ledger 记录已接受 SHA、精确命令及非零测试数。提供者任务随后拥有互不重叠的文件；注册表/适配器与最终真实生产者—消费者测试由串行编辑者负责。F2/F3 阶段计划仍受其原有前置条件阻塞；本图不豁免它们。每个任务简述引用相应登记行和已冻结的家族 schema，声明可编辑/只读路径、来源基线、预期失败/成功、上限与错误测试和回滚。发现不一致须交回主控者修订契约包并接受新 SHA；工作者不能擅自扩大公共类型。

验收分为契约/替身、真实提供者、真实集成三个独立结果。D0/P0/S0 用现有聚焦测试和兼容夹具；S1–S4 包含 Memory/TCP 一致性、确定性回放、饱和、取消、Agent 超时与存储失败；C1/C2/G1 包含无界面纠正/重放、混合 epoch 拒绝、桥接整批校验、功能协商和重复销毁；P8–P11 按需包含语义、设备/无界面和视觉证据；P13/P14 包含独占激活、导出产物身份和回滚。截图、通过的 mock、仅编译或规划勾选都不能单独证明运行时验收。

## 10. 变更控制与已知缺口

本文在实现前冻结**所有权、依赖方向、语义类别、身份和失败策略**。准确的 Rust 声明、新队列数值、二进制家族布局和数字 ID 只有在各自可编译接口落地并记录 SHA 后才成为可调用约束。这样的区别既避免把尚未构建的 API 说成已存在，也让后续任务共享一套总接口图。

发表时仍有这些缺口：完整 F1 数值门面/寻路及零缺口验收；F2 server crate 与经过测量的队列/时限契约；F3 client-core crate 与 Rust 核心 Godot 生产者；目标语义家族 schema/注册表；音频/生命周期从符号名到数字名的宿主协商；完整真实集成证据。上表为每项指定了所有者。未来功能若不在表中，应先以 OpenSpec delta 明确权威、生产者/消费者、语义家族或私有接口、版本影响、上限、测试与契约落地；不得分叉现有服务器、客户端镜像或宿主桥接。
