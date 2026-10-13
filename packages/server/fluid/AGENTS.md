# fluid 包：权威流体推进与 native kernel 包装

`packages/server/fluid` 提供与权威模拟解耦的纯流体推进：有界待更新队列（`Queue`）、
单格规则求值与区块重扫扫描的 native 包装。行为规格见
`openspec/specs/authoritative-fluid/spec.md` 与
`openspec/specs/fluid-survival/spec.md`，Go/Rust 分工纪律见
`docs/notes/go-rust-division.md`。依赖方向：只允许 `packages/server/updates`、
`packages/shared/core` 与 `packages/shared/nativeabi`（`packages/audit/audit/dependencies.py`
的 `allowed` 表强制）；tunable 一律由调用方传参，本包不定义、
不读取任何隐藏默认值。

## 生产路径只经 native kernel (`fluid/eval_native.go`, `fluid/rescan_native.go`)

- 单格求值唯一调用点是 `finishEvalBatch` → `nativeabi.FluidEvalBatch`：
  `Queue.Advance` 阶段一先按全序弹出 ≤budget 项、再一次批量求值、解码后经
  `strongerWrite` 并入候选写入集。重扫唯一调用点是 `ScanRescanRegion` →
  `nativeabi.FluidRescan`：sim/realm 组装 MFL1 盒后经本包调用，
  `nativeabi` 边不进 sim。
- 两个 kernel 的非 OK 状态码以稳定中文文案 panic，不存在生产 Go fallback；
  扫描区域列域与起始区段在编码前显式校验（防窄域静默截断成另一个合法区域），
  由 realm 侧 `TestScanRescanRegionRejectsInvalidRegion` 钉死。
- 布局契约（eval 布局 v1、MFL1 重扫布局 v1）的常量与 engine 侧
  `fluid_eval.rs`/`fluid_rescan.rs` 逐字一致，两侧注释互指；改布局必须同步升
  输入布局版本号并两侧同改，golden 与差分门禁兜底。

## oracle 与冻结判定面 (`fluid/oracle_test.go`, `fluid/rules.go`)

- `evalCell`/`flowingSurvives` 已逐字冻结在 `oracle_test.go`，仅作差分对照，
  生产路径零调用；删除 oracle 留后续独立 change（沿 `drop-go-test-oracles`
  先例）。在此之前不得改写——差分门禁的价值正是迁移前实现原样钉住。
- `Replaceable` 判定表保留在 `rules.go` 作冻结判定面：生产路径零调用（现存
  命中仅为 `packages/server/sim/runtime/fluid.go` 测试镜像与测试文件），供 oracle、
  性质测试与 fuzz 不变量引用。
- 差分强制点：`TestFluidEvalBatchMatchesOracle`（eval 逐位差分）与 realm 侧
  `TestRescanDifferentialOceanUniformChunks`/`TestRescanDifferentialMixedSurface`/
  `TestRescanDifferentialBudgetResumeAcrossTicks`（重扫逐位差分，oracle 住
  `packages/server/sim/realm/environment_oracle_test.go`）。

## 状态与编排留 Go (`fluid/queue.go`)

- `Queue` 的待办存储由统一调度器 `updates.Queue`（kind=`KindFluidFlow`，经
  `Scheduler()` 暴露同实例供后续域挂载）承载：dueTick 全序、(pos, kind) 去重、
  只提前不推迟与预算取批在 updates；`strongerWrite` 同 tick 冲突合并、
  `lessPos` 排序提交与变更再入队仍是本包的两阶段 Advance 编排；kernel 只做
  无状态纯函数求值，队列 scratch 跨 tick 复用、按需增长。
- 批量求值链（编码 → 调用 → 解码）预热后零分配，以 `TestEvalNoAlloc` 锁定；
  队列调度不变量由性质测试钉死（`TestOrderIndependence_PerTickChangesMatch`、
  `TestBudgetEquivalence_DamBreakSameFinalState`、
  `TestConvergeRandomWaterBodiesReachFixedPoint`）。

## helper 中心与回归测试 (`fluid/helpers_test.go`)

- 共享测试基建（`memWorld` 替身、快照对比、地形夹具、队列断言助手）唯一住
  `helpers_test.go`；差分/golden/fuzz/0-alloc 各一文件，oracle bench 在
  `eval_bench_oracle_test.go`（build tag `fluid_oracle_bench`，默认不编译）。
- 钉死回归的测试入口：
  - `TestFluidEvalBatchMatchesOracle`（kernel 与 Go oracle 逐位一致）；
  - `TestFluidEvalBatchGoldenVectors`（输出字节 golden 向量）；
  - `FuzzFluidEval`（任意输入的结构不变量：源不死、只写可替换目标、等级界）；
  - `TestEvalNoAlloc`（批量求值链零分配）；
  - `TestEndToEnd_SourceSpreadsExactlySevenOnFlatGround`（公共 API 端到端）。

## Focused Verification

- 定点测试：`go test ./packages/server/fluid -race -count=1`。
- 重扫差分门禁：`go test ./packages/server/sim/realm -run TestRescanDifferential -race -count=1`。
- 触碰 Rust kernel 或布局契约：先 `make rust`（共享 CARGO_TARGET_DIR 回拷），
  提交前 `make rust-check`。
