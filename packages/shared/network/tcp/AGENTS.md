### 包边界 (`packages/shared/network/tcp/tcp.go`、`packages/shared/network/tcp/stream.go`)

`ListenTCP` 与 `DialTCP` 只把 TCP listener、连接、stream 和 socket 生命周期接到父包 `packages/shared/network` 的接口上。除标准库外，本包只能单向依赖父包；packet、schema、codec 与 login 状态机均由父包持有，TCP 层不得加入 gameplay 决策或另建登录路径。

listener 的 `Addr` 返回实际绑定地址，server stream 的 `Peer` 返回远端地址；上层只通过父包接口消费这些 transport 信息。

`tcpStream` 通过父包 `network.Codec` 编解码 packet，并以 `network.ReadFrame`、`network.WriteFrame` 读写 frame。保持这一入口唯一，才能让 Memory 与 TCP 受同一协议和校验契约约束。

### 所有权与取消 (`packages/shared/network/tcp/stream.go`、`packages/shared/network/tcp/tcp_test.go`)

每个 stream 同时各有一个 read owner 和 write owner，每个 listener 同时只有一个 `Accept` owner；等待 owner 的调用必须响应自己的 `context` 取消或 deadline，不能因前一个调用阻塞而失去取消能力。

读写操作把 `context` deadline 安装到对应 socket 方向，并用 `context.AfterFunc` 在取消时唤醒阻塞 I/O。清理必须停止或等待回调结束后再清空 deadline；`Accept` 的轮询 deadline 也必须在返回时清空，避免一次超时污染后续调用。

这些不变量由 `tcp_test.go` 中的 owner 等待、预取消、阻塞 I/O 唤醒以及后续 `Send`/`Recv`/`Accept` 可复用回归覆盖；新增并发入口不得绕过同一 owner 和清理路径。

### Socket 与关闭语义 (`packages/shared/network/tcp/tcp.go`、`packages/shared/network/tcp/stream.go`)

拨号和接受连接后都配置 `TCP_NODELAY`、keepalive 与 30 秒 keepalive period。任一步配置失败都要关闭已取得的 socket；codec 初始化失败也要关闭连接，不能泄漏部分初始化资源。

stream 与 listener 的 `Close` 均须幂等。EOF、unexpected EOF、closed pipe、socket closed、connection reset、aborted 和 broken pipe 统一映射为 `network.ErrClosed`；若 `context` 已取消或到期，则保留 `context.Canceled` 或 `context.DeadlineExceeded`，不伪装成连接关闭。

`tcp_test.go` 的并发关闭、关闭唤醒、坏 frame 隔离和 socket option 失败测试锁定这些语义；协议或 codec 错误只关闭当前连接，不得连带关闭 listener。

### Framing 与回归边界 (`packages/shared/network/tcp/transport_consistency_test.go`、`packages/shared/network/tcp/tcp_test.go`、`packages/shared/network/tcp/login_tcp_regression_test.go`)

正常 Memory/TCP parity 必须通过 `transport_consistency_test.go` 的真实 `network.NewMemoryStreamPair` 与 TCP pair 运行相同 transcript。raw Memory 与 raw wire 绕过只用于注入旧版本、非法编码或 malformed 状态，不得成为生产 helper 或普通一致性测试路径。

`login_tcp_regression_test.go` 保留 raw TCP 登录拒绝和 malformed UTF-8 的 wire 级回归；`tcp_test.go` 作为同包私有测试保留 owner、deadline、socket 配置失败与底层连接关闭覆盖。移动测试时不得为访问私有状态而扩大生产 API。

### 定点验证 (`packages/shared/network/tcp/`)

- 修改本子包：`go test ./packages/shared/network/tcp -race -count=1`
- 修改父包共享 packet、codec、frame、login 或 stream 契约：`go test ./packages/shared/network/... -race -count=1`
- 修改依赖方向：`make archcheck`
