# codec 包：编解码与帧封装

`packages/shared/network/codec` 承载线上协议的字节层：packet↔wire 双向编解码、
zstd 压缩的 chunk snapshot 编解码、长度前缀帧封装与字节原语。协议语义
（密封 packet、冻结 ID、`Validate`）住 `packages/shared/network/protocol`，会话
与传输编排住根包；本包只依赖 `protocol` 与 `packages/shared/core`（依赖方向由
`packages/audit` 的 `TestInternalDependenciesAreOneWay` 强制），wire
长度上限经 `protocol` 导出常量取值，不 import `packages/shared/companion`。
全树共享的信任边界与协议演进纪律见上级 `../AGENTS.md`，本文件不重复。

## 规范 uvarint 与字节原语 (`codec/codec_primitives.go`)

- `byteEncoder`/`byteDecoder` 是唯一字节原语：定宽整数一律 little-endian，
  uvarint 只接受规范编码，bool 只接受 0/1，string 拒绝非法或超长 UTF-8，
  float 拒绝 NaN/Inf。`TestPrimitiveFixedWidthEncodingIsLittleEndian`、
  `TestCanonicalUvarintBoundaries`、`TestPrimitiveBoolOnlyAcceptsZeroOrOne`、
  `TestPrimitiveStringRejectsInvalidOrOversizedInput`、
  `TestPrimitiveFloatRejectsNonFiniteValues`、
  `TestPrimitiveDoneRejectsTrailingBytes` 逐项钉死；`FuzzPrimitiveDecoder`
  以随机输入守住解码器不变量。
- 原语是本包私有实现：protocol 层与根包不直接消费，导出原语等于扩大
  攻击面并绕过 `Validate` 前置。

## 编码原子性与分发 (`codec/codec_client.go`, `codec/codec_server.go`)

- `byteEncoder` 的 fail-first 语义保证编码原子：首个错误被记录后跳过
  后续写入，只回传错误、不产出半截 payload；多记录消息的校验与往返
  原子性由 `TestChatEventCompanionSpeechCombinationsAreRejectedAtomically`、
  `TestChatEventTaskLifecycleCombinationsAreRejectedAtomically`、
  `TestCompanionStatesRejectsFiveDuplicateOrUnsortedAtomically` 钉死。
- `encodeClientPacketPayload`/`decodeClientPacketPayload`/
  `encodeServerControlPayload`/`decodeServerControlPayload` 按 state 与
  包 ID 分发，ID 查表只经 `protocol` 访问器；解码尾部统一转调
  `protocol.ValidateDecodedClientWirePacket`，解码输出永远通过完整校验。
  `TestSmallPacketRejectsMalformedPayloads`、
  `TestPlayClientPacketIDOneIsUnknown`、
  `TestCodecDelegatesControlPacketsAndCloseIsIdempotent` 钉死分发与拒绝。
- companion/hostile/projectile 的 wire 编解码函数住 `companion_wire.go`/
  `hostile_wire.go`/`codec_projectile.go`，保持 unexported 包内直呼：它们消费
  `protocol` 导出
  的 wire 长度常量取上限，不得改从 `packages/shared/companion` 直接取值
  （否则破坏 archcheck 边与常量同源契约）。

## 帧封装 (`codec/frame.go`)

- `WriteFrame`/`ReadFrame` 是线上唯一的帧边界：长度前缀为规范 uvarint 且
  不含自身，0 长度与超过 `MaxFrameBytes` 的帧一律拒绝。`ReadFrame` 在
  规范性与帧长校验通过前不按声明长度分配内存，
  `TestFrameReadRejectsInvalidLengthsBeforeFrame`、
  `TestFrameReadOwnsSingleFrameAllocation`、
  `TestFrameReadRejectsShortFrameAndInvalidPacketID`、
  `TestFrameWriteHandlesShortWritesAndBounds`、
  `TestFrameSplitAndCoalescedReads` 与 `FuzzReadFrame` 钉死；根包 `tcp`
  transport 只经 `network.ReadFrame`/`network.WriteFrame` 别名消费，
  不得自带帧实现。

## 上限守卫与 chunk snapshot (`codec/codec.go`, `codec/chunk_codec.go`)

- 每层读取都先守住上界再分配或解压：单 packet payload 受
  `MaxSmallPayload`（`checkSmallPayload`，`TestSnapshotBounds` 钉死越界
  拒绝）、压缩 snapshot 受 `MaxCompressedSnapshot`、解码 snapshot 受
  `MaxDecodedSnapshot`（`TestChunkSnapshotRejectsZstdExpansionBeyondLimit`、
  `TestSnapshotBounds`）、drop 批量受记录数上界
  （`TestItemDropDecodeRejectsOversizedCountBeforeAllocation`）。放宽任何
  一层都会把解压/分配 DoS 面暴露给 wire。
- chunk snapshot 编解码维护逻辑视图与 wire 视图的对账：
  `TestLogicalSnapshotSizeMatchesWire`、
  `TestChunkSnapshotLogicalWireCoversAllStorages`、
  `TestWorstLegalLogicalSnapshotHasOneExactAllocation`（最坏合法快照恰好
  一次分配）钉死；`NewCodec` 构造 zstd encoder/decoder 时钉死单并发、
  encoder CRC 与 decoder 内存上限（`WithDecoderMaxMemory(MaxDecodedSnapshot)`
  加 cap-limit）等选项，它们是解压 DoS 守卫的一部分，改动须连同上限
  测试与基准一起评估（数值只记录）。

## fuzz 与 golden 义务 (`codec/*_fuzz_test.go`, `codec/codec_golden_test.go`, `codec/testdata/`)

- wire 域的全部 7 个 fuzz 入口（`FuzzSmallPacketCodec`、
  `FuzzChunkSnapshotCodec`、`FuzzReadFrame`、`FuzzPrimitiveDecoder`、
  `FuzzCompanionMessageCodec`、`FuzzHostileMessageCodec`、
  `FuzzProjectileMessageCodec`）住本包：新增或
  改动任何 wire 布局必须同步扩展对应 fuzz 种子与断言，删除或跳过即违反
  协议演进契约。
- golden 是 wire 字节的冻结证据：`TestProtocolV1SmallPacketGolden`、
  `TestProtocolV2RemotePlayerGolden`、`TestProtocolV4DropGolden`、
  `TestCompanionMessageGolden`、`TestProtocolV12ChestStateGolden` 内嵌
  hex 夹具，`TestChunkSnapshotV1Fixture` 只读
  `testdata/chunk-snapshot-v1.bin`（本子树唯一版本化 fixture，随包原地
  演化，不复制第二份）。改布局导致 golden 失败时按协议演进流程升版，
  不得改 golden 迁就实现。

## helper 中心与回归测试 (`codec/codec_helpers_test.go`)

- 共享 wire 夹具收敛在 `codec_helpers_test.go`（`remotePlayerStatesWireFixture`
  等）；`benchmark_helpers_test.go` 只承载 chunk codec 域基准夹具
  （`worstLegalBenchmarkSnapshot`）。新增共享夹具先进 helper 中心
  （规则见 `docs/test-organization.md`）。
- 版本冻结回归（`TestProtocolV*` 族）与 Validate/wire 混合测试按被测
  主体落位：纯 ID/Validate 语义在 `protocol`，wire 往返与拒绝矩阵在本包；
  移动测试不得为访问私有状态而扩大生产 API。

## Focused Verification

- 定点测试：`go test ./packages/shared/network/codec -race -count=1`。
- fuzz 冒烟（改动 wire 布局时）：`go test ./packages/shared/network/codec -fuzz
  FuzzSmallPacketCodec -fuzztime 10s`（其余入口同式，性能数值只记录）。
- 协议演进连带传输与登录：`go test ./packages/shared/network/... -race -count=1`。
- 依赖方向与文档守卫：`make archcheck`。
