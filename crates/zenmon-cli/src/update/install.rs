//! Turning verified archive bytes into the installed binary.
//!
//! The replacement is **rename-only**, and that is the load-bearing decision.
//! Windows refuses to *delete* a running executable but happily *renames* it,
//! so moving the old binary aside and moving the new one in works while zenmon
//! is running — including from the very process doing the update. The
//! alternative, a detached helper that waits for this process to exit, is what
//! dotori shipped first: when something else held the binary the helper's final
//! delete was denied, the next run then failed a non-forcing move, and the CLI
//! printed success while the installed binary was still the old one.
//!
//! Nothing here is deferred: when the command returns, the new binary is in
//! place.

use std::io::{Cursor, Read};
use std::path::{Path, PathBuf};
use std::process::Command;

use semver::Version;
use zenmon_core::error::{Result, ZenmonError};

/// Highest `.old-N` suffix tried before giving up. Reaching this means a
/// thousand old binaries are pinned by running processes, which is a different
/// problem than the one this loop solves.
const MAX_OLD_SLOTS: u32 = 1000;

#[derive(Debug, Clone)]
pub struct Installation {
    /// The binary being replaced — this process's own executable.
    pub exe: PathBuf,
    pub dir: PathBuf,
}

#[derive(Debug, Default)]
pub struct SwapOutcome {
    /// Old binaries that could not be deleted because something is still
    /// executing them. Not a failure; the next update clears them.
    pub retained: usize,
}

/// Identifies what `update apply` would replace.
///
/// There is no marker file. dotori needs one because it has two install shapes
/// (a bootstrap exe and an NSIS installation) whose apply paths differ; the
/// zenmon CLI is a single binary, so "the file this process is running from" is
/// the whole answer, and a downloaded binary dropped anywhere on PATH is
/// already a managed install with no bootstrap step.
pub fn current_installation() -> Result<Installation> {
    let exe = std::env::current_exe()
        .map_err(|err| ZenmonError::internal(format!("could not locate this executable: {err}")))?;
    // Resolve symlinks so the update lands on the real file rather than
    // replacing a link with a binary.
    let exe = strip_verbatim(std::fs::canonicalize(&exe).unwrap_or(exe));
    let dir = exe
        .parent()
        .ok_or_else(|| ZenmonError::internal(format!("{} has no parent directory", exe.display())))?
        .to_path_buf();

    refuse_cargo_managed(&dir, cargo_bin_dir())?;

    Ok(Installation { exe, dir })
}

/// Drops Windows' `\\?\` verbatim prefix, which `canonicalize` always adds.
///
/// Every path here ends up in a message the user reads, and `\\?\D:\bin\zenmon.exe`
/// is not a path anyone recognises as their own. Only the plain drive form is
/// stripped — `\\?\UNC\...` is left alone, since removing its prefix would
/// change what the path means.
fn strip_verbatim(path: PathBuf) -> PathBuf {
    let text = path.to_string_lossy();
    match text.strip_prefix(r"\\?\") {
        // `D:\...` — a drive letter, a colon, a separator
        Some(rest)
            if rest.len() >= 3
                && rest.as_bytes()[0].is_ascii_alphabetic()
                && rest.as_bytes()[1] == b':' =>
        {
            PathBuf::from(rest)
        }
        _ => path,
    }
}

fn cargo_bin_dir() -> Option<PathBuf> {
    if let Some(home) = std::env::var_os("CARGO_HOME") {
        return Some(PathBuf::from(home).join("bin"));
    }
    let home = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE"))?;
    Some(PathBuf::from(home).join(".cargo").join("bin"))
}

/// Refuses to fight cargo over the same file.
///
/// Overwriting `~/.cargo/bin/zenmon` would leave `.crates2.json` claiming a
/// version that is no longer installed, and the next `cargo install` would
/// silently revert the update. There is no `--force` for this: forcing it just
/// means two managers own one path.
pub fn refuse_cargo_managed(exe_dir: &Path, cargo_bin: Option<PathBuf>) -> Result<()> {
    let Some(cargo_bin) = cargo_bin else {
        return Ok(());
    };
    if !same_dir(exe_dir, &cargo_bin) {
        return Ok(());
    }
    Err(ZenmonError::invalid_input(format!(
        "this zenmon was installed by cargo ({}), which owns that file. \
         Update it with `cargo install --path crates/zenmon-cli` (or `cargo install zenmon`), \
         or install a release binary somewhere else on your PATH and update that one.",
        cargo_bin.display()
    )))
}

/// Compares two directories, canonicalizing when both exist so that
/// `~/.cargo/bin` and a symlinked or `..`-containing spelling of it still
/// match. Falls back to a plain comparison (case-insensitive on Windows) when
/// canonicalization is not possible.
fn same_dir(left: &Path, right: &Path) -> bool {
    if let (Ok(left), Ok(right)) = (std::fs::canonicalize(left), std::fs::canonicalize(right)) {
        return left == right;
    }
    if cfg!(windows) {
        left.as_os_str()
            .to_string_lossy()
            .eq_ignore_ascii_case(&right.as_os_str().to_string_lossy())
    } else {
        left == right
    }
}

/// Pulls the executable out of a release archive.
///
/// Format is chosen by the archive's file name, not by the platform, so a
/// manifest is free to ship either on either — the reader is the same code.
pub fn extract_binary(archive: &[u8], archive_name: &str, binary: &str) -> Result<Vec<u8>> {
    let lower = archive_name.to_ascii_lowercase();
    if lower.ends_with(".zip") {
        extract_from_zip(archive, archive_name, binary)
    } else if lower.ends_with(".tar.gz") || lower.ends_with(".tgz") {
        extract_from_tar_gz(archive, archive_name, binary)
    } else {
        Err(ZenmonError::invalid_input(format!(
            "release artifact {archive_name} is not a .zip or .tar.gz archive"
        )))
    }
}

fn extract_from_zip(archive: &[u8], archive_name: &str, binary: &str) -> Result<Vec<u8>> {
    let mut zip = zip::ZipArchive::new(Cursor::new(archive)).map_err(|err| {
        ZenmonError::invalid_input(format!("{archive_name} is not a readable zip: {err}"))
    })?;
    let mut entry = zip
        .by_name(binary)
        .map_err(|_| missing_entry(archive_name, binary))?;

    let mut bytes = Vec::with_capacity(entry.size() as usize);
    entry.read_to_end(&mut bytes).map_err(|err| {
        ZenmonError::invalid_input(format!(
            "could not read {binary} from {archive_name}: {err}"
        ))
    })?;
    Ok(bytes)
}

fn extract_from_tar_gz(archive: &[u8], archive_name: &str, binary: &str) -> Result<Vec<u8>> {
    let decoder = flate2::read::GzDecoder::new(Cursor::new(archive));
    let mut tar = tar::Archive::new(decoder);
    let entries = tar.entries().map_err(|err| {
        ZenmonError::invalid_input(format!("{archive_name} is not a readable tar.gz: {err}"))
    })?;

    for entry in entries {
        let mut entry = entry.map_err(|err| {
            ZenmonError::invalid_input(format!("could not read {archive_name}: {err}"))
        })?;
        let path = entry.path().map_err(|err| {
            ZenmonError::invalid_input(format!("{archive_name} has an unreadable entry: {err}"))
        })?;
        if path.as_os_str() == binary {
            let mut bytes = Vec::new();
            entry.read_to_end(&mut bytes).map_err(|err| {
                ZenmonError::invalid_input(format!(
                    "could not read {binary} from {archive_name}: {err}"
                ))
            })?;
            return Ok(bytes);
        }
    }
    Err(missing_entry(archive_name, binary))
}

fn missing_entry(archive_name: &str, binary: &str) -> ZenmonError {
    ZenmonError::invalid_input(format!(
        "release artifact {archive_name} does not contain {binary}; \
         the manifest and the archive disagree"
    ))
}

/// Stages the new binary, proves it runs and reports the expected version,
/// then swaps it in.
pub fn stage_and_swap(
    install: &Installation,
    bytes: &[u8],
    expected: &Version,
) -> Result<SwapOutcome> {
    let staged = staged_path(&install.exe)?;
    let _ = std::fs::remove_file(&staged);
    std::fs::write(&staged, bytes).map_err(|err| {
        ZenmonError::internal(format!(
            "could not write {}: {err}. Is the install directory writable?",
            staged.display()
        ))
    })?;
    make_executable(&staged)?;

    if let Err(err) = verify_staged(&staged, expected) {
        let _ = std::fs::remove_file(&staged);
        return Err(err);
    }

    // Move the running binary aside. Windows allows renaming a file that is
    // being executed; it does not allow deleting one.
    let retired = free_old_path(&install.exe)?;
    std::fs::rename(&install.exe, &retired).map_err(|err| {
        let _ = std::fs::remove_file(&staged);
        ZenmonError::internal(format!(
            "could not move the current binary aside ({} -> {}): {err}",
            install.exe.display(),
            retired.display()
        ))
    })?;

    if let Err(err) = std::fs::rename(&staged, &install.exe) {
        // Put it back. Leaving the install directory with no binary at all is
        // far worse than failing the update.
        let restored = std::fs::rename(&retired, &install.exe);
        let _ = std::fs::remove_file(&staged);
        return Err(ZenmonError::internal(format!(
            "could not move the new binary into place: {err}{}",
            match restored {
                Ok(()) => " (the previous binary was restored)".to_owned(),
                Err(restore_err) => format!(
                    " — and restoring the previous binary from {} failed too: {restore_err}. \
                     Rename it back by hand.",
                    retired.display()
                ),
            }
        )));
    }

    Ok(SwapOutcome {
        retained: sweep_retired(&install.dir, &install.exe),
    })
}

/// `zenmon.exe` -> `zenmon.new.exe`; `zenmon` -> `zenmon.new`.
///
/// The extension has to stay last on Windows, or the staged file cannot be
/// executed for the self-check below.
fn staged_path(exe: &Path) -> Result<PathBuf> {
    let dir = exe.parent().unwrap_or(Path::new("."));
    let stem = exe
        .file_stem()
        .ok_or_else(|| ZenmonError::internal(format!("{} has no file name", exe.display())))?
        .to_string_lossy()
        .into_owned();

    let name = match exe.extension() {
        Some(ext) => format!("{stem}.new.{}", ext.to_string_lossy()),
        None => format!("{stem}.new"),
    };
    Ok(dir.join(name))
}

/// Retired binaries keep no executable extension, so nothing runs them by
/// accident and they are trivial to sweep.
fn old_path(exe: &Path, slot: u32) -> PathBuf {
    let dir = exe.parent().unwrap_or(Path::new("."));
    let name = exe.file_name().map(|n| n.to_string_lossy().into_owned());
    dir.join(format!("{}.old-{slot}", name.unwrap_or_default()))
}

fn free_old_path(exe: &Path) -> Result<PathBuf> {
    (0..MAX_OLD_SLOTS)
        .map(|slot| old_path(exe, slot))
        .find(|candidate| !candidate.exists())
        .ok_or_else(|| {
            ZenmonError::internal(format!(
                "no free .old-N slot next to {}; {MAX_OLD_SLOTS} retired binaries are still \
                 pinned by running processes",
                exe.display()
            ))
        })
}

/// Deletes retired binaries, returning how many are still in use.
fn sweep_retired(dir: &Path, exe: &Path) -> usize {
    let Some(prefix) = exe
        .file_name()
        .map(|name| format!("{}.old-", name.to_string_lossy()))
    else {
        return 0;
    };
    let Ok(entries) = std::fs::read_dir(dir) else {
        return 0;
    };

    entries
        .flatten()
        .filter(|entry| entry.file_name().to_string_lossy().starts_with(&prefix))
        .filter(|entry| std::fs::remove_file(entry.path()).is_err())
        .count()
}

#[cfg(unix)]
fn make_executable(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).map_err(|err| {
        ZenmonError::internal(format!(
            "could not make {} executable: {err}",
            path.display()
        ))
    })
}

#[cfg(not(unix))]
fn make_executable(_path: &Path) -> Result<()> {
    Ok(())
}

/// Runs the staged binary and checks it reports the version the manifest
/// promised.
///
/// Comparison is on the parsed semver, not on the whole string. dotori required
/// an exact match against `format!("dotori {version}")` while its actual output
/// carried a build timestamp, so the check never passed and its bootstrap
/// update path had never once worked end to end.
fn verify_staged(staged: &Path, expected: &Version) -> Result<()> {
    let output = Command::new(staged)
        .arg("--version")
        .output()
        .map_err(|err| {
            ZenmonError::internal(format!(
                "the downloaded binary could not be executed: {err}. \
             It may be built for a different platform."
            ))
        })?;

    if !output.status.success() {
        return Err(ZenmonError::internal(format!(
            "the downloaded binary exited with {} when asked for its version",
            output.status
        )));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let reported = parse_version_output(&stdout).ok_or_else(|| {
        ZenmonError::internal(format!(
            "could not read a version from the downloaded binary's output: {:?}",
            stdout.trim()
        ))
    })?;

    if &reported != expected {
        return Err(ZenmonError::invalid_input(format!(
            "the downloaded binary reports version {reported} but the manifest promised \
             {expected}; the release is inconsistent and was not installed"
        )));
    }
    Ok(())
}

/// Pulls the semver out of a `--version` line, tolerating anything around it.
fn parse_version_output(output: &str) -> Option<Version> {
    output
        .split_whitespace()
        .find_map(|token| Version::parse(token.trim_start_matches('v')).ok())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(name);
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn zip_with(entries: &[(&str, &[u8])]) -> Vec<u8> {
        let mut buffer = Vec::new();
        {
            let mut writer = zip::ZipWriter::new(Cursor::new(&mut buffer));
            for (name, bytes) in entries {
                writer
                    .start_file::<_, ()>(*name, zip::write::SimpleFileOptions::default())
                    .unwrap();
                writer.write_all(bytes).unwrap();
            }
            writer.finish().unwrap();
        }
        buffer
    }

    fn tar_gz_with(entries: &[(&str, &[u8])]) -> Vec<u8> {
        let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        {
            let mut builder = tar::Builder::new(&mut encoder);
            for (name, bytes) in entries {
                let mut header = tar::Header::new_gnu();
                header.set_size(bytes.len() as u64);
                header.set_mode(0o755);
                header.set_cksum();
                builder.append_data(&mut header, name, *bytes).unwrap();
            }
            builder.finish().unwrap();
        }
        encoder.finish().unwrap()
    }

    #[test]
    fn extracts_the_binary_from_a_zip() {
        let archive = zip_with(&[("zenmon.exe", b"BINARY"), ("README", b"x")]);
        let bytes = extract_binary(&archive, "zenmon-0.1.0.zip", "zenmon.exe").unwrap();
        assert_eq!(bytes, b"BINARY");
    }

    #[test]
    fn extracts_the_binary_from_a_tar_gz() {
        let archive = tar_gz_with(&[("zenmon", b"BINARY"), ("README", b"x")]);
        let bytes = extract_binary(&archive, "zenmon-0.1.0.tar.gz", "zenmon").unwrap();
        assert_eq!(bytes, b"BINARY");
    }

    /// The manifest names the entry; if the archive does not have it, saying
    /// "they disagree" points at the release, not at the user's machine.
    #[test]
    fn a_missing_entry_blames_the_release_not_the_download() {
        let zip = zip_with(&[("something-else", b"x")]);
        let err = extract_binary(&zip, "a.zip", "zenmon.exe").unwrap_err();
        assert!(err.to_string().contains("disagree"), "{err}");

        let tar = tar_gz_with(&[("something-else", b"x")]);
        let err = extract_binary(&tar, "a.tar.gz", "zenmon").unwrap_err();
        assert!(err.to_string().contains("disagree"), "{err}");
    }

    #[test]
    fn rejects_an_archive_format_it_cannot_read() {
        let err = extract_binary(b"x", "zenmon.7z", "zenmon").unwrap_err();
        assert!(err.to_string().contains(".zip or .tar.gz"), "{err}");
    }

    #[test]
    fn corrupt_archives_are_reported_rather_than_panicking() {
        assert!(extract_binary(b"not a zip", "a.zip", "zenmon").is_err());
        assert!(extract_binary(b"not a gzip", "a.tar.gz", "zenmon").is_err());
    }

    #[test]
    fn staged_path_keeps_the_executable_extension_last() {
        assert_eq!(
            staged_path(Path::new("/opt/bin/zenmon.exe")).unwrap(),
            Path::new("/opt/bin/zenmon.new.exe")
        );
        assert_eq!(
            staged_path(Path::new("/usr/local/bin/zenmon")).unwrap(),
            Path::new("/usr/local/bin/zenmon.new")
        );
    }

    #[test]
    fn retired_binaries_keep_no_executable_extension() {
        assert_eq!(
            old_path(Path::new("/opt/bin/zenmon.exe"), 3),
            Path::new("/opt/bin/zenmon.exe.old-3")
        );
    }

    #[test]
    fn free_old_path_skips_slots_already_taken() {
        let dir = temp_dir("zenmon-update-old-slots");
        let exe = dir.join("zenmon.exe");
        std::fs::write(&exe, b"x").unwrap();
        std::fs::write(old_path(&exe, 0), b"pinned").unwrap();
        std::fs::write(old_path(&exe, 1), b"pinned").unwrap();

        assert_eq!(free_old_path(&exe).unwrap(), old_path(&exe, 2));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn sweeping_removes_retired_binaries_and_counts_what_it_could_not() {
        let dir = temp_dir("zenmon-update-sweep");
        let exe = dir.join("zenmon.exe");
        std::fs::write(&exe, b"current").unwrap();
        std::fs::write(old_path(&exe, 0), b"old").unwrap();
        std::fs::write(old_path(&exe, 1), b"old").unwrap();
        // must not be touched: neither the live binary nor an unrelated file
        std::fs::write(dir.join("other.txt"), b"keep").unwrap();

        assert_eq!(sweep_retired(&dir, &exe), 0);
        assert!(!old_path(&exe, 0).exists());
        assert!(!old_path(&exe, 1).exists());
        assert!(exe.exists());
        assert!(dir.join("other.txt").exists());

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// dotori's latent bug: its self-check demanded an exact string while the
    /// real output carried extra text, so the path never passed.
    #[test]
    fn reads_a_version_from_output_with_extra_text_around_it() {
        assert_eq!(
            parse_version_output("zenmon 0.1.0"),
            Some(Version::new(0, 1, 0))
        );
        assert_eq!(
            parse_version_output("zenmon 0.2.1 (built 2026-08-05T10:00:00Z)"),
            Some(Version::new(0, 2, 1))
        );
        assert_eq!(
            parse_version_output("zenmon v1.4.0\n"),
            Some(Version::new(1, 4, 0))
        );
        assert_eq!(
            parse_version_output("zenmon 0.3.0-rc.1")
                .unwrap()
                .to_string(),
            "0.3.0-rc.1"
        );
        assert_eq!(parse_version_output("no version here"), None);
        assert_eq!(parse_version_output(""), None);
    }

    #[test]
    fn strips_the_windows_verbatim_prefix_from_paths_users_will_read() {
        assert_eq!(
            strip_verbatim(PathBuf::from(r"\\?\D:\bin\zenmon.exe")),
            PathBuf::from(r"D:\bin\zenmon.exe")
        );
        // a UNC verbatim path means something different without its prefix
        let unc = PathBuf::from(r"\\?\UNC\nas\share\zenmon.exe");
        assert_eq!(strip_verbatim(unc.clone()), unc);
        // and anything already plain is untouched
        let plain = PathBuf::from("/usr/local/bin/zenmon");
        assert_eq!(strip_verbatim(plain.clone()), plain);
    }

    #[test]
    fn refuses_to_replace_a_cargo_installed_binary() {
        let cargo_bin = temp_dir("zenmon-update-cargo-bin");

        let err = refuse_cargo_managed(&cargo_bin, Some(cargo_bin.clone())).unwrap_err();
        let message = err.to_string();
        assert!(message.contains("cargo"), "{message}");
        assert!(message.contains("cargo install"), "{message}");

        let elsewhere = temp_dir("zenmon-update-elsewhere");
        assert!(refuse_cargo_managed(&elsewhere, Some(cargo_bin.clone())).is_ok());
        // no cargo at all on this machine is not a reason to refuse
        assert!(refuse_cargo_managed(&elsewhere, None).is_ok());

        let _ = std::fs::remove_dir_all(&cargo_bin);
        let _ = std::fs::remove_dir_all(&elsewhere);
    }
}
