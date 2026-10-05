package archcheck_test

import (
	"os"
	"os/exec"
	"path/filepath"
	"regexp"
	"strings"
	"testing"
)

func TestGodotBuildCoreVerifiesLargeSymbolTables(t *testing.T) {
	root := repositoryRoot(t)
	script := readBaselineDoc(t, root, filepath.Join("scripts", "godot", "build-core.sh"))
	start := strings.Index(script, "  while IFS= read -r exported_symbol; do")
	end := strings.Index(script, "  printf 'client-core symbols verified:")
	if start < 0 || end <= start {
		t.Fatal("cannot locate executable symbol verification block")
	}
	block := script[start:end]
	directory := t.TempDir()
	exportsPath := filepath.Join(directory, "exports")
	symbolsPath := filepath.Join(directory, "symbols")
	exports := "mornlea_client_core_abi_version\nmornlea_client_core_environment_pull\n"
	if err := os.WriteFile(exportsPath, []byte(exports), 0o600); err != nil {
		t.Fatal(err)
	}
	for _, missing := range []bool{false, true} {
		symbols := exports
		if missing {
			symbols = "mornlea_client_core_abi_version\n"
		}
		// A real Go ELF library has many unrelated exports. A quiet grep may
		// exit before a piped writer finishes, causing a false pipefail error.
		symbols += strings.Repeat("unrelated_native_symbol\n", 20000)
		if err := os.WriteFile(symbolsPath, []byte(symbols), 0o600); err != nil {
			t.Fatal(err)
		}
		command := "set -euo pipefail\nfail() { printf '%s\\n' \"$*\" >&2; exit 1; }\n" +
			"defined_symbols=$(cat \"$1\")\nexported_symbols=$(cat \"$2\")\nsymbol_prefix=\ncore_library=fixture\n" + block
		output, err := exec.Command("bash", "-c", command, "symbol-fixture", symbolsPath, exportsPath).CombinedOutput()
		if missing {
			if err == nil || !strings.Contains(string(output), "exported symbol is missing") {
				t.Fatalf("missing export accepted: err=%v output=%s", err, output)
			}
		} else if err != nil {
			t.Fatalf("valid large symbol table rejected: %v\n%s", err, output)
		}
	}
}

// TestGodotBuildCoreScriptPin pins the shared-library build script for the
// Go client core. The script is the producer half of the distribution
// contract: it must build the c-shared library into the ignored Godot bridge
// bin tree next to the GDExtension, colocate the Rust engine dynamic
// libraries the core links, and verify the loaded contract (exported symbol
// surface, canonical header, producer identity, dependency resolution)
// without writing outside the bin tree plus user-level caches.
func TestGodotBuildCoreScriptPin(t *testing.T) {
	root := repositoryRoot(t)
	scriptPath := filepath.Join(root, "scripts", "godot", "build-core.sh")
	info, err := os.Stat(scriptPath)
	if err != nil {
		t.Fatal(err)
	}
	if info.Mode().Perm()&0o111 == 0 {
		t.Errorf("%s must be executable", scriptPath)
	}
	script := readBaselineDoc(t, root, filepath.Join("scripts", "godot", "build-core.sh"))
	for _, required := range []string{
		"go build",
		"-buildmode=c-shared",
		"addons/mornlea_bridge/bin",
		"macos-universal",
		"x86_64-unknown-linux-gnu",
		"libmornlea_client_core.so",
		"libmornlea_engine.so",
		"patchelf --set-rpath '$ORIGIN'",
		"-u LD_LIBRARY_PATH -u LD_PRELOAD",
		"scratch_dir}/distribution",
		"--verify",
		"--target",
		"--profile",
		"unsupported Godot desktop target",
		"make rust",
		"libmornlea_client_core.dylib",
		"libmornlea_engine.dylib",
		"libmornlea_client.dylib",
		"include/mornlea_client_core.h",
		"libmornlea_client_core.h",
		"nm -gU",
		"otool -L",
		"otool -D",
		"install_name_tool",
		"codesign",
		"@loader_path",
		"@rpath/libmornlea_engine.dylib",
		"dlopen",
		"mornlea_client_core_abi_version",
		"mktemp",
		"user-level Go build cache",
	} {
		if !strings.Contains(script, required) {
			t.Errorf("build-core.sh is missing %q", required)
		}
	}

	// The deterministic symbol list inside the script must cover every
	// `//export` directive of the producer, so a new export cannot land
	// without the shared-library verification learning its name.
	exports := readBaselineDoc(t, root, filepath.Join("packages", "client", "cmd", "mornlea-godot-core", "exports.go"))
	exportPattern := regexp.MustCompile(`(?m)^//export ([A-Za-z0-9_]+)$`)
	matches := exportPattern.FindAllStringSubmatch(exports, -1)
	if len(matches) == 0 {
		t.Fatal("exports.go declares no //export symbols")
	}
	for _, match := range matches {
		if !strings.Contains(script, match[1]) {
			t.Errorf("build-core.sh symbol verification is missing export %q", match[1])
		}
	}

	// The build output tree must stay reproducible and untracked; the script
	// writes only into this ignored distribution directory.
	ignore := readBaselineDoc(t, root, filepath.Join("apps", "mornlea-godot", ".gitignore"))
	if !strings.Contains(ignore, "addons/mornlea_bridge/bin/") {
		t.Error("Godot project ignore rules must exclude the bridge bin distribution tree")
	}
}
