---
doc_id: compatibility-guide
doc_revision: 2026-10-08.1
language: zh-CN
counterpart: compatibility.md
---
# 兼容性与升级

[English](compatibility.md)。本文承接 README「兼容性与升级」一节，给出线上协议、世界/玩家/伙伴/动物存档的版本契约、升级前后的备份与回退纪律，以及 benchmark 报告兼容规则；全部条目以当前代码与 `openspec/specs/` 为准。

当前版本号只在下方「当前版本矩阵」维护；其他说明文档链接到这里，不再各自写版本号（[局域网专用服务端](lan-server.md)与[玩家手册](gameplay.md)仍是待迁移的旧文档，迁移为中英双语时改为链接本节）。矩阵由 `packages/audit` 的 `TestCompatibilityMatrixMatchesSource` 与源码常量逐项比对，表与源码不一致时测试失败。

## 当前版本矩阵

| 契约 | 当前版本 | 权威源码常量 |
|---|---|---|
| protocol | v45 | `packages/shared/network/protocol/packet.go` `ProtocolVersion` |
| player schema | v9 | `packages/server/storage/player/player_codec.go` `CurrentSchema`；Rust `mornlea_storage/src/player.rs` `CURRENT_SCHEMA` |
| chunk schema | v9 | `packages/server/storage/chunk/chunk_codec.go` `currentChunkSchema`；Rust `mornlea_storage/src/chunk.rs` `CURRENT_SCHEMA` |
| world metadata | v6 | `packages/server/storage/metadata.go` `currentMetadataVersion` |
| `companions.ai` schema | v5 | `packages/server/storage/companion/companion_codec.go` `CurrentSchema`；Rust `mornlea_storage/src/companion.rs` `CURRENT_SCHEMA` |
| `hostile_mobs` schema | v2 | `packages/server/storage/hostile/hostile_codec.go` `CurrentSchema`；Rust `mornlea_storage/src/hostile.rs` `CURRENT_SCHEMA` |
| `passive_mobs` schema | v1 | `packages/server/storage/passive/passive_codec.go` `CurrentSchema`；Rust `mornlea_storage/src/passive.rs` `CURRENT_SCHEMA` |
| engine ABI | v11 | `packages/engine/include/mornlea_engine.h` `MORNLEA_ENGINE_ABI_VERSION`；Rust `mornlea_engine/src/ffi.rs` `ABI_VERSION` |
| client ABI | v19 | `packages/engine/include/mornlea_client.h` `MORNLEA_CLIENT_ABI_VERSION`；Rust `mornlea_client/src/ffi.rs` `CLIENT_ABI_VERSION` |
| benchmark scenario | v23 | `packages/client/cmd/mornlea/benchmark/benchmark.go` `scenarioVersion` |

这张表与根 `AGENTS.md` 的基线句、`openspec/config.yaml` 的版本矩阵同源（后两者由 `TestBaselineVersionsMatchCode` 守护）。`hostile_mobs.bin`、`passive_mobs.bin` 与 `companions.ai` 的外层 envelope 版本另有常量（当前均为 1），表中记的是记录布局的 schema 版本，两者不要混写。

## 线上协议

- 当前协议版本见上方矩阵（定义在 `packages/shared/network/protocol` 的 `ProtocolVersion`，根包 `packages/shared/network` 别名再导出）；所有不匹配版本都会在握手阶段、进入 Play 前被稳定拒绝，不提供版本协商或降级解码；
- 逐版语义以 `packages/shared/network/protocol/packet.go` 顶部注释为准，本文不再逐版重写；
- 客户端只声明按键意图，权威结算全在服务端；`SaturationZero` 是瞬态提示位、不入任何存档，饱和度与疲劳数值是纯服务端量、不占 wire 字段；`DayPhaseOffset` 只经世界 metadata 持久化，不进玩家存档。

### 历史：协议 v26–v34 逐版说明（截至协议 v34 的原文）

- 线上协议为 v34（定义在 `packages/shared/network/protocol` 的 `ProtocolVersion`，根包 `packages/shared/network` 别名再导出）；所有不匹配版本都会在握手阶段、进入 Play 前被稳定拒绝，不提供版本协商或降级解码；更早版本的逐版语义见 `packages/shared/network/protocol/packet.go` 顶部注释；
- 近几版协议全部是既有 packet 尾部追加或新增消息，v34 起 `PassiveState` record 步长例外由 37 变为 38，其余既有长度上限不变，也不新增 `RejectReason`：
  - v26 新增 Play S→C ID 20 `PlaceBlockSucceeded(sequence)`，只回发给放置发起会话作为成功放置确认；
  - v27 新增 Play C→S ID 14 `BoneMeal`，与 `TillSoil` 同形（序号 + 朝向），目标格由权威射线决定；
  - v28 在 `PlayerInput` 尾部 `Eating` 之后追加 1 字节 `Sprinting` 疾跑意图位；
  - v29 在 `PlayerState` 尾部 `Hunger` 之后、`WorldTimeTicks` 之前追加 1 字节 `SaturationZero` 饱和度归零提示位；
  - v30 新增 Play S→C ID 22/23/24 三类夜行者消息 `HostileSpawn`/`HostileState`/`HostileDespawn`：每类为 `ServerTick` u64 加 count u8 加至多 64 条按 ID 严格升序的记录（spawn 携带 ID/维度/位置/朝向/生命，state 携带 ID/位置/速度/朝向/生命，despawn 只携带 ID），按会话视野订阅发布；
  - v31 在 `PlayerState` 尾部 `SaturationZero` 之后、`WorldTimeTicks` 之前追加 2 字节 `DayPhaseOffset` 显示相位偏移（u16，值域 `0..23999`，越界在校验与编解码处稳定拒绝）；显示相位按 `(WorldTimeTicks + DayPhaseOffset) % 24000` 计算，偏移只平移呈现相位、不回写绝对时间；
- v32 在 Play S→C registry 尾部追加 ID 25 `CombatHit(ServerTick, Damage, TargetKind)`；固定 10-byte 载荷只私发给成功攻击的玩家会话，受击者、旁观者、trusted observer 与 hostile 攻击目标均不接收；
- v33 在 Play S→C registry 尾部追加 ID 26/27/28 三类被动牛消息 `PassiveSpawn`/`PassiveState`/`PassiveDespawn`：每类为 `ServerTick` u64 加 count u8 加至多 64 条按 ID 严格升序的记录（spawn 携带 ID/维度/位置/朝向/生命，state 携带 ID/位置/速度/朝向/生命，despawn 只携带 ID），按会话视野订阅发布；
- v34 在 `PassiveState` record 尾部 `Health` 之后追加 1 字节放牧标志 `Grazing`（u8，仅 0/1 合法，越界在校验与编解码处稳定拒绝；record 步长由 37 变为 38）；放牧态是瞬态呈现位、不入任何存档；
- 客户端只声明按键意图，权威结算全在服务端；`SaturationZero` 是瞬态提示位、不入任何存档，饱和度与疲劳数值是纯服务端量、不占 wire 字段；`DayPhaseOffset` 只经世界 metadata v3 持久化，不进玩家存档。

## engine 与 client ABI

- 数值引擎的 engine ABI 版本见上方矩阵：同一次构建的 Go binary 与 `libmornlea_engine` 是不可跨版本混装的 release unit；`packages/shared/nativeabi`、C header 和 Rust 动态库必须同步替换，不提供旧版 fallback；逐版变化见 `packages/engine/crates/mornlea_engine/src/ffi.rs` 中 `ABI_VERSION` 处的注释；
- 图形客户端的 client ABI 版本见上方矩阵：同一次构建的 `mornlea` 与 `libmornlea_client.dylib` 是不可跨版本混装的 release unit；无图形专服不链接 client 库，不受 client ABI 演进影响；无参数 identity export `mornlea_client_abi_version()` 只报告实际动态库身份，不接收期望版本，也不执行协商或返回 client status；逐版变化见 `packages/engine/crates/mornlea_client/src/ffi.rs` 中 `CLIENT_ABI_VERSION` 处的注释；
- 协议、存档 schema 与 benchmark scenario 不随两条 ABI 演进，既有世界与玩家存档在新客户端上照常读取。

### 历史：engine ABI v10 与 client ABI v12–v14 说明（原文）

- 数值引擎的 engine ABI 为 v10：同一次构建的 Go binary 与 `libmornlea_engine` 是不可跨版本混装的 release unit；v10 相对 v9 的变化是把 worldgen 请求固化为 `MGW1` layout 3 的 15 材质契约（v9 曾在 v8 mesh registry surface 上增加流体批量求值与重扫入口），`packages/shared/nativeabi`、C header 和 Rust 动态库必须同步替换，不提供旧版 fallback；
- 图形客户端的 client ABI 为 v14：同一次构建的 `mornlea` 与 `libmornlea_client.dylib` 是不可跨版本混装的 release unit；无图形专服不链接 client 库，不受 client ABI 演进影响；
- 无参数 identity export `mornlea_client_abi_version()` 只报告实际动态库身份 14，不接收期望版本，也不执行协商或返回 client status；
- selected-main v13 的 28 个 versioned exports 与当前 v14 的 29 个 versioned exports 在版本不匹配时都返回 `ABI_VERSION`，并在读取语义输入或改变 window、renderer、capture、UI/cache 状态前拒绝调用；
- v14-only 的 `mornlea_client_render_apply_world_updates` 在 selected-main v13 动态库中不存在；要求该 MRW1 symbol 的 v14 bridge 与 v13 动态库组合时必须在 link、load 或 bind 阶段硬失败，不进入 FFI body、不返回 client status，也不使用兼容入口或 fallback；
- v13 引入 window composite capture：保留两段式容量查询、紧凑 top-down BGRA8 输出与 Go `Window.Capture` bridge；v14 原样保留该 surface 及菜单/游戏帧循环中 poll 后、render 前的 single-outstanding capture pump；
- v12 引入进程内 WKWebView 菜单桥：`render_upload_ui_font` 出口与帧 TLV tag 9 UI 段（layout v1–v4 编解码）退役（下发即 `INVALID_ARGUMENT`），新增 `ui_push_state` JSON 状态下行出口，`render_drain_ui_events` 签名不变、字节格式改为版本化 JSON 事件信封（空队列 0 字节）；这些 surface 在 v13 与 v14 中继续保留；

## 存档版本

- 世界 metadata 的当前版本见上方矩阵：受支持的旧版本读入即迁移，并在下一次正常自动保存或关服时写为当前版本；只认识旧版本的程序遇到未来 metadata 必须稳定拒绝且不得覆盖原文件；逐版布局见 `packages/server/storage/metadata.go` 注释；
- 玩家存档的当前 schema 见上方矩阵：受支持的旧版本沿既有迁移链读取，迁移结果在下一次正常保存时改写为当前版本；未来版本必须稳定拒绝且不得覆盖；逐版布局见 `packages/server/storage/player` 与 `mornlea_storage/src/player.rs` 的注释；
- 区块存档保持 schema v9：v1..v8 沿既有迁移链读取，其中 v8→v9 是恒等迁移，不为旧区块追注水，已接受的代价是新旧区块之间出现干湿边界；未来版本必须稳定拒绝且不得覆盖；
- 夜行者独立写入世界根目录的 `hostile_mobs.bin`，当前写出 schema v2（记录尾部追加 1 字节 kind）：固定头加至多 64 条定长记录，CRC 与逐项合法性校验覆盖全文件，任何损坏、越界或非法记录都整份拒绝且视为无有效存档（缺失文件视为空集合，恢复失败不会以空集合覆盖旧文件）；v1 旧文件只读放行（kind 取 0，下次正常保存才升级），未来版本必须稳定拒绝；
- 被动动物独立写入世界目录的 `passive_mobs.bin` schema v1：魔数 `PMST`，固定 32 字节头加至多 32 条 72 字节定长记录，记录按 ID 严格升序，CRC-32C 覆盖身份字段与载荷；长度不符即视为损坏；逃跑计时、出生区块等运行期事实不入档，恢复时重新派生；该文件没有历史版本，未来版本必须稳定拒绝；
- 伙伴状态独立写入世界根目录的 `companions.ai` schema v5：v1..v4 只读迁移、encoder 只写 v5，物理上限 393,904 bytes；active 与 inactive 身体记录合计最多 64 条；active 记录可持久化当前任务、FIFO 与恢复镜像，inactive 只保留停用 tombstone，名称和生效 persona 始终来自当前配置；
- 列顶高度表、天空光和静态方块光只从权威方块镜像派生，不写入区块、玩家或伙伴存档，也不进入网络 payload；程序化天空只消费权威世界时间。

### 历史：世界 metadata v3、玩家 schema v8 与 `hostile_mobs` schema v1 说明（原文）

- 世界 metadata 保持 v3，在 v2 载荷末尾追加 8 字节 `DayPhaseOffset`（u64，值域 `0..23999`，越界旧值读入时归一），既有段布局一字不动；v1/v2 世界读入即迁移：世界时间保持原值、偏移取 `0`（显示相位行为不变），并在下一次正常自动保存或关服时写为 v3；只认识旧版本的程序遇到未来 metadata 必须稳定拒绝且不得覆盖原文件；
- 玩家存档保持 schema v8：在饥饿状态之后追加定长 17 字节重生点尾段（present 标志 1 字节 + 床尾格坐标 3×f32 12 字节 + 维度 u32 4 字节，无重生点时以 present=0 占满 17 字节）；受支持的 v1..v7 沿既有迁移链读取（例如 v4 一律补为满血、v6 按新玩家初值补齐三层饥饿状态、v7 一律迁移为「无重生点」，死亡重生沿用世界出生锚点语义），迁移结果在下一次正常保存时改写为当前版本；未来版本必须稳定拒绝且不得覆盖；
- 夜行者独立写入世界根目录的 `hostile_mobs.bin` schema v1：固定头加至多 64 条定长记录，CRC 与逐项合法性校验覆盖全文件，任何损坏、越界或非法记录都整份拒绝且视为无有效存档（缺失文件视为空集合，恢复失败不会以空集合覆盖旧文件）；该文件没有历史版本，未来版本必须稳定拒绝；

## 备份与回退

- 升级前必须正常关服，等待玩家、伙伴与世界存储刷写完成并备份完整世界目录，再启动新版本程序；
- 回退时必须先停服，再恢复升级前的完整备份；不承诺把上方矩阵中任何当前版本的 metadata、区块、玩家档、`hostile_mobs.bin`、`passive_mobs.bin`、`companions.ai` 或新物品降级写回，不能让旧程序直接打开已升级目录后继续写入；
- 异常退出时玩家、伙伴与区块文件各自原子，但它们之间没有跨文件事务。

## 内嵌默认材质包

- 内嵌默认包为 Pastelcraft 子集（MIT，作者 XradicalD，版本 `Pastelcraft 1.21.11 [R21]`；
  来源、改名映射与逐文件哈希见 `packages/client/assets/packs/pastelcraft/` 的
  `ATTRIBUTION.md`/`PROVENANCE.json`），替代此前的 Pixel Perfection 子集
  （CC BY-SA 4.0）；牛肉图标仍沿用 OpenGameArt `16x16 Food`（CC0），牛皮与牛头
  仍是程序化像素；
- 槽位名（`textures/<name>.png`）、`pack.json` 契约（`format=1`）、16×16/≤64KiB
  全包拒绝语义与缺槽位程序化回退均不变，用户目录 override 按同名槽位继续生效；
- `openspec/specs` 无需为本次换肤变更：`texture-pack-loading` 的行为契约未动，
  变化的只是默认包内的像素内容（随所属 change 的 delta spec 生效与归档时同步）。

## benchmark 报告兼容

- benchmark producer 的当前 scenario 版本见上方矩阵，固定输入仍是七名远端玩家、零伙伴、不注入聊天，被测世界不注水且不含农业方块；scenario 版本变化记录的是被测进程本身的改变（HUD 固定上传布局与保留面最坏组合、每帧实例前缀字节数、HUD 图集列数、稳定方块与 mesh registry、worldgen ABI、权威 tick 工作量等），性能数值只在同 scenario 版本内可比；逐版理由见 `packages/client/cmd/mornlea/benchmark/benchmark.go` 中 `scenarioVersion` 处的注释与 [性能基线](perf-baseline.md)；
- 旧 scenario 版本的报告仍可读取并做同版本比较；跨 scenario 比较只接受 `packages/tools/perfcheck/compare.go` 允许的唯一显式迁移，当前是 `--allow-scenario-upgrade 22:23`，更早的迁移授权已退役，只作归档证据；
- 跨 transport 比较要求两侧 scenario 版本与 `git_commit` 都一致，否则拒绝；
- 性能数值只记录，不改变退出状态；报告结构、来源身份、真实 overflow、数据丢失和 I/O 错误仍然硬失败。

### 历史：scenario v22 时期说明（原文）

- benchmark producer 为 scenario v22，固定输入仍是七名远端玩家、零伙伴、不注入聊天，被测世界不注水且不含农业方块（v22 起固定世界在合格草地上方确定性新增自然短草）；scenario 版本变化记录的是被测进程本身的改变（HUD 固定上传布局与保留面最坏组合、每帧实例前缀字节数、HUD 图集列数、稳定方块与 mesh registry、worldgen ABI、权威 tick 工作量等），性能数值只在同 scenario 版本内可比；
- 旧 scenario 版本的报告仍可读取并做同版本比较；跨 scenario 比较只接受显式 `--allow-scenario-upgrade 21:22`，这是当前唯一显式迁移授权（历史的 `20:21` 随 producer 升到 scenario v22 退役，只作归档证据，不再是当前可授权迁移）；
