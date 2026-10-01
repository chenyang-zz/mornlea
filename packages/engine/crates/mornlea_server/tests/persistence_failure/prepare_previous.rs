//! Sealed previous package builds and actual offline reader qualification.

use super::{Scope, hex_of, repo_root, run_script, sha256_file, tree_hash};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::fs;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::{Path, PathBuf};
use std::process::Command;

const SOURCE: &str = "d042982d33bb1694d768b75b01c297bd02534a08";
const ORACLE: &str = "packages/server/storage/runtime_migration_verify_test.go";

fn git_bytes(identity: &str) -> Vec<u8> {
    let output = Command::new("git")
        .arg("-C")
        .arg(repo_root())
        .args(["show", identity])
        .output()
        .expect("read tracked source");
    assert!(output.status.success(), "git show {identity}");
    output.stdout
}

fn prepare(source: &str, root: &Path) -> (i32, String) {
    run_script(&[
        "prepare-previous",
        "--source",
        source,
        "--run-dir",
        root.to_str().expect("utf8 run directory"),
    ])
}

fn identity(paths: &[PathBuf]) -> BTreeMap<PathBuf, (u32, String)> {
    paths
        .iter()
        .map(|path| {
            let meta = fs::symlink_metadata(path).expect("stat package artifact");
            assert!(meta.is_file(), "regular artifact {}", path.display());
            (path.clone(), (meta.permissions().mode(), sha256_file(path)))
        })
        .collect()
}

fn assert_lease_free(world: &Path) {
    let output = Command::new("python3")
        .args(["-c", "import fcntl,os,sys; fd=os.open(sys.argv[1],os.O_RDONLY); fcntl.flock(fd,fcntl.LOCK_EX|fcntl.LOCK_NB); fcntl.flock(fd,fcntl.LOCK_UN); os.close(fd)"])
        .arg(world.join("world.lock"))
        .output()
        .expect("reacquire actual world lease");
    assert!(output.status.success(), "lease released: {output:?}");
}

#[test]
fn actual_package_binds_sealed_source_native_and_offline_reader() {
    let scope = Scope::fresh("prepare");
    let root = scope.path("package");
    let (code, output) = prepare(SOURCE, &root);
    assert_eq!(code, 0, "actual new-command contract: {output}");
    let manifest_path = root.join("previous-runtime.json");
    assert_eq!(
        output
            .lines()
            .filter(|line| line.starts_with("OK "))
            .collect::<Vec<_>>(),
        [format!(
            "OK prepared previous_manifest={} source={SOURCE}",
            manifest_path.display()
        )]
    );
    let manifest: Value = serde_json::from_slice(&fs::read(&manifest_path).unwrap()).unwrap();
    let server = root.join("previous-server");
    let verifier = root.join("previous-verifier");
    let exported = root.join("previous-source");
    let native = exported.join(format!(
        "packages/engine/target/release/libmornlea_engine.{}",
        if cfg!(target_os = "macos") {
            "dylib"
        } else {
            "so"
        }
    ));
    let oracle = exported.join(ORACLE);
    let expected = json!({
        "schema_version": 1,
        "previous_source_sha": SOURCE,
        "oracle_sha256": hex_of(&git_bytes(&format!("HEAD:{ORACLE}"))),
        "previous_executable": server,
        "previous_sha256": sha256_file(&server),
        "verifier_executable": verifier,
        "verifier_sha256": sha256_file(&verifier),
        "protocol_version": 45,
        "save_schemas": {"player":9,"chunk":9,"world_metadata":6,"companions_ai":5,"hostile_mobs":2,"passive_mobs":1},
        "native_dependencies": [{"path":native,"sha256":sha256_file(&native)}]
    });
    assert_eq!(
        manifest, expected,
        "exact package schema and actual identities"
    );
    assert_eq!(
        fs::read(&oracle).unwrap(),
        git_bytes(&format!("HEAD:{ORACLE}"))
    );
    for path in [
        "packages/server/cmd/mornlea-server/main.go",
        "packages/server/storage/disk.go",
        "packages/shared/nativeabi/native.go",
    ] {
        assert_eq!(
            fs::read(exported.join(path)).unwrap(),
            git_bytes(&format!("{SOURCE}:{path}")),
            "sealed source {path}"
        );
    }
    assert!(!exported.join(".git").exists());
    assert!(
        !exported
            .join("packages/engine/crates/mornlea_server")
            .exists()
    );
    for executable in [&server, &verifier] {
        assert_ne!(
            fs::metadata(executable).unwrap().permissions().mode() & 0o111,
            0
        );
    }
    let help = Command::new(&server)
        .arg("--help")
        .output()
        .expect("actual previous server help");
    assert_eq!(help.status.code(), Some(1), "sealed Go help outcome");
    assert!(
        String::from_utf8_lossy(&help.stderr).contains("flag: help requested"),
        "actual Go help: {help:?}"
    );
    assert!(!root.join("world").exists());
    let artifacts = vec![manifest_path, server, verifier.clone(), native, oracle];
    let before = identity(&artifacts);
    let (code, output) = prepare(SOURCE, &root);
    assert_eq!(code, 1, "repeat refuses: {output}");
    assert_eq!(
        before,
        identity(&artifacts),
        "repeat preserves artifact bytes and modes"
    );

    for future in [false, true] {
        let world = scope.path(if future { "future-world" } else { "world" });
        let fixture = Command::new(&verifier)
            .current_dir(exported.join("packages/server/storage"))
            .args([
                "-test.run=^TestRuntimeMigrationPrepareCallableFixture$",
                "-test.count=1",
            ])
            .env("MORNLEA_VERIFY_FIXTURE", &world)
            .env(
                "MORNLEA_VERIFY_FIXTURE_FUTURE",
                if future { "1" } else { "0" },
            )
            .output()
            .expect("actual packaged fixture producer");
        assert!(fixture.status.success(), "fixture: {fixture:?}");
        let before = tree_hash(&world, &["world.lock"]);
        let report_path = scope.path(if future {
            "future-report.json"
        } else {
            "report.json"
        });
        let checked = Command::new(&verifier)
            .args([
                "-test.run=^TestRuntimeMigrationVerifyWorld$",
                "-test.count=1",
            ])
            .env("MORNLEA_VERIFY_WORLD", &world)
            .env("MORNLEA_VERIFY_OUTPUT", &report_path)
            .output()
            .expect("actual packaged offline verifier");
        assert_eq!(
            checked.status.code(),
            Some(i32::from(future)),
            "actual old reader: {checked:?}"
        );
        let report: Value = serde_json::from_slice(&fs::read(report_path).unwrap()).unwrap();
        assert_eq!(report["compatible"], !future);
        assert_eq!(report["read_files"], if future { 6 } else { 7 });
        assert_eq!(report["source_sha"], SOURCE);
        assert_eq!(report["executable_sha256"], manifest["verifier_sha256"]);
        if future {
            assert!(
                report["errors"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|error| error.as_str().unwrap().starts_with("passive_mobs.bin:"))
            );
        } else {
            assert_eq!(report["world_tree_sha256"], before);
            assert_eq!(report["errors"], json!([]));
        }
        assert_eq!(
            tree_hash(&world, &["world.lock"]),
            before,
            "offline reader preserves durable tree"
        );
        assert_lease_free(&world);
    }
}

#[test]
fn inputs_refuse_before_destination_changes_or_builds() {
    let scope = Scope::fresh("prepare-input");
    for source in ["1111111111111111111111111111111111111111", "d042982d"] {
        let destination = scope.path(source);
        let (code, output) = prepare(source, &destination);
        assert_eq!(code, 1, "source refuses: {output}");
        assert!(output.contains("FAIL incompatible_save"), "{output}");
        assert!(!destination.exists());
    }
    let occupied = scope.path("occupied");
    fs::create_dir(&occupied).unwrap();
    fs::write(occupied.join("keep"), b"unchanged").unwrap();
    let leaf = scope.path("file");
    fs::write(&leaf, b"unchanged").unwrap();
    let alias = scope.path("alias");
    symlink(&occupied, &alias).unwrap();
    let before = identity(&[occupied.join("keep"), leaf.clone()]);
    let outside = repo_root().join("prepare-previous-must-not-exist");
    let unclean = PathBuf::from(format!("{}/./unclean", scope.root.display()));
    let paths = [
        PathBuf::from("relative-root"),
        unclean,
        alias.clone(),
        alias.join("child"),
        occupied.clone(),
        leaf.clone(),
        outside.clone(),
        repo_root(),
    ];
    for path in paths {
        let (code, output) = prepare(SOURCE, &path);
        assert_eq!(code, 1, "unsafe destination {}: {output}", path.display());
        assert!(output.contains("FAIL invalid_manifest"), "{output}");
        assert_eq!(before, identity(&[occupied.join("keep"), leaf.clone()]));
    }
    assert!(!outside.exists());
    assert!(!occupied.join("child").exists());
    assert!(!scope.path("unclean").exists());
    for args in [
        vec!["prepare-previous"],
        vec!["prepare-previous", "--unknown"],
        vec!["prepare-previous", "--source"],
    ] {
        let (code, output) = run_script(&args);
        assert_eq!(code, 2, "usage refusal: {output}");
    }
}

#[test]
fn missing_make_preserves_export_without_completed_package() {
    let scope = Scope::fresh("prepare-tool");
    let tools = scope.path("tools");
    fs::create_dir(&tools).unwrap();
    for tool in ["bash", "python3", "git", "dirname", "basename"] {
        let located = Command::new("bash")
            .args(["-c", "command -v \"$1\"", "locate", tool])
            .output()
            .unwrap();
        assert!(located.status.success());
        symlink(
            String::from_utf8(located.stdout).unwrap().trim(),
            tools.join(tool),
        )
        .unwrap();
    }
    let root = scope.path("package");
    let output = Command::new(tools.join("bash"))
        .arg(repo_root().join("scripts/rust-server-opt-in.sh"))
        .args(["prepare-previous", "--source", SOURCE, "--run-dir"])
        .arg(&root)
        .env("PATH", &tools)
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("FAIL activation_failed make rust"),
        "{output:?}"
    );
    assert!(
        root.join("previous-source").join(ORACLE).is_file(),
        "partial sealed export retained"
    );
    assert!(
        !root.join("previous-runtime.json").exists(),
        "build failure never publishes package"
    );
}
