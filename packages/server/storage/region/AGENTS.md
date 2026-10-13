# region 包：region 文件格式原语

`packages/server/storage/region` 承载 region 文件的格式原语：superblock/bank
编解码、扇区空间分配与坐标换算，以及跨根包与 chunk 包共享的 `RegionKey`。
本包是 storagedef 之上的格式叶子：只依赖 `packages/shared/core` 与
`packages/server/storage/storagedef`，不感知 chunk 记录层容器与根包编排（方向由
`packages/audit` 的 `TestInternalDependenciesAreOneWay` 强制）。行为
规格见 `openspec/specs/local-data-migration/spec.md`。

## 双 bank 布局与校验 (`region/region_format.go`)

- region 文件由 superblock 与两个互为备份的 bank 组成，扇区偏移全部对齐
  `SectorSize`；bank 选择在双副本间按 generation 取新、平局取 A，定义与
  取值以代码常量和 `TestSelectRegionBankValidityAndTies` 为准，不在指南
  复制数值。
- 布局是冻结契约：superblock 与 bank 的精确字节布局由
  `TestSuperblockExactLayoutAndRoundTrip`、`TestRegionBankExactLayout`
  钉死；损坏拒绝（CRC/版本/键不匹配）由 `TestSuperblockRejectsCorruption`、
  `TestRegionBankRejectsCorruption` 钉死。
- `MaxCompressedChunk` 定义在本包而非 chunk 包：bank 校验在解码时按它
  拒绝超限 entry，而本包不得反向依赖 chunk 包；chunk 信封编码共用同一
  常量，避免两包各自漂移。

## 扇区空间分配 (`region/region_space.go`)

- allocator 永不分配 active extent，按 first-fit 复用空闲区间，只在无
  空闲区间适配时文件尾追加：`TestAllocatorNeverUsesActiveExtentsAndUsesFirstFit`、
  `TestAllocatorAppendsOnlyWhenNoFreeExtentFits` 钉死。
- `ProductionSpacePolicy` 是权威保存路径的压缩判定策略，刻意暴露为可变量：
  根包编排测试在同进程内替换它来触达压缩路径；生产取值以代码为准。
- `CompactionHooks` 是压缩替换路径的故障注入点，生产路径全部为 nil；钩子
  只供容器侧（`packages/server/storage/chunk`）与测试注入，不承载策略。

## RegionKey 与坐标换算 (`region/types.go`, `region/coords.go`)

- `RegionKey` 同时被根包 DiskStore 的容器缓存与 chunk 记录层容器持有，
  因此定义在本格式原语包而非任一使用方；根包以类型别名再导出保持
  `storage.RegionKey` 引用不变。
- `RegionFor` 的 region 坐标换算用 floor division 正确处理负坐标，
  `TestRegionForUsesFloorDivision`、`TestRegionForHandlesMinInt32` 钉死。

## helper 中心与回归测试 (`region/region_format_test.go`, `region/region_space_test.go`, `region/coords_test.go`)

- 本包没有独立 `*_helpers_test.go`：夹具内联在各测试文件中；按「每包最多
  一个 helper 中心」纪律，新增共享夹具先收敛到这里再考虑建文件（规则见
  `docs/test-organization.md`）。
- 其余钉死回归的入口：`TestRegionBankRoundTripAndSelection`（bank 编解码
  与双副本选择）、`TestRegionBankAcceptsEmptyGenerationZeroAndMaxGeneration`
  （generation 边界）、`TestEncodeRegionBankRejectsInvalidStructure`
  （非法 bank 结构拒绝）。

## Focused Verification

- 定点测试：`go test ./packages/server/storage/region -race -count=1`（纯格式
  原语，秒级，不编译执行其他域的测试）。
- 依赖方向与文档守卫：`make archcheck`。
