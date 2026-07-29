# CLAUDE.md

## Project Overview

zenmon is a Rust CLI + TUI tool for monitoring and debugging Zenoh networks. Single binary `zenmon` with headless CLI subcommands and an interactive ratatui TUI dashboard, plus `zenmon-tray`, a Tauri desktop app that runs background capture from the system tray.

## Build & Run

```bash
cargo build --release          # Release binary at ./target/release/zenmon
cargo check                    # Quick type check
cargo run -- sub "test/**"     # Run via cargo
```

These only build the CLI/TUI crates — `zenmon-tray` is excluded from the
workspace `default-members` on purpose (see below).

### zenmon-tray must be built with the Tauri CLI, never bare cargo

Tauri picks dev-vs-production from the **`custom-protocol` cargo feature**
(`dev = !custom_protocol`, in tauri's own `build.rs`), and only the Tauri CLI
passes it. `cargo build --release -p zenmon-tray` therefore yields a *dev*
binary that loads the UI from the vite dev server — run standalone it shows a
blank `ERR_CONNECTION_REFUSED` page. Worse, both builds write to the **same
path**, so a stray cargo build silently replaces a working binary.

```bash
cd tray
npm install                          # first time only
npm run tauri dev                    # dev: vite HMR + live logs
npm run tauri build -- --no-bundle   # → ./target/release/zenmon-tray.exe
npm run tauri build                  # …plus installers
```

Guards against re-tripping this: `default-members` excludes the tray from bare
cargo builds, and a dev binary prints a warning naming the fix on startup. To
identify a binary, run it and read the first line — do **not** grep the exe for
UI strings, Tauri compresses embedded assets. Details in `tray/README.md`.

Requires: Rust 1.75+, Node 20+ (tray only), zenohd for testing (homebrew: `brew install zenoh`)

## Project Structure

```
crates/
  zenmon-core/    # Library: Zenoh session, subscribe, query, registry
  zenmon-cli/     # Binary: clap CLI, produces `zenmon`
  zenmon-tui/     # Library: ratatui views, event loop, app state
tray/             # Tauri app: system-tray capture toggle
  src/            #   React + TS frontend
  src-tauri/      #   Rust backend (workspace member `zenmon-tray`)
```

- `zenmon-core` is the shared library — CLI, TUI and the tray app all depend on it
- `zenmon-tui` is a library crate called by `zenmon-cli` via `zenmon tui` subcommand
- Single CLI binary: `zenmon` (defined in zenmon-cli/Cargo.toml)
- `tray/src-tauri` is the workspace member; `crates/` stays pure-Rust crates

## zenmon-tray (Tauri app)

Toggles `zenmon capture --dir`-equivalent recording from the tray, so traffic
can be captured for post-incident debugging without keeping a terminal open.
Full notes in `tray/README.md`; the load-bearing ones:

- **Capture loop** (`tray/src-tauri/src/capture.rs`) is ported from `zenmon-cli`'s
  `Command::Capture` dir-mode arm, with two deliberate differences: the stop
  condition is a `watch<bool>` driven by the tray toggle instead of `ctrl_c()`,
  and `enforce_retention` is throttled to once a minute instead of once per
  message (the CLI's per-write `read_dir` is fine for a short run, not for days).
- **Shared operations** live in `state.rs`, so the tray menu and the settings
  webview can't drift — both call the same start/stop/select code.
- **`ZenmonConfig` cannot be built as a struct literal** outside zenmon-core
  (`endpoint_override`/`mode_override` are private). Go through
  `config::resolve_config_with_env`, as `Profile::to_zenmon_config` does.
- **Styling** is ported from dotori's launcher (`dotori_rcs/gui/src/styles/app.css`,
  spec `docs/gui_design.md`) so the two apps read as one toolchain: flat surfaces
  separated by lines, accent blue means *interactive* (never status), status uses
  go/warn/stop, shadows only on dismissible things.
- **Permissions**: Tauri v2 grants no plugin permissions without
  `src-tauri/capabilities/default.json` — without it `emit`/`listen` fail silently
  and the capture-status events never reach the UI.
- **The capture status channel ticks once per captured message.** Anything hanging
  off it must throttle: `set_icon`/`set_tooltip` are each a
  `Shell_NotifyIcon(NIM_MODIFY)` that repaints the tray, so driving them at message
  rate makes the icon strobe. `state.rs` coalesces to 1 Hz, `tray.rs` skips no-op
  icon writes, and state transitions bypass both.

## Key Patterns

- **Zenoh error handling**: Zenoh errors don't implement `Into<color_eyre::Report>`. Use `.map_err(|e| eyre!(e))` pattern.
- **Payload parsing**: Use `MessagePayload::from_zbytes()` which tries `try_to_string()` first, then `from_slice()`. Never use `to_bytes()` + `serde_json::from_slice()` directly — it fails for cross-language string payloads.
- **TUI logs**: TUI mode sets tracing filter to `"off"` to prevent stderr output from corrupting ratatui display.
- **Non-blocking TUI**: Reconnection and queries run in background tokio tasks. Never block the event loop with await on network calls.
- **Topic discovery**: Topics are collected from received messages (not admin space). Admin space doesn't list pub/sub key expressions.

## Testing

No unit tests yet. Manual testing:

```bash
# Terminal 1: Start router
zenohd

# Terminal 2: Start TUI
./target/release/zenmon tui

# Terminal 3: Publish test data
./target/release/zenmon pub test/hello '{"msg":"world"}' --att '{"source":"debug"}'
```

## Conventions

- Commit messages: `feat(scope):`, `fix(scope):`, `chore:`
- Korean comments are OK in design docs, English in code
- Design spec: `docs/superpowers/specs/`
- Implementation plans: `docs/superpowers/plans/`
