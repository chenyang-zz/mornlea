# 图形客户端命令

本文件是 `packages/client/cmd/mornlea` 子树的总纲：目录地图、依赖方向与四种启动模式的装配
差异。每个子包的职责不变量、精确路径与钉死回归的测试名见各自目录的
`AGENTS.md`；同目录 `CLAUDE.md` 只做薄导入（内容由
`TestClaudeImportsAgentGuidance` 逐字节把关）。

## Directory Map

```
packages/client/cmd/mornlea/
├── main.go                    # 进程入口：四种启动模式路由与 runDependencies 装配
├── options.go                 # CLI flag 解析、生效配置解析与模式互斥校验
├── run_test.go                # main 装配接线与进程内存上限回归
├── options_test.go            # flag 解析与模式互斥契约
├── storage_test.go            # main 选项默认值与 --world 覆盖
├── ai_model_settings_test.go  # 本地模式 AI 模型设置与密钥注入
├── app/                       # application 装配主体（指南见 app/AGENTS.md）
├── capture/                   # 视觉 golden 抓帧与比对（基线在仓库根 `testdata/visual-golden/world/`，指南见 capture/AGENTS.md）
├── devcapture/                # 运行中交互客户端的本地回环捕获服务（指南见 devcapture/AGENTS.md）
└── benchmark/                 # 性能场景与多人 benchmark 观察者（指南见 benchmark/AGENTS.md）
```

## Dependency Direction

依赖方向单向且由 `packages/audit` 强制（契约见 openspec 主规格
`repository-code-organization`）：

- 接受：`packages/client/cmd/mornlea`（main）→ `app`、`capture`、`benchmark`、`devcapture`；
  `capture` → `app`；`benchmark` → `app`；`devcapture` → `app`。
- 拒绝：`app` 反向导入 `capture`、`benchmark` 或 `devcapture`；`capture` 与
  `benchmark` 相互导入；子树出现未登记的新包。
- 强制点：`TestClientCommandSubpackageDependencyDirections` 源码级扫描子树
  生产 import 边（不用 `go list`，断言不随 GOOS 翻转）；
  `TestClientCommandDependencyViolationsDetectDrift` 以合成边钉住检查器本身。
  新增子包必须先登记 `packages/audit` 的 `clientCommandAllowedEdges`。
- capture/benchmark 对 app 状态的访问一律经各自包内定义的消费端接口
  （`SceneApplication`、`BenchmarkApplication`），app 只导出最小方法集，
  不为 capture/benchmark 暴露内部字段。
- devcapture 方向相反：app 侧声明 `CaptureCoordinator` 消费端接口，devcapture
  实现之并消费 app 的并发安全状态访问器，main 负责装配注入；devcapture 不
  导入 capture/benchmark。
- 跨单元边：本子树是 client 模块中唯一允许 import `packages/server` 的位置
  （app/benchmark 在进程内装配本地权威 Host）；client 域库包对 server 的禁令
  由 `packages/audit/audit/boundaries.py` 强制。

## Entry Modes

四种模式与一种演示都在 `main.go` 的 `runWithDependencies` 路由，装配差异只发生在 main：

| 模式 | 触发 | 装配行为 |
|---|---|---|
| 普通本地 | 无特殊 flag | 启动停留在主菜单，进入游戏后才打开存档、启动本地 Host 并登录；AI 模型设置只注入此模式 |
| 远程联机 | `--connect` | 跳过本地 Host 与菜单延迟装配，经 TCP 连接并登录远程服务端；不携带 AI 运行时 |
| 视觉抓帧 | `--capture`（可带 `--update-golden`、`--capture-scenes`、`--capture-gifs`） | 确定性内存世界加离屏 renderer，跑完 capture 包的固定场景表，`--capture-scenes` 按场景表保序只跑子集（未知/重复名拒绝），`--capture-gifs` 让纯比对也生成 GIF 剧本（更新基线恒生成）；不能与 `--benchmark`/`--connect` 同用 |
| motion 演示 | `--motion-demo`（GIF 输出路径） | 与抓帧同源的无头装配，只跑 capture 包的 motion 演示入口（由 `--motion-scene` 选择采掘、人物行走、掉落散开或密度过程的有界 GIF 连抓）；不进场景表与比对，不能与其余无头/联机路径同用 |
| 性能场景 | `--benchmark` + `--perf-output` | 固定工作负载、`--benchmark-transport` 指定 transport 的无头观察者路径 |
| 画面捕获（叠加项） | `--dev-capture`（可带 `--dev-capture-addr`） | 不是独立模式：叠加在普通本地/远程联机交互路径上，main 拉起 devcapture 的回环捕获服务并注入 `CaptureCoordinator`；与 `--benchmark`/`--capture` 互斥，服务启动失败仅告警降级、游戏照常运行 |

模式无关的装配边界（实现住 `app/`）：

- 装配只组合配置、profile、transport/login、权威 Host、客户端镜像、Rust
  renderer、菜单、音频与关闭顺序，不复制服务端玩法规则。
- 普通本地游戏同样创建 Memory transport 并经过与远程连接相同的登录边界；
  不因同进程运行而直接读取或修改权威模拟状态。
- capture 与 benchmark 都忽略用户材质覆盖（配置强制回落 `config.Defaults()`）、
  不创建交互窗口、也不请求音频设备。

`--capture` 跑的固定场景表住 `capture/capture.go` 的 `captureScenes`（当前 27
景，完整清单、顺序约束与尾序以 `captureScenes` 与 `capture/AGENTS.md` 为准，
本总纲不复制会漂移的枚举）。常显 HUD（快捷栏贴条与选中框、状态行、氧气、
进食轨道、物品名弹条、准星、聊天呈现与命中 marker）的 GPU 呈现已迁
WebView 组件；采掘不再有屏幕进度条，其进度反馈由世界空间方块裂纹承载
（`remove-mining-hud-bar`）。只承载这部分像素的 `hud-hotbar-health`、
`hud-survival-feedback`与 `hud-item-name-popup` 三景随之退役移出清单，其像素
验收由前端 HUD 组件断言与 `frontend/visual` 部件基线承接；capture 的世界、夜景与材质场景继续走离屏 GPU；容器四景的旧场景登记已移除，
生产帧不绘制 GPU 面板，面板像素由前端 fixture 承接。

## Documentation Sync Policy

- 修改任一子包的行为、导出面或测试入口，必须同步该子包的 `AGENTS.md`；根
  文档只维护目录地图、依赖方向与模式差异，不复制子包细节。
- 子树根的 `CLAUDE.md` 是 `AGENTS.md` 的薄导入，内容逐字节固定；改动直接
  落在 `AGENTS.md`，不要把内容写进 `CLAUDE.md`。子包目录不放 `CLAUDE.md`，
  代理沿目录祖先链读到本总纲与子包指南。
- 行为规格的权威在 `openspec/specs/`：视觉验证 `visual-verification`、
  benchmark 工作负载 `bounded-benchmark-workload`。长期基线只陈述当前事实，
  不复制 change 叙事。

## Focused Verification

按子包定点（分层纪律见 `docs/notes/test-quickstart.md`；涉 Rust 侧先
`make rust`）：

| 改动域 | 命令 |
|---|---|
| main 装配 / CLI | `go test ./packages/client/cmd/mornlea -count=1` |
| Godot 隔离 | `go test ./packages/client/cmd/mornlea/... -run 'Godot|Legacy|Default' -race -count=1` |
| app 装配主体 | `go test ./packages/client/cmd/mornlea/app -race -count=1` |
| capture 视觉 | `go test ./packages/client/cmd/mornlea/capture -race -count=1`；无窗口视觉 `make visual-check` |
| benchmark 性能 | `go test ./packages/client/cmd/mornlea/benchmark -race -count=1`；多人门禁 `make test-multiplayer` |
| devcapture 捕获服务 | `go test ./packages/client/cmd/mornlea/devcapture -race -count=1` |
| 依赖方向 / 文档守卫 | `make archcheck` |
