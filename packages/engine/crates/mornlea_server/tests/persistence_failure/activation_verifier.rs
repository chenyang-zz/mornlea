//! Actual sealed Go verifier consumption over current Rust storage bytes.

use super::*;
use mornlea_server::core::contracts::{
    DiskBackend, LoadedValue, OwnedSnapshot, SaveKey, SaveRequest, SaveTicket, SaveUrgency,
    SaveValue,
};
use mornlea_server::store::disk::{DiskOptions, DiskStore};

fn package_path() -> PathBuf {
    let path = std::env::var("MORNLEA_PREVIOUS_PACKAGE")
        .expect("explicit MORNLEA_PREVIOUS_PACKAGE names the accepted sealed package");
    assert!(!path.is_empty(), "package fixture must not be empty");
    PathBuf::from(path)
}

fn package() -> serde_json::Value {
    let value = read_manifest(&package_path());
    assert_eq!(
        value["previous_executable"],
        previous_bin().to_string_lossy().as_ref()
    );
    value
}

fn prepare_fixture(world: &Path, future: bool) {
    let output = Command::new(package()["verifier_executable"].as_str().unwrap())
        .args([
            "-test.run=^TestRuntimeMigrationPrepareCallableFixture$",
            "-test.count=1",
        ])
        .env_remove("MORNLEA_VERIFY_WORLD")
        .env_remove("MORNLEA_VERIFY_OUTPUT")
        .env("MORNLEA_VERIFY_FIXTURE", world)
        .env(
            "MORNLEA_VERIFY_FIXTURE_FUTURE",
            if future { "1" } else { "0" },
        )
        .output()
        .expect("run actual Go fixture producer");
    assert!(
        output.status.success(),
        "fixture producer: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn rollback_refuses(manifest: &Path, world: &Path, run: &Path) {
    let before = tree_hash(world, &[LOCK_BASENAME]);
    let (code, output) = run_script(&[
        "rollback",
        "--manifest",
        &manifest.to_string_lossy(),
        "--data-policy",
        "compatible",
    ]);
    if code == 0 {
        stop_previous(&read_manifest(manifest));
    }
    assert_ne!(
        code, 0,
        "incompatible world starts no previous writer: {output}"
    );
    assert!(
        output.contains("incompatible_save"),
        "typed refusal: {output}"
    );
    assert_eq!(tree_hash(world, &[LOCK_BASENAME]), before);
    assert_ne!(read_manifest(manifest)["phase"], "PreviousRunning");
    assert!(!run.join("previous-listen.addr").exists());
    let lease = mornlea_server::store::lease::WorldLease::acquire(world)
        .expect("rejected world is unowned");
    drop(lease);
}

#[test]
fn actual_valid_rust_save_rolls_back() {
    let scope = Scope::fresh("vsave");
    let run = scope.path("run");
    let world = run.join("world");
    let backup = run.join("backup");
    let (mut go, _, seed, identity) = prepare_go_world(&previous_bin(), &world, "save-probe");
    stop_child(&mut go, "prepare storage save");
    let manifest = script_activate(&scope, &world, &backup, &run, &[]);
    quiesce_rust(&manifest);
    let baseline = manifest_str(&read_manifest(&manifest), "world_tree_sha256");
    let metadata =
        mornlea_storage::decode_world_metadata(&fs::read(world.join("world.meta")).unwrap())
            .unwrap();
    let mut disk = DiskStore::open(
        &world,
        DiskOptions {
            create: metadata,
            region_handle_cap: 1,
        },
    )
    .unwrap();
    let LoadedValue::Metadata(mut metadata) = disk.load(SaveKey::Metadata).unwrap() else {
        panic!("metadata load")
    };
    metadata.world_time_ticks += 1;
    let snapshot = OwnedSnapshot::try_new(
        SaveKey::Metadata,
        2,
        1024,
        SaveUrgency::Autosave,
        SaveValue::Metadata(metadata),
    )
    .unwrap();
    let completion = disk.write(
        SaveTicket::try_from_raw(1).unwrap(),
        SaveRequest {
            snapshots: vec![snapshot],
        },
    );
    assert!(completion.error.is_none());
    assert_eq!(completion.committed, vec![(SaveKey::Metadata, 2)]);
    disk.sync().unwrap();
    disk.close().unwrap();
    let changed = tree_hash(&world, &[LOCK_BASENAME]);
    assert_ne!(baseline, changed);
    assert_eq!(
        tree_hash(&backup, &[LOCK_BASENAME, BACKUP_IDENTITY_BASENAME]),
        baseline
    );
    let previous = rollback_compatible(&manifest, "actual valid Rust save");
    let report = read_manifest(&run.join("previous-verifier-report.json"));
    assert_eq!(report["compatible"], true);
    assert!(report["read_files"].as_u64().unwrap() > 0);
    assert_eq!(report["world_tree_sha256"], changed);
    assert_eq!(previous["world_tree_sha256"], changed);
    assert_eq!(
        go_login(&last_go_listen(&run), identity, "save-probe")
            .unwrap()
            .1,
        seed
    );
    assert!(
        mornlea_server::store::lease::WorldLease::acquire(&world).is_err(),
        "actual previous runtime owns the sole lease"
    );
    stop_previous(&previous);
}

fn preexisting_corrupt_refuses(label: &str, relative: &str) {
    let scope = Scope::fresh(label);
    let run = scope.path("run");
    let world = run.join("world");
    prepare_fixture(&world, false);
    let path = fs::read_dir(world.join(relative))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    fs::write(&path, b"corrupt canonical unloaded save").unwrap();
    let manifest = script_activate(&scope, &world, &run.join("backup"), &run, &[]);
    quiesce_rust(&manifest);
    rollback_refuses(&manifest, &world, &run);
}

#[test]
fn preexisting_corrupt_unloaded_player_refuses() {
    preexisting_corrupt_refuses("player", "players");
}

#[test]
fn preexisting_corrupt_distant_region_refuses() {
    preexisting_corrupt_refuses("region", "dimensions/0/regions");
}

#[test]
fn missing_previous_manifest_is_usage() {
    let scope = Scope::fresh("missing");
    let run = scope.path("run");
    let world = run.join("world");
    prepare_fixture(&world, false);
    let previous = previous_bin();
    let (code, output) = run_script(&[
        "activate",
        "--world",
        &world.to_string_lossy(),
        "--backup",
        &run.join("backup").to_string_lossy(),
        "--run-dir",
        &run.to_string_lossy(),
        "--rust-bin",
        &rust_bin().to_string_lossy(),
        "--previous-bin",
        &previous.to_string_lossy(),
        "--previous-sha256",
        &sha256_file(&previous),
        "--dry-run",
    ]);
    assert_eq!(code, 2, "new callable contract requires package: {output}");
    assert!(!run.join(MANIFEST_BASENAME).exists());
    assert!(!run.join("backup").exists());
}

#[test]
fn actual_future_family_refuses_before_previous_start() {
    let scope = Scope::fresh("future");
    let run = scope.path("run");
    let world = run.join("world");
    prepare_fixture(&world, false);
    let future = scope.path("future");
    prepare_fixture(&future, true);
    let manifest = script_activate(&scope, &world, &run.join("backup"), &run, &[]);
    quiesce_rust(&manifest);
    fs::copy(
        future.join("passive_mobs.bin"),
        world.join("passive_mobs.bin"),
    )
    .unwrap();
    rollback_refuses(&manifest, &world, &run);
    let report = read_manifest(&run.join("previous-verifier-report.json"));
    assert_eq!(report["compatible"], false);
    assert_eq!(report["read_files"], 6);
    assert!(
        report["errors"]
            .as_array()
            .unwrap()
            .iter()
            .any(|error| error.as_str().unwrap().contains("passive_mobs"))
    );
}

#[test]
fn actual_restore_and_crashed_previous_reverify() {
    let scope = Scope::fresh("vrestore");
    let run = scope.path("run");
    let world = run.join("world");
    prepare_fixture(&world, false);
    let backup = run.join("backup");
    let manifest = script_activate(&scope, &world, &backup, &run, &[]);
    quiesce_rust(&manifest);
    fs::write(world.join("passive_mobs.bin"), b"broken").unwrap();
    let (code, output) = run_script(&[
        "rollback",
        "--manifest",
        &manifest.to_string_lossy(),
        "--data-policy",
        "restore-backup",
    ]);
    assert_eq!(code, 0, "actual restore verifier succeeds: {output}");
    let report_path = run.join("previous-verifier-report.json");
    let report = read_manifest(&report_path);
    assert_eq!(report["compatible"], true);
    assert_eq!(report["read_files"], 7);
    assert_eq!(
        report["world_tree_sha256"],
        tree_hash(&backup, &[LOCK_BASENAME, BACKUP_IDENTITY_BASENAME])
    );
    let running = read_manifest(&manifest);
    let report_bytes = fs::read(&report_path).unwrap();
    let report_time = fs::metadata(&report_path).unwrap().modified().unwrap();
    let (code, output) = run_script(&[
        "rollback",
        "--manifest",
        &manifest.to_string_lossy(),
        "--data-policy",
        "compatible",
    ]);
    assert_eq!(code, 0, "live idempotent rollback: {output}");
    assert_eq!(read_manifest(&manifest), running);
    assert_eq!(fs::read(&report_path).unwrap(), report_bytes);
    assert_eq!(
        fs::metadata(&report_path).unwrap().modified().unwrap(),
        report_time
    );
    stop_previous(&running);
    fs::remove_file(&report_path).unwrap();
    let restarted = rollback_compatible(&manifest, "crashed previous reverified");
    assert_eq!(read_manifest(&report_path)["compatible"], true);
    assert_ne!(restarted["pid"], running["pid"]);
    stop_previous(&restarted);
    let future = scope.path("future");
    prepare_fixture(&future, true);
    fs::copy(
        future.join("passive_mobs.bin"),
        world.join("passive_mobs.bin"),
    )
    .unwrap();
    fs::remove_file(run.join("previous-listen.addr")).unwrap();
    let before = tree_hash(&world, &[LOCK_BASENAME]);
    let (code, output) = run_script(&[
        "rollback",
        "--manifest",
        &manifest.to_string_lossy(),
        "--data-policy",
        "compatible",
    ]);
    assert_ne!(code, 0, "crashed previous future world refuses: {output}");
    assert!(output.contains("incompatible_save"));
    assert_eq!(read_manifest(&manifest)["pid"], restarted["pid"]);
    assert_eq!(tree_hash(&world, &[LOCK_BASENAME]), before);
    assert!(!run.join("previous-listen.addr").exists());
    drop(mornlea_server::store::lease::WorldLease::acquire(&world).unwrap());
}

#[test]
fn unsafe_report_and_log_aliases_refuse_without_world_mutation() {
    use std::os::unix::fs::symlink;
    for kind in [
        "report-symlink",
        "report-hardlink",
        "log-symlink",
        "log-hardlink",
    ] {
        let scope = Scope::fresh(kind);
        let run = scope.path("run");
        let world = run.join("world");
        prepare_fixture(&world, false);
        let manifest = script_activate(&scope, &world, &run.join("backup"), &run, &[]);
        quiesce_rust(&manifest);
        let output = run.join(if kind.starts_with("report") {
            "previous-verifier-report.json"
        } else {
            "previous-verifier.log"
        });
        if kind.ends_with("symlink") {
            symlink(world.join("world.meta"), output).unwrap();
        } else {
            fs::hard_link(world.join("world.meta"), output).unwrap();
        }
        rollback_refuses(&manifest, &world, &run);
    }
}

// Copies only immutable package artifacts, never edits the accepted fixture.
fn copied_package(scope: &Scope) -> (PathBuf, serde_json::Value) {
    let original = package_path();
    let original_root = original.parent().unwrap();
    let root = scope.path("package");
    let mut record = package();
    for field in ["previous_executable", "verifier_executable"] {
        let from = PathBuf::from(record[field].as_str().unwrap());
        let to = root.join(from.strip_prefix(original_root).unwrap());
        fs::create_dir_all(to.parent().unwrap()).unwrap();
        fs::copy(from, &to).unwrap();
        record[field] = serde_json::json!(to);
    }
    let from = PathBuf::from(record["native_dependencies"][0]["path"].as_str().unwrap());
    let to = root.join(from.strip_prefix(original_root).unwrap());
    fs::create_dir_all(to.parent().unwrap()).unwrap();
    fs::copy(from, &to).unwrap();
    record["native_dependencies"][0]["path"] = serde_json::json!(to);
    let relative = "previous-source/packages/server/storage/runtime_migration_verify_test.go";
    let to = root.join(relative);
    fs::create_dir_all(to.parent().unwrap()).unwrap();
    fs::copy(original_root.join(relative), to).unwrap();
    let path = root.join("previous-runtime.json");
    write_package(&path, &record);
    (path, record)
}

fn write_package(path: &Path, record: &serde_json::Value) {
    fs::write(path, serde_json::to_vec(record).unwrap()).unwrap();
}

fn custom_activate(
    world: &Path,
    run: &Path,
    package_path: &Path,
    record: &serde_json::Value,
    dry: bool,
) -> (i32, String) {
    let rust = rust_bin();
    let world = world.to_string_lossy();
    let backup = Path::new(run).join("backup");
    let backup = backup.to_string_lossy();
    let run = run.to_string_lossy();
    let package_path = package_path.to_string_lossy();
    let rust = rust.to_string_lossy();
    let mut args = vec![
        "activate",
        "--world",
        &world,
        "--backup",
        &backup,
        "--run-dir",
        &run,
        "--rust-bin",
        &rust,
        "--previous-bin",
        record["previous_executable"].as_str().unwrap(),
        "--previous-sha256",
        record["previous_sha256"].as_str().unwrap(),
        "--previous-manifest",
        &package_path,
    ];
    if dry {
        args.push("--dry-run");
    }
    run_script(&args)
}

#[test]
fn package_identity_refuses_before_stopping_live_rust() {
    let scope = Scope::fresh("identity");
    let run = scope.path("run");
    let world = run.join("world");
    prepare_fixture(&world, false);
    let (path, record) = copied_package(&scope);
    let (code, output) = custom_activate(&world, &run, &path, &record, false);
    assert_eq!(code, 0, "copy binds accepted artifact identities: {output}");
    let manifest = run.join(MANIFEST_BASENAME);
    let original = read_manifest(&manifest);
    let socket = run.join("control.sock");
    let second_scope = Scope::fresh("other-package");
    let (other_path, other_record) = copied_package(&second_scope);
    let (code, output) = custom_activate(&world, &run, &other_path, &other_record, false);
    assert_ne!(
        code, 0,
        "activation resume refuses a different package: {output}"
    );
    assert!(output.contains("identity_mismatch"));
    assert_eq!(read_manifest(&manifest), original);
    assert_single_rust_owner(&original, &socket);
    for field in [
        "source",
        "schema",
        "oracle",
        "path",
        "previous-hash",
        "verifier-hash",
        "native-hash",
        "record-bytes",
        "binding",
    ] {
        let mut changed = record.clone();
        match field {
            "source" => changed["previous_source_sha"] = serde_json::json!("0".repeat(40)),
            "schema" => changed["schema_version"] = serde_json::json!(2),
            "oracle" => changed["oracle_sha256"] = serde_json::json!("0".repeat(64)),
            "path" => {
                changed["verifier_executable"] =
                    serde_json::json!(path.parent().unwrap().join("other-verifier"))
            }
            "previous-hash" => changed["previous_sha256"] = serde_json::json!("0".repeat(64)),
            "verifier-hash" => changed["verifier_sha256"] = serde_json::json!("0".repeat(64)),
            "native-hash" => {
                changed["native_dependencies"][0]["sha256"] = serde_json::json!("0".repeat(64))
            }
            "binding" => set_manifest_field(&manifest, "previous_runtime", serde_json::json!({})),
            "record-bytes" => {}
            _ => unreachable!(),
        }
        write_package(&path, &changed);
        if field == "record-bytes" {
            let mut file = fs::OpenOptions::new().append(true).open(&path).unwrap();
            writeln!(file).unwrap();
        }
        let (code, output) = run_script(&[
            "rollback",
            "--manifest",
            &manifest.to_string_lossy(),
            "--data-policy",
            "compatible",
        ]);
        assert_ne!(code, 0, "package tamper {field} refuses: {output}");
        assert!(
            output.contains(if field == "schema" {
                "invalid_manifest"
            } else {
                "identity_mismatch"
            }),
            "typed {field}: {output}"
        );
        assert_eq!(read_manifest(&manifest)["phase"], "RustRunning");
        assert_single_rust_owner(&original, &socket);
        write_package(&path, &record);
        fs::write(&manifest, serde_json::to_vec(&original).unwrap()).unwrap();
    }
    for artifact in ["previous_executable", "verifier_executable", "native"] {
        let artifact_path = PathBuf::from(if artifact == "native" {
            record["native_dependencies"][0]["path"].as_str().unwrap()
        } else {
            record[artifact].as_str().unwrap()
        });
        let mut file = fs::OpenOptions::new()
            .append(true)
            .open(&artifact_path)
            .unwrap();
        writeln!(file, "tamper").unwrap();
        let (code, output) = run_script(&[
            "rollback",
            "--manifest",
            &manifest.to_string_lossy(),
            "--data-policy",
            "compatible",
        ]);
        assert_ne!(code, 0, "artifact tamper refuses: {output}");
        assert!(output.contains("identity_mismatch"));
        assert_single_rust_owner(&original, &socket);
        let original_path = if artifact == "native" {
            package()["native_dependencies"][0]["path"]
                .as_str()
                .unwrap()
                .to_owned()
        } else {
            package()[artifact].as_str().unwrap().to_owned()
        };
        fs::copy(original_path, artifact_path).unwrap();
    }
    quiesce_rust(&manifest);
}

#[test]
fn invalid_package_layout_shape_and_overlap_precede_activation_artifacts() {
    use std::os::unix::fs::symlink;
    for kind in [
        "extra",
        "missing",
        "duplicate",
        "oversized",
        "bool",
        "protocol",
        "save",
        "symlink-leaf",
        "symlink-ancestor",
        "overlap",
    ] {
        let scope = Scope::fresh(kind);
        let run = scope.path("run");
        let world = run.join("world");
        prepare_fixture(&world, false);
        let (mut path, mut record) = copied_package(&scope);
        match kind {
            "extra" => record["extra"] = serde_json::json!(0),
            "missing" => {
                record.as_object_mut().unwrap().remove("oracle_sha256");
            }
            "bool" => record["schema_version"] = serde_json::json!(true),
            "protocol" => record["protocol_version"] = serde_json::json!(46),
            "save" => record["save_schemas"]["passive_mobs"] = serde_json::json!(2),
            _ => {}
        }
        write_package(&path, &record);
        match kind {
            "duplicate" => {
                let text = fs::read_to_string(&path).unwrap();
                fs::write(&path, text.replacen('{', "{\"schema_version\":1,", 1)).unwrap();
            }
            "oversized" => {
                fs::write(&path, vec![b' '; 1024 * 1024 + 1]).unwrap();
            }
            "symlink-leaf" => {
                let saved = path.with_extension("saved");
                fs::rename(&path, &saved).unwrap();
                symlink(saved, &path).unwrap();
            }
            "symlink-ancestor" => {
                let alias = scope.path("alias");
                symlink(path.parent().unwrap(), &alias).unwrap();
                path = alias.join("previous-runtime.json");
            }
            "overlap" => {
                // A mutable run ancestor may not contain package provenance.
                let (code, output) = custom_activate(&world, &scope.root, &path, &record, true);
                assert_ne!(code, 0, "package overlap refuses: {output}");
                assert!(output.contains("invalid_manifest"));
                assert!(!scope.path(MANIFEST_BASENAME).exists());
                assert!(!scope.path("backup").exists());
                continue;
            }
            _ => {}
        }
        let (code, output) = custom_activate(&world, &run, &path, &record, true);
        assert_ne!(code, 0, "invalid {kind}: {output}");
        assert!(
            output.contains(if matches!(kind, "protocol" | "save") {
                "incompatible_save"
            } else {
                "invalid_manifest"
            }),
            "typed {kind}: {output}"
        );
        assert!(!run.join(MANIFEST_BASENAME).exists());
        assert!(!run.join("backup").exists());
    }
}

#[test]
fn strict_report_rejections_are_negative_contract_evidence() {
    let scope = Scope::fresh("reports");
    let run = scope.path("run");
    let world = run.join("world");
    prepare_fixture(&world, false);
    let manifest = script_activate(&scope, &world, &run.join("backup"), &run, &[]);
    quiesce_rust(&manifest);
    let original = read_manifest(&manifest);
    let before = tree_hash(&world, &[LOCK_BASENAME]);
    let harness = scope.path("negative-report.py");
    // The production oracle still executes. Only its emitted report is damaged,
    // and every path must refuse; this cannot establish a successful provider.
    fs::write(&harness, r#"
import json, os, subprocess, sys
kind = os.environ['NEGATIVE_REPORT_KIND']
os.environ['MORNLEA_VERIFY_FIXTURE'] = '/must-not-create-a-fixture'
os.environ['MORNLEA_VERIFY_FIXTURE_FUTURE'] = '1'
actual_run = subprocess.run
def damaged_report(args, **kwargs):
    assert args[1:] == ['-test.run=^TestRuntimeMigrationVerifyWorld$', '-test.count=1', '-test.timeout=55s']
    assert kwargs['timeout'] == 60
    assert 'MORNLEA_VERIFY_FIXTURE' not in kwargs['env']
    assert 'MORNLEA_VERIFY_FIXTURE_FUTURE' not in kwargs['env']
    outcome = actual_run(args, **kwargs)
    assert outcome.returncode == 0
    path = kwargs['env']['MORNLEA_VERIFY_OUTPUT']
    with open(path) as handle:
        report = json.load(handle)
    if kind == 'missing':
        os.unlink(path)
        return outcome
    if kind == 'extra': report['extra'] = 0
    if kind == 'missing-field': del report['read_files']
    if kind == 'bool-schema': report['schema_version'] = True
    if kind == 'bool-read': report['read_files'] = True
    if kind == 'no-read': report['read_files'] = 0
    if kind == 'false': report['compatible'] = False
    if kind == 'errors': report['errors'] = ['bad save']
    if kind == 'stale-executable': report['executable_sha256'] = '0' * 64
    if kind == 'stale-source': report['source_sha'] = '0' * 40
    if kind == 'stale-world': report['world_tree_sha256'] = '0' * 64
    if kind == 'uppercase-hash': report['world_tree_sha256'] = 'F' * 64
    raw = json.dumps(report)
    if kind == 'duplicate': raw = raw.replace('{', '{"read_files":1,', 1)
    if kind == 'malformed': raw = '{'
    if kind == 'oversized': raw = ' ' * (1024 * 1024 + 1)
    with open(path, 'w') as handle:
        handle.write(raw)
    return outcome
subprocess.run = damaged_report
exec(compile(sys.stdin.read(), '<actual verifier consumer>', 'exec'))
"#).unwrap();
    for kind in [
        "missing",
        "extra",
        "missing-field",
        "bool-schema",
        "bool-read",
        "no-read",
        "false",
        "errors",
        "stale-executable",
        "stale-source",
        "stale-world",
        "uppercase-hash",
        "duplicate",
        "malformed",
        "oversized",
    ] {
        let action = "export NEGATIVE_REPORT_KIND=\"$5\"\nnegative_report_harness=\"$4\"\npy() { python3 \"$negative_report_harness\" \"$@\"; }\nverify_previous_world \"$1\" \"$2\" \"$3\"";
        let (code, output) = run_script_functions(
            &scope,
            action,
            &[
                &manifest.to_string_lossy(),
                &world.to_string_lossy(),
                &run.to_string_lossy(),
                &harness.to_string_lossy(),
                kind,
            ],
        );
        assert_ne!(code, 0, "negative report {kind} refuses: {output}");
        assert!(
            output.contains("incompatible_save"),
            "typed report {kind}: {output}"
        );
        assert_eq!(read_manifest(&manifest), original);
        assert_eq!(tree_hash(&world, &[LOCK_BASENAME]), before);
        assert!(!run.join("previous-listen.addr").exists());
    }
}

#[test]
fn actual_timeout_kills_and_collects_only_the_verifier_child() {
    use std::os::unix::fs::PermissionsExt;
    let scope = Scope::fresh("timeout");
    let run = scope.path("run");
    let world = run.join("world");
    prepare_fixture(&world, false);
    let manifest = script_activate(&scope, &world, &run.join("backup"), &run, &[]);
    quiesce_rust(&manifest);
    let original = read_manifest(&manifest);
    let before = tree_hash(&world, &[LOCK_BASENAME]);
    let child = scope.path("negative-verifier");
    // This negative deadline fixture never supplies compatible-provider proof.
    // Exec retains one PID, so subprocess.run must kill and wait for that owner.
    fs::write(&child, "#!/usr/bin/env python3\nimport os, time\nprint(os.getpid(), flush=True)\ntime.sleep(120)\n").unwrap();
    fs::set_permissions(&child, fs::Permissions::from_mode(0o700)).unwrap();
    let mut record = package();
    record["verifier_executable"] = serde_json::json!(child);
    record["verifier_sha256"] = serde_json::json!(sha256_file(&child));
    set_manifest_field(&manifest, "previous_runtime", record);
    let mut unrelated = TerminationChild(Command::new("sleep").arg("120").spawn().unwrap());
    let started = Instant::now();
    let (code, output) = run_script_functions(
        &scope,
        "verify_previous_world \"$1\" \"$2\" \"$3\"",
        &[
            &manifest.to_string_lossy(),
            &world.to_string_lossy(),
            &run.to_string_lossy(),
        ],
    );
    assert_ne!(code, 0, "negative child times out: {output}");
    assert!(
        output.contains("incompatible_save") && output.contains("timed out after"),
        "typed absolute timeout: {output}"
    );
    assert!(started.elapsed() >= Duration::from_secs(60));
    assert!(started.elapsed() < Duration::from_secs(70));
    let pid: u32 = fs::read_to_string(run.join("previous-verifier.log"))
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    assert!(process_terminated(pid));
    assert!(
        !Path::new(&format!("/proc/{pid}")).exists(),
        "owned child was collected, not left zombie"
    );
    assert!(
        unrelated.0.try_wait().unwrap().is_none(),
        "unrelated child stays live"
    );
    assert_eq!(read_manifest(&manifest)["phase"], original["phase"]);
    assert_eq!(tree_hash(&world, &[LOCK_BASENAME]), before);
    assert!(!run.join("previous-listen.addr").exists());
    unrelated.0.kill().unwrap();
    unrelated.0.wait().unwrap();
}

#[test]
fn rejected_restore_retains_installed_backup_and_retired_world() {
    let scope = Scope::fresh("badrestore");
    let run = scope.path("run");
    let world = run.join("world");
    prepare_fixture(&world, false);
    let player = fs::read_dir(world.join("players"))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    fs::write(player, b"preexisting corrupt backup player").unwrap();
    let backup = run.join("backup");
    let manifest = script_activate(&scope, &world, &backup, &run, &[]);
    quiesce_rust(&manifest);
    fs::write(world.join("after-activation.bin"), b"retained changed tree").unwrap();
    let changed = tree_hash(&world, &[LOCK_BASENAME]);
    let nonce = manifest_str(&read_manifest(&manifest), "start_nonce");
    let (code, output) = run_script(&[
        "rollback",
        "--manifest",
        &manifest.to_string_lossy(),
        "--data-policy",
        "restore-backup",
    ]);
    assert_ne!(code, 0, "actual installed backup oracle refuses: {output}");
    assert!(output.contains("incompatible_save"));
    assert_eq!(
        tree_hash(&world, &[LOCK_BASENAME]),
        tree_hash(&backup, &[LOCK_BASENAME, BACKUP_IDENTITY_BASENAME])
    );
    assert_eq!(
        tree_hash(
            &PathBuf::from(format!("{}.retired.{nonce}", world.display())),
            &[LOCK_BASENAME]
        ),
        changed
    );
    assert_eq!(
        read_manifest(&manifest)["restore_stage"],
        "backup_installed"
    );
    assert_eq!(
        read_manifest(&run.join("previous-verifier-report.json"))["compatible"],
        false
    );
    assert!(!run.join("previous-listen.addr").exists());
    drop(mornlea_server::store::lease::WorldLease::acquire(&world).unwrap());
}

/// Owns the rollback process group so a blocked pre-spawn log open can be
/// collected without leaving its Python helper or affecting another fixture.
struct RollbackGroup(Option<Child>);

impl RollbackGroup {
    fn collect(mut self, returned: bool) -> std::process::Output {
        if !returned {
            self.stop_group();
        }
        self.0.take().unwrap().wait_with_output().unwrap()
    }

    fn stop_group(&mut self) {
        if let Some(child) = &mut self.0 {
            let _ = Command::new("kill")
                .args(["-KILL", "--", &format!("-{}", child.id())])
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

impl Drop for RollbackGroup {
    fn drop(&mut self) {
        self.stop_group();
    }
}

fn fifo_log_refuses_with_bounded_cli(held_reader: bool) {
    use std::os::unix::fs::OpenOptionsExt;
    use std::os::unix::process::CommandExt;

    let scope = Scope::fresh(if held_reader {
        "fifo-reader"
    } else {
        "fifo-alone"
    });
    let run = scope.path("run");
    let world = run.join("world");
    prepare_fixture(&world, false);
    let manifest = script_activate(&scope, &world, &run.join("backup"), &run, &[]);
    quiesce_rust(&manifest);
    let original = read_manifest(&manifest);
    let before = tree_hash(&world, &[LOCK_BASENAME]);
    let log = run.join("previous-verifier.log");
    assert!(Command::new("mkfifo").arg(&log).status().unwrap().success());
    // A nonblocking reader is held by this test, never by an unjoined thread.
    // It makes a FIFO writer open succeed so descriptor validation is exercised.
    let reader = held_reader.then(|| {
        fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NONBLOCK | libc::O_CLOEXEC)
            .open(&log)
            .unwrap()
    });
    let mut rollback = RollbackGroup(Some(
        Command::new("bash")
            .arg(optin_script())
            .args([
                "rollback",
                "--manifest",
                &manifest.to_string_lossy(),
                "--data-policy",
                "compatible",
            ])
            .process_group(0)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap(),
    ));
    let returned = wait_until(Duration::from_secs(5), || {
        rollback.0.as_mut().unwrap().try_wait().unwrap().is_some()
    });
    // Release the reader and collect the complete owned group before any
    // observation assertion, including the expected failure on the old script.
    drop(reader);
    let output = rollback.collect(returned);
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        returned,
        "FIFO log must refuse before the verifier deadline: {text}"
    );
    assert!(!output.status.success(), "FIFO log is incompatible: {text}");
    assert!(
        text.contains("incompatible_save"),
        "typed FIFO refusal: {text}"
    );
    assert_eq!(read_manifest(&manifest), original);
    assert_eq!(tree_hash(&world, &[LOCK_BASENAME]), before);
    assert!(!run.join("previous-listen.addr").exists());
    assert!(!run.join("previous-verifier-report.json").exists());
    drop(mornlea_server::store::lease::WorldLease::acquire(&world).unwrap());
}

#[test]
fn fifo_log_without_reader_refuses_before_verifier_spawn() {
    fifo_log_refuses_with_bounded_cli(false);
}

#[test]
fn fifo_log_with_held_reader_refuses_before_verifier_spawn() {
    fifo_log_refuses_with_bounded_cli(true);
}

/// Extracts the actual `cmd_rollback` batch consumer reads without
/// executing the CLI, using the same anchored split technique as
/// `process_termination_shell`.
fn rollback_batch_consumer_reads() -> String {
    let source = fs::read_to_string(optin_script()).expect("read production rollback consumer");
    let (_, block) = source
        .split_once(
            "\n    local world backup socket nonce previous recorded_previous phase\n    {\n",
        )
        .expect("production rollback batch consumer exists");
    let (reads, _) = block
        .split_once("\n    } < <(manifest_get_batch ")
        .expect("production rollback batch consumer closes");
    reads.to_owned()
}

/// The batched manifest read must keep NUL as an exclusive field
/// separator: an embedded NUL inside one field, or a stream shorter than
/// the declared fields, refuses with a typed `invalid_manifest` instead of
/// silently misaligning every later consumer. Ordinary and empty fields
/// still decode in argument order, and the real rollback CLI refuses a
/// NUL-bearing manifest before producing any side effect.
#[test]
fn manifest_batch_embedded_nul_and_truncation_refuse() {
    // A JSON `\u0000` escape decodes to a literal NUL inside the field, so
    // the extracted helper refuses that field as it is reached; the NUL
    // sits in the first requested field here, so the refusal precedes any
    // separator, while a later field's refusal may follow earlier emitted
    // fields and the consumers' short-batch refusal covers that stream.
    let scope = Scope::fresh("batch-nul");
    let manifest = scope.path("nul.json");
    fs::write(
        &manifest,
        "{\"world_path\": \"a\\u0000b\", \"phase\": \"RustRunning\"}",
    )
    .unwrap();
    let (code, output) = run_script_functions(
        &scope,
        "manifest_get_batch \"$1\" world_path phase",
        &[&manifest.to_string_lossy()],
    );
    assert_ne!(code, 0, "embedded NUL field refuses: {output}");
    assert!(
        output.contains("FAIL invalid_manifest")
            && output.contains("embedded NUL")
            && output.contains("world_path"),
        "typed embedded NUL refusal: {output}"
    );

    // Empty and absent fields occupy their own slots, and ordinary fields
    // (strings, integers, booleans) decode in the exact argument order.
    let scope = Scope::fresh("batch-order");
    let manifest = scope.path("fields.json");
    fs::write(
        &manifest,
        r#"{"empty_field": "", "present_field": "value", "count": 7, "flag": true}"#,
    )
    .unwrap();
    let action = r#"{ IFS= read -rd '' first
IFS= read -rd '' second
IFS= read -rd '' third
IFS= read -rd '' fourth
IFS= read -rd '' fifth
} < <(manifest_get_batch "$1" empty_field present_field count flag absent_field)
printf 'first=[%s]second=[%s]third=[%s]fourth=[%s]fifth=[%s]' "$first" "$second" "$third" "$fourth" "$fifth""#;
    let (code, output) = run_script_functions(&scope, action, &[&manifest.to_string_lossy()]);
    assert_eq!(code, 0, "ordinary batched fields decode: {output}");
    assert_eq!(
        output.trim_end(),
        "first=[]second=[value]third=[7]fourth=[true]fifth=[]",
        "empty and absent fields keep their own slots in argument order"
    );

    // A stream with fewer terminators than fields must refuse through the
    // exact `cmd_rollback` consumer reads rather than leaving later fields
    // silently empty.
    let scope = Scope::fresh("batch-short");
    let truncated = scope.path("truncated.bin");
    fs::write(
        &truncated,
        b"world-value\0backup-value\0partial-without-terminator",
    )
    .unwrap();
    let reads = rollback_batch_consumer_reads();
    let action = format!("{{\n{reads}\n}} < \"$1\"\necho extraction-should-not-continue");
    let (code, output) = run_script_functions(&scope, &action, &[&truncated.to_string_lossy()]);
    assert_ne!(code, 0, "truncated batch refuses: {output}");
    assert!(
        output.contains("FAIL invalid_manifest")
            && output.contains("manifest field batch is incomplete"),
        "typed truncation refusal: {output}"
    );
    assert!(
        !output.contains("extraction-should-not-continue"),
        "truncation refusal stops the consumer: {output}"
    );

    // End to end: the real CLI refuses a NUL-bearing recorded path before
    // any rollback side effect, bounded by the same deadline the FIFO
    // fixtures already impose on this CLI.
    use std::os::unix::process::CommandExt;

    let scope = Scope::fresh("batch-cli");
    let run = scope.path("run");
    let world = run.join("world");
    prepare_fixture(&world, false);
    let manifest = script_activate(&scope, &world, &run.join("backup"), &run, &[]);
    quiesce_rust(&manifest);
    let before = tree_hash(&world, &[LOCK_BASENAME]);
    set_manifest_field(&manifest, "world_path", serde_json::json!("a\0b"));
    let corrupted = fs::read(&manifest).unwrap();
    let mut rollback = RollbackGroup(Some(
        Command::new("bash")
            .arg(optin_script())
            .args([
                "rollback",
                "--manifest",
                &manifest.to_string_lossy(),
                "--data-policy",
                "compatible",
            ])
            .process_group(0)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap(),
    ));
    let returned = wait_until(Duration::from_secs(5), || {
        rollback.0.as_mut().unwrap().try_wait().unwrap().is_some()
    });
    let output = rollback.collect(returned);
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        returned,
        "embedded NUL refuses before the verifier deadline: {text}"
    );
    assert!(
        !output.status.success(),
        "embedded NUL world path is invalid: {text}"
    );
    assert!(
        text.contains("FAIL invalid_manifest") && text.contains("embedded NUL"),
        "typed embedded NUL refusal: {text}"
    );
    assert_eq!(
        fs::read(&manifest).unwrap(),
        corrupted,
        "refusal mutates no manifest byte"
    );
    assert_eq!(tree_hash(&world, &[LOCK_BASENAME]), before);
    assert!(!run.join("previous-listen.addr").exists());
    assert!(!run.join("previous-verifier-report.json").exists());
    assert!(!run.join("previous-verifier.log").exists());
    drop(mornlea_server::store::lease::WorldLease::acquire(&world).unwrap());
}
