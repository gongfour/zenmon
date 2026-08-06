//! Tray self-update via `tauri-plugin-updater`.
//!
//! Everything runs on the Rust side and both entry points — the tray menu's
//! "Check for Updates…" and the settings window's Updates section — call the
//! same `check`/`apply` here, per the state.rs rule that shared operations
//! live in one place. The webview is only a *display* for the `update-status`
//! events, so no updater capability entry is needed in capabilities/.
//!
//! Releases are version-locked across the workspace (one tag, one release),
//! so a single updater check answers for the tray *and* the bundled CLI:
//! the NSIS installer this downloads carries the matching `zenmon.exe`.

use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_updater::UpdaterExt;

use crate::config;
use crate::state::AppState;

/// Event name the frontend listens on for update status pushes.
pub const UPDATE_STATUS_EVENT: &str = "update-status";

/// Progress events during download are coalesced to this interval — one event
/// per received chunk would hammer the IPC bridge for no visible benefit.
const PROGRESS_INTERVAL: Duration = Duration::from_millis(200);

#[derive(Clone, Serialize, Default)]
pub struct UpdateStatus {
    /// "idle" | "checking" | "up_to_date" | "available" | "downloading"
    /// | "installing" | "error"
    pub phase: String,
    pub current_version: String,
    pub available_version: Option<String>,
    pub downloaded: Option<u64>,
    pub total: Option<u64>,
    pub message: Option<String>,
}

impl UpdateStatus {
    fn new(phase: &str) -> Self {
        Self {
            phase: phase.to_string(),
            current_version: env!("CARGO_PKG_VERSION").to_string(),
            ..Default::default()
        }
    }
}

/// Managed separately from `AppState` so the capture path never contends with
/// a download in progress for the same lock.
#[derive(Default)]
pub struct UpdateState {
    /// The update `check` found, kept so `apply` doesn't re-fetch the
    /// manifest. `apply` takes it; a stale one is refreshed by re-checking.
    pending: Mutex<Option<tauri_plugin_updater::Update>>,
    /// Last status pushed — what the settings window paints on (re)open.
    last: Mutex<UpdateStatus>,
}

pub fn current_status(app: &AppHandle) -> UpdateStatus {
    let state = app.state::<UpdateState>();
    let last = state.last.lock().expect("update state poisoned");
    if last.phase.is_empty() {
        UpdateStatus::new("idle")
    } else {
        last.clone()
    }
}

fn push(app: &AppHandle, status: UpdateStatus) {
    {
        let state = app.state::<UpdateState>();
        *state.last.lock().expect("update state poisoned") = status.clone();
    }
    let _ = app.emit(UPDATE_STATUS_EVENT, &status);
}

/// A dev binary must never install a release over itself — that is exactly
/// the "stray build replaces a working binary" accident CLAUDE.md documents,
/// with the updater as the accomplice instead of bare cargo.
fn reject_dev_build() -> Result<(), String> {
    if cfg!(dev) {
        return Err(
            "this is a dev build — self-update is disabled so it can't overwrite \
             itself with a release install. Use `npm run tauri build`."
                .to_string(),
        );
    }
    Ok(())
}

/// latest.json only carries the Windows NSIS build; on macOS the tray is
/// updated by `zenmon update apply` (crates/zenmon-cli/src/update/tray.rs).
/// Saying so beats surfacing the plugin's "platform not found" error.
fn reject_unsupported_platform() -> Result<(), String> {
    if cfg!(windows) {
        Ok(())
    } else {
        Err(
            "tray self-update is Windows-only for now — update this tray with \
             `zenmon update apply` instead"
                .to_string(),
        )
    }
}

/// Ask the release endpoint whether a newer version exists. Stores the found
/// update for a later `apply` and reports the outcome as an `UpdateStatus`.
pub async fn check(app: &AppHandle) -> Result<UpdateStatus, String> {
    reject_dev_build()?;
    reject_unsupported_platform()?;
    push(app, UpdateStatus::new("checking"));

    let updater = app.updater().map_err(|e| e.to_string());
    let result = match updater {
        Ok(updater) => updater.check().await.map_err(|e| e.to_string()),
        Err(e) => Err(e),
    };

    match result {
        Ok(Some(update)) => {
            let mut status = UpdateStatus::new("available");
            status.available_version = Some(update.version.clone());
            {
                let state = app.state::<UpdateState>();
                *state.pending.lock().expect("update state poisoned") = Some(update);
            }
            push(app, status.clone());
            Ok(status)
        }
        Ok(None) => {
            let status = UpdateStatus::new("up_to_date");
            push(app, status.clone());
            Ok(status)
        }
        Err(e) => {
            let mut status = UpdateStatus::new("error");
            status.message = Some(e.clone());
            push(app, status);
            Err(e)
        }
    }
}

/// Download and install the pending update, then restart.
///
/// A running capture is the one thing worth interrupting this for: the restart
/// kills it, so the caller must pass `confirmed = true` once the user has
/// agreed. The capture is then stopped *gracefully* (flush + Zenoh teardown,
/// same as Quit) so the last segment closes cleanly, and
/// `last_capture_running` is persisted as true — the relaunched app resumes
/// capture through the ordinary resume-on-launch path.
pub async fn apply(app: &AppHandle, confirmed: bool) -> Result<(), String> {
    reject_dev_build()?;
    reject_unsupported_platform()?;

    let update = {
        let state = app.state::<UpdateState>();
        let taken = state.pending.lock().expect("update state poisoned").take();
        taken
    };
    // No stored check (or it was already consumed): do a fresh one rather
    // than failing — "Update" pressed twice should not need a third click.
    let update = match update {
        Some(update) => update,
        None => {
            check(app).await?;
            let state = app.state::<UpdateState>();
            let taken = state.pending.lock().expect("update state poisoned").take();
            match taken {
                Some(update) => update,
                None => return Err("already up to date".to_string()),
            }
        }
    };

    let capturing = {
        let state = app.state::<AppState>();
        let inner = state.inner.lock().expect("state poisoned");
        inner.capture.is_some()
    };
    if capturing && !confirmed {
        // Sentinel the frontend recognizes to raise its confirm dialog. Raised
        // before any download: the answer may be "not now", and then the
        // bandwidth would have been spent for nothing.
        return Err("capture-running".to_string());
    }

    let mut status = UpdateStatus::new("downloading");
    status.available_version = Some(update.version.clone());
    push(app, status);

    // Download first, while a running capture keeps running — a failed or
    // aborted download must leave the app exactly as it was.
    let progress_app = app.clone();
    let version = update.version.clone();
    let mut downloaded: u64 = 0;
    let mut last_pushed: Option<Instant> = None;

    let bytes = update
        .download(
            move |chunk, content_length| {
                downloaded += chunk as u64;
                let due = last_pushed.is_none_or(|at| at.elapsed() >= PROGRESS_INTERVAL);
                if due {
                    last_pushed = Some(Instant::now());
                    let mut status = UpdateStatus::new("downloading");
                    status.available_version = Some(version.clone());
                    status.downloaded = Some(downloaded);
                    status.total = content_length;
                    push(&progress_app, status);
                }
            },
            || {},
        )
        .await
        .map_err(|e| {
            let mut status = UpdateStatus::new("error");
            status.message = Some(e.to_string());
            push(app, status);
            e.to_string()
        })?;

    // The point of no return: close the capture cleanly, then hand over to
    // the installer. On Windows the install step kills this process (the NSIS
    // run uses /P /R and relaunches the app), so "installing" is the last
    // status that reliably lands.
    stop_capture_for_restart(app).await;
    push(app, UpdateStatus::new("installing"));

    if let Err(e) = update.install(bytes) {
        let mut status = UpdateStatus::new("error");
        status.message = Some(e.to_string());
        push(app, status);
        return Err(e.to_string());
    }

    // Only reached on platforms whose installer does not kill the process
    // (not Windows). Restart explicitly so the new binary takes over.
    app.restart();
}

/// Stop a running capture the way Quit does — flush, close the segment, tear
/// down the Zenoh session — but record it as *running* so the post-update
/// relaunch resumes it.
async fn stop_capture_for_restart(app: &AppHandle) {
    let handle = {
        let state = app.state::<AppState>();
        let mut inner = state.inner.lock().expect("state poisoned");
        inner.config.app.last_capture_running = inner.capture.is_some();
        if let Err(err) = config::save(&state.paths, &inner.config) {
            tracing::error!(error = %err, "failed to save config before update");
        }
        inner.capture.take()
    };

    if let Some(handle) = handle {
        handle.stop();
        handle.join().await;
        crate::state::push_status(app);
    }
}
