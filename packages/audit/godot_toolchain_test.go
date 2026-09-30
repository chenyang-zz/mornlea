package archcheck_test

import (
	"archive/zip"
	"bufio"
	"bytes"
	"crypto/sha256"
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"regexp"
	"runtime"
	"strings"
	"testing"
)

var sha256Pattern = regexp.MustCompile(`^[0-9a-f]{64}$`)

func TestGodotResolverPrintsCachedExecutablePath(t *testing.T) {
	fixture := t.TempDir()
	scriptDir := filepath.Join(fixture, "scripts", "godot")
	sourceRoot := repositoryRoot(t)
	writeFile(t, filepath.Join(scriptDir, "version.env"), []byte(readBaselineDoc(t, sourceRoot, "scripts/godot/version.env")))
	resolver := filepath.Join(scriptDir, "godot.sh")
	writeExecutable(t, resolver, readBaselineDoc(t, sourceRoot, "scripts/godot/godot.sh"))
	cache := filepath.Join(fixture, "cache")
	editorRelative := "4.7.2-stable/darwin-universal/Godot.app/Contents/MacOS/Godot"
	if runtime.GOOS == "linux" {
		editorRelative = "4.7.2-stable/linux-x86_64/Godot_v4.7.2-stable_linux.x86_64"
	}
	editor := filepath.Join(cache, editorRelative)
	writeExecutable(t, editor, "#!/bin/sh\ncase \"$1\" in --version) printf '4.7.2.stable.fixture\\n' ;; *) printf 'unexpected editor invocation\\n' ;; esac\n")
	command := exec.Command(resolver, "--print-path")
	command.Env = append(os.Environ(), "MORNLEA_GODOT_CACHE_DIR="+cache, "MORNLEA_GODOT_BIN=")
	output, err := command.CombinedOutput()
	if err != nil || strings.TrimSpace(string(output)) != editor {
		t.Fatalf("Godot resolver did not return the validated executable path: %v\n%s", err, output)
	}
}

func TestGodotExportProbeResolvesCachedEditor(t *testing.T) {
	if runtime.GOOS != "darwin" || runtime.GOARCH != "arm64" {
		t.Skip("embedded Python export qualification targets macOS arm64")
	}
	fixture := t.TempDir()
	root := filepath.Join(fixture, "repository")
	scriptDir := filepath.Join(root, "scripts", "godot")
	sourceRoot := repositoryRoot(t)
	for _, relative := range []string{
		"scripts/godot/python-runtime-check.sh",
		"scripts/godot/godot.sh",
		"scripts/godot/python-version.env",
		"scripts/godot/py4godot/build-inputs.env",
		"scripts/godot/version.env",
	} {
		writeFile(t, filepath.Join(root, relative), []byte(readBaselineDoc(t, sourceRoot, relative)))
	}
	writeExecutable(t, filepath.Join(scriptDir, "python-runtime-check.sh"), readBaselineDoc(t, sourceRoot, "scripts/godot/python-runtime-check.sh"))
	writeExecutable(t, filepath.Join(scriptDir, "godot.sh"), readBaselineDoc(t, sourceRoot, "scripts/godot/godot.sh"))
	writeExecutable(t, filepath.Join(scriptDir, "build-python-runtime.sh"), "#!/bin/sh\nexit 0\n")
	python := filepath.Join(root, "apps/mornlea-godot/addons/py4godot/cpython-3.14.4-darwin64/python/bin/python3.14")
	writeExecutable(t, python, "#!/bin/sh\ncase \"$*\" in *'-m pip'*) exit 1 ;; esac\nprintf 'arm64 3.14.4 1\\n'\n")
	pythonCache := filepath.Join(fixture, "python-cache")
	if err := os.MkdirAll(pythonCache, 0o700); err != nil {
		t.Fatal(err)
	}
	godotCache := filepath.Join(fixture, "godot-cache")
	editor := filepath.Join(godotCache, "4.7.2-stable/darwin-universal/Godot.app/Contents/MacOS/Godot")
	writeExecutable(t, editor, "#!/bin/sh\nprintf 'fixture.invalid\\n'\n")
	command := exec.Command(filepath.Join(scriptDir, "python-runtime-check.sh"), "--exported", "--offline")
	command.Env = append(os.Environ(), "MORNLEA_PY4GODOT_CACHE_DIR="+pythonCache, "MORNLEA_GODOT_CACHE_DIR="+godotCache, "MORNLEA_GODOT_BIN=")
	outputBytes, err := command.CombinedOutput()
	output := string(outputBytes)
	if err == nil || !strings.Contains(output, "Godot version mismatch: got fixture.invalid") {
		t.Fatalf("export probe did not resolve the cached editor before qualification: %v\n%s", err, output)
	}
}

func TestGodotIsolatedProbeUsesCachedEditorWithScrubbedPath(t *testing.T) {
	if runtime.GOOS != "darwin" || runtime.GOARCH != "arm64" {
		t.Skip("embedded Python qualification targets macOS arm64")
	}
	fixture := t.TempDir()
	root := filepath.Join(fixture, "repository")
	scriptDir := filepath.Join(root, "scripts", "godot")
	sourceRoot := repositoryRoot(t)
	for _, relative := range []string{
		"scripts/godot/python-runtime-check.sh",
		"scripts/godot/godot.sh",
		"scripts/godot/python-version.env",
		"scripts/godot/py4godot/build-inputs.env",
		"scripts/godot/version.env",
	} {
		writeFile(t, filepath.Join(root, relative), []byte(readBaselineDoc(t, sourceRoot, relative)))
	}
	writeExecutable(t, filepath.Join(scriptDir, "python-runtime-check.sh"), readBaselineDoc(t, sourceRoot, "scripts/godot/python-runtime-check.sh"))
	writeExecutable(t, filepath.Join(scriptDir, "godot.sh"), readBaselineDoc(t, sourceRoot, "scripts/godot/godot.sh"))
	writeExecutable(t, filepath.Join(scriptDir, "build-python-runtime.sh"), "#!/bin/sh\nexit 0\n")
	writeExecutable(t, filepath.Join(scriptDir, "build-extension.sh"), "#!/bin/sh\nexit 0\n")
	python := filepath.Join(root, "apps/mornlea-godot/addons/py4godot/cpython-3.14.4-darwin64/python/bin/python3.14")
	writeExecutable(t, python, "#!/bin/sh\ncase \"$*\" in *'-m pip'*) exit 1 ;; esac\nprintf 'arm64 3.14.4 1\\n'\n")
	pythonCache := filepath.Join(fixture, "python-cache")
	if err := os.MkdirAll(pythonCache, 0o700); err != nil {
		t.Fatal(err)
	}
	godotCache := filepath.Join(fixture, "godot-cache")
	editor := filepath.Join(godotCache, "4.7.2-stable/darwin-universal/Godot.app/Contents/MacOS/Godot")
	writeExecutable(t, editor, "#!/bin/sh\ncase \"$*\" in *'--version'*) printf '4.7.2.stable.fixture\\n' ;; *) printf 'Python bridge host check passed.\\n' ;; esac\n")
	bin := filepath.Join(fixture, "bin")
	writeExecutable(t, filepath.Join(bin, "sandbox-exec"), "#!/bin/sh\n[ \"$1\" = '-p' ] || exit 2\nshift 2\nexec \"$@\"\n")
	command := exec.Command(filepath.Join(scriptDir, "python-runtime-check.sh"), "--coexistence", "--offline")
	command.Env = append(os.Environ(), "PATH="+bin+":"+os.Getenv("PATH"), "MORNLEA_PY4GODOT_CACHE_DIR="+pythonCache, "MORNLEA_GODOT_CACHE_DIR="+godotCache, "MORNLEA_GODOT_BIN=")
	outputBytes, err := command.CombinedOutput()
	if err != nil || !strings.Contains(string(outputBytes), "Py4Godot and mornlea_godot coexist") {
		t.Fatalf("isolated probe did not execute the cached editor with a scrubbed PATH: %v\n%s", err, outputBytes)
	}
}

func TestGodotFetchMaterializesVerifiedEditor(t *testing.T) {
	for _, test := range []struct {
		name, entry                       string
		mode                              os.FileMode
		verifyOnly, previous, wantSuccess bool
		failPublication, failRecovery     bool
	}{
		{"cold cache publishes executable", "Godot.app/Contents/MacOS/Godot", 0o755, false, false, true, false, false},
		{"verified archive replaces old editor", "Godot.app/Contents/MacOS/Godot", 0o755, false, true, true, false, false},
		{"missing editor is rejected", "README", 0o644, false, false, false, false, false},
		{"nonexecutable editor is rejected", "Godot.app/Contents/MacOS/Godot", 0o644, false, false, false, false, false},
		{"symlink application is rejected", "Godot.app", os.ModeSymlink | 0o755, false, false, false, false, false},
		{"invalid editor preserves old editor", "README", 0o644, false, true, false, false, false},
		{"verify only never extracts", "Godot.app/Contents/MacOS/Godot", 0o755, true, false, true, false, false},
		{"publication failure restores previous editor", "Godot.app/Contents/MacOS/Godot", 0o755, false, true, false, true, false},
		{"recovery failure retains previous editor in staging", "Godot.app/Contents/MacOS/Godot", 0o755, false, true, false, true, true},
	} {
		t.Run(test.name, func(t *testing.T) {
			fixture := t.TempDir()
			script := filepath.Join(fixture, "repository/scripts/godot/fetch.sh")
			writeExecutable(t, script, readBaselineDoc(t, repositoryRoot(t), "scripts/godot/fetch.sh"))
			var archive bytes.Buffer
			writer := zip.NewWriter(&archive)
			header := &zip.FileHeader{Name: test.entry, Method: zip.Store}
			header.SetMode(test.mode)
			entry, err := writer.CreateHeader(header)
			if err != nil {
				t.Fatal(err)
			}
			editor := []byte("#!/bin/sh\nexit 0\n")
			payload := editor
			if test.mode&os.ModeSymlink != 0 {
				target := filepath.Join(fixture, "foreign-Godot.app")
				writeExecutable(t, filepath.Join(target, "Contents/MacOS/Godot"), string(editor))
				payload = []byte(target)
			}
			if _, err := entry.Write(payload); err != nil {
				t.Fatal(err)
			}
			if err := writer.Close(); err != nil {
				t.Fatal(err)
			}
			archivePath := filepath.Join(fixture, "editor.zip")
			templatesPath := filepath.Join(fixture, "templates.tpz")
			writeFile(t, archivePath, archive.Bytes())
			templates := []byte("verified templates fixture")
			writeFile(t, templatesPath, templates)
			pins := fmt.Sprintf("GODOT_VERSION=fixture\nGODOT_MACOS_UNIVERSAL_URL=file://%s\nGODOT_MACOS_UNIVERSAL_SHA256=%x\nGODOT_EXPORT_TEMPLATES_URL=file://%s\nGODOT_EXPORT_TEMPLATES_SHA256=%x\n", archivePath, sha256.Sum256(archive.Bytes()), templatesPath, sha256.Sum256(templates))
			writeFile(t, filepath.Join(filepath.Dir(script), "version.env"), []byte(pins))
			cache := filepath.Join(fixture, "cache")
			artifact := filepath.Join(cache, "fixture/darwin-universal")
			installed := filepath.Join(artifact, "Godot.app/Contents/MacOS/Godot")
			if test.previous {
				writeExecutable(t, installed, "previous editor")
			}
			arguments := []string{"--target", "darwin-universal", "--cache-dir", cache}
			if test.verifyOnly {
				writeFile(t, filepath.Join(artifact, "Godot_vfixture_macos.universal.zip"), archive.Bytes())
				writeFile(t, filepath.Join(artifact, "Godot_vfixture_export_templates.tpz"), templates)
				arguments = append(arguments, "--verify-only")
			}
			command := exec.Command(script, arguments...)
			bin := t.TempDir()
			faultLog := filepath.Join(fixture, "move-faults")
			command.Env = append(os.Environ(), "PATH="+bin+":"+os.Getenv("PATH"))
			if _, err := exec.LookPath("ditto"); err != nil {
				// Linux policy CI exercises publication with a real ZIP extractor;
				// macOS runs the qualified application extractor directly.
				writeExecutable(t, filepath.Join(bin, "ditto"), "#!/usr/bin/env bash\nset -euo pipefail\n[[ $# -eq 4 && $1 == -x && $2 == -k ]]\nexec unzip -q \"$3\" -d \"$4\"\n")
			}
			if test.failPublication {
				realMove, err := exec.LookPath("mv")
				if err != nil {
					t.Fatal(err)
				}
				// Keep download and backup moves real; inject I/O failures only
				// when publishing the staged editor or restoring its predecessor.
				writeExecutable(t, filepath.Join(bin, "mv"), fmt.Sprintf(`#!/usr/bin/env bash
set -euo pipefail
if [[ $# -eq 3 && $1 == -- ]]; then
  case "$2" in
    */.godot-extract.*/Godot.app)
      [[ -f "${2%%/Godot.app}/previous-Godot.app/Contents/MacOS/Godot" ]]
      printf 'publication\n' >> %q
      exit 73
      ;;
    */.godot-extract.*/previous-Godot.app)
      printf 'recovery\n' >> %q
      if %t; then exit 74; fi
      ;;
  esac
fi
exec %q "$@"
`, faultLog, faultLog, test.failRecovery, realMove))
			}
			output, err := command.CombinedOutput()
			if (err == nil) != test.wantSuccess {
				t.Fatalf("fetch success=%t: %v\n%s", test.wantSuccess, err, output)
			}
			if test.failPublication {
				if got := string(readFile(t, faultLog)); got != "publication\nrecovery\n" {
					t.Fatalf("fault fixture did not reach publication and recovery: %q\n%s", got, output)
				}
			}
			if test.verifyOnly || (!test.wantSuccess && !test.previous) || test.failRecovery {
				if _, err := os.Stat(filepath.Join(artifact, "Godot.app")); !os.IsNotExist(err) {
					t.Fatalf("fetch published an unqualified editor: %v\n%s", err, output)
				}
			} else {
				want := editor
				if !test.wantSuccess {
					want = []byte("previous editor")
				}
				if got := readFile(t, installed); !bytes.Equal(got, want) {
					t.Fatalf("installed editor = %q, want %q", got, want)
				}
				info, err := os.Stat(installed)
				if err != nil || info.Mode()&0o111 == 0 {
					t.Fatalf("installed editor is not executable: %v", err)
				}
			}
			entries, err := os.ReadDir(artifact)
			if err != nil {
				t.Fatal(err)
			}
			retainedStage := ""
			if test.failRecovery {
				matches, err := filepath.Glob(filepath.Join(artifact, ".godot-extract.*", "previous-Godot.app"))
				if err != nil || len(matches) != 1 {
					t.Fatalf("expected one retained editor: %v %v\n%s", matches, err, output)
				}
				retainedStage = filepath.Dir(matches[0])
				if got := readFile(t, filepath.Join(matches[0], "Contents/MacOS/Godot")); string(got) != "previous editor" {
					t.Fatalf("retained editor = %q, want prior bytes", got)
				}
				recoveryPath, err := filepath.EvalSymlinks(matches[0])
				if err != nil {
					t.Fatal(err)
				}
				want := "godot fetch: editor recovery remains at " + recoveryPath + "\n"
				if strings.Count(string(output), want) != 1 {
					t.Fatalf("recovery diagnostic missing or duplicated: want %q\n%s", want, output)
				}
			}
			for _, entry := range entries {
				if strings.HasPrefix(entry.Name(), ".godot-extract.") && filepath.Join(artifact, entry.Name()) != retainedStage {
					t.Fatalf("fetch left staging data behind: %s", entry.Name())
				}
			}
		})
	}
}

func TestGodotDesktopOnlyToolchainPin(t *testing.T) {
	root := repositoryRoot(t)
	envPath := filepath.Join(root, "scripts", "godot", "version.env")
	values := readGodotVersionEnvironment(t, envPath)
	want := map[string]string{
		"GODOT_VERSION":                 "4.7.2-stable",
		"GODOT_MACOS_UNIVERSAL_URL":     "https://github.com/godotengine/godot/releases/download/4.7.2-stable/Godot_v4.7.2-stable_macos.universal.zip",
		"GODOT_MACOS_UNIVERSAL_SHA256":  "c58a24e31d720be9d62f60cb5627c4e695fb72f21b0cfe1bc9ccaa9a3b3ba63e",
		"GODOT_EXPORT_TEMPLATES_URL":    "https://github.com/godotengine/godot/releases/download/4.7.2-stable/Godot_v4.7.2-stable_export_templates.tpz",
		"GODOT_EXPORT_TEMPLATES_SHA256": "f298490b8d44d934be425a5a65a51bf15f422428b229a06a6e11d9ffea248011",
	}
	for key, expected := range want {
		if got := values[key]; got != expected {
			t.Errorf("%s = %q, want %q", key, got, expected)
		}
	}
	for key, value := range values {
		upper := strings.ToUpper(key + "=" + value)
		for _, forbidden := range []string{"ANDROID", "IOS", "MOBILE", "WEB", "CONSOLE"} {
			if strings.Contains(upper, forbidden) {
				t.Errorf("desktop-only Godot pin contains forbidden target %q in %s", forbidden, key)
			}
		}
		if strings.HasSuffix(key, "_SHA256") && !sha256Pattern.MatchString(value) {
			t.Errorf("%s is not a lowercase SHA-256 digest", key)
		}
	}

	fetchPath := filepath.Join(root, "scripts", "godot", "fetch.sh")
	info, err := os.Stat(fetchPath)
	if err != nil {
		t.Fatal(err)
	}
	if info.Mode().Perm()&0o111 == 0 {
		t.Errorf("%s must be executable", fetchPath)
	}
	fetch := readBaselineDoc(t, root, filepath.Join("scripts", "godot", "fetch.sh"))
	for _, required := range []string{"darwin-universal", "--verify-only", "MORNLEA_GODOT_CACHE_DIR", "unsupported Godot desktop target"} {
		if !strings.Contains(fetch, required) {
			t.Errorf("fetch.sh is missing %q", required)
		}
	}
}

func TestGodotRustDependencyPin(t *testing.T) {
	root := repositoryRoot(t)
	workspace := readBaselineDoc(t, root, filepath.Join("packages", "engine", "Cargo.toml"))
	if !strings.Contains(workspace, `"crates/mornlea_godot"`) {
		t.Error("Rust workspace does not include crates/mornlea_godot")
	}
	manifest := readBaselineDoc(t, root, filepath.Join("packages", "engine", "crates", "mornlea_godot", "Cargo.toml"))
	for _, required := range []string{
		`name = "mornlea_godot"`,
		`crate-type = ["rlib", "cdylib"]`,
		`version = "=0.5.5"`,
		`features = ["api-4-7"]`,
	} {
		if !strings.Contains(manifest, required) {
			t.Errorf("mornlea_godot Cargo.toml is missing %q", required)
		}
	}
}

func TestGodotPythonRuntimePin(t *testing.T) {
	root := repositoryRoot(t)
	envPath := filepath.Join(root, "scripts", "godot", "python-version.env")
	values := readGodotVersionEnvironment(t, envPath)
	want := map[string]string{
		"PY4GODOT_VERSION":                     "4.7-alpha21",
		"PY4GODOT_SOURCE_REVISION":             "d8e17428deeb0428587349b663f6da26cd71ef3a",
		"PY4GODOT_RELEASE_URL":                 "https://github.com/niklas2902/py4godot/releases/download/4.7-alpha21/py4godot.zip",
		"PY4GODOT_RELEASE_SHA256":              "7fa28db6e5614523a4a9e092ca2e75fb76dfa1f4c385aaf6dd624eb33bf3eb4c",
		"PY4GODOT_ARCHIVE_ROOT":                "install_dir/addons/py4godot",
		"PY4GODOT_GDEXTENSION_VERSION":         "4.7-alpha-21",
		"PY4GODOT_GODOT_COMPATIBILITY_MINIMUM": "4.7.0",
		"PY4GODOT_ENTRY_SYMBOL":                "initialize_pythonscript",
		"PY4GODOT_CPYTHON_VERSION":             "3.14.4",
		"PY4GODOT_TARGET":                      "darwin-arm64",
	}
	for key, expected := range want {
		if got := values[key]; got != expected {
			t.Errorf("%s = %q, want %q", key, got, expected)
		}
	}
	for key, value := range values {
		upper := strings.ToUpper(key + "=" + value)
		for _, forbidden := range []string{"ANDROID", "IOS", "MOBILE", "WEB", "CONSOLE", "WINDOWS", "LINUX"} {
			if strings.Contains(upper, forbidden) {
				t.Errorf("current macOS Python pin contains forbidden target %q in %s", forbidden, key)
			}
		}
		if strings.HasSuffix(key, "_SHA256") && !sha256Pattern.MatchString(value) {
			t.Errorf("%s is not a lowercase SHA-256 digest", key)
		}
	}

	checkPath := filepath.Join(root, "scripts", "godot", "python-runtime-check.sh")
	info, err := os.Stat(checkPath)
	if err != nil {
		t.Fatal(err)
	}
	if info.Mode().Perm()&0o111 == 0 {
		t.Errorf("%s must be executable", checkPath)
	}
	check := readBaselineDoc(t, root, filepath.Join("scripts", "godot", "python-runtime-check.sh"))
	for _, required := range []string{
		"--qualify",
		"--exported",
		"--offline",
		"MORNLEA_PY4GODOT_CACHE_DIR",
		"PYTHONNOUSERSITE",
		"PYTHONPATH",
		"addons/py4godot",
		"unsupported Py4Godot desktop target",
	} {
		if !strings.Contains(check, required) {
			t.Errorf("python-runtime-check.sh is missing %q", required)
		}
	}

	ignore := readBaselineDoc(t, root, filepath.Join("apps", "mornlea-godot", ".gitignore"))
	if !strings.Contains(ignore, "addons/py4godot/") {
		t.Error("Godot project ignore rules must exclude the reproducible Py4Godot runtime")
	}
}

func readGodotVersionEnvironment(t *testing.T, path string) map[string]string {
	t.Helper()
	file, err := os.Open(path)
	if err != nil {
		t.Fatal(err)
	}
	defer file.Close()
	values := make(map[string]string)
	scanner := bufio.NewScanner(file)
	for scanner.Scan() {
		line := strings.TrimSpace(scanner.Text())
		if line == "" || strings.HasPrefix(line, "#") {
			continue
		}
		key, value, ok := strings.Cut(line, "=")
		if !ok || key == "" || value == "" {
			t.Fatalf("invalid Godot version environment line %q", line)
		}
		values[key] = value
	}
	if err := scanner.Err(); err != nil {
		t.Fatal(err)
	}
	return values
}
