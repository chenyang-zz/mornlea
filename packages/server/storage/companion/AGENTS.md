# companion 包：companion 存档域 codec

`packages/server/storage/companion` 承载 companion 存档域：companions.ai 聚合
文件（MCAI 信封）的编解码、namespace identity、lifecycle、memory mirror、
tombstone、任务区/FIFO 载荷校验与伙伴存档值类型。
本包是纯 codec 域：依赖 `packages/shared/companion` 领域模型与 `packages/shared/core`
值类型，哨兵经 `packages/server/storage/storagedef` 取用；不感知根包编排
（companions.ai 文件的原子替换与路径编排在根包
DiskStore/MemoryStore），`CompanionStore` 接口属根包存储契约家族，定义
留在根包 `types.go`。依赖方向由 `packages/audit` 的
`TestInternalDependenciesAreOneWay` 强制。行为规格见
`openspec/specs/local-data-migration/spec.md`；全树共享的迁移与数据安全
纪律见上级 `../AGENTS.md`，本文件不重复。

## 信封编解码 (`companion_codec.go`, `codec_primitives.go`, `companion_v5.go`)

- `Encode`/`Decode` 是本域唯一编解码入口（原根包内部入口改名导出）：
  当前 encoder 只写 v5，decoder 以字面白名单接受 v1..v5，其中 v1..v4
  仅作只读迁移输入；`TestCompanionDecodeSchemaWhitelistListsLiteralV5`
  钉死成员资格，未来 schema 返回 `ErrFutureVersion`。
- v5 payload 以 16-byte UUIDv4 namespace 开始；每条 record 在 221-byte body
  后写 active/task/FIFO flags、memory epoch，以及 active mirror
  （revision/operation/summary）或 inactive tombstone operation。wire 与严格
  round-trip 由 `TestCompanionCodecV5WireAndStrictRoundTrip`，identity/lifecycle
  耦合矩阵由 `TestCompanionCodecV5RejectsIdentityLifecycleMatrix` 钉死。
- `MaxFileLength` 是 v5 可达结构的精确物理文件上界；编码与解码两侧均据此
  拒绝越界，`TestCompanionCodecV5MaximumReachableLength` 锁定精确最大位形。
  CRC/截断/未来版本/保留位/错位任务字段等严格拒绝由
  `TestCompanionCodecRejectsCRCTruncationFutureVersionAndOversizedRecords` 与
  `TestCompanionRestoreRejectsCorruptTaskPayloads` 覆盖。
- 记录身份约束：记录按 ID 严格升序、不允许重复，
  `TestCompanionCodecRejectsDuplicateOrUnsortedIDs` 钉死；边界容量位形
  由 `TestCompanionCodecAcceptsMaximumStoredRecords` 钉死。
- 隐私边界：存档只持久化身体快照与任务域载荷，不持久化名字、任务文本
  与 persona，`TestCompanionCodecDoesNotPersistNameTaskOrPersona` 钉死。
- `codec_primitives.go` 是本域私有字节原语副本：与 chunk/player/hostile
  包的同名助手同源，域内 codec 是唯一消费方，域间不共享原语包。

## 持久化上界同源 (`companion_types.go`)

- 任务区与 FIFO 的持久化上界常量与 `packages/shared/companion` 的运行期上界
  同源（直接引用同一常量，如任务指令上界绑定
  `companion.MaxPlanCommandBytes`、FIFO 条数绑定
  `companion.MaxTaskQueueDepth`、摘要上界绑定
  `companion.MaxDialogueSummaryBytes`）：同一常量保证「能被写进请求的
  摘要/指令」与「能被存档保留」两条边界不可能漂移。取值以这些常量为准，
  不在指南复制数字。
- `Identity`/`StoredCompanions`/`CompanionSave`/
  `StoredCompanionLifecycle`/`StoredCompanionTask`/`StoredCompanionQueue` 是域
  值类型，根包以类型别名再导出保持
  `storage.X` 引用与类型身份不变；`ErrCompanionsNotFound` 随域定义
  （「聚合存档缺失」是本域存档契约的一部分），根包绑定同一错误值再导出。
- `MergeV5` 负责 missing/legacy/v5 bootstrap：identity 按 namespace-first、
  CompanionID 升序生成，active/inactive 转换维护 epoch、mirror 与 tombstone，
  overflow、容量或 entropy 失败不得返回半迁移结果；对应回归集中在
  `companion_v5_test.go` 的 `TestCompanionMergeV5*` 族。

## 跨重启恢复与迁移 (`companion_restore_test.go`, `companion_summary_test.go`)

- 任务区与 FIFO 跨重启精确恢复、只含 FIFO 的队列往返、损坏任务载荷
  拒绝、计划步骤与 FIFO 条数上界，由 `TestCompanionRestoreTasksAndFIFOAcrossRestart`、
  `TestCompanionRestoreFIFOOnlyQueueRoundTrip`、
  `TestCompanionRestoreRejectsCorruptTaskPayloads`、
  `TestCompanionRestorePlanAndFIFOBounds`、
  `TestCompanionRestoreV3MaxVariableStepBudget` 钉死。
- committed `testdata/companions-v1.bin` 至 `companions-v4.bin` 只读迁移，首次
  保存写当前 v5；v4 summary 只经 `MergeV5` 搬入 lifecycle mirror。覆盖点
  位于 `TestCompanionCodecV1RoundTripAndGolden`、
  `TestCompanionCodecV2RoundTripAndGolden`、
  `TestCompanionCodecV3RoundTripAndGolden`、
  `TestCompanionCodecV4GoldenReadOnlyAndV5Rewrite` 与
  `TestCompanionLegacySummariesMigrateOnlyThroughMergeV5`。
- 当前 `testdata/companions-v5.bin` golden 同时覆盖 nonzero mirror、
  canonical-zero mirror、inactive tombstone、task 与 FIFO；
  `TestCompanionCodecV5GoldenRoundTrip` 普通运行逐字节比较。v5 summary 上界、
  canonical-zero 与损坏字节由 `TestCompanionCodecV5SummaryBoundariesAndSeparation`
  和 `TestCompanionCodecV5RejectsCorruptSummaryBytes` 钉死。
- 域内最小装配 `companionFileFixture`（定义在
  `companion_restore_test.go`）：以本包 `Encode`/`Decode` 直接读写
  companions.ai 文件，替代根包 DiskStore 夹具——域包测试禁止反向导入
  根包。只承载装配不改断言；原子替换由 DiskStore 用例覆盖，revision 冲突
  由留根的 `companion_store_test.go` 对 Memory/Disk 共同覆盖；关闭后 Disk
  拒绝数据 API，而 Memory 保留可观测、可复用语义，probe 仍拒绝新工作。
- `-update-storage-fixtures` flag 语义与 chunk/player 包一致（声明在
  `companion_summary_test.go`）：置位时重写本包 committed fixture，
  普通运行只读比较、漂移一律失败。

## helper 中心与回归测试 (`companion_restore_test.go`)

- 共享夹具（`companionFileFixture`、记录/队列构造器）住
  `companion_restore_test.go`；每包最多一个 helper 中心，规则见
  `docs/test-organization.md`。
- golden 冻结回归由上述 v1..v5 用例钉住；模糊入口位于
  `companion_codec_fuzz_test.go`，`FuzzDecodeCompanions` 以全部五版 committed
  fixture 与合成载荷为种子。

## Focused Verification

- 定点测试：`go test ./packages/server/storage/companion -race -count=1`（纯
  codec 域，秒级，不编译执行其他域的测试）。
- 根包编排：`go test ./packages/server/storage -run 'CompanionStore|DiskCompanion' -race -count=1`。
- 依赖方向与文档守卫：`make archcheck`。
