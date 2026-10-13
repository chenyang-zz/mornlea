# chunk 包：chunk 信封编解码与 region 记录层容器

`packages/server/storage/chunk` 承载 chunk 存档域：信封编解码、schema 迁移、
chunk 值类型与 region 记录层容器（`Region`）。记录层容器随本包而非
region 包：`Region.Save`/`Region.Load` 直接调用本包信封编解码并经手
`ChunkSave`/`StoredChunk`。依赖方向：`chunk` → {`region`, `storagedef`,
`core`, `world`}，禁止依赖根包或其他域子包（方向由 `packages/audit`
的 `TestInternalDependenciesAreOneWay` 强制）。行为规格见
`openspec/specs/local-data-migration/spec.md`；全树共享的迁移与数据安全
纪律见上级 `../AGENTS.md`，本文件不重复。

## 信封编解码 (`chunk/chunk_codec.go`, `chunk/chunk_codec_container.go`, `chunk/chunk_codec_logical.go`, `chunk/chunk_codec_primitives.go`)

- `Encode`/`Decode` 是有界、版本化的 zstd 信封入口（原根包内部入口改名
  导出）：解压上界在分配前拒绝，`maxDecodedChunk` 与 region 侧
  `region.MaxCompressedChunk` 分工以代码为准；损坏信封在分配前拒绝，
  `TestChunkPayloadRejectsMalformedEnvelope` 钉死。
- 未来 schema 拒绝且不改写调用方状态：`TestFutureSchemaIsRejectedWithoutMutation`、
  `TestChunkCodecRejectsFutureSchema`；保存请求整体校验
  （nil chunk、零 revision、键位不匹配）由 `TestChunkPayloadRejectsInvalidSave`
  与 `chunk.ValidateChunkSave` 覆盖。
- 编解码确定性（同输入同字节）由 `TestChunkPayloadRoundTripsDeterministically`
  钉死；derived state（高度图等）不进信封、解码后重建，由
  `TestChunkEncodingIgnoresDerivedHeights`、`TestChunkRoundTripRestoresDerivedHeights`
  钉死。

## 迁移链与版本化 fixture (`chunk/migration.go`, `chunk/testdata/`)

- 迁移注册表逐版本连续无空洞：`TestMigrationRegistryIsContinuous`；每级
  迁移语义由各 `TestChunkV*Fixture*`/`TestChunkV*Migration*` 族以冻结
  golden 逐字节钉住（含 `TestChunkV1ThroughV4FixturesChainMigrateToEmptyChests`
  的跨级链式迁移）。
- 当前 schema 的编码结果由 `TestChunkV9Fixture` 冻结，防止字节布局无声
  漂移；golden 字节归本包 `testdata/` 单一来源，只随本包演化。
- `-update-storage-fixtures` flag 语义：本包测试二进制自声明同名 flag
  （`chunk/chunk_codec_helpers_test.go`），置位时重写本包 `testdata/` 的
  committed fixture；普通运行只读比较，golden 缺失或漂移一律失败，不
  静默重建。chunk/player/companion 三包各自持有同名 flag，互不冲突。

## 记录层容器 (`chunk/region.go`, `chunk/region_space.go`)

- `Region` 是单个 region 文件的记录层容器（原根包局部类型 `*region`）：
  持有当前生效 bank 与双 bank 副本，负责落盘、读取、崩溃恢复回退与整
  文件压缩替换；根包 DiskStore 以 `map[region.RegionKey]*Region` 缓存
  编排，容器不感知根包。
- 提交原子性：保存/压缩在任何失败位形下文件都停留在旧完整版本或新完整
  版本，`TestRegionCommitFailureAlwaysReopensOldOrNew`、
  `TestRegionCompactFailureReopensCompleteCanonical`、
  `TestRegionCompactClosesCanonicalBeforeRename` 钉死；崩溃注入经
  `TestRegionCrashSubprocessAlwaysReopensOldOrNew`（子进程位形由
  `TestRegionCrashHelper` 承接）。
- 恢复回退：active bank 解码失败时回退 inactive 副本并推进 revision，
  回退资格、revision 溢出与未来载荷拒绝由 `TestRegionRecovers*` 族钉死；
  取消传播由 `TestRegionSyncHonorsCancellation`、
  `TestRegionCanceledSaveDoesNotSwitchBank` 钉死。
- 整文件只读复制：`Region.CopyTo` 在容器读锁内流式复制整个 region 文件，
  与并行的 Save/Compact 写提交互斥，供根包 Backup 走与保存方相同的
  串行化路径（复制总是完整提交点）；阻塞语义由根包
  `TestWorldBackupRegionCopyBlocksConcurrentSaveOnSameRegion`、成品可解码
  性由 `TestWorldBackupWithConcurrentSavesDuringCopyYieldsCommittedRevision`
  钉死。
- 压缩策略消费本包侧 `region.SpacePolicy`/`region.CompactionHooks`：
  触发判定的容器行为由 `TestRegionSaveReusesInactiveOnlyExtentWithoutGrowing`
  与 `TestRegionCompactReplacesFragmentedFileWithoutChangingChunks` 钉死。

## 值类型与文件注入 (`chunk/types.go`)

- `ChunkSave`/`StoredChunk` 是编排层流转的值类型，根包以类型别名再导出；
  `chunkDTO` 保持非导出，codec 边界外的代码不经手 DTO。
- `ErrChunkNotFound`/`ErrRevisionConflict` 随容器定义在本包（产生方是
  容器读写与等 revision 冲突判定），根包以同一错误值别名再导出。
- `File` 抽象 + `regionFileHooks`（原根包 `regionFile`/`regionFileHooks`）
  供根包编排测试注入文件观察桩；`Region.ReplaceFile`/`SetCompactionHooks`
  是仅有的注入入口，生产装配不得绕过 `CreateRegion`/`OpenRegion`。该装配
  约定没有专属自动化断言兜底：archcheck 的
  `TestInternalDependenciesAreOneWay` 把本包生产 import 面限制在根包
  （实测唯一生产消费方），残余的绕过只能发生在根包内部，靠 openspec 主
  规格 `repository-code-organization` 的容器编排条款评审把关。

## helper 中心与回归测试 (`chunk/chunk_codec_helpers_test.go`)

- 共享夹具（合成逻辑 chunk、信封字节拼装、fluid fixture、
  `updateStorageFixtures` flag）住 `chunk_codec_helpers_test.go`；每包最多
  一个 helper 中心，规则见 `docs/test-organization.md`。
- 容器级编排回归（save/bank 推进、region 键隔离、close 后状态）由
  `TestRegionSaveReopenAndAdvanceBank`、`TestRegionRejectsChunkFromDifferentRegion`、
  `TestRegionLoadAfterCloseReturnsClosed`、`TestRegionSaveAfterCloseReturnsClosed`
  钉死；容器/流体/箱子/熔炉/掉落物各域编解码往返由
  `chunk_container_height_test.go`、`chunk_fluid_test.go`、
  `chunk_chest_test.go`、`chunk_furnace_test.go`、`chunk_drop_test.go`、
  `chunk_light_block_test.go` 各自覆盖。
- 性能入口 `BenchmarkChunkEncode`/`BenchmarkChunkDecode` 与容量位形
  `BenchmarkDiskStoreSave32`/`BenchmarkDiskStoreColdLoad` 只记录数值不设
  门槛；模糊入口 `FuzzDecodeChunkPayload` 以 fixture 全前缀截断为种子。

## Focused Verification

- 定点测试：`go test ./packages/server/storage/chunk -race -count=1`（本子树最大
  的域，含崩溃子进程与压缩用例；快速迭代可加 `-short`）。
- 依赖方向与文档守卫：`make archcheck`。
