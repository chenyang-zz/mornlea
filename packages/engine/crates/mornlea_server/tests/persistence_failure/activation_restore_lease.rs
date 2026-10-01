//! Actual restore ownership and native directory transition consumers.

use super::*;
use mornlea_server::store::lease::WorldLease;
use std::os::unix::fs::MetadataExt;

fn restore(scope: &Scope, manifest: &Path, world: &Path, backup: &Path) -> (i32, String) {
    let nonce = manifest_str(&read_manifest(manifest), "start_nonce");
    run_script_functions(
        scope,
        "restore_backup_world \"$1\" \"$2\" \"$3\" \"$4\"",
        &[
            &manifest.to_string_lossy(),
            &world.to_string_lossy(),
            &backup.to_string_lossy(),
            &nonce,
        ],
    )
}

fn inode(path: &Path) -> (u64, u64) {
    let info = fs::symlink_metadata(path).expect("stat real inode");
    (info.dev(), info.ino())
}

fn go_refuses(scope: &Scope, world: &Path) {
    let log_path = scope.path("refused-go.log");
    let log = File::create(&log_path).expect("refusal log");
    let mut child = Command::new(previous_bin())
        .args([
            "--world",
            &world.to_string_lossy(),
            "--listen",
            "127.0.0.1:0",
        ])
        .stdout(log.try_clone().expect("clone log"))
        .stderr(log)
        .spawn()
        .expect("spawn actual previous writer");
    if !wait_until(Duration::from_secs(15), || {
        child.try_wait().expect("poll Go").is_some()
    }) {
        stop_child(&mut child, "unexpected admitted Go writer");
        panic!("actual Go writer was admitted while restore owns the lease");
    }
    assert!(!child.wait().expect("collect Go").success());
    let log = fs::read_to_string(log_path).expect("read refusal");
    assert!(log.contains("lock"), "actual native lock refusal: {log}");
}

#[test]
fn held_real_world_lease_refuses_exact_restore_wrapper_without_mutation() {
    let scope = Scope::fresh("held-restore");
    let run = scope.path("run");
    let world = run.join("world");
    let backup = run.join("backup");
    let (mut go, _, _, _) = prepare_go_world(&previous_bin(), &world, "restore-held");
    stop_child(&mut go, "preparing Go world");
    let manifest = script_activate(&scope, &world, &backup, &run, &[]);
    let record = quiesce_rust(&manifest);
    let nonce = manifest_str(&record, "start_nonce");
    let before = fs::read(&manifest).expect("manifest before");
    let current = tree_hash(&world, &[LOCK_BASENAME]);
    let backup_hash = tree_hash(&backup, &[LOCK_BASENAME, BACKUP_IDENTITY_BASENAME]);
    let directory = inode(&world);
    let lock = inode(&world.join(LOCK_BASENAME));
    let owner = WorldLease::acquire(&world).expect("hold actual Rust lease");
    let (code, output) = restore(&scope, &manifest, &world, &backup);
    assert_ne!(
        code, 0,
        "held native lease must refuse restoration: {output}"
    );
    assert!(output.contains("FAIL writer_live"), "{output}");
    assert_eq!(fs::read(&manifest).expect("manifest after"), before);
    assert_eq!(tree_hash(&world, &[LOCK_BASENAME]), current);
    assert_eq!(
        tree_hash(&backup, &[LOCK_BASENAME, BACKUP_IDENTITY_BASENAME]),
        backup_hash
    );
    assert_eq!(inode(&world), directory);
    assert_eq!(inode(&world.join(LOCK_BASENAME)), lock);
    assert!(!PathBuf::from(format!("{}.restore.{nonce}", world.display())).exists());
    assert!(!PathBuf::from(format!("{}.retired.{nonce}", world.display())).exists());
    go_refuses(&scope, &world);
    drop(owner);
    WorldLease::acquire(&world).expect("lease releases after caller drops it");
}

struct RestoreFixture {
    scope: Scope,
    manifest: PathBuf,
    world: PathBuf,
    backup: PathBuf,
    seed: u64,
    player: [u8; 16],
    directory: (u64, u64),
    lock: (u64, u64),
}

impl RestoreFixture {
    fn new(name: &str, equal: bool) -> Self {
        let scope = Scope::fresh(name);
        let run = scope.path("run");
        let world = run.join("world");
        let backup = run.join("backup");
        let (mut go, _, seed, player) = prepare_go_world(&previous_bin(), &world, "restore-probe");
        stop_child(&mut go, "preparing Go restore fixture");
        let manifest = script_activate(&scope, &world, &backup, &run, &[]);
        quiesce_rust(&manifest);
        if !equal {
            fs::write(world.join("original-only.dat"), b"retained original bytes")
                .expect("distinguish the original directory role");
        }
        let directory = inode(&world);
        let lock = inode(&world.join(LOCK_BASENAME));
        Self {
            scope,
            manifest,
            world,
            backup,
            seed,
            player,
            directory,
            lock,
        }
    }

    fn sibling(&self, kind: &str) -> PathBuf {
        let nonce = manifest_str(&read_manifest(&self.manifest), "start_nonce");
        PathBuf::from(format!("{}.{kind}.{nonce}", self.world.display()))
    }

    fn assert_installed(&self) {
        let record = read_manifest(&self.manifest);
        assert_eq!(record["restore_stage"], "backup_installed");
        assert_eq!(
            record["restore_directory_identity"],
            format!("{}:{}", self.directory.0, self.directory.1)
        );
        assert_eq!(inode(&self.sibling("retired")), self.directory);
        assert_eq!(inode(&self.world.join(LOCK_BASENAME)), self.lock);
        assert_eq!(
            inode(&self.sibling("retired").join(LOCK_BASENAME)),
            self.lock
        );
        assert_eq!(
            tree_hash(&self.world, &[LOCK_BASENAME]),
            tree_hash(&self.backup, &[LOCK_BASENAME, BACKUP_IDENTITY_BASENAME])
        );
        assert!(!self.sibling("restore").exists());
        let guard = WorldLease::acquire(&self.world).expect("completed helper released its lease");
        assert!(WorldLease::acquire(&self.sibling("retired")).is_err());
        drop(guard);
    }

    fn actual_restart(&self) {
        let (code, output) = run_script(&[
            "rollback",
            "--manifest",
            &self.manifest.to_string_lossy(),
            "--data-policy",
            "restore-backup",
        ]);
        assert_eq!(code, 0, "sealed verifier and actual Go restart: {output}");
        let record = read_manifest(&self.manifest);
        let report = read_manifest(
            &self
                .manifest
                .parent()
                .unwrap()
                .join("previous-verifier-report.json"),
        );
        assert_eq!(report["compatible"], true);
        assert_eq!(
            report["world_tree_sha256"],
            tree_hash(&self.backup, &[LOCK_BASENAME, BACKUP_IDENTITY_BASENAME])
        );
        let (_, seed) = go_login(
            &last_go_listen(self.manifest.parent().unwrap()),
            self.player,
            "restore-probe",
        )
        .expect("actual previous Go admits original identity");
        assert_eq!(seed, self.seed);
        assert!(WorldLease::acquire(&self.world).is_err());
        stop_previous(&record);
    }
}

/// Own only this helper process; collection never reaps another case's writer.
struct HelperChild {
    child: Child,
    marker: PathBuf,
    log: PathBuf,
}

impl HelperChild {
    fn paused(fixture: &RestoreFixture, selected: &str) -> Self {
        let marker = fixture.scope.path("restore-boundary");
        let log = fixture.scope.path("restore-helper.log");
        let handle = File::create(&log).expect("helper log");
        let source = r#"
import importlib.util, pathlib, select, sys
spec = importlib.util.spec_from_file_location('restore', sys.argv[1])
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)
def boundary(name):
    if name == sys.argv[6]:
        pathlib.Path(sys.argv[7]).write_text(name)
        if not select.select([sys.stdin], [], [], 15)[0]:
            raise RuntimeError('owned boundary deadline expired')
        if sys.stdin.readline().strip() != 'continue':
            raise RuntimeError('owned boundary refused continuation')
try:
    module._restore(pathlib.Path(sys.argv[2]), pathlib.Path(sys.argv[3]),
                    pathlib.Path(sys.argv[4]), sys.argv[5], boundary=boundary)
except module.Refusal as error:
    print('FAIL', error.code, error, file=sys.stderr)
    sys.exit(1)
"#;
        let child = Command::new("python3")
            .env("PYTHONDONTWRITEBYTECODE", "1")
            .args(["-c", source])
            .arg(repo_root().join("scripts/rust-server-restore.py"))
            .arg(&fixture.manifest)
            .arg(&fixture.world)
            .arg(&fixture.backup)
            .arg(manifest_str(
                &read_manifest(&fixture.manifest),
                "start_nonce",
            ))
            .arg(selected)
            .arg(&marker)
            .stdin(Stdio::piped())
            .stdout(handle.try_clone().expect("clone helper log"))
            .stderr(handle)
            .spawn()
            .expect("spawn actual restore constructor");
        let mut owned = Self { child, marker, log };
        assert!(
            wait_until(Duration::from_secs(15), || {
                if owned.child.try_wait().expect("poll helper").is_some() {
                    panic!(
                        "helper exited before {selected}: {}",
                        fs::read_to_string(&owned.log).unwrap()
                    );
                }
                owned.marker.exists()
            }),
            "helper never reached {selected}"
        );
        assert_eq!(fs::read_to_string(&owned.marker).unwrap(), selected);
        owned
    }

    fn resume(&mut self) {
        self.child
            .stdin
            .take()
            .expect("owned helper input")
            .write_all(b"continue\n")
            .expect("resume helper");
        assert!(
            wait_until(Duration::from_secs(15), || self
                .child
                .try_wait()
                .unwrap()
                .is_some()),
            "helper never completed"
        );
        assert!(
            self.child.wait().unwrap().success(),
            "actual helper: {}",
            fs::read_to_string(&self.log).unwrap()
        );
    }

    fn kill_collect(&mut self) {
        self.child.kill().expect("kill paused owned helper");
        assert!(wait_until(Duration::from_secs(15), || self
            .child
            .try_wait()
            .unwrap()
            .is_some()));
        self.child.wait().expect("collect owned helper");
    }
}

impl Drop for HelperChild {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[test]
fn actual_boundaries_hold_one_inode_against_rust_and_previous_go() {
    for boundary in ["staged", "swapped", "installed"] {
        let fixture = RestoreFixture::new(boundary, false);
        let backup_before = tree_hash(&fixture.backup, &[LOCK_BASENAME]);
        let original_hash = tree_hash(&fixture.world, &[LOCK_BASENAME]);
        let mut helper = HelperChild::paused(&fixture, boundary);
        assert!(
            fixture.world.is_dir(),
            "canonical world remains present at {boundary}"
        );
        for root in [
            &fixture.world,
            &fixture.sibling("restore"),
            &fixture.sibling("retired"),
        ] {
            if root.exists() {
                assert_eq!(inode(&root.join(LOCK_BASENAME)), fixture.lock);
                assert!(
                    WorldLease::acquire(root).is_err(),
                    "Rust refuses {boundary} at {}",
                    root.display()
                );
                go_refuses(&fixture.scope, root);
            }
        }
        assert_eq!(
            read_manifest(&fixture.manifest)["restore_original_tree_sha256"],
            original_hash
        );
        helper.resume();
        fixture.assert_installed();
        assert_eq!(tree_hash(&fixture.backup, &[LOCK_BASENAME]), backup_before);
    }
}

#[test]
fn kill_after_exact_exchange_or_retirement_resumes_with_directory_roles_even_for_equal_hashes() {
    for boundary in ["swapped", "retired"] {
        for equal in [false, true] {
            let fixture = RestoreFixture::new(boundary, equal);
            let backup_before = tree_hash(&fixture.backup, &[LOCK_BASENAME]);
            let mut helper = HelperChild::paused(&fixture, boundary);
            let record = read_manifest(&fixture.manifest);
            assert_eq!(
                record["restore_stage"],
                if boundary == "swapped" {
                    "staged"
                } else {
                    "swapped"
                }
            );
            assert_eq!(
                inode(&fixture.sibling(if boundary == "swapped" {
                    "restore"
                } else {
                    "retired"
                })),
                fixture.directory
            );
            assert_ne!(inode(&fixture.world), fixture.directory);
            assert_eq!(inode(&fixture.world.join(LOCK_BASENAME)), fixture.lock);
            helper.kill_collect();
            drop(WorldLease::acquire(&fixture.world).expect("kill releases native helper lock"));
            let (code, output) = restore(
                &fixture.scope,
                &fixture.manifest,
                &fixture.world,
                &fixture.backup,
            );
            assert_eq!(code, 0, "actual shell crash continuation: {output}");
            fixture.assert_installed();
            assert_eq!(tree_hash(&fixture.backup, &[LOCK_BASENAME]), backup_before);
            fixture.actual_restart();
        }
    }
}

#[test]
fn installed_restore_is_idempotent_and_rejects_a_live_owner_through_each_alias() {
    let fixture = RestoreFixture::new("idempotent", false);
    let (code, output) = restore(
        &fixture.scope,
        &fixture.manifest,
        &fixture.world,
        &fixture.backup,
    );
    assert_eq!(code, 0, "{output}");
    fixture.assert_installed();
    let before = fs::read(&fixture.manifest).unwrap();
    let current = inode(&fixture.world);
    for _ in 0..2 {
        let (code, output) = restore(
            &fixture.scope,
            &fixture.manifest,
            &fixture.world,
            &fixture.backup,
        );
        assert_eq!(code, 0, "{output}");
        assert_eq!(fs::read(&fixture.manifest).unwrap(), before);
        assert_eq!(inode(&fixture.world), current);
    }
    for root in [&fixture.world, &fixture.sibling("retired")] {
        let guard = WorldLease::acquire(root).expect("hold an actual installed alias");
        let (code, output) = restore(
            &fixture.scope,
            &fixture.manifest,
            &fixture.world,
            &fixture.backup,
        );
        assert_ne!(code, 0);
        assert!(output.contains("FAIL writer_live"), "{output}");
        assert_eq!(fs::read(&fixture.manifest).unwrap(), before);
        drop(guard);
    }
}

fn constructor(fixture: &RestoreFixture, setup: &str) -> (i32, String) {
    let log = fixture.scope.path("defensive-helper.log");
    let handle = File::create(&log).unwrap();
    let source = format!(
        r#"
import importlib.util, pathlib, sys
spec = importlib.util.spec_from_file_location('restore', sys.argv[1])
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)
{setup}
try:
    module._restore(pathlib.Path(sys.argv[2]), pathlib.Path(sys.argv[3]),
                    pathlib.Path(sys.argv[4]), sys.argv[5])
except module.Refusal as error:
    print('FAIL', error.code, error, file=sys.stderr)
    sys.exit(1)
"#
    );
    let child = Command::new("python3")
        .env("PYTHONDONTWRITEBYTECODE", "1")
        .args(["-c", &source])
        .arg(repo_root().join("scripts/rust-server-restore.py"))
        .arg(&fixture.manifest)
        .arg(&fixture.world)
        .arg(&fixture.backup)
        .arg(manifest_str(
            &read_manifest(&fixture.manifest),
            "start_nonce",
        ))
        .stdout(handle.try_clone().unwrap())
        .stderr(handle)
        .spawn()
        .expect("spawn actual defensive constructor");
    let mut owned = HelperChild {
        child,
        log,
        marker: fixture.scope.path("unused"),
    };
    assert!(wait_until(Duration::from_secs(15), || owned
        .child
        .try_wait()
        .unwrap()
        .is_some()));
    let code = owned.child.wait().unwrap().code().unwrap_or(-1);
    (code, fs::read_to_string(&owned.log).unwrap())
}

#[test]
fn native_unavailable_copy_failure_and_barrier_failure_keep_real_artifacts() {
    for (fault, artifacts) in [
        (
            "module.ctypes.CDLL = lambda *args, **kwargs: object()",
            false,
        ),
        (
            "def failed_copy(backup, staged):\n    staged.mkdir()\n    raise OSError('typed injected copy failure')\nmodule._copy = failed_copy",
            true,
        ),
        (
            "real_sync = module._sync\ndef failed_sync(path):\n    if '.restore.' in str(path):\n        raise OSError('typed injected barrier failure')\n    real_sync(path)\nmodule._sync = failed_sync",
            true,
        ),
    ] {
        let fixture = RestoreFixture::new("defensive", false);
        let before = fs::read(&fixture.manifest).unwrap();
        let world = tree_hash(&fixture.world, &[LOCK_BASENAME]);
        let backup = tree_hash(&fixture.backup, &[LOCK_BASENAME]);
        let (code, output) = constructor(&fixture, fault);
        assert_ne!(code, 0);
        assert!(output.contains("FAIL restore_failed"), "{output}");
        assert_eq!(fs::read(&fixture.manifest).unwrap(), before);
        assert_eq!(inode(&fixture.world), fixture.directory);
        assert_eq!(inode(&fixture.world.join(LOCK_BASENAME)), fixture.lock);
        assert_eq!(tree_hash(&fixture.world, &[LOCK_BASENAME]), world);
        assert_eq!(tree_hash(&fixture.backup, &[LOCK_BASENAME]), backup);
        assert_eq!(fixture.sibling("restore").exists(), artifacts);
        assert!(!fixture.sibling("retired").exists());
        drop(WorldLease::acquire(&fixture.world).expect("typed refusal releases actual lease"));
    }
}

#[test]
fn strict_manifest_backup_and_lock_identity_refuse_before_mutation() {
    for kind in [
        "duplicate",
        "absent-stage",
        "wrong-lease",
        "wrong-directory",
        "backup-duplicate",
        "special-backup",
    ] {
        let fixture = RestoreFixture::new("invalid", false);
        let mut record = read_manifest(&fixture.manifest);
        let expected = match kind {
            "duplicate" => {
                let source = fs::read_to_string(&fixture.manifest).unwrap();
                fs::write(
                    &fixture.manifest,
                    source.replacen('{', "{\"restore_stage\": null,", 1),
                )
                .unwrap();
                "invalid_manifest"
            }
            "absent-stage" => {
                record.as_object_mut().unwrap().remove("restore_stage");
                fs::write(&fixture.manifest, serde_json::to_vec(&record).unwrap()).unwrap();
                "invalid_manifest"
            }
            "wrong-lease" => {
                set_manifest_field(
                    &fixture.manifest,
                    "lease_identity",
                    serde_json::json!("world.lock:0:1"),
                );
                "identity_mismatch"
            }
            "wrong-directory" => {
                copy_dir(&fixture.backup, &fixture.sibling("restore"));
                set_manifest_field(
                    &fixture.manifest,
                    "restore_stage",
                    serde_json::json!("staged"),
                );
                set_manifest_field(
                    &fixture.manifest,
                    "restore_directory_identity",
                    serde_json::json!("0:1"),
                );
                set_manifest_field(
                    &fixture.manifest,
                    "restore_original_tree_sha256",
                    serde_json::json!(tree_hash(&fixture.world, &[LOCK_BASENAME])),
                );
                "restore_failed"
            }
            "backup-duplicate" => {
                let identity = fixture.backup.join(BACKUP_IDENTITY_BASENAME);
                let source = fs::read_to_string(&identity).unwrap();
                fs::write(
                    identity,
                    source.replacen('{', "{\"migration_version\": 1,", 1),
                )
                .unwrap();
                "backup_mismatch"
            }
            "special-backup" => {
                std::os::unix::fs::symlink(&fixture.world, fixture.backup.join("unsafe-link"))
                    .unwrap();
                "backup_mismatch"
            }
            _ => unreachable!(),
        };
        let before = fs::read(&fixture.manifest).unwrap();
        let current = tree_hash(&fixture.world, &[LOCK_BASENAME]);
        let (code, output) = restore(
            &fixture.scope,
            &fixture.manifest,
            &fixture.world,
            &fixture.backup,
        );
        assert_ne!(code, 0);
        assert!(
            output.contains(&format!("FAIL {expected}")),
            "{kind}: {output}"
        );
        assert_eq!(fs::read(&fixture.manifest).unwrap(), before);
        assert_eq!(tree_hash(&fixture.world, &[LOCK_BASENAME]), current);
        assert_eq!(inode(&fixture.world), fixture.directory);
        assert_eq!(inode(&fixture.world.join(LOCK_BASENAME)), fixture.lock);
        assert!(!fixture.sibling("retired").exists());
    }
}

#[test]
fn stale_and_legacy_copied_stage_locks_are_guarded_before_deletion_or_rebinding() {
    for legacy in [false, true] {
        let fixture = RestoreFixture::new("stage-owner", false);
        let staged = fixture.sibling("restore");
        copy_dir(&fixture.backup, &staged);
        if legacy {
            set_manifest_field(
                &fixture.manifest,
                "restore_stage",
                serde_json::json!("staged"),
            );
        }
        let stage_lock = inode(&staged.join(LOCK_BASENAME));
        assert_ne!(stage_lock, fixture.lock);
        let stage_before = tree_hash(&staged, &[LOCK_BASENAME]);
        let before = fs::read(&fixture.manifest).unwrap();
        let guard =
            WorldLease::acquire(&staged).expect("actual writer owns stale copied stage lock");
        let (code, output) = restore(
            &fixture.scope,
            &fixture.manifest,
            &fixture.world,
            &fixture.backup,
        );
        assert_ne!(code, 0);
        assert!(output.contains("FAIL writer_live"), "{output}");
        assert_eq!(fs::read(&fixture.manifest).unwrap(), before);
        assert_eq!(inode(&staged.join(LOCK_BASENAME)), stage_lock);
        assert_eq!(tree_hash(&staged, &[LOCK_BASENAME]), stage_before);
        assert_eq!(inode(&fixture.world), fixture.directory);
        assert!(!fixture.sibling("retired").exists());
        drop(guard);
        let (code, output) = restore(
            &fixture.scope,
            &fixture.manifest,
            &fixture.world,
            &fixture.backup,
        );
        assert_eq!(code, 0, "quiescent stage lineage can be replaced: {output}");
        fixture.assert_installed();
    }
}

#[test]
fn legacy_missing_stage_recovers_and_foreign_current_collision_refuses() {
    for stage in ["staged", "old_retired"] {
        let fixture = RestoreFixture::new("legacy-missing", false);
        fs::rename(&fixture.world, fixture.sibling("retired")).unwrap();
        set_manifest_field(&fixture.manifest, "restore_stage", serde_json::json!(stage));
        let (code, output) = restore(
            &fixture.scope,
            &fixture.manifest,
            &fixture.world,
            &fixture.backup,
        );
        assert_eq!(code, 0, "legacy absent staged copy: {output}");
        fixture.assert_installed();
    }
    let fixture = RestoreFixture::new("foreign-current", false);
    let retired = fixture.sibling("retired");
    fs::rename(&fixture.world, &retired).unwrap();
    copy_dir(&fixture.backup, &fixture.world);
    set_manifest_field(
        &fixture.manifest,
        "restore_stage",
        serde_json::json!("old_retired"),
    );
    let before = fs::read(&fixture.manifest).unwrap();
    let foreign = inode(&fixture.world);
    let (code, output) = restore(
        &fixture.scope,
        &fixture.manifest,
        &fixture.world,
        &fixture.backup,
    );
    assert_ne!(code, 0);
    assert!(output.contains("FAIL identity_mismatch"), "{output}");
    assert_eq!(fs::read(&fixture.manifest).unwrap(), before);
    assert_eq!(inode(&fixture.world), foreign);
    assert_eq!(inode(&retired), fixture.directory);
    assert_eq!(inode(&retired.join(LOCK_BASENAME)), fixture.lock);
}

#[test]
fn historical_installed_checkpoint_qualifies_same_lineage_and_refuses_split_lineage() {
    for same in [true, false] {
        let fixture = RestoreFixture::new("legacy-installed", false);
        let retired = fixture.sibling("retired");
        fs::rename(&fixture.world, &retired).unwrap();
        copy_dir(&fixture.backup, &fixture.world);
        fs::remove_file(fixture.world.join(BACKUP_IDENTITY_BASENAME)).unwrap();
        if same {
            fs::remove_file(fixture.world.join(LOCK_BASENAME)).unwrap();
            fs::hard_link(
                retired.join(LOCK_BASENAME),
                fixture.world.join(LOCK_BASENAME),
            )
            .unwrap();
        }
        let current_lock = inode(&fixture.world.join(LOCK_BASENAME));
        set_manifest_field(
            &fixture.manifest,
            "lease_identity",
            serde_json::json!(format!("world.lock:{}:{}", current_lock.0, current_lock.1)),
        );
        set_manifest_field(
            &fixture.manifest,
            "restore_stage",
            serde_json::json!("backup_installed"),
        );
        let before = fs::read(&fixture.manifest).unwrap();
        let original = tree_hash(&retired, &[LOCK_BASENAME]);
        let (code, output) = constructor(&fixture, "");
        if same {
            assert_eq!(code, 0, "same-lineage legacy qualification: {output}");
            fixture.assert_installed();
            assert_eq!(
                read_manifest(&fixture.manifest)["restore_original_tree_sha256"],
                original
            );
        } else {
            assert_ne!(code, 0);
            assert!(
                output.contains("FAIL identity_mismatch"),
                "split lineage: {output}"
            );
            assert_eq!(fs::read(&fixture.manifest).unwrap(), before);
            assert_eq!(inode(&retired.join(LOCK_BASENAME)), fixture.lock);
            assert_eq!(inode(&fixture.world.join(LOCK_BASENAME)), current_lock);
            assert_eq!(tree_hash(&retired, &[LOCK_BASENAME]), original);
        }
    }
    let fixture = RestoreFixture::new("retired-absent", true);
    assert_eq!(
        restore(
            &fixture.scope,
            &fixture.manifest,
            &fixture.world,
            &fixture.backup
        )
        .0,
        0
    );
    fs::remove_dir_all(fixture.sibling("retired")).unwrap();
    let before = fs::read(&fixture.manifest).unwrap();
    assert_eq!(constructor(&fixture, "").0, 0);
    assert_eq!(fs::read(&fixture.manifest).unwrap(), before);
}

#[test]
fn streamed_copy_preserves_regular_file_and_directory_metadata() {
    use std::os::unix::fs::PermissionsExt;
    use std::time::SystemTime;

    let fixture = RestoreFixture::new("metadata-copy", true);
    let directory = fixture.backup.join("offline-copy");
    fs::create_dir(&directory).unwrap();
    let file = directory.join("large.dat");
    fs::write(&file, vec![0x5a; 2 * 1024 * 1024 + 37]).unwrap();
    fs::set_permissions(&file, fs::Permissions::from_mode(0o640)).unwrap();
    fs::set_permissions(&directory, fs::Permissions::from_mode(0o750)).unwrap();
    let time = SystemTime::UNIX_EPOCH + Duration::from_secs(1_234_567_890);
    File::options()
        .write(true)
        .open(&file)
        .unwrap()
        .set_modified(time)
        .unwrap();
    File::open(&directory).unwrap().set_modified(time).unwrap();
    let hash = tree_hash(&fixture.backup, &[LOCK_BASENAME, BACKUP_IDENTITY_BASENAME]);
    set_manifest_field(
        &fixture.manifest,
        "backup_tree_sha256",
        serde_json::json!(hash),
    );
    let (code, output) = restore(
        &fixture.scope,
        &fixture.manifest,
        &fixture.world,
        &fixture.backup,
    );
    assert_eq!(code, 0, "streamed native installation: {output}");
    for relative in ["", "offline-copy", "offline-copy/large.dat"] {
        let source = fs::metadata(fixture.backup.join(relative)).unwrap();
        let installed = fs::metadata(fixture.world.join(relative)).unwrap();
        assert_eq!(installed.permissions().mode(), source.permissions().mode());
        assert_eq!(
            installed.modified().unwrap(),
            source.modified().unwrap(),
            "preserve copy mtime at {relative}"
        );
    }
    fixture.assert_installed();
}

#[test]
fn secondary_copied_stage_descriptor_remains_held_until_installed_boundary_returns() {
    for legacy in [false, true] {
        let fixture = RestoreFixture::new("secondary-span", false);
        let stage = fixture.sibling("restore");
        copy_dir(&fixture.backup, &stage);
        if legacy {
            set_manifest_field(
                &fixture.manifest,
                "restore_stage",
                serde_json::json!("staged"),
            );
        }
        let alias = fixture.scope.path("copied-lock-alias");
        fs::create_dir(&alias).unwrap();
        fs::hard_link(stage.join(LOCK_BASENAME), alias.join(LOCK_BASENAME)).unwrap();
        let copied_inode = inode(&alias.join(LOCK_BASENAME));
        let mut helper = HelperChild::paused(&fixture, "installed");
        assert_ne!(copied_inode, fixture.lock);
        assert!(
            WorldLease::acquire(&alias).is_err(),
            "deleted copied inode remains guarded through publication"
        );
        helper.resume();
        drop(WorldLease::acquire(&alias).expect("secondary guard releases only at attempt end"));
        fixture.assert_installed();
    }
}

#[test]
fn uncertain_retirement_barrier_is_retried_before_installed_publication() {
    let mut fixture = RestoreFixture::new("uncertain-retire", false);
    let nested = fixture.world.parent().unwrap().join("nested");
    fs::create_dir(&nested).unwrap();
    let world = nested.join("world");
    fs::rename(&fixture.world, &world).unwrap();
    fixture.world = world;
    set_manifest_field(
        &fixture.manifest,
        "world_path",
        serde_json::json!(fixture.world),
    );
    let (code, output) = constructor(
        &fixture,
        r#"
real_rename = module._NativeRename.rename
def failed_after_retire(self, old, new, *, exchange):
    real_rename(self, old, new, exchange=exchange)
    if not exchange:
        raise OSError('typed injected uncertainty after actual retirement')
module._NativeRename.rename = failed_after_retire
"#,
    );
    assert_ne!(code, 0);
    assert!(output.contains("FAIL restore_failed"), "{output}");
    assert_eq!(read_manifest(&fixture.manifest)["restore_stage"], "swapped");
    assert_eq!(inode(&fixture.sibling("retired")), fixture.directory);
    assert!(!fixture.sibling("restore").exists());
    let before = fs::read(&fixture.manifest).unwrap();
    let (code, output) = constructor(
        &fixture,
        r#"
real_sync = module._sync
def refused_parent(path):
    if path == pathlib.Path(sys.argv[3]).parent:
        raise OSError('typed injected pending directory barrier failure')
    real_sync(path)
module._sync = refused_parent
"#,
    );
    assert_ne!(
        code, 0,
        "uncertain parent barrier must be retried before successful publication: {output}"
    );
    assert!(output.contains("FAIL restore_failed"), "{output}");
    assert_eq!(fs::read(&fixture.manifest).unwrap(), before);
    assert_eq!(inode(&fixture.sibling("retired")), fixture.directory);
    assert_eq!(
        restore(
            &fixture.scope,
            &fixture.manifest,
            &fixture.world,
            &fixture.backup
        )
        .0,
        0
    );
    fixture.assert_installed();
}
