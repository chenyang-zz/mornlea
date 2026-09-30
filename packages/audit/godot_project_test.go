package archcheck_test

import (
	"errors"
	"io/fs"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"testing"
)

func TestGodotProjectBootstrapIsPureAndStable(t *testing.T) {
	root := repositoryRoot(t)
	project := readBaselineDoc(t, root, filepath.Join("apps", "mornlea-godot", "project.godot"))
	if !strings.Contains(project, `run/main_scene="res://app/bootstrap/bootstrap.tscn"`) {
		t.Error("Godot project main scene must be the stable Bootstrap scene")
	}
	for _, relative := range []string{
		"apps/mornlea-godot/project.godot",
		"apps/mornlea-godot/app/bootstrap/bootstrap.tscn",
		"apps/mornlea-godot/app/bootstrap/setup_required.tscn",
	} {
		text := readBaselineDoc(t, root, filepath.FromSlash(relative))
		for _, forbidden := range []string{"MornleaClientBridge", ".gdextension", "addons/mornlea_bridge/bin"} {
			if strings.Contains(text, forbidden) {
				t.Errorf("%s serializes native dependency %q", relative, forbidden)
			}
		}
	}
	bootstrap := readBaselineDoc(t, root, filepath.Join("apps", "mornlea-godot", "app", "bootstrap", "bootstrap.gd"))
	if !strings.Contains(bootstrap, `res://app/bootstrap/setup_required.tscn`) {
		t.Error("pure-GDScript Bootstrap must route to the setup-required scene")
	}
	ignore := readBaselineDoc(t, root, filepath.Join("apps", "mornlea-godot", ".gitignore"))
	for _, required := range []string{".godot/", "addons/mornlea_bridge/bin/"} {
		if !strings.Contains(ignore, required) {
			t.Errorf("project .gitignore is missing %q", required)
		}
	}
	if strings.Contains(ignore, ".uid") {
		t.Error("Godot script and shader UID sidecars must remain tracked")
	}
}

func TestGodotBootstrapDoesNotInitiateNetwork(t *testing.T) {
	root := repositoryRoot(t)
	for _, relative := range []string{
		"apps/mornlea-godot/app/bootstrap/bootstrap.gd",
		"apps/mornlea-godot/app/bootstrap/setup_required.gd",
	} {
		text := readBaselineDoc(t, root, filepath.FromSlash(relative))
		for _, forbidden := range []string{
			"StreamPeerTCP",
			"PacketPeer",
			"HTTPClient",
			"HTTPRequest",
			"connect_to_host",
			"connect_to_url",
		} {
			if strings.Contains(text, forbidden) {
				t.Errorf("%s initiates network through %q", relative, forbidden)
			}
		}
	}
}

func TestGodotBootstrapDiagnosesIncompleteDistribution(t *testing.T) {
	root := repositoryRoot(t)
	bootstrap := readBaselineDoc(t, root, filepath.Join("apps", "mornlea-godot", "app", "bootstrap", "bootstrap.gd"))
	for _, required := range []string{
		"python-extension",
		"python-plugin",
		"python-bridge",
		"python-interpreter",
		"python-stdlib",
		"project-bridge-extension",
		"project-bridge-library",
		"4.7-alpha-21",
		"initialize_pythonscript",
		"gdext_rust_init",
		"mismatched",
		"build-python-runtime.sh --verify --offline",
		"build-extension.sh --target %s --profile debug --verify",
		"python=not-imported",
		"network=not-started",
	} {
		if !strings.Contains(bootstrap, required) {
			t.Errorf("Bootstrap is missing distribution diagnostic %q", required)
		}
	}
	for _, forbidden := range []string{
		"res://features/",
		"res://platform/",
		"StreamPeerTCP",
		"HTTPClient",
		"HTTPRequest",
	} {
		if strings.Contains(bootstrap, forbidden) {
			t.Errorf("Bootstrap must not import features or initiate network through %q", forbidden)
		}
	}

	smoke := readBaselineDoc(t, root, filepath.Join("scripts", "godot", "openable-smoke.sh"))
	for _, required := range []string{
		"--without-native",
		"--without-python",
		"deny network*",
		"python-extension-version",
		"project-bridge-entry-symbol",
	} {
		if !strings.Contains(smoke, required) {
			t.Errorf("openable-smoke.sh is missing clean-state assertion %q", required)
		}
	}
}

func TestGodotPreparedBootstrapHandsOffToPythonDynamically(t *testing.T) {
	root := repositoryRoot(t)
	bootstrap := readBaselineDoc(t, root, filepath.Join("apps", "mornlea-godot", "app", "bootstrap", "bootstrap.gd"))
	for _, required := range []string{
		`const APP_ROOT_SCENE_PATH := "res://app/host/app_root.tscn"`,
		"missing.is_empty() and mismatched.is_empty()",
		"ResourceLoader.load(APP_ROOT_SCENE_PATH",
		"state=ready",
		"python=imported",
	} {
		if !strings.Contains(bootstrap, required) {
			t.Errorf("prepared Bootstrap handoff is missing %q", required)
		}
	}
	for _, forbidden := range []string{
		`preload("res://app/host/`,
		`const APP_ROOT_SCENE: PackedScene`,
	} {
		if strings.Contains(bootstrap, forbidden) {
			t.Errorf("permanent Bootstrap serializes Python host through %q", forbidden)
		}
	}
}

func TestGodotDesktopOnlyFeatureSkeleton(t *testing.T) {
	root := repositoryRoot(t)
	manifestPaths := []string{
		"features/session/feature.tres",
		"features/player_view/feature.tres",
		"features/world/feature.tres",
		"features/actors/feature.tres",
		"features/ui/feature.tres",
		"platform/desktop/input/feature.tres",
		"platform/desktop/lifecycle/feature.tres",
		"platform/desktop/audio/feature.tres",
	}
	// The session/world roots are required providers; the input, player-view,
	// actors, and UI roots are implemented optional consumers in the minimum
	// loop. Remaining roots stay disabled until their own implementation tasks.
	implemented := map[string]bool{
		"features/session/feature.tres":       true,
		"features/player_view/feature.tres":   true,
		"features/world/feature.tres":         true,
		"features/actors/feature.tres":        true,
		"features/ui/feature.tres":            true,
		"platform/desktop/input/feature.tres": true,
	}
	required := map[string]bool{
		"features/session/feature.tres": true,
		"features/world/feature.tres":   true,
	}
	catalog := readBaselineDoc(t, root, filepath.Join("apps", "mornlea-godot", "config", "feature_catalog.tres"))
	for _, relative := range manifestPaths {
		fullPath := filepath.Join("apps", "mornlea-godot", filepath.FromSlash(relative))
		manifest := readBaselineDoc(t, root, fullPath)
		if implemented[relative] {
			if !strings.Contains(manifest, "enabled = true") {
				t.Errorf("implemented Godot feature %s must stay enabled", relative)
			}
			if required[relative] && !strings.Contains(manifest, "metadata/required = true") {
				t.Errorf("implemented pilot feature %s must be required by the catalog plan", relative)
			}
		} else if !strings.Contains(manifest, "enabled = false") {
			t.Errorf("unimplemented Godot skeleton %s must stay explicitly disabled", relative)
		}
		if !strings.Contains(catalog, "res://"+relative) {
			t.Errorf("explicit feature catalog is missing %s", relative)
		}
	}
	for _, relative := range []string{
		"platform/mobile",
		"platform/android",
		"platform/ios",
		"platform/web",
		"platform/console",
	} {
		path := filepath.Join(root, "apps", "mornlea-godot", filepath.FromSlash(relative))
		if _, err := os.Lstat(path); err == nil {
			t.Errorf("unsupported Godot platform directory exists: %s", relative)
		} else if !errors.Is(err, os.ErrNotExist) {
			t.Errorf("inspect unsupported Godot platform directory %s: %v", relative, err)
		}
	}
}

func TestGodotPythonFeatureSkeletonsAndScriptOwnership(t *testing.T) {
	root := repositoryRoot(t)
	projectRoot := filepath.Join(root, "apps", "mornlea-godot")
	skeletons := map[string]string{
		"features/actors":            "actors_feature",
		"features/player_view":       "player_view_feature",
		"features/session":           "session_feature",
		"features/ui":                "ui_feature",
		"features/world":             "world_feature",
		"platform/desktop/audio":     "desktop_audio_feature",
		"platform/desktop/input":     "desktop_input_feature",
		"platform/desktop/lifecycle": "desktop_lifecycle_feature",
	}
	// The session pilot feature renders a Control scene, so its script bases
	// on the generated Control class; every reserved skeleton stays Node.
	scriptBases := map[string]string{"features/session": "Control"}
	registeredManifests := make(map[string]bool, len(skeletons))
	for directory, className := range skeletons {
		manifest := filepath.ToSlash(filepath.Join(directory, "feature.tres"))
		registeredManifests[manifest] = true
		scriptName := className + ".py"
		scriptPath := filepath.Join(projectRoot, filepath.FromSlash(directory), scriptName)
		script := readBaselineDoc(t, root, filepath.Join("apps", "mornlea-godot", filepath.FromSlash(directory), scriptName))
		base := scriptBases[directory]
		if base == "" {
			base = "Node"
		}
		for _, required := range []string{
			"class " + className + "(" + base + "):",
			"def validate_feature",
			"def bind_host",
			"def activate_feature",
			"def reset_feature",
			"def deactivate_feature",
		} {
			if !strings.Contains(script, required) {
				t.Errorf("Python feature skeleton %s is missing %q", scriptPath, required)
			}
		}
		scene := readBaselineDoc(t, root, filepath.Join("apps", "mornlea-godot", filepath.FromSlash(directory), "feature_root.tscn"))
		resourcePath := "res://" + filepath.ToSlash(filepath.Join(directory, scriptName))
		if !strings.Contains(scene, resourcePath) || !strings.Contains(scene, "script = ExtResource") {
			t.Errorf("feature scene %s does not attach %s", directory, resourcePath)
		}
	}
	// Reserved coarse capabilities have no implementation scene, so their
	// manifests live in the separate capability registry rather than the
	// product activation catalog. Keep this audit aware of that ownership
	// boundary while still rejecting every other unregistered manifest.
	capabilityRegistry := readBaselineDoc(t, root, filepath.Join("apps", "mornlea-godot", "catalog", "capability_registry.tres"))
	for _, token := range strings.Split(capabilityRegistry, `"`) {
		if strings.HasPrefix(token, "res://") {
			registeredManifests[strings.TrimPrefix(token, "res://")] = true
		}
	}

	// A manifest is reserved for a replaceable coarse boundary, not an internal scene.
	for _, top := range []string{"features", "platform"} {
		err := filepath.WalkDir(filepath.Join(projectRoot, top), func(path string, entry fs.DirEntry, walkErr error) error {
			if walkErr != nil {
				return walkErr
			}
			if entry.IsDir() || entry.Name() != "feature.tres" {
				return nil
			}
			relative, err := filepath.Rel(projectRoot, path)
			if err != nil {
				return err
			}
			if !registeredManifests[filepath.ToSlash(relative)] {
				t.Errorf("ordinary component owns an unregistered feature manifest: %s", relative)
			}
			return nil
		})
		if err != nil {
			t.Fatal(err)
		}
	}

	allowedGDScript := map[string]bool{
		"app/bootstrap/bootstrap.gd":      true,
		"app/bootstrap/setup_required.gd": true,
	}
	err := filepath.WalkDir(projectRoot, func(path string, entry fs.DirEntry, walkErr error) error {
		if walkErr != nil {
			return walkErr
		}
		if entry.IsDir() {
			switch entry.Name() {
			case ".godot", ".venv", "tests":
				return filepath.SkipDir
			}
			if path == filepath.Join(projectRoot, "addons", "py4godot") {
				return filepath.SkipDir
			}
			return nil
		}
		if filepath.Ext(path) != ".gd" {
			return nil
		}
		relative, err := filepath.Rel(projectRoot, path)
		if err != nil {
			return err
		}
		if !allowedGDScript[filepath.ToSlash(relative)] {
			t.Errorf("production GDScript exists outside the bootstrap allowlist: %s", relative)
		}
		return nil
	})
	if err != nil {
		t.Fatal(err)
	}

	validatorPath := filepath.Join(root, "scripts", "godot", "validate-project.sh")
	info, err := os.Stat(validatorPath)
	if err != nil {
		t.Fatal(err)
	}
	if info.Mode().Perm()&0o111 == 0 {
		t.Error("validate-project.sh must be executable")
	}
	validator := readBaselineDoc(t, root, filepath.Join("scripts", "godot", "validate-project.sh"))
	for _, required := range []string{"--script-ownership", "app/bootstrap/bootstrap.gd", "app/bootstrap/setup_required.gd"} {
		if !strings.Contains(validator, required) {
			t.Errorf("validate-project.sh is missing script-ownership rule %q", required)
		}
	}
	fixtureRoot := t.TempDir()
	if err := os.MkdirAll(filepath.Join(fixtureRoot, "features", "world"), 0o755); err != nil {
		t.Fatal(err)
	}
	illegalPath := filepath.Join(fixtureRoot, "features", "world", "illegal.gd")
	if err := os.WriteFile(illegalPath, []byte("extends Node\n"), 0o600); err != nil {
		t.Fatal(err)
	}
	command := exec.Command(validatorPath, "--script-ownership")
	command.Env = append(os.Environ(), "MORNLEA_GODOT_PROJECT_ROOT="+fixtureRoot)
	output, err := command.CombinedOutput()
	if err == nil {
		t.Fatal("script-ownership validation accepted production GDScript outside Bootstrap")
	}
	if !strings.Contains(string(output), "features/world/illegal.gd") {
		t.Fatalf("script-ownership failure did not identify the violating path:\n%s", output)
	}
}

func TestGodotBridgeHostIsIdentityOnlyAndDesktopBound(t *testing.T) {
	root := repositoryRoot(t)
	descriptor := readBaselineDoc(t, root, filepath.Join("apps", "mornlea-godot", "addons", "mornlea_bridge", "mornlea_bridge.gdextension"))
	for _, required := range []string{
		`entry_symbol = "gdext_rust_init"`,
		`compatibility_minimum = "4.7"`,
		`macos.debug.arm64 = "res://addons/mornlea_bridge/bin/macos-universal/debug/libmornlea_godot.dylib"`,
		`macos.release.arm64 = "res://addons/mornlea_bridge/bin/macos-universal/release/libmornlea_godot.dylib"`,
		`macos.debug.x86_64 = "res://addons/mornlea_bridge/bin/macos-universal/debug/libmornlea_godot.dylib"`,
		`macos.release.x86_64 = "res://addons/mornlea_bridge/bin/macos-universal/release/libmornlea_godot.dylib"`,
		`linux.debug.x86_64 = "res://addons/mornlea_bridge/bin/linux-x86_64/debug/libmornlea_godot.so"`,
		`linux.release.x86_64 = "res://addons/mornlea_bridge/bin/linux-x86_64/release/libmornlea_godot.so"`,
		`windows.debug.x86_64 = "res://addons/mornlea_bridge/bin/windows-x86_64/debug/mornlea_godot.dll"`,
		`windows.release.x86_64 = "res://addons/mornlea_bridge/bin/windows-x86_64/release/mornlea_godot.dll"`,
	} {
		if !strings.Contains(descriptor, required) {
			t.Errorf("desktop GDExtension descriptor is missing %q", required)
		}
	}
	for _, forbidden := range []string{"android", "ios", "web", "wasm", "console"} {
		if strings.Contains(strings.ToLower(descriptor), forbidden) {
			t.Errorf("desktop GDExtension descriptor contains unsupported selector %q", forbidden)
		}
	}

	bridgeHost := readBaselineDoc(t, root, filepath.Join("apps", "mornlea-godot", "addons", "mornlea_bridge", "bridge_host.py"))
	for _, required := range []string{
		"class bridge_host(Node):",
		"ClassDB.instance()",
		`MornleaClientBridge`,
		"def initialize_bridge",
		"def host_protocol_version",
		"def feature_family_version",
		"def client_core_abi_version",
		"def godot_api_version",
		"def godot_rust_version",
		"def bridge_identity",
	} {
		if !strings.Contains(bridgeHost, required) {
			t.Errorf("Python bridge host is missing %q", required)
		}
	}
	for _, forbidden := range []string{"ctypes", "cffi", "dlopen", "begin_connection", "submit_input", "frame_snapshot", "world_batch"} {
		if strings.Contains(bridgeHost, forbidden) {
			t.Errorf("identity-only Python bridge host contains premature data-plane token %q", forbidden)
		}
	}

	appScene := readBaselineDoc(t, root, filepath.Join("apps", "mornlea-godot", "app", "host", "app_root.tscn"))
	for _, required := range []string{`[node name="ClientBridge" type="MornleaClientBridge" parent="."]`} {
		if !strings.Contains(appScene, required) {
			t.Errorf("app-root scene is missing native bridge wiring %q", required)
		}
	}
	appRoot := readBaselineDoc(t, root, filepath.Join("apps", "mornlea-godot", "app", "host", "app_root.py"))
	for _, required := range []string{`get_node("ClientBridge")`, `"../ClientBridge"`} {
		if !strings.Contains(appRoot, required) {
			t.Errorf("app-root host handoff is missing %q", required)
		}
	}

	buildScript := readBaselineDoc(t, root, filepath.Join("scripts", "godot", "build-extension.sh"))
	for _, required := range []string{"aarch64-apple-darwin", "x86_64-unknown-linux-gnu", "x86_64-pc-windows-msvc", "bridge_host_check.tscn"} {
		if !strings.Contains(buildScript, required) {
			t.Errorf("desktop bridge build is missing %q", required)
		}
	}
	pythonRuntime := readBaselineDoc(t, root, filepath.Join("scripts", "godot", "python-runtime-check.sh"))
	for _, required := range []string{"--coexistence", "bridge_host_check.tscn", "Python bridge host check passed."} {
		if !strings.Contains(pythonRuntime, required) {
			t.Errorf("isolated bridge coexistence check is missing %q", required)
		}
	}
}

func TestGodotLifecycleSmokeContract(t *testing.T) {
	root := repositoryRoot(t)
	smoke := readBaselineDoc(t, root, filepath.Join("scripts", "godot", "smoke.sh"))
	for _, required := range []string{
		"--iterations",
		"--isolated-python",
		"build-python-runtime.sh",
		"build-extension.sh",
		"sandbox-exec",
		"deny-network.sh",
		"PYTHONPATH",
		"--mornlea-smoke-token=",
		"pgrep -f",
		"*.crash",
		"*.ips",
		"[mornlea-lifecycle] rust-init=scene",
		"[mornlea-lifecycle] rust-init=main-loop",
		"[mornlea-lifecycle] python-init=host",
		"[mornlea-lifecycle] python-deinit=features",
		"[mornlea-lifecycle] python-deinit=bridge",
		"[mornlea-lifecycle] rust-deinit=main-loop",
		"[mornlea-lifecycle] rust-deinit=scene",
		"leaked at exit",
	} {
		if !strings.Contains(smoke, required) {
			t.Errorf("Godot lifecycle smoke is missing %q", required)
		}
	}
	appRoot := readBaselineDoc(t, root, filepath.Join("apps", "mornlea-godot", "app", "host", "app_root.py"))
	for _, required := range []string{
		"[mornlea-lifecycle] python-init=host",
		"[mornlea-lifecycle] python-deinit=features",
		"[mornlea-lifecycle] python-deinit=bridge",
	} {
		if !strings.Contains(appRoot, required) {
			t.Errorf("Python host lifecycle is missing marker %q", required)
		}
	}
}

func TestGodotFeatureHostIsPythonOwnedAndExplicit(t *testing.T) {
	root := repositoryRoot(t)
	projectRoot := filepath.Join(root, "apps", "mornlea-godot")
	for _, relative := range []string{
		"app/host/app_root.gd",
		"app/host/feature_catalog.gd",
		"app/host/feature_host.gd",
		"app/host/feature_manifest.gd",
	} {
		path := filepath.Join(projectRoot, filepath.FromSlash(relative))
		if _, err := os.Stat(path); err == nil {
			t.Errorf("retired GDScript host still exists: %s", relative)
		} else if !errors.Is(err, os.ErrNotExist) {
			t.Errorf("inspect retired GDScript host %s: %v", relative, err)
		}
	}

	host := readBaselineDoc(t, root, filepath.Join("apps", "mornlea-godot", "app", "host", "feature_host.py"))
	for _, required := range []string{
		"HOST_PROTOCOL_MAJOR = 1",
		"HOST_PROTOCOL_MINOR = 0",
		"plan_catalog",
		"activate_catalog",
		"validate_feature",
		"bind_host",
		"reset_features",
		"deactivate_features",
		"required_bridge_families",
		"entry_scene_path.startswith(\"res://\")",
	} {
		if !strings.Contains(host, required) {
			t.Errorf("Python feature host is missing %q", required)
		}
	}

	catalog := readBaselineDoc(t, root, filepath.Join("apps", "mornlea-godot", "config", "feature_catalog.tres"))
	for _, required := range []string{
		"metadata/host_protocol_major = 1",
		"metadata/host_protocol_minor = 0",
		"metadata/manifest_paths = PackedStringArray",
	} {
		if !strings.Contains(catalog, required) {
			t.Errorf("explicit Python feature catalog is missing %q", required)
		}
	}

	appScene := readBaselineDoc(t, root, filepath.Join("apps", "mornlea-godot", "app", "host", "app_root.tscn"))
	for _, required := range []string{"res://app/host/app_root.py", "res://app/host/feature_host.py"} {
		if !strings.Contains(appScene, required) {
			t.Errorf("Python app-root scene is missing %q", required)
		}
	}
	if strings.Contains(appScene, ".gd\"") {
		t.Error("Python app-root scene must not retain a GDScript host")
	}

	contractCheck := readBaselineDoc(t, root, filepath.Join("scripts", "godot", "feature-contract-check.sh"))
	for _, required := range []string{
		"res://tests/scenes/feature_contract_check.tscn",
		"Python feature contract checks passed.",
		"--extensibility-probe",
	} {
		if !strings.Contains(contractCheck, required) {
			t.Errorf("feature-contract-check.sh is missing %q", required)
		}
	}
}
