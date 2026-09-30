package archcheck_test

import (
	"os"
	"path/filepath"
	"slices"
	"testing"
)

func TestCodeCommentLanguageScannerMutations(t *testing.T) {
	fixtures := []struct {
		name       string
		path       string
		clean      string
		mutations  []string
		wantLines  []int
		wantTokens []string
	}{
		{
			name:  "go",
			path:  "sample.go",
			clean: "package sample\nconst localized = `中文 // 不是注释`\n// English comment.\n",
			mutations: []string{
				"package sample\nconst localized = `中文 // 不是注释`\n// 中文行注释\n",
				"package sample\nconst localized = \"中文 /* 不是注释 */\"\n/* 中文块注释 */\n",
			},
			wantLines:  []int{3, 3},
			wantTokens: []string{"// 中文行注释", "/* 中文块注释 */"},
		},
		{
			name:  "rust",
			path:  "sample.rs",
			clean: "const LOCALIZED: &str = r#\"中文 // 不是注释\"#;\n// English comment.\n",
			mutations: []string{
				"const LOCALIZED: &str = r#\"中文 // 不是注释\"#;\n// 中文行注释\n",
				"const LOCALIZED: &str = \"中文 /* 不是注释 */\";\n/* outer /* 中文嵌套注释 */ block */\n",
			},
			wantLines:  []int{2, 2},
			wantTokens: []string{"// 中文行注释", "/* outer /* 中文嵌套注释 */ block */"},
		},
		{
			name:  "c",
			path:  "sample.c",
			clean: "const char *localized = \"中文 // 不是注释\";\n// English comment.\n",
			mutations: []string{
				"const char *localized = \"中文 // 不是注释\";\n// 中文行注释\n",
				"const char *localized = \"中文 /* 不是注释 */\";\n/* 中文块注释 */\n",
			},
			wantLines:  []int{2, 2},
			wantTokens: []string{"// 中文行注释", "/* 中文块注释 */"},
		},
		{
			name:  "gdscript",
			path:  "sample.gd",
			clean: "var localized = \"中文 # 不是注释\"\nvar multiline = \"\"\"中文\n# 仍然不是注释\"\"\"\n# English comment.\n",
			mutations: []string{
				"var localized = \"中文 # 不是注释\"\n# 中文行注释\n",
			},
			wantLines:  []int{2},
			wantTokens: []string{"# 中文行注释"},
		},
		{
			name:  "javascript",
			path:  "sample.js",
			clean: "const localized = \"中文 // 不是注释\";\nconst template = `中文 /* 不是注释 */`;\nconst matcher = /\\/\\/中文/;\n",
			mutations: []string{
				"const localized = \"中文 // 不是注释\";\n// 中文行注释\n",
				"const localized = `中文 /* 不是注释 */`;\n/* 中文块注释 */\n",
			},
			wantLines:  []int{2, 2},
			wantTokens: []string{"// 中文行注释", "/* 中文块注释 */"},
		},
		{
			name:  "typescript",
			path:  "sample.ts",
			clean: "const localized: string = '中文 // 不是注释';\n// English comment.\n",
			mutations: []string{
				"const localized: string = '中文 // 不是注释';\n// 中文行注释\n",
				"const localized: string = \"中文 /* 不是注释 */\";\n/* 中文块注释 */\n",
			},
			wantLines:  []int{2, 2},
			wantTokens: []string{"// 中文行注释", "/* 中文块注释 */"},
		},
	}

	for _, fixture := range fixtures {
		t.Run(fixture.name, func(t *testing.T) {
			clean, err := scanCodeCommentLanguage([]commentScanSource{{path: fixture.path, data: []byte(fixture.clean)}})
			if err != nil {
				t.Fatal(err)
			}
			if len(clean) != 0 {
				t.Fatalf("localized string literal was reported as a comment: %+v", clean)
			}

			for index, mutation := range fixture.mutations {
				findings, err := scanCodeCommentLanguage([]commentScanSource{{path: fixture.path, data: []byte(mutation)}})
				if err != nil {
					t.Fatal(err)
				}
				if len(findings) != 1 {
					t.Fatalf("mutation %d findings = %d, want 1: %+v", index, len(findings), findings)
				}
				if findings[0].line != fixture.wantLines[index] {
					t.Errorf("mutation %d line = %d, want %d", index, findings[0].line, fixture.wantLines[index])
				}
				if findings[0].text != fixture.wantTokens[index] {
					t.Errorf("mutation %d token = %q, want %q", index, findings[0].text, fixture.wantTokens[index])
				}
			}
		})
	}
}

func TestCommentScannerCollectsFirstPartySourcesAndSkipsDeclaredTrees(t *testing.T) {
	root := t.TempDir()
	files := map[string]string{
		"packages/core/main.go":                                 "package core\n",
		"packages/core/testdata/localized.ts":                   "const text = '中文';\n",
		"scripts/check.mjs":                                     "export const ok = true;\n",
		"apps/game/main.gd":                                     "extends Node\n",
		"packages/core/vendor/copied.go":                        "package copied\n",
		"packages/core/generated/schema.go":                     "package generated\n",
		"packages/engine/target/debug/build.rs":                 "fn main() {}\n",
		"packages/web/node_modules/library/index.ts":            "export {};\n",
		"packages/web/dist/index.js":                            "void 0;\n",
		"packages/core/licenses/copied_license.c":               "int copied;\n",
		"apps/game/.godot/imported/cache.gd":                    "extends Node\n",
		"apps/mornlea-godot/addons/py4godot/xterm.js":           "`unclosed third-party terminfo",
		"apps/mornlea-godot/addons/mornlea_tools/controller.gd": "# Project-owned plugin.\nextends Node\n",
		"apps/game/addons/py4godot/local.gd":                    "# Another app owns this source.\nextends Node\n",
		"outside/not_first_party.go":                            "package outside\n",
	}
	for path, content := range files {
		absolute := filepath.Join(root, filepath.FromSlash(path))
		if err := os.MkdirAll(filepath.Dir(absolute), 0o700); err != nil {
			t.Fatal(err)
		}
		if err := os.WriteFile(absolute, []byte(content), 0o600); err != nil {
			t.Fatal(err)
		}
	}

	sources, err := collectFirstPartyCommentSources(root)
	if err != nil {
		t.Fatal(err)
	}
	paths := make([]string, 0, len(sources))
	for _, source := range sources {
		paths = append(paths, source.path)
	}
	slices.Sort(paths)
	want := []string{
		"apps/game/addons/py4godot/local.gd",
		"apps/game/main.gd",
		"apps/mornlea-godot/addons/mornlea_tools/controller.gd",
		"packages/core/main.go",
		"packages/core/testdata/localized.ts",
		"scripts/check.mjs",
	}
	if !slices.Equal(paths, want) {
		t.Fatalf("collected paths = %v, want %v", paths, want)
	}
}

func TestCommentScannerTokenizesCurrentFirstPartySources(t *testing.T) {
	sources, err := collectFirstPartyCommentSources(repositoryRoot(t))
	if err != nil {
		t.Fatal(err)
	}
	covered := make(map[commentLanguage]bool)
	for _, source := range sources {
		language, ok := commentLanguageForPath(source.path)
		if !ok {
			t.Fatalf("collector returned unsupported source %s", source.path)
		}
		covered[language] = true
		if _, err := sourceCommentTokens(source); err != nil {
			t.Fatalf("tokenize %s: %v", source.path, err)
		}
	}
	for _, language := range []commentLanguage{commentLanguageGo, commentLanguageRust, commentLanguageC, commentLanguageJavaScript, commentLanguageTypeScript} {
		if !covered[language] {
			t.Errorf("repository scan did not cover %s", language)
		}
	}
}
