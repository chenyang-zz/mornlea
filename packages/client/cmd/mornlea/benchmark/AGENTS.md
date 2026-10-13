# benchmark 包：固定工作负载性能场景

`packages/client/cmd/mornlea/benchmark` 实现 benchmark 模式：固定工作负载的无头观察者路径、
性能测量与报告写出、多人 benchmark 的权威服务端观察者与探针。行为规格见
`openspec/specs/bounded-benchmark-workload/spec.md`。本包只依赖 app，不依赖
capture（方向由 `TestClientCommandSubpackageDependencyDirections` 强制）。

## 场景契约 (`benchmark/benchmark.go`)

- 场景身份是 `scenarioVersion`（当前 v22）：场景语义、GPU 完成时间定义与报告
  结构都绑定版本，迁移链以它为终点。根 `AGENTS.md` 的「benchmark scenario
  vN」基线断言由 `packages/audit` 的基线版本测试直接
  读取本包常量核对——升版时必须同步根文档。v22 记录的是自然短草对被测进程
  与被测世界的改变（稳定方块与 mesh registry 追加 `ShortGrassID`、worldgen
  `MGW1` layout 3、固定世界出现确定性短草），叠加在 v21（常显 HUD 层迁出
  GPU 保留面）基线之上。v22 之后追加的树苗材质层把当前被测进程的植物材质
  判定集合扩为 `[31..54] ∪ {68, 164}`、engine ABI 升为 v11、mesh registry
  烘焙条目数由 89 增至 90；树苗不出现在固定 benchmark 世界，被测世界、网格
  输出与阈值逐项不变，因此场景版本仍为 v22——只追加编号而不改变固定世界
  方块的登记不单独升版（雪层编号是同一先例）。无头观察路径仍零 WebView
  参与，不构造 WebView、不下行桥状态。
- 固定工作负载不随环境漂移：世界种子取 app 导出的 `BenchmarkSeed`，注水强制
  关闭（main 装配层与 `multiplayer_benchmark_server.go` 双侧钉死），Dev 面板
  不构造，输入、世界与报告身份不随用户配置漂移（配置强制回落
  `config.Defaults()`，忽略用户材质覆盖，不创建交互窗口，不请求音频设备）。

## 数值与报告纪律 (`benchmark/benchmark_measure.go`, `benchmark/benchmark_report.go`)

- 性能阈值与数值只记录，不改变退出状态；真实 overflow、数据丢失、报告身份或
  结构不完整以及 I/O 错误仍必须失败。
- 阈值不得放宽：`TestPerformanceThresholdsRejectTickP99AtTenMilliseconds` 与
  `TestWriteBenchmarkReportRecordsPerformanceOutsideThresholds` 钉住阈值语义。
- 报告完整性与写出原子性：
  `TestValidateBenchmarkReportStillRejectsIncompleteSamples`、
  `TestValidateBenchmarkReportRejectsDroppedSamples`、
  `TestWriteBenchmarkReportDoesNotOverwriteAcceptedOutputOnFailure`、
  `TestWriteBenchmarkReportAtomicPromotionIsComplete`。

## 消费端接口 (`benchmark/benchmark_application.go`)

- `BenchmarkApplication` 是 benchmark 对宿主应用状态的唯一访问面（预热、阶段
  测量、GPU 完成时间采样、多人探针的渲染计时注入与传输关闭）；方法集以本包
  生产代码实际引用为准，不为对称性添加无人消费的方法，也不扩散 app 内部字段。
- 装配入口 `RunBenchmark` 接收具体 `*application.Application`：固定场景的加载
  等待判据（app 导出的 `WaitUntilLoaded`）以具体应用的加载管线为契约，测试侧
  经 `application.NewOffscreenRenderApplicationForTest` 直接装配具体应用。

## 多人观察者与探针 (`multiplayer_benchmark.go` 等, `multiplayer_probe_epoch.go`)

- 八会话服务端探针的真实性与有界性由
  `TestScenarioV7EightSessionServerProbeIsRealAndBounded` 钉住——CI 步骤
  「50ms 服务端探针门禁」逐字运行该入口。
- 测量窗口与 epoch 语义由 `TestBenchmarkServerEpoch*`（探针 epoch）与
  `TestBenchmarkServerMeasuredWindow*`（测量窗口边界）两族钉住；CI 步骤
  「M3C v6 八玩家与性能报告门禁」对本包按 `-run` 选中
  `PerformanceThresholds`/`BenchmarkServerEpoch`/`BenchmarkServerMeasuredWindow`
  分支（其余分支落在 `packages/client/client`、`packages/server/server` 与
  `packages/tools/perfcheck`）。
- `raceEnabled` 常量对：`benchmark_server_race_helpers_test.go` 在 `-race`
  构建下定义为 true，`benchmark_server_norace_helpers_test.go` 在非 `-race`
  构建下定义为 false。同一测试文件据此在 race 构建下追加时序敏感断言、非
  race 下收敛路径；两个文件必须成对维护，只改一侧会让一个构建模式编译失败。

## Focused Verification

- 定点测试：`go test ./packages/client/cmd/mornlea/benchmark -race -count=1`（含真实 renderer
  场景，分钟级；快速迭代先按 `testing.Short()` 纪律加 `-short`）。
- 多人门禁：`make test-multiplayer`（benchmark 用例经本包选中）。
