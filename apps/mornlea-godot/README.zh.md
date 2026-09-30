---
doc_id: godot-client-project
language: zh
counterpart: README.md
revision: 2026-09-30.1
---

# Mornlea Godot 客户端

此目录是桌面客户端试点以及未来经批准全量迁移所共用的稳定 Godot 项目根目录。请在 Godot Project Manager 中直接打开此目录，不要打开仓库根目录。后续迁移阶段继续沿用该位置，避免再次搬迁场景 UID、`res://` 路径、工具链与发布身份。

## 打开项目

项目要求 Godot 4.7.2 Standard。可先核验已钉定的官方制品元数据而不下载文件：

```bash
scripts/godot/fetch.sh --verify-only
```

以下命令在不显示前台窗口的情况下打开并导入项目：

```bash
# Linux x86_64
scripts/godot/godot.sh --headless --path apps/mornlea-godot --editor --quit-after 120

# macOS
scripts/godot/godot.sh --headless --path apps/mornlea-godot --editor --quit
```

Godot 4.7.2 的 Linux EditorHelp 文档回调可能在首次导入后的立即退出时仍未完成。限定 120 帧的导入让该回调在编辑器退出前完成。

可以通过 `MORNLEA_GODOT_BIN` 指定 Godot 4.7.2 可执行文件的绝对路径。如果官方 `/Applications/Godot.app` 安装版本与项目钉定版本一致，包装脚本也会直接使用它。

永久主场景仅使用纯 GDScript 和 Godot 内置节点。打开编辑器时，Python 与原生制品都不是前置条件。Bootstrap 会报告缺失的 Python 扩展、内嵌解释器、标准库与项目桥文件，不兼容的 Godot 或扩展描述符、检测到的桌面目标，以及精确的准备命令。此诊断路径既不会导入 Python 功能，也不会连接服务器。

当前 macOS Apple Silicon 运行时与桥接使用以下命令准备：

```bash
scripts/godot/build-python-runtime.sh --verify --offline
scripts/godot/build-extension.sh --target aarch64-apple-darwin --profile debug --verify
```

以下命令不会修改项目工作区，可分别验证两条干净状态诊断路径：

```bash
scripts/godot/openable-smoke.sh --without-native
scripts/godot/openable-smoke.sh --without-python
```

## Linux x86_64 发行单元

Linux 构建使用与 macOS 项目相同的生产功能目录。已资格验证的目标为 `x86_64-unknown-linux-gnu`，Godot 导出预设为 `Mornlea Linux x86_64`。此发行单元打包已有桌面试点功能，不代表完整游戏功能已经对齐。

准备 Go 1.26、Rust 1.97.1、`scripts/godot/py4godot/linux-build-inputs.env` 固定的 GNU C++ 工具链，以及 `patchelf`、`binutils`、`ripgrep`、`unzip` 和 `shasum`。然后准备宿主 Rust 库、官方 Godot 编辑器及导出模板和内嵌 Python 运行时，再无窗口导出并验证生产主场景：

```bash
make rust
scripts/godot/fetch.sh --target linux-x86_64
scripts/godot/build-python-runtime.sh --verify --target x86_64-unknown-linux-gnu
make godot-build
make godot-export-linux
```

默认输出为已忽略的 `build/godot-linux/` 目录。分发时必须保留整个目录：`mornlea.x86_64`、`mornlea.pck`、同目录布局中的 Rust/Go 共享库和 `addons/py4godot/cpython-3.14.4-linux64/` 是一个完整单元。CPython 从文件系统中的运行时目录加载标准库，因此仅复制可执行文件和 PCK 不够。生产 Python 脚本也保留在映射的文件系统路径中，以满足 Py4Godot 的类加载契约。当前 ELF 文件要求 glibc 2.34 或更新版本，以及提供 `GLIBCXX_3.4.32` 的系统 `libstdc++`（GCC 13.2 或更新版本）。直接启动导出的可执行文件即可，无需系统 Python、`PYTHONPATH`、`LD_LIBRARY_PATH`、运行时安装器或网络下载。

Python 加载器使用 `scripts/godot/py4godot/linux-build-inputs.env` 标识的编译器构建；加载器、补丁系列、源码归档和完整运行时校验和分别固定。不同编译器或被修改的运行时会在导出前被拒绝。编辑器和模板归档依据 `scripts/godot/version.env` 进行 SHA-256 校验；Linux 编辑器摘要是在核对官方发行 SHA-512 清单后得到的。

`make godot-build` 离线使用经过校验的外部缓存构建 debug 和 release 原生文件。`make godot-export-linux` 要求这些已准备的文件，校验源码与资源闭包和生成资产，从已验证的离线缓存恢复干净的内嵌运行时并校验其摘要，再从 release 发行单元激活并关闭生产功能目录。验证要求 Bootstrap 就绪，六个活跃功能（会话、角色、桌面输入、玩家视角、UI 和世界），两个明确禁用的音频与生命周期骨架，以及 Python 干净退出。导出会重新物化被忽略的 Python 插件，因此应在没有其他 Godot 进程使用项目运行时时执行。所有自动化检查均无窗口运行。已有原生文件准备完毕时，可导出到另一个绝对路径目录：

```bash
scripts/godot/export-linux.sh --verify --output /absolute/path/mornlea-linux
```

导出器先在暂存目录中完成导出和验证，再发布替换文件。它拒绝没有自身所有权标记的非空输出目录；暂存或验证失败时保留先前已有发行单元。`distribution.env` 记录工具和运行时版本、源码提交以及是否存在本地修改。

## 资源同步

注册 atlas 的权威来源继续是 `packages/client/assets`，注册的 Noto Sans CJK 字体继续以 `packages/client/render/assets` 为权威来源。使用以下命令物化 Godot 项目内的衍生资源：

```bash
scripts/godot/sync-assets.sh
scripts/godot/sync-assets.sh --check
```

生成目录包含按材质层优先、mip 层次次优先排列的 RGBA8 atlas、注册字体及其 OFL/来源文件、保留的材质许可证记录，以及确定性清单。清单记录源 Git 树、每个输入的校验和、聚合输入校验和、atlas 布局与每个输出的校验和。不得编辑 `assets/generated/` 下的文件，也不得手工添加文件；检查器会拒绝缺失、改写、符号链接及手写内容。

## Python 开发检查

生产 Python 依赖保持为空。内嵌 CPython 运行时只包含已经通过资格验证的 Py4Godot 单元，不安装开发工具。Ruff 与 mypy 仅作为开发依赖，由 `uv.lock` 精确解析；`uv` 可以根据锁文件填充本地且被忽略的 `.venv/`，但不会修改内嵌运行时或向其中安装软件包。`typing/` 下的本地 Py4Godot 存根提供受检查接口，不导入生成的插件代码。

使用以下命令运行格式、静态检查、严格类型检查、边界 mutation tests 与源码策略检查：

```bash
scripts/godot/python-check.sh --locked
```

边界检查会拒绝导入 companion Agent、直接访问原生 ABI 或动态库、从 Python 发起网络操作、运行时安装器、进程执行、不受控动态导入，以及非英文源码注释。

## Python 功能宿主契约

Bootstrap 完成依赖交接后，由 `app/host/app_root.py` 与 `app/host/feature_host.py` 负责目录规划和功能生命周期。`config/feature_catalog.tres` 是显式白名单，它直接列出粗粒度 `feature.tres` 清单，而不会扫描目录。每份清单声明 Host 协议 `1.0`、一个项目内入口场景、稳定依赖、带版本的桥接功能族要求、必需或可选失败语义、预算等级与重置策略。

生命周期顺序是确定的：验证目录、按排序后的依赖顺序实例化、验证 Python 功能、注入唯一的类型化 Godot 桥接服务、按 epoch 激活、重置，并按反序停用。不兼容或启动失败的必需功能会终止装配并释放此前已激活的功能；可选功能会以可观察结果被禁用，其依赖者不能越过该状态静默启动。功能脚本不会导入同级项目模块，也不会动态发现实现；Godot 资源路径承担有界组合，同时隔离解释器继续确保项目目录不进入 `sys.path`。

使用以下命令运行内嵌 Python 契约、增量扩展与原生桥集成检查：

```bash
scripts/godot/feature-contract-check.sh
scripts/godot/feature-contract-check.sh --extensibility-probe
scripts/godot/feature-contract-check.sh --bridge-integration
```

## 转录地形检查

世界功能的原生地形管线（紧凑 quad 解码、网格预备 worker、RenderingServer RID 表与每帧预算）通过一个无头检查端到端验证：检查把确定性的 protocol v44 场景回放进真实的试点会话。转录助手（`packages/client/cmd/mornlea-godot-transcripts`）在回环地址上提供 `testdata/godot-pilot/transcripts/terrain/` 下的场景：第一轮发布初始区块快照、方块增量、区块遗忘与断开；第二轮模拟重新进入世界。检查场景激活生产目录、经会话功能连接，并在每个阶段后断言桥的结构化地形摘要，包括遗忘与断开/重置后没有残留区段。运行方式：

```bash
make godot-terrain-check
```

## 专用服务器地形冒烟

同一条地形链还会面向真实权威服务器（而非转录）再验证一次。冒烟门禁（`scripts/godot/terrain-smoke.sh`）把 `mornlea-server` 构建到一次性临时目录，在确定性的回环端口上启动它，并把世界、配置与日志全部落在临时路径中，确保不触碰仓库存档；等待其启动日志行后运行无头冒烟场景。该场景激活生产目录，让试点登录真实服务器，等待与转录检查初始快照相同的加载判据（确认会话阶段 Play 且结构化地形摘要中至少存在一个存活区段），随后运行固定的 300 帧预算，并在加载判据达成后对任何地形错误字大声失败，最后证明干净关闭不残留任何区段。门禁断言成功标记，要求服务器经 SIGTERM 干净退出，并通过退出 trap 回收全部子进程并核验无幸存者。运行方式：

```bash
scripts/godot/terrain-smoke.sh
```

## 架构边界

Godot 只负责桌面窗口、键盘鼠标采集、呈现、试点 UI 与 Godot 资源生命周期。Go 客户端运行时继续负责协议 v44、镜像、预测、语义帧状态与有界网络处理。网格、光照、碰撞、射线检测和物理由 engine ABI v11 中的数值实现继续负责。权威 Go 服务器仍是世界与玩家真值的唯一所有者。

未来功能通过同一根目录中的粗粒度 `features/`、`platform/desktop/`、`config/` 与唯一 `addons/mornlea_bridge/` 边界扩展。Bootstrap 必须保持与具体功能无关。移动端、Web、主机、触摸、传感器和移动生命周期支持均不属于本项目范围。

编辑器缓存和原生桥接二进制会被忽略；Godot 生成脚本与着色器 `.uid` 伴随文件后应纳入版本控制。
