package archcheck_test

import (
	"crypto/sha256"
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"testing"
)

func TestGodotPythonForkIsPinnedAndMinimal(t *testing.T) {
	root := repositoryRoot(t)
	forkRoot := filepath.Join(root, "scripts", "godot", "py4godot")
	values := readGodotVersionEnvironment(t, filepath.Join(forkRoot, "build-inputs.env"))
	want := map[string]string{
		"PY4GODOT_HARDENED_VERSION":      "4.7-alpha21-mornlea.2",
		"PY4GODOT_SOURCE_ARCHIVE_URL":    "https://codeload.github.com/niklas2902/py4godot/tar.gz/d8e17428deeb0428587349b663f6da26cd71ef3a",
		"PY4GODOT_SOURCE_ARCHIVE_SHA256": "e7a763fee661acecf1a4878501737299ba5fb7f6f0e58b4d3ac95fc58047fdd0",
		"PY4GODOT_SOURCE_ARCHIVE_ROOT":   "py4godot-d8e17428deeb0428587349b663f6da26cd71ef3a",
		"PY4GODOT_BUILD_TARGET":          "darwin-arm64",
		"PY4GODOT_APPLE_CLANG_VERSION":   "21.0.0",
		"PY4GODOT_MACOS_SDK_VERSION":     "26.5",
	}
	for key, expected := range want {
		if got := values[key]; got != expected {
			t.Errorf("%s = %q, want %q", key, got, expected)
		}
	}
	for _, key := range []string{
		"PY4GODOT_PATCH_SERIES_SHA256",
		"PY4GODOT_HARDENED_LOADER_SHA256",
		"PY4GODOT_HARDENED_ARTIFACT_SHA256",
	} {
		if !sha256Pattern.MatchString(values[key]) {
			t.Errorf("%s must pin a lowercase SHA-256 digest", key)
		}
	}

	seriesPath := filepath.Join(forkRoot, "patches", "series")
	series := strings.Fields(readBaselineDoc(t, root, filepath.Join("scripts", "godot", "py4godot", "patches", "series")))
	if len(series) == 0 || len(series) > 3 {
		t.Fatalf("Py4Godot hardening must use one to three focused patches, got %d", len(series))
	}
	var digestInput strings.Builder
	var allPatches strings.Builder
	for _, name := range series {
		if filepath.Base(name) != name || !strings.HasSuffix(name, ".patch") {
			t.Fatalf("invalid patch-series entry %q", name)
		}
		patchBytes, err := os.ReadFile(filepath.Join(filepath.Dir(seriesPath), name))
		if err != nil {
			t.Fatal(err)
		}
		patchDigest := sha256.Sum256(patchBytes)
		fmt.Fprintf(&digestInput, "%x  %s\n", patchDigest, name)
		patchText := string(patchBytes)
		for _, required := range []string{"Upstream-Defect:", "Deletion-Condition:"} {
			if !strings.Contains(patchText, required) {
				t.Errorf("patch %s is missing %s", name, required)
			}
		}
		allPatches.Write(patchBytes)
	}
	seriesDigest := sha256.Sum256([]byte(digestInput.String()))
	if got, expected := fmt.Sprintf("%x", seriesDigest), values["PY4GODOT_PATCH_SERIES_SHA256"]; got != expected {
		t.Errorf("patch-series digest = %s, want %s", got, expected)
	}

	patches := allPatches.String()
	for _, required := range []string{
		"PyConfig_InitIsolatedConfig",
		"config.use_environment = 0",
		"config.user_site_directory = 0",
		"config.safe_path = 1",
		"Py_InitializeFromConfig",
		"construct_without_init",
		"casted_from",
	} {
		if !strings.Contains(patches, required) {
			t.Errorf("hardening patch stack is missing %q", required)
		}
	}
	for _, forbidden := range []string{"AUTO_INSTALL", "install_dependencies.py", "get-pip.py"} {
		if strings.Contains(patches, forbidden) {
			t.Errorf("hardening patch stack must not add installer surface %q", forbidden)
		}
	}
}

func TestGodotPythonIsolationBuildEntryPoint(t *testing.T) {
	root := repositoryRoot(t)
	buildPath := filepath.Join(root, "scripts", "godot", "build-python-runtime.sh")
	info, err := os.Stat(buildPath)
	if err != nil {
		t.Fatal(err)
	}
	if info.Mode().Perm()&0o111 == 0 {
		t.Errorf("%s must be executable", buildPath)
	}
	build := readBaselineDoc(t, root, filepath.Join("scripts", "godot", "build-python-runtime.sh"))
	for _, required := range []string{
		"--verify",
		"--offline",
		"MORNLEA_PY4GODOT_CACHE_DIR",
		"PY4GODOT_SOURCE_ARCHIVE_SHA256",
		"PY4GODOT_PATCH_SERIES_SHA256",
		"PY4GODOT_HARDENED_LOADER_SHA256",
		"PY4GODOT_HARDENED_ARTIFACT_SHA256",
		"patches/series",
		"generated singleton rewrite count mismatch",
		"py4godot/utils/smart_cast.py",
		"unsupported Py4Godot build target",
	} {
		if !strings.Contains(build, required) {
			t.Errorf("build-python-runtime.sh is missing %q", required)
		}
	}
	for _, forbidden := range []string{"pip install", "get-pip.py", "install_dependencies.py", "-auto_install True"} {
		if strings.Contains(build, forbidden) {
			t.Errorf("build-python-runtime.sh contains forbidden installer path %q", forbidden)
		}
	}

	runtimeCheck := readBaselineDoc(t, root, filepath.Join("scripts", "godot", "python-runtime-check.sh"))
	if !strings.Contains(runtimeCheck, "build-python-runtime.sh") {
		t.Error("python-runtime-check.sh must qualify the hardened materialization entry point")
	}
	for _, required := range []string{
		"python-runtime-export-presets.cfg",
		"GODOT_EXPORT_TEMPLATES_SHA256",
		"MornleaPythonQualification.app",
		"Contents/Resources/addons/py4godot",
		"MORNLEA_PY4GODOT_QUALIFY_ITERATIONS",
	} {
		if !strings.Contains(runtimeCheck, required) {
			t.Errorf("python-runtime-check.sh is missing export qualification marker %q", required)
		}
	}
}

func TestGodotLinuxPythonForkHasIndependentBuildPins(t *testing.T) {
	root := repositoryRoot(t)
	values := readGodotVersionEnvironment(t, filepath.Join(root, "scripts", "godot", "py4godot", "linux-build-inputs.env"))
	for key, expected := range map[string]string{
		"PY4GODOT_BUILD_TARGET": "x86_64-unknown-linux-gnu",
		"PY4GODOT_GCC_VERSION":  "14.2.0",
	} {
		if got := values[key]; got != expected {
			t.Errorf("%s = %q, want %q", key, got, expected)
		}
	}
	macValues := readGodotVersionEnvironment(t, filepath.Join(root, "scripts", "godot", "py4godot", "build-inputs.env"))
	for _, key := range []string{"PY4GODOT_HARDENED_LOADER_SHA256", "PY4GODOT_HARDENED_ARTIFACT_SHA256"} {
		if !sha256Pattern.MatchString(values[key]) || strings.Trim(values[key], "0") == "" {
			t.Errorf("%s must independently pin verified Linux bytes", key)
		}
		if values[key] == macValues[key] {
			t.Errorf("%s must not reuse the macOS artifact identity", key)
		}
	}
}

func TestGodotPythonBuilderRejectsForeignHostBeforeMaterialization(t *testing.T) {
	sourceRoot := repositoryRoot(t)
	for _, test := range []struct{ target, host, arch string }{
		{"darwin-arm64", "Linux", "x86_64"},
		{"x86_64-unknown-linux-gnu", "Darwin", "arm64"},
	} {
		t.Run(test.target, func(t *testing.T) {
			root := t.TempDir()
			scriptDir := filepath.Join(root, "repository", "scripts", "godot")
			for _, relative := range []string{"build-python-runtime.sh", "python-version.env", "py4godot/build-inputs.env", "py4godot/linux-build-inputs.env"} {
				writeFile(t, filepath.Join(scriptDir, relative), []byte(readBaselineDoc(t, sourceRoot, filepath.Join("scripts", "godot", relative))))
			}
			bin := filepath.Join(root, "bin")
			writeExecutable(t, filepath.Join(bin, "uname"), fmt.Sprintf("#!/bin/sh\ncase \"$1\" in -s) echo %s ;; -m) echo %s ;; *) exit 2 ;; esac\n", test.host, test.arch))
			cache := filepath.Join(root, "cache")
			command := exec.Command("bash", filepath.Join(scriptDir, "build-python-runtime.sh"), "--verify", "--offline", "--target", test.target, "--cache-dir", cache)
			command.Env = append(os.Environ(), "PATH="+bin+":"+os.Getenv("PATH"))
			output, err := command.CombinedOutput()
			if err == nil || !strings.Contains(string(output), "unsupported Py4Godot build target on host: "+test.host+"-"+test.arch) {
				t.Fatalf("foreign-host native build was not rejected: %v\n%s", err, output)
			}
			if _, err := os.Stat(cache); !os.IsNotExist(err) {
				t.Fatalf("host rejection must precede cache or artifact materialization: %v", err)
			}
		})
	}
}
