# devcapture 包：本地开发捕获服务

`packages/client/cmd/mornlea/devcapture` 给运行中的交互式客户端暴露仅绑定回环地址的 HTTP
捕获服务（`/status`、`/screenshot`、`/record`）：外部调用方经 curl 拉取窗口
合成截图与有界短录屏帧序列。行为契约见 openspec change `dev-capture` 的
delta spec（`openspec/changes/dev-capture/specs/dev-capture/spec.md`），用法、
端点语义与排查指引见 `docs/notes/dev-capture.md`。本包只依赖 app，不依赖
capture/benchmark；app 也不反向导入本包（方向由
`TestClientCommandSubpackageDependencyDirections` 强制，边登记在
`clientCommandAllowedEdges`）。生产接线（注入协调器、拉起监听、注册优雅关闭）
由 main 完成，本包不感知窗口与渲染细节。

## 协调器契约与单 outstanding (`devcapture/service.go`)

- `Service` 实现 app 侧声明的 `CaptureCoordinator`（编译期断言
  `var _ app.CaptureCoordinator = (*Service)(nil)`）。方向是 capture/benchmark
  消费端接口模式的反向：接口住 app、本包实现、main 装配注入。
- 请求通道容量 1 即单 outstanding：`PendingCapture`/`CompleteCapture` 都必须
  非阻塞（`TestPendingCaptureWithoutRequestIsImmediate`、
  `TestCompleteCaptureWithFullDoneDoesNotBlock`）；交付缓冲已满说明消费者已
  超时离开，按丢弃计数处理、绝不反压帧循环
  （`TestCompleteCaptureWithNilDoneCountsDrop`）。
- HTTP 侧经 `requestFrame` 发一帧并等待交付：通道满以 `ErrCaptureBusy` 立即
  失败、超上限 `frameWait` 有界收敛、客户端断开即取消
  （`TestRequestFrameBusyOnOutstanding`）。错误统一映射为 503 与稳定中文
  文案；文案是契约的一部分，改动需同步 delta spec 与 `docs/notes/dev-capture.md`。

## HTTP 端点与录制编排 (`devcapture/http.go`, `devcapture/record.go`)

- 三端点以方法模式挂载，非 GET 由 mux 统一 405
  （`TestScreenshotRejectsNonGet`）；错误 JSON 一律 `{"error": <稳定中文>}`，
  捕获不可用文案由 `captureErrorText` 按 `client.ErrCaptureUnavailable` 分流
  钉位（`TestScreenshotUnavailableReturnsStable503`）。
- 录制参数边界（seconds/fps/总帧上限、`format∈{png,gif}`、默认值）以
  `parseRecordParams` 及其测试为准（`TestRecordParamParsingDefaultsAndBounds`、
  `TestRecordRejectsOutOfRangeParams`），不在指南复制会漂移的数值；校验必须
  发生在任何帧捕获之前。
- 采样恪守单 outstanding：循环「等本帧目标时刻 → 发一帧 → 等交付」，帧间隔
  等待全部在本包 goroutine；单帧失败终止采样但仍交付 zip + `manifest.error`
  （局部证据好于全盘丢弃），总截止超限才 503 放弃整次，并发录制互斥
  （`TestRecordSuccessProducesParsableZip`、`TestRecordTerminatesOnCaptureError`、
  `TestRecordAbortsBeyondDeadline`、`TestRecordRejectsConcurrentRecording`）。
- BGRA8→NRGBA 转换住 `bgraToNRGBA`（`devcapture/bgra.go`），有意不 import
  `packages/client/cmd/mornlea/capture` 复用其未导出助手：两侧契约来源不同（离屏 readback vs
  窗口合成捕获），小体量重复是 dev-capture design 的既定裁决；通道序与 alpha
  透传语义由 `TestBGRAToNRGBASwapsChannelsPerRow`、
  `TestBGRAToNRGBAPassesAlphaThrough` 钉住。

## 回环强制与端口文件生命周期 (`devcapture/service.go`, `devcapture/portfile.go`)

- 回环约束是 spec 的 MUST，且是绑定前的防御闸：任何注入地址都过 `listen`
  的主机检查（仅 `127.0.0.1`/`::1`/`localhost`），非回环拒绝
  （`TestStartRejectsNonLoopbackHost`）。
- 端口顺延只对「地址被占」生效、至多 `maxBindAttempts` 次；实际端口写
  `~/.mornlea/dev-capture.json`（pid/port/started_at），`Stop` 幂等清理；
  发现文件写失败视为启动失败（`TestStartCascadesPastBusyPort`、
  `TestStartWritesPortFileAndStopRemovesIt`、`TestRemovePortFileToleratesMissing`）。
- 重复 `Start` 必须在任何副作用（顺延绑定、写发现文件）之前失败，否则第二个
  监听会以顺延后的端口覆写发现文件且无失败信号（`TestStartTwiceFails`）。

## helper 中心与测试装配 (`devcapture/helpers_test.go`)

- `newTestService` 构造不监听、不写文件的裸 `Service`；handler 单测经
  `httptest` 加 fake 协调器驱动三端点与失败路径，不启动真实窗口。时间源
  （`Options.Now`/`Options.Sleep`）与 `Options.PortFilePath` 注入保证录制
  节奏与端口文件路径的测试确定性。

## Focused Verification

- 定点测试：`go test ./packages/client/cmd/mornlea/devcapture -race -count=1`（`httptest`
  全覆盖，秒级）。
- 依赖方向 / 文档守卫：`make archcheck`。
