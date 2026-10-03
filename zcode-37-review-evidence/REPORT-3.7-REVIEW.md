# 3.7 复核缺口补足报告（第二轮，ZCode，2026-10-03）

对应正式复核发现 REVIEW37-01/02/03 与报告纠正项。第一轮证据包为提交 `5210448bc`（实际源码 `6c20153c7`）；本轮在 `5210448bc` 之上新增两处源码修复与 78/78 行库存绑定，全部记录于本目录。

## REVIEW37-01 — 逐库存行绑定 + 缺行 Memory/TCP authority 路径（完成）

- 新测试 `local_remote_parity::integration::inventory_command_transcripts_match_across_adapters`：对全部 21 行 `command.protocol.client.*`（19 个 `Command` 族 + ChatCommand + KeepAliveReply）各构造一枚经检构造器校验的 wire 包（参数取自 mornlea_protocol 自有夹具），经 Memory 与 TCP 双 adapter 驱动同一转录（12 包→tick0（10/0/0）→9 包+过期重提→tick1（10/0/1）→排空），断言两侧 `Transcript` 全等，且与直连 `RealEndpoint` 回放的逐 tick 有序事件与逻辑会话事实（phase、`last_applied_sequence`、`next_arrival`）完全一致——即 source-bound 状态与有序输出对照，不涉私有布局。
- `zcode-37-review-evidence/inventory-binding.{md,json}`：78/78 行库存逐行绑定真实集成证据；生成脚本对每条绑定验证「`test <名> … ok`」确实存在于所指日志，未验证行强制标 OPEN（当前 0 行 OPEN）。伴生动作 4 行按其真实入口（Agent 租约/MCP，非客户端 adapter）绑定 `full_corpus` 真实 helper 用例与 `agent_process` 集成。
- digest/provider-only 不再作为集成依据：绑定表的首选证据为 adapter/进程/磁盘路径测试。

## REVIEW37-02 — FIFO001 限定修复 + 一次原完整目标（完成）

- `scripts/rust-server-opt-in.sh` 限定性能修复（reviewer 授权跨界 3.8 专属文件的裁决已记入账本）：`py()` 增 `-S -E`（全部 helper 仅标准库）；`cmd_rollback` 7 次 manifest 读批为 1 次（NUL 分隔，字段域不含 NUL）；`previous_binding_validate` 3→1；`check_baseline_identities` 7 族校验批为 1 次保持首分歧次序；`confine_paths` 9→1 保持逐路径首失败次序。拒绝身份/文本、`process_terminated()` 字节边界、oracle 字面量、一切截止与安全检查逐项保留（独立评审确认）。
- 实测：rollback prework python3 启动 54→26 次（-52%），单次启动成本约减半；prework 墙钟 ~1.99s（原 ~2.44s 空载 / 负载下 4.5–5.0s）。
- **修复后在源码 SHA（本轮最终源码）上完成一次原完整目标**：`26-persistence-full-postfix` = **271 通过 / 0 失败 / 0 忽略**（562.9s），原 5 秒 FIFO 截止、全部安全检查与历史失败保留；fifo 双例此前另 3 次聚焦复跑 + self-test 全过。

## REVIEW37-03 — pending-save 断连的真实 store 组合（完成）

- 新测试 `local_remote_parity::integration::tcp_disconnect_pending_save_completes_through_real_disk_store`：TCP 登录→真实脏通道暂存+select（in_flight=1）→`close(PeerGone)` 会话退役且 save 存续→**真实 `DiskStore::open`→`write`（产出 completion，非手工构造）→`sync`→`apply_completion`（acked=1、retry 空、in_flight=0）→`close`→重开→`load` 复载 revision=9/persisted_revision=9**。fixture 边界在测试注释中如实记录：暂存快照为合成极小夹具，completion/sync/reload 为一次性临时目录上的真实 I/O；未扩大为可执行 runtime 实现。

## 报告纠正（已落实）

1. `zcode-37-evidence/summary.json` 重建为全部 24 条记录（原仅 14 条）；本轮 `zcode-37-review-evidence/summary.json` 含 25–27 号记录与绑定表汇总。
2. 完整 persistence 失败历史保留：第一轮 270/1（`fifo_log_without_reader` 5 秒截止）如实保留于 04/18/23 号记录与第一轮报告；本轮 271/271 为修复后结果，不追溯改写。
3. previous package 与双 binary selector 已补录（`selector-identities.txt`）：previous-server `7ad94532…`、previous-verifier `46f76ffa…`、manifest `3b1e1f44…`（oracle `360609e4` 与脚本字面量一致）、release `mornlea-server` `d2219bb6…`、**CARGO_BIN_EXE debug `mornlea-server` `6cdf72a2…`、helper 解释器 `bc56ea9c…`**。
4. CARGO_BIN_EXE 与 release 区分：`activation.rs:85` 的 `rust_bin()` 在 cargo test 下优先取 `CARGO_BIN_EXE_mornlea-server`（debug，`6cdf72a2…`）；`MORNLEA_RUST_SERVER_BIN`（release，`d2219bb6…`）仅在无该编译期变量时生效。此前报告未区分，本轮更正。
5. Rust 源一致性更正：`054b51b2..6c20153c7` 含 `b42375a6c`（`core/actor_snow.rs` +377 行、`core/mod.rs`、`AGENTS.md`）——第一轮「未改任何 Rust 代码」的宣称只对单提交 `6c20153c7` 成立，对区间不成立，予以撤回并更正。
6. 首轮 `seventeenth_connection` EINVAL：根因未确证。已证事实仅为：3 次聚焦复跑 + 整套件复跑均通过、ROOT 在 `054b51b2`（Rust 源不同）上通过该套件；不宣称「负载根因」。

## 执行记录（本轮全部命令）

| 记录 | 命令要点 | 退出 | 耗时 | 结果 |
|---|---|---|---|---|
| 25 | `cargo test -p mornlea_server --locked --test local_remote_parity` | 0 | 0.7s | 44/44（含两新测试） |
| 26 | `--test persistence_failure`（完整、夹具环境） | 0 | 562.9s | **271/271** |
| 27 | `go test ./packages/audit -count=1` | 0 | 177.4s | ok（171.6s） |
| — | `bash -n` / `--self-test` / `fifo_log`×4 / `activation::activation_verifier` / `activation::` 全模块 | 0 | — | 全过（B 工人记录） |
| — | `cargo fmt --all --check` / `clippy --tests -D warnings` | 0 | — | 净（A 工人记录） |

平台：macOS 26.6.2 arm64；rustc 1.97.1（rustup 锁定）；go1.26.0 darwin/arm64；Python 3.12.14。环境：`TMPDIR=/private/tmp/f2z37`、`MORNLEA_AGENT_PYTHON=<主 worktree venv>`、`MORNLEA_PREVIOUS_PACKAGE=<pkg3>/previous-runtime.json`、`MORNLEA_PREVIOUS_SERVER_BIN=<pkg3>/previous-server`、`MORNLEA_RUST_SERVER_BIN=<release>`。

## 未执行项、阻塞和风险

1. 远端（云 Linux）复验仍阻塞：环境按用户指示停止断连；本地 loopback TCP 即真实 TCP adapter（复核已确认），但跨机远端运行证据仍缺。
2. `seventeenth_connection` EINVAL 根因未确证（不复现，已如上更正表述）；如再发需单独定位。
3. 3.7l3ns0/3.8/3.9l10/4.1/4.2 仍开放；本轮对 3.8 专属脚本的跨界为 reviewer 授权的限定性能修复，已记账本。
4. 后继 RUNTIME003/SNOW004/SUB005/PUB006/EVID007 按各自范围对账，本轮未代替亦未扩张。
