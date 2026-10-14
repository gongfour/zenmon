# CLAUDE.md

## Project Overview

zenmon is a Rust CLI + TUI tool for monitoring and debugging Zenoh networks. Single binary `zenmon` with headless CLI subcommands and an interactive ratatui TUI dashboard, plus `zenmon-tray`, a Tauri desktop app that runs background capture from the system tray.

The product goal is to make Zenoh easy to explore, use, and debug for any
application. Basic workflows must work without a contract or project-specific
key naming. Keep application schemas and conventions in user-supplied files;
present contracts and scenario adapters as optional advanced features.

`README.md` is the short introduction and quick start. Detailed usage belongs
in `docs/usage.md`; tray-specific development notes belong in `tray/README.md`.

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

Requires: current stable Rust (use the committed lockfile), Node 20+ (tray only).
Automated CLI tests start their own loopback Zenoh sessions; a local `zenohd`
is useful for manual client-mode testing.

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
- Single CLI binary: `zenmon` (defined in zenmon-cli/Cargo.toml). `main.rs` only
  parses, resolves the config and dispatches; each command lives in
  `zenmon-cli/src/cmd/<command>.rs` and takes its clap `…Args` struct plus a
  `Ctx` of global flags. Helpers shared across commands are in `common.rs`.
- `tray/src-tauri` is the workspace member; `crates/` stays pure-Rust crates

## zenmon-tray (Tauri app)

Toggles `zenmon capture --dir`-equivalent recording from the tray, so traffic
can be captured for post-incident debugging without keeping a terminal open.
Full notes in `tray/README.md`; the load-bearing ones:

- **Capture pipeline** is one `Recorder` in `zenmon-core/src/capture.rs`,
  shared by `zenmon capture` and the tray (`tray/src-tauri/src/capture.rs`):
  exclude → record → write → retention. Retention runs on segment rotation and
  at most once a minute otherwise, never per message (each run lists the dir).
  Only the session and stop condition differ per app: Ctrl+C/`--count` for the
  CLI, a `watch<bool>` toggle for the tray. A failed tray write keeps the
  capture running but sets `last_error` (amber icon, "write failing") until the
  next successful write clears it.
- **Shared operations** live in `state.rs`, so the tray menu and the settings
  webview can't drift — both call the same start/stop/select code.
- **`ZenmonConfig` cannot be built as a struct literal** outside zenmon-core
  (`endpoint_override`/`mode_override` are private). Go through
  `config::resolve_config_with_env`, as `Profile::to_zenmon_config` does.
- **Styling** is specified in `tray/src/styles/app.css` (tokens + rules, the
  canonical spec): flat surfaces separated by lines, accent blue means
  *interactive* (never status), status uses go/warn/stop, shadows only on
  dismissible things.
- **Windows subsystem**: release builds set `windows_subsystem = "windows"` so an
  autostart/double-click launch is tray-only with no console window; the attribute
  is gated on `not(debug_assertions)` (dev keeps its console) and `main.rs`
  reattaches to the parent console — std handles included — so a release binary run
  from a terminal still streams logs.
- **Permissions**: Tauri v2 grants no plugin permissions without
  `src-tauri/capabilities/default.json` — without it `emit`/`listen` fail silently
  and the capture-status events never reach the UI.
- **The capture status channel ticks once per captured message.** Anything hanging
  off it must throttle: `set_icon`/`set_tooltip` are each a
  `Shell_NotifyIcon(NIM_MODIFY)` that repaints the tray, so driving them at message
  rate makes the icon strobe. `state.rs` coalesces to 1 Hz, `tray.rs` skips no-op
  icon writes, and state transitions bypass both.
- **Updates flow both ways through one release.** CLI-driven:
  `zenmon update apply` updates an installed tray alongside the CLI
  (`crates/zenmon-cli/src/update/tray.rs`, `trayArtifacts` in zenmon.json —
  all platforms). Tray-driven (`update.rs`/`cli.rs`, Windows only): the tray
  self-updates via `tauri-plugin-updater` (signed NSIS, endpoint `latest.json`
  on the GitHub release), and the release installer bundles `zenmon.exe` as an
  externalBin so a tray update updates the CLI too; "Install CLI" just adds
  the install dir to the user PATH. All tray updater logic is Rust-side — no
  `updater:*` capability needed. `bundle.createUpdaterArtifacts` +
  `externalBin` live in the CI-only overlay
  `tray/src-tauri/tauri.release.conf.json`, NOT tauri.conf.json, so local
  builds need neither the signing key (`TAURI_SIGNING_PRIVATE_KEY` secret;
  private key outside the repo, see tray/README.md § Updater signing) nor a
  prebuilt CLI. Dev builds refuse self-update and CLI install; non-Windows
  builds refuse self-update with a pointer to `zenmon update`.

## Key Patterns

- **Zenoh error handling**: Zenoh errors don't implement `Into<color_eyre::Report>`. Use `.map_err(|e| eyre!(e))` pattern.
- **Payload parsing**: Use `MessagePayload::from_zbytes()` which tries `try_to_string()` first, then `from_slice()`. Never use `to_bytes()` + `serde_json::from_slice()` directly — it fails for cross-language string payloads.
- **TUI logs**: TUI mode sets tracing filter to `"off"` to prevent stderr output from corrupting ratatui display.
- **Non-blocking TUI**: Reconnection and queries run in background tokio tasks. Never block the event loop with await on network calls.
- **Topic discovery**: Topics are collected from received messages (not admin space). Admin space doesn't list pub/sub key expressions.

## Testing

`cargo test --workspace` runs the suite (~590 tests). Most are pure logic;
`crates/zenmon-cli/tests/e2e.rs` runs the real binary (`sub`, `pub`, `query`,
`capture` → `trace read`) against an in-process Zenoh peer on loopback, so no
`zenohd` is needed. The TUI and anything interactive still need the manual
loop below.

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
