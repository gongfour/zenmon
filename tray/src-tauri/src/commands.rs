//! Tauri commands — the settings webview's entire surface. Each one is a thin
//! wrapper over `crate::state`, which the tray menu drives too.

use tauri::{AppHandle, Manager};

use crate::capture::CaptureStatus;
use crate::config::{AppConfig, Profile};
use crate::state::AppState;

#[tauri::command]
pub fn get_config(app: AppHandle) -> AppConfig {
    let state = app.state::<AppState>();
    let inner = state.inner.lock().expect("state poisoned");
    inner.config.clone()
}

#[tauri::command]
pub fn save_config(app: AppHandle, config: AppConfig) -> Result<(), String> {
    crate::state::replace_config(&app, config)
}

#[tauri::command]
pub fn get_status(app: AppHandle) -> CaptureStatus {
    crate::state::status(&app)
}

#[tauri::command]
pub fn toggle_capture(app: AppHandle) -> Result<(), String> {
    crate::state::toggle_capture(&app)
}

#[tauri::command]
pub fn select_profile(app: AppHandle, name: String) -> Result<(), String> {
    crate::state::select_profile(&app, &name)
}

/// `dir` is the output directory of the profile currently shown in the form —
/// possibly unsaved, possibly not the selected profile. Omitted (tray menu),
/// it falls back to the selected profile's directory.
#[tauri::command]
pub fn open_store_folder(app: AppHandle, dir: Option<String>) -> Result<(), String> {
    crate::state::open_store_folder(&app, dir.as_deref())
}

/// A fresh profile pre-filled with the same defaults first-run uses, so the
/// frontend never has to hardcode paths or rotation numbers.
#[tauri::command]
pub fn new_profile(app: AppHandle, name: String) -> Profile {
    let state = app.state::<AppState>();
    Profile::seed(&name, &state.paths.data_local_dir)
}

#[tauri::command]
pub fn hide_settings(app: AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.hide();
    }
}

#[tauri::command]
pub async fn check_updates(app: AppHandle) -> Result<crate::update::UpdateStatus, String> {
    crate::update::check(&app).await
}

/// `confirmed` acknowledges that a running capture will be stopped (and
/// resumed by the relaunched app). Without it, a running capture makes this
/// return the "capture-running" sentinel instead of proceeding.
#[tauri::command]
pub async fn apply_tray_update(app: AppHandle, confirmed: bool) -> Result<(), String> {
    crate::update::apply(&app, confirmed).await
}

/// Last pushed update status — what the settings window paints on (re)open.
#[tauri::command]
pub fn get_update_status(app: AppHandle) -> crate::update::UpdateStatus {
    crate::update::current_status(&app)
}

#[tauri::command]
pub async fn cli_status() -> crate::cli::CliStatus {
    crate::cli::status().await
}

#[tauri::command]
pub async fn install_cli() -> Result<crate::cli::CliStatus, String> {
    crate::cli::install().await
}

#[tauri::command]
pub async fn update_cli() -> Result<serde_json::Value, String> {
    crate::cli::update().await
}
