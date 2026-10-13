# 网络协议与传输子树

本文件是 `packages/shared/network` 子树的总纲：目录地图、依赖方向、别名再导出政策
与全树共享的信任边界、传输一致性、协议演进契约。protocol 与 codec 子包的
包内不变量、精确路径与钉死回归的测试名见各自目录的 `AGENTS.md`；`tcp/`
沿用其目录指南。本子树任何目录（含子树根）不放 `CLAUDE.md`：代理沿目录
祖先链读到仓库根 `CLAUDE.md`/`AGENTS.md`、本总纲与子包指南即可（shared
模块本身无中间层指南）。

## Directory Map

```
packages/shared/network/
├── AGENTS.md                 # 本总纲
├── types.go                  # 别名再导出门面：protocol/codec 迁出符号的 network.X 再导出
├── stream.go                 # 共享 stream 接口：ClientPacketStream/ServerPacketStream/Listener
├── transport.go              # Play endpoint 门面：ClientEndpoint/ServerEndpoint 与 packet/message 转换
├── memory.go                 # Memory transport：NewMemoryStreamPair/NewMemoryPair 与容量背压
├── login.go                  # 登录状态机：BeginServerLogin/PendingLogin/LoginClient 族
├── *_test.go                 # 根包测试：登录/endpoint 门面、Memory 编排、wire 前缀回归与基准
├── protocol/                 # packet/message/registry/snapshot 协议层（指南见 protocol/AGENTS.md）
├── codec/                    # 编解码与帧封装（指南见 codec/AGENTS.md）
│   └── testdata/             # chunk snapshot 版本化 fixture（golden；清单以目录为准）
└── tcp/                      # TCP transport：listener、dial、stream 实现（指南见 tcp/AGENTS.md）
```

## Dependency Direction

依赖方向单向，由 `packages/audit/audit/dependencies.py` 的 `allowed` 表
登记并以依赖检查强制（`go list` 枚举 go.work
五个模块全部内部包的生产 import 逐边比对；未登记的
新包直接报错）。契约文本见 openspec 主规格 `repository-code-organization`。

- 接受：根包 → {`packages/shared/core`, `packages/shared/network/protocol`,
  `packages/shared/network/codec`}；`protocol` → {`packages/shared/core`,
  `packages/shared/companion`}（companion 边服务于常量同源锁定与消息 DTO 的
  领域类型，如 `companion.ID`）；`codec` → {`protocol`, `packages/shared/core`}；
  `tcp` → 根包（transport-only）。
- 拒绝：任何子包反向导入根包（根包类型经 `types.go` 别名对外，协议层与
  编解码层互不感知对方的消费面）；`codec` 依赖 `packages/shared/companion`（wire
  长度上限经 `protocol` 导出常量取值，codec 生产代码不 import companion
  领域包）。
- 新增子包或新边必须先证明方向合理并登记 `allowed` 表，不许先写代码后补
  登记。

## 别名再导出政策 (`network/types.go`)

- 迁出符号在根包 `types.go` 以别名/绑定再导出，保证既有 `network.X` 消费
  面（server/client/sim/cmd 与 `tcp`）源码零改动：密封接口与值类型用
  `type X = protocol.X` 形态（保方法集与 switch 断言身份），`Codec` 用
  `type Codec = codec.Codec`；错误用 `var ErrX = pkg.ErrX` 绑定同一错误值；
  常量与函数经 const/var 绑定同一底层值（`ProtocolVersion`、
  `StateHandshake` 族、`MaxFrameBytes` 族、`WriteFrame`/`ReadFrame`/
  `NewCodec`/`ValidateClientPacket` 等）。别名不产生运行时转发，类型身份、
  `errors.Is` 身份与错误消息不变。
- `ProtocolVersion` 的定义点在 `protocol/packet.go`（版本历史 doc comment
  亦在彼处），根包只再导出；版本钉死由 `protocol` 的
  `TestProtocolVersionPinned` 强制。
- 别名清单是闭集：只覆盖消费方实际引用的迁出符号；未列入的子包导出不加
  别名，根包内部代码直接以 `protocol.`/`codec.` 限定名消费。

## 信任边界

- 所有线上 packet 在编码前和解码后都要经过 state、类型、枚举、数值及长度上限校验；未知 ID、尾随字节和非规范编码必须拒绝。
- 发送成功后的消息及其 slice 视为不可变。需要继续修改时先复制，不得依赖 Memory transport 恰好传递 Go 对象引用。

## 传输一致性

- Memory 与 TCP 必须受同一 packet/codec 契约约束，并复用登录和上层模拟路径；不得为本地连接建立绕过校验、登录或权威模拟的特权路径。
- 根包持有共享 stream 接口、Play endpoint 门面、登录状态机和 Memory transport；协议消息层住 `protocol`、编解码与帧封装住 `codec`、TCP listener/dial/stream 住 `tcp` 且只承担 transport 职责。四者都不决定业务结果，登录状态转换集中在既有 `Login*` 路径。
- TCP 面向可信局域网，现有协议没有认证或加密。不要把它描述为公网安全，也不要用这一既有边界作为降低输入校验的理由。

## 协议演进

升级协议时同步检查 packet 定义、registry ID、双向 codec、Validate、登录版本拒绝、Memory/TCP 一致性、fuzz 与 golden。保留既有 ID 和布局的兼容结论必须由测试证明，不能只改版本常量。

## Documentation Sync Policy

- 修改任一子包的行为、导出面或测试入口，必须同步该子包的 `AGENTS.md`；
  总纲只维护目录地图、依赖方向、别名政策与共享契约，不复制子包细节，
  子包细节不回写总纲。
- 长度上限、版本号、超时预算一律以代码常量与性质测试为准，指南只点名
  常量与测试，不抄数值。
- 网络子包布局或依赖边变化时，同步 `packages/audit/audit/dependencies.py`
  的 `allowed` 表与 openspec 主规格 `repository-code-organization`，三者
  不一致即漂移。

## Focused Verification

按改动域定点（分层纪律见 `docs/notes/test-quickstart.md`）：

| 改动域 | 命令 |
|---|---|
| 根包编排（stream/endpoints/memory/login/types） | `go test ./packages/shared/network -race -count=1` |
| protocol 协议层 | `go test ./packages/shared/network/protocol -race -count=1` |
| codec 编解码与帧 | `go test ./packages/shared/network/codec -race -count=1` |
| tcp transport | `go test ./packages/shared/network/tcp -race -count=1` |
| 全子树（跨域改动） | `go test ./packages/shared/network/... -race -count=1` |
| 依赖方向 / 文档守卫 | `make archcheck` |

- 当前文档入口：`docs/notes/lan-server.md`（TCP 可信局域网信任边界与部署约束）。
