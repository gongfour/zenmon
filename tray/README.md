# zenmon-tray

System-tray toggle for background Zenoh capture. Records traffic to a rotating
NDJSON segment store (the same format `zenmon capture --dir` writes and
`zenmon trace` reads), so a session can be captured for post-incident debugging
without keeping a terminal open.

Tauri v2 + React. `src/` is the frontend, `src-tauri/` is the Rust backend and
the Cargo workspace member.

---

## ⚠️ Build it with the Tauri CLI, never with bare `cargo`

Tauri decides dev-vs-production from the **`custom-protocol` cargo feature**
(`dev = !custom_protocol`, in tauri's own `build.rs`). Only the Tauri CLI passes
it. So:

| Command | Result |
|---|---|
| `npm run tauri build -- --no-bundle` | ✅ standalone binary, UI embedded |
| `npm run tauri dev` | ✅ dev binary + vite server, HMR |
| `cargo build --release -p zenmon-tray` | ❌ **dev** binary with no server running → blank `ERR_CONNECTION_REFUSED` page |

Both write to the **same path** (`target/release/zenmon-tray.exe`), so a stray
cargo build silently replaces a working binary with a broken one. Two guards
are in place:

1. `zenmon-tray` is **excluded from the workspace `default-members`**, so a bare
   `cargo build --release` at the repo root does not touch it. (`cargo build
   --workspace` still does — that's an explicit request.)
2. A dev binary **prints a warning naming the problem and the fix** on startup.
   If you ever see `ERR_CONNECTION_REFUSED` in the settings window, run the
   binary from a terminal: the warning tells you exactly which build you have.

To check which binary you have without reading the source:

```bash
./target/release/zenmon-tray.exe 2>&1 | head -1
# prints "warning: this is a DEV build …"  → rebuild with the Tauri CLI
# prints nothing                           → production binary
```

Do not grep the exe for UI strings — Tauri compresses embedded assets, so a
production binary will not contain them in plain text.

---

## Build & run

```bash
cd tray
npm install                            # first time only

npm run tauri dev                      # development: vite HMR + live logs
npm run tauri build -- --no-bundle     # → ../target/release/zenmon-tray.exe
npm run tauri build                    # …and installers
```

The binary starts hidden — it lives in the system tray:

- **Left click** — start / stop capture
- **Right click** — menu (capture toggle, profile, Settings…, Open Store Folder, Quit)
- Closing the settings window hides it; only **Quit** exits (capture survives
  closing the window)

Logs go to stdout *and* `%LOCALAPPDATA%\zenmon-tray\data\logs\zenmon-tray.log`.
`RUST_LOG` overrides the default filter (`warn,zenmon_tray=debug,zenmon_core=debug`).

On Windows the release binary is **GUI-subsystem**, so launching it at login or
by double-click shows nothing but the tray icon — no console window. Started
from a terminal it reattaches to that terminal and still streams logs; the only
difference is that the shell does not block on it, so output arrives beside the
next prompt. Dev builds are unaffected (see below).

---

## Installing & updating a release build

Releases carry the tray in the `trayArtifacts` list of `zenmon.json`, and
**`zenmon update apply` updates an installed tray alongside the CLI** — it
detects the installation, compares versions, downloads, verifies the checksum,
quits the running tray, swaps it, and relaunches (`crates/zenmon-cli/src/update/tray.rs`).
No tray installed means the whole topic is skipped; the updater never installs
a tray you didn't choose to have.

First install is manual, per platform:

- **Windows**: run the `*-setup.exe` from the release. It registers the
  uninstall key the updater later uses for detection.
- **macOS**: untar `zenmon-tray-*-aarch64-apple-darwin.app.tar.gz` into
  `/Applications` (or `~/Applications`). Download with `curl`/`gh` — a
  browser download gets the quarantine attribute, and the bundle is ad-hoc
  signed, not notarized, so Gatekeeper would block it. If a browser was used:
  `xattr -dr com.apple.quarantine /Applications/zenmon-tray.app`. The updater
  itself downloads without the attribute, so updates never hit Gatekeeper.

---

## Layout

```
src/
  App.tsx           settings form (profiles, connection, storage, preferences)
  api.ts            typed wrapper over the Tauri command surface
  theme.ts          system|light|dark resolution → data-theme attribute
  styles/app.css    design tokens (see below)
src-tauri/src/
  lib.rs            Tauri builder, plugins, window events, command registry
  state.rs          shared operations + AppState (the tray and the webview
                    both go through here, so the two cannot drift)
  capture.rs        the capture loop and its status channel
  config.rs         profile schema, load/save, bridge into zenmon-core
  tray.rs           tray icon, menu, click handling, status visuals
  commands.rs       #[tauri::command] wrappers
```

## Things worth knowing before changing this

- **`tauri.conf.json` has no `version` key, on purpose.** Tauri falls back to the
  `zenmon-tray` crate version, which is `version.workspace = true` — so the
  installer name and the app's reported version track the workspace version with
  nothing to bump twice. The release workflow's guard only compares the git tag
  against the workspace version (`.github/workflows/release.yml`), so a
  hardcoded value here would be unguarded and could quietly name the installer
  after the wrong release. Do not add it back.
- **`windows_subsystem = "windows"` is gated on `not(debug_assertions)`**, and a
  release binary calls `AttachConsole(ATTACH_PARENT_PROCESS)` in `main.rs`
  before anything else. Without the gate, `npm run tauri dev` would lose its
  console; without the attach, a release binary run from a terminal would print
  nothing at all — including the DEV-build warning the section above tells you
  to look for. Attaching also has to reinstall the std handles by hand
  (`CONOUT$`), since a GUI-subsystem process starts without them and Rust's
  stdio re-reads them on every write.
- **`ZenmonConfig` cannot be built as a struct literal** outside `zenmon-core` —
  `endpoint_override`/`mode_override` are private. Go through
  `config::resolve_config_with_env`, as `Profile::to_zenmon_config` does.
- **The capture status channel ticks once per captured message.** Anything
  hanging off it must throttle: `set_icon`/`set_tooltip` are each a
  `Shell_NotifyIcon(NIM_MODIFY)` that repaints the tray, and driving them at
  message rate makes the icon visibly strobe. `state.rs` coalesces counter
  updates to 1 Hz and `tray.rs` skips no-op icon writes; state transitions still
  go through immediately.
- **`enforce_retention` is throttled to once a minute**, unlike the CLI's
  per-message call — a `read_dir` per message is fine for a short interactive
  run, not for days unattended.
- **Tauri v2 grants no plugin permissions without
  `src-tauri/capabilities/default.json`.** Without it `emit`/`listen` fail
  silently and capture status never reaches the settings window.
- **Styling is ported from dotori's launcher** (`dotori_rcs/gui/src/styles/app.css`,
  spec `docs/gui_design.md`) so the two apps read as one toolchain: flat
  surfaces separated by lines, accent blue means *interactive* and never status,
  status uses go/warn/stop, shadows only on things that dismiss.
- **The settings window's CSS viewport can be narrow** (~450 px on a 200 % DPI
  display), so the form has a stacking breakpoint at 620 px. Check both widths
  when adding rows.
