//! The `zenmon` CLI as seen from the tray: is one installed, which one, can
//! we install or update it.
//!
//! The release installer bundles `zenmon.exe` next to `zenmon-tray.exe`
//! (externalBin in tauri.release.conf.json), so "install the CLI" is not a
//! download — it is putting the install directory on the user PATH. Updating
//! the tray replaces that bundled copy in the same step (one release, one
//! version). A CLI that lives anywhere else keeps updating itself through its
//! own `zenmon update`, which this module drives with `--json` and surfaces —
//! the version gate, the cargo-bin refusal and the checksum pipeline all stay
//! in one place, the CLI.

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::Serialize;

/// Windows: don't let a console window flash for every spawned `zenmon`.
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

const VERSION_TIMEOUT: Duration = Duration::from_secs(10);
/// `update apply` downloads a release archive; generous but bounded.
const UPDATE_TIMEOUT: Duration = Duration::from_secs(300);

#[derive(Clone, Serialize)]
pub struct CliStatus {
    /// The tray's own version — with one release train, also the version the
    /// CLI is expected to be at.
    pub tray_version: String,
    /// What `zenmon` resolves to on PATH (or the bundled copy, when only the
    /// registry PATH knows about it yet — see `pending_path`).
    pub path: Option<String>,
    pub version: Option<String>,
    /// "tray" (lives in the tray's install dir, updates with the tray),
    /// "cargo" (~/.cargo/bin — cargo owns it), "external" (anything else).
    pub managed_by: Option<String>,
    /// An installer-bundled `zenmon.exe` sits next to the tray binary, so
    /// "Install CLI" is available.
    pub bundled_available: bool,
    /// The user PATH in the registry has the install dir, but this process
    /// (and shells started before the change) don't see it yet.
    pub pending_path: bool,
    /// CLI is present and older than the tray — i.e. older than the release
    /// train this tray came from.
    pub update_available: bool,
}

fn exe_name() -> &'static str {
    if cfg!(windows) {
        "zenmon.exe"
    } else {
        "zenmon"
    }
}

fn tray_dir() -> Option<PathBuf> {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(Path::to_path_buf))
}

/// The `zenmon.exe` the installer put next to the tray binary, if any.
///
/// Dev builds always answer "none": there, the neighbor would be
/// `target/debug/zenmon.exe`, and offering to put a debug target dir on the
/// user PATH is a trap, not a feature.
fn bundled_cli() -> Option<PathBuf> {
    if cfg!(dev) {
        return None;
    }
    let candidate = tray_dir()?.join(exe_name());
    candidate.is_file().then_some(candidate)
}

/// First `zenmon` on this process's PATH.
fn resolve_on_path() -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|dir| dir.join(exe_name()))
        .find(|p| p.is_file())
}

fn cargo_bin_dir() -> Option<PathBuf> {
    if let Some(home) = std::env::var_os("CARGO_HOME") {
        return Some(PathBuf::from(home).join("bin"));
    }
    directories::BaseDirs::new().map(|d| d.home_dir().join(".cargo").join("bin"))
}

/// Case-insensitive, trailing-separator-insensitive directory comparison —
/// how Windows PATH entries actually behave.
fn same_dir(a: &Path, b: &Path) -> bool {
    let norm = |p: &Path| {
        p.to_string_lossy()
            .trim_end_matches(['\\', '/'])
            .to_lowercase()
    };
    norm(a) == norm(b)
}

fn classify(path: &Path) -> &'static str {
    let dir = path.parent().unwrap_or(path);
    if tray_dir().is_some_and(|tray| same_dir(&tray, dir)) {
        return "tray";
    }
    if cargo_bin_dir().is_some_and(|cargo| same_dir(&cargo, dir)) {
        return "cargo";
    }
    "external"
}

/// Run `<exe> --version` and pull the first token that parses as a semver —
/// the same tolerance the CLI's own staged-update self-check applies.
async fn version_of(exe: &Path) -> Result<semver::Version, String> {
    let output = run(exe, &["--version"], VERSION_TIMEOUT).await?;
    output
        .split_whitespace()
        .find_map(|token| semver::Version::parse(token).ok())
        .ok_or_else(|| format!("no version in `zenmon --version` output: {output:?}"))
}

/// Spawn with a timeout, no console window, and the child killed if we stop
/// waiting. Returns stdout on success, a message carrying stderr otherwise.
async fn run(exe: &Path, args: &[&str], timeout: Duration) -> Result<String, String> {
    let mut cmd = tokio::process::Command::new(exe);
    cmd.args(args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);
    #[cfg(windows)]
    cmd.creation_flags(CREATE_NO_WINDOW);

    let output = tokio::time::timeout(timeout, cmd.output())
        .await
        .map_err(|_| format!("`{}` timed out after {timeout:?}", exe.display()))?
        .map_err(|e| format!("failed to run `{}`: {e}", exe.display()))?;

    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    if output.status.success() {
        return Ok(stdout);
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    let detail = if stderr.trim().is_empty() {
        stdout
    } else {
        stderr.into_owned()
    };
    // The CLI reports its errors on one meaningful line (color-eyre without a
    // TTY); keep the tail in case something chattier ran first.
    let tail: String = {
        let trimmed = detail.trim();
        let count = trimmed.chars().count();
        trimmed.chars().skip(count.saturating_sub(500)).collect()
    };
    Err(format!(
        "`{} {}` failed ({}): {}",
        exe.display(),
        args.join(" "),
        output.status,
        tail
    ))
}

/// Where the CLI stands relative to this tray. Never errors: every "can't
/// tell" collapses into "not installed" plus whatever *is* known.
pub async fn status() -> CliStatus {
    let tray_version = env!("CARGO_PKG_VERSION").to_string();
    let bundled = bundled_cli();

    let mut pending_path = false;
    let resolved = resolve_on_path().or_else(|| {
        // Not on this process's PATH — but if the registry user PATH already
        // has the install dir, install has happened and only the environment
        // propagation is behind.
        let bundled = bundled.clone()?;
        let dir = bundled.parent()?;
        if user_path_contains(dir) {
            pending_path = true;
            Some(bundled)
        } else {
            None
        }
    });

    let (path, version, managed_by) = match &resolved {
        Some(exe) => {
            let version = match version_of(exe).await {
                Ok(v) => Some(v),
                Err(err) => {
                    tracing::warn!(error = %err, "zenmon --version failed");
                    None
                }
            };
            (
                Some(exe.display().to_string()),
                version,
                Some(classify(exe).to_string()),
            )
        }
        None => (None, None, None),
    };

    let update_available = match (&version, semver::Version::parse(&tray_version)) {
        (Some(cli), Ok(tray)) => *cli < tray,
        _ => false,
    };

    CliStatus {
        tray_version,
        path,
        version: version.map(|v| v.to_string()),
        managed_by,
        bundled_available: bundled.is_some(),
        pending_path,
        update_available,
    }
}

/// Put the tray's install directory (which carries the bundled `zenmon.exe`)
/// on the user PATH. Idempotent. Windows-only, like the installer itself.
pub async fn install() -> Result<CliStatus, String> {
    let bundled = bundled_cli().ok_or_else(|| {
        "no bundled zenmon CLI next to the tray binary — only installer builds carry one \
         (dev builds never offer this)"
            .to_string()
    })?;
    let dir = bundled
        .parent()
        .ok_or_else(|| "bundled CLI has no parent directory".to_string())?;
    add_to_user_path(dir)?;
    Ok(status().await)
}

/// Drive the CLI's own updater: `zenmon update apply --json`. Returns the
/// CLI's JSON verdict verbatim, so the tray shows exactly what a terminal
/// would. Tray-managed copies refuse — they update with the tray.
pub async fn update() -> Result<serde_json::Value, String> {
    let exe = resolve_on_path().ok_or_else(|| "zenmon CLI not found on PATH".to_string())?;
    if classify(&exe) == "tray" {
        return Err(
            "this zenmon is the copy bundled with zenmon-tray — it is replaced by \
             \"Update\" above, in the same step as the tray itself"
                .to_string(),
        );
    }

    let stdout = run(&exe, &["update", "apply", "--json"], UPDATE_TIMEOUT).await?;
    let line = stdout
        .lines()
        .rev()
        .find(|l| !l.trim().is_empty())
        .ok_or_else(|| "zenmon update produced no output".to_string())?;
    serde_json::from_str(line.trim())
        .map_err(|e| format!("unparseable zenmon update output ({e}): {line}"))
}

#[cfg(windows)]
fn user_path_contains(dir: &Path) -> bool {
    read_user_path()
        .is_some_and(|(value, _)| std::env::split_paths(&value).any(|entry| same_dir(&entry, dir)))
}

#[cfg(not(windows))]
fn user_path_contains(_dir: &Path) -> bool {
    false
}

/// The user `Path` value under HKCU\Environment, with its registry type so a
/// write can preserve REG_EXPAND_SZ vs REG_SZ.
#[cfg(windows)]
fn read_user_path() -> Option<(String, winreg::enums::RegType)> {
    use winreg::enums::HKEY_CURRENT_USER;
    use winreg::RegKey;

    let env = RegKey::predef(HKEY_CURRENT_USER)
        .open_subkey("Environment")
        .ok()?;
    let raw = env.get_raw_value("Path").ok()?;
    let units: Vec<u16> = raw
        .bytes
        .chunks_exact(2)
        .map(|c| u16::from_le_bytes([c[0], c[1]]))
        .take_while(|&u| u != 0)
        .collect();
    Some((String::from_utf16_lossy(&units), raw.vtype))
}

#[cfg(windows)]
fn add_to_user_path(dir: &Path) -> Result<(), String> {
    use winreg::enums::{HKEY_CURRENT_USER, KEY_READ, KEY_WRITE, REG_EXPAND_SZ};
    use winreg::{RegKey, RegValue};

    let dir_str = dir.display().to_string();
    // No user Path value yet → create as REG_EXPAND_SZ, the convention for
    // Path: other tools append entries with %VAR% references and expect them
    // expanded.
    let (current, vtype) = read_user_path().unwrap_or((String::new(), REG_EXPAND_SZ));

    if std::env::split_paths(&current).any(|entry| same_dir(&entry, dir)) {
        return Ok(());
    }

    let next = if current.trim_end_matches(';').trim().is_empty() {
        dir_str
    } else {
        format!("{};{}", current.trim_end_matches(';'), dir_str)
    };

    // UTF-16-LE with a terminating null, the registry's string encoding.
    let mut bytes: Vec<u8> = next.encode_utf16().flat_map(u16::to_le_bytes).collect();
    bytes.extend_from_slice(&[0, 0]);

    let env = RegKey::predef(HKEY_CURRENT_USER)
        .open_subkey_with_flags("Environment", KEY_READ | KEY_WRITE)
        .map_err(|e| format!("open HKCU\\Environment failed: {e}"))?;
    env.set_raw_value("Path", &RegValue { bytes, vtype })
        .map_err(|e| format!("writing user Path failed: {e}"))?;

    broadcast_environment_change();
    Ok(())
}

#[cfg(not(windows))]
fn add_to_user_path(_dir: &Path) -> Result<(), String> {
    Err("installing the CLI onto PATH is implemented for Windows only".to_string())
}

/// Tell running apps (Explorer above all) that the environment changed, so
/// new terminals see the new PATH without a relogin. Fire-and-forget: a
/// hung window must not hang us, hence the timeout flag.
#[cfg(windows)]
fn broadcast_environment_change() {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        SendMessageTimeoutW, HWND_BROADCAST, SMTO_ABORTIFHUNG, WM_SETTINGCHANGE,
    };

    let param: Vec<u16> = "Environment\0".encode_utf16().collect();
    // SAFETY: broadcast of a documented message; the pointer outlives the
    // (synchronous, timeout-bounded) call.
    unsafe {
        SendMessageTimeoutW(
            HWND_BROADCAST,
            WM_SETTINGCHANGE,
            0,
            param.as_ptr() as isize,
            SMTO_ABORTIFHUNG,
            2000,
            std::ptr::null_mut(),
        );
    }
}
