package archcheck_test

import (
	"bytes"
	"encoding/json"
	"errors"
	"fmt"
	"go/ast"
	"go/build"
	"go/constant"
	"go/importer"
	"go/parser"
	"go/token"
	"go/types"
	"io"
	"io/fs"
	"os"
	"os/exec"
	"path/filepath"
	"regexp"
	"sort"
	"strconv"
	"strings"
	"testing"
)

const (
	modulePath                       = "github.com/channing771/mornlea"
	expectedLegacyIdentityAllowances = 42
	expectedLegacyIdentityMatches    = 46
)

var (
	legacyDataDirectory  = identityToken("minecraft", "-go")
	legacyBackupIdentity = identityToken(".mc", "go-world-backup-v1.json")
	currentIdentityRoots = []string{
		"go.work",
		"packages/contracts",
		"packages/server",
		"packages/shared",
		"packages/client",
		"packages/tools",
		"packages/audit",
		"packages/engine/Cargo.toml",
		"packages/engine/Cargo.lock",
		"packages/engine/crates",
		"packages/engine/include",
		"Makefile",
		".github/workflows/ci.yml",
		"scripts/agent-hooks",
		".gitignore",
	}
	forbiddenCurrentIdentity = []string{
		identityToken("module minecraft", "-go"),
		identityToken(`"minecraft`, `-go/internal/`),
		identityToken("github.com/channing771/minecraft", "-go"),
		identityToken("cmd/mc", "go"),
		identityToken("cmd/mc", "god"),
		identityToken("bin/mc", "go"),
		identityToken("mc", "go_mesh"),
		identityToken("libmc", "go_mesh"),
		identityToken("mc", "go_engine"),
		identityToken("MC", "GO_ENGINE_"),
		identityToken("MC", "GO_STATUS_"),
		identityToken("MINECRAFT", "_GO_"),
		identityToken("MC", "GOD_"),
	}
	legacyIdentityPattern = regexp.MustCompile(identityToken("(?i)(?:minecraft[-_]", "go|mc", "go)"))
)

func identityToken(parts ...string) string {
	return strings.Join(parts, "")
}

type legacyIdentityAllowance struct {
	path     string
	literal  string
	owner    string
	expected int
}

var legacyIdentityAllowances = []legacyIdentityAllowance{
	// The Rust store shares the existing on-disk backup identity with Go.
	{"packages/engine/crates/mornlea_server/src/store/recovery.rs", legacyBackupIdentity, "BACKUP_IDENTITY", 1},
	{"packages/shared/config/config.go", legacyDataDirectory, "defaultPaths", 1},
	{"packages/shared/config/migration_test.go", legacyDataDirectory, "TestLoadDefaultUsesMornleaCurrentAndMinecraftGoLegacy", 1},
	{"packages/shared/config/migration_test.go", legacyDataDirectory, "TestLoadDefaultPrefersExistingMornleaConfig", 1},
	{"packages/shared/config/migration_test.go", legacyDataDirectory, "TestLoadDefaultMigratesLegacyConfigAndPreservesSource", 1},
	{"packages/shared/config/migration_test.go", legacyDataDirectory, "TestLoadDefaultRejectsInvalidAuthoritativeConfig", 1},
	{"packages/shared/config/migration_test.go", legacyDataDirectory, "TestLoadDefaultRejectsInvalidLegacyConfigWithoutCreatingCurrent", 1},
	{"packages/shared/config/migration_test.go", legacyDataDirectory, "TestLoadDefaultMissingBothReturnsDefaultsWithoutCreatingFile", 1},
	{"packages/shared/config/migration_test.go", legacyDataDirectory, "TestLoadDefaultRejectsUnsafeParentPermissions", 1},
	{"packages/shared/config/migration_test.go", legacyDataDirectory, "TestLoadDefaultRejectsUnsafeTargetPermissions", 1},
	{"packages/shared/config/migration_test.go", legacyDataDirectory, "TestLoadDefaultRejectsSymlinkTarget", 1},
	{"packages/shared/config/migration_test.go", legacyDataDirectory, "TestLoadDefaultRejectsSameInodeSymlinkInsertedBeforeOpen", 1},
	{"packages/shared/config/migration_test.go", legacyDataDirectory, "TestLoadDefaultRejectsTargetReplacedAfterPathValidation", 1},
	{"packages/shared/config/migration_test.go", legacyDataDirectory, "TestLoadDefaultReadsConcurrentWinnerWithoutOverwritingIt", 1},
	{"packages/shared/config/migration_test.go", legacyDataDirectory, "TestLoadDefaultRejectsUnsafeConcurrentWinnerPermissions", 1},
	{"packages/shared/config/migration_test.go", legacyDataDirectory, "TestLoadDefaultPublishFailurePreservesLegacyAndCleansTemporary", 1},
	{"packages/shared/config/migration_test.go", legacyDataDirectory, "TestLoadDefaultDirectorySyncFailureIncludesTargetAndDoesNotLog", 1},
	{"packages/shared/config/migration_test.go", legacyDataDirectory, "TestLoadDefaultLogsOnlySuccessfulMigrationPublisher", 3},
	{"packages/shared/profile/profile.go", legacyDataDirectory, "defaultPaths", 1},
	{"packages/shared/profile/profile_test.go", legacyDataDirectory, "TestLoadOrCreateRejectsInsecureParentBeforeRenaming", 1},
	{"packages/shared/profile/profile_test.go", legacyDataDirectory, "TestLoadOrCreateKeepsIDWhenNameChanges", 1},
	{"packages/shared/profile/profile_test.go", legacyDataDirectory, "TestLoadOrCreateDefaultPrefersExistingMornleaProfile", 1},
	{"packages/shared/profile/profile_test.go", legacyDataDirectory, "TestLoadOrCreateDefaultMigratesLegacyProfileExactly", 1},
	{"packages/shared/profile/profile_test.go", legacyDataDirectory, "TestLoadOrCreateDefaultMigrationAppliesRequestedName", 1},
	{"packages/shared/profile/profile_test.go", legacyDataDirectory, "TestLoadOrCreateDefaultRejectsInvalidAuthoritativeProfile", 1},
	{"packages/shared/profile/profile_test.go", legacyDataDirectory, "TestLoadOrCreateCustomPathSkipsDefaultMigration", 1},
	{"packages/shared/profile/profile_test.go", legacyDataDirectory, "TestLoadOrCreateDefaultMissingBothCreatesSingleUUIDv4", 1},
	{"packages/shared/profile/profile_test.go", legacyDataDirectory, "TestLoadOrCreateDefaultConcurrentCreationReturnsSingleWinner", 1},
	{"packages/shared/profile/profile_test.go", legacyDataDirectory, "TestLoadOrCreateDefaultReadsConcurrentMigrationWinner", 1},
	{"packages/shared/profile/profile_test.go", legacyDataDirectory, "TestLoadOrCreateDefaultPublishFailureDoesNotGenerateIdentity", 1},
	{"packages/shared/profile/profile_test.go", legacyDataDirectory, "TestLoadOrCreateDefaultRejectsUnsafeParentPermissions", 1},
	{"packages/shared/profile/profile_test.go", legacyDataDirectory, "TestLoadOrCreateDefaultRejectsUnsafeTargetPermissions", 1},
	{"packages/shared/profile/profile_test.go", legacyDataDirectory, "TestLoadOrCreateDefaultRejectsTargetReplacedAfterPathValidation", 1},
	{"packages/shared/profile/profile_test.go", legacyDataDirectory, "TestLoadOrCreateDefaultRejectsSameInodeSymlinkInsertedBeforeOpen", 1},
	{"packages/shared/profile/profile_test.go", legacyDataDirectory, "TestLoadOrCreateDefaultLogsOnlySuccessfulMigrationPublisher", 3},
	{"packages/client/cmd/mornlea/run_test.go", legacyDataDirectory, "legacyDataPath", 1},
	{"packages/server/cmd/mornlea-server/main_test.go", legacyDataDirectory, "legacyConfigPath", 1},
	{"packages/server/storage/backup.go", legacyBackupIdentity, "backupIdentityName", 1},
	{"packages/server/storage/backup_test.go", legacyBackupIdentity, "TestWorldBackupCopiesCompleteWorldAndReusesMatchingBackup", 1},
	{"packages/server/storage/backup_test.go", legacyBackupIdentity, "TestWorldBackupRejectsEveryMismatchedIdentityField", 1},
	{"packages/server/storage/backup_test.go", legacyBackupIdentity, "TestWorldBackupRejectsOversizedIdentity", 1},
	{"packages/server/storage/backup_test.go", legacyBackupIdentity, "readWorldBackupIdentity", 1},
}

type sourceStringLiteral struct {
	value     string
	owner     string
	start     int
	end       int
	canonical bool
}

type goIdentityFile struct {
	relative string
	source   []byte
	parsed   *ast.File
}

type goIdentityScanner struct {
	root        string
	fileSet     *token.FileSet
	files       map[string]goIdentityFile
	directories map[string]bool
}

func newGoIdentityScanner(root string) *goIdentityScanner {
	return &goIdentityScanner{
		root:        root,
		fileSet:     token.NewFileSet(),
		files:       make(map[string]goIdentityFile),
		directories: make(map[string]bool),
	}
}

func TestNativeEngineLibraryIdentity(t *testing.T) {
	root := repositoryRoot(t)
	engineCrate := filepath.Join(root, "packages", "engine", "crates", "mornlea_engine")
	if info, err := os.Stat(filepath.Join(engineCrate, "Cargo.toml")); err != nil || info.IsDir() {
		t.Fatalf("Rust engine crate 必须存在: %v", err)
	}
	if _, err := os.Stat(filepath.Join(root, "packages", "engine", "crates", "mornlea_mesh")); !os.IsNotExist(err) {
		t.Fatalf("旧 Rust crate 不得存在: %v", err)
	}

	requireIdentity := func(relative, want, old string) {
		t.Helper()
		content, err := os.ReadFile(filepath.Join(root, filepath.FromSlash(relative)))
		if err != nil {
			t.Fatalf("读取 %s: %v", relative, err)
		}
		if !bytes.Contains(content, []byte(want)) {
			t.Errorf("%s 必须包含 %q", relative, want)
		}
		if bytes.Contains(content, []byte(old)) {
			t.Errorf("%s 不得包含旧身份 %q", relative, old)
		}
	}

	requireIdentity("packages/engine/Cargo.toml", `members = [
    "crates/mornlea_engine",
    "crates/mornlea_client",
    "crates/mornlea_godot",
    "crates/mornlea_domain",
    "crates/mornlea_protocol",
    "crates/mornlea_storage",
    "crates/mornlea_server",
    "crates/mornlea_client_core",
]`, "crates/mornlea_mesh")
	requireIdentity("packages/engine/crates/mornlea_engine/Cargo.toml", `name = "mornlea_engine"`, `name = "mornlea_mesh"`)
	requireIdentity("packages/engine/crates/mornlea_engine/build.rs", "@rpath/libmornlea_engine.dylib", "libmornlea_mesh.dylib")
	requireIdentity("Makefile", "libmornlea_engine.dylib", "libmornlea_mesh.dylib")
	requireIdentity("packages/shared/nativeabi/native.go", "-lmornlea_engine", "-lmornlea_mesh")
	for _, relative := range []string{"AGENTS.md", "README.md", "README.zh.md", "openspec/config.yaml", "docs/notes/progress.md"} {
		requireIdentity(relative, "mornlea_engine", "libmornlea_mesh")
	}
}

func TestMornleaCurrentIdentity(t *testing.T) {
	if root := os.Getenv("MORNLEA_IDENTITY_TEST_ROOT"); root != "" {
		actual := make([]int, len(legacyIdentityAllowances))
		goScanner := newGoIdentityScanner(root)
		// 合成 mutation 各自只建自己需要的目录（tools、client 或 server 模块
		// 子树）；存在才扫，统一扫描会让只建一侧的 mutation 误报「身份扫描根
		// 不存在」。cmd/internal 已随根模块解散退役，mutation 的落点改用
		// packages/tools 子树。
		for _, relative := range []string{"packages/tools", "packages/client", "packages/server", "packages/engine/crates"} {
			if _, err := os.Lstat(filepath.Join(root, filepath.FromSlash(relative))); err == nil {
				scanCurrentIdentityRoot(t, root, relative, actual, goScanner)
			}
		}
		goScanner.scanConstants(t)
		return
	}

	root := repositoryRoot(t)
	requireLinuxServerBundleIdentity(t, root)
	// 根模块已解散：顶层不得再有 go.mod，全部 Go 单元由 go.work 直辖（use 集
	// 与 packages/ 下 go.mod 的一致性由 unit boundary 检查另守）。此处只钉
	// 「根目录无模块清单」这一身份事实，旧的单模块前缀断言随 go.mod 一并退役。
	if _, err := os.Lstat(filepath.Join(root, "go.mod")); err == nil {
		t.Errorf("仓库根不得保留 go.mod：六个单元模块（shared/server/client/tools/audit/contracts）由 go.work 直辖")
	} else if !errors.Is(err, fs.ErrNotExist) {
		t.Fatalf("检查根 go.mod: %v", err)
	}

	actual := make([]int, len(legacyIdentityAllowances))
	goScanner := newGoIdentityScanner(root)
	if len(legacyIdentityAllowances) != expectedLegacyIdentityAllowances {
		t.Fatalf("旧数据身份 allowlist tuple 数 = %d，期望 %d", len(legacyIdentityAllowances), expectedLegacyIdentityAllowances)
	}
	expectedMatches := 0
	for index, allowance := range legacyIdentityAllowances {
		if allowance.expected <= 0 {
			t.Fatalf("allowlist[%d] 的 expected 必须为正数", index)
		}
		expectedMatches += allowance.expected
	}
	if expectedMatches != expectedLegacyIdentityMatches {
		t.Fatalf("旧数据身份 allowlist match 总数 = %d，期望 %d", expectedMatches, expectedLegacyIdentityMatches)
	}
	for _, relative := range currentIdentityRoots {
		scanCurrentIdentityRoot(t, root, filepath.FromSlash(relative), actual, goScanner)
	}
	goScanner.scanConstants(t)
	for index, allowance := range legacyIdentityAllowances {
		if actual[index] != allowance.expected {
			t.Errorf("旧数据身份 allowlist 计数错误：%s %s.%s = %d，期望 %d", allowance.path, allowance.literal, allowance.owner, actual[index], allowance.expected)
		}
	}
	if build.Default.GOOS == "darwin" {
		testCurrentIdentityMutations(t)
	}
}

func requireLinuxServerBundleIdentity(t *testing.T, root string) {
	t.Helper()
	var linux *identityBuild
	for _, target := range supportedIdentityBuilds() {
		if target.name == "linux-server" {
			linux = &target
			break
		}
	}
	if linux == nil || linux.context.GOOS != "linux" || linux.context.GOARCH != "amd64" || !linux.context.CgoEnabled {
		t.Fatalf("Linux server identity context 必须是 linux/amd64 且启用 CGO: %+v", linux)
	}

	help := exec.Command("make", "help")
	help.Dir = root
	output, err := help.CombinedOutput()
	if err != nil {
		t.Fatalf("运行 make help: %v\n%s", err, output)
	}
	if !bytes.Contains(output, []byte("make build-linux-server 构建 Linux amd64 专服与同目录 Rust .so")) {
		t.Error("make help 未公开 canonical build-linux-server target")
	}

	workflow, err := os.ReadFile(filepath.Join(root, ".github", "workflows", "ci.yml"))
	if err != nil {
		t.Fatalf("读取 CI workflow: %v", err)
	}
	for _, required := range []string{"native-linux:", "runs-on: ubuntu-24.04", `make ci-native-linux CI_CANDIDATE_SHA="$GITHUB_SHA"`} {
		if !bytes.Contains(workflow, []byte(required)) {
			t.Errorf("CI workflow 缺少 Linux native bundle 标记 %q", required)
		}
	}
	if bytes.Contains(workflow, []byte("rg ")) {
		t.Error("CI workflow 不得依赖 runner 未安装的 rg")
	}
}

func testCurrentIdentityMutations(t *testing.T) {
	t.Helper()
	expectedOutput := map[string]string{
		"cross-package selector constant chain": "常量值包含未获允许",
		"build constrained constant chain":      "常量值包含未获允许",
		"unresolved import":                     "类型检查",
		"unresolved identifier":                 "类型检查",
		"escaped allowlisted owner":             "常量值包含未获允许",
	}
	mutations := map[string]func(*testing.T, string){
		"command path": func(t *testing.T, root string) {
			writeIdentityMutationFile(t, root, filepath.Join("packages", "tools", identityToken("mc", "go"), "main.go"), "package main\n")
		},
		"symlink path": func(t *testing.T, root string) {
			writeIdentityMutationFile(t, root, filepath.Join("packages", "tools", "target"), "package main\n")
			if err := os.Symlink("target", filepath.Join(root, "packages", "tools", identityToken("mc", "go-wrapper"))); err != nil {
				t.Fatal(err)
			}
		},
		"escaped literal": func(t *testing.T, root string) {
			writeIdentityMutationFile(t, root, filepath.Join("packages", "tools", "escaped.go"), `package sample
const artifact = "\x6d\x63\x67\x6f_mesh"
`)
		},
		"escaped import": func(t *testing.T, root string) {
			writeIdentityMutationFile(t, root, filepath.Join("packages", "tools", "escaped_import.go"), `package sample
import _ "github.com/channing771/minecraft\x2dgo/internal/core"
`)
		},
		"binary expression": func(t *testing.T, root string) {
			writeIdentityMutationFile(t, root, filepath.Join("packages", "tools", "binary.go"), `package sample
const artifact = "mc" + "go_mesh"
`)
		},
		"constant identifier chain": func(t *testing.T, root string) {
			writeIdentityMutationFile(t, root, filepath.Join("packages", "tools", "chain.go"), `package sample
const prefix = "mc"
const identity = prefix + "go"
const artifact = identity + "_mesh"
`)
		},
		"cross-file constant identifier chain": func(t *testing.T, root string) {
			writeIdentityMutationFile(t, root, filepath.Join("packages", "tools", "prefix.go"), `package sample
const prefix = "mc"
`)
			writeIdentityMutationFile(t, root, filepath.Join("packages", "tools", "artifact.go"), `package sample
const artifact = prefix + "go_mesh"
`)
		},
		"cross-package selector constant chain": func(t *testing.T, root string) {
			writeIdentityMutationFile(t, root, filepath.Join("packages", "tools", "helper", "pieces.go"), `package helper
const Prefix = "mc"
const Suffix = "go_mesh"
`)
			writeIdentityMutationFile(t, root, filepath.Join("packages", "tools", "consumer", "main.go"), `package consumer
import "github.com/channing771/mornlea/packages/tools/helper"
const artifact = helper.Prefix + helper.Suffix
`)
		},
		"build constrained constant chain": func(t *testing.T, root string) {
			writeIdentityMutationFile(t, root, filepath.Join("packages", "server", "cmd", "mornlea-server", "platform_darwin.go"), `//go:build darwin

package platform
const platformPrefix = "safe"
`)
			writeIdentityMutationFile(t, root, filepath.Join("packages", "server", "cmd", "mornlea-server", "platform_linux.go"), `//go:build linux

package platform
const platformPrefix = "mc"
`)
			writeIdentityMutationFile(t, root, filepath.Join("packages", "server", "cmd", "mornlea-server", "artifact.go"), `package platform
const artifact = platformPrefix + "go_mesh"
`)
		},
		"unresolved import": func(t *testing.T, root string) {
			writeIdentityMutationFile(t, root, filepath.Join("packages", "tools", "unresolved.go"), `package sample
import _ "example.invalid/missing"
`)
		},
		"unresolved identifier": func(t *testing.T, root string) {
			writeIdentityMutationFile(t, root, filepath.Join("packages", "tools", "unresolved.go"), `package sample
const artifact = missingIdentityPiece
`)
		},
		"escaped allowlisted owner": func(t *testing.T, root string) {
			writeIdentityMutationFile(t, root, filepath.Join("packages", "client", "cmd", "mornlea", "run_test.go"), `package main
func legacyDataPath() string { return "minecraft\x2dgo" }
`)
		},
		"invalid Go source": func(t *testing.T, root string) {
			writeIdentityMutationFile(t, root, filepath.Join("packages", "tools", "invalid.go"), "package sample\nfunc (")
		},
	}
	for name, setup := range mutations {
		t.Run(name, func(t *testing.T) {
			root := t.TempDir()
			setup(t, root)
			command := exec.Command(os.Args[0], "-test.run=^TestMornleaCurrentIdentity$")
			command.Env = append(os.Environ(), "MORNLEA_IDENTITY_TEST_ROOT="+root)
			output, err := command.CombinedOutput()
			if err == nil {
				t.Errorf("identity guard 接受了 mutation\n%s", output)
			} else if want := expectedOutput[name]; want != "" && !bytes.Contains(output, []byte(want)) {
				t.Errorf("identity guard 以错误原因拒绝 mutation，输出缺少 %q\n%s", want, output)
			}
		})
	}
}

func writeIdentityMutationFile(t *testing.T, root, relative, source string) {
	t.Helper()
	path := filepath.Join(root, relative)
	if err := os.MkdirAll(filepath.Dir(path), 0o700); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(path, []byte(source), 0o600); err != nil {
		t.Fatal(err)
	}
}

func scanCurrentIdentityRoot(t *testing.T, root, relative string, actual []int, goScanner *goIdentityScanner) {
	t.Helper()
	path := filepath.Join(root, relative)
	scanCurrentIdentityPath(t, relative)
	info, err := os.Lstat(path)
	if err != nil {
		t.Fatalf("读取身份扫描根 %s: %v", relative, err)
	}
	if info.Mode()&os.ModeSymlink != 0 {
		return
	}
	if !info.IsDir() {
		scanCurrentIdentityFile(t, root, path, actual, goScanner)
		return
	}

	files := 0
	err = filepath.WalkDir(path, func(path string, entry fs.DirEntry, walkErr error) error {
		if walkErr != nil {
			return walkErr
		}
		entryRelative, err := filepath.Rel(root, path)
		if err != nil {
			return err
		}
		// node_modules 是冻结安装（pnpm install --frozen-lockfile）产出的
		// 依赖仓库：gitignored、内容并非本仓库作者身份的一部分，第三方
		// 二进制里的字节不参与身份契约，整目录跳过。
		if entry.IsDir() && entry.Name() == "node_modules" {
			return fs.SkipDir
		}
		scanCurrentIdentityPath(t, filepath.ToSlash(entryRelative))
		if entry.IsDir() || entry.Type()&os.ModeSymlink != 0 {
			return nil
		}
		info, err := entry.Info()
		if err != nil {
			return err
		}
		if !info.Mode().IsRegular() {
			return nil
		}
		files++
		scanCurrentIdentityFile(t, root, path, actual, goScanner)
		return nil
	})
	if err != nil {
		t.Fatalf("扫描身份根 %s: %v", relative, err)
	}
	if files == 0 {
		t.Fatalf("身份扫描根 %s 没有普通文件", relative)
	}
}

func scanCurrentIdentityPath(t *testing.T, relative string) {
	t.Helper()
	for _, forbidden := range forbiddenCurrentIdentity {
		if strings.Contains(relative, forbidden) {
			t.Errorf("路径 %s 包含禁止的当前技术身份 %q", relative, forbidden)
		}
	}
	if match := legacyIdentityPattern.FindString(relative); match != "" {
		t.Errorf("路径 %s 包含未获允许的旧身份 %q", relative, match)
	}
}

func scanCurrentIdentityFile(t *testing.T, root, path string, actual []int, goScanner *goIdentityScanner) {
	t.Helper()
	source, err := os.ReadFile(path)
	if err != nil {
		t.Fatalf("读取 %s: %v", path, err)
	}
	relative, err := filepath.Rel(root, path)
	if err != nil {
		t.Fatalf("计算 %s 相对路径: %v", path, err)
	}
	relative = filepath.ToSlash(relative)
	var (
		parsed   *ast.File
		literals []sourceStringLiteral
	)
	if filepath.Ext(path) == ".go" {
		parsed, literals = parseSourceStringLiterals(t, goScanner.fileSet, path, source)
		scanGoImports(t, relative, goScanner.fileSet, parsed)
		goScanner.files[relative] = goIdentityFile{
			relative: relative,
			source:   source,
			parsed:   parsed,
		}
		goScanner.directories[filepath.Dir(relative)] = true
	}

	if filepath.Ext(path) == ".rs" {
		literals = parseRustCompatibilityConstants(source)
	}
	for _, forbidden := range forbiddenCurrentIdentity {
		if bytes.Contains(source, []byte(forbidden)) {
			t.Errorf("%s 包含禁止的当前技术身份 %q", relative, forbidden)
		}
	}
	matches := legacyIdentityPattern.FindAllIndex(source, -1)
	for _, match := range matches {
		consumed := false
		for index, allowance := range legacyIdentityAllowances {
			if allowance.path != relative {
				continue
			}
			for _, literal := range literals {
				if literal.canonical && literal.value == allowance.literal && literal.owner == allowance.owner && match[0] >= literal.start && match[1] <= literal.end {
					actual[index]++
					consumed = true
					break
				}
			}
			if consumed {
				break
			}
		}
		if !consumed {
			line := bytes.Count(source[:match[0]], []byte{'\n'}) + 1
			t.Errorf("%s:%d 包含未获允许的旧身份 %q", relative, line, source[match[0]:match[1]])
		}
	}
}

func parseSourceStringLiterals(t *testing.T, fileSet *token.FileSet, path string, source []byte) (*ast.File, []sourceStringLiteral) {
	t.Helper()
	parsed, err := parser.ParseFile(fileSet, path, source, 0)
	if err != nil {
		t.Fatalf("解析 %s: %v", path, err)
	}
	tokenFile := fileSet.File(parsed.Pos())
	var literals []sourceStringLiteral
	appendLiterals := func(node ast.Node, owner string) {
		ast.Inspect(node, func(node ast.Node) bool {
			literal, ok := node.(*ast.BasicLit)
			if !ok || literal.Kind != token.STRING {
				return true
			}
			value, err := strconv.Unquote(literal.Value)
			if err != nil {
				t.Fatalf("解析 %s 字符串 literal: %v", path, err)
			}
			literals = append(literals, sourceStringLiteral{
				value:     value,
				owner:     owner,
				start:     tokenFile.Offset(literal.Pos()),
				end:       tokenFile.Offset(literal.End()),
				canonical: literal.Value == strconv.Quote(value),
			})
			return true
		})
	}
	for _, declaration := range parsed.Decls {
		switch declaration := declaration.(type) {
		case *ast.FuncDecl:
			appendLiterals(declaration, declaration.Name.Name)
		case *ast.GenDecl:
			for _, specification := range declaration.Specs {
				value, ok := specification.(*ast.ValueSpec)
				if !ok || len(value.Names) == 0 {
					continue
				}
				appendLiterals(value, value.Names[0].Name)
			}
		}
	}
	return parsed, literals
}

func scanGoImports(t *testing.T, relative string, fileSet *token.FileSet, parsed *ast.File) {
	t.Helper()
	for _, specification := range parsed.Imports {
		path, err := strconv.Unquote(specification.Path.Value)
		if err != nil {
			t.Fatalf("解析 %s import path: %v", relative, err)
		}
		if containsLegacyIdentity(path) {
			t.Errorf("%s:%d 的 import path 包含未获允许的旧身份 %q", relative, fileSet.Position(specification.Path.Pos()).Line, path)
		}
	}
}

type identityBuild struct {
	name      string
	context   build.Context
	root      string
	fullRoots bool
}

func TestSupportedIdentityBuildsDoNotCrossCompileDarwinFullRoots(t *testing.T) {
	original := build.Default
	t.Cleanup(func() { build.Default = original })
	build.Default.GOOS = "linux"
	build.Default.GOARCH = "amd64"
	build.Default.CgoEnabled = true

	targets := supportedIdentityBuilds()
	if len(targets) != 1 || targets[0].name != "linux-server" || targets[0].fullRoots {
		t.Fatalf("Linux host identity builds = %+v，期望只检查 Linux server 闭包", targets)
	}
}

func supportedIdentityBuilds() []identityBuild {
	linux := build.Default
	linux.GOOS = "linux"
	linux.GOARCH = "amd64"
	linux.CgoEnabled = true
	linuxServer := identityBuild{name: "linux-server", context: linux, root: "packages/server/cmd/mornlea-server"}
	if build.Default.GOOS != "darwin" {
		return []identityBuild{linuxServer}
	}
	darwin := build.Default
	darwin.CgoEnabled = true
	return []identityBuild{{name: "darwin-cgo", context: darwin, fullRoots: true}, linuxServer}
}

func (scanner *goIdentityScanner) buildDirectories(target identityBuild) ([]string, error) {
	if target.fullRoots {
		directories := make([]string, 0, len(scanner.directories))
		for directory := range scanner.directories {
			directories = append(directories, directory)
		}
		sort.Strings(directories)
		return directories, nil
	}
	if !scanner.directories[target.root] {
		return nil, nil
	}
	selected := map[string]bool{target.root: true}
	queue := []string{target.root}
	for len(queue) > 0 {
		directory := queue[0]
		queue = queue[1:]
		files, err := scanner.matchingFiles(&target.context, directory, target.fullRoots)
		if err != nil {
			return nil, err
		}
		for _, file := range files {
			for _, specification := range file.parsed.Imports {
				path, err := strconv.Unquote(specification.Path.Value)
				if err != nil {
					return nil, fmt.Errorf("解析 %s import path: %w", file.relative, err)
				}
				if path != modulePath && !strings.HasPrefix(path, modulePath+"/") {
					continue
				}
				dependency := strings.TrimPrefix(strings.TrimPrefix(path, modulePath), "/")
				if dependency == "" {
					dependency = "."
				}
				if scanner.directories[dependency] && !selected[dependency] {
					selected[dependency] = true
					queue = append(queue, dependency)
				}
			}
		}
	}
	directories := make([]string, 0, len(selected))
	for directory := range selected {
		directories = append(directories, directory)
	}
	sort.Strings(directories)
	return directories, nil
}

func (scanner *goIdentityScanner) matchingFiles(context *build.Context, directory string, includeTests bool) ([]goIdentityFile, error) {
	var files []goIdentityFile
	for _, file := range scanner.files {
		if filepath.Dir(file.relative) != directory || !includeTests && strings.HasSuffix(file.relative, "_test.go") {
			continue
		}
		matched, err := context.MatchFile(filepath.Join(scanner.root, filepath.FromSlash(directory)), filepath.Base(file.relative))
		if err != nil {
			return nil, fmt.Errorf("匹配 %s 的 build constraints: %w", file.relative, err)
		}
		if matched {
			files = append(files, file)
		}
	}
	sort.Slice(files, func(i, j int) bool { return files[i].relative < files[j].relative })
	return files, nil
}

func (scanner *goIdentityScanner) externalImporter(context *build.Context, directories []string, includeTests bool) (types.Importer, error) {
	imports := make(map[string]bool)
	for _, directory := range directories {
		files, err := scanner.matchingFiles(context, directory, includeTests)
		if err != nil {
			return nil, err
		}
		for _, file := range files {
			for _, specification := range file.parsed.Imports {
				path, err := strconv.Unquote(specification.Path.Value)
				if err != nil {
					return nil, fmt.Errorf("解析 %s import path: %w", file.relative, err)
				}
				if path != "C" && path != modulePath && !strings.HasPrefix(path, modulePath+"/") {
					imports[path] = true
				}
			}
		}
	}
	paths := make([]string, 0, len(imports))
	for path := range imports {
		paths = append(paths, path)
	}
	sort.Strings(paths)
	if len(paths) == 0 {
		return importer.ForCompiler(scanner.fileSet, "gc", func(path string) (io.ReadCloser, error) {
			return nil, fmt.Errorf("构建集合没有外部 import，却请求了 %s", path)
		}), nil
	}

	// 导出数据经 go.work 解析：被类型检查的文件横跨 client/server 等多个
	// workspace 模块（根模块已解散，不存在能覆盖全部单元的单模块视图）；外部
	// 依赖版本由各模块 require 汇合出的 workspace 模块图唯一决定，与 replace
	// 全本地一样确定。
	arguments := append([]string{"list", "-e", "-export", "-deps", "-json"}, paths...)
	command := exec.Command("go", arguments...)
	command.Dir = scanner.root
	externalCgoEnabled := context.CgoEnabled
	if context.GOOS != build.Default.GOOS || context.GOARCH != build.Default.GOARCH {
		// identity 仍按目标平台的 CGO build constraints 选本仓库源码；外部导出数据
		// 不应在 macOS 上假装执行 Linux C toolchain，真实 bundle 由 Ubuntu CI 构建。
		externalCgoEnabled = false
	}
	command.Env = append(os.Environ(),
		"GOOS="+context.GOOS,
		"GOARCH="+context.GOARCH,
		fmt.Sprintf("CGO_ENABLED=%d", boolInt(externalCgoEnabled)),
	)
	output, err := command.Output()
	if err != nil {
		return nil, fmt.Errorf("go list 外部类型导出数据: %w", err)
	}
	type listedPackage struct {
		ImportPath string
		Export     string
		Error      *struct{ Err string }
	}
	exports := make(map[string]string)
	failures := make(map[string]string)
	decoder := json.NewDecoder(bytes.NewReader(output))
	for {
		var listed listedPackage
		if err := decoder.Decode(&listed); err != nil {
			if err == io.EOF {
				break
			}
			return nil, fmt.Errorf("解析 go list 外部类型导出数据: %w", err)
		}
		if listed.Export != "" {
			exports[listed.ImportPath] = listed.Export
		}
		if listed.Error != nil {
			failures[listed.ImportPath] = listed.Error.Err
		}
	}
	lookup := func(path string) (io.ReadCloser, error) {
		if failure := failures[path]; failure != "" {
			return nil, fmt.Errorf("解析 %s: %s", path, failure)
		}
		if export := exports[path]; export != "" {
			file, err := os.Open(export)
			if err != nil {
				return nil, fmt.Errorf("打开 %s 的导出数据: %w", path, err)
			}
			return file, nil
		}
		return nil, fmt.Errorf("go list 未提供 %s 的导出数据", path)
	}
	return importer.ForCompiler(scanner.fileSet, "gc", lookup), nil
}

func boolInt(value bool) int {
	if value {
		return 1
	}
	return 0
}

type identitySourceImporter struct {
	scanner  *goIdentityScanner
	context  *build.Context
	external types.Importer
	packages map[string]*types.Package
	loading  map[string]bool
}

func newIdentitySourceImporter(scanner *goIdentityScanner, context *build.Context, external types.Importer) *identitySourceImporter {
	return &identitySourceImporter{
		scanner:  scanner,
		context:  context,
		external: external,
		packages: make(map[string]*types.Package),
		loading:  make(map[string]bool),
	}
}

func (sourceImporter *identitySourceImporter) Import(path string) (*types.Package, error) {
	return sourceImporter.ImportFrom(path, "", 0)
}

func (sourceImporter *identitySourceImporter) ImportFrom(path, directory string, mode types.ImportMode) (*types.Package, error) {
	if path != modulePath && !strings.HasPrefix(path, modulePath+"/") {
		if importerFrom, ok := sourceImporter.external.(types.ImporterFrom); ok {
			return importerFrom.ImportFrom(path, directory, mode)
		}
		return sourceImporter.external.Import(path)
	}
	if loaded := sourceImporter.packages[path]; loaded != nil {
		return loaded, nil
	}
	if sourceImporter.loading[path] {
		return nil, fmt.Errorf("本模块 import cycle: %s", path)
	}
	relative := strings.TrimPrefix(path, modulePath)
	relative = strings.TrimPrefix(relative, "/")
	if relative == "" {
		relative = "."
	}
	files, err := sourceImporter.scanner.matchingFiles(sourceImporter.context, relative, false)
	if err != nil {
		return nil, err
	}
	if len(files) == 0 {
		return nil, fmt.Errorf("本模块 package %s 在 %s/%s 没有匹配的生产 Go 文件", path, sourceImporter.context.GOOS, sourceImporter.context.GOARCH)
	}
	packageName := files[0].parsed.Name.Name
	parsed := make([]*ast.File, 0, len(files))
	for _, file := range files {
		if file.parsed.Name.Name != packageName {
			return nil, fmt.Errorf("本模块 package %s 同时声明 %s 和 %s", path, packageName, file.parsed.Name.Name)
		}
		parsed = append(parsed, file.parsed)
	}
	sourceImporter.loading[path] = true
	defer delete(sourceImporter.loading, path)
	configuration := types.Config{
		Importer:    sourceImporter,
		FakeImportC: true,
		GoVersion:   "go1.26",
	}
	loaded, err := configuration.Check(path, sourceImporter.scanner.fileSet, parsed, nil)
	if err != nil {
		return nil, fmt.Errorf("类型检查 %s: %w", path, err)
	}
	sourceImporter.packages[path] = loaded
	return loaded, nil
}

func (scanner *goIdentityScanner) scanConstants(t *testing.T) {
	t.Helper()
	for _, target := range supportedIdentityBuilds() {
		directories, err := scanner.buildDirectories(target)
		if err != nil {
			t.Fatalf("准备 %s identity package 集合: %v", target.name, err)
		}
		if len(directories) == 0 {
			continue
		}
		external, err := scanner.externalImporter(&target.context, directories, target.fullRoots)
		if err != nil {
			t.Fatalf("准备 %s identity importer: %v", target.name, err)
		}
		for _, directory := range directories {
			sourceImporter := newIdentitySourceImporter(scanner, &target.context, external)
			files, err := scanner.matchingFiles(&target.context, directory, target.fullRoots)
			if err != nil {
				t.Fatalf("准备 %s/%s identity 文件: %v", target.name, directory, err)
			}
			packages := make(map[string][]goIdentityFile)
			for _, file := range files {
				packages[file.parsed.Name.Name] = append(packages[file.parsed.Name.Name], file)
			}
			packageNames := make([]string, 0, len(packages))
			for packageName := range packages {
				packageNames = append(packageNames, packageName)
			}
			sort.Strings(packageNames)
			for _, packageName := range packageNames {
				files := packages[packageName]
				parsed := make([]*ast.File, 0, len(files))
				for _, file := range files {
					parsed = append(parsed, file.parsed)
				}
				typeInfo := &types.Info{Types: make(map[ast.Expr]types.TypeAndValue)}
				packagePath := modulePath
				if directory != "." {
					packagePath += "/" + filepath.ToSlash(directory)
				}
				if strings.HasSuffix(packageName, "_test") {
					packagePath += "_test"
				}
				configuration := types.Config{
					Importer:    sourceImporter,
					FakeImportC: true,
					GoVersion:   "go1.26",
				}
				checked, err := configuration.Check(packagePath, scanner.fileSet, parsed, typeInfo)
				if err != nil {
					t.Fatalf("%s 类型检查 %s (%s): %v", target.name, directory, packageName, err)
				}
				if !strings.HasSuffix(packageName, "_test") {
					sourceImporter.packages[packagePath] = checked
				}
				for _, file := range files {
					scanGoConstantIdentities(t, file, scanner.fileSet, typeInfo)
				}
			}
		}
	}
}

func scanGoConstantIdentities(t *testing.T, file goIdentityFile, fileSet *token.FileSet, typeInfo *types.Info) {
	t.Helper()
	relative, source, parsed := file.relative, file.source, file.parsed
	definitionExpressions := make(map[ast.Expr]bool)
	ast.Inspect(parsed, func(node ast.Node) bool {
		value, ok := node.(*ast.ValueSpec)
		if !ok {
			return true
		}
		for _, expression := range value.Values {
			definitionExpressions[expression] = true
		}
		return true
	})

	inspect := func(node ast.Node, owner string) {
		ast.Inspect(node, func(node ast.Node) bool {
			expression, ok := node.(ast.Expr)
			if !ok {
				return true
			}
			typeAndValue, ok := typeInfo.Types[expression]
			if !ok || typeAndValue.Value == nil || typeAndValue.Value.Kind() != constant.String {
				return true
			}
			value := constant.StringVal(typeAndValue.Value)
			if !containsLegacyIdentity(value) {
				return true
			}
			tokenFile := fileSet.File(expression.Pos())
			start := tokenFile.Offset(expression.Pos())
			end := tokenFile.Offset(expression.End())
			if legacyIdentityPattern.Match(source[start:end]) {
				return true
			}
			if _, isIdentifier := expression.(*ast.Ident); isIdentifier && !definitionExpressions[expression] {
				return true
			}
			t.Errorf("%s:%d 的常量值包含未获允许的旧身份 %q", relative, fileSet.Position(expression.Pos()).Line, value)
			return true
		})
	}
	for _, declaration := range parsed.Decls {
		switch declaration := declaration.(type) {
		case *ast.FuncDecl:
			inspect(declaration, declaration.Name.Name)
		case *ast.GenDecl:
			for _, specification := range declaration.Specs {
				value, ok := specification.(*ast.ValueSpec)
				if !ok || len(value.Names) == 0 {
					continue
				}
				inspect(value, value.Names[0].Name)
			}
		}
	}
}

func containsLegacyIdentity(value string) bool {
	for _, forbidden := range forbiddenCurrentIdentity {
		if strings.Contains(value, forbidden) {
			return true
		}
	}
	return legacyIdentityPattern.MatchString(value)
}

// Rust compatibility identities are admitted only as a complete named const
// declaration before any string or block comment. This deliberately narrow
// grammar cannot grant an allowance to text inside another source construct.
func parseRustCompatibilityConstants(source []byte) []sourceStringLiteral {
	pattern := regexp.MustCompile(`(?m)^(?:pub )?const ([A-Z][A-Z_0-9]*): &str = ("[^"\\\n]*");$`)
	var literals []sourceStringLiteral
	for _, match := range pattern.FindAllSubmatchIndex(source, -1) {
		prefix := source[:match[0]]
		if bytes.Contains(prefix, []byte("/*")) || bytes.Contains(prefix, []byte{'"'}) {
			continue
		}
		value, err := strconv.Unquote(string(source[match[4]:match[5]]))
		if err != nil {
			continue
		}
		literals = append(literals, sourceStringLiteral{
			value: value, owner: string(source[match[2]:match[3]]),
			start: match[4], end: match[5], canonical: true,
		})
	}
	return literals
}

func TestRustCompatibilityIdentityLiterals(t *testing.T) {
	literal := strconv.Quote(legacyBackupIdentity)
	for _, test := range []struct {
		name   string
		source string
		want   int
	}{
		{"canonical", "pub const BACKUP_IDENTITY: &str = " + literal + ";\n", 1},
		{"line comment", "// const BACKUP_IDENTITY: &str = " + literal + ";\n", 0},
		{"block comment", "/*\npub const BACKUP_IDENTITY: &str = " + literal + ";\n*/", 0},
		{"raw body", "let text = r#\"\npub const BACKUP_IDENTITY: &str = " + literal + ";\n\"#;", 0},
		{"other expression", "let value = " + literal + ";\n", 0},
		{"raw string", `const BACKUP_IDENTITY: &str = r` + literal + ";\n", 0},
		{"computed", `const BACKUP_IDENTITY: &str = concat!(".mc", "go-world-backup-v1.json");` + "\n", 0},
	} {
		t.Run(test.name, func(t *testing.T) {
			got := parseRustCompatibilityConstants([]byte(test.source))
			if len(got) != test.want {
				t.Fatalf("literal count %d, want %d", len(got), test.want)
			}
			if test.want == 1 && (got[0].owner != "BACKUP_IDENTITY" || got[0].value != legacyBackupIdentity) {
				t.Fatal("compatibility owner or bytes changed")
			}
		})
	}
}
