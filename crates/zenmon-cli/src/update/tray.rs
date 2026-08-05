//! Detecting and replacing an installed zenmon-tray.
//!
//! The tray rides along on `zenmon update apply`: when a tray installation is
//! found on this machine and the release carries a tray build for this
//! platform, the tray is updated in the same run. No tray installed means the
//! whole module is a no-op — the CLI never installs a tray you didn't choose
//! to have.
//!
//! Install shapes differ per platform and so does the replacement story:
//! - macOS: the artifact is a tar.gz of `zenmon-tray.app`; the bundle is
//!   swapped wholesale. Piecewise replacement would break the ad-hoc code
//!   signature's seal, which macOS validates per-bundle.
//! - Windows: the artifact is the NSIS installer itself, run with `/S`; the
//!   installer owns the file layout, uninstall registry entries and shortcuts,
//!   so re-running it *is* the update path.

use std::path::{Path, PathBuf};
use std::process::Command;

use semver::Version;
use zenmon_core::error::{Result, ZenmonError};

use super::manifest::ReleaseArtifact;

/// Process image name of the running tray, used to quit before replacing and
/// to decide whether to relaunch after.
#[cfg(target_os = "macos")]
const TRAY_PROCESS: &str = "zenmon-tray";
#[cfg(windows)]
const TRAY_PROCESS: &str = "zenmon-tray.exe";

#[derive(Debug, Clone)]
pub struct TrayInstall {
    /// macOS: the `.app` bundle directory. Windows: the installed executable.
    pub path: PathBuf,
    /// `None` when an installation exists but its version could not be read;
    /// treated as "old" so the update proceeds rather than being skipped on
    /// missing information.
    pub version: Option<Version>,
}

// ---------------------------------------------------------------- macOS ----

#[cfg(target_os = "macos")]
pub fn detect() -> Option<TrayInstall> {
    let mut candidates = vec![PathBuf::from("/Applications/zenmon-tray.app")];
    if let Some(home) = std::env::var_os("HOME") {
        candidates.push(PathBuf::from(home).join("Applications/zenmon-tray.app"));
    }
    let path = candidates.into_iter().find(|p| p.is_dir())?;
    let version = std::fs::read_to_string(path.join("Contents/Info.plist"))
        .ok()
        .and_then(|plist| plist_string_value(&plist, "CFBundleShortVersionString"))
        .and_then(|raw| Version::parse(&raw).ok());
    Some(TrayInstall { path, version })
}

/// Pulls one `<key>…</key><string>…</string>` value out of an XML plist.
///
/// A five-line scan instead of a plist crate: Tauri writes the Info.plist and
/// always as XML, this reads a single well-known key from it, and a failed
/// read only downgrades "compare versions" to "assume an update is due".
#[cfg(any(target_os = "macos", test))]
fn plist_string_value(plist: &str, key: &str) -> Option<String> {
    let key_tag = format!("<key>{key}</key>");
    let after = &plist[plist.find(&key_tag)? + key_tag.len()..];
    let start = after.find("<string>")? + "<string>".len();
    let end = after[start..].find("</string>")? + start;
    Some(after[start..end].trim().to_owned())
}

#[cfg(target_os = "macos")]
pub fn install(current: &TrayInstall, artifact: &ReleaseArtifact, bytes: &[u8]) -> Result<()> {
    let parent = current.path.parent().ok_or_else(|| {
        ZenmonError::internal(format!(
            "{} has no parent directory",
            current.path.display()
        ))
    })?;

    // Staged next to the destination, not in $TMPDIR: the final step is a
    // rename, which must not cross filesystems.
    let staging = parent.join(format!(".zenmon-tray-update-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&staging);

    let unpacked = unpack_app(bytes, &staging, &artifact.binary, &artifact.url);
    let result = unpacked.and_then(|new_app| {
        let was_running = tray_is_running();
        if was_running {
            quit_tray()?;
        }
        swap_app_bundle(&new_app, &current.path, &staging)?;
        if was_running {
            // Failing to relaunch is worth a message but not a failed update —
            // the new bundle is already in place.
            if let Err(err) = Command::new("open").arg(&current.path).status() {
                eprintln!("note: could not relaunch {}: {err}", current.path.display());
            }
        }
        Ok(())
    });

    let _ = std::fs::remove_dir_all(&staging);
    result
}

/// Unpacks the tar.gz into `staging` and returns the path of the new bundle.
#[cfg(any(target_os = "macos", test))]
fn unpack_app(bytes: &[u8], staging: &Path, app_name: &str, archive_name: &str) -> Result<PathBuf> {
    std::fs::create_dir_all(staging).map_err(|err| {
        ZenmonError::internal(format!("could not create {}: {err}", staging.display()))
    })?;
    let decoder = flate2::read::GzDecoder::new(std::io::Cursor::new(bytes));
    tar::Archive::new(decoder).unpack(staging).map_err(|err| {
        ZenmonError::invalid_input(format!("could not unpack {archive_name}: {err}"))
    })?;

    let new_app = staging.join(app_name);
    if !new_app.is_dir() {
        return Err(ZenmonError::invalid_input(format!(
            "release artifact {archive_name} does not contain {app_name}; \
             the manifest and the archive disagree"
        )));
    }
    Ok(new_app)
}

/// Replaces `dest` with `new_app` by rename, keeping the old bundle inside
/// `staging` so a failed second rename can put it back.
#[cfg(any(target_os = "macos", test))]
fn swap_app_bundle(new_app: &Path, dest: &Path, staging: &Path) -> Result<()> {
    let retired = staging.join("previous.app");
    std::fs::rename(dest, &retired).map_err(|err| {
        ZenmonError::internal(format!(
            "could not move the installed app aside ({} -> {}): {err}. \
             Is {} writable?",
            dest.display(),
            retired.display(),
            dest.parent().unwrap_or(Path::new("/")).display()
        ))
    })?;

    if let Err(err) = std::fs::rename(new_app, dest) {
        // Leaving no app at all is worse than a failed update.
        let restored = std::fs::rename(&retired, dest);
        return Err(ZenmonError::internal(format!(
            "could not move the new app into place: {err}{}",
            match restored {
                Ok(()) => " (the previous app was restored)".to_owned(),
                Err(restore_err) => format!(
                    " — and restoring the previous app from {} failed too: {restore_err}. \
                     Move it back by hand.",
                    retired.display()
                ),
            }
        )));
    }
    Ok(())
}

#[cfg(target_os = "macos")]
fn tray_is_running() -> bool {
    Command::new("pgrep")
        .args(["-x", TRAY_PROCESS])
        .output()
        .map(|out| out.status.success())
        .unwrap_or(false)
}

/// SIGTERM, then wait for the process to leave. Tauri shuts down cleanly on
/// TERM; the wait keeps the swap from racing a capture loop mid-flush.
#[cfg(target_os = "macos")]
fn quit_tray() -> Result<()> {
    let _ = Command::new("pkill").args(["-x", TRAY_PROCESS]).status();
    for _ in 0..50 {
        if !tray_is_running() {
            return Ok(());
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    Err(ZenmonError::internal(
        "zenmon-tray did not exit within 5s; quit it from the tray menu and re-run \
         `zenmon update apply`",
    ))
}

// -------------------------------------------------------------- Windows ----

/// Uninstall registry keys the NSIS installer writes, per install mode
/// (Tauri's default is `currentUser`, so HKCU first).
#[cfg(windows)]
const UNINSTALL_KEYS: [&str; 2] = [
    r"HKCU\Software\Microsoft\Windows\CurrentVersion\Uninstall\zenmon-tray",
    r"HKLM\Software\Microsoft\Windows\CurrentVersion\Uninstall\zenmon-tray",
];

#[cfg(windows)]
pub fn detect() -> Option<TrayInstall> {
    for key in UNINSTALL_KEYS {
        let Some(output) = reg_query(key) else {
            continue;
        };
        let Some(location) = parse_reg_sz(&output, "InstallLocation") else {
            continue;
        };
        let exe = PathBuf::from(location).join(TRAY_PROCESS);
        if !exe.is_file() {
            continue;
        }
        let version =
            parse_reg_sz(&output, "DisplayVersion").and_then(|raw| Version::parse(&raw).ok());
        return Some(TrayInstall { path: exe, version });
    }
    None
}

#[cfg(windows)]
fn reg_query(key: &str) -> Option<String> {
    let output = Command::new("reg").args(["query", key]).output().ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).into_owned())
}

/// Reads one `NAME    REG_SZ    value` line out of `reg query` output.
#[cfg(any(windows, test))]
fn parse_reg_sz(output: &str, name: &str) -> Option<String> {
    output.lines().find_map(|line| {
        let mut parts = line.split_whitespace();
        (parts.next() == Some(name) && parts.next() == Some("REG_SZ")).then(|| {
            // The value may itself contain spaces (`C:\Program Files\...`),
            // so it is everything after the REG_SZ column, not one token.
            let value = parts.collect::<Vec<_>>().join(" ");
            value.trim().to_owned()
        })
    })
}

#[cfg(windows)]
pub fn install(current: &TrayInstall, artifact: &ReleaseArtifact, bytes: &[u8]) -> Result<()> {
    let installer = std::env::temp_dir().join(&artifact.url);
    std::fs::write(&installer, bytes).map_err(|err| {
        ZenmonError::internal(format!("could not write {}: {err}", installer.display()))
    })?;

    let was_running = tray_is_running();
    if was_running {
        // Forced: the tray has no window to receive a close message. The
        // capture loop appends whole lines per message, so a kill loses at
        // most the line in flight.
        let _ = Command::new("taskkill")
            .args(["/IM", TRAY_PROCESS, "/F"])
            .output();
    }

    // `/S` — NSIS silent install. Per-user installs (Tauri's default) need no
    // elevation, so this runs synchronously to completion.
    let status = Command::new(&installer).arg("/S").status().map_err(|err| {
        ZenmonError::internal(format!("could not run {}: {err}", installer.display()))
    })?;
    let _ = std::fs::remove_file(&installer);
    if !status.success() {
        return Err(ZenmonError::internal(format!(
            "the tray installer exited with {status}"
        )));
    }

    if was_running {
        if let Err(err) = Command::new(&current.path).spawn() {
            eprintln!("note: could not relaunch {}: {err}", current.path.display());
        }
    }
    Ok(())
}

#[cfg(windows)]
fn tray_is_running() -> bool {
    Command::new("tasklist")
        .args(["/FI", &format!("IMAGENAME eq {TRAY_PROCESS}"), "/NH"])
        .output()
        .map(|out| String::from_utf8_lossy(&out.stdout).contains(TRAY_PROCESS))
        .unwrap_or(false)
}

// ------------------------------------------------------ other platforms ----

#[cfg(not(any(target_os = "macos", windows)))]
pub fn detect() -> Option<TrayInstall> {
    // No tray is published for other platforms, so there is nothing to find.
    None
}

#[cfg(not(any(target_os = "macos", windows)))]
pub fn install(_current: &TrayInstall, _artifact: &ReleaseArtifact, _bytes: &[u8]) -> Result<()> {
    Err(ZenmonError::internal(
        "no tray update path exists for this platform",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_version_from_a_tauri_info_plist() {
        let plist = r#"<?xml version="1.0" encoding="UTF-8"?>
<plist version="1.0">
<dict>
  <key>CFBundleIdentifier</key>
  <string>com.gongfour.zenmon.tray</string>
  <key>CFBundleShortVersionString</key>
  <string>0.2.0</string>
  <key>CFBundleVersion</key>
  <string>0.2.0</string>
</dict>
</plist>"#;
        assert_eq!(
            plist_string_value(plist, "CFBundleShortVersionString").as_deref(),
            Some("0.2.0")
        );
        assert_eq!(
            plist_string_value(plist, "CFBundleIdentifier").as_deref(),
            Some("com.gongfour.zenmon.tray")
        );
        assert_eq!(plist_string_value(plist, "NoSuchKey"), None);
        assert_eq!(plist_string_value("not a plist", "CFBundleVersion"), None);
    }

    #[test]
    fn reads_reg_sz_values_including_ones_with_spaces() {
        let output = "\r\nHKEY_CURRENT_USER\\...\\zenmon-tray\r\n\
                      \x20   DisplayVersion    REG_SZ    0.1.0\r\n\
                      \x20   InstallLocation    REG_SZ    C:\\Program Files\\zenmon tray\r\n";
        assert_eq!(
            parse_reg_sz(output, "DisplayVersion").as_deref(),
            Some("0.1.0")
        );
        assert_eq!(
            parse_reg_sz(output, "InstallLocation").as_deref(),
            Some(r"C:\Program Files\zenmon tray")
        );
        assert_eq!(parse_reg_sz(output, "Missing"), None);
    }

    fn tar_gz_of_app(app_name: &str) -> Vec<u8> {
        let dir = std::env::temp_dir().join(format!("zenmon-tray-tar-src-{app_name}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join(app_name).join("Contents/MacOS")).unwrap();
        std::fs::write(dir.join(app_name).join("Contents/Info.plist"), b"<plist/>").unwrap();
        std::fs::write(
            dir.join(app_name).join("Contents/MacOS/zenmon-tray"),
            b"NEW",
        )
        .unwrap();

        let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        {
            let mut builder = tar::Builder::new(&mut encoder);
            builder
                .append_dir_all(app_name, dir.join(app_name))
                .unwrap();
            builder.finish().unwrap();
        }
        let bytes = encoder.finish().unwrap();
        let _ = std::fs::remove_dir_all(&dir);
        bytes
    }

    #[test]
    fn unpacks_and_swaps_an_app_bundle() {
        let root = std::env::temp_dir().join("zenmon-tray-swap-test");
        let _ = std::fs::remove_dir_all(&root);
        let dest = root.join("zenmon-tray.app");
        std::fs::create_dir_all(dest.join("Contents/MacOS")).unwrap();
        std::fs::write(dest.join("Contents/MacOS/zenmon-tray"), b"OLD").unwrap();

        let staging = root.join(".staging");
        let bytes = tar_gz_of_app("zenmon-tray.app");
        let new_app = unpack_app(&bytes, &staging, "zenmon-tray.app", "t.tar.gz").unwrap();
        swap_app_bundle(&new_app, &dest, &staging).unwrap();

        let installed = std::fs::read(dest.join("Contents/MacOS/zenmon-tray")).unwrap();
        assert_eq!(installed, b"NEW");
        // the old bundle was retired into staging, not deleted out from under
        // a possibly-running process
        assert!(staging
            .join("previous.app/Contents/MacOS/zenmon-tray")
            .exists());

        let _ = std::fs::remove_dir_all(&root);
    }

    /// The manifest names the bundle; an archive without it is a broken
    /// release, and the message should blame the release.
    #[test]
    fn an_archive_missing_the_bundle_blames_the_release() {
        let root = std::env::temp_dir().join("zenmon-tray-missing-app-test");
        let _ = std::fs::remove_dir_all(&root);

        let bytes = tar_gz_of_app("something-else.app");
        let err = unpack_app(&bytes, &root, "zenmon-tray.app", "t.tar.gz").unwrap_err();
        assert!(err.to_string().contains("disagree"), "{err}");

        let _ = std::fs::remove_dir_all(&root);
    }
}
