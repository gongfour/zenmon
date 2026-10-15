//! Shared app state plus the operations both the tray menu and the webview
//! commands drive. Keeping them here (rather than in `commands.rs`) means the
//! two entry points can't drift — clicking "Capture" in the tray and toggling
//! it in the settings window run exactly the same code.

use std::sync::{Mutex, MutexGuard};

use tauri::menu::CheckMenuItem;
use tauri::{AppHandle, Emitter, Manager, Wry};

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
    pub operations: tokio::sync::Mutex<()>,
}

pub struct Inner {
    pub config: AppConfig,
    pub capture: Option<CaptureHandle>,
    pub stopped_status: Option<CaptureStatus>,
    pub operation_error: Option<String>,
    pub shutting_down: bool,
    /// Last capture state actually pushed to the tray icon. Every
    /// `Shell_NotifyIcon` call repaints the icon, so re-sending the same one at
    /// message rate makes it visibly flicker — this lets the tray skip
    /// no-op icon updates.
    pub tray_icon_state: Option<(CaptureState, bool)>,
    /// The tray menu's "Capture" check item, replaced on every menu rebuild.
    /// Kept so status pushes can set its checkmark: the menu is not rebuilt on
    /// start/stop, and the platform flips a check item on every click by
    /// itself, so without this the checkmark drifts from the real state.
    pub capture_menu_item: Option<CheckMenuItem<Wry>>,
}

impl AppState {
    pub fn new(paths: Paths, config: AppConfig) -> Self {
        Self {
            paths,
            operations: tokio::sync::Mutex::new(()),
            inner: Mutex::new(Inner {
                config,
                capture: None,
                stopped_status: None,
                operation_error: None,
                shutting_down: false,
                tray_icon_state: None,
                capture_menu_item: None,
            }),
        }
    }

    /// The shared state, even after a panic poisoned the lock. Every field is
    /// valid on its own, so a handler that panicked mid-update cannot leave
    /// anything worse than a stale value — while refusing the lock forever
    /// would make every later click panic too and take the tray down.
    pub fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

/// Current status, whether or not a capture task exists.
pub fn status(app: &AppHandle) -> CaptureStatus {
    let state = app.state::<AppState>();
    let inner = state.lock();
    let mut status = match &inner.capture {
        Some(handle) => handle.status(),
        None => inner
            .stopped_status
            .clone()
            .unwrap_or_else(|| CaptureStatus::idle(&inner.config.app.selected_profile)),
    };
    status.operation_error = inner.operation_error.clone();
    status
}

pub fn capture_active(inner: &Inner) -> bool {
    inner.capture.as_ref().is_some_and(CaptureHandle::is_active)
}

/// Persist first. A failure leaves the in-memory config and running task intact.
fn commit_config(state: &AppState, config: AppConfig) -> Result<(), String> {
    config::save(&state.paths, &config).map_err(|e| format!("Could not save settings: {e}"))?;
    state.lock().config = config;
    Ok(())
}

fn validate_config(config: &AppConfig) -> Result<(), String> {
    let mut names = std::collections::HashSet::new();
    for profile in &config.profiles {
        if profile.name.trim().is_empty() || !names.insert(&profile.name) {
            return Err("Profile names must be nonempty and unique".into());
        }
        if profile.key_expr.trim().is_empty() {
            return Err("Capture key expression must not be empty".into());
        }
        profile.to_zenmon_config().map_err(|e| e.to_string())?;
        zenmon_core::capture::CaptureExclude::parse(&[profile.key_expr.as_str()])
            .map_err(|e| e.to_string())?;
        let exclude: Vec<_> = profile
            .exclude
            .iter()
            .map(|e| e.trim())
            .filter(|e| !e.is_empty())
            .collect();
        zenmon_core::capture::CaptureExclude::parse(&exclude).map_err(|e| e.to_string())?;
    }
    config.selected_profile().ok_or("No profile selected")?;
    Ok(())
}

fn needs_restart(old: &AppConfig, new: &AppConfig, running: bool) -> bool {
    // Names identify the selected profile and appear in capture status too.
    running && old.selected_profile() != new.selected_profile()
}

impl AppState {
    /// Caller owns `operations` until teardown completes. Status stays observable
    /// while the task owns its writer and Zenoh session; never drop a live task.
    async fn drain(&self, notify: impl Fn()) -> Result<(), String> {
        let handle = {
            let mut inner = self.lock();
            inner.capture.take().inspect(|handle| {
                let mut status = handle.status();
                status.state = CaptureState::Stopping;
                inner.stopped_status = Some(status);
                handle.stop();
            })
        };
        if let Some(handle) = handle {
            notify();
            let status = handle.join().await;
            let error = status.last_error.clone();
            self.lock().stopped_status = Some(status);
            notify();
            if let Some(error) = error {
                return Err(format!("Capture stopped with an error: {error}"));
            }
        }
        Ok(())
    }
}

fn start_saved(app: &AppHandle) -> Result<(), String> {
    let state = app.state::<AppState>();
    let mut inner = state.lock();
    let profile = inner
        .config
        .selected_profile()
        .cloned()
        .ok_or("No profile selected")?;
    let config = profile.to_zenmon_config().map_err(|e| e.to_string())?;
    let handle = capture::spawn(config, profile);
    let rx = handle.status_rx.clone();
    inner.capture = Some(handle);
    inner.stopped_status = None;
    drop(inner);
    spawn_status_watcher(app.clone(), rx);
    Ok(())
}

enum Action {
    Start,
    Toggle,
    Select(String),
    Replace(AppConfig),
}

async fn operate(app: &AppHandle, action: Action) -> Result<(), String> {
    let state = app.state::<AppState>();
    let _operation = state
        .operations
        .try_lock()
        .map_err(|_| "Capture transition in progress; try again when it finishes".to_string())?;
    if state.lock().shutting_down {
        return Err("Application is shutting down".into());
    }
    let result = async {
        let (old, running) = {
            let inner = state.lock();
            (inner.config.clone(), capture_active(&inner))
        };
        match action {
            Action::Start | Action::Toggle => {
                let stop = matches!(action, Action::Toggle) && running;
                if matches!(action, Action::Start) && running {
                    return Ok(());
                }
                let mut new = old;
                new.app.last_capture_running = !stop;
                if !stop {
                    validate_config(&new)?;
                }
                commit_config(&state, new)?;
                // A failed task can still be closing its connection. Join it too.
                // A previous failure remains visible until the explicit retry.
                let drained = state.drain(|| push_status(app)).await;
                if stop {
                    drained?;
                } else {
                    start_saved(app)?;
                }
            }
            Action::Select(_) | Action::Replace(_) => {
                let mut new = match action {
                    Action::Select(name) => {
                        if !old.profiles.iter().any(|p| p.name == name) {
                            return Err(format!("No such profile: {name}"));
                        }
                        let mut config = old.clone();
                        config.app.selected_profile = name;
                        config
                    }
                    Action::Replace(config) => config,
                    _ => unreachable!(),
                };
                if new.selected_profile().is_none() {
                    if let Some(first) = new.profiles.first() {
                        new.app.selected_profile = first.name.clone();
                    }
                }
                new.app.last_capture_running = running;
                validate_config(&new)?;
                let restart = needs_restart(&old, &new, running);
                commit_config(&state, new)?;
                if restart {
                    state.drain(|| push_status(app)).await.map_err(|e| {
                        format!("Settings saved, but capture could not restart: {e}")
                    })?;
                    start_saved(app)
                        .map_err(|e| format!("Settings saved, but capture could not start: {e}"))?;
                }
            }
        }
        Ok(())
    }
    .await;
    // Rebuild even after a failed restart: the selected setting was saved.
    let menu = crate::tray::rebuild_menu(app);
    let result = result.and(menu);
    state.lock().operation_error = result.as_ref().err().cloned();
    push_status(app);
    result
}

pub async fn start_capture(app: &AppHandle) -> Result<(), String> {
    operate(app, Action::Start).await
}

pub async fn toggle_capture(app: &AppHandle) -> Result<(), String> {
    operate(app, Action::Toggle).await
}

pub async fn select_profile(app: &AppHandle, name: &str) -> Result<(), String> {
    operate(app, Action::Select(name.into())).await
}

pub async fn replace_config(app: &AppHandle, config: AppConfig) -> Result<(), String> {
    operate(app, Action::Replace(config)).await
}

/// Quit and update take the same operation lock as profile changes. Keep it
/// through exit/install so nothing can start a new task after this drain.
pub async fn prepare_exit(app: &AppHandle) -> Result<(), String> {
    let state = app.state::<AppState>();
    let mut config = state.lock().config.clone();
    config.app.last_capture_running = capture_active(&state.lock());
    commit_config(&state, config)?;
    state.drain(|| push_status(app)).await?;
    state.lock().shutting_down = true;
    Ok(())
}

pub fn report_operation_error(app: &AppHandle, error: String) {
    tracing::error!(%error, "capture operation failed");
    app.state::<AppState>().lock().operation_error = Some(error);
    push_status(app);
}

pub fn open_store_folder(app: &AppHandle) -> Result<(), String> {
    let state = app.state::<AppState>();
    let inner = state.lock();
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
fn spawn_status_watcher(app: AppHandle, mut rx: tokio::sync::watch::Receiver<CaptureStatus>) {
    use std::time::Instant;

    tauri::async_runtime::spawn(async move {
        push_status(&app);
        let mut last_visual = Some(rx.borrow().visual());
        let mut last_push = Instant::now();

        while rx.changed().await.is_ok() {
            let status = rx.borrow().clone();
            let visual_changed = last_visual != Some(status.visual());
            if visual_changed || last_push.elapsed() >= VISUAL_REFRESH_INTERVAL {
                // Always read the current owner; an old watcher must never
                // publish its terminal status over a replacement capture.
                push_status(&app);
                last_visual = Some(status.visual());
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
    use super::*;
    use std::sync::Arc;

    fn fixture() -> (tempfile::TempDir, AppState) {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths {
            config_file: dir.path().join("config.toml"),
            data_local_dir: dir.path().into(),
        };
        let config = config::load(&paths);
        (dir, AppState::new(paths, config))
    }

    #[test]
    fn preferences_and_inactive_profiles_do_not_restart_capture() {
        let (_dir, state) = fixture();
        let old = state.lock().config.clone();
        let mut new = old.clone();
        new.app.resume_capture_on_launch = true;
        new.profiles
            .push(config::Profile::seed("other", &state.paths.data_local_dir));
        new.profiles[1].endpoint = "tcp/localhost:7448".into();
        assert!(!needs_restart(&old, &new, true));
        new.profiles[0].exclude.push("private/**".into());
        assert!(needs_restart(&old, &new, true));
        assert!(
            !needs_restart(&old, &new, false),
            "failed/idle tasks must not auto-start"
        );
        new = old.clone();
        new.profiles[0].compression = zenmon_core::trace::Compression::None;
        assert!(needs_restart(&old, &new, true));
        new = old.clone();
        new.profiles
            .push(config::Profile::seed("other", &state.paths.data_local_dir));
        new.app.selected_profile = "other".into();
        assert!(needs_restart(&old, &new, true));
    }

    #[test]
    fn save_failure_preserves_applied_settings_and_existing_file() {
        let (_dir, state) = fixture();
        let original = state.lock().config.clone();
        config::save(&state.paths, &original).unwrap();
        let before = std::fs::read(&state.paths.config_file).unwrap();
        let mut paths = state.paths.clone();
        paths.config_file = paths.config_file.join("blocked.toml");
        let blocked = AppState::new(paths, original.clone());
        let mut next = original.clone();
        next.app.resume_capture_on_launch = true;
        assert!(commit_config(&blocked, next).is_err());
        assert_eq!(blocked.lock().config, original);
        assert_eq!(std::fs::read(&state.paths.config_file).unwrap(), before);
    }

    #[test]
    fn successful_save_replaces_atomically_and_matches_memory() {
        let (_dir, state) = fixture();
        let mut next = state.lock().config.clone();
        commit_config(&state, next.clone()).unwrap();
        next.app.resume_capture_on_launch = true;
        commit_config(&state, next.clone()).unwrap();
        assert_eq!(config::load(&state.paths), next);
        assert_eq!(state.lock().config, next);
        assert_eq!(
            std::fs::read_dir(&state.paths.data_local_dir)
                .unwrap()
                .count(),
            1
        );
    }

    #[test]
    fn invalid_profiles_are_rejected_before_switching() {
        let (_dir, state) = fixture();
        let valid = state.lock().config.clone();
        validate_config(&valid).unwrap();
        let mut invalid = valid.clone();
        invalid.profiles.push(valid.profiles[0].clone());
        assert!(validate_config(&invalid).is_err());
        invalid = valid.clone();
        invalid.profiles[0].key_expr = "a//b".into();
        assert!(validate_config(&invalid).is_err());
        invalid = valid;
        invalid.profiles[0].exclude.push("a//b".into());
        assert!(validate_config(&invalid).is_err());
    }

    #[tokio::test]
    async fn stopping_waits_for_teardown_and_blocks_overlapping_operations() {
        let (_dir, state) = fixture();
        let state = Arc::new(state);
        let (handle, stopped, finish) = capture::controlled_test_capture(None);
        state.lock().capture = Some(handle);
        let worker = state.clone();
        let task = tokio::spawn(async move {
            let _operation = worker.operations.lock().await;
            worker.drain(|| {}).await
        });
        stopped.await.unwrap();
        assert!(state.operations.try_lock().is_err());
        assert_eq!(
            state.lock().stopped_status.as_ref().unwrap().state,
            CaptureState::Stopping
        );
        assert!(!task.is_finished());
        finish.send(()).unwrap();
        task.await.unwrap().unwrap();
        assert!(state.operations.try_lock().is_ok());
        assert_eq!(
            state.lock().stopped_status.as_ref().unwrap().state,
            CaptureState::Idle
        );
    }

    #[tokio::test]
    async fn teardown_failure_is_retained_and_returned_to_restart_caller() {
        let (_dir, state) = fixture();
        let (handle, stopped, finish) = capture::controlled_test_capture(Some("flush failed"));
        state.lock().capture = Some(handle);
        let release = tokio::spawn(async move {
            stopped.await.unwrap();
            finish.send(()).unwrap();
        });
        let error = state.drain(|| {}).await.unwrap_err();
        release.await.unwrap();
        assert!(error.contains("flush failed"));
        let inner = state.lock();
        assert!(inner.capture.is_none());
        assert_eq!(
            inner.stopped_status.as_ref().unwrap().state,
            CaptureState::Failed
        );
        assert_eq!(
            inner.stopped_status.as_ref().unwrap().last_error.as_deref(),
            Some("flush failed")
        );
    }
}
