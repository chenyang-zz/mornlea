# Mornlea

<p align="center">
  <img src="https://img.shields.io/badge/Go-1.26-00ADD8" alt="Go 1.26">
  <img src="https://img.shields.io/badge/Rust-1.97.1-f74c00" alt="Rust 1.97.1">
  <img src="https://img.shields.io/badge/client-macOS-9cf" alt="macOS 客户端">
  <img src="https://img.shields.io/badge/protocol-v45-blue" alt="protocol v45">
  <img src="https://img.shields.io/badge/license-MIT-green" alt="MIT">
  <a href="https://github.com/chenyang-zz/mornlea/actions/workflows/ci.yml"><img src="https://github.com/chenyang-zz/mornlea/actions/workflows/ci.yml/badge.svg" alt="CI"></a>
</p>

[English](README.md) · 简体中文

Mornlea 是一个独立的体素生存游戏。客户端、权威服务端、世界格式和二进制协议都是自研的。它不兼容 Minecraft 协议，也不读取 Minecraft 存档或 Mojang 美术资源。

单机和局域网走同一条登录与模拟路径。世界和玩家的结果由服务端决定。客户端只保留镜像、预测和呈现。

当前契约：protocol v45、player schema v9、chunk schema v9、world metadata v6、`companions.ai` schema v5、`hostile_mobs` schema v2、`passive_mobs` schema v1、engine ABI v11、client ABI v19、benchmark scenario v23。

## 功能

**世界。** 程序化地形、橡树与树苗、短草、矿石、流动的水和积雪。服务端推进固定昼夜、季节、温度和天气（晴、雨、雷暴）。天空光和方块光由客户端根据权威方块镜像推导。世界、玩家、敌对生物和被动生物都会存档。

**生存。** 挖掘与放置、带耐久的工具、个人 2×2 合成和 3×3 工作台、熔炉、箱子。可种植小麦、马铃薯和胡萝卜。饥饿、食物、氧气、生命、摔落和铁质护甲都由服务端结算。战斗包括剑、弓，以及两种敌对生物（夜行者与掷骨者）和被动的牛。按住 Shift 潜行，双击 `W` 疾跑，可以在床上睡觉，也可以用水桶装水。新玩家的初始背包是空的。世界难度为 `peaceful`、`normal` 或 `hard`。

**多人。** 普通单机会在进程内启动服务端。`mornlea-server` 是面向可信局域网的无图形 TCP 服务端：没有认证或加密，最多 8 名玩家，也没有游戏内的服务器列表。用 `--connect` 连接。

**伙伴。** Go 服务端最多运行四名具名伙伴，并执行他们的 `go_to`、`follow`、`mine` 和 `place` 任务。可选的 Python 服务负责规划与台词，不能自行改写世界。

**客户端。** macOS 上的默认客户端是 `mornlea`：Rust wgpu 渲染器、进程内 WebView 菜单（主菜单、设置、加载、暂停）和 WebView HUD。`F5` 在第一人称、第三人称背面和第三人称正面之间循环。`apps/mornlea-godot` 是可选的桌面试点，不是 `make run` 启动的程序。

[docs/notes/limitations.md](docs/notes/limitations.md) 里有些较早的边界描述写于天气、护甲、潜行、弓和牛之前。该说明与代码不一致时，以代码和 [openspec/specs](openspec/specs/) 为准。

## 截图

无头视觉基线，640×360。这些是入库的世界场景，不是实机录像。

| 正午地形 | 橡树林 |
| --- | --- |
| ![正午地形](testdata/visual-golden/world/terrain-noon.png) | ![橡树林](testdata/visual-golden/world/oak-grove.png) |
| 雨 | 积雪 |
| ![正午雨景](testdata/visual-golden/world/rain-noon.png) | ![积雪](testdata/visual-golden/world/snow-cover.png) |

## 环境要求

- 图形客户端需要 macOS。客户端入口使用 Darwin 构建约束，主要在 Apple Silicon 上验证。
- Go 1.26。
- 通过 rustup 安装的 Rust 1.97.1。版本钉在 `packages/engine/rust-toolchain.toml`。
- C 工具链与 CGO。macOS 上是 Xcode Command Line Tools：

```bash
xcode-select --install
```

- Make。
- 只有启用伙伴 Agent 时才需要 Python 3.12 和 `uv`。
- 只有改可选试点时才需要 Godot 4.7.2。见 [apps/mornlea-godot/README.zh.md](apps/mornlea-godot/README.zh.md)。

Linux 可以用 `make build-linux-server` 构建无图形服务端。该目标不产出图形客户端。

## 快速开始

```bash
git clone https://github.com/chenyang-zz/mornlea.git
cd mornlea
make run
```

首次启动会生成视距内的地形，比之后的启动更慢。默认存档是 `worlds/default`。

```bash
make run ARGS="--world worlds/demo"
```

`make run` 会先构建固定版本的 Rust 库，再启动客户端。不要把二进制和另一次构建的 `mornlea_engine` 库混用。

## 使用

本地游玩会打开主菜单，然后进入进程内世界。远程游玩不创建那个本地世界：

```bash
make rust
go run ./packages/server/cmd/mornlea-server --listen :25565 --world worlds/lan --seed 42 --max-players 8
go run ./packages/client/cmd/mornlea --connect 127.0.0.1:25565 --name PlayerA
```

`--seed` 只在创建世界目录时生效。`--max-players` 只接受 `1..8`。不要把这个 TCP 端口暴露到公网。`--difficulty` 等更多服务端参数见 [docs/notes/lan-server.md](docs/notes/lan-server.md)。

`make build` 会链接 `bin/mornlea` 和 `bin/mornlea-server`，并复制 `bin/libmornlea_engine.dylib`。随后它会从 `packages/client/assets/packs/pixel_perfection` 复制署名文件，而这个目录已经不在树里。复制失败发生在二进制写完之后。内嵌默认材质是 `packages/client/assets/packs/pastelcraft` 里的 Pastelcraft 子集。

`make build-linux-server` 产出 Linux amd64 无图形服务端，以及相邻的 `bin/libmornlea_engine.so`。这两个文件必须一起发布。

### 操作

| 输入 | 动作 |
| --- | --- |
| `W` `A` `S` `D` | 移动 |
| 空格 | 跳跃；按住则在水中上浮 |
| 双击 `W` | 向前移动时疾跑 |
| 左 Shift | 潜行 |
| 鼠标 | 转动视角 |
| 按住左键 | 挖掘、近战或拉弓 |
| 按住右键 | 使用：放置、打开、翻地、进食、门、床、工作台、装备护甲 |
| `1`–`9` | 快捷栏 |
| `E` | 打开背包，或关闭当前容器 |
| `Q` | 从选中的快捷栏丢出一件物品 |
| Enter | 聊天，也用来向伙伴发送 `@名字 指令` |
| `F5` | 切换视角 |
| Esc | 关闭当前面板，或暂停 |
| `F3` | 调试面板，仅在带 `--dev` 时可用 |

背包里的点击可以整堆移动、拆分或快捷搬运。移动由服务端结算。数值、配方和伙伴指令见 [docs/notes/gameplay.md](docs/notes/gameplay.md)。

## 项目结构

仓库根目录不是 Go module。`go.work` 列出六个模块。Go 导入路径前缀仍是 `github.com/channing771/mornlea/packages/<unit>`，GitHub 仓库则是 `chenyang-zz/mornlea`。

```text
packages/
  contracts/   Go 与 Python 共用的 JSON 契约
  shared/      领域类型、物理、网络、世界、engine ABI 桥
  server/      权威模拟、流体、存储、mornlea-server
  client/      镜像、预测、渲染 CPU 侧、mornlea
  tools/       perfcheck、agent board 及其他开发工具
  audit/       架构门禁
  engine/      Rust workspace：mornlea_engine、mornlea_client，以及迁移中的 crate
  agent/       可选的 Python 伙伴 Agent
apps/
  mornlea-godot/   可选的 Godot 桌面试点
```

当前由 Go 持有正在运行的服务端、默认客户端使用的协议会话，以及存储。`mornlea_engine` 是数值内核（网格、光照、碰撞、射线、物理、世界生成、流体）。`mornlea_client` 持有 Darwin 窗口、输入、WebView 壳和 GPU 渲染。Go 不调用 WebGPU。Rust 的 domain、protocol、storage crate 以及 Godot 应用属于迁移工作。目标终态见 [docs/architecture-target.zh.md](docs/architecture-target.zh.md)；正在运行的系统见 [docs/architecture.zh.md](docs/architecture.zh.md)。

## 文档

| 主题 | 文档 |
| --- | --- |
| 玩法 | [docs/notes/gameplay.md](docs/notes/gameplay.md) |
| 配置 | [docs/notes/configuration.md](docs/notes/configuration.md) |
| 局域网服务端 | [docs/notes/lan-server.md](docs/notes/lan-server.md) |
| 材质包 | [docs/texture-packs.md](docs/texture-packs.md) |
| 限制 | [docs/notes/limitations.md](docs/notes/limitations.md) |
| 存档与协议升级 | [docs/notes/compatibility.md](docs/notes/compatibility.md) |
| 视觉基线 | [docs/notes/visual-verification.md](docs/notes/visual-verification.md) |
| 当前架构 | [docs/architecture.zh.md](docs/architecture.zh.md) |
| 目标架构 | [docs/architecture-target.zh.md](docs/architecture-target.zh.md) |
| 已交付内容 | [docs/notes/progress.md](docs/notes/progress.md) |
| 文档索引 | [docs/README.zh.md](docs/README.zh.md) |

多份玩家说明是中文。

## 常用命令

| 命令 | 作用 |
| --- | --- |
| `make help` | 列出 Makefile 目标 |
| `make run` | 构建 Rust 库并启动 macOS 客户端 |
| `make build` | 链接两个二进制并复制 engine dylib；末尾的署名复制仍然会失败 |
| `make build-linux-server` | Linux amd64 无图形服务端以及 `libmornlea_engine.so` |
| `make test` | 六个模块的 Go 测试 |
| `make test-race` | 带 race detector 的同一组测试 |
| `make dev-check` | 短 Go 检查，加上 Rust fmt、clippy 和测试 |
| `make rust` | 构建固定版本的 Rust cdylib |
| `make rust-check` | Rust fmt、clippy 和 workspace 测试 |
| `make visual-check` | 把无头帧与视觉基线比较 |
| `make companion-agent-check` | Python 格式、lint、类型和单元测试 |
| `make companion-agent-integration` | 无外网的 Go/Python 进程合同 |

## 参与贡献

改代码前先读 [AGENTS.md](AGENTS.md)。小修复可以直接提 pull request。协议、存档、性能契约和跨包改动要先走 OpenSpec：[docs/openspec.zh.md](docs/openspec.zh.md) 和 [docs/development-process.zh.md](docs/development-process.zh.md)。

不要加入 Mojang 材质或其他未授权美术资源。

## 许可证

项目本身是 [MIT](LICENSE)。

内嵌的默认方块材质是 [Pastelcraft](https://modrinth.com/resourcepack/pastelcraft)（作者 XradicalD，Square Dreams）的重命名子集，同样是 MIT。署名见 [packages/client/assets/packs/pastelcraft/ATTRIBUTION.md](packages/client/assets/packs/pastelcraft/ATTRIBUTION.md)。没有映射的层回退到程序化材质。
