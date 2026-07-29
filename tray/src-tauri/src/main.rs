// Deliberately NOT `windows_subsystem = "windows"`: launching from a terminal
// should attach to it and stream `tracing` output, which is how this gets
// operated day to day. The tradeoff is a console window on a double-click or
// autostart launch. Revisit with an `AttachConsole(ATTACH_PARENT_PROCESS)`
// hybrid if that becomes annoying.

fn main() {
    // `dev` is set by tauri-build when the `custom-protocol` feature is off,
    // i.e. when this was built by bare cargo instead of the Tauri CLI. Such a
    // binary loads the UI from the vite dev server, so running it standalone
    // shows a bare ERR_CONNECTION_REFUSED page with no hint as to why. Say so
    // up front — this is otherwise a genuinely confusing failure.
    #[cfg(dev)]
    eprintln!(
        "warning: this is a DEV build of zenmon-tray — the settings window loads from \
         the vite dev server at the configured devUrl.\n\
         \x20 Run it with `npm run tauri dev` from tray/, or build a standalone binary \
         with `npm run tauri build -- --no-bundle`.\n\
         \x20 (A bare `cargo build --release -p zenmon-tray` produces this dev binary and \
         overwrites the real one at the same path.)"
    );

    zenmon_tray::run();
}
