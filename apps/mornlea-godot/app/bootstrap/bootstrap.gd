extends Node

# Bootstrap intentionally owns diagnostics only; it never imports Python features or
# starts networking before both native extensions have passed identity checks.
const SETUP_REQUIRED_SCENE: PackedScene = preload("res://app/bootstrap/setup_required.tscn")
const APP_ROOT_SCENE_PATH := "res://app/host/app_root.tscn"
const EXPECTED_GODOT_VERSION := "4.7.2-stable"
const PYTHON_DESCRIPTOR := "res://addons/py4godot/python.gdextension"
const BRIDGE_DESCRIPTOR := "res://addons/mornlea_bridge/mornlea_bridge.gdextension"


func _runtime_profile() -> String:
	return "debug" if OS.has_feature("debug") else "release"


func _required_artifacts() -> Array:
	var linux := OS.get_name() == "Linux"
	var runtime := "cpython-3.14.4-linux64" if linux else "cpython-3.14.4-darwin64"
	var suffix := "so" if linux else "dylib"
	var bridge_platform := "linux-x86_64" if linux else "macos-universal"
	var profile := _runtime_profile()
	var python_root := "res://addons/py4godot/%s/python/" % runtime
	var bridge_root := "res://addons/mornlea_bridge/bin/%s/%s/" % [bridge_platform, profile]
	var artifacts := [
		{"id": "python-extension", "path": PYTHON_DESCRIPTOR},
		{"id": "python-plugin", "path": python_root + "bin/pythonscript." + suffix},
		{"id": "python-bridge", "path": python_root + "bin/main." + suffix},
		{"id": "python-interpreter", "path": python_root + "lib/libpython3.14." + suffix},
		{"id": "python-stdlib", "path": python_root + "lib/python3.14/os.py"},
		{"id": "project-bridge-extension", "path": BRIDGE_DESCRIPTOR},
		{"id": "project-bridge-library", "path": bridge_root + "libmornlea_godot." + suffix},
		{"id": "client-core-library", "path": bridge_root + "libmornlea_client_core." + suffix},
		{"id": "engine-library", "path": bridge_root + "libmornlea_engine." + suffix},
	]
	if not linux:
		artifacts.append({"id": "client-render-library", "path": bridge_root + "libmornlea_client.dylib"})
	return artifacts


func _ready() -> void:
	var target := _target_triple()
	var missing := _missing_artifacts()
	var mismatched := _identity_mismatches(target)
	var prepare_command := (
		"scripts/godot/build-python-runtime.sh --verify --offline"
		+ " && scripts/godot/build-core.sh --verify"
		+ " && scripts/godot/build-extension.sh --target %s --profile debug --verify" % target
	)
	if missing.is_empty() and mismatched.is_empty():
		if _handoff_to_python():
			print(
				(
					"[mornlea-bootstrap] state=ready target=%s missing=none mismatched=none"
					+ " python=imported network=not-started"
				)
				% target
			)
			return
		mismatched.append("python-host-load")
	var setup_required := SETUP_REQUIRED_SCENE.instantiate()
	add_child(setup_required)
	setup_required.configure(target, missing, mismatched, prepare_command)
	var missing_text := "none" if missing.is_empty() else ",".join(missing)
	var mismatched_text := "none" if mismatched.is_empty() else ",".join(mismatched)
	print(
		(
			"[mornlea-bootstrap] state=setup-required target=%s missing=%s mismatched=%s"
			+ " prepare=%s python=not-imported network=not-started"
		)
		% [target, missing_text, mismatched_text, prepare_command]
	)


func _handoff_to_python() -> bool:
	# The path is loaded only after both native distribution identities pass, so
	# opening an incomplete checkout never parses or instantiates Python scripts.
	if not ClassDB.class_exists("MornleaClientBridge"):
		return false
	if not ResourceLoader.get_recognized_extensions_for_type("Script").has("py"):
		return false
	var resource := ResourceLoader.load(APP_ROOT_SCENE_PATH, "PackedScene", ResourceLoader.CACHE_MODE_REUSE)
	if not resource is PackedScene:
		return false
	var app_root := (resource as PackedScene).instantiate()
	if app_root == null:
		return false
	add_child(app_root)
	return true


func _missing_artifacts() -> PackedStringArray:
	var missing := PackedStringArray()
	for artifact in _required_artifacts():
		if not FileAccess.file_exists(artifact.path):
			missing.append("%s@%s" % [artifact.id, artifact.path])
	return missing


func _identity_mismatches(target: String) -> PackedStringArray:
	var mismatched := PackedStringArray()
	var profile := _runtime_profile()
	var platform := "linux" if OS.get_name() == "Linux" else "macos"
	var selector := "%s.%s.%s" % [platform, profile, Engine.get_architecture_name()]
	var version := Engine.get_version_info()
	var actual_godot_version := "%s.%s.%s-%s" % [
		version.get("major", -1),
		version.get("minor", -1),
		version.get("patch", -1),
		version.get("status", "unknown"),
	]
	if actual_godot_version != EXPECTED_GODOT_VERSION:
		mismatched.append("godot-version")
	if target.begins_with("unsupported-"):
		mismatched.append("desktop-target")
	# Descriptor text is inspected without loading either extension, preserving the
	# project's ability to open when generated binaries are absent or incompatible.
	if FileAccess.file_exists(PYTHON_DESCRIPTOR):
		if not _file_contains(PYTHON_DESCRIPTOR, 'entry_symbol = "initialize_pythonscript"'):
			mismatched.append("python-extension-entry-symbol")
		if not _file_contains(PYTHON_DESCRIPTOR, 'version = "4.7-alpha-21"'):
			mismatched.append("python-extension-version")
		var python_runtime := "cpython-3.14.4-linux64" if OS.get_name() == "Linux" else "cpython-3.14.4-darwin64"
		var python_suffix := "so" if OS.get_name() == "Linux" else "dylib"
		if not _file_contains(PYTHON_DESCRIPTOR, '%s = "%s/python/bin/pythonscript.%s"' % [selector, python_runtime, python_suffix]):
			mismatched.append("python-extension-target")
	if FileAccess.file_exists(BRIDGE_DESCRIPTOR):
		if not _file_contains(BRIDGE_DESCRIPTOR, 'entry_symbol = "gdext_rust_init"'):
			mismatched.append("project-bridge-entry-symbol")
		if not _file_contains(BRIDGE_DESCRIPTOR, 'compatibility_minimum = "4.7"'):
			mismatched.append("project-bridge-godot-api")
		var bridge_platform := "linux-x86_64" if OS.get_name() == "Linux" else "macos-universal"
		var bridge_suffix := "so" if OS.get_name() == "Linux" else "dylib"
		var bridge_path := "%s/%s/libmornlea_godot.%s" % [bridge_platform, profile, bridge_suffix]
		if not _file_contains(BRIDGE_DESCRIPTOR, '%s = "res://addons/mornlea_bridge/bin/%s"' % [selector, bridge_path]):
			mismatched.append("project-bridge-target")
	return mismatched


func _file_contains(path: String, expected: String) -> bool:
	var file := FileAccess.open(path, FileAccess.READ)
	if file == null:
		return false
	return file.get_as_text().contains(expected)


func _target_triple() -> String:
	# Platform scope is deliberately desktop-only; unsupported targets fail closed.
	if OS.get_name() == "Linux" and Engine.get_architecture_name() == "x86_64":
		return "x86_64-unknown-linux-gnu"
	if OS.get_name() != "macOS":
		return "unsupported-desktop-target"
	match Engine.get_architecture_name():
		"arm64":
			return "aarch64-apple-darwin"
		"x86_64":
			return "x86_64-apple-darwin"
		_:
			return "unsupported-desktop-target"
