package archcheck_test

import (
	"fmt"
	"go/parser"
	"go/token"
	"io/fs"
	"maps"
	"os"
	"path/filepath"
	"slices"
	"strings"
	"testing"
)

// allowed 列出每个内部包允许直接依赖的内部包。
//
// packages/server/server → packages/shared/physics：伙伴任务编排经 CompanionAction 提交移动输入，
// 该 API 的字段类型就是 physics.Input；发令者与伙伴交互几何从 tick 入口捕获的
// runtime.TickTunables 显式消费 physics 值，不在 server 内重读活动快照。该方向是
// 编排层依赖领域值类型，与 sim→physics 同向，不引入反向耦合。
//
// packages/server/server → packages/contracts/companion-agent/mcp-v1：MCP listener 在组件
// 构造时直接嵌入并解析跨语言 machine contract，避免另写一份工具 schema；contracts
// 包只暴露独立字节副本且不反向依赖任何生产包。
//
// packages/server/sim/realm → packages/server/fluid：流动的调度
// 算法（待更新队列、全序排序、封闭盆地上的不动点收敛）被拆成独立包，使其能脱离权威
// 引擎被穷举性质测试；流动的数据所有权仍在 realm，fluid 只暴露
// Advance(now, FluidWorld, budget) 这样的无状态纯函数入口，不持有世界。
// packages/server/fluid → packages/shared/nativeabi（变更 rust-engine-fluid）：Advance 的单格
// 流体规则求值经 FluidEvalBatch 批量送入 Rust engine kernel（eval_native.go 是
// 包内唯一调用点），Go 侧保留全部编排（调度、预算、冲突合并、排序提交），kernel
// 只做逐项无状态纯函数求值。fluid 除此之外只允许依赖 packages/shared/core（当前实现未
// 依赖 packages/shared/world，即便设计意图允许，也不预先登记未使用的边）；fluid MUST
// NOT 反向依赖 sim/network/render/storage，否则它会退化成 sim 的内部实现，丧失
// 独立测试的意义。
//
// packages/server/updates → packages/shared/core（变更 unified-block-updates-world-streaming）：
// 统一确定性方块更新调度器（定时面 `Queue` 与随机面 `Sampler`）只消费位置与坐标类型，
// 预算、到期 tick 与处理回调全部由调用方注册/传入，不持有世界、不读 tunable。
// `packages/server/fluid` 与 `packages/server/sim/realm` 的允许列表在此预登记对它的
// 依赖（同 change 的调度迁移任务将实际 import：流体队列与湿度 FIFO 换接统一定时面、
// 随机抽样哈希链换用 `Sampler`），属该 change 明确批准的预登记例外；updates MUST NOT
// 反向依赖 fluid/sim，否则统一调度器会重新耦合进单一域。
//
// packages/server/sim/entity → packages/server/updates（变更 unified-block-updates-world-streaming）：
// entity 侧散布的 splitmix64 哈希链副本（采掘/踩踏产量与毒土豆、短草种子、树叶树苗
// 掉落、吃草抽选、生成候选哈希、漫游朝向派生）收敛为随机面 `Sampler` 的唯一真相，
// 消除与 sim/realm 各持一份、逐字复制且可能各自漂移的欠账。updates 只暴露纯函数判定，
// 不反向依赖 entity，实体状态所有权仍在 entity。
//
// packages/server/sim/realm → packages/shared/worldgen（变更 oak-sapling-regrowth）：随机
// tick 的树苗生长需要 engine 的树形几何，realm 经 worldgen 的既有 ABI 桥（与
// `MGW1` 同包，只有 nativeabi 接触 C ABI）取回相对根坐标的方块偏移列表并逐格
// 校验/写入。树形数值仍只在 Rust 计算、Go 侧无几何实现与 fallback；worldgen 只
// 依赖 core/world/nativeabi，不反向依赖任何 sim 包，因此这条边是「环境推进消费
// 世界生成数值」，不引入反向耦合。
var allowed = map[string][]string{
	"packages/audit":                            {},
	"packages/client/audio":                     {},
	"packages/contracts/companion-agent/mcp-v1": {},
	"packages/server/cmd/mornlea-server":        {"packages/server/server", "packages/server/storage", "packages/shared/companion", "packages/shared/config", "packages/shared/core", "packages/shared/logging", "packages/shared/network", "packages/shared/network/tcp", "packages/shared/world", "packages/shared/worldgen"},
	"packages/shared/companion":                 {"packages/shared/core", "packages/shared/pathfind"},
	"packages/shared/core":                      {"packages/shared/nativeabi"},
	"packages/shared/nativeabi":                 {},
	"packages/shared/config":                    {"packages/shared/companion", "packages/shared/core", "packages/shared/physics", "packages/shared/tuning", "packages/shared/logging"},
	"packages/server/fluid":                     {"packages/server/updates", "packages/shared/core", "packages/shared/nativeabi"},
	"packages/server/updates":                   {"packages/shared/core"},
	"packages/shared/physics":                   {"packages/shared/core", "packages/shared/nativeabi"},
	"packages/shared/pathfind":                  {"packages/shared/core"},
	"packages/shared/logging":                   {},
	"packages/shared/network":                   {"packages/shared/core", "packages/shared/network/codec", "packages/shared/network/protocol"},
	"packages/shared/network/codec":             {"packages/shared/core", "packages/shared/network/protocol"},
	"packages/shared/network/protocol":          {"packages/shared/companion", "packages/shared/core"},
	"packages/shared/network/tcp":               {"packages/shared/network"},
	"packages/shared/profile":                   {"packages/shared/core"},
	"packages/server/sim/contract":              {"packages/shared/companion", "packages/shared/core", "packages/shared/physics", "packages/shared/world"},
	"packages/server/sim/entity":                {"packages/shared/companion", "packages/shared/core", "packages/shared/physics", "packages/shared/world", "packages/server/sim/contract", "packages/server/sim/realm", "packages/server/updates", "packages/shared/tuning"},
	"packages/server/sim/realm":                 {"packages/shared/core", "packages/server/fluid", "packages/server/updates", "packages/shared/world", "packages/shared/worldgen"},
	"packages/server/sim/runtime":               {"packages/shared/companion", "packages/shared/core", "packages/shared/physics", "packages/shared/world", "packages/server/sim/contract", "packages/server/sim/entity", "packages/server/sim/realm", "packages/shared/tuning"},
	"packages/shared/tuning":                    {"packages/shared/core"},
	"packages/server/storage": {
		"packages/shared/core",
		"packages/server/storage/chunk", "packages/server/storage/companion", "packages/server/storage/hostile",
		"packages/server/storage/passive", "packages/server/storage/player", "packages/server/storage/region",
		"packages/server/storage/storagedef", "packages/shared/world",
	},
	"packages/server/storage/chunk":      {"packages/shared/core", "packages/server/storage/region", "packages/server/storage/storagedef", "packages/shared/world"},
	"packages/server/storage/player":     {"packages/shared/core", "packages/server/storage/storagedef"},
	"packages/server/storage/companion":  {"packages/shared/companion", "packages/shared/core", "packages/server/storage/storagedef"},
	"packages/server/storage/hostile":    {"packages/shared/core", "packages/server/storage/storagedef"},
	"packages/server/storage/passive":    {"packages/shared/core", "packages/server/storage/storagedef"},
	"packages/server/storage/region":     {"packages/shared/core", "packages/server/storage/storagedef"},
	"packages/server/storage/storagedef": {},
	"packages/shared/world":              {"packages/shared/core"},
	"packages/shared/worldgen":           {"packages/shared/core", "packages/shared/world", "packages/shared/nativeabi"},
	"packages/client/mesh":               {"packages/shared/core", "packages/shared/world", "packages/shared/nativeabi"},
	"packages/client/lod":                {"packages/shared/core", "packages/shared/nativeabi"},
	"packages/client/assets":             {"packages/shared/core", "packages/shared/world", "packages/client/mesh", "packages/shared/worldgen"},
	"packages/client/presentation":       {"packages/client/mesh", "packages/shared/core"},
	"packages/client/runtime": {
		"packages/client/assets", "packages/client/client", "packages/client/mesh",
		"packages/client/presentation", "packages/shared/core", "packages/shared/network",
		"packages/shared/network/protocol", "packages/shared/network/tcp", "packages/shared/physics",
	},
	// The Go client-core is a pilot transition exception: it may compose the
	// portable runtime/presentation packages until Rust client-core replaces it.
	"packages/client/cmd/mornlea-godot-core": {
		"packages/client/assets", "packages/client/presentation", "packages/client/render",
		"packages/client/runtime", "packages/shared/core", "packages/shared/network",
		"packages/shared/network/protocol",
	},
	"packages/client/render":             {"packages/shared/core", "packages/shared/world", "packages/client/mesh", "packages/client/assets"},
	"packages/client/render/hud":         {"packages/shared/core", "packages/client/mesh", "packages/client/assets", "packages/client/render"},
	"packages/server/server":             {"packages/contracts/companion-agent/mcp-v1", "packages/shared/companion", "packages/shared/core", "packages/shared/network", "packages/shared/pathfind", "packages/shared/physics", "packages/shared/world", "packages/shared/worldgen", "packages/server/sim/contract", "packages/server/sim/runtime", "packages/server/storage", "packages/server/server/persistence"},
	"packages/server/server/persistence": {"packages/shared/companion", "packages/shared/core", "packages/shared/physics", "packages/server/sim/contract", "packages/server/sim/runtime", "packages/server/storage"},
	"packages/client/client":             {"packages/shared/companion", "packages/shared/core", "packages/shared/physics", "packages/shared/network", "packages/shared/world", "packages/client/mesh", "packages/client/assets", "packages/client/render"},
	// The tools unit currently has five main packages. perfcheck and
	// gfxspike compose client mirror and render packages; cross-unit
	// direction is governed by the unit-boundary require table, and the
	// packages do not depend on each other inside the unit.
	// runtime-oracle is an offline contract inventory and replay tool:
	// production code may only read repository sources and testdata, and
	// must not depend on live authority or the native ABI.
	"packages/tools/agent-board":          {},
	"packages/tools/composite_grass_side": {},
	"packages/tools/gfxspike":             {"packages/client/assets", "packages/client/client", "packages/client/mesh", "packages/client/render", "packages/shared/core", "packages/shared/world", "packages/shared/worldgen"},
	"packages/tools/perfcheck":            {"packages/client/client"},
	"packages/tools/cmd/runtime-oracle":   {},
}

func TestInternalDependenciesAreOneWay(t *testing.T) {
	// 根模块已解散：白名单键横跨 go.work 直辖的全部单元模块（audit 本身在
	// packages/audit）。client 模块只列入六个域库子树——
	// `packages/client/cmd/mornlea` 命令子树由下方 `clientCommandAllowedEdges`
	// 单独治理，混入本表会双重登记其跨域边。tools 模块在 wildcard `./...` 之
	// 外追加非 wildcard 的 `./gfxspike`：gfxspike 整包 `//go:build darwin`，
	// 模式展开阶段的 wildcard 在 Linux 会静默跳过它，下方「每个白名单键必须
	// 被枚举到」的反断言必红；只有直接点名目录的模式才吃到 `-e` 的带错列
	// 出——ImportPath 完好、imports 为空，导入核对此时无边可查、反断言键
	// 命中，macOS 与 Linux CI 收敛到同一包集。保留 `./...` 让未来新增的
	// tools 包继续自动进入本检查。新模块 use 进 go.work 后同样自动进入。
	out := listWorkspacePackages(t, "{{.ImportPath}}|{{join .Imports \" \"}}", map[string][]string{
		"packages/client": {
			"./client/...", "./render/...", "./mesh/...", "./lod/...", "./audio/...", "./assets/...",
			"./runtime/...", "./presentation/...", "./cmd/mornlea-godot-core",
		},
		"packages/tools": {"./...", "./gfxspike"},
	})

	actual := make(map[string]bool)
	imports := make(map[string][]string)
	for _, line := range out {
		parts := strings.SplitN(line, "|", 2)
		pkg := localName(parts[0])
		actual[pkg] = true
		if _, ok := allowed[pkg]; !ok {
			t.Errorf("新增内部包 %s 未登记依赖白名单", pkg)
			continue
		}
		if len(parts) == 2 {
			imports[pkg] = strings.Fields(parts[1])
		}
	}
	for pkg := range allowed {
		if !actual[pkg] {
			t.Errorf("依赖白名单中的包 %s 不存在", pkg)
		}
	}

	for pkg, packageImports := range imports {
		allowSet := make(map[string]bool, len(allowed[pkg]))
		for _, dependency := range allowed[pkg] {
			allowSet[dependency] = true
		}
		for _, importPath := range packageImports {
			dependency := localName(importPath)
			if dependency == importPath {
				continue
			}
			if !allowSet[dependency] {
				t.Errorf("%s 不允许直接依赖 %s", pkg, dependency)
			}
		}
	}
}

// TestServerPersistenceDoesNotDependOnServer 钉住持久化子包到根包的反向依赖禁令。
//
// 生产代码（未带 persistence_contract 构建约束的文件）不得导入 `packages/server/server`；
// 唯一的例外是被 //go:build persistence_contract 门控的契约测试文件，其为校验
// 根包兼容 re-export 与哨兵恒等而显式依赖 `packages/server/server`。
func TestServerPersistenceDoesNotDependOnServer(t *testing.T) {
	if slices.Contains(allowed["packages/server/server/persistence"], "packages/server/server") {
		t.Fatalf("packages/server/server/persistence 不允许依赖 packages/server/server：子包不得反向依赖父包")
	}
	root := repositoryRoot(t)
	dir := filepath.Join(root, "packages", "server", "server", "persistence")
	entries, err := os.ReadDir(dir)
	if err != nil {
		t.Fatalf("读取 %s: %v", dir, err)
	}
	for _, entry := range entries {
		if entry.IsDir() || !strings.HasSuffix(entry.Name(), ".go") {
			continue
		}
		path := filepath.Join(dir, entry.Name())
		data, err := os.ReadFile(path)
		if err != nil {
			t.Fatalf("读取 %s: %v", path, err)
		}
		if strings.Contains(string(data), "persistence_contract") {
			continue
		}
		parsed, err := parser.ParseFile(token.NewFileSet(), path, nil, parser.ImportsOnly)
		if err != nil {
			t.Fatalf("解析 %s: %v", path, err)
		}
		for _, imp := range parsed.Imports {
			importPath := strings.Trim(imp.Path.Value, `"`)

			if localName(importPath) == "packages/server/server" {
				t.Errorf("持久化生产文件 %s 不允许导入 packages/server/server（仅 //go:build persistence_contract 的契约文件可导入）", entry.Name())
			}
		}
	}
}

// clientCommandAllowedEdges 列出客户端命令子树 `packages/client/cmd/mornlea` 允许的包间依赖
// 边（本地 import path）。依赖方向契约为：薄 main 装配全部功能域子包；
// capture、benchmark 与 devcapture 各自依赖 app；app 不反向依赖任何子包，
// capture 与 benchmark 互不依赖——否则两侧的重型测试会经由包图重新耦合，
// `go test` 的单包定点与测试分层同时失效。devcapture 实现 app 声明的
// `CaptureCoordinator`（consumer 侧接口模式）：接口定义住在 app、实现住在
// devcapture，app MUST NOT 反向 import devcapture。main 是 `package main`，
// 语言层面不可被子包导入，但允许边表仍显式登记它的装配目标，让契约可读而非
// 依赖编译器兜底。
var clientCommandAllowedEdges = map[string][]string{
	"packages/client/cmd/mornlea":            {"packages/client/cmd/mornlea/app", "packages/client/cmd/mornlea/benchmark", "packages/client/cmd/mornlea/capture", "packages/client/cmd/mornlea/devcapture"},
	"packages/client/cmd/mornlea/app":        {},
	"packages/client/cmd/mornlea/benchmark":  {"packages/client/cmd/mornlea/app"},
	"packages/client/cmd/mornlea/capture":    {"packages/client/cmd/mornlea/app"},
	"packages/client/cmd/mornlea/devcapture": {"packages/client/cmd/mornlea/app"},
}

// clientCommandRequiredEdges 是必须真实存在的装配边：main 必须装配
// app/benchmark/capture/devcapture 四个子包（devcapture 由 main 拉起监听并
// 注入 app），capture、benchmark 与 devcapture 必须经 app 访问宿主状态
// （devcapture 经此实现 `CaptureCoordinator` 并消费 app 状态访问器）。只查
// 「禁止边」守不住悄悄改走旁路的装配（例如 main 绕过 capture 的公开入口直取
// 其内部符号），必需边消失同样是方向契约的漂移。
var clientCommandRequiredEdges = map[string][]string{
	"packages/client/cmd/mornlea":            {"packages/client/cmd/mornlea/app", "packages/client/cmd/mornlea/benchmark", "packages/client/cmd/mornlea/capture", "packages/client/cmd/mornlea/devcapture"},
	"packages/client/cmd/mornlea/benchmark":  {"packages/client/cmd/mornlea/app"},
	"packages/client/cmd/mornlea/capture":    {"packages/client/cmd/mornlea/app"},
	"packages/client/cmd/mornlea/devcapture": {"packages/client/cmd/mornlea/app"},
}

// isClientCommandPackage 报告本地 import path 是否落在客户端命令子树内。
func isClientCommandPackage(localPath string) bool {
	return localPath == "packages/client/cmd/mornlea" || strings.HasPrefix(localPath, "packages/client/cmd/mornlea/")
}

// clientCommandDependencyViolations 对照允许边表与必需边表检查给定的包依赖
// 边，返回全部违规描述；空切片表示符合契约。输入是「本地包路径 → 生产
// import 的本地路径有序列表」，与真实目录解耦，使「注入反向边」的失败路径
// 可以在纯内存中核对，不必改动源码树。
func clientCommandDependencyViolations(edges map[string][]string) []string {
	var violations []string
	for pkg, packageImports := range edges {
		allowed, registered := clientCommandAllowedEdges[pkg]
		if !registered {
			violations = append(violations, fmt.Sprintf("客户端命令子树新增包 %s 未登记依赖白名单", pkg))
			continue
		}
		allowSet := make(map[string]bool, len(allowed))
		for _, dependency := range allowed {
			allowSet[dependency] = true
		}
		for _, dependency := range packageImports {
			if !isClientCommandPackage(dependency) || allowSet[dependency] {
				continue
			}
			violations = append(violations, fmt.Sprintf(
				"客户端命令包 %s 不允许依赖 %s：方向必须是 main → app/benchmark/capture/devcapture、capture/benchmark/devcapture → app，且 capture 与 benchmark 互不依赖",
				pkg, dependency))
		}
	}
	// 逐包核对必需边；包整体被删除时由「必需边找不到宿主」统一暴露。
	for pkg, required := range clientCommandRequiredEdges {
		importSet := make(map[string]bool, len(edges[pkg]))
		for _, dependency := range edges[pkg] {
			importSet[dependency] = true
		}
		for _, dependency := range required {
			if !importSet[dependency] {
				violations = append(violations, fmt.Sprintf("客户端命令包 %s 缺少必需依赖边 → %s", pkg, dependency))
			}
		}
	}
	slices.Sort(violations)
	return violations
}

// TestClientCommandSubpackageDependencyDirections 把真实源码树的包依赖边喂给
// `clientCommandDependencyViolations`，钉住客户端命令的依赖方向契约。
//
// 边的来源是逐文件解析生产 .go（parser.ImportsOnly 模式，跳过 `_test.go`），
// 而不是 `go list`：`go list` 的导入边随 GOOS 与构建约束变化——带
// `//go:build darwin` 的包（main/app/benchmark）在 Linux 下整个加载失败
// （报 build constraints exclude all Go files，`.Imports` 为空），不带 tag 的
// capture 反而照常报出完整导入边；建立在 `go list` 上的断言因此随运行平台
// 翻转。源码级 AST 解析与平台无关，两个 CI 平台看到同一份边集。测试文件不
// 计入——子包测试经 app 包导出的测试装配入口（`testkit.go`）复用是合法的，
// 不应污染生产依赖方向。
func TestClientCommandSubpackageDependencyDirections(t *testing.T) {
	edges := clientCommandImportEdges(t)
	if violations := clientCommandDependencyViolations(edges); len(violations) > 0 {
		t.Errorf("客户端命令依赖方向违反契约，%d 条违规：\n%s", len(violations), strings.Join(violations, "\n"))
	}
}

// clientCommandImportEdges 扫描 `packages/client/cmd/mornlea` 子树内全部含生产 Go 源文件的
// 目录，返回「本地包路径 → 去重排序后的本地生产 import 列表」。子树内出现
// 未登记的新包目录时由检查器的白名单核对报错，新增子包不可能静默绕过。
func clientCommandImportEdges(t *testing.T) map[string][]string {
	t.Helper()
	root := repositoryRoot(t)
	subtree := filepath.Join(root, "packages", "client", "cmd", "mornlea")
	edges := make(map[string][]string)
	err := filepath.WalkDir(subtree, func(path string, entry fs.DirEntry, walkErr error) error {
		if walkErr != nil {
			return walkErr
		}
		if !entry.IsDir() {
			return nil
		}
		entries, err := os.ReadDir(path)
		if err != nil {
			return err
		}
		imports := make(map[string]bool)
		for _, file := range entries {
			name := file.Name()
			if file.IsDir() || !strings.HasSuffix(name, ".go") || strings.HasSuffix(name, "_test.go") {
				continue
			}
			parsed, err := parser.ParseFile(token.NewFileSet(), filepath.Join(path, name), nil, parser.ImportsOnly)
			if err != nil {
				return err
			}
			for _, imported := range parsed.Imports {
				importPath := strings.Trim(imported.Path.Value, `"`)
				if local := localName(importPath); local != importPath {
					imports[local] = true
				}
			}
		}
		if !hasProductionGoFile(entries) {
			// testdata 等纯资产目录不是包，跳过；含生产源文件的包即使没有
			// 任何本地 import 也要登记（如纯叶子包），否则白名单核对漏检。
			return nil
		}
		relative, err := filepath.Rel(root, path)
		if err != nil {
			return err
		}
		edges[filepath.ToSlash(relative)] = slices.Sorted(maps.Keys(imports))
		return nil
	})
	if err != nil {
		t.Fatalf("扫描 %s: %v", subtree, err)
	}
	return edges
}

// hasProductionGoFile 报告目录项中是否存在非 `_test.go` 的 Go 源文件。
func hasProductionGoFile(entries []os.DirEntry) bool {
	for _, file := range entries {
		if !file.IsDir() && strings.HasSuffix(file.Name(), ".go") && !strings.HasSuffix(file.Name(), "_test.go") {
			return true
		}
	}
	return false
}

// TestClientCommandDependencyViolationsDetectDrift 用合成边核对检查器对每类
// 漂移都真的报错。真实源码树处于契约内时，方向断言的负向路径没有天然的失败
// 信号；必须另有合成断言钉住检查器本身，否则检查逻辑被改坏后门禁静默变松。
func TestClientCommandDependencyViolationsDetectDrift(t *testing.T) {
	// contractEdges 是恰好满足契约的最小合成边集。
	contractEdges := func() map[string][]string {
		return map[string][]string{
			"packages/client/cmd/mornlea":            {"packages/client/cmd/mornlea/app", "packages/client/cmd/mornlea/benchmark", "packages/client/cmd/mornlea/capture", "packages/client/cmd/mornlea/devcapture"},
			"packages/client/cmd/mornlea/app":        {},
			"packages/client/cmd/mornlea/benchmark":  {"packages/client/cmd/mornlea/app"},
			"packages/client/cmd/mornlea/capture":    {"packages/client/cmd/mornlea/app"},
			"packages/client/cmd/mornlea/devcapture": {"packages/client/cmd/mornlea/app"},
		}
	}
	if violations := clientCommandDependencyViolations(contractEdges()); len(violations) != 0 {
		t.Fatalf("契约内的合成边不应报违规: %v", violations)
	}

	for _, edge := range []struct {
		name string
		from string
		to   string
	}{
		{"app反向依赖capture", "packages/client/cmd/mornlea/app", "packages/client/cmd/mornlea/capture"},
		{"app反向依赖benchmark", "packages/client/cmd/mornlea/app", "packages/client/cmd/mornlea/benchmark"},
		{"capture依赖benchmark", "packages/client/cmd/mornlea/capture", "packages/client/cmd/mornlea/benchmark"},
		{"benchmark依赖capture", "packages/client/cmd/mornlea/benchmark", "packages/client/cmd/mornlea/capture"},
	} {
		t.Run(edge.name, func(t *testing.T) {
			edges := contractEdges()
			edges[edge.from] = append(edges[edge.from], edge.to)
			violations := clientCommandDependencyViolations(edges)
			if len(violations) != 1 || !strings.Contains(violations[0], edge.from+" 不允许依赖 "+edge.to) {
				t.Fatalf("注入禁止边 %s → %s 未被拒绝: %v", edge.from, edge.to, violations)
			}
		})
	}

	t.Run("必需边缺失", func(t *testing.T) {
		edges := contractEdges()
		edges["packages/client/cmd/mornlea"] = nil
		violations := clientCommandDependencyViolations(edges)
		if len(violations) != 4 {
			t.Fatalf("main 卸掉四个子包的装配应报 4 条缺失: %v", violations)
		}
	})

	t.Run("未登记新包", func(t *testing.T) {
		edges := contractEdges()
		edges["packages/client/cmd/mornlea/widgets"] = []string{"packages/client/cmd/mornlea/app"}
		violations := clientCommandDependencyViolations(edges)
		if len(violations) != 1 || !strings.Contains(violations[0], "未登记依赖白名单") {
			t.Fatalf("未登记子包未被拒绝: %v", violations)
		}
	})
}

// simAllowedEdges 列出权威模拟子树 `packages/server/sim` 四个子包允许的内部依赖
// 边（本地 import path）。依赖方向契约为：`contract` 为叶子，`realm` 只依赖
// 环境，`entity` 可依赖 `contract`/`realm` 与共享层的 `packages/shared/tuning`
// 快照，`runtime` 是唯一允许同时编排其余三者和 tuning 快照的权威入口；任何
// 反向边都会让事务边界或快照所有权重新耦合，`go test` 的单包定点与事务收敛
// 同时失效。tuning 已上提 `packages/shared`（纯调参值对象，不在 sim 子树内），
// 自身的生产依赖由全局 `allowed` 表管。该表与全局 `allowed` 中对应四项保持
// 一致，重复列出是为了让子树检查器可在不依赖全局表的情况下独立校验方向
// 与合成反向边。
var simAllowedEdges = map[string][]string{
	"packages/server/sim/contract": {"packages/shared/companion", "packages/shared/core", "packages/shared/physics", "packages/shared/world"},
	"packages/server/sim/realm":    {"packages/shared/core", "packages/server/fluid", "packages/server/updates", "packages/shared/world", "packages/shared/worldgen"},
	"packages/server/sim/entity":   {"packages/shared/companion", "packages/shared/core", "packages/shared/physics", "packages/shared/world", "packages/server/sim/contract", "packages/server/sim/realm", "packages/server/updates", "packages/shared/tuning"},
	"packages/server/sim/runtime":  {"packages/shared/companion", "packages/shared/core", "packages/shared/physics", "packages/shared/world", "packages/server/sim/contract", "packages/server/sim/entity", "packages/server/sim/realm", "packages/shared/tuning"},
}

// simRequiredEdges 是模拟子树必须真实存在的编排边：`runtime` 必须同时
// 装配三个下层子包与共享层的 tuning 快照，否则权威 tick 的阶段编排会悄悄
// 绕过某层状态的所有权边界，单 mutation 提交路径不再覆盖全部写入。
var simRequiredEdges = map[string][]string{
	"packages/server/sim/runtime": {"packages/server/sim/contract", "packages/server/sim/entity", "packages/server/sim/realm", "packages/shared/tuning"},
}

// TestSimAllowedEdgesMatchesGlobalAllowed 校验模拟子树的局部白名单
// 与全局 `allowed` 的对应项完全一致，防止双真相表静默漂移。
func TestSimAllowedEdgesMatchesGlobalAllowed(t *testing.T) {
	for pkg, simAllowed := range simAllowedEdges {
		globalAllowed, ok := allowed[pkg]
		if !ok {
			t.Fatalf("模拟子树包 %s 未在全局 allowed 中登记", pkg)
		}
		sortedSim := slices.Clone(simAllowed)
		slices.Sort(sortedSim)
		sortedGlobal := slices.Clone(globalAllowed)
		slices.Sort(sortedGlobal)
		if !slices.Equal(sortedSim, sortedGlobal) {
			t.Errorf("模拟子树包 %s 的局部白名单与全局 allowed 不一致：局部 %v，全局 %v", pkg, sortedSim, sortedGlobal)
		}
	}
	for pkg := range allowed {
		if isSimPackage(pkg) {
			if _, ok := simAllowedEdges[pkg]; !ok {
				t.Errorf("全局 allowed 中的模拟子树包 %s 未在 simAllowedEdges 中登记", pkg)
			}
		}
	}
}

// isSimPackage 报告本地 import path 是否落在权威模拟子树内。
func isSimPackage(localPath string) bool {
	return localPath == "packages/server/sim" || strings.HasPrefix(localPath, "packages/server/sim/")
}

// simDependencyViolations 对照模拟子树的允许边表与必需边表检查给定的
// 包依赖边，返回全部违规描述；空切片表示符合契约。输入是「本地包路径 →
// 生产 import 的本地路径有序列表」，与真实目录解耦，使「注入反向边」
// 的失败路径可以在纯内存中核对，不必改动源码树。
func simDependencyViolations(edges map[string][]string) []string {
	var violations []string
	for pkg, packageImports := range edges {
		allowed, registered := simAllowedEdges[pkg]
		if !registered {
			if isSimPackage(pkg) {
				violations = append(violations, fmt.Sprintf("模拟子树新增包 %s 未登记依赖白名单", pkg))
			}
			continue
		}
		allowSet := make(map[string]bool, len(allowed))
		for _, dependency := range allowed {
			allowSet[dependency] = true
		}
		for _, dependency := range packageImports {
			if allowSet[dependency] {
				continue
			}
			violations = append(violations, fmt.Sprintf(
				"模拟子包 %s 不允许依赖 %s：方向必须是 contract 不依赖 realm/entity/runtime、realm 不依赖 contract/entity/runtime、entity 不依赖 runtime、runtime 编排其余三者与 packages/shared/tuning 快照",
				pkg, dependency))
		}
	}
	// 逐包核对必需边；包整体被删除时由「必需边找不到宿主」统一暴露。
	for pkg, required := range simRequiredEdges {
		importSet := make(map[string]bool, len(edges[pkg]))
		for _, dependency := range edges[pkg] {
			importSet[dependency] = true
		}
		for _, dependency := range required {
			if !importSet[dependency] {
				violations = append(violations, fmt.Sprintf("模拟子包 %s 缺少必需依赖边 → %s", pkg, dependency))
			}
		}
	}
	slices.Sort(violations)
	return violations
}

// simImportEdges 扫描 `packages/server/sim` 子树内全部含生产 Go 源文件的目录，
// 返回「本地包路径 → 去重排序后的本地生产 import 列表」。子树内出现
// 未登记的新包目录时由检查器的白名单核对报错，新增子包不可能静默绕过。
func simImportEdges(t *testing.T) map[string][]string {
	t.Helper()
	root := repositoryRoot(t)
	subtree := filepath.Join(root, "packages", "server", "sim")
	edges := make(map[string][]string)
	err := filepath.WalkDir(subtree, func(path string, entry fs.DirEntry, walkErr error) error {
		if walkErr != nil {
			return walkErr
		}
		if !entry.IsDir() {
			return nil
		}
		entries, err := os.ReadDir(path)
		if err != nil {
			return err
		}
		imports := make(map[string]bool)
		for _, file := range entries {
			name := file.Name()
			if file.IsDir() || !strings.HasSuffix(name, ".go") || strings.HasSuffix(name, "_test.go") {
				continue
			}
			parsed, err := parser.ParseFile(token.NewFileSet(), filepath.Join(path, name), nil, parser.ImportsOnly)
			if err != nil {
				return err
			}
			for _, imported := range parsed.Imports {
				importPath := strings.Trim(imported.Path.Value, `"`)
				if local := localName(importPath); local != importPath {
					imports[local] = true
				}
			}
		}
		if !hasProductionGoFile(entries) {
			return nil
		}
		relative, err := filepath.Rel(root, path)
		if err != nil {
			return err
		}
		edges[filepath.ToSlash(relative)] = slices.Sorted(maps.Keys(imports))
		return nil
	})
	if err != nil {
		t.Fatalf("扫描 %s: %v", subtree, err)
	}
	return edges
}

// TestSimSubpackageDependencyDirections 把真实源码树的模拟子树包依赖边
// 喂给 `simDependencyViolations`，钉住权威模拟的依赖方向契约。
//
// 边的来源是逐文件解析生产 .go（parser.ImportsOnly 模式，跳过 `_test.go`），
// 与 `TestClientCommandSubpackageDependencyDirections` 同理保持平台无关。
// 测试文件不计入——子包测试经同包或跨包测试装配复用是合法的，不应污染
// 生产依赖方向；全局 `TestInternalDependenciesAreOneWay` 另以 `go list`
// 覆盖全仓内部包的完整白名单，本测试聚焦模拟子树内部的方向与必需边。
func TestSimSubpackageDependencyDirections(t *testing.T) {
	edges := simImportEdges(t)
	if violations := simDependencyViolations(edges); len(violations) > 0 {
		t.Errorf("模拟子树依赖方向违反契约，%d 条违规：\n%s", len(violations), strings.Join(violations, "\n"))
	}
}

// TestSimDependencyViolationsDetectDrift 用合成边核对检查器对每类漂移
// 都真的报错。真实源码树处于契约内时，方向断言的负向路径没有天然的
// 失败信号；必须另有合成断言钉住检查器本身，否则检查逻辑被改坏后门禁
// 静默变松。
func TestSimDependencyViolationsDetectDrift(t *testing.T) {
	contractEdges := func() map[string][]string {
		return map[string][]string{
			"packages/server/sim/contract": {"packages/shared/core", "packages/shared/world", "packages/shared/companion", "packages/shared/physics"},
			"packages/server/sim/realm":    {"packages/shared/core", "packages/server/fluid", "packages/shared/world", "packages/shared/worldgen"},
			"packages/server/sim/entity":   {"packages/shared/companion", "packages/shared/core", "packages/shared/physics", "packages/shared/world", "packages/server/sim/contract", "packages/server/sim/realm", "packages/shared/tuning"},
			"packages/server/sim/runtime":  {"packages/shared/companion", "packages/shared/core", "packages/shared/physics", "packages/shared/world", "packages/server/sim/contract", "packages/server/sim/entity", "packages/server/sim/realm", "packages/shared/tuning"},
		}
	}
	if violations := simDependencyViolations(contractEdges()); len(violations) != 0 {
		t.Fatalf("契约内的合成边不应报违规: %v", violations)
	}

	for _, edge := range []struct {
		name string
		from string
		to   string
	}{
		{"contract反向依赖tuning", "packages/server/sim/contract", "packages/shared/tuning"},
		{"contract反向依赖realm", "packages/server/sim/contract", "packages/server/sim/realm"},
		{"contract反向依赖entity", "packages/server/sim/contract", "packages/server/sim/entity"},
		{"contract反向依赖runtime", "packages/server/sim/contract", "packages/server/sim/runtime"},
		{"realm反向依赖contract", "packages/server/sim/realm", "packages/server/sim/contract"},
		{"realm反向依赖tuning", "packages/server/sim/realm", "packages/shared/tuning"},
		{"realm反向依赖entity", "packages/server/sim/realm", "packages/server/sim/entity"},
		{"realm反向依赖runtime", "packages/server/sim/realm", "packages/server/sim/runtime"},
		{"entity反向依赖runtime", "packages/server/sim/entity", "packages/server/sim/runtime"},
	} {
		t.Run(edge.name, func(t *testing.T) {
			edges := contractEdges()
			edges[edge.from] = append(edges[edge.from], edge.to)
			violations := simDependencyViolations(edges)
			if len(violations) != 1 || !strings.Contains(violations[0], edge.from+" 不允许依赖 "+edge.to) {
				t.Fatalf("注入禁止边 %s → %s 未被拒绝: %v", edge.from, edge.to, violations)
			}
		})
	}

	t.Run("必需边缺失", func(t *testing.T) {
		edges := contractEdges()
		edges["packages/server/sim/runtime"] = []string{"packages/server/sim/contract", "packages/server/sim/realm", "packages/shared/tuning"}
		violations := simDependencyViolations(edges)
		found := false
		for _, violation := range violations {
			if strings.Contains(violation, "缺少必需依赖边") && strings.Contains(violation, "packages/server/sim/entity") {
				found = true
				break
			}
		}
		if !found {
			t.Fatalf("runtime 卸掉 entity 装配应报缺失: %v", violations)
		}
	})

	t.Run("未登记新包", func(t *testing.T) {
		edges := contractEdges()
		edges["packages/server/sim/extra"] = []string{"packages/shared/core"}
		violations := simDependencyViolations(edges)
		if len(violations) != 1 || !strings.Contains(violations[0], "未登记依赖白名单") {
			t.Fatalf("未登记子包未被拒绝: %v", violations)
		}
	})
}

var oracleAllowedTestImports = map[string]bool{
	"packages/shared/network/codec":     true,
	"packages/shared/network/protocol":  true,
	"packages/shared/core":              true,
	"packages/shared/world":             true,
	"packages/shared/companion":         true,
	"packages/shared/pathfind":          true,
	"packages/shared/nativeabi":         true,
	"packages/server/storage/chunk":     true,
	"packages/server/storage/player":    true,
	"packages/server/storage/companion": true,
	"packages/server/storage/hostile":   true,
	"packages/server/storage/passive":   true,
	"packages/server/storage/region":    true,
	"packages/server/storage/storagedef": true,
}

func validateOracleImports(filename, src string) error {
	fset := token.NewFileSet()
	f, err := parser.ParseFile(fset, filename, src, parser.ImportsOnly)
	if err != nil {
		return fmt.Errorf("parse error: %w", err)
	}
	isTest := strings.HasSuffix(filename, "_test.go")
	for _, imp := range f.Imports {
		importPath := strings.Trim(imp.Path.Value, `"`)
		rel := localName(importPath)
		if rel == importPath {
			if strings.HasPrefix(importPath, "github.com/channing771/mornlea/") {
				return fmt.Errorf("unknown internal import %s", importPath)
			}
			continue
		}
		if !isTest {
			return fmt.Errorf("production oracle file %s cannot import internal package %s", filename, rel)
		}
		if !oracleAllowedTestImports[rel] {
			return fmt.Errorf("oracle test file %s not permitted to import %s", filename, rel)
		}
	}
	return nil
}

func TestRuntimeOracleInternalDependencies(t *testing.T) {
	root := filepath.Join(repositoryRoot(t), "packages", "tools", "cmd", "runtime-oracle")
	entries, err := os.ReadDir(root)
	if err != nil {
		t.Fatalf("read oracle dir: %v", err)
	}
	for _, entry := range entries {
		if entry.IsDir() || !strings.HasSuffix(entry.Name(), ".go") {
			continue
		}
		path := filepath.Join(root, entry.Name())
		data, err := os.ReadFile(path)
		if err != nil {
			t.Fatalf("read %s: %v", path, err)
		}
		if err := validateOracleImports(entry.Name(), string(data)); err != nil {
			t.Errorf("%s violates oracle import rules: %v", entry.Name(), err)
		}
	}
}

func TestOracleDependencyRedMatrix(t *testing.T) {
	// 4. rejected ordinary .go codec import in oracle
	srcProdCodec := `package main
import _ "github.com/channing771/mornlea/packages/shared/network/codec"
`
	if err := validateOracleImports("discover.go", srcProdCodec); err == nil {
		t.Fatal("production oracle codec import must be rejected")
	}

	// 5. accepted _test.go codec import in oracle
	srcTestCodec := `package main
import _ "github.com/channing771/mornlea/packages/shared/network/codec"
`
	if err := validateOracleImports("runner_test.go", srcTestCodec); err != nil {
		t.Fatalf("test oracle codec import should be accepted, got: %v", err)
	}

	// 6. rejected _test.go online server import in oracle
	srcTestServer := `package main
import _ "github.com/channing771/mornlea/packages/server/server"
`
	if err := validateOracleImports("runner_test.go", srcTestServer); err == nil {
		t.Fatal("test oracle online server import must be rejected")
	}

	// 7. rejected ordinary .go storagedef import in oracle
	srcProdStorageDef := `package main
import _ "github.com/channing771/mornlea/packages/server/storage/storagedef"
`
	if err := validateOracleImports("inventory.go", srcProdStorageDef); err == nil {
		t.Fatal("production oracle storagedef import must be rejected")
	}

	// 8. accepted _test.go storagedef import in oracle
	srcTestStorageDef := `package main
import _ "github.com/channing771/mornlea/packages/server/storage/storagedef"
`
	if err := validateOracleImports("storage_manifest_test.go", srcTestStorageDef); err != nil {
		t.Fatalf("test oracle storagedef import should be accepted, got: %v", err)
	}
}
