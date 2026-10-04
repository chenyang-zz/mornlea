# Agent 运行记录

记录规划者（Planner，`docs/agents/planner.md` / `docs/agents/planner-prompt.md`）每轮运行的输入、变更行与结论；实现者（Implementer）的关键裁决如有需要也在此追加。本文件只记事实，规划单一真相源仍是 `docs/feature-backlog.md`。

## 2026-08-24 08:04 PDT（规划者首轮）

- **读取输入**：`docs/feature-backlog.md`、`docs/notes/progress.md`、`AGENTS.md`、Discussion #71（正文 + 0 条评论）、`origin/main` 近 20 提交（头 `6922f189`）、`codex/*` 分支与 `.worktrees/`、hunger/farming/authoritative-fluid/fluid-presentation/lod-shell/first-night 等归档 change 的「遗留与简化清单 / 非目标 / 延期与放弃」。
- **变更行**：新增 `B-27`..`B-32`、`D-09`、`F-03`；修订 `A-04`（分支头 `7c3d5e60` → `eb1923eb`，持久化修复已提交、worktree 干净）、`B-01`（肉类依赖 B-27）、`B-02`（无限水源规则随本行裁决）、`B-13`（v25 近战上线后攻击疲劳已可先行）、`B-26`（与 B-27 联动评估）；B–F 组表新增「版本与契约影响」列（对齐 planner 提示词 7 字段要求），A 组表不动（契约冻结于批次设计）。
- **新增行来源**：hunger 遗留 1/6/10/11、authoritative-fluid 与 fluid-presentation proposal 非目标（岩浆/造石、水流推力、流体音效、第三人称与姿态）。
- **未落行（判定）**：hunger 遗留 8（回血计时冻结，现状与 MC 一致、仅可选升级）→ 待澄清；潜行、梯子、水下呼吸装备/附魔（无来源或依赖附魔整体裁决）→ 待澄清挂 Discussion；farming 遗留 8 为删除线勘误、无需行；farming 遗留 1–25 其余条目核对后全部已有对应行。
- **提交**：`83cc9020`（docs: plan B-27..B-32, D-09, F-03）。
- **讨论同步**：追加评论（未改正文表格，正文状态与仓库一致；新行以仓库文件为准）。
- **留给下一轮 / 用户**：
  1. `docs/superpowers/specs/2026-08-23-egui-tool-ui-selection-design.md` 被 `AGENTS.md` 与本表 D 组引用，但**从未入库**（工作区有未跟踪副本）——需用户确认后提交，本轮按「保留用户改动」未代交。
  2. `docs/agents/planner-prompt.md` 工作区改动仅为文件尾换行，保留未提交。
  3. 待澄清项待用户/讨论结论后落行或放弃。
  4. 旧分支清理（如已合入 main 的 `codex/archive-five-way-wave`、`codex/authoritative-player-melee`）非规划者职责，仅记录。

## 2026-09-01 09:19 CST（规划者第二轮）

- **读取输入**：`docs/feature-backlog.md`、`docs/notes/agent-runs.md`（上轮 2026-08-24 08:04 PDT，本轮为第二轮）、`docs/notes/progress.md`、根 `AGENTS.md` 与 `internal/AGENTS.md`、Discussion #71（`gh api graphql` 因 token 失效不可用，改经公开网页读取正文与可见评论；30 条更早评论未翻页）、`origin/main` 近 20 提交（头 `b14e78ec`，本轮网络故障一次 `git fetch` 失败后重试成功）、`git branch -a`/`git worktree list` 及 15 个 worktree 的头 SHA 与脏状态、`openspec/changes/archive/` 自 2026-08-24 起新归档的 40 个 change 的 `design.md`/`proposal.md` 遗留横扫（4 个并行只读子代理分批抽取）。
- **变更行**：
  - 新增 `B-38`（采掘耕地连带收获上方作物）、`B-39`（骨粉获取路径）、`B-40`（熟马铃薯与熟胡萝卜）、`B-41`（流体音频扩展）、`D-13`（他者采掘裂纹呈现）、`D-14`（方块交互粒子与音效）、`D-15`（设置项扩展）、`E-15`（删除 `rust-engine-fluid` Go oracle）、`E-16`（删除 `internal/mesh` oracle 并更正措辞）、`E-17`（`internal/client` Receiver 就绪探测）、`E-18`（`EstimatedBytes` 双计入语义修正）、`F-09`（A-01 归档顺延项清偿）；全部出自归档 change 的显式非目标/延期条目，默认 `排队`，三个跨机制面（B-41/D-14/D-15/E-18）为 `设计候选`。
  - 校对：`A-03` 已认领→**已完成**（PR #124 squash merge `90188fbc`、change 归档 `archive/2026-08-30-tiered-swords-combat`、协议 v32；本地 worktree 头 `8c9e7fe3` 为被取代旧实现）；`B-04` 排队→**就绪**（发布列车队首晋升：前序 A-05 已完成、无在途编号/协议/schema/ABI 持有行）；`B-11`/`E-03`/`E-04`/`E-14`/`B-24`/`A-01` 备注补证据与移交关系（B-11 记录未登记在途分支 `feat/B-11-authoritative-difficulty` 头 `95f733f0`；E-04/E-14 的 mesh 切片阻塞随 A-02 合入解除、独立成 E-16）。
- **未落行（判定）**：已对齐 MC 或「若要」式条件升级——耕地×锄头耐久（fix-hoe-harvest-durability 遗留 1）、干耕地退化概率可配、骨粉随机多阶段、伙伴踩踏、泛化多掉落判据、受伤/死亡进食进度镜像与进度分母跟随 tunable、`CGWindowListCreateImage` 后继、persistence all-owner 契约转正、双门联动/铁门/铰链；无来源出处——梯子、水下呼吸装备/附魔（依赖附魔裁决）。**丢弃**：任务编号注释全仓清理（已由 `fix/task-comment-code-comments` 清偿，archcheck 门禁在位）、三份 rust-engine 主规格措辞（E-14 已清偿）、pause-menu 移交 golden（F-05 已清偿）、`region_crash_test` 归属（实施期已裁决）、主菜单世界全景背景（D-12 已交付）、pathfind 非目标的其它包整理（已被四个分拆 change 覆盖）、tiered-swords 的暴击/护甲/投射物/状态效果/难度（B-23/B-24/B-25/B-11 已有行）。
- **提交**：`1dda29dd`（docs: plan B-38..B-41, D-13..D-15, E-15..E-18, F-09）与本运行记录提交，**两笔均留在本地未推送**（本地 `main` 领先 `origin/main` 2 个快进提交）。
- **推送**：**失败**——重试 5 次（含 `-c http.version=HTTP/1.1`），前两次为网络层失败（`HTTP2 framing layer`／`Empty reply`／443 连不上），恢复后稳定报 `could not read Username for 'https://github.com'`：`credential.helper=osxkeychain` 在本会话取不到 github.com 凭据，且 `gh` 默认账号 token 失效，无可用的非交互凭据源。规划者不做任何凭据旁路，终止推送；两笔提交可由用户在本机恢复凭据后直接 `git push origin main`（快进，无重放需要，远端未前进）。
- **讨论同步**：**未执行**——同上凭据原因，正文刷新 `scripts/agents/refresh-discussion.py --update`（依赖 `gh api graphql`）与状态变更评论均发不出去。仓库文件为准，讨论镜像当前落后（仍列 A-03 已认领、无就绪组、缺本轮 12 条新行），待凭据恢复后补一次 `--update` 与汇总评论。
- **留给下一轮 / 用户**：
  1. **本地两笔 docs 提交待推送**（`1dda29dd` + 运行记录提交，快进）：需先恢复 github.com 凭据（keychain 或 `gh auth login -h github.com`），再 `git push origin main`；凭据恢复后同一次会话里补讨论正文 `--update` 与状态变更评论（A-03 已完成、B-04 就绪、12 条新行、B-11 在途分支提示）。
  2. 未登记的在途分支（均未并入 `main`、工作区干净）：`feat/B-11-authoritative-difficulty`（头 `95f733f0`，含完整 change 产物与 server/storage/sim 实现，最后活动 2026-08-28，本表仍排队）、`feat/extract-companion-agent-service`（头 `09e18bdc`）、`refactor/sim-ownership-convergence`（头 `b54abb9a`）、`.claude/worktrees/feat-ui-changes`（头 `59c2544e`，未跟踪目录）。认领登记与处置属控制会话/用户裁决，规划者未触碰。
  3. `A-03-tiered-swords-combat` worktree（头 `8c9e7fe3`）为被 PR #124 取代的旧实现，保留未动；`fix/frame-stutter` worktree 有 11 个未提交文件但其分支头已并入 `main`；`openspec/changes/archive/2026-08-29-tiered-swords-combat` 与 `2026-08-30-tiered-swords-combat` 仅 `ledger.md` 不同，疑似重复归档目录。
  4. `archive/2026-08-28-placeable-torches/proposal.md` 的「延期与放弃」章节仅剩占位符（「收尾时全文誊入未决项」未兑现），属归档产物缺口，待用户决定是否补录。
  5. `F-04`（LAN 专用服务端事实同步）仍为已认领，但本机无对应分支/worktree，可能在其他开发机上，无法核对进度。
  6. 上轮遗留待澄清项（潜行、梯子、水下呼吸装备、回血计时冻结、旧区块注水迁移）继续挂起；本轮新增待澄清见上文「未落行」。上轮第 1 项（`2026-08-23-egui-tool-ui-selection-design.md` 未入库）已随其入库关闭。
  7. Discussion #71 评论流（60 条）中有 30 条更早评论本轮未翻页读取，均为实现者状态变更评论，不影响本轮结论，但下轮若做评论级对账需翻页。

## 2026-09-02（规划者第三轮）

- **读取输入**：`docs/feature-backlog.md`、`docs/notes/agent-runs.md`（上轮 2026-09-01 09:19 CST）、`docs/notes/progress.md`、根 `AGENTS.md`（版本矩阵现为协议 v32、玩家 schema v8、区块 schema v9、metadata v3、`companions.ai` v4、`hostile_mobs` v1、engine ABI v9、client ABI v14、benchmark scenario v21）、Discussion #71（`gh api graphql` 因 token 失效 + 未认证 API 限流不可用，改经公开网页读取正文与可见评论；60 条评论中最末一条为 2026-08-30 F-07，**自上轮以来无新评论**；30 条更早评论仍未翻页）、`origin/main` 近 25 提交（头 `47b29b7d`，即 PR #133 合并点；上轮遗留的两笔 docs 提交 `1dda29dd` + 运行记录提交**已由用户推送成功**）、`git branch -a`/`git worktree list` 与 16 个 worktree 头 SHA 与脏状态、上轮之后新归档的 2 个 change（`archive/2026-08-31-cozy-farming-ui-theme`、`archive/2026-09-01-webview-game-ui-unification`，均随 `ffd27129` 入库）的 `proposal.md`/`design.md`/`spike-checklist.md` 遗留横扫、`git ls-files` 来源入库校验。
- **变更行**：
  - 新增 `B-42`（潜行与潜行放置）、`B-43`（楼梯与半砖）、`B-44`（梯子）、`D-16`（容器面板与 tooltip 迁移 WebView，webview 统一 Phase 2）、`D-17`（聊天输入迁移与 Go HUD 全量退役，Phase 3）、`D-18`（滚轮切换快捷栏）。全部有已入库出处：B-42/B-43/B-44 出自 `archive/2026-08-27-sprint/design.md`、`archive/2026-08-06-m4k-authoritative-chests/proposal.md`、`docs/superpowers/specs/2026-07-27-m2b-authoritative-player-movement-design.md` §1.2 的显式非目标，D-16/D-17/D-18 出自 `archive/2026-09-01-webview-game-ui-unification/proposal.md` 的 Phase 2/3「另行立项」与 `spike-checklist.md` 末段的既有客户端能力缺口记录。B-43 为 `设计候选`（形态/光照/选取状态编码需先设计，与 B-16 同类），其余默认 `排队`。
  - 校对：`D-11` 备注补前提变化——Phase 1 已把快捷栏等常显层迁 WebView，其快捷栏半边改为前端组件面、容器产物格半边仍属 GPU HUD 保留面，需先重划范围再设计（状态维持 `设计候选`）。其余行状态与 git/worktree 一致：A 组全部已完成/已取消；B-04 维持 `就绪`（串行队首在位、无人认领，本轮不另晋升核心玩法行）；B-11 未登记在途分支头仍 `95f733f0`（2026-08-28 后零活动）；F-04 仍无本机 worktree 可核对。
- **未落行（判定）**：**待澄清**——漏斗/比较器等容器自动化取放（m4k/m4e 两处非目标，但价值相对「首夜生存+自给家园」边界及与 B-19 红石的关系待确认）；创造飞行/旁观模式（m2b 非目标，依赖游戏模式整体裁决，umbrella 无独立闭环）；「退回主菜单→再次进入游戏」装配两次未在 120s 内完成（spike-checklist 记录为应用装配行为、与 WebView 参与无关，需确认是否为可交互复现的真实缺陷）；水中冲刺（sprint 非目标，属「若要」式细化）；鞘翅互斥（依赖飞行）。**不落行**——cozy 主题 proposal 范围外的「游戏内 UI 统一 WebView」已由 webview-game-ui-unification 本体消解；上轮已对齐 MC 或已清偿项无新增。
- **提交**：`d477e6ef`（docs: plan B-42..B-44, D-16..D-18）+ 本运行记录提交。
- **推送**：**成功**——`git push origin main` 因 HTTPS 凭据不可用失败（`could not read Username for 'https://github.com'`，osxkeychain 在本会话取不到 github.com 条目、gh token 失效）；改用显式 SSH URL `git push ssh://git@github.com/channing771/mornlea.git main:main`（SSH 认证可用，未改 `origin` 配置、未强推），快进 `47b29b7d..66dbc8af`，`git fetch origin` 后 `main` 与 `origin/main` 一致。
- **讨论同步**：**未执行**——`scripts/agents/refresh-discussion.py --update` 依赖 `gh api graphql`，本轮 `gh` 的 github.com token 失效且未认证 API 撞 5000/hr 共享限流（HTTP 403），正文刷新与状态变更评论（6 条落行 + 1 条 D-11 校对）均未发出。正文已用脚本 dry-run 验证（就绪 1、排队 23、设计候选 36、已认领 1、已完成 42、已取消 4），待凭据恢复后补一次 `--update` 与汇总评论；讨论镜像现落后于仓库表（仍列 A-03 已认领、B-04 排队、缺上轮与本轮共 18 条新行）。
- **留给下一轮 / 用户**：
  1. `gh` 的 github.com token 仍失效（`gh auth status` 报 invalid），未认证 REST/GraphQL 又撞 5000/hr 共享限流——讨论正文 `--update` 与状态变更评论本轮能否发出取决于凭据，见推送/同步节。
  2. 未并入 `main` 且本机活跃过的分支：`feat/extract-companion-agent-service`（头已推进到 `09e18bdc` 2026-09-01「close task 12」）、`refactor/sim-ownership-convergence`（`b54abb9a`）、`worktree-feat-ui-changes`（`.claude/worktrees/feat-ui-changes` 头推进到 `3328af5f` 2026-09-01，含 crack 场景 regolden）、`feat/B-11-authoritative-difficulty`（`95f733f0`，零活动）；`fix/frame-stutter` worktree 仍有 11 个未提交文件（`internal/client/receiver.go`/`mirror.go` 与 `cmd/mornlea` 多个测试）。登记与处置均属控制会话/用户裁决。
  3. 上轮遗留未动项：疑似重复归档目录 `2026-08-29-tiered-swords-combat` 与 `2026-08-30-tiered-swords-combat`（仅 ledger 不同）、被 PR #124 取代的 `A-03-tiered-swords-combat` worktree（`8c9e7fe3`）、`archive/2026-08-28-placeable-torches/proposal.md` 延期与放弃章节占位符未兑现。
  4. 待澄清项继续挂起（潜行、梯子两项已随本轮出处入库落行关闭；回血计时冻结、旧区块注水迁移、水下呼吸装备/附魔、漏斗、创造/旁观模式、主菜单重入装配等仍开放）。
  5. `2026-08-31` 前后交付的 cozy 主题、dev-capture、pixel style、rust-render-world-cache、mining-crack-overlay、webview-game-ui-unification Phase 1 均为控制会话 change、无对应 backlog 行，本表不追溯补行；如需履历入表由用户裁决。

## 2026-09-27（规划者第四轮）

- **读取输入**：`docs/feature-backlog.md`、`docs/notes/agent-runs.md`（上轮 2026-09-02 第三轮）、`docs/notes/progress.md`、根 `AGENTS.md`（版本矩阵现为协议 v45、玩家 schema v9、区块 schema v9、metadata v6、`companions.ai` v5、`hostile_mobs` v2、`passive_mobs` v1、engine ABI v11、client ABI v19、benchmark scenario v23）、`openspec/config.yaml`、Discussion #71（`gh api graphql` 撞未认证共享限流 403 + token 仍失效不可用，改经 web-reader 读公开页：正文仍为首轮状态、60 条评论中 30 条隐藏项加载失败，可见评论止于 2026-08-30 F-07）、`origin/main`（`git fetch` 后头 `81a56bb8`）、`git branch -a`/`git worktree list` 及 6 个本地 worktree 头 SHA 与脏状态、上轮之后新归档的 48 个 change（`2026-09-02` 起）的 `proposal.md` 非目标/延期节横扫、`git ls-files` 来源入库校验（10 个新行来源全部已跟踪）。
- **变更行**：
  - 新增 `B-47`（弓合成配方，B-23 非目标「弓合成另立后续行」）、`B-48`（门/床潜行放置接线，B-42 Non-Goals + B-42 行待认领备注）、`B-49`（箭矢滞留与拾取，B-23 非目标）、`B-50`（敌对生物避水，water-avoidance 非目标）、`B-51`（难度伤害倍率，B-23 非目标「B-11 显式不含」）、`B-52`（草退化与蔓延光照条件，grass-spread 非目标）、`D-20`（被动牛表现打磨，cow-behavior「列为后续候选」）、`D-21`（护甲可见呈现，armor 非目标）、`E-20`（近环远环空洞带闭合，section-mesh「另行立项裁决」）、`E-21`（存档冷启动加载优先级，E-19 延期第 1 项）；全部有已入库出处，默认 `排队`，版本列只写方向性结论。
  - 校对：`B-20` 备注补有界双维已交付（PR #168，第三维/并行 tick/专用传送 packet 显式排除，本行维持设计候选）；`B-36` 备注补未认领在途分支 `codex/b36-axes-shovels`（头 `2a745374`）与 worktree 脏改动（含本表），本轮不晋升；`B-39` 备注补 B-23 骨头掉落已交付（来源半边满足、配方半边待本行）；`D-15` 备注补 E-19 视距滑块归入视距半边。其余行状态与 git 一致，无待标完成行（B-02/B-11/B-23/B-24/B-27/B-33/B-34/B-35/B-42/D-19/E-19 均已由控制会话回填为已完成）。
  - 未晋升：串行队首 B-36 前序 B-35 已完成、版本槽空闲，但 worktree 证据不一致（未认领在途分支活跃），按规则不晋升；B-37/B-38/B-39/B-40/B-44 保持排队（B-39/B-40 推进稳定编号，不得与 B-36 同时晋升）。
- **未落行（判定）**：**待澄清**——`-connect` 远程加载屏（loading 非目标「如需覆盖另立」，价值待确认）、伙伴使用弓/投射物（B-23 非目标，C 组规则要求真实玩家验证先行）、钓鱼/闪电伤害/附魔/多材质护甲/护甲修复（weather/armor 非目标的否定式提及，无正向设计出处）、数字键快速放入（B-35 非目标，需先经 D-15 键位裁决）、加载中途取消（MC 亦无，价值待确认）。**丢弃/不落行**——跨容器快捷搬运与拖拽分批/自动整理（B-35 范围冻结排除；基础拖拽已由 `inventory-drag-drop` PR #182 交付但无 backlog 行，按第三轮先例不追溯补行）、潜行耐力（MC 无此机制）、splitmix64 收敛（E-19 延期 6，纯卫生，随后续触碰面顺手收敛）、掷骨者不攻击被动牛（既有行为细节，非独立缺口）。
- **提交**：`85b9e64a`（docs: plan B-47..B-52, D-20..D-21, E-20..E-21）+ 本运行记录提交（两笔均在 `dev` 分支，见推送节）。
- **推送**：**成功**——工作区 `dev` 两笔提交内容干净且 `origin/main` 为其祖先，规划者在临时 worktree 将其 cherry-pick 到 `origin/main`（`0d438801` + `bcddb784`）后以 `ssh://git@github.com/chenyang-zz/mornlea.git HEAD:main` 快进推送 `81a56bb8..bcddb784`；`dev` 原提交 `85b9e64a`/`f8ace9da` 保留，用户改动（SKILL.md/`.commandcode/`）未触碰。
- **讨论同步**：**未执行**——正文刷新脚本依赖 `gh api graphql`（token 失效 + 未认证限流，graphql 配额 0，重置约 02:00 UTC 后仍可能因无认证而拒绝）；状态变更评论（10 条落行 + 4 条校对）均未发出。正文镜像已落后三轮（仍列 A-03 已认领、B-04 排队、缺 B-38..B-52/D-13..D-21/E-15..E-21/F-09..F-11 共 30+ 行），以仓库文件为准。
- **留给下一轮 / 用户**：
  1. `gh` 的 github.com token 仍失效（`channing771`→`chenyang-zz` 迁移后未重认证），Discussion 正文 `--update` 与评论已积压三轮，待凭据恢复后一次性补发。
  2. 2026-08-30 之后的新评论不可见（本轮仅确认正文 + 30 条旧评论），其中至少含 B-23 行备注引用的 discussioncomment-18412981；下轮优先翻页对账。
  3. `codex/b36-axes-shovels` worktree 脏改动含 `docs/feature-backlog.md`——有另一会话正在改规划表，认领登记与处置属控制会话裁决；本轮 B-36 备注仅记录观察，未触碰其分支。
  4. 无对应 backlog 行但已合入 `main` 的控制会话 change（grass-spread PR #181、inventory-drag-drop PR #182、weather/seasonal/snow、graze-lure、water-avoidance、menu-vista、section-mesh、held-items、godot-pilot P8–P14）是否追溯补履历行，待用户裁决（本轮维持第三轮先例：不补）。
  5. 遗留未动项：疑似重复归档目录 `2026-08-29/2026-08-30-tiered-swords-combat`、被取代的 `A-03-tiered-swords-combat` worktree、torch proposal 延期章节占位符、F-04 仍无本机 worktree 可核对（`lan-server.md` 陈旧被 4 个归档 setup ruling 点名）。
  6. 上轮待澄清（回血计时冻结、旧区块注水迁移、水下呼吸/附魔、漏斗、创造/旁观模式、主菜单重入装配、水中冲刺、鞘翅）继续挂起，本轮新增见上文「未落行」。

## 2026-09-29（规划者第五轮）

- **读取输入**：`docs/feature-backlog.md`、`docs/notes/agent-runs.md`（上轮 2026-09-27 第四轮；其后用户以 `9c7cd42b` 修正第四轮推送结果记录）、`docs/notes/progress.md`、根 `AGENTS.md`、`openspec/config.yaml`、Discussion #71（`gh` token 仍失效且未认证 API 撞共享限流 403，改经公开网页旧 URL `channing771/mornlea` 读取：正文仍为落后镜像、共 68 条评论、最后一条 2026-09-13（B-35 完成）——**上轮以来无新评论、无正文更新**；2026-09-02..13 评论已逐条对账，全部为已反映在表的状态变更）、`origin/main`（fetch 后头 `9c7cd42b`，上轮以来仅该用户修正提交、无功能合入）、`git branch -a`/`git worktree list` 与 14 个 worktree 的头 SHA 与脏状态、归档横扫（当前检出 `dev` 相对 `origin/main` 多 5 个归档 change：`2026-09-24-rust-storage-safety-repairs`、`2026-09-25-interface-first-parallel-development`、`2026-09-25-rust-storage-codec-closure`、`2026-09-25-target-runtime-interface-architecture`、`2026-09-27-rust-native-numerical-closure`——其中后四个为上轮扫 `main` 时不可见的补充横扫对象；逐一读 proposal/design 非目标与遗留节）。
- **变更行**：**无新增行**——五个 dev-only 归档的 Non-Goals 均为架构过渡/治理范围声明（无新玩法、无独立玩法缺口），讨论零新评论。校对（备注级、状态均不变）：`A-03`（被取代旧实现的本地 worktree `.worktrees/A-03-tiered-swords-combat` 已确认移除，分支仍在本地与远端）、`B-36`（复核 `codex/b36-axes-shovels` 头未推进仍 `2a745374`、最后提交 2026-09-15，worktree 仍脏含本表 → 连续第二轮维持排队不晋升）、`E-01`/`E-02`/`E-03`（补控制会话 Rust 迁移在途证据：`dev` 领先 `origin/main` 249 提交；活动 change `rust-authoritative-server`（51 节点待实现）、`rust-client-core`、`rust-runtime-foundation-acceptance` 与七个 `godot-*`；`cursor/rust-authoritative-server-98e6` 与 `f2/provider-*` 分支当日活跃；F1 运行时/域事件/region/协议收口已在 `main`，存储两收口与数值族契约仅在 `dev`）。无待标完成行（上轮以来 `origin/main` 无功能合入）；晋升检查：串行队首 B-36 前序已完成、版本槽空闲，但 worktree 证据不一致，按规则不晋升。
- **未落行（判定）**：MC 覆盖核对新增三条**待澄清**（讨论通道不可用，暂记于此，恢复后补挂评论）——①经验与等级系统（唯一出处是 `archive/2026-08-29-client-ui-vanilla-alignment/design.md` 非目标对「经验条」的否定式提及，机制本体无设计出处，需经验机制整体裁决）；②矿车与铁轨（MC 基础交通/物流机制，无仓库内出处）；③村民与交易（MC 中后期机制，无仓库内出处，可能超出「首夜生存+自给家园」边界）。其余 MC 基础面复核后均已有行或维持上轮判定。
- **提交**：`c99e91da`（docs: reconcile backlog evidence 2026-09-29）+ 本运行记录提交；两笔均在基于 `origin/main`（`9c7cd42b`）的临时 detached worktree `/tmp/mornlea-planner-r5` 制作（沿第四轮先例，不触碰 `dev` 工作区与其 249 个在途提交）。
- **推送**：两笔快进提交以 SSH URL `ssh://git@github.com/chenyang-zz/mornlea.git HEAD:main` 推送（HTTPS 凭据不可用沿既有先例）；推送结果以 `origin/main` 实际头为准，失败则终止不重放不强推、下轮补记。
- **讨论同步**：**未执行**——`gh` token 失效（连续第五轮）且未认证 REST/GraphQL 撞共享限流，正文 `--update` 与状态变更评论（本轮 5 条校对 + 3 条待澄清）均发不出；正文镜像已落后四轮，dry-run 现值为已认领 1、排队 26、设计候选 37、已完成 56、已取消 4、就绪 0，待凭据恢复后一次性补发。
- **留给下一轮 / 用户**：
  1. `gh` 凭据连续第五轮不可用（`gh auth status` 报 `chenyang-zz` token invalid）；Discussion 镜像与评论积压待恢复后补发。
  2. `dev` 分支（头 `365a0339`）领先 `origin/main` 249 提交，是 Rust 权威服务端/Godot 目标架构在途线（当日 `f2/provider-*` 与 `cursor/rust-authoritative-server-98e6` 仍活跃提交）；五个归档 change、`docs/interface-first-parallel-development.md` 与 `docs/runtime-interface-architecture.md` 仅存在于 `dev`，合入节奏与追溯属控制会话/用户裁决，规划者未触碰。
  3. 无 backlog 行但已在 `main` 的控制会话 change 新增两条：first-person hands（含 `feat/first-person-hands` 归档与后续 PR #177 improve-first-person-held-items）与 `remove-mining-hud-bar`；连同上轮清单是否追溯补履历行待用户裁决（维持不补先例）。
  4. `codex/b36-axes-shovels` 未认领在途分支与脏 worktree（改动含本表）仍待控制会话裁决。
  5. 遗留未动项更新：重复归档目录 `2026-08-29/2026-08-30-tiered-swords-combat` 仍在；`archive/2026-08-28-placeable-torches/proposal.md` 延期与放弃节仍为占位符；F-04 仍无本机 worktree 可核对；上轮遗留的 `A-03-tiered-swords-combat` worktree 已消失（本轮已核实并在 A-03 行更新备注）；`mornlea-f2-w25c` 的 worktree 注册指向已删除目录（`git worktree prune` 属用户裁决）；`f2/provider-2-6c` worktree 有两个未提交文件（crafting.rs 面，当日活跃线，未触碰）。
  6. 待澄清累计清单（历轮挂起 + 本轮新增三项）：回血计时冻结、旧区块注水迁移、水下呼吸装备/附魔、漏斗、创造/旁观模式、主菜单重入装配、水中冲刺、鞘翅、`-connect` 远程加载屏、伙伴用弓/投射物、钓鱼/闪电/附魔/多材质护甲/护甲修复、数字键快速放入、加载中途取消、经验与等级系统、矿车与铁轨、村民与交易。

## 2026-09-30（规划者第六轮）

- **读取输入**：`docs/feature-backlog.md`、`docs/notes/agent-runs.md`（上轮 2026-09-29 第五轮）、`docs/notes/progress.md`、根 `AGENTS.md`（版本矩阵仍为协议 v45、玩家 schema v9、区块 schema v9、metadata v6、`companions.ai` v5、`hostile_mobs` v2、`passive_mobs` v1、engine ABI v11、client ABI v19、benchmark scenario v23）、`openspec/config.yaml`、Discussion #71（`gh api graphql` 撞未认证共享限流 403；`curl` 实测新 URL `chenyang-zz/mornlea/discussions/71` 返回 Not Found，改经旧 URL `channing771/mornlea` 公开页读取：可见评论 30 条、末条仍为 2026-09-13（B-35 完成）、正文仍缺 B-47 起的新行——**上轮以来无新评论、无正文更新**）、`origin/main`（fetch 后头仍 `2f17607e`，上轮以来零新提交）、`git branch -a`（`codex/*` 与 `f2/*` 在途分支均在）/`git worktree list`（13 个 worktree）及各 worktree 头 SHA 与脏状态、归档横扫（`git log --all --since=2026-09-29` 在 `openspec/changes/archive/` 下零结果，最新归档仍为 `2026-09-27-rust-native-numerical-closure`）。
- **变更行**：**无新增行**——讨论零新评论、零新归档、MC 覆盖上轮已全量复核（本轮无变化）。校对（仅备注、状态均不变）：`B-36`（`codex/b36-axes-shovels` 头仍 `2a745374`、最后提交仍 2026-09-15，worktree 脏改动未收敛含本表 → 连续第三轮维持排队不晋升）；其余行状态与 git 一致，无待标完成行（`origin/main` 上轮以来无功能合入）。晋升检查：串行队首 B-36 前序已完成、版本槽空闲，但 worktree 证据不一致，按规则不晋升；各组尾号不变（B-52/D-21/E-21/F-11/C-11）。
- **未落行（判定）**：本轮三通道均为空，无新请求可判定；上轮三条待澄清（经验与等级系统、矿车与铁轨、村民与交易）与历轮挂起项继续挂起（讨论通道仍不可用，无法补挂评论）。
- **提交**：`ef25c733`（docs: plan B-36 recheck 2026-09-30）+ 本运行记录提交；两笔均在基于 `origin/main`（`2f17607e`）的临时 detached worktree `/tmp/mornlea-planner-r6` 制作（沿上轮先例，不触碰 `dev` 工作区与其在途提交）。
- **推送**：两笔快进提交以 SSH URL `ssh://git@github.com/chenyang-zz/mornlea.git HEAD:main` 推送（HTTPS 凭据不可用沿既有先例）；推送结果以 `origin/main` 实际头为准，失败则终止不重放不强推、下轮补记。
- **讨论同步**：**未执行**——`gh` token 失效（连续第六轮）且未认证 REST/GraphQL 撞共享限流；额外发现新仓库路径下 discussion 页返回 Not Found（旧 `channing771` 路径仍可读，迁移后讨论归属待用户确认）。正文 `--update` 与状态变更评论（本轮仅 B-36 备注复核）均发不出；正文镜像已落后五轮，以仓库文件为准。
- **留给下一轮 / 用户**：
  1. `gh` 凭据连续第六轮不可用；Discussion 镜像与评论积压待恢复后补发；另需确认讨论 #71 在新仓库路径下的可达性（本轮 `chenyang-zz/mornlea/discussions/71` 为 Not Found）。
  2. `dev` 分支领先 `origin/main` 仍 249 提交（Rust 权威服务端/Godot 目标架构在途线；本轮 `f2/provider-*`、`cursor/rust-authoritative-server-98e6`、`codex/godot-view-layer`、`codex/unified-desktop-panels` 等 worktree 均有当日左右的活跃痕迹）；合入节奏与追溯属控制会话/用户裁决，规划者未触碰。
  3. `codex/b36-axes-shovels` 未认领在途分支与脏 worktree（改动含本表与 golden）连续三轮无收敛，认领登记与处置属控制会话裁决。
  4. 无 backlog 行但已合入 `main` 的控制会话 change 是否追溯补履历行，待用户裁决（维持不补先例）。
  5. 遗留未动项：重复归档目录 `2026-08-29/2026-08-30-tiered-swords-combat`、torch proposal 延期章节占位符、F-04 仍无本机 worktree 可核对；待澄清累计清单见上轮第 6 项（本轮无新增）。

## 2026-10-01（规划者第七轮）

- **读取输入**：`docs/feature-backlog.md`、`docs/notes/agent-runs.md`（上轮 2026-09-30 第六轮）、`docs/notes/progress.md`、根 `AGENTS.md`（版本矩阵仍为协议 v45、玩家 schema v9、区块 schema v9、metadata v6、`companions.ai` v5、`hostile_mobs` v2、`passive_mobs` v1、engine ABI v11、client ABI v19、benchmark scenario v23）、`openspec/config.yaml`、Discussion #71（`gh api graphql` 撞未认证共享限流 403；改经旧 URL `channing771/mornlea` 公开页读取：可见评论止于 2026-09-13（B-35 完成），页内无 2026-09-14 及之后日期——**上轮以来无新评论、无正文更新**）、`origin/main`（fetch 后头 `ce70821f`，上轮以来仅两笔 planner 自身提交、零功能合入）、`git branch -a`（`codex/*` 与 `f2/*` 在途分支均在）/`git worktree list`（10 个 worktree，含 B-36 脏 worktree）及 B-36 worktree 头 SHA 与脏状态、归档横扫（`git log --all --since=2026-09-30 -- openspec/changes/archive/` 零结果，最新归档仍为 `2026-09-27-rust-native-numerical-closure`）。
- **变更行**：**无新增行**——讨论零新评论、零新归档、MC 覆盖上轮已全量复核（本轮无变化）。校对（仅备注、状态不变）：`B-36`（`codex/b36-axes-shovels` 头仍 `2a745374`、最后提交仍 2026-09-15，`.worktrees/codex-b36-axes-shovels` 脏改动未收敛（含本表与 golden）→ 连续第四轮维持排队不晋升）；其余行状态与 git 一致，无待标完成行（`origin/main` 上轮以来无功能合入）。晋升检查：串行队首 B-36 前序已完成、版本槽空闲，但 worktree 证据不一致，按规则不晋升；各组尾号不变（B-52/D-21/E-21/F-11/C-11）。
- **未落行（判定）**：本轮三通道均为空，无新请求可判定；上轮三条待澄清（经验与等级系统、矿车与铁轨、村民与交易）与历轮挂起项继续挂起（讨论通道仍不可用，无法补挂评论）。
- **提交**：`docs: plan B-36 recheck 2026-10-01` + 本运行记录提交；两笔均在基于 `origin/main`（`ce70821f`）的临时 detached worktree `/tmp/mornlea-planner-r7` 制作（沿上轮先例，不触碰 `dev` 工作区与其在途提交）。
- **推送**：两笔快进提交以 SSH URL `ssh://git@github.com/chenyang-zz/mornlea.git HEAD:main` 推送（HTTPS 凭据不可用沿既有先例）；推送结果以 `origin/main` 实际头为准，失败则终止不重放不强推、下轮补记。
- **讨论同步**：**未执行**——`gh` token 失效（连续第七轮）且未认证 REST/GraphQL 撞共享限流；正文 `--update` 与状态变更评论（本轮仅 B-36 备注复核）均发不出；正文镜像已落后六轮，以仓库文件为准。
- **留给下一轮 / 用户**：
  1. `gh` 凭据连续第七轮不可用；Discussion 镜像与评论积压待恢复后补发；另需确认讨论 #71 在新仓库路径下的可达性（上轮 `chenyang-zz/mornlea/discussions/71` 为 Not Found，本轮未复测）。
  2. `dev` 分支领先 `origin/main`（Rust 权威服务端/Godot 目标架构在途线；本轮 `f2/provider-*`、`codex/godot-view-layer`、`codex/unified-desktop-panels` 等 worktree 仍在）；合入节奏与追溯属控制会话/用户裁决，规划者未触碰。
  3. `codex/b36-axes-shovels` 未认领在途分支与脏 worktree（改动含本表与 golden）连续四轮无收敛，认领登记与处置属控制会话裁决。
  4. 无 backlog 行但已合入 `main` 的控制会话 change 是否追溯补履历行，待用户裁决（维持不补先例）。
  5. 遗留未动项：重复归档目录 `2026-08-29/2026-08-30-tiered-swords-combat`、torch proposal 延期章节占位符、F-04 仍无本机 worktree 可核对；待澄清累计清单见上轮第 6 项（本轮无新增）。
## 2026-10-02（规划者第八轮）

- **读取输入**：`docs/feature-backlog.md`、`docs/notes/agent-runs.md`（上轮 2026-10-01 第七轮）、`docs/notes/progress.md`、根 `AGENTS.md`（版本矩阵仍为协议 v45、玩家 schema v9、区块 schema v9、metadata v6、`companions.ai` v5、`hostile_mobs` v2、`passive_mobs` v1、engine ABI v11、client ABI v19、benchmark scenario v23）、`openspec/config.yaml`、Discussion #71（`gh api graphql` 撞未认证共享限流 403；改经旧 URL `channing771/mornlea` 公开页读取：可见评论仍 68 条、日期止于 2026-09-13（B-35 完成），页内无 2026-09-14 及之后日期——**上轮以来无新评论、无正文更新**）、`origin/main`（fetch 后头 `5b7b8edf1`，上轮以来仅两笔 planner 自身提交、零功能合入）、`git branch -a`/`git worktree list`（12 个 worktree）及各 worktree 头 SHA 与脏状态、归档横扫（`git log --all --since=2026-10-01 -- openspec/changes/archive/` 零结果，最新归档仍为 `2026-09-27-rust-native-numerical-closure`）。
- **变更行**：**无新增行**——讨论零新评论、零新归档、MC 覆盖上轮已全量复核（本轮无变化）。校对（仅备注、状态不变）：`B-36`（`codex/b36-axes-shovels` 头仍 `2a745374`、最后提交仍 2026-09-15，`.worktrees/codex-b36-axes-shovels` 脏改动未收敛（含本表与 golden）→ 连续第五轮维持排队不晋升）；另核实 `B-04` 已完成为旧提交 `114003bcc` 所标（非本轮漏标）；其余行状态与 git 一致，无待标完成行（`origin/main` 上轮以来无功能合入）。晋升检查：串行队首 B-36 前序已完成、版本槽空闲，但 worktree 证据不一致，按规则不晋升；各组尾号不变（B-52/D-21/E-21/F-11/C-11）。
- **未落行（判定）**：本轮三通道均为空，无新请求可判定；上轮三条待澄清（经验与等级系统、矿车与铁轨、村民与交易）与历轮挂起项继续挂起（讨论通道仍不可用，无法补挂评论）。
- **提交**：`docs: plan B-36 recheck 2026-10-02` + 本运行记录提交；两笔均在基于 `origin/main`（`5b7b8edf1`）的临时 detached worktree `/tmp/mornlea-planner-r8` 制作（沿上轮先例，不触碰 `dev` 工作区与其在途提交）。
- **推送**：两笔快进提交以 SSH URL `ssh://git@github.com/chenyang-zz/mornlea.git HEAD:main` 推送（HTTPS 凭据不可用沿既有先例）；推送结果以 `origin/main` 实际头为准，失败则终止不重放不强推、下轮补记。
- **讨论同步**：**未执行**——`gh` token 失效（连续第八轮）且未认证 REST/GraphQL 撞共享限流；正文 `--update` 与状态变更评论（本轮仅 B-36 备注复核）均发不出；正文镜像已落后七轮，以仓库文件为准。
- **留给下一轮 / 用户**：
  1. `gh` 凭据连续第八轮不可用；Discussion 镜像与评论积压待恢复后补发；讨论 #71 在新仓库路径下的可达性仍待确认（上上轮 `chenyang-zz/mornlea/discussions/71` 为 Not Found）。
  2. `dev` 分支领先 `origin/main` 仍 249 提交（Rust 权威服务端/Godot 目标架构在途线；本轮新增 worktree `agents/deerflow-code-alignment-rules`、`codex/godot-view-layer`、`codex/unified-desktop-panels`、`feat/rust-client-core`、`feat/agent-dev`、`feat/first-person-hands` 均有未提交改动）；合入节奏与追溯属控制会话/用户裁决，规划者未触碰。
  3. `codex/b36-axes-shovels` 未认领在途分支与脏 worktree（改动含本表与 golden）连续五轮无收敛，认领登记与处置属控制会话裁决。
  4. 无 backlog 行但已合入 `main` 的控制会话 change 是否追溯补履历行，待用户裁决（维持不补先例）。
  5. 遗留未动项：重复归档目录 `2026-08-29/2026-08-30-tiered-swords-combat`、torch proposal 延期章节占位符、F-04 仍无本机 worktree 可核对；待澄清累计清单见上轮第 6 项（本轮无新增）。

## 2026-10-03（规划者第九轮）

- **读取输入**：`docs/feature-backlog.md`、`docs/notes/agent-runs.md`（上轮 2026-10-02 第八轮）、`docs/notes/progress.md`、根 `AGENTS.md`（版本矩阵仍为协议 v45、玩家 schema v9、区块 schema v9、metadata v6、`companions.ai` v5、`hostile_mobs` v2、`passive_mobs` v1、engine ABI v11、client ABI v19、benchmark scenario v23）、`openspec/config.yaml`、Discussion #71（`gh api graphql` 撞未认证共享限流 403；改经旧 URL `channing771/mornlea` 公开页读取：可见日期止于 2026-09-13（B-35 完成），正文仍缺 B-47 起新行——**上轮以来无新评论、无正文更新**）、`origin/main`（HTTPS fetch 失败：HTTP2 framing 层错误与 Empty reply；经 SSH 取远端头：上轮以来仅两笔 planner 自身提交 `cfca1080`/`e9b5ea60`、零功能合入）、`git branch -a`/`git worktree list`（21 个 worktree，含 B-36 脏 worktree）及 B-36 worktree 头 SHA 与脏状态、归档横扫（`git log --all --since=2026-10-02 -- openspec/changes/archive/` 零结果，最新归档仍为 `2026-09-27-rust-native-numerical-closure`）。
- **变更行**：**无新增行**——讨论零新评论、零新归档、MC 覆盖此前已全量复核（本轮无变化）。校对（仅备注、状态不变）：`B-36`（`codex/b36-axes-shovels` 头仍 `2a745374`、最后提交仍 2026-09-15，`.worktrees/codex-b36-axes-shovels` 脏改动未收敛（含本表与 golden）→ 连续第六轮维持排队不晋升）；其余行状态与 git 一致，无待标完成行（`origin/main` 上轮以来无功能合入）。晋升检查：串行队首 B-36 前序已完成、版本槽空闲，但 worktree 证据不一致，按规则不晋升；各组尾号不变（B-52/D-21/E-21/F-11/C-11）。
- **未落行（判定）**：本轮三通道均为空，无新请求可判定；待澄清累计清单继续挂起（讨论通道仍不可用，无法补挂评论）。
- **提交**：两笔均在临时 detached worktree（`/tmp/mornlea-planner-r8`，沿上轮先例，不触碰 `dev` 工作区与其在途提交）制作。首轮推送因远端已前进被拒（另一规划者会话的 10-02 第八轮 `cfca1080`/`e9b5ea60` 先合入）——按规则不强推，将本轮复核重放于新头 `e9b5ea60` 之上：`docs: plan B-36 recheck 2026-10-03` + 本运行记录提交。
- **推送**：（重放后）两笔快进提交以 SSH URL `ssh://git@github.com/chenyang-zz/mornlea.git HEAD:main` 推送（HTTPS 凭据不可用沿既有先例）；推送结果以 `origin/main` 实际头为准。
- **讨论同步**：**未执行**——`gh` token 失效（连续第九轮）且未认证 REST/GraphQL 撞共享限流；正文 `--update` 与状态变更评论（本轮仅 B-36 备注复核）均发不出；正文镜像已落后八轮，以仓库文件为准。
- **留给下一轮 / 用户**：
  1. `gh` 凭据连续第九轮不可用；Discussion 镜像与评论积压待恢复后补发；`chenyang-zz/mornlea/discussions/71` 可达性仍待确认。
  2. 每日规划轮次疑似在两个会话并行运行（本轮与 10-02 第八轮同日窗口不知对方存在，均建了同名 `/tmp/mornlea-planner-r8` 临时 worktree 且都只做 B-36 复核）——若为定时调度重复投递，建议用户检查调度配置，避免同轮重复提交。
  3. `codex/b36-axes-shovels` 未认领在途分支与脏 worktree（改动含本表与 golden）连续六轮无收敛，认领登记与处置属控制会话裁决。
  4. 无 backlog 行但已合入 `main` 的控制会话 change 是否追溯补履历行，待用户裁决（维持不补先例）。
  5. 遗留未动项：重复归档目录 `2026-08-29/2026-08-30-tiered-swords-combat`、torch proposal 延期章节占位符、F-04 仍无本机 worktree 可核对；待澄清累计清单见 09-29 轮第 6 项（本轮无新增）。

## 2026-10-04（规划者第十轮）

- **读取输入**：`docs/feature-backlog.md`、`docs/notes/agent-runs.md`（上轮 2026-10-03 第九轮）、`docs/notes/progress.md`、根 `AGENTS.md`（版本矩阵仍为协议 v45、玩家 schema v9、区块 schema v9、metadata v6、`companions.ai` v5、`hostile_mobs` v2、`passive_mobs` v1、engine ABI v11、client ABI v19、benchmark scenario v23）、`openspec/config.yaml`、Discussion #71（`gh api graphql` 未认证撞共享限流 403，本轮不再重试；改经旧 URL `channing771/mornlea` 公开页读取：可见评论止于 2026-09-13（B-35 完成），正文仍缺 B-47 起新行——**上轮以来无新评论、无正文更新**）、`origin/main`（上轮以来仅两笔 planner 自身提交 `82f4d073`/`689366c0`、零功能合入）、`git branch -a`/`git worktree list`（21 个 worktree，含 B-36 脏 worktree）及 B-36 worktree 头 SHA 与脏状态、归档横扫（`git log origin/main --since='2026-10-03' -- openspec/changes/archive/` 零结果，最新归档仍为 `2026-09-27-rust-native-numerical-closure`）。
- **变更行**：**无新增行**——讨论零新评论、零新归档、MC 覆盖此前已全量复核（本轮无变化）。校对（仅备注、状态不变）：`B-36`（`codex/b36-axes-shovels` 头仍 `2a745374`、最后提交仍 2026-09-15，`.worktrees/codex-b36-axes-shovels` 脏改动未收敛（含本表与 golden）→ 连续第七轮维持排队不晋升）；其余行状态与 git 一致，无待标完成行（`origin/main` 上轮以来无功能合入）。晋升检查：串行队首 B-36 前序已完成、版本槽空闲，但 worktree 证据不一致，按规则不晋升；各组尾号不变（B-52/D-21/E-21/F-11/C-11）。
- **未落行（判定）**：本轮三通道均为空，无新请求可判定；待澄清累计清单继续挂起（讨论通道仍不可用，无法补挂评论）。
- **提交**：两笔均在临时 detached worktree（`/tmp/mornlea-planner-20261004`，独立命名避免与并行会话的 `r8` 同名冲突，不触碰 `dev` 工作区与其在途提交）制作：`docs: plan B-36 recheck 2026-10-04` + 本运行记录提交。
- **推送**：**成功**——首笔（backlog）先行快进推送 `689366c02..1d593daa4` 验证通道，通道正常后本笔随即同路推送；两笔均以 SSH URL `ssh://git@github.com/chenyang-zz/mornlea.git HEAD:main` 快进（HTTPS 凭据不可用沿既有先例），推送结果以 `origin/main` 实际头为准。
- **讨论同步**：**未执行**——`gh` 未认证撞共享限流（连续第十轮，本轮按配额提醒不再重试）；正文 `--update` 与状态变更评论（本轮仅 B-36 备注复核）均发不出；正文镜像已落后九轮，以仓库文件为准。
- **留给下一轮 / 用户**：
  1. `gh` 凭据连续第十轮不可用；Discussion 镜像与评论积压待恢复后补发；`chenyang-zz/mornlea/discussions/71` 可达性仍待确认。
  2. 上轮记录的「双会话并行」疑问本轮未复现（本轮改用独立 worktree 名）；若定时调度仍重复投递，用户可检查调度配置。
  3. `codex/b36-axes-shovels` 未认领在途分支与脏 worktree（改动含本表与 golden）连续七轮无收敛，认领登记与处置属控制会话裁决。
  4. 无 backlog 行但已合入 `main` 的控制会话 change 是否追溯补履历行，待用户裁决（维持不补先例）。
  5. 遗留未动项：重复归档目录 `2026-08-29/2026-08-30-tiered-swords-combat`、torch proposal 延期章节占位符、F-04 仍无本机 worktree 可核对；待澄清累计清单见 09-29 轮第 6 项（本轮无新增）。

