# passive 包：passive 存档域 codec

`packages/server/storage/passive` 承载 passive（被动牛）存档域：passive_mobs.bin
聚合文件（PMST 信封）的编解码、记录字段校验与被动牛存档值类型。本包是
纯 codec 域：只依赖 `packages/shared/core` 并经 `packages/server/storage/storagedef`
取哨兵；不感知根包编排（passive_mobs.bin 文件的原子替换与路径编排在根
包 DiskStore/MemoryStore），`PassiveMobStore` 接口属根包存储契约家族，
定义留在根包 `types.go`。依赖方向由 `packages/audit` 的
`TestInternalDependenciesAreOneWay` 强制。行为规格见 change
`passive-cattle-b27` 的 `passive-mob-persistence` 能力规格；全树共享的迁移与数据安全
纪律见上级 `../AGENTS.md`，本文件不重复。

## 信封编解码 (`passive/passive_codec.go`, `passive/codec_primitives.go`)

- `Encode`/`Decode` 是本域唯一编解码入口：
  头部与记录布局由 `TestPassiveCodecHeaderAndRecordLayout` 钉死，golden
  字节往返由 `TestPassiveCodecGoldenRoundTrip` 冻结（当前 schema 的
  golden 演进随本包 testdata，改布局必须显式重生成并评审字节）。
- `MaxPassiveMobs` 是一份存档可包含的记录数上限（与权威侧全服被动牛
  上限同源），`MaxFileLength` 是物理文件字节上界；编码与解码两侧都在
  解析前拒绝越界，`TestPassiveCodecAcceptsMaximumRecordsAndEnforcesFileLimit`
  钉死。取值以代码常量为准，不在指南复制数字。
- 损坏文件拒绝与非法保存请求拒绝由 `TestPassiveCodecRejectsCorruptFiles`、
  `TestPassiveCodecRejectsInvalidSaves` 钉死；空集合往返由
  `TestPassiveCodecEmptyCollectionRoundTrip` 钉死。
- 规范形不变量：编码结果与输入记录顺序无关（写盘前规范化排序），由
  `TestPassiveCodecCanonicalFormIndependentOfInputOrder` 钉死；解码成功
  的字节必须是规范形（re-encode 逐字节相等）。
- `codec_primitives.go` 是本域私有字节原语副本（byte + f32 子集）：与
  hostile/chunk/player/companion 包的同名助手同源，域内 codec 是唯一消费方，
  域间不共享原语包。

## 自包含值类型 (`passive/passive_types.go`)

- `StoredPassiveMob`/`StoredPassiveMobs`/`PassiveMobsSave` 在本包内自包含
  定义：权威侧的被动牛身体类型属于 `packages/server/sim`，存储子树不得依赖
  sim（archcheck `allowed` 表中本包允许集无 sim），server 装配层负责
  两者之间的转换。字段语义以字段注释为准：逃跑计时、出生区块与新生标记
  是运行时派生物，刻意不落盘，恢复后逃跑清零、出生区块按加载位置重新锚定。
- 记录尾段是 schema v1 保留段（编码恒写零、解码遇非零即损坏），为未来
  字段演进占位而不破坏固定记录步长。
- `ErrPassiveMobsNotFound`/`MaxPassiveMobs` 随域定义，根包以同一错误值/
  常量再导出保持 `storage.X` 引用与 `errors.Is` 身份不变；存档缺失由
  调用方视同空集合。

## helper 中心与回归测试 (`passive/passive_codec_test.go`)

- 共享夹具（记录构造器 `fixturePassiveRecords` 等）住
  `passive_codec_test.go`；每包最多一个 helper 中心，规则见
  `docs/test-organization.md`。
- 模糊入口 `FuzzDecodePassiveMobs`（`passive/passive_codec_fuzz_test.go`）
  以 golden 全前缀截断为种子，驱动头部与记录边界的每个截断位形。本包
  fixture 是只读 golden：与 chunk/player/companion 不同，没有
  `-update-storage-fixtures` 重写路径，字节变更走显式文件替换加评审。

## Focused Verification

- 定点测试：`go test ./packages/server/storage/passive -race -count=1`（纯
  codec 域，秒级，不编译执行其他域的测试）。
- 根包编排：`go test ./packages/server/storage -run 'PassiveStore|DiskPassive' -race -count=1`。
- 依赖方向与文档守卫：`make archcheck`。
