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

/// A handle whose task has already ended — Failed, or Idle once the stream
/// closed on its own — is a report, not a running capture.
fn is_live(handle: &CaptureHandle) -> bool {
    matches!(
        handle.status().state,
        CaptureState::Starting | CaptureState::Running
    )
}

/// Start capture on the selected profile. No-op if already running.
pub fn start_capture(app: &AppHandle) -> Result<(), String> {
    let state = app.state::<AppState>();
    let mut inner = state.inner.lock().expect("state poisoned");
    // Clear a dead handle so "Start capture" after a failure retries in one
    // click — previously the first click went to the toggle's stop path and
    // only disposed of the zombie (and wiped the error message with it).
    if inner.capture.as_ref().is_some_and(|h| !is_live(h)) {
        inner.capture.take();
    }
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
    // Judged by the task's actual state, not by handle presence — the UI's
    // button label does the same, and the two disagreeing is exactly the
    // "Start capture that actually stops" bug.
    let running = {
        let state = app.state::<AppState>();
        let inner = state.inner.lock().expect("state poisoned");
        inner.capture.as_ref().is_some_and(is_live)
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

/// Reject configs that would save fine and then fail at capture start.
///
/// Without this, an empty key expression (say) is accepted silently and the
/// failure surfaces minutes or days later when capture is toggled — far from
/// the edit that caused it. The error belongs next to the Save button.
fn validate_config(config: &AppConfig) -> Result<(), String> {
    if config.profiles.is_empty() {
        return Err("at least one profile is required".to_string());
    }
    let mut seen = std::collections::HashSet::new();
    for profile in &config.profiles {
        let name = profile.name.trim();
        if name.is_empty() {
            return Err("a profile has an empty name".to_string());
        }
        if !seen.insert(name.to_lowercase()) {
            return Err(format!("two profiles are named \"{name}\""));
        }
        if profile.key_expr.trim().is_empty() {
            return Err(format!(
                "profile \"{name}\": key expression is empty — nothing would be \
                 recorded. Use ** to record everything"
            ));
        }
        if profile.mode == "client" && profile.endpoint.trim().is_empty() {
            return Err(format!(
                "profile \"{name}\": an endpoint is required in client mode"
            ));
        }
        if profile.output_dir.as_os_str().is_empty() {
            return Err(format!("profile \"{name}\": output directory is empty"));
        }
    }
    Ok(())
}

/// Replace the whole config (settings window "Save"). Restarts a live capture
/// so edits to the active profile take effect immediately.
pub fn replace_config(app: &AppHandle, mut new_config: AppConfig) -> Result<(), String> {
    validate_config(&new_config)?;
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

/// Open a capture directory in the file manager. The settings window passes
/// the *edited* profile's directory — which may differ from the selected
/// profile's and may not be saved yet; `None` (the tray menu) falls back to
/// the selected profile. Without the parameter, the button inside profile-2's
/// Storage section silently opened profile-1's folder.
pub fn open_store_folder(app: &AppHandle, dir: Option<&str>) -> Result<(), String> {
    let dir: std::path::PathBuf = match dir.map(str::trim) {
        Some(explicit) if !explicit.is_empty() => explicit.into(),
        _ => {
            let state = app.state::<AppState>();
            let inner = state.inner.lock().expect("state poisoned");
            let profile = inner
                .config
                .selected_profile()
                .ok_or_else(|| "no profile selected".to_string())?;
            profile.output_dir.clone()
        }
    };

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

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::validate_config;
    use crate::config::{AppConfig, AppSettings, Profile};

    fn config_with(profiles: Vec<Profile>) -> AppConfig {
        AppConfig {
            schema_version: 1,
            app: AppSettings {
                selected_profile: profiles.first().map(|p| p.name.clone()).unwrap_or_default(),
                resume_capture_on_launch: false,
                last_capture_running: false,
            },
            profiles,
        }
    }

    fn seed(name: &str) -> Profile {
        Profile::seed(name, Path::new("C:/tmp/zenmon-test"))
    }

    #[test]
    fn accepts_a_seed_profile() {
        assert!(validate_config(&config_with(vec![seed("default")])).is_ok());
    }

    #[test]
    fn rejects_an_empty_profile_list() {
        assert!(validate_config(&config_with(vec![])).is_err());
    }

    #[test]
    fn rejects_an_empty_key_expression() {
        let mut profile = seed("default");
        profile.key_expr = "  ".to_string();
        let err = validate_config(&config_with(vec![profile])).unwrap_err();
        assert!(err.contains("key expression"), "got: {err}");
    }

    #[test]
    fn rejects_a_missing_endpoint_in_client_mode() {
        let mut profile = seed("default");
        profile.endpoint = String::new();
        let err = validate_config(&config_with(vec![profile])).unwrap_err();
        assert!(err.contains("endpoint"), "got: {err}");
    }

    #[test]
    fn allows_a_missing_endpoint_in_peer_mode() {
        let mut profile = seed("default");
        profile.endpoint = String::new();
        profile.mode = "peer".to_string();
        assert!(validate_config(&config_with(vec![profile])).is_ok());
    }

    #[test]
    fn rejects_duplicate_names_case_insensitively() {
        let err = validate_config(&config_with(vec![seed("Prod"), seed("prod")])).unwrap_err();
        assert!(err.contains("named"), "got: {err}");
    }

    #[test]
    fn rejects_an_empty_name() {
        let mut profile = seed("default");
        profile.name = " ".to_string();
        assert!(validate_config(&config_with(vec![profile])).is_err());
    }
}
