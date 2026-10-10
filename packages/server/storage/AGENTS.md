# 世界存储子树

本文件是 `packages/server/storage` 子树的总纲：目录地图、依赖方向、别名再导出政策
与全树共享的数据安全纪律。六个域子包的包内不变量、精确路径与钉死回归的
测试名见各自目录的 `AGENTS.md`。本子树任何目录（含子树根）不放
`CLAUDE.md`：代理沿目录祖先链读到仓库根 `CLAUDE.md`/`AGENTS.md`、
`packages/server/AGENTS.md`、本总纲与子包指南即可。

## Directory Map

```
packages/server/storage/
├── AGENTS.md                # 本总纲
├── types.go                 # Store/WorldStore/PlayerStore/CompanionStore/HostileMobStore/PassiveMobStore 接口家族、Metadata、域符号别名再导出
├── disk.go                  # DiskStore：region 句柄缓存 LRU 治理（引用计数 + 按类别拆锁）、批量保存排序与聚合文件原子替换
├── memory.go                # MemoryStore：无磁盘 I/O 的同构内存 Store（规范化编码字节 + revision 语义）
├── world_files.go           # 世界目录装配：metadata 路径编排与 world.lock 文件锁
├── metadata.go              # world metadata 编解码与旧版迁移
├── backup.go                # 世界目录整本备份与身份校验
├── chunk_keys.go            # ChunkKeys：按序枚举磁盘上已持久化区块
├── *_test.go                # 根包编排测试：store 契约、原子替换、损坏/未来版本拒写、关闭与世界锁
├── storagedef/              # 跨域哨兵叶子：ErrCorrupt/ErrFutureVersion（包注释自述，不另设指南）
├── region/                  # region 格式原语（指南见 region/AGENTS.md）
├── chunk/                   # chunk 信封编解码与 region 记录层容器（指南见 chunk/AGENTS.md）
│   └── testdata/            # chunk 版本化 fixture（golden；清单以目录为准）
├── player/                  # player 存档域 codec（指南见 player/AGENTS.md）
│   └── testdata/            # player 版本化 fixture（golden；清单以目录为准）
├── companion/               # companion 存档域 codec（指南见 companion/AGENTS.md）
│   └── testdata/            # companions 版本化 fixture（golden；清单以目录为准）
├── hostile/                 # hostile 存档域 codec（指南见 hostile/AGENTS.md）
│   └── testdata/            # hostile-mobs 版本化 fixture（golden；清单以目录为准）
└── passive/                 # passive 存档域 codec（指南见 passive/AGENTS.md）
    └── testdata/            # passive-mobs 版本化 fixture（golden；清单以目录为准）
```

## Dependency Direction

依赖方向单向，由 `packages/audit/dependency_test.go` 的 `allowed` 表
登记并以 `TestInternalDependenciesAreOneWay` 强制（`go list` 枚举 go.work
六模块的全部内部包，生产 import 逐边比对；未登记的新包直接报错）。契约文本见
openspec 主规格 `repository-code-organization`。

- 接受：根包 → {`packages/shared/core`, `packages/shared/world`, `region`, `chunk`,
  `player`, `companion`, `hostile`, `passive`, `storagedef`}；`chunk` → {`region`,
  `storagedef`, `core`, `world`}；`player` → {`core`, `storagedef`}；
  `companion` → {`packages/shared/companion`, `core`, `storagedef`}（既有伙伴
  领域边随迁）；`hostile` → {`core`, `storagedef`}；`passive` → {`core`,
  `storagedef`}；`region` →
  {`core`, `storagedef`}；`storagedef` → {}（零依赖叶子）。
- 拒绝：任何子包反向导入根包；子包之间 `chunk` → `region` 之外的相互
  依赖；存储各包依赖 `packages/shared/network`（线上消息与落盘 DTO 在 server
  装配层转换）。
- 新增子包或新边必须先证明方向合理并登记 `allowed` 表，不许先写代码后补
  登记。

## 别名再导出政策 (`storage/types.go`)

- 迁出符号在根包 `types.go` 以别名/绑定再导出，保证既有 `storage.X` 消费
  面零改动：值类型用 `type X = pkg.X` 形态（`StoredChunk`/`ChunkSave`、
  `StoredPlayer`/`PlayerSave`/`PlayerLocation`、`StoredCompanions` 家族、
  `StoredHostileMob`/`StoredHostileMobs`/`HostileMobsSave`、
  `StoredPassiveMob`/`StoredPassiveMobs`/`PassiveMobsSave`、`RegionKey`），
  错误用 `var ErrX = pkg.ErrX` 绑定同一错误值（`ErrCorrupt`/
  `ErrFutureVersion` ← `storagedef`，`ErrChunkNotFound`/
  `ErrRevisionConflict` ← `chunk`，`ErrCompanionsNotFound` ← `companion`，
  `ErrHostileMobsNotFound` ← `hostile`，`ErrPassiveMobsNotFound` ← `passive`）；`RegionFor` 是转发函数，
  `MaxHostileMobs`/`MaxPassiveMobs` 是常量再导出。别名不产生运行时转发，`errors.Is` 身份与
  错误消息逐字节不变。
- 别名清单是闭集：只覆盖消费方实际引用的迁出符号；未列入的域内导出不加
  别名，根包内部代码直接以 `region.`/`chunk.`/`player.`/`companion.`/
  `hostile.`/`passive.` 限定名消费。域包新增导出（如 `chunk.Encode`/`chunk.Decode`、
  `player.Encode`/`player.Decode`）只承接既有调用方，不为对称性加导出。
- `ErrPlayerNotFound`/`ErrWorldLocked` 仍定义在根包：产生方是根包编排
  （player 文件加载路径与世界文件锁），不经子包。

## 共享格式与数据安全纪律

以下约束对全树生效，权威陈述只在这一处；子包指南只点名各自的钉死测试，
不复述条文。

- schema 号只在各 codec 与 metadata 的代码权威常量中维护；指南和新测试
  不复制会随演进漂移的版本数字。
- 旧版本经逐版本只读迁移加载，写出只使用当前格式，高于当前版本的数据必须
  拒绝；迁移不得猜测无法从旧数据恢复的用户意图。行为规格见
  `openspec/specs/local-data-migration/spec.md`。
- 编解码入口先守住文件、记录、计数与解压上限，再分配或遍历负载。
- 独立文件更新沿用临时文件写满、sync、close、rename、父目录 sync 的原子
  替换路径：rename 前失败保留旧文件，rename 后的持久性错误要准确报告
  提交状态。
- `Close` 幂等，停止新工作后释放 region、文件锁等资源并汇总关闭错误。
- 任何格式或写入改动都要覆盖容量上限、故障注入、损坏输入、重启恢复与
  零数据丢失；不得通过放宽上限或忽略 I/O 错误让测试通过。
- 版本化 bin fixture 是各自域的唯一 golden 来源：只随域包原地演化（git mv
  迁移须逐字节不变），其他包只读引用，不复制第二份（跨包只读示例见
  `player/AGENTS.md` 的 fixture 节）。

## Documentation Sync Policy

- 修改任一子包的行为、导出面或测试入口，必须同步该子包的 `AGENTS.md`；
  总纲只维护目录地图、依赖方向、别名政策与共享纪律，不复制子包细节，
  子包细节不回写总纲。
- 容量上限、阈值、场景清单一律以代码常量与性质测试为准，指南只点名常量
  与测试，不抄数值。
- 存储子包布局或依赖边变化时，同步 `packages/audit/dependency_test.go`
  的 `allowed` 表与 openspec 主规格 `repository-code-organization`，三者
  不一致即漂移。

## Focused Verification

按改动域定点（分层纪律见 `docs/notes/test-quickstart.md`）：

| 改动域 | 命令 |
|---|---|
| 根包编排（disk/memory/world_files/backup/metadata/chunk_keys） | `go test ./packages/server/storage -race -count=1` |
| region 格式原语 | `go test ./packages/server/storage/region -race -count=1` |
| chunk codec 与记录层容器 | `go test ./packages/server/storage/chunk -race -count=1` |
| player codec | `go test ./packages/server/storage/player -race -count=1` |
| companion codec | `go test ./packages/server/storage/companion -race -count=1` |
| hostile codec | `go test ./packages/server/storage/hostile -race -count=1` |
| passive codec | `go test ./packages/server/storage/passive -race -count=1` |
| 全子树（跨域改动） | `go test ./packages/server/storage/... -race -count=1` |
| 依赖方向 / 文档守卫 | `go test ./packages/audit -count=1` |

## Offline Runtime Qualification

`runtime_migration_verify_test.go` owns the test-only callable
`TestRuntimeMigrationVerifyWorld` and its real save-codec characterization.
It acquires an existing exclusive read-only world lease, decodes canonical durable
files, proves the complete tree and modes unchanged, and emits a report outside
the world. It must never repair saves, create missing world files, start a server,
or become a production Go API. This migration oracle ends at Rust runtime
qualification. Run `go test ./packages/server/storage -run RuntimeMigration -race -count=1`.
