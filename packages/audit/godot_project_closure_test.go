package archcheck_test

import (
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"testing"
)

func TestGodotProjectRootRejectsEscapes(t *testing.T) {
	root := repositoryRoot(t)
	projectRoot := filepath.Join(root, "apps", "mornlea-godot")
	if output, err := runGodotProjectValidator(t, projectRoot); err != nil {
		t.Fatalf("validate the real Godot project:\n%s\n%v", output, err)
	}

	fixture := newGodotClosureFixture(t)
	external := filepath.Join(t.TempDir(), "outside.tres")
	writeGodotFixtureFile(t, external, "[gd_resource format=3]\n")
	link := filepath.Join(fixture, "features", "escaped.tres")
	if err := os.MkdirAll(filepath.Dir(link), 0o755); err != nil {
		t.Fatal(err)
	}
	if err := os.Symlink(external, link); err != nil {
		t.Fatal(err)
	}
	output, err := runGodotProjectValidator(t, fixture)
	if err == nil || !strings.Contains(output, "symbolic link escapes the project root") {
		t.Fatalf("validator accepted an escaping symbolic link:\n%s", output)
	}

	fixture = newGodotClosureFixture(t)
	writeGodotFixtureFile(t, filepath.Join(fixture, "features", "absolute.tres"),
		"[gd_resource format=3]\n[resource]\nmetadata/library = \"/Users/developer/libmornlea.dylib\"\n")
	output, err = runGodotProjectValidator(t, fixture)
	if err == nil || !strings.Contains(output, "absolute development-machine path") {
		t.Fatalf("validator accepted an absolute development-machine path:\n%s", output)
	}
}

func TestGodotResourceClosureRejectsExternalReferences(t *testing.T) {
	fixture := newGodotClosureFixture(t)
	writeGodotFixtureFile(t, filepath.Join(fixture, "features", "world", "feature.tres"),
		"[gd_resource format=3]\n[resource]\nmetadata/path = \"res://../outside.tres\"\n")
	output, err := runGodotProjectValidator(t, fixture)
	if err == nil || !strings.Contains(output, "resource path escapes the project root") {
		t.Fatalf("validator accepted a parent resource reference:\n%s", output)
	}

	fixture = newGodotClosureFixture(t)
	project := readGodotFixtureFile(t, filepath.Join(fixture, "project.godot"))
	project += "\n[autoload]\nGlobalState=\"*res://app/global_state.py\"\n"
	writeGodotFixtureFile(t, filepath.Join(fixture, "project.godot"), project)
	output, err = runGodotProjectValidator(t, fixture)
	if err == nil || !strings.Contains(output, "unregistered autoload") {
		t.Fatalf("validator accepted an unregistered autoload:\n%s", output)
	}
}

func TestGodotUIDPolicyKeepsGeneratedStateOutAndIdentityIn(t *testing.T) {
	root := repositoryRoot(t)
	ignore := readBaselineDoc(t, root, filepath.Join("apps", "mornlea-godot", ".gitignore"))
	if !strings.Contains(ignore, ".godot/") {
		t.Error("Godot generated state must be ignored")
	}
	if strings.Contains(ignore, ".uid") {
		t.Error("Godot UID sidecars must not be ignored")
	}
	if strings.Contains(ignore, "*.import") {
		t.Error("Godot resource import sidecars must not be ignored")
	}
	generatedProbe := filepath.Join("apps", "mornlea-godot", ".godot", "audit-probe")
	generatedCheck := exec.Command("git", "check-ignore", "--quiet", generatedProbe)
	generatedCheck.Dir = root
	if output, err := generatedCheck.CombinedOutput(); err != nil {
		t.Fatalf(".godot/ is not ignored by the effective Git rules: %s: %v", output, err)
	}
	uidProbe := filepath.Join("apps", "mornlea-godot", "app", "bootstrap", "audit-probe.gd.uid")
	uidCheck := exec.Command("git", "check-ignore", "--no-index", "--quiet", uidProbe)
	uidCheck.Dir = root
	if err := uidCheck.Run(); err == nil {
		t.Fatal("a parent or project Git rule ignores Godot UID sidecars")
	} else if exitError, ok := err.(*exec.ExitError); !ok || exitError.ExitCode() != 1 {
		t.Fatalf("inspect effective UID ignore policy: %v", err)
	}
	importProbe := filepath.Join("apps", "mornlea-godot", "assets", "generated", "fonts", "audit-probe.otf.import")
	importCheck := exec.Command("git", "check-ignore", "--no-index", "--quiet", importProbe)
	importCheck.Dir = root
	if err := importCheck.Run(); err == nil {
		t.Fatal("a parent or project Git rule ignores Godot resource import sidecars")
	} else if exitError, ok := err.(*exec.ExitError); !ok || exitError.ExitCode() != 1 {
		t.Fatalf("inspect effective import-sidecar ignore policy: %v", err)
	}
	for _, relative := range []string{
		"app/bootstrap/bootstrap.gd.uid",
		"app/bootstrap/setup_required.gd.uid",
		"addons/mornlea_bridge/mornlea_bridge.gdextension.uid",
	} {
		if info, err := os.Stat(filepath.Join(root, "apps", "mornlea-godot", filepath.FromSlash(relative))); err != nil || info.IsDir() {
			t.Errorf("tracked Godot identity sidecar %s is missing: %v", relative, err)
		}
	}

	fixture := newGodotClosureFixture(t)
	writeGodotFixtureFile(t, filepath.Join(fixture, ".gitignore"), ".godot/\n*.uid\n")
	output, err := runGodotProjectValidator(t, fixture)
	if err == nil || !strings.Contains(output, "UID sidecars must remain tracked") {
		t.Fatalf("validator accepted ignored UID sidecars:\n%s", output)
	}

	fixture = newGodotClosureFixture(t)
	writeGodotFixtureFile(t, filepath.Join(fixture, ".gitignore"), ".godot/\n*.import\n")
	output, err = runGodotProjectValidator(t, fixture)
	if err == nil || !strings.Contains(output, "Godot import sidecars must remain tracked") {
		t.Fatalf("validator accepted ignored Godot resource import sidecars:\n%s", output)
	}

	fixture = newGodotClosureFixture(t)
	writeGodotFixtureFile(t, filepath.Join(fixture, ".gitignore"), ".venv/\n")
	output, err = runGodotProjectValidator(t, fixture)
	if err == nil || !strings.Contains(output, ".godot/ must be ignored") {
		t.Fatalf("validator accepted generated Godot state without an ignore rule:\n%s", output)
	}
}

func TestGodotExportClosureExcludesDevelopmentResources(t *testing.T) {
	root := repositoryRoot(t)
	presets := readBaselineDoc(t, root, filepath.Join("apps", "mornlea-godot", "export_presets.cfg"))
	for _, required := range []string{
		`platform="macOS"`,
		"tests/**",
		"typing/**",
		"pyproject.toml",
		"uv.lock",
		"assets/provenance/**",
		"addons/mornlea_bridge/bin/linux-x86_64/**",
		"addons/mornlea_bridge/bin/windows-x86_64/**",
		"addons/py4godot/cpython-*-linux*/**",
		"addons/py4godot/cpython-*-windows*/**",
	} {
		if !strings.Contains(presets, required) {
			t.Errorf("macOS export closure is missing %q", required)
		}
	}

	fixture := newGodotClosureFixture(t)
	presets = strings.ReplaceAll(readGodotFixtureFile(t, filepath.Join(fixture, "export_presets.cfg")), "tests/**,", "")
	writeGodotFixtureFile(t, filepath.Join(fixture, "export_presets.cfg"), presets)
	output, err := runGodotProjectValidator(t, fixture)
	if err == nil || !strings.Contains(output, "export exclusion is missing: tests/**") {
		t.Fatalf("validator accepted tests in the export closure:\n%s", output)
	}

	fixture = newGodotClosureFixture(t)
	presets = strings.ReplaceAll(readGodotFixtureFile(t, filepath.Join(fixture, "export_presets.cfg")), "README*,", "README*,app/bootstrap/setup_required.*,")
	writeGodotFixtureFile(t, filepath.Join(fixture, "export_presets.cfg"), presets)
	output, err = runGodotProjectValidator(t, fixture)
	if err == nil || !strings.Contains(output, "export excludes required Bootstrap dependency") {
		t.Fatalf("validator accepted missing Bootstrap preload dependencies:\n%s", output)
	}

	fixture = newGodotClosureFixture(t)
	writeGodotFixtureFile(t, filepath.Join(fixture, "features", "unselected", "feature.tres"), "[gd_resource format=3]\n")
	writeGodotFixtureFile(t, filepath.Join(fixture, "config", "feature_catalog.tres"), "[gd_resource format=3]\n")
	output, err = runGodotProjectValidator(t, fixture)
	if err == nil || !strings.Contains(output, "feature is not selected by the export catalog") {
		t.Fatalf("validator accepted an unselected feature in the export closure:\n%s", output)
	}
}

func TestGodotDesktopOnlyRejectsUnsupportedTargets(t *testing.T) {
	for _, platform := range []string{"Android", "iOS", "Web", "Nintendo Switch"} {
		t.Run(platform+" export preset", func(t *testing.T) {
			fixture := newGodotClosureFixture(t)
			presets := strings.ReplaceAll(readGodotFixtureFile(t, filepath.Join(fixture, "export_presets.cfg")), `platform="macOS"`, `platform="`+platform+`"`)
			writeGodotFixtureFile(t, filepath.Join(fixture, "export_presets.cfg"), presets)
			output, err := runGodotProjectValidator(t, fixture)
			if err == nil || !strings.Contains(output, "unsupported export platform") {
				t.Fatalf("validator accepted a non-desktop export preset:\n%s", output)
			}
		})
	}

	for _, selector := range []string{"android.debug.arm64", "ios.debug.arm64", "web.debug.wasm32", "switch.debug.arm64"} {
		t.Run(selector+" GDExtension selector", func(t *testing.T) {
			fixture := newGodotClosureFixture(t)
			descriptorPath := filepath.Join(fixture, "addons", "mornlea_bridge", "mornlea_bridge.gdextension")
			descriptor := readGodotFixtureFile(t, descriptorPath)
			descriptor += "\n" + selector + " = \"res://addons/mornlea_bridge/bin/unsupported/libmornlea_godot.so\"\n"
			writeGodotFixtureFile(t, descriptorPath, descriptor)
			output, err := runGodotProjectValidator(t, fixture)
			if err == nil || !strings.Contains(output, "unsupported platform selector") {
				t.Fatalf("validator accepted a non-desktop GDExtension selector:\n%s", output)
			}
		})
	}

	for _, action := range []string{"touch_primary", "accelerometer_x"} {
		t.Run(action+" input action", func(t *testing.T) {
			fixture := newGodotClosureFixture(t)
			projectPath := filepath.Join(fixture, "project.godot")
			project := readGodotFixtureFile(t, projectPath)
			project += "\n[input]\n" + action + "={\n}\n"
			writeGodotFixtureFile(t, projectPath, project)
			output, err := runGodotProjectValidator(t, fixture)
			if err == nil || !strings.Contains(output, "unsupported touch or mobile-sensor input") {
				t.Fatalf("validator accepted a non-desktop input action:\n%s", output)
			}
		})
	}

	fixture := newGodotClosureFixture(t)
	writeGodotFixtureFile(t, filepath.Join(fixture, "platform", "mobile", "lifecycle.py"), "from __future__ import annotations\n")
	output, err := runGodotProjectValidator(t, fixture)
	if err == nil || !strings.Contains(output, "unsupported mobile lifecycle or platform adapter") {
		t.Fatalf("validator accepted a mobile lifecycle adapter:\n%s", output)
	}
}

func TestGodotPythonBoundaryRejectsAmbientRuntimeResolution(t *testing.T) {
	mutations := []struct {
		name     string
		source   string
		expected string
	}{
		{name: "runtime installer", source: "from __future__ import annotations\nimport pip\n", expected: "forbidden Python runtime dependency"},
		{name: "runtime download", source: "from __future__ import annotations\nimport urllib.request\n", expected: "forbidden Python runtime dependency"},
		{name: "ambient path mutation", source: "from __future__ import annotations\nimport sys\nsys.path.append('/tmp')\n", expected: "forbidden Python runtime search"},
		{name: "system Python path", source: "from __future__ import annotations\nimport sys\nsys.path.append('/usr/local/lib/python3.14')\n", expected: "forbidden Python runtime search"},
		{name: "user site-package dependency", source: "from __future__ import annotations\nimport site\nsite.getusersitepackages()\n", expected: "forbidden Python runtime search"},
		{name: "runtime installer call", source: "from __future__ import annotations\nimport importlib\nimportlib.import_module('pip').main(['install', 'example'])\n", expected: "forbidden Python runtime search"},
		{name: "non-English comment", source: "from __future__ import annotations\n# 禁止的架构注释。\n", expected: "non-English source comment"},
	}
	for _, mutation := range mutations {
		t.Run(mutation.name, func(t *testing.T) {
			fixture := newGodotClosureFixture(t)
			writeGodotFixtureFile(t, filepath.Join(fixture, "features", "world", "world_feature.py"), mutation.source)
			output, err := runGodotProjectValidator(t, fixture)
			if err == nil || !strings.Contains(output, mutation.expected) {
				t.Fatalf("validator accepted %s:\n%s", mutation.name, output)
			}
		})
	}

	fixture := newGodotClosureFixture(t)
	writeGodotFixtureFile(t, filepath.Join(fixture, "app", "bootstrap", "illegal.gd"), "extends Node\n")
	output, err := runGodotProjectValidator(t, fixture)
	if err == nil || !strings.Contains(output, "production GDScript is outside the bootstrap allowlist") {
		t.Fatalf("validator accepted production GDScript outside Bootstrap:\n%s", output)
	}

	fixture = newGodotClosureFixture(t)
	bootstrapPath := filepath.Join(fixture, "app", "bootstrap", "bootstrap.gd")
	bootstrap := readGodotFixtureFile(t, bootstrapPath) + "var downloader := HTTPRequest.new()\n"
	writeGodotFixtureFile(t, bootstrapPath, bootstrap)
	output, err = runGodotProjectValidator(t, fixture)
	if err == nil || !strings.Contains(output, "forbidden Godot runtime network") {
		t.Fatalf("validator accepted a Bootstrap runtime download path:\n%s", output)
	}
}

func runGodotProjectValidator(t *testing.T, projectRoot string, arguments ...string) (string, error) {
	t.Helper()
	validator := filepath.Join(repositoryRoot(t), "scripts", "godot", "validate-project.sh")
	command := exec.Command(validator, arguments...)
	command.Env = append(os.Environ(), "MORNLEA_GODOT_PROJECT_ROOT="+projectRoot)
	output, err := command.CombinedOutput()
	return string(output), err
}

// newGodotClosureFixture keeps mutation tests independent from generated
// runtimes while preserving every source-owned project boundary under review.
func newGodotClosureFixture(t *testing.T) string {
	t.Helper()
	root := t.TempDir()
	files := map[string]string{
		".gitignore":    ".godot/\n.venv/\n.ruff_cache/\naddons/mornlea_bridge/bin/\naddons/py4godot/\n__pycache__/\n*.py[cod]\n",
		"project.godot": "config_version=5\n\n[application]\nrun/main_scene=\"res://app/bootstrap/bootstrap.tscn\"\n",
		"export_presets.cfg": `[preset.0]

name="Mornlea macOS"
platform="macOS"
runnable=true
export_filter="all_resources"
include_filter="app/**/*.py,features/**/*.py,platform/desktop/**/*.py,addons/mornlea_bridge/*.py,assets/generated/**"
exclude_filter="tests/**,typing/**,.venv/**,.ruff_cache/**,pyproject.toml,uv.lock,README*,assets/provenance/**,assets/generated/**/*.provenance.json,assets/generated/**/PROVENANCE.json,addons/mornlea_bridge/bin/linux-x86_64/**,addons/mornlea_bridge/bin/windows-x86_64/**,addons/py4godot/cpython-*-linux*/**,addons/py4godot/cpython-*-windows*/**"
export_path=""

[preset.0.options]

custom_template/debug=""
custom_template/release=""
`,
		"app/bootstrap/bootstrap.gd":                           "extends Node\n# Bootstrap owns dependency diagnostics before Python can load.\n",
		"app/bootstrap/bootstrap.gd.uid":                       "uid://bootstrapfixture\n",
		"app/bootstrap/setup_required.gd":                      "extends Control\n# Setup diagnostics never own gameplay behavior.\n",
		"app/bootstrap/setup_required.gd.uid":                  "uid://setupfixture\n",
		"addons/mornlea_bridge/mornlea_bridge.gdextension":     "[configuration]\nentry_symbol = \"gdext_rust_init\"\n\n[libraries]\nmacos.debug.arm64 = \"res://addons/mornlea_bridge/bin/macos-universal/debug/libmornlea_godot.dylib\"\n",
		"addons/mornlea_bridge/mornlea_bridge.gdextension.uid": "uid://bridgefixture\n",
		"app/host/app_root.py":                                 "from __future__ import annotations\n# Python owns feature lifecycle after Bootstrap hands off.\n",
	}
	for relative, contents := range files {
		writeGodotFixtureFile(t, filepath.Join(root, filepath.FromSlash(relative)), contents)
	}
	return root
}

func writeGodotFixtureFile(t *testing.T, path string, contents string) {
	t.Helper()
	if err := os.MkdirAll(filepath.Dir(path), 0o755); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(path, []byte(contents), 0o600); err != nil {
		t.Fatal(err)
	}
}

func readGodotFixtureFile(t *testing.T, path string) string {
	t.Helper()
	contents, err := os.ReadFile(path)
	if err != nil {
		t.Fatal(err)
	}
	return string(contents)
}
