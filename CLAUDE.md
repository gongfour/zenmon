# CLAUDE.md

## Project Overview

zenmon is a Rust CLI + TUI tool for monitoring and debugging Zenoh networks. Single binary `zenmon` with headless CLI subcommands and an interactive ratatui TUI dashboard, plus `zenmon-tray`, a Tauri desktop app that runs background capture from the system tray.

## Build & Run

```bash
cargo build --release          # Release binary at ./target/release/zenmon
cargo check                    # Quick type check
cargo run -- sub "test/**"     # Run via cargo
```

The tray app is a Tauri project and **must be built through the Tauri CLI**, not
plain cargo — Tauri decides dev-vs-production by the `custom-protocol` feature
(`dev = !custom_protocol` in tauri's build.rs), which only the CLI enables. A
plain `cargo build --release -p zenmon-tray` produces a binary that tries to
load the UI from the vite dev server and shows a connection-refused page.

```bash
cd tray
npm install                    # first time only
npm run tauri dev              # dev: vite HMR + console logs
npm run tauri build -- --no-bundle   # → ./target/release/zenmon-tray.exe
npm run tauri build            # …plus installers
```

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
