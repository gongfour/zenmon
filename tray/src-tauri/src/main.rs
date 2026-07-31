// Release builds are GUI-subsystem: this is a tray app, and an autostart or
// double-click launch must not pop a blank console window. Dev builds
// (`npm run tauri dev`) stay console-subsystem, so the day-to-day loop of
// running it from a terminal and watching `tracing` scroll by is unchanged.
// A *release* binary launched from a terminal reattaches to it in
// `attach_parent_console` below and still streams — the shell just no longer
// blocks on it, so output lands next to your prompt. There is always the file
// log at `%LOCALAPPDATA%\zenmon-tray\data\logs\zenmon-tray.log`.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

/// Reattach a GUI-subsystem build to the console of the process that launched
/// it, if there is one.
///
/// `windows_subsystem = "windows"` means Windows hands us no console and no
/// standard handles, so the stdout `tracing` layer would write into the void.
/// `AttachConsole` fails when there is no parent console — autostart,
/// double-click, `Start-Process` — which is exactly the case we want silent.
#[cfg(all(windows, not(debug_assertions)))]
fn attach_parent_console() {
    use windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE;
    use windows_sys::Win32::Storage::FileSystem::{
        CreateFileW, FILE_GENERIC_READ, FILE_GENERIC_WRITE, FILE_SHARE_READ, FILE_SHARE_WRITE,
        OPEN_EXISTING,
    };
    use windows_sys::Win32::System::Console::{
        AttachConsole, GetStdHandle, SetStdHandle, ATTACH_PARENT_PROCESS, STD_ERROR_HANDLE,
        STD_INPUT_HANDLE, STD_OUTPUT_HANDLE,
    };

    let conin: Vec<u16> = "CONIN$\0".encode_utf16().collect();
    let conout: Vec<u16> = "CONOUT$\0".encode_utf16().collect();

    // SAFETY: raw Win32 calls with their returns checked; the only handles
    // installed are ones a successful `CreateFileW` on the console device
    // returned.
    unsafe {
        if AttachConsole(ATTACH_PARENT_PROCESS) == 0 {
            return;
        }

        // Attaching does not populate the std handles of a process that started
        // without them, and Rust's stdio re-reads them on every write, so point
        // them at the console device by hand.
        for (id, path, access) in [
            (STD_INPUT_HANDLE, &conin, FILE_GENERIC_READ),
            (STD_OUTPUT_HANDLE, &conout, FILE_GENERIC_WRITE),
            (STD_ERROR_HANDLE, &conout, FILE_GENERIC_WRITE),
        ] {
            // An already-valid handle came from a redirect (`zenmon-tray >
            // log.txt`, or a pipe) — leave that one pointed where it is.
            let existing = GetStdHandle(id);
            if !existing.is_null() && existing != INVALID_HANDLE_VALUE {
                continue;
            }
            let console = CreateFileW(
                path.as_ptr(),
                access,
                FILE_SHARE_READ | FILE_SHARE_WRITE,
                std::ptr::null(),
                OPEN_EXISTING,
                0,
                std::ptr::null_mut(),
            );
            if console != INVALID_HANDLE_VALUE {
                SetStdHandle(id, console);
            }
        }
    }
}

fn main() {
    #[cfg(all(windows, not(debug_assertions)))]
    attach_parent_console();

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
