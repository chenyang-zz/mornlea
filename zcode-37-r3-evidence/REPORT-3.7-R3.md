# 3.7 第三轮整改报告（ZCode，2026-10-03）

对应第二轮正式复核四项发现（R2-NUL、REVIEW37-01 精确绑定、REVIEW37-03 顺序语义标注、执行身份与 manifest 自条目）。代码基线 `484cb02de`；整改提交 `74ee37edd`（脚本）+ `1b0367c5a`（测试）；封存验证全部在该干净源码身份上执行。

## 1. R2-NUL（P2）— 已修复并以回归覆盖（提交 `74ee37edd`/`1b0367c5a`）

- `manifest_get_batch` 在输出任何字段前检查解码值内嵌 NUL，类型化拒绝 `FAIL invalid_manifest manifest field <name> contains an embedded NUL`（JSON `\u0000` 会解码为字面 NUL，原注释「NUL 不可能出现」已更正）；`cmd_rollback` 7 处与 `previous_binding_validate` 3 处 `IFS= read -rd ''` 全部加 `|| fail "invalid_manifest" "manifest field batch is incomplete"`，截断/短读成为类型化拒绝。
- 回归测试 `activation_verifier::manifest_batch_embedded_nul_and_truncation_refuse`（+157 行）：以**生产文本抽取**（复用 `run_script_functions` 与 `process_termination_shell` 模式，锚点为 `cmd_rollback` 独有读块）覆盖四类——NUL 字段拒绝、空字段占位、正常字段序、截断流拒绝；另有端到端负例：真实 activation manifest 的 `world_path` 写入 `\u0000`，跑真实 `rollback --data-policy compatible`，断言类型化拒绝、manifest 字节不变、世界树不变、无 listen/verifier 产物、租约释放，沿用原 5 秒界。
- 评审确认：`process_terminated` 字节边界、oracle 字面量、`-S -E`、全部截止与安全检查不变；拒绝不变量「stdout 永不含非分隔 NUL」对所有分支成立（dict/list 经 `json.dumps` 转义）。
- 本轮只主张「字段错位已杜绝为类型化拒绝」，不宣称更广泛的安全绕过修复。

## 2. REVIEW37-01（P1）— 逐行精确绑定完成：71 BOUND / 7 OPEN

- `zcode-37-r3-evidence/bindings-v2.json`（源数据由只读抽取代理逐测试源码核对产出，控制器策展）+ `gen-inventory-binding-v2.py`：**拒绝宽前缀**（仅 `模块::` 不带函数名即 FAIL），每个引用测试名必须在所指日志中逐字出现 `test <名> … ok` 才计 BOUND；每行携带 class（admission/provider/order-guard/real-integration）、scenario、preconditions、expected、assertions。
- 结果 **71/78 BOUND**（名称零错配），**7 行 OPEN** 为真实实现缺口（Rust 权威尚未发布的事件族：`event.domain.chat`、`companion-despawn`、`companion-spawn`、`forget-chunks`、`remote-player-despawn/spawn/states`——这些事件在 `mornlea_server` 测试中从未被构造，属迁移后续节点的生产实现工作，3.7 集成证据不可替代填补，按「缺失项保持开放」处置）；`RequestChunkResync` 以 admission 类绑定双 adapter 转录并保留 provider-leg gap 注记。
- 多个 `event.domain.*` 行以「状态级锚点 + note」诚实分类：引用的 provider 测试钉住事件族镜像的确切状态（方块/revision/槽位/位置），事件本体尚未由 Rust 权威发布；五族（combat-hit、player-state、place-block-succeeded、chunk-snapshot、command-rejected）有事件级断言。

## 3. REVIEW37-03 — 顺序语义精确标注（提交 `1b0367c5a`，仅注释）

`tcp_disconnect_pending_save_completes_through_real_disk_store` 的文档注释改为精确主张：断连发生在 save 处于 pending（已 select、未提交）时；真实 DiskStore completion 在关闭**之后**结算；本用例证明「关闭后经真实独占 store 结算」，**不主张**与断连并发的后台写交叠（未做交叠声称，故无需交叠证据；后台 owner 证据为 `persistence_failure::background` 域，另案对账）。断言与名称未动。

## 4. 执行身份与 manifest — 干净源码封存 + 自条目纠正

- 25–27 号（第二轮）记录如实保留其历史身份（HEAD `5210448bc` + 两份 dirty 文件），不追溯改写；第三轮 31–34 号在**提交后的干净树**上执行：全部 `sealed=true`，`source=1b0367c5ac6caacefa9aa9274ca9086633e5d121`，`tree=f660a17d34306ffdab586ac63e6e428744165ce9`，`tracked_dirty_at_start=null`。记录器（`zcode-37-r3-evidence/run-record.py`）逐条捕获 HEAD/tree/脏状态/脏 diff 哈希，脏树时拒标 sealed。
- manifest v3（`gen-manifest.py`）：**排除自身条目**（第二轮自条目为空串 SHA `e3b0c442…` 系生成时重定向截断又被 glob 收入——已定性并纠正），写后逐条复核，17 条目全验证；由证据提交对象绑定（见交付 SHA）。
- 选择器身份（`selector-identities.txt`）：previous-server `7ad94532…`、verifier `46f76ffa…`、manifest `3b1e1f44…`、release server `d2219bb6…`、**CARGO_BIN_EXE debug server `6cdf72a2…`**（`rust_bin()` 在 cargo test 下优先取此 debug 二进制）、helper 解释器 `bc56ea9c…`。

## 5. 执行记录（第三轮，全部 sealed @ `1b0367c5a`）

| 记录 | 命令要点 | 退出 | 耗时 | 结果 |
|---|---|---|---|---|
| 31 | `cargo test --test local_remote_parity` | 0 | 3.3s | 44/44 |
| 32 | `--test persistence_failure`（完整、夹具环境、干净树） | 0 | 545.3s | **272/272**（271+新 NUL 回归；原 5 秒 FIFO 截止保留） |
| 33 | `--test persistence_failure manifest_batch_embedded_nul` | 0 | 4.4s | 1/1 |
| 34 | `go test ./packages/audit -count=1` | 0 | 184.4s | ok（177.6s） |

工人侧门禁（提交前，dirty 状态下）：`bash -n`、`--self-test`、activation_verifier 15/15、fifo_log 2/2、parity 44/44、fmt/clippy 净。平台：macOS 26.6.2 arm64 · rustc 1.97.1 · go1.26.0 · Python 3.12.14；`TMPDIR=/private/tmp/f2z37`。

## 6. 未执行项、阻塞和风险

1. 远端（云 Linux）复验仍阻塞（环境停止断连；本地 loopback TCP 按复核裁决计为真实 adapter）。
2. 7 行 OPEN 事件族 + RequestChunkResync provider-leg：Rust 权威未实现对应事件发布/resync 语义，属后续节点生产工作，非 3.7 可闭合。
3. `seventeenth_connection` EINVAL 根因仍仅为「未复现」记录（3 聚焦+整套复跑过）。
4. 3.7l3ns0/3.8/3.9l10/4.1/4.2 仍开放；后继 RUNTIME003/SNOW004/SUB005/PUB006/EVID007 按各自范围对账，未代替未扩张。
