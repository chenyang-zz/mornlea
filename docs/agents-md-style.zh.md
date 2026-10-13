---
doc_id: agent-guidance-style
doc_revision: 2026-09-19.1
language: zh-CN
counterpart: agents-md-style.md
---
# AGENTS.md 内容呈现规范

本文件是各目录 `AGENTS.md`/`CLAUDE.md` 的编写与呈现细则：节结构、内容判据、
自查清单都在这里。条文是原则（见 `docs/AGENTS.md`「编写规则」），本文件是
可执行的步骤；两者冲突时以 `docs/AGENTS.md` 为准。首个完整先例是
`cmd/mornlea` 子树（2026-08：总纲 + `app/`、`capture/`、`benchmark/` 三份
子包文档），结构性参照是 deer-flow 仓库 `backend/packages/harness` 的分层
指南（历史证据，`docs/superpowers/` 有对应规划记录）。

## 作用域与分层

- 规则住**最近作用域**：约束单个包的内容放该包目录的 `AGENTS.md`；跨包的
  地图、方向与入口差异放子树根的总纲。祖先链叠加生效，子文件只补充、不
  复述父文件已有的条文。
- 重要目录包括仓库根、顶层模块或包，以及拥有独立所有权、依赖、生命周期或
  验证边界，或协调多个包、入口、资产类别的子树根。每个重要目录都应在所辖
  内容旁有一份精简 `AGENTS.md`，说明目录用途、目录地图、边界、入口和定点
  验证。
- 新建、重组或实质性改变重要目录职责时，必须在同一变更中新建或更新该目录
  的 `AGENTS.md`。如果没有独立不变量，就继承父级，不要为了对称性新增指南。
- 每份指南都应绑定到可判定、作用域特定的不变量，不要把它写成通用教程。
- 薄导入 `CLAUDE.md` 只出现在仓库根与子树根（如 `cmd/mornlea/CLAUDE.md`），
  内容逐字节一致并登记进 `packages/audit` 的 `claudeImportDocs`
  （`TestClaudeImportsAgentGuidance` 把关）；修改只落在 `AGENTS.md`，不把
  正文写进 `CLAUDE.md`。**子包目录不放 `CLAUDE.md`**：代理沿目录祖先链读到
  子树总纲与父级指南即可，嵌套副本只会制造同步负担。

## 总纲文档骨架（子树根）

依次包含以下节，节名可按域调整但职责不变：

1. **开头作用域段**：一句话声明"本文件是 `<子树>` 的总纲"，指明子包指南
   的去向与本子树 `CLAUDE.md` 的薄导入分工（范例：
   `cmd/mornlea/AGENTS.md` 开头段）。
2. **Directory Map**：ASCII 目录树，每行以 `#` 单行角色注释；子包行注明
   "指南见 `<pkg>/AGENTS.md`"；资产目录（如 `testdata/golden/`）标注来源
   或去向。树与实际文件清单一致——写前 `ls` 核对。
3. **Dependency Direction**：允许边与禁止边成对列出；每条方向写明强制它的
   测试名（archcheck 断言），以及新增包的登记点（如
   `clientCommandAllowedEdges`）。
4. **入口/模式差异**（按域适用）：表格呈现各模式触发条件与装配行为差异；
   模式无关的共享边界以条目列出，并注明实现所在的子包。
5. **Documentation Sync Policy**：改什么必须同步什么；明确"根文档不复制
   子包细节"。
6. **Focused Verification**：按改动域的定点命令表。命令与分层纪律引用
   `docs/notes/test-quickstart.md`，不复制分层条文。

## 子包文档骨架

1. **开头作用域段**：本包做什么（一句话）、行为规格在哪个 openspec 主规格、
   使用纪律在哪个 `docs/notes/` 文档（如适用）、依赖方向一句 + 强制测试名。
2. **不变量节**：每节一个主题，节标题内嵌精确文件路径，格式
   `## 主题 (\`<pkg>/file.go\`, \`<pkg>/file2.go\`)`。每条不变量必须可判定：
   写清楚**约束内容 + 存在依据 + 强制点**（强制点 = 具体测试函数名或
   archcheck 断言名；没有强制点的约束要说明靠什么兜底，如评审或门禁步骤）。
3. **helper 中心与回归测试节**：指出本包唯一 `*_helpers_test.go`，列出
   钉死回归的测试入口——真实测试函数名 + 括号内一句话说明被证性质。
4. **Focused Verification**：本包定点命令与相关 make 目标。

## 内容判据（写什么、怎么写）

- **写不变量，不写教程**：不介绍"这是什么技术"、不写入门叙事；只写会被
  违反的约束与判断依据。
- **可判定**：每条约束都能对照代码或测试核实。"保持高效""注意性能"一类
  空泛表述不合格；"基线缺失时不静默创建，必须显式请求更新"合格。
- **不复制会漂移的值**：分辨率、容量、阈值、场景清单、超时预算一律以代码
  常量与其性质测试为准，指南只点名常量与测试，不抄数值。
- **只陈述当前事实**：change 叙事、演进过程、被否决方案不入指南（那些住
  OpenSpec 产物与 `docs/superpowers/`）。
- **精确路径**：节标题与正文点名的每个文件、每个测试函数、每条命令，写前
  逐一核实存在。反引号内标识符必须真实存在（`.go` 注释有
  `TestCommentBacktickIdentifiersExist` 兜底，文档内引用需人工执行同样
  标准）。
- **语言**：`AGENTS.md` 属机器治理文件，规范正文统一使用英文；标识符、命令和路径保留精确原文。
- **数字加实测限定**：计数、耗时等易过期数字必须带"YYYY-MM 实测"式限定，
  或改以"以代码/测试为准"表述。
- **无任务编号**：不出现 `[A-F]-[0-9]{2}` 形式的任务标识，溯源用 change 名
  或 openspec 产物。

## 重复控制

- 同一段约束只在一处权威陈述：跨包边界住总纲，子包文档用指针引用；子包
  细节不回写总纲。导航入口只链接权威正文，不复制说明。
- 与父级 `AGENTS.md` 重复的条文（即使措辞不同）是漂移温床——发现即改为
  指针或删除。

## 自查清单（新建或修改指南文档后）

- [ ] 节结构符合上述骨架；节标题内嵌的路径全部真实存在
- [ ] 点名的测试函数经 `go test <pkg> -list` 或 grep 核实存在；命令逐条
      在仓库根可执行
- [ ] 数字要么有实测限定，要么已改为"以代码/测试为准"
- [ ] 与父级/子级指南无重复条文；跨包约束只在总纲出现一次
- [ ] `CLAUDE.md` 只存在于仓库根与子树根且逐字节等于模板、已登记
      `claudeImportDocs`；子包目录无 `CLAUDE.md`
- [ ] `make archcheck` 全绿（薄导入、反引号、依赖
      方向守卫）
- [ ] 无任务编号；规范正文使用英文，标识符保持精确拼写

## 范例

- 子树总纲：`cmd/mornlea/AGENTS.md`（Directory Map / Dependency Direction /
  Entry Modes / Documentation Sync Policy / Focused Verification 五节）。
- 子包文档：`cmd/mornlea/capture/AGENTS.md`（golden 纪律、场景表、消费端
  接口、helper 中心与回归测试、Focused Verification）；`app/`、
  `benchmark/` 同式。
- 消费端接口纪律、测试装配入口护栏等跨包约束的写法，见
  `cmd/mornlea/app/AGENTS.md` 的导出面纪律与 `testkit.go` 节。
