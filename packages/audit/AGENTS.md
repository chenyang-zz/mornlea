# 审计单元指南（packages/audit）

## 作用域

本文件补充仓库根 `AGENTS.md`，适用于 `packages/audit`（Go 包名保持
`archcheck`，模块 `github.com/channing771/mornlea/packages/audit`）。本单元是
仓库的架构门禁测试集：依赖方向、单元 require 边界、身份扫描与基线版本一致
性全部住在这里。原 `internal/` 过渡指南已随根模块解散并入本文件。

## 单元边界

- 审计单元是跨模块的枚举校验者，只以 `go list`、读文件与源码 AST 的方式观察
  被审单元；本单元 MUST NOT require 或 import shared、server、client、tools、
  contracts 的任何包——住进任何依赖边都会失去审计资格（由
  `unit_boundary_test.go` 的单元 require 表强制，audit 是叶子单元）。
- 包级依赖方向的唯一真相是本包 `dependency_test.go` 的 `allowed` 表；文档与
  其他指南不得复制该表。新增包或依赖边必须先证明方向合理并同步架构门禁。
- 枚举一律经 `workspaceModules`（解析根 `go.work` 的 use 列表）跨模块执行：
  根模块已解散，仓库根的 `./...` 不可用且模式本就不跨嵌套模块；新单元模块
  立项必须同时登记 go.work use，否则 `TestWorkspaceUseSetMatchesUnitModules`
  与各枚举检查报红。

## 定点验证

- 依赖边界与全部架构守卫：`go test ./packages/audit -count=1`。
- `packages/tools/cmd/runtime-oracle` production sources stay stdlib-only; test
  files may import `packages/server/storage/storagedef` for save error-class
  checks. That import is whitelisted only in `dependency_test.go`
  `oracleAllowedTestImports` and is enforced by
  `TestRuntimeOracleInternalDependencies`.
