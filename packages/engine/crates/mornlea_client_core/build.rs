//! Attributes diagnostics at build time; live sessions never read files or spawn commands.

use std::{
    env, fs,
    io::Write,
    path::PathBuf,
    process::{Command, Stdio},
};

fn command(program: &str, args: &[&str]) -> String {
    let output = Command::new(program)
        .args(args)
        .output()
        .expect("build identity command");
    assert!(
        output.status.success(),
        "build identity command failed: {program}"
    );
    String::from_utf8(output.stdout)
        .expect("identity output UTF-8")
        .trim()
        .to_owned()
}

fn main() {
    let manifest = PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("manifest directory"));
    let source = command("git", &["rev-parse", "HEAD"]);
    assert!(
        source.len() == 40 && source.bytes().all(|c| c.is_ascii_hexdigit()),
        "source SHA"
    );
    let branch = Command::new("git")
        .args(["symbolic-ref", "-q", "HEAD"])
        .output()
        .expect("Git ref");
    let mut paths = vec!["HEAD".to_owned(), "packed-refs".to_owned()];
    if branch.status.success() {
        paths.push(
            String::from_utf8(branch.stdout)
                .expect("Git ref UTF-8")
                .trim()
                .to_owned(),
        );
    }
    for path in &paths {
        let git_path = command("git", &["rev-parse", "--git-path", path.as_str()]);
        println!(
            "cargo:rerun-if-changed={}",
            manifest.join(git_path).display()
        );
    }
    let mut bytes = Vec::new();
    for name in [
        "src/contracts.rs",
        "src/presentation/frame.rs",
        "src/presentation/mod.rs",
    ] {
        let path = manifest.join(name);
        println!("cargo:rerun-if-changed={}", path.display());
        bytes.extend_from_slice(name.as_bytes());
        bytes.push(0);
        bytes.extend(fs::read(path).expect("contract input"));
    }
    let mut hasher = Command::new("shasum")
        .args(["-a", "256"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .or_else(|_| {
            Command::new("sha256sum")
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .spawn()
        })
        .expect("SHA256 build tool");
    hasher
        .stdin
        .take()
        .expect("digest input")
        .write_all(&bytes)
        .expect("digest write");
    let output = hasher.wait_with_output().expect("contract digest");
    assert!(output.status.success(), "contract digest failed");
    let output = String::from_utf8(output.stdout).expect("digest UTF-8");
    let contract = output.split_whitespace().next().expect("contract digest");
    assert!(
        contract.len() == 64 && contract.bytes().all(|c| c.is_ascii_hexdigit()),
        "contract SHA"
    );
    println!("cargo:rustc-env=MORNLEA_CORE_SOURCE_SHA={source}");
    println!("cargo:rustc-env=MORNLEA_CORE_CONTRACT_SHA={contract}");
}
