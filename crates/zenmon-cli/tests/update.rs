//! End-to-end tests for `zenmon update` against a `path` remote.
//!
//! The registry is isolated with `ZENMON_REMOTES`, and `apply` only ever runs
//! on a *copy* of the binary in a temp directory — never on the test binary
//! itself, which `apply` would replace. The release carries no tray build, so
//! a tray installed on the machine running the tests is never touched.
#![cfg(feature = "self-update")]

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use serde_json::Value;
use sha2::{Digest, Sha256};

struct Sandbox {
    root: PathBuf,
}

impl Sandbox {
    fn new(test: &str) -> Sandbox {
        let root =
            std::env::temp_dir().join(format!("zenmon-update-{}-{test}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("release")).unwrap();
        std::fs::create_dir_all(root.join("bin")).unwrap();
        Sandbox { root }
    }

    fn release(&self) -> PathBuf {
        self.root.join("release")
    }

    /// `exe` with this sandbox's registry, and the remote `local` registered.
    fn zenmon(&self, exe: &Path, args: &[&str]) -> Command {
        let mut cmd = Command::new(exe);
        cmd.env("ZENMON_REMOTES", self.root.join("remotes.toml"))
            // Never reach GitHub from a test, even on a fallback path.
            .env_remove("GITHUB_TOKEN")
            .args(args);
        cmd
    }

    fn add_remote(&self) {
        let release = self.release();
        run(self.zenmon(
            Path::new(env!("CARGO_BIN_EXE_zenmon")),
            &[
                "remote",
                "add",
                "local",
                "--path",
                release.to_str().unwrap(),
                "--default",
            ],
        ));
    }

    /// Publish a release manifest: a build for this platform at `archive`, or
    /// with `None`, only a build for some other platform.
    fn publish(&self, version: &str, archive: Option<(&str, &[u8])>) {
        let target = format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH);
        let artifacts = match archive {
            Some((name, bytes)) => {
                std::fs::write(self.release().join(name), bytes).unwrap();
                let sha = format!("{:x}", Sha256::digest(bytes));
                format!(
                    r#"[{{"target":"{target}","url":"{name}","sha256":"{sha}","binary":"zenmon"}}]"#
                )
            }
            // A manifest must list something: a build for some other platform.
            None => format!(
                r#"[{{"target":"plan9-mips","url":"zenmon-{version}-plan9.tar.gz","sha256":"{}","binary":"zenmon"}}]"#,
                "a".repeat(64)
            ),
        };
        let manifest = format!(
            r#"{{"schemaVersion":1,"version":"{version}","channel":"stable","artifacts":{artifacts}}}"#
        );
        std::fs::write(self.release().join("zenmon.json"), manifest).unwrap();
    }
}

impl Drop for Sandbox {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn run(mut cmd: Command) -> Output {
    let out = cmd.output().expect("run zenmon");
    assert!(
        out.status.success(),
        "zenmon failed ({}):\nstdout: {}\nstderr: {}",
        out.status,
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    out
}

fn json(out: &Output) -> Value {
    serde_json::from_slice(&out.stdout).expect("one JSON object on stdout")
}

#[test]
fn check_reports_a_newer_release_on_a_path_remote() {
    let sb = Sandbox::new("check");
    sb.add_remote();
    sb.publish(
        "99.0.0",
        Some(("zenmon-99.0.0.tar.gz", b"not inspected by check")),
    );

    let out = run(sb.zenmon(
        Path::new(env!("CARGO_BIN_EXE_zenmon")),
        &["--json", "update", "check"],
    ));
    let v = json(&out);

    assert_eq!(v["available"], "99.0.0", "{v}");
    assert_eq!(v["update_available"], true, "{v}");
    assert_eq!(v["target_supported"], true, "{v}");
    assert_eq!(v["remote"]["name"], "local", "{v}");
    assert_eq!(v["remote"]["kind"], "path", "{v}");
}

#[test]
fn check_says_when_the_release_has_no_build_for_this_platform() {
    let sb = Sandbox::new("unsupported");
    sb.add_remote();
    sb.publish("99.0.0", None);

    let v = json(&run(sb.zenmon(
        Path::new(env!("CARGO_BIN_EXE_zenmon")),
        &["--json", "update", "check"],
    )));
    assert_eq!(v["target_supported"], false, "{v}");
}

/// The whole apply path: fetch, checksum, extract, run the staged binary's
/// `--version`, swap. The "new release" is a script reporting 99.0.0, so
/// this runs on Unix only.
#[cfg(unix)]
#[test]
fn apply_swaps_a_copied_binary_for_the_release() {
    use std::os::unix::fs::PermissionsExt;

    let sb = Sandbox::new("apply");
    sb.add_remote();

    // The release: a tar.gz holding `zenmon`, a script that reports 99.0.0.
    let stage = sb.root.join("stage");
    std::fs::create_dir_all(&stage).unwrap();
    let script = stage.join("zenmon");
    std::fs::write(&script, "#!/bin/sh\necho 'zenmon 99.0.0'\n").unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    let archive = sb.root.join("zenmon-99.0.0.tar.gz");
    let tar = Command::new("tar")
        .args([
            "czf",
            archive.to_str().unwrap(),
            "-C",
            stage.to_str().unwrap(),
            "zenmon",
        ])
        .status()
        .expect("tar");
    assert!(tar.success());
    sb.publish(
        "99.0.0",
        Some(("zenmon-99.0.0.tar.gz", &std::fs::read(&archive).unwrap())),
    );

    // Run `apply` from a copy, so the swap replaces the copy.
    let copy = sb.root.join("bin").join("zenmon");
    std::fs::copy(env!("CARGO_BIN_EXE_zenmon"), &copy).unwrap();
    let v = json(&run(sb.zenmon(&copy, &["--json", "update", "apply"])));

    assert_eq!(v["installed"], true, "{v}");
    assert_eq!(v["to"], "99.0.0", "{v}");
    assert!(v["tray"].is_null(), "no tray build in this release: {v}");
    let swapped = Command::new(&copy).arg("--version").output().unwrap();
    assert_eq!(
        String::from_utf8_lossy(&swapped.stdout).trim(),
        "zenmon 99.0.0"
    );
}
