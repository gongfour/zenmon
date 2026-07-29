//! Shared app state plus the operations both the tray menu and the webview
//! commands drive. Keeping them here (rather than in `commands.rs`) means the
//! two entry points can't drift — clicking "Capture" in the tray and toggling
//! it in the settings window run exactly the same code.

use std::sync::Mutex;

use tauri::{AppHandle, Emitter, Manager};

use crate::capture::{self, CaptureHandle, CaptureState, CaptureStatus};
use crate::config::{self, AppConfig, Paths};

/// Event name the frontend listens on for capture status pushes.
pub const CAPTURE_STATUS_EVENT: &str = "capture-status";

/// How often a *running* capture's counters are allowed to reach the tray and
/// the webview. State transitions bypass this.
const VISUAL_REFRESH_INTERVAL: std::time::Duration = std::time::Duration::from_secs(1);

pub struct AppState {
    pub paths: Paths,
    pub inner: Mutex<Inner>,
}

pub struct Inner {
    pub config: AppConfig,
    pub capture: Option<CaptureHandle>,
    /// Last capture state actually pushed to the tray icon. Every
    /// `Shell_NotifyIcon` call repaints the icon, so re-sending the same one at
    /// message rate makes it visibly flicker — this lets the tray skip
    /// no-op icon updates.
    pub tray_icon_state: Option<CaptureState>,
}

impl AppState {
    pub fn new(paths: Paths, config: AppConfig) -> Self {
        Self {
            paths,
            inner: Mutex::new(Inner {
                config,
                capture: None,
                tray_icon_state: None,
            }),
        }
    }
}

/// Current status, whether or not a capture task exists.
pub fn status(app: &AppHandle) -> CaptureStatus {
    let state = app.state::<AppState>();
    let inner = state.inner.lock().expect("state poisoned");
    match &inner.capture {
        Some(handle) => handle.status(),
        None => CaptureStatus::idle(&inner.config.app.selected_profile),
    }
}

fn persist(state: &AppState, inner: &Inner) {
    if let Err(err) = config::save(&state.paths, &inner.config) {
        tracing::error!(error = %err, "failed to save config");
    }
}

/// Start capture on the selected profile. No-op if already running.
pub fn start_capture(app: &AppHandle) -> Result<(), String> {
    let state = app.state::<AppState>();
    let mut inner = state.inner.lock().expect("state poisoned");
    if inner.capture.is_some() {
        return Ok(());
    }

    let profile = inner
        .config
        .selected_profile()
        .cloned()
        .ok_or_else(|| "no profile selected".to_string())?;
    let zenmon_config = profile.to_zenmon_config().map_err(|e| e.to_string())?;

    inner.capture = Some(capture::spawn(zenmon_config, profile));
    inner.config.app.last_capture_running = true;
    persist(&state, &inner);
    drop(inner);

    spawn_status_watcher(app.clone());
    Ok(())
}

/// Stop capture. The spawned task keeps running to completion (flush +
/// teardown) after the stop signal — dropping the handle here is safe because
/// the signal is already buffered in the watch channel.
pub fn stop_capture(app: &AppHandle) {
    let state = app.state::<AppState>();
    let mut inner = state.inner.lock().expect("state poisoned");
    if let Some(handle) = inner.capture.take() {
        handle.stop();
    }
    inner.config.app.last_capture_running = false;
    persist(&state, &inner);
    drop(inner);

    push_status(app);
}

pub fn toggle_capture(app: &AppHandle) -> Result<(), String> {
    let running = {
        let state = app.state::<AppState>();
        let inner = state.inner.lock().expect("state poisoned");
        inner.capture.is_some()
    };
    if running {
        stop_capture(app);
        Ok(())
    } else {
        start_capture(app)
    }
}

pub fn select_profile(app: &AppHandle, name: &str) -> Result<(), String> {
    let was_running = {
        let state = app.state::<AppState>();
        let mut inner = state.inner.lock().expect("state poisoned");
        if inner.config.app.selected_profile == name {
            return Ok(());
        }
        if !inner.config.profiles.iter().any(|p| p.name == name) {
            return Err(format!("no such profile: {name}"));
        }
        inner.config.app.selected_profile = name.to_string();
        let was_running = inner.capture.is_some();
        persist(&state, &inner);
        was_running
    };

    // Re-point a live capture at the newly selected profile so the running
    // session never silently diverges from what the UI shows.
    if was_running {
        stop_capture(app);
        start_capture(app)?;
    }
    crate::tray::rebuild_menu(app)?;
    push_status(app);
    Ok(())
}

/// Replace the whole config (settings window "Save"). Restarts a live capture
/// so edits to the active profile take effect immediately.
pub fn replace_config(app: &AppHandle, mut new_config: AppConfig) -> Result<(), String> {
    let was_running = {
        let state = app.state::<AppState>();
        let mut inner = state.inner.lock().expect("state poisoned");
        let was_running = inner.capture.is_some();

        // The selected profile may have been renamed or deleted.
        if !new_config
            .profiles
            .iter()
            .any(|p| p.name == new_config.app.selected_profile)
        {
            if let Some(first) = new_config.profiles.first() {
                new_config.app.selected_profile = first.name.clone();
            }
        }
        new_config.app.last_capture_running = was_running;
        inner.config = new_config;
        persist(&state, &inner);
        was_running
    };

    if was_running {
        stop_capture(app);
        start_capture(app)?;
    }
    crate::tray::rebuild_menu(app)?;
    push_status(app);
    Ok(())
}

pub fn open_store_folder(app: &AppHandle) -> Result<(), String> {
    let state = app.state::<AppState>();
    let inner = state.inner.lock().expect("state poisoned");
    let profile = inner
        .config
        .selected_profile()
        .ok_or_else(|| "no profile selected".to_string())?;
    let dir = profile.output_dir.clone();
    drop(inner);

    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    #[cfg(windows)]
    let result = std::process::Command::new("explorer").arg(&dir).spawn();
    #[cfg(target_os = "macos")]
    let result = std::process::Command::new("open").arg(&dir).spawn();
    #[cfg(all(unix, not(target_os = "macos")))]
    let result = std::process::Command::new("xdg-open").arg(&dir).spawn();

    // Windows Explorer exits non-zero even on success, so only a spawn
    // failure is worth reporting.
    result.map(|_| ()).map_err(|e| e.to_string())
}

pub fn show_settings(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
    }
}

/// Push the current status to both the webview and the tray visuals.
pub fn push_status(app: &AppHandle) {
    let status = status(app);
    crate::tray::refresh_visuals(app, &status);
    let _ = app.emit(CAPTURE_STATUS_EVENT, &status);
}

/// Follow the running capture's status channel and mirror it into the tray
/// visuals and the webview. Ends when the capture task does.
///
/// The channel ticks once per captured message — potentially hundreds of times
/// a second — but nothing downstream benefits from that rate: the tray shows a
/// message *count* and the settings window shows human-readable numbers. So
/// updates are coalesced to [`VISUAL_REFRESH_INTERVAL`], with state changes
/// (start/stop/failure) always going through immediately.
fn spawn_status_watcher(app: AppHandle) {
    use std::time::Instant;

    tauri::async_runtime::spawn(async move {
        let mut rx = {
            let state = app.state::<AppState>();
            let inner = state.inner.lock().expect("state poisoned");
            match &inner.capture {
                Some(handle) => handle.status_rx.clone(),
                None => return,
            }
        };

        push_status(&app);
        let mut last_state = Some(rx.borrow().state);
        let mut last_push = Instant::now();

        while rx.changed().await.is_ok() {
            let status = rx.borrow().clone();
            let state_changed = last_state != Some(status.state);
            if state_changed || last_push.elapsed() >= VISUAL_REFRESH_INTERVAL {
                crate::tray::refresh_visuals(&app, &status);
                let _ = app.emit(CAPTURE_STATUS_EVENT, &status);
                last_state = Some(status.state);
                last_push = Instant::now();
            }
        }

        // The task ended (stopped, or the subscription died) — make sure the
        // final state lands even if it arrived inside the throttle window.
        push_status(&app);
    });
}
