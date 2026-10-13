# protocol 包：协议消息层

`packages/shared/network/protocol` 承载端无关的协议消息层：密封的
`ClientPacket`/`ServerPacket` packet 集合、冻结的包 ID 注册表与逐字段
`Validate` 规则。wire 字节编解码住 `packages/shared/network/codec`，会话与传输
编排住根包，本包不依赖二者。本包只依赖 `packages/shared/core` 与
`packages/shared/companion`（后者服务于常量同源锁定与消息 DTO 的领域类型，见
下）；依赖方向由
`packages/audit` 的 `TestInternalDependenciesAreOneWay` 强制。行为规格
见 openspec 主规格 `repository-code-organization`（协议版本契约承接
`docs/notes/compatibility.md`）；
全树共享的信任边界与协议演进纪律见上级 `../AGENTS.md`，本文件不重复。

## 密封接口与版本钉死 (`protocol/packet.go`, `protocol/message.go`)

- `ClientPacket`/`ServerPacket` 与 `ClientMessage`/`ServerMessage` 是封闭
  集合：marker 方法同包钉死，包外无法新增实现；只有同包类型可上 wire。
  `TestProtocolMessageShapesImplementSealedInterfaces` 钉死形状契约。
- `ProtocolVersion` 是当前唯一支持的协议版本的权威定义点（版本历史
  doc comment 亦在彼处）；升级协议时在 `packet.go` 顶部注释续写语义，
  `TestProtocolVersionPinned` 钉死版本常量，`TestProtocolV1StateAndErrorCodesAreFrozen`
  与 `TestRejectReasonsAreStableProtocolValues` 钉死 state/拒绝码枚举。
- `ValidateDecodedClientWirePacket` 放行 Handshake/Login 阶段的 wire
  packet 并对 Play 阶段转调完整校验，是解码侧的公共入口；根包 Memory
  transport 与 codec 解码路径共用它，不得另建绕过版本拒绝的放行分支。

## 冻结包 ID 注册表 (`protocol/registry.go`)

- 包 ID 访问器 `ClientPacketID`/`ClientPacketForID`/`ServerPacketID`/
  `ServerPacketForID`/`CommandRejectReasonID`/`CommandRejectReasonForID`
  是 ID 查表唯一入口：编码与按 ID 解码必须经它们，不得在别处硬编码 ID。
  `TestProtocolV1RegistryRejectsUnknownIDsAndStates` 钉死未知 ID/state
  拒绝。
- 既有 packet 的 ID 冻结：`TestProtocolV1PacketIDsAreFrozen`、
  `TestProtocolV2RemotePlayerPacketIDsAreFrozen`、
  `TestProtocolV3HotbarPacketIDsAreFrozen`、
  `TestProtocolV4DropPacketIDsAreFrozen`、
  `TestProtocolV7FurnacePacketIDsAreFrozen`、
  `TestProtocolV12ChestStatePacketIDIsFrozen`、
  `TestProtocolV22TillSoilPacketIDIsFrozen`、
  `TestProtocolV27BoneMealPacketIDIsFrozen`、
  `TestProtocolV42EquipArmorPacketIDIsFrozen`、
  `TestProjectileMessageIDsAreFrozen`、
  `TestGridCraftingPacketIDsAreFrozen`、
  `TestCommandRejectReasonIDsAreFrozen` 各钉死对应版本的 ID；新增消息只
  追加新 ID（`TestCompanionMessageIDsAreAppendOnly`/
  `TestHostileMessageIDsAreFrozen` 钉死 S→C 追加面），不改既有编号。

## Validate 契约 (`protocol/packet.go`, `protocol/snapshot.go`, `protocol/message_*.go`)

- `ValidateClientPacket`/`ValidateServerPacket` 是 Play 阶段逐字段校验的
  唯一入口：NaN/Inf、越界枚举、非法方块 ID、容量上限一律拒绝；
  `TestValidateClientPacket`/`TestValidateServerPacket` 钉死矩阵。
- `LoginStart.ViewDistance`（v40 尾部追加）的合法闭区间由
  `LoginViewDistanceMin`/`LoginViewDistanceMax` 常量钉死，域外值在
  `ValidateClientPacket` 与根包登录驱动双侧拒绝（驱动侧回冻结
  `LoginProtocolViolation`），钳制语义不落在本包。
- `ChunkSnapshot.Validate`/`SectionData.Validate` 保证压缩区段可安全解码：
  `TestChunkSnapshotValidatesCanonicalSections`、
  `TestChunkSnapshotRejectsMalformedSections`、
  `TestSectionDataRejectsUnknownBlockEveryStorage` 钉死 canonical 形状与
  未知方块拒绝；解码层的分配上界依赖这里的值域结论，放宽校验等于打开
  DoS 面。
- 逐域消息校验由对应测试钉死：`TestHotbarMessagesValidateFixedBounds`、
  `TestItemDropMessagesValidateBoundedBatches`、
  `TestHostileMessagesValidateRejectsInvalidRecords`、
  `TestContainerNeutralMessagesRejectUnknownKind`、
  `TestMoveContainerStackChestUnifiedSlotRange`。

## companion 常量同源契约 (`protocol/message_companion.go`, `protocol/message_hostile.go`)

- companion/hostile 的 wire 常量（`MaxCompanionStates`、
  `CompanionSpawnMaxWireBytes` 族、`HostileSpawnMaxWireBytes` 族、
  `ChatSpeechTextMaxBytes` 等）在本包定义并导出：
  `MaxCompanionStates` 直接取自 `companion.MaxActive`，保证协议上限与
  领域上限同源；codec 层的预分配拒绝经这些导出常量取值，因此 codec
  生产代码不 import `packages/shared/companion`。修改常量定义位置或回退为字面量
  会同时破坏同源锁定与 archcheck 边。
- `TestChatEventTaskEnumsAreFrozen`、
  `TestCompanionMessageIDsAreAppendOnly` 钉死任务枚举与消息追加面。

## 回归测试组织 (`protocol/*_test.go`)

- 本包没有独立 `*_helpers_test.go`：包 ID 冻结族测试与 Validate 矩阵
  自带同值夹具（与 codec 侧少量同值副本是按被测主体拆分的机械结果，
  两侧各自钉死取值）；新增共享夹具先收敛到最贴近的主测试文件
  （规则见 `docs/test-organization.md`）。
- 本包测试只覆盖纯协议语义（密封形状、冻结 ID、Validate）；wire 字节
  往返、golden 与 fuzz 全部住 `packages/shared/network/codec`，本包不得为触达
  unexported 编码器而复制或导出它们。

## Focused Verification

- 定点测试：`go test ./packages/shared/network/protocol -race -count=1`。
- 协议演进（ID/Validate/版本变更连带 codec 与传输）：
  `go test ./packages/shared/network/... -race -count=1`。
- 依赖方向与文档守卫：`make archcheck`。
