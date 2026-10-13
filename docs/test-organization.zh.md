---
doc_id: test-organization
doc_revision: 2026-09-16.1
language: zh-CN
counterpart: test-organization.md
---
# 测试文件编排规范（细则）

本文件是 `AGENTS.md`「实现约定」中两条测试编排条文的操作细则：判定判据、拆分步骤、
helper 落位规则与验收清单都在这里。条文是原则，本文件是可执行的步骤。

## 原则与边界

- **零行为变化**：测试文件的拆分与 helper 迁移是纯重组。测试函数名、子测试
  （`t.Run` 标签）是测试入口，一律不改——这同时是 openspec 主规格
  `repository-code-organization` 的要求（重构不改测试入口，`-run` 过滤器保持兼容）。
- 测试与被测代码**同目录**（`go test` 只认同目录 `_test.go`，白盒断言依赖同包），
  不做集中式 `tests/` 镜像目录。
- 单包的测试文件重组按本文件直接执行；跨多包的批量推广是跨组件改动，走 OpenSpec
  change。

## 目录与包形态

- 新测试文件跟随包内既有形态：包内以白盒同包为主就写同包，以 `foo_test` 外部包为
  主就写外部包。既有分布：`packages/shared/core`、`packages/shared/physics`、`packages/shared/world`、
  `packages/client/mesh` 偏外部黑盒；`packages/server/server`、`packages/server/sim/...`（`contract`/`realm`/`entity`/`runtime` 四子包）偏白盒——白盒断言与所属私有状态同包。
- 白盒与外部包不得为了重组而互换——那是行为可见性变化，超出纯重组范围。

## 测试文件命名

- 基本式 `<主题>_test.go`，主题名取自**被测源文件名**（`rules.go` →
  `rules_test.go`）或**被证性质**。
- 前缀与后缀语义（文件名纯组织性，`-run`/`-list` 只认函数名）：

| 名字段 | 语义 | 例 |
|---|---|---|
| `property_` 前缀 | 被证性质（决策验证关口性质测试集） | `property_rescan_test.go` |
| `_integration` 后缀 | 跨组件集成 | `farming_integration_test.go` |
| `_restart` 后缀 | 重启/持久化恢复 | `drop_restart_test.go` |
| `_e2e` 后缀 | 端到端 | `hunger_loop_e2e_test.go` |
| `_parity` 后缀 | 双实现对照（Memory/TCP、Go/Rust oracle） | `parity_test.go` |
| `_oracle` 后缀 | 以测试 oracle 为基准 | `oracle_test.go` |
| `_fuzz` 后缀 | Fuzz 入口 | `codec_fuzz_test.go` |
| `_bench`/`_benchmark`/`_perf` 后缀 | Benchmark 入口 | `bench_test.go` |
| `_external` 后缀 | `foo_test` 外部包专属视角 | `attached_external_test.go` |
| `_golden` 后缀 | golden 逐字节断言 | `codec_golden_test.go` |

- 前缀表「被证性质」、后缀表「测试种类」，语义不同**不得叠加**（不写
  `property_x_integration_test.go`）。

## 何时拆分（判据）

- **主题混装是唯一硬判据**：一个测试文件装了 ≥2 个不相干主题（判定方法：`grep
  '^func Test' <file>` 后按函数名前缀聚类——前缀分属不同功能域即混装；或测试对应
  的被测源文件多于一个且无性质关联），就是拆分候选。
- 行数只是检视信号，不是判据：文件超过约 800 行时检查是否混装；**单主题大文件
  不拆**（如 `packages/server/sim/entity/mining_test.go` 千余行全部是采掘，保持原样；`packages/server/sim/runtime` 与 `packages/server/sim/realm` 同理）。
- 拆分时机：该文件因任何改动被触碰时，顺势拆分；不为拆而拆地发起独立任务（批量
  推广走 OpenSpec）。

## 拆分操作步骤（SOP）

1. **快照**：`go test ./<pkg> -list '.*' | sort > /tmp/<pkg>-before.txt`。
2. **helper 落位判定**：对文件内每个非 `Test`/`Benchmark`/`Fuzz` 的声明，grep 其
   引用文件集合（`grep -ln '<名字>' <pkg>/*_test.go`）；被多于一个测试文件引用 →
   迁入 helper 中心；单文件私有 → 随其唯一消费者走。
3. **建目标文件并逐字搬运**：按上文命名规范建文件，测试函数连同其注释、专属
   类型与常量整体搬运；不合并、不改写、不重排语句。
4. **自指注释修复**：搬运后注释里「本文件」指称失真的，把指称改为点名文件（如
   「本文件用它当 oracle」→「queue_bounded_test.go 用它当 oracle」）。这是唯一
   允许的注释改写之一。
5. **文件头注释**：每个新文件写 English header comments，说明本文件职责（被证性质/主题、属哪
   组测试集）；被拆文件的原头说明按内容分摊到各新文件头——**任何一句不得整体
   消失**。
6. **import 最小化**：每个新文件只保留自己用到的 import。
7. **验收**：执行下节清单。
8. **提交**：单聚焦 commit，`test(<pkg>): ...` 前缀，说明拆分映射。

## helper 中心规则

- 每包**最多一个**共享 helper 中心：优先扩展包内既有 `*_helpers_test.go`；没有才
  新建 `helpers_test.go`；不得另立并行中心（如客户端三个子包各有自己的中心：
  `packages/client/cmd/mornlea/app/app_test_helpers_test.go`、
  `packages/client/cmd/mornlea/capture/capture_test_helpers_test.go`、
  `packages/client/cmd/mornlea/benchmark/benchmark_helpers_test.go`，各自扩展，不要再建）。
- 纯 helper 文件（不含任何 `Test`/`Benchmark`/`Fuzz` 函数）必须以
  `*_helpers_test.go` 命名，不得顶着普通测试文件名。
- 中心体量上限：超过约 500 行、或横跨两个以上不相干域（如测试替身 + 地形夹具 +
  白盒断言三域并存）时，按域再拆（如 `fixtures_test.go`）。
- helper comments must use English and explain their purpose；跨文件准入的 helper 注明它的消费者范围。

## 验收清单（零行为变化）

- [ ] 测试函数名逐一不变（`diff` 改造前后两份 `-list` 快照，排序后**集合一致**
      即可——声明顺序随文件拆分必然改变，这是预期行为不是回归）；
- [ ] 子测试标签（`t.Run` 的首个参数）逐一不变；
- [ ] 生产文件、`testdata/`、golden 零改动（`git diff --stat` 只含 `*_test.go`）；
- [ ] 注释零信息损失（被拆文件头说明的每句话都在某个新文件里有落点）；
- [ ] `gofmt -l <pkg>` 无输出、`go vet ./<pkg>`、`go test ./<pkg> -race -count=1`；
- [ ] 改动 Go 文件后 `make archcheck`。

## 范例

`packages/server/fluid`（2026-08 首个试点，时居 `internal/fluid`，commit `fa12c56`）：

| 改造前 | 改造后 |
|---|---|
| `property_test.go`（805 行，混 340 行共享工具 + 四条性质） | `property_rescan_test.go`（168）、`property_budget_test.go`（66）、`property_order_test.go`（162）、`property_converge_test.go`（148） |
| `memworld_test.go`（33 行纯 helper，无测试函数） | 并入 `helpers_test.go`（393 行） |
| `queue_bounded_test.go` 顶部跨文件 helper | `sortItems`/`queuedDueTick` 入 `helpers_test.go`；`boundedPos` 单文件私有，留原处 |

客户端命令分包（2026-08，openspec change `split-client-subpackages`）：
`cmd/mornlea` 单包（89 个 Go 文件）按功能域 `git mv` 迁移为薄 main 加 `app/`、
`capture/`、`benchmark/` 三个子包；全程测试函数名与 `t.Run` 标签逐一不变，
三个子包 `go test -list` 入口并集与迁移前单包集合一致；helper 中心按「每包
一个」落位（见上文 helper 中心规则），跨包白盒装配收敛为
`packages/client/cmd/mornlea/app/testkit.go` 的导出测试装配入口；golden 资产随 capture 域
`git mv` 至 `cmd/mornlea/capture/testdata/golden`（现居仓库根 `testdata/visual-golden/world`），子包依赖方向由
`packages/audit` 的 `TestClientCommandSubpackageDependencyDirections` 强制。

混装识别范例：`cmd/mornlea` 单包时期的 `app_input_test.go`（约 1300 行、38 个测试，前缀横跨输入
预测门控、挖掘 overlay、快捷栏放置、熔炉 UI、箱子 UI、合成、丢弃/进食/使用键七个
功能域）已于 2026-08 按本文件拆为 `app_input_prediction_test.go`、`app_mining_overlay_test.go`、
`app_hotbar_placement_test.go`、`app_furnace_ui_test.go`、`app_chest_ui_test.go`、
`app_inventory_crafting_test.go` 与 `app_use_key_test.go`（客户端分包后均居
`cmd/mornlea/app/`，现 `packages/client/cmd/mornlea/app/`），共享消息/镜像助手迁入同包的 `app_test_helpers_test.go`
（此例为识别示范；新候选以判据为准）。

## Rust 映射

上文判据、SOP 与验收清单是 Go 侧表述；Rust crate（`packages/engine/crates/mornlea_engine`
与 `packages/engine/crates/mornlea_client`）按本节映射执行，原则同构：零行为变化、按主题
拆分、helper 先 grep 引用集合再落位。

### 存放与拆分形态

- 测试以 `#[cfg(test)]` 子模块与被测代码**同 crate 同模块树**存放（内联
  `mod tests` 或兄弟文件），不做 `tests/` 集成目录——与 Go 侧「不做集中式
  `tests/` 镜像目录」同构；现状如此，保持。
- 混主题的巨型内联 `mod tests` 按主题拆成**兄弟文件**：源文件目录化（如
  `greedy.rs` → `greedy/mod.rs`）后在模块根挂载 `#[cfg(test)] mod
  <主题>_tests;`，测试函数连同其 doc 注释、专属常量逐字搬入，`use` 按主题裁剪
  到最小（clippy `-D warnings` 下 unused_imports 会红）。范例：engine
  `packages/engine/crates/mornlea_engine/src/greedy/`（2026-08 Rust 首个试点，commit
  `b2a6edb`）；client `packages/engine/crates/mornlea_client/src/render/water_tests.rs`
  （既有先例，与 `render/plant_tests.rs` 同式，都在 `render/mod.rs` 以
  `#[cfg(test)] mod …;` 挂载）。
- 挂载布局惯例：`#[cfg(test)] mod …;` 声明集中为 `mod.rs` 尾部一个块
  （`greedy/mod.rs` 尾部先例）；client `render/mod.rs` 里挂载与生产 `pub mod`
  按字母序混排是历史形态，新拆分不采用。

### helper 中心

- 跟随既有先例，不另立并行中心；两种粒度：
  - crate 级共享用 `#[cfg(test)] pub(crate) mod tests`（engine crate 的
    `src/input.rs`；`light.rs` 与 `ffi.rs` 的测试经
    `crate::input::tests::valid_input`、`crate::input::tests::ENTRY_BYTES`
    具名导入复用其夹具）；
  - 模块内多测试文件共享用 `test_support.rs`（engine crate 的
    `src/greedy/test_support.rs`，挂载为 `#[cfg(test)] mod test_support;`，
    主题文件经 `use super::test_support::{…}` 具名导入引用）。
- 落位前同样先 grep 引用集合：被多于一个测试模块引用的 helper 才迁入中心，
  单模块私有的留在消费它的测试文件内。
- 中心体量上限与 Go 侧同判据：超过约 500 行、或横跨两个以上不相干域时按域
  再拆；现状 engine `src/greedy/test_support.rs` 63 行、`src/input.rs` 的
  `tests` 模块约 190 行，均远未触及。
- helper doc comments（`///`）must use English and explain their purpose；跨模块准入的注明消费者范围。

### 验收清单（Rust 等价物）

- [ ] `#[test]` 函数名逐字不变（Go 侧「子测试标签不变」在 Rust 无对应物）；
- [ ] `cargo test -p <crate> -- --list` 的**裸函数名集合**改造前后一致——模块
      路径前缀随兄弟文件挂载必然改变，属预期，与 Go 侧 `-list` 的集合语义
      同构；
- [ ] `cargo fmt --check`、`cargo clippy --workspace --all-targets --
      -D warnings` 干净，crate 测试 `cargo test -p <crate> --locked` 全绿
      （仓库根 `make rust-check` 三项一并覆盖）；
- [ ] cdylib 出口不变：仓库根 `make rust`。

裸名排序快照提取（在 `packages/engine/` 下执行，`rust-toolchain.toml` 已钉 1.97.1；
`grep` 滤掉汇总行，`sed` 截 `: test` 后缀，`awk` 取 `::` 末段）：

```bash
cargo test -p mornlea_engine -- --list \
  | grep ': test$' | sed 's/: test$//' | awk -F'::' '{print $NF}' | sort
```

### 注意事项

- 锁 C ABI 契约的测试模块不按主题重排：engine crate `src/ffi.rs` 的
  `#[cfg(test)] mod tests` 是 engine C ABI 的契约回归面，排布对照 ABI 出口而
  非功能主题，本节的按主题拆分 SOP 对它不适用，保持原状。
