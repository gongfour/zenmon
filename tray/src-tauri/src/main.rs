// Deliberately NOT `windows_subsystem = "windows"`: launching from a terminal
// should attach to it and stream `tracing` output, which is how this gets
// operated day to day. The tradeoff is a console window on a double-click or
// autostart launch. Revisit with an `AttachConsole(ATTACH_PARENT_PROCESS)`
// hybrid if that becomes annoying.

fn main() {
    zenmon_tray::run();
}
