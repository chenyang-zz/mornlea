# 3.7 真实本地/远端运行与端到端集成证明 — 取证报告（ZCode，2026-10-03）

按用户指令交付：只补齐 3.7 的运行与集成证据，不勾选任务、不改 tracked 文件、不提交、不推送。3.7 是否关闭由用户裁决（本行标「待 review」）。

## 1. 基线与身份

| 项 | 值 |
|---|---|
| 主 worktree 基线 | `a9401855fa42e34c4c1bfb79aff27a4b86d14d41`（`dev`，工作树干净；其 tasks.md 为陈旧全未勾选状态，非本任务对象） |
| 被验证 SHA（F2 前沿） | `6c20153c7792d72d4cbbd1e80ad3fa5b3f9e69d0`（分支 `codex/f2-mac-20261003-task3`，本地领先远端 `bd1be6e45` 一个提交，未推送） |
| 隔离取证环境 | 独立 worktree `/Users/chen/work/mornlea-f2-37-zcode`，分支 `f2/zcode-37-evidence` @ `6c20153c7`；tracked 零修改（仅 untracked `zcode-37-evidence/`） |
| 平台/构建身份 | macOS 26.6.2 · arm64 · rustc 1.97.1（rustup 锁定）· go1.26.0 darwin/arm64 · Python 3.12.14（agent venv）· cargo `--offline --locked` |
| 并行控制器状态 | Codex ROOT 仍活跃于其自有 worktree（处理 3.9l10 FIFO 截止诊断），文件所有权无交叠；其 fixture 进程 PID 60663 全程未被触碰 |
| 远端环境 | 云端 Linux 已按用户指示停止并断连（ledger 2026-10-03 Mac handoff 记录）→ 远端复验不可用，列为开放阻塞项 |

已有提交核实：`6c20153c7` 本身是 3.9l10 的实现提交（`fix(storage): preserve portable verifier fixture invariants`，只改 Go 存储测试 fixture、`scripts/rust-server-opt-in.sh` oracle 哈希与 OpenSpec 文档，**不含任何 Rust 代码**，与 ROOT 在 `054b51b2` 上整套件全过的 Rust 源完全一致）。本次验证**未产生任何新提交**。

## 2. 能力覆盖清单（对照 tasks.md 与 plans/01-server-slices.md 的 3.7 packet）

3.7 packet 要求：四具名套件全跑、聚焦 Go oracle、Memory/TCP 全库存回放、有序域/控制输出断言、真实保存重启、3.6d 真实 Rust→Python/MCP 候选准入、Go fixture 摘要核对、三诱导场景（断连保存/启动读取失败/过期候选）各有明确结果。实测结论：

| # | 3.7 能力 | 真实测试载体（全部执行） | 结果 |
|---|---|---|---|
| 1 | Memory/TCP 同一转录回放、会话编号/回执/有序事件/排空控制帧逐项相等、store seed 跨传输一致 | `local_remote_parity::integration::shared_transcript_matches_across_adapters` | ✅ PASS（01 日志） |
| 2 | TCP 传输中 pending save 的断连诱导：save 保持 in-flight、会话退役、断连后 ack | `local_remote_parity::integration::tcp_disconnect_during_pending_save_keeps_save_in_flight` | ✅ PASS（01 日志） |
| 3 | 真实保存→关闭→重开→读回（chunk revision/block、玩家位姿/生命、seed 等逻辑字段比对，DiskStore 真实提交+sync） | `persistence_failure::integration::real_save_restart_round_trip_through_store` | ✅ PASS（23 日志） |
| 4 | 启动读取失败诱导（损坏 metadata）：报告失败并保留原始字节，不铸造空白世界 | `persistence_failure::integration::failed_read_at_startup_reports_without_blank_world` | ✅ PASS（23 日志） |
| 5 | 完整能力库存（78 行 capability-inventory）Go 源摘要核对 + fixture 文件存在性 | `server_replay::full_corpus::inventory_source_digests_match_go_fixtures` | ✅ PASS（02 日志） |
| 6 | 全库存逻辑回放确定性（逻辑状态/事件跨运行一致，非私有布局） | `server_replay::full_corpus::logical_state_and_events_match_across_runs` | ✅ PASS（02 日志） |
| 7 | 真实 Python helper 下 Agent 候选准入 + **过期候选拒绝** | `server_replay::full_corpus::real_agent_candidate_admitted_and_stale_refused` | ✅ PASS（02 日志） |
| 8 | 真实 Rust→Python/MCP：计划经真网关+MCP 快照工具+权威准入；记忆 commit/reconcile；cancel/shutdown 截止；release 失败保租约 | `agent_process::integration::{rust_plan_python_mcp_and_authority, real_memory_commit_reconcile, block_cancel_deadline_and_shutdown, release_failure_retains_same_lease}` + finalization 2 例 | ✅ 6/6 PASS（05 无捕获形态 19 日志含真实子进程身份） |
| 9 | 3.6d 门禁重复（04 号计划：3.7 repeats this gate） | `make companion-agent-check`（ruff/mypy/419 Python 单测）、`make companion-agent-integration`（Go -race 跨语言）、`uv run pytest tests/test_http_v1.py::test_shutdown_stops_accepting_cancels_runs_then_closes_model_and_sqlite` | ✅ 全 PASS（20/21/22 日志） |
| 10 | 玩法组合（全部规则族真实回放：作物/流体/合成/熔炉/战斗/投射/掉落/睡眠/环境/容器/伴生物…） | `server_replay` 套件 452 例（含 `phase_order::*` 冻结顺序与订阅搬运次序） | ✅ 452/452（02 日志） |
| 11 | 订阅与可见有序发布 | `phase_order::*`（订阅先于敌对可见性等）、`server_contract::{publication,prepared_publication,prepared_delivery}`、`local_remote_parity::tcp::mixed_publication_has_identical_owned_memory_and_tcp_frames`、`prepared_delivery::actual_*_canonical_fifo_and_budgets` | ✅ 随套件全过 |
| 12 | 会话/准入/传输契约（登录、容量、超时、EOF 退役、两观察者 fixture…） | `server_contract` 275 例 + `local_remote_parity` 42 例 | ✅ 275/275（24 复跑）+ 42/42 |
| 13 | 聚焦 Go oracle（3.1/3.2/3.3a/3.3b/3.4b/3.5 packet 所列 8 条命令） | 见 §3 表 07–14 | ✅ 8/8 全过 |

## 3. 运行记录（命令 → 退出码/耗时/日志）

所有命令在隔离 worktree 由 `zcode-37-evidence/run-record.py` 记录（argv、cwd、UTC 起止、exit、env、完整 stdout；`*.json` 为元数据、`*.log` 为全文；SHA256 见 `manifest-sha256.txt`）。

阶段 1（无激活夹具）：

| 记录 | 命令（要点） | 退出 | 耗时 | 结果 |
|---|---|---|---|---|
| 01 | `cargo test -p mornlea_server --test local_remote_parity` | 0 | 1.5s | 42/42 |
| 02 | `--test server_replay` | 0 | 8.6s | 452/452 |
| 03 | `--test server_contract` | 101 | 5.4s | 274/275（见 §5-R3） |
| 04 | `--test persistence_failure` | 101 | 28.3s | 229/271（42 例缺激活夹具 env，属 3.8/3.9l 域） |
| 05 | `--test agent_process` | 0 | 16.0s | 6/6 |
| 06 | `make rust`（release + dylib 部署） | 0 | 400.8s | PASS |
| 07 | `go test ./packages/server/sim/runtime -run 'StepWithTunables…|…'` | 0 | 9.5s | ok |
| 08 | `go test ./packages/shared/network -count=1` | 0 | 2.7s | ok |
| 09 | `go test ./packages/server/server -run 'Login'` | 0 | 24.6s | ok |
| 10 | `… -run 'Local|Login'` | 0 | 13.3s | ok |
| 11 | `… -run 'TCP|Login'` | 0 | 76.9s | ok |
| 12 | `go test ./packages/server/server/persistence -run 'Autosave|…'` | 0 | 5.5s | ok |
| 13 | `go test ./packages/server/storage/chunk -run '^TestRegion'` | 0 | 2.9s | ok |
| 14 | `go test ./packages/server/storage -run 'WorldLock|DiskStore(Sync|Close)|WorldBackup'` | 0 | 4.5s | ok |

阶段 2（夹具与门禁形态）：

| 记录 | 命令 | 退出 | 耗时 | 结果 |
|---|---|---|---|---|
| 15–17 | `--test server_contract agent_mcp::lifecycle::seventeenth…`（×3 聚焦复跑） | 0 | ~1s×3 | 3/3 PASS |
| 18 | `--test persistence_failure`（MORNLEA_PREVIOUS_PACKAGE 误传目录） | 101 | 21.0s | 被 23 取代；教训已记录 |
| 19 | `--test agent_process -- --nocapture`（3.6d 门禁形态） | 0 | 18.2s | 6/6，真实子进程身份：`pid=12493 source_sha256=6beba9c8… executable_sha256=bc56ea9c… stop_ms=12` |
| 20 | `make companion-agent-check` | 0 | 50.0s | ruff/mypy/419 Python 单测全过 |
| 21 | `make companion-agent-integration` | 0 | 69.0s | Go -race 跨语言 16.8s ok |
| 22 | `uv run pytest -q tests/test_http_v1.py::test_shutdown…` | 0 | 4.7s | 1 passed |

阶段 3（修正夹具：`MORNLEA_PREVIOUS_PACKAGE=<pkg>/previous-runtime.json`（manifest 文件）、PATH 前置 venv python3.12）：

| 记录 | 命令 | 退出 | 耗时 | 结果 |
|---|---|---|---|---|
| 23 | `--test persistence_failure` | 101 | 566.4s | **270/271**；唯一失败 `activation::activation_verifier::fifo_log_without_reader_refuses_before_verifier_spawn`（5s 观察截止；与 ROOT 两次整套件记录完全一致） |
| 24 | `--test server_contract`（整套件复跑） | 0 | 3.1s | **275/275** |

## 4. 夹具身份与一次性数据

- 密封 previous 包（只读复用 ROOT mac-evidence 已验证 custody）：`previous-package3/`，manifest SHA256 `3b1e1f44…`；`previous-server` `7ad94532…`、`previous-verifier` `46f76ffa…`、previous 源 `d042982d`、oracle `360609e4…`（== HEAD `scripts/rust-server-opt-in.sh` 期望值）。
- 本次 Rust 二进制：`packages/engine/target/cargo/release/mornlea-server` SHA256 `d2219bb6…`（本 worktree `make rust` 产物）。
- Python helper：主 worktree agent venv `python 3.12.14`（只读解释器；helper 源/解释器 SHA 见 19 日志）。
- 一次性测试数据：`TMPDIR=/private/tmp/f2z37` 与各测试自建 TempDir；**未触碰任何生产世界**；ROOT 的 restore fixture（PID 60663 `/private/tmp/mornlea-f2-previous-server`）全程存活未动。

## 5. 未满足项（保持开放）与风险

1. **[开放·阻塞远端项] 远端复验不可用**：云端 Linux 环境已按用户指示停止断连；3.7 的"远端"半边只能引用 ledger 中既有云侧记录（更早 SHA）+ 本次 Mac 本地全量证据。
2. **[开放·3.9l10 域·非 3.7 场景] FIFO 5 秒观察截止 flake**：整套件负载下 `fifo_log_without_reader/held_reader_refuses_before_verifier_spawn` 超时；ROOT 插桩证据（Python 实体仅 ~160–260ms，4.5–4.8s 消耗在数十次 Python 启动的调度间隙；FIFO open 本身 <0.3ms，errno6 行为正确）支持环境时序结论。本机 270/271 与 ROOT 一致复现；聚焦/小组运行通过。修复归属 ROOT 的 3.9l10 验收，本任务不越权改动。
3. **[已分类·不复现] server_contract `seventeenth_connection…` 单次 EINVAL**：03 号整套件 1 例失败；3 次聚焦复跑 + 24 号整套件复跑全过；且 ROOT 在 Rust 等价源（`054b51b2`）上 275/275。定性为并行负载下客户端 socket 一次性观察，非产品缺陷；记录备查。
4. **[范围外仍开放] `3.7l3ns0`、`3.8`、`3.9l10`、`4.1`、`4.2`** 保持未勾选，不在本任务范围。
5. 风险：主 worktree `dev` 的变更工件（tasks.md 全未勾选）与前沿分支严重漂移，集成时需以前沿分支工件为准合并，避免误用陈旧清单。

## 6. 结论

3.7 packet 定义的全部能力——四具名套件、8 条聚焦 Go oracle、Memory/TCP 全库存回放与输出对照、订阅/可见有序发布、真实保存重启、真实 Rust→Python/MCP 集成与 3.6d 门禁重复、以及断连保存/启动读取失败/过期候选三诱导场景——在 `6c20153c7` 的 macOS 本地隔离环境**全部取得真实通过证据**；唯一整套件失败为已知的 3.9l10 域 FIFO 截止 flake（保持开放）。无代码改动（tracked 零修改）、无提交、无推送。3.7 勾选与「待 review」标记由用户执行。

证据目录：`/Users/chen/work/mornlea-f2-37-zcode/zcode-37-evidence/`（53 个文件 SHA256 清单：`manifest-sha256.txt`；逐命令元数据 `NN-*.json`；全文日志 `NN-*.log`；复现脚本 `run-all*.sh`/`run-record.py`）。
