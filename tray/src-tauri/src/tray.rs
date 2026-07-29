//! System tray icon + menu. Rebuilt wholesale whenever the profile list or
//! selection changes; only the icon and tooltip are patched in place on the
//! (much more frequent) capture status updates.

use tauri::image::Image;
use tauri::menu::{CheckMenuItem, IsMenuItem, Menu, MenuItem, PredefinedMenuItem, Submenu};
use tauri::tray::TrayIconBuilder;
use tauri::{AppHandle, Manager, Wry};

use crate::capture::{CaptureState, CaptureStatus};
use crate::state::AppState;

pub const TRAY_ID: &str = "main";

const ID_CAPTURE: &str = "capture";
const ID_SETTINGS: &str = "settings";
const ID_OPEN_FOLDER: &str = "open_folder";
const ID_QUIT: &str = "quit";
const PROFILE_PREFIX: &str = "profile:";

// Status colors from dotori's launcher palette (`gui/src/styles/app.css`):
// --go / --warn / --stop / --faint. Accent blue is deliberately absent —
// in that design accent means "interactive", never "status".
const COLOR_ONLINE: [u8; 4] = [0x15, 0x80, 0x3D, 0xFF];
const COLOR_WARNING: [u8; 4] = [0xB4, 0x53, 0x09, 0xFF];
const COLOR_ERROR: [u8; 4] = [0xB9, 0x1C, 0x1C, 0xFF];
const COLOR_IDLE: [u8; 4] = [0x94, 0xA3, 0xB8, 0xFF];

/// Build the tray icon and its menu. Call once, from `setup`.
pub fn build(app: &AppHandle) -> tauri::Result<()> {
    let menu = build_menu(app)?;

    TrayIconBuilder::with_id(TRAY_ID)
        .icon(solid_icon(COLOR_IDLE))
        .tooltip("zenmon-tray — not capturing")
        .menu(&menu)
        .show_menu_on_left_click(true)
        .on_menu_event(|app, event| {
            let id = event.id.as_ref();
            let app = app.clone();
            match id {
                ID_CAPTURE => {
                    if let Err(err) = crate::state::toggle_capture(&app) {
                        tracing::error!(error = %err, "toggle capture failed");
                        // Reflect the failure instead of leaving the checkmark lying.
                        crate::state::push_status(&app);
                    }
                }
                ID_SETTINGS => crate::state::show_settings(&app),
                ID_OPEN_FOLDER => {
                    if let Err(err) = crate::state::open_store_folder(&app) {
                        tracing::error!(error = %err, "open store folder failed");
                    }
                }
                ID_QUIT => crate::quit(&app),
                other => {
                    if let Some(name) = other.strip_prefix(PROFILE_PREFIX) {
                        if let Err(err) = crate::state::select_profile(&app, name) {
                            tracing::error!(error = %err, "select profile failed");
                        }
                    }
                }
            }
        })
        .build(app)?;

    Ok(())
}

/// Rebuild and install the menu — for profile add/remove/rename/selection.
pub fn rebuild_menu(app: &AppHandle) -> Result<(), String> {
    let menu = build_menu(app).map_err(|e| e.to_string())?;
    let tray = app
        .tray_by_id(TRAY_ID)
        .ok_or_else(|| "tray icon not found".to_string())?;
    tray.set_menu(Some(menu)).map_err(|e| e.to_string())
}

fn build_menu(app: &AppHandle) -> tauri::Result<Menu<Wry>> {
    let (profiles, selected, capturing) = {
        let state = app.state::<AppState>();
        let inner = state.inner.lock().expect("state poisoned");
        (
            inner
                .config
                .profiles
                .iter()
                .map(|p| p.name.clone())
                .collect::<Vec<_>>(),
            inner.config.app.selected_profile.clone(),
            inner.capture.is_some(),
        )
    };

    let capture =
        CheckMenuItem::with_id(app, ID_CAPTURE, "Capture", true, capturing, None::<&str>)?;

    let profile_items: Vec<CheckMenuItem<Wry>> = profiles
        .iter()
        .map(|name| {
            CheckMenuItem::with_id(
                app,
                format!("{PROFILE_PREFIX}{name}"),
                name,
                true,
                name == &selected,
                None::<&str>,
            )
        })
        .collect::<tauri::Result<_>>()?;
    let profile_refs: Vec<&dyn IsMenuItem<Wry>> = profile_items
        .iter()
        .map(|i| i as &dyn IsMenuItem<Wry>)
        .collect();
    let profile_menu = Submenu::with_items(app, "Profile", true, &profile_refs)?;

    let settings = MenuItem::with_id(app, ID_SETTINGS, "Settings…", true, None::<&str>)?;
    let open_folder =
        MenuItem::with_id(app, ID_OPEN_FOLDER, "Open Store Folder", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, ID_QUIT, "Quit", true, None::<&str>)?;
    let sep1 = PredefinedMenuItem::separator(app)?;
    let sep2 = PredefinedMenuItem::separator(app)?;

    let items: Vec<&dyn IsMenuItem<Wry>> = vec![
        &capture,
        &profile_menu,
        &sep1,
        &settings,
        &open_folder,
        &sep2,
        &quit,
    ];
    Menu::with_items(app, &items)
}

/// Patch the icon and tooltip to match the current capture status.
pub fn refresh_visuals(app: &AppHandle, status: &CaptureStatus) {
    let Some(tray) = app.tray_by_id(TRAY_ID) else {
        return;
    };

    let color = match status.state {
        CaptureState::Running => COLOR_ONLINE,
        CaptureState::Starting => COLOR_WARNING,
        CaptureState::Failed => COLOR_ERROR,
        CaptureState::Idle => COLOR_IDLE,
    };
    let _ = tray.set_icon(Some(solid_icon(color)));

    let tooltip = match (&status.last_error, status.state) {
        (Some(err), _) => format!("zenmon-tray — {}: {err}", status.profile_name),
        (None, CaptureState::Running) => format!(
            "zenmon-tray — {} ({} msgs)",
            status.profile_name, status.messages_written
        ),
        (None, CaptureState::Starting) => {
            format!("zenmon-tray — {} (connecting…)", status.profile_name)
        }
        (None, _) => "zenmon-tray — not capturing".to_string(),
    };
    let _ = tray.set_tooltip(Some(&tooltip));
}

fn solid_icon(rgba: [u8; 4]) -> Image<'static> {
    let size = 32u32;
    let mut buf = Vec::with_capacity((size * size * 4) as usize);
    for _ in 0..(size * size) {
        buf.extend_from_slice(&rgba);
    }
    Image::new_owned(buf, size, size)
}
