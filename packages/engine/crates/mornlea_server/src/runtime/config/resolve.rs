//! Read-only default config path resolution mirroring Go `config.LoadDefault`.
//!
//! Go owns the user's config file: it creates it, migrates the legacy
//! pre-rename config file, and writes settings. The Rust server only reads
//! the Mornlea path, with the same safety gates Go applies before trusting it:
//! a regular non-symlink file with mode 0600 inside a 0700 directory, and the
//! same inode before and after open. When only the legacy file exists, Rust
//! neither migrates nor reads it; it refuses and asks for one Go start.

/// Directory of the pre-rename config file that Go migrates on first start.
/// Rust only checks for its presence; the identity audit allowlists it.
const LEGACY_CONFIG_DIR: &str = "minecraft-go";

use std::ffi::OsString;
use std::fs;
use std::io::{self, Read};
use std::path::{Path, PathBuf};

use super::{ConfigError, RuntimeConfig};

/// The Mornlea config path and the legacy path Go migrates from.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ConfigPaths {
    pub current: PathBuf,
    pub legacy: PathBuf,
}

impl ConfigPaths {
    /// Go `defaultPaths` under an already-resolved user config directory.
    pub fn under(config_dir: &Path) -> Self {
        Self {
            current: config_dir.join("mornlea").join("config.json"),
            legacy: config_dir.join(LEGACY_CONFIG_DIR).join("config.json"),
        }
    }

    /// Go `defaultPaths` from the process environment.
    pub fn from_env() -> Result<Self, ConfigError> {
        user_config_dir(|name| std::env::var_os(name)).map(|dir| Self::under(&dir))
    }
}

/// Go `os.UserConfigDir` for the platforms the server ships on.
pub fn user_config_dir(getenv: impl Fn(&str) -> Option<OsString>) -> Result<PathBuf, ConfigError> {
    let get = |name: &str| getenv(name).filter(|value| !value.is_empty());
    if cfg!(windows) {
        return get("AppData")
            .map(PathBuf::from)
            .ok_or_else(|| ConfigError::NoConfigDir("%AppData% is not defined".into()));
    }
    if cfg!(target_vendor = "apple") {
        return get("HOME")
            .map(|home| PathBuf::from(home).join("Library/Application Support"))
            .ok_or_else(|| ConfigError::NoConfigDir("$HOME is not defined".into()));
    }
    match get("XDG_CONFIG_HOME") {
        Some(dir) => {
            let dir = PathBuf::from(dir);
            if dir.is_absolute() {
                Ok(dir)
            } else {
                Err(ConfigError::NoConfigDir(
                    "path in $XDG_CONFIG_HOME is relative".into(),
                ))
            }
        }
        None => get("HOME")
            .map(|home| PathBuf::from(home).join(".config"))
            .ok_or_else(|| {
                ConfigError::NoConfigDir("neither $XDG_CONFIG_HOME nor $HOME are defined".into())
            }),
    }
}

impl RuntimeConfig {
    /// Resolve and load the default config file read-only; see the module doc.
    pub fn resolve() -> Result<Self, ConfigError> {
        let config = Self::resolve_paths(&ConfigPaths::from_env()?)?;
        config.emit_warnings();
        Ok(config)
    }

    /// [`Self::resolve`] for explicit paths. Missing Mornlea and legacy files
    /// yield defaults; nothing is created, migrated, or written.
    pub fn resolve_paths(paths: &ConfigPaths) -> Result<Self, ConfigError> {
        validate_parent(&paths.current)?;
        if let Some(bytes) = read_checked(&paths.current)? {
            return Self::decode(&bytes);
        }
        match fs::metadata(&paths.legacy) {
            Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(Self::defaults()),
            Err(err) => Err(ConfigError::Path {
                path: paths.legacy.clone(),
                detail: format!("inspect legacy config: {err}"),
            }),
            Ok(_) => Err(ConfigError::LegacyConfigNeedsMigration {
                legacy: paths.legacy.clone(),
                current: paths.current.clone(),
            }),
        }
    }
}

fn insecure(path: &Path, detail: String) -> ConfigError {
    ConfigError::InsecurePath {
        path: path.to_path_buf(),
        detail,
    }
}

fn path_error(path: &Path, action: &str, err: io::Error) -> ConfigError {
    ConfigError::Path {
        path: path.to_path_buf(),
        detail: format!("{action}: {err}"),
    }
}

/// Go `validateDefaultConfigParent`: a missing directory is fine; an existing
/// one must be a 0700 directory.
fn validate_parent(current: &Path) -> Result<(), ConfigError> {
    let parent = current.parent().unwrap_or(Path::new("."));
    let info = match fs::metadata(parent) {
        Ok(info) => info,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(err) => return Err(path_error(parent, "inspect config directory", err)),
    };
    let perm = permission_bits(&info);
    if !info.is_dir() || perm != Some(0o700) {
        return Err(insecure(
            parent,
            format!("config directory must be a 0700 directory (mode {perm:?})"),
        ));
    }
    Ok(())
}

/// Go `validateDefaultConfigFile`. Symlink, non-regular and mode are checked
/// one at a time so each refusal has its own pin; Go reports them together.
fn validate_file(path: &Path, info: &fs::Metadata) -> Result<(), ConfigError> {
    if info.file_type().is_symlink() {
        return Err(insecure(path, "config file must not be a symlink".into()));
    }
    if !info.is_file() {
        return Err(insecure(path, "config file must be a regular file".into()));
    }
    let perm = permission_bits(info);
    if perm != Some(0o600) {
        return Err(insecure(
            path,
            format!("config file must be mode 0600 (mode {perm:?})"),
        ));
    }
    Ok(())
}

/// Opens a path the way Go's injected `open` does, so tests can swap the
/// inode after the first lstat and before the open.
type OpenFn = Box<dyn FnOnce(&Path) -> io::Result<fs::File>>;

fn default_open(path: &Path) -> io::Result<fs::File> {
    fs::File::open(path)
}

/// Go `readDefaultConfigIfExistsWithOpen`: lstat, open, fstat, then lstat
/// again, refusing any swap between the checks.
fn read_checked(path: &Path) -> Result<Option<Vec<u8>>, ConfigError> {
    read_checked_with_open(path, Box::new(default_open))
}

fn read_checked_with_open(path: &Path, open: OpenFn) -> Result<Option<Vec<u8>>, ConfigError> {
    let checked = match fs::symlink_metadata(path) {
        Ok(info) => info,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(path_error(path, "inspect config file", err)),
    };
    validate_file(path, &checked)?;
    let mut file = open(path).map_err(|err| path_error(path, "open config file", err))?;
    let opened = file
        .metadata()
        .map_err(|err| path_error(path, "inspect opened config file", err))?;
    validate_file(path, &opened)?;
    if !same_file(&checked, &opened) {
        return Err(ConfigError::Replaced {
            path: path.to_path_buf(),
        });
    }
    let current = fs::symlink_metadata(path)
        .map_err(|err| path_error(path, "re-inspect config file", err))?;
    validate_file(path, &current)?;
    if !same_file(&current, &opened) {
        return Err(ConfigError::Replaced {
            path: path.to_path_buf(),
        });
    }
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)
        .map_err(|err| path_error(path, "read config file", err))?;
    Ok(Some(bytes))
}

/// Unix permission bits. Go reports synthetic `0666`/`0444`/`0777` modes on
/// Windows, which never equal the required modes, so `None` refuses there too.
#[cfg(unix)]
fn permission_bits(info: &fs::Metadata) -> Option<u32> {
    use std::os::unix::fs::PermissionsExt;
    Some(info.permissions().mode() & 0o777)
}

#[cfg(not(unix))]
fn permission_bits(_info: &fs::Metadata) -> Option<u32> {
    None
}

#[cfg(unix)]
fn same_file(a: &fs::Metadata, b: &fs::Metadata) -> bool {
    use std::os::unix::fs::MetadataExt;
    a.dev() == b.dev() && a.ino() == b.ino()
}

#[cfg(not(unix))]
fn same_file(_a: &fs::Metadata, _b: &fs::Metadata) -> bool {
    false
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::{PermissionsExt, symlink};

    struct TempDir(PathBuf);

    impl TempDir {
        fn new(tag: &str) -> Self {
            let path = std::env::temp_dir().join(format!(
                "mornlea-config-resolve-{tag}-{}",
                std::process::id()
            ));
            let _ = fs::remove_dir_all(&path);
            fs::create_dir_all(&path).unwrap();
            Self(path)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn chmod(path: &Path, mode: u32) {
        fs::set_permissions(path, fs::Permissions::from_mode(mode)).unwrap();
    }

    fn write_current(paths: &ConfigPaths, body: &str, dir_mode: u32, file_mode: u32) {
        let parent = paths.current.parent().unwrap();
        fs::create_dir_all(parent).unwrap();
        fs::write(&paths.current, body).unwrap();
        chmod(&paths.current, file_mode);
        chmod(parent, dir_mode);
    }

    #[test]
    fn user_config_dir_matches_go_unix_rules() {
        if cfg!(target_vendor = "apple") {
            return;
        }
        let env = |pairs: &'static [(&'static str, &'static str)]| {
            move |name: &str| {
                pairs
                    .iter()
                    .find(|(key, _)| *key == name)
                    .map(|(_, value)| OsString::from(value))
            }
        };
        assert_eq!(
            user_config_dir(env(&[("XDG_CONFIG_HOME", "/x"), ("HOME", "/h")])).unwrap(),
            PathBuf::from("/x")
        );
        assert_eq!(
            user_config_dir(env(&[("XDG_CONFIG_HOME", ""), ("HOME", "/h")])).unwrap(),
            PathBuf::from("/h/.config")
        );
        assert!(user_config_dir(env(&[("XDG_CONFIG_HOME", "rel")])).is_err());
        assert!(user_config_dir(env(&[])).is_err());
        assert_eq!(
            ConfigPaths::under(Path::new("/c")).legacy,
            PathBuf::from("/c")
                .join(LEGACY_CONFIG_DIR)
                .join("config.json")
        );
    }

    #[test]
    fn missing_files_yield_defaults_without_creating_anything() {
        let dir = TempDir::new("missing");
        let paths = ConfigPaths::under(&dir.0);
        let config = RuntimeConfig::resolve_paths(&paths).expect("defaults");
        assert_eq!(config.fluid_updates_per_tick(), 512);
        assert!(!paths.current.parent().unwrap().exists());
    }

    #[test]
    fn secure_file_is_read_and_left_untouched() {
        let dir = TempDir::new("secure");
        let paths = ConfigPaths::under(&dir.0);
        let body = r#"{"fluidEnabled":false}"#;
        write_current(&paths, body, 0o700, 0o600);
        let config = RuntimeConfig::resolve_paths(&paths).expect("secure file");
        assert!(!config.fluid_enabled());
        assert_eq!(fs::read_to_string(&paths.current).unwrap(), body);
    }

    #[test]
    fn insecure_modes_and_symlinks_are_refused() {
        let dir = TempDir::new("insecure");
        let paths = ConfigPaths::under(&dir.0);
        write_current(&paths, "{}", 0o700, 0o644);
        assert!(matches!(
            RuntimeConfig::resolve_paths(&paths),
            Err(ConfigError::InsecurePath { .. })
        ));
        write_current(&paths, "{}", 0o755, 0o600);
        assert!(matches!(
            RuntimeConfig::resolve_paths(&paths),
            Err(ConfigError::InsecurePath { .. })
        ));
        chmod(paths.current.parent().unwrap(), 0o700);
        let target = dir.0.join("target.json");
        fs::write(&target, "{}").unwrap();
        chmod(&target, 0o600);
        fs::remove_file(&paths.current).unwrap();
        symlink(&target, &paths.current).unwrap();
        let err = RuntimeConfig::resolve_paths(&paths).expect_err("symlink");
        assert!(
            matches!(&err, ConfigError::InsecurePath { detail, .. } if detail.contains("symlink")),
            "{err}"
        );
    }

    #[test]
    fn sixty_hundred_directory_is_refused_as_not_a_regular_file() {
        let dir = TempDir::new("dir-as-file");
        let paths = ConfigPaths::under(&dir.0);
        let parent = paths.current.parent().unwrap();
        fs::create_dir_all(parent).unwrap();
        chmod(parent, 0o700);
        fs::create_dir(&paths.current).unwrap();
        chmod(&paths.current, 0o600);
        let err = RuntimeConfig::resolve_paths(&paths).expect_err("directory");
        assert!(
            matches!(&err, ConfigError::InsecurePath { detail, .. } if detail.contains("regular file")),
            "{err}"
        );
    }

    #[test]
    fn sixty_hundred_fifo_is_refused_as_not_a_regular_file() {
        let dir = TempDir::new("fifo");
        let paths = ConfigPaths::under(&dir.0);
        let parent = paths.current.parent().unwrap();
        fs::create_dir_all(parent).unwrap();
        chmod(parent, 0o700);
        // Permission bits are 0600 so the mode check cannot fire first and
        // hide the non-regular refusal. The crate forbids unsafe_code, so
        // the named pipe is created with the system mkfifo.
        let status = std::process::Command::new("mkfifo")
            .arg("-m")
            .arg("600")
            .arg(&paths.current)
            .status()
            .expect("spawn mkfifo");
        assert!(status.success(), "mkfifo {status}");
        chmod(&paths.current, 0o600);
        // A blocking open of a FIFO waits for a writer, so the injected open
        // uses O_NONBLOCK: if the regular-file check ever goes away the read
        // returns an empty body at once and this test fails instead of hanging.
        let err = read_checked_with_open(
            &paths.current,
            Box::new(|path| {
                use std::os::unix::fs::OpenOptionsExt;
                fs::OpenOptions::new()
                    .read(true)
                    .custom_flags(libc::O_NONBLOCK)
                    .open(path)
            }),
        )
        .expect_err("fifo");
        assert!(
            matches!(&err, ConfigError::InsecurePath { detail, .. } if detail.contains("regular file")),
            "{err}"
        );
    }

    #[test]
    fn replaced_inode_before_open_is_refused() {
        let dir = TempDir::new("swap");
        let paths = ConfigPaths::under(&dir.0);
        write_current(&paths, r#"{"fluidEnabled":false}"#, 0o700, 0o600);
        let replacement = dir.0.join("replacement.json");
        fs::write(&replacement, r#"{"fluidEnabled":true}"#).unwrap();
        chmod(&replacement, 0o600);
        let current = paths.current.clone();
        let err = read_checked_with_open(
            &current,
            Box::new(move |path| {
                fs::rename(&replacement, path)?;
                fs::File::open(path)
            }),
        )
        .expect_err("swap");
        assert!(matches!(err, ConfigError::Replaced { .. }), "{err}");
        // The replacement is still on disk at the current path; nothing was decoded.
        assert_eq!(
            fs::read_to_string(&paths.current).unwrap(),
            r#"{"fluidEnabled":true}"#
        );
    }

    #[test]
    fn replaced_inode_after_open_is_refused() {
        let dir = TempDir::new("swap-after-open");
        let paths = ConfigPaths::under(&dir.0);
        write_current(&paths, r#"{"fluidEnabled":false}"#, 0o700, 0o600);
        let replacement = dir.0.join("replacement.json");
        fs::write(&replacement, r#"{"fluidEnabled":true}"#).unwrap();
        chmod(&replacement, 0o600);
        let current = paths.current.clone();
        // The handle is the original inode, so the pre-open check passes and
        // only the re-lstat after open can notice the swap.
        let err = read_checked_with_open(
            &current,
            Box::new(move |path| {
                let original = fs::File::open(path)?;
                fs::rename(&replacement, path)?;
                Ok(original)
            }),
        )
        .expect_err("swap after open");
        assert!(matches!(err, ConfigError::Replaced { .. }), "{err}");
    }

    #[test]
    fn same_inode_symlink_inserted_before_open_is_refused() {
        let dir = TempDir::new("swap-symlink");
        let paths = ConfigPaths::under(&dir.0);
        write_current(&paths, r#"{"fluidEnabled":false}"#, 0o700, 0o600);
        let original = dir.0.join("original.json");
        let current = paths.current.clone();
        let err = read_checked_with_open(
            &current,
            Box::new(move |path| {
                fs::rename(path, &original)?;
                symlink(&original, path)?;
                fs::File::open(path)
            }),
        )
        .expect_err("symlink swap");
        assert!(
            matches!(&err, ConfigError::InsecurePath { detail, .. } if detail.contains("symlink")),
            "{err}"
        );
    }

    #[test]
    fn malformed_secure_file_is_an_error() {
        let dir = TempDir::new("malformed");
        let paths = ConfigPaths::under(&dir.0);
        write_current(&paths, "{", 0o700, 0o600);
        assert!(matches!(
            RuntimeConfig::resolve_paths(&paths),
            Err(ConfigError::Parse(_))
        ));
    }

    #[test]
    fn legacy_only_file_is_refused_not_migrated() {
        let dir = TempDir::new("legacy");
        let paths = ConfigPaths::under(&dir.0);
        fs::create_dir_all(paths.legacy.parent().unwrap()).unwrap();
        fs::write(&paths.legacy, r#"{"fluidEnabled":false}"#).unwrap();
        let err = RuntimeConfig::resolve_paths(&paths).expect_err("legacy only");
        match &err {
            ConfigError::LegacyConfigNeedsMigration { legacy, .. } => {
                assert_eq!(legacy, &paths.legacy);
            }
            other => panic!("expected legacy refusal, got {other}"),
        }
        assert!(err.to_string().contains(LEGACY_CONFIG_DIR));
        assert!(!paths.current.exists(), "Rust must not migrate");
        // With the Mornlea file present the legacy file is ignored, as in Go.
        write_current(&paths, "{}", 0o700, 0o600);
        assert!(RuntimeConfig::resolve_paths(&paths).is_ok());
    }
}
