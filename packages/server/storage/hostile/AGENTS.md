# hostile 包：hostile 存档域 codec

`packages/server/storage/hostile` 承载 hostile（夜行者）存档域：hostile_mobs.bin
聚合文件（MHST 信封）的编解码、记录字段校验与夜行者存档值类型。本包是
纯 codec 域：只依赖 `packages/shared/core` 并经 `packages/server/storage/storagedef`
取哨兵；不感知根包编排（hostile_mobs.bin 文件的原子替换与路径编排在根
包 DiskStore/MemoryStore），`HostileMobStore` 接口属根包存储契约家族，
定义留在根包 `types.go`。依赖方向由 `packages/audit` 的
`TestInternalDependenciesAreOneWay` 强制。行为规格见
`openspec/specs/local-data-migration/spec.md`；全树共享的迁移与数据安全
纪律见上级 `../AGENTS.md`，本文件不重复。

## 信封编解码 (`hostile/hostile_codec.go`, `hostile/codec_primitives.go`)

- `Encode`/`Decode` 是本域唯一编解码入口（原根包内部入口改名导出）：
  头部与记录布局由 `TestHostileCodecHeaderAndRecordLayout` 钉死，golden
  字节往返由 `TestHostileCodecGoldenRoundTrip` 冻结（当前 schema 的
  golden 演进随本包 testdata，改布局必须显式重生成并评审字节）。
- `MaxHostileMobs` 是一份存档可包含的记录数上限（与权威侧全服夜行者
  上限同源），`MaxFileLength` 是物理文件字节上界；编码与解码两侧都在
  解析前拒绝越界，`TestHostileCodecAcceptsMaximumRecordsAndEnforcesFileLimit`
  钉死。取值以代码常量为准，不在指南复制数字。
- 损坏文件拒绝与非法保存请求拒绝由 `TestHostileCodecRejectsCorruptFiles`、
  `TestHostileCodecRejectsInvalidSaves` 钉死；空集合往返由
  `TestHostileCodecEmptyCollectionRoundTrip` 钉死。
- 规范形不变量：编码结果与输入记录顺序无关（写盘前规范化排序），由
  `TestHostileCodecCanonicalFormIndependentOfInputOrder` 钉死；解码成功
  的字节必须是规范形（re-encode 逐字节相等）。
- `codec_primitives.go` 是本域私有字节原语副本（byte + f32 子集）：与
  chunk/player/companion 包的同名助手同源，域内 codec 是唯一消费方，
  域间不共享原语包。

## 自包含值类型 (`hostile/hostile_types.go`)

- `StoredHostileMob`/`StoredHostileMobs`/`HostileMobsSave` 在本包内自包含
  定义：权威侧的夜行者身体类型属于 `packages/server/sim`，存储子树不得依赖
  sim（archcheck `allowed` 表中本包允许集无 sim），server 装配层负责
  两者之间的转换。字段语义以字段注释为准：路径与规划世代是运行时派生
  物，刻意不落盘，恢复后路径为空、首 tick 重新规划。
- `ErrHostileMobsNotFound`/`MaxHostileMobs` 随域定义，根包以同一错误值/
  常量再导出保持 `storage.X` 引用与 `errors.Is` 身份不变；存档缺失由
  调用方视同空集合。

## helper 中心与回归测试 (`hostile/hostile_codec_test.go`)

- 共享夹具（记录构造器 `fixtureHostileRecords` 等）住
  `hostile_codec_test.go`；每包最多一个 helper 中心，规则见
  `docs/test-organization.md`。
- 模糊入口 `FuzzDecodeHostileMobs`（`hostile/hostile_codec_fuzz_test.go`）
  以 golden 全前缀截断为种子，驱动头部与记录边界的每个截断位形。本包
  fixture 是只读 golden：与 chunk/player/companion 不同，没有
  `-update-storage-fixtures` 重写路径，字节变更走显式文件替换加评审。

## Focused Verification

- 定点测试：`go test ./packages/server/storage/hostile -race -count=1`（纯
  codec 域，秒级，不编译执行其他域的测试）。
- 根包编排：`go test ./packages/server/storage -run 'HostileStore|DiskHostile' -race -count=1`。
- 依赖方向与文档守卫：`make archcheck`。
