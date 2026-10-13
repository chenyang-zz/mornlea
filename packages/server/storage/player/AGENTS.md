# player 包：player 存档域 codec

`packages/server/storage/player` 承载 player 存档域：MCPL 信封编解码、schema
迁移链与玩家存档值类型。本包是纯 codec 域：只依赖 `packages/shared/core` 与
`packages/server/storage/storagedef`，不感知根包编排（player 文件的原子替换与
路径编排在根包 DiskStore/MemoryStore）或其他域子包；`PlayerStore` 接口
属根包存储契约家族，定义留在根包 `types.go`。依赖方向由
`packages/audit` 的 `TestInternalDependenciesAreOneWay` 强制。行为
规格见 `openspec/specs/local-data-migration/spec.md`；全树共享的迁移与
数据安全纪律见上级 `../AGENTS.md`，本文件不重复。

## 信封编解码 (`player/player_codec.go`, `player/codec_primitives.go`)

- `Encode`/`Decode` 是本域唯一编解码入口（原根包内部入口改名导出）：
  解码按「先校验信封头与负载上界、再分配」推进，
  `TestPlayerCodecRejectsPayloadOverLimitBeforeAllocation`、
  `TestPlayerCodecRejectsCorruptEnvelope`、`TestPlayerCodecRejectsInvalidSave`
  钉死入口拒绝语义。
- `EnvelopeLength`/`MaxPayload` 刻意导出：根包据此推出 player 文件的
  物理字节读取上界（根包 `disk.go` 的 `maxPlayerFileLength` 是唯一推导
  处），两包共用同一常量避免上界漂移；`CurrentSchema` 导出供根包构造
  「未来 schema」故障注入。
- 往返与字形兼容回归：`TestPlayerCodecRoundTrip`、
  `TestPlayerCodecRoundTripWithoutRespawn` 钉死基础往返；背包/快捷栏
  槽位非法载荷由 `TestPlayerCodecRejectsInvalidHotbarPayload` 钉死。
- `codec_primitives.go` 是本域私有字节原语副本：与 chunk/companion/hostile
  包的同名助手同源，域内 codec 是唯一消费方，域间不共享原语包。

## 迁移链 (`player/player_migration.go`)

- 迁移注册表逐版本连续无空洞：`TestPlayerMigrationRegistryIsContinuous`；
  未来 schema 拒绝由 `TestPlayerFutureSchemaIsRejected` 钉死。
- 负载扩展只允许在末尾追加字段：解码按「从末尾切走固定长度」逐层剥离，
  只有末尾追加才能保持旧层切分点不变、冻结的旧 fixture 仍可解码。每级
  迁移语义由 `TestPlayerV1FixtureMigratesToEmptyHotbar` 等
  `TestPlayerV*Fixture*`/`TestPlayerV*Migration*` 族以冻结 golden 钉死，
  当前 schema 字节由 `TestPlayerV9Fixture` 冻结。

## 值类型与 fixture 单一来源 (`player/player_types.go`, `player/testdata/`)

- `PlayerSave`/`StoredPlayer`/`PlayerLocation` 是域值类型，根包以类型
  别名再导出保持 `storage.X` 引用与类型身份不变；DTO 细节保持非导出。
- `player/testdata/` 的版本化 bin 是本域 fixture 唯一来源：**根包 store
  测试跨包只读本包 testdata**（根包 `player_store_test.go` 只读读取
  `player/testdata/player-v4.bin` 验证迁移编排，不在根 testdata 复制
  第二份 golden）。`-update-storage-fixtures` flag 语义与 chunk 包一致：
  本包测试二进制自声明同名 flag，置位时重写本包 committed fixture，
  普通运行只读比较、漂移一律失败。

## helper 中心与回归测试 (`player/player_codec_test.go`)

- 本包没有独立 `*_helpers_test.go`：fixture 更新 flag 与夹具住在
  `player_codec_test.go`；新增共享夹具先收敛到这里（规则见
  `docs/test-organization.md`）。
- 模糊入口 `FuzzDecodePlayer` 以 fixture 与合成载荷为种子；性能入口
  `BenchmarkPlayerCodec` 只记录数值不设门槛。

## Focused Verification

- 定点测试：`go test ./packages/server/storage/player -race -count=1`（纯 codec
  域，秒级，不编译执行其他域的测试）。
- 根包编排（原子替换、损坏/未来文件拒写、revision 复用）：
  `go test ./packages/server/storage -run 'PlayerStore|DiskPlayer' -race -count=1`。
- 依赖方向与文档守卫：`make archcheck`。
