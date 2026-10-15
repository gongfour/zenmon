pub mod capture;
pub mod cli;
pub mod commands;
pub mod config;
pub mod state;
pub mod tray;
pub mod update;

use tauri::{AppHandle, Manager};

use crate::state::AppState;

/// Serialize exit with capture transitions without blocking the UI thread.
pub fn quit(app: &AppHandle) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let state = app.state::<AppState>();
        let _operation = state.operations.lock().await;
        if state.lock().shutting_down {
            return;
        }
        if let Err(error) = state::prepare_exit(&app).await {
            state::report_operation_error(&app, error);
            state::show_settings(&app);
            return;
        }
        app.exit(0);
    });
}

fn init_logging(paths: &config::Paths) {
    use tracing_subscriber::prelude::*;
    use tracing_subscriber::EnvFilter;

    // Default: our own crates at debug, everything else at warn (zenoh and
    // the webview stack are extremely chatty). `RUST_LOG` overrides wholesale.
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("warn,zenmon_tray=debug,zenmon_core=debug"));

    let log_dir = paths.data_local_dir.join("logs");
    let file_layer = std::fs::create_dir_all(&log_dir)
        .and_then(|_| std::fs::File::create(log_dir.join("zenmon-tray.log")))
        .ok()
        .map(|file| {
            tracing_subscriber::fmt::layer()
                .with_writer(file)
                .with_ansi(false)
        });

    let _ = tracing_subscriber::registry()
        .with(filter)
        .with(file_layer)
        .with(tracing_subscriber::fmt::layer())
        .try_init();
}

pub fn run() {
    let paths = config::resolve_paths().expect("no resolvable config directory for this user");
    init_logging(&paths);
    let app_config = config::load(&paths);
    let resume = app_config.app.resume_capture_on_launch && app_config.app.last_capture_running;

    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            None,
        ))
        .manage(AppState::new(paths, app_config))
        .manage(update::UpdateState::default())
        .setup(move |app| {
            // A tray app does not belong in the Dock or the Cmd-Tab switcher.
            // Accessory keeps the menu-bar icon and still lets the settings
            // window come forward when opened from the tray.
            #[cfg(target_os = "macos")]
            app.set_activation_policy(tauri::ActivationPolicy::Accessory);

            let handle = app.handle().clone();
            tray::build(&handle)?;
            if resume {
                let resume_handle = handle.clone();
                tauri::async_runtime::spawn(async move {
                    if let Err(err) = state::start_capture(&resume_handle).await {
                        state::report_operation_error(&resume_handle, err);
                    }
                });
            }
            state::push_status(&handle);
            Ok(())
        })
        .on_window_event(|window, event| {
            // Close-to-tray: hide instead of quitting. Only the tray's Quit
            // item actually exits, so capture survives closing the settings
            // window.
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                let _ = window.hide();
                api.prevent_close();
            }
        })
        .invoke_handler(tauri::generate_handler![
            commands::get_config,
            commands::save_config,
            commands::get_status,
            commands::toggle_capture,
            commands::select_profile,
            commands::open_store_folder,
            commands::new_profile,
            commands::hide_settings,
            commands::check_updates,
            commands::apply_tray_update,
            commands::get_update_status,
            commands::cli_status,
            commands::install_cli,
            commands::update_cli,
        ])
        .build(tauri::generate_context!())
        .expect("error while building zenmon-tray")
        .run(|app, event| {
            // Native Quit (including Cmd+Q) must use the same writer teardown
            // as the tray menu. Our final app.exit() is allowed through.
            if let tauri::RunEvent::ExitRequested { api, .. } = event {
                if !app.state::<AppState>().lock().shutting_down {
                    api.prevent_exit();
                    quit(app);
                }
            }
        });
}
