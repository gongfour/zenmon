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

This is one of **two directions through the same release**: the CLI updates the
tray (this section, all platforms), and the tray updates itself and the CLI
(*Settings → Updates*, Windows — see [Updates](#updates) below). Whichever app
you drive, both end up on the release's single version.

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
  update.rs         tray self-update (tauri-plugin-updater), status events
  cli.rs            the zenmon CLI as seen from the tray: detect / install
                    (user PATH) / drive `zenmon update apply --json`
```

---

## Updates

*Settings → Updates* (or the tray menu's **Check for Updates…**) drives two
things through one release feed — the workspace has a single version, one tag,
one release, so one check answers for both apps:

- **The tray itself** — `tauri-plugin-updater` against
  `releases/latest/download/latest.json`. "Update & restart" downloads the
  signed NSIS installer, stops a running capture *cleanly* (segment closed,
  `last_capture_running` saved as true), and hands over to the installer, which
  relaunches the app (`/P /R`); resume-on-launch then brings capture back.
  A failed download changes nothing — capture keeps running through it.
- **The `zenmon` CLI** — the release installer bundles `zenmon.exe` next to
  `zenmon-tray.exe` (externalBin), so a tray update *is* a CLI update for that
  copy. "Install CLI" merely appends the install directory to the user `PATH`
  (HKCU\Environment + `WM_SETTINGCHANGE`; new terminals see it, running ones
  don't). A CLI found elsewhere on `PATH` is updated by running its own
  `zenmon update apply --json` and surfacing the verdict — version gate,
  checksum pipeline and the cargo-bin refusal all stay in the CLI.

Known limits, on purpose:

- **Tray self-update is Windows-only.** `latest.json` carries only the NSIS
  build, and both entry points say so instead of surfacing the plugin's
  "platform not found". The macOS tray is updated by `zenmon update apply`
  (see "Installing & updating a release build" above), which also means the
  macOS .app carries no bundled CLI — externalBin lives in the Windows-only
  release overlay.
- Dev builds refuse both self-update (a release must never overwrite a dev
  binary) and "Install CLI" (the neighbor would be `target/debug/zenmon.exe`).
- Uninstalling the tray removes the bundled CLI but leaves the `PATH` entry
  behind — harmless (the directory is gone), fix is manual.
- No background/auto update. Checks and installs are always user-initiated.

## Updater signing

`tauri-plugin-updater` refuses unsigned installers — signature verification
cannot be disabled. Therefore:

- `tauri.conf.json` carries the **public** key; the private key lives outside
  the repo (conventionally `~/.tauri/zenmon-tray.key`) and in the repository
  secret **`TAURI_SIGNING_PRIVATE_KEY`** (plus
  `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` if the key has one). Generate with:

  ```bash
  npm run tauri signer generate -- -w ~/.tauri/zenmon-tray.key
  ```

- **Losing the private key strands every installed tray** — a new key can't
  sign updates the old pubkey will accept, so users would have to reinstall by
  hand. Back the key file up; rotating it means shipping a release signed with
  the old key that carries the new pubkey.
- `bundle.createUpdaterArtifacts` (which is what demands the key at build
  time) is **not** in `tauri.conf.json` — it lives in the release overlay
  `src-tauri/tauri.release.conf.json` together with the bundled-CLI
  `externalBin`. A plain local `npm run tauri build` therefore needs neither
  the key nor a prebuilt `zenmon.exe`. To reproduce the release build locally:

  ```bash
  cargo build --release --locked -p zenmon-cli
  mkdir -p tray/src-tauri/binaries
  cp target/release/zenmon.exe tray/src-tauri/binaries/zenmon-x86_64-pc-windows-msvc.exe
  cd tray
  TAURI_SIGNING_PRIVATE_KEY=~/.tauri/zenmon-tray.key \
  TAURI_SIGNING_PRIVATE_KEY_PASSWORD= \
    npm run tauri build -- --config src-tauri/tauri.release.conf.json --bundles nsis -- --locked
  ```

  Set `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` even when the key has no password:
  without the variable the signer *prompts* for one, which in a redirected or
  CI-like shell looks like the build hanging forever after "Finished 1 bundle".
  (CI is immune only because referencing an unset secret still defines the env
  var as an empty string.)

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
- **The updater has no capability entry, deliberately.** Capabilities gate the
  *webview's* access to plugin APIs, and the webview never calls the updater —
  check/download/install all run in Rust (`update.rs`), shared by the tray
  menu and the settings window like every other operation, and the frontend
  only listens for `update-status` events (covered by `core:default`). Add
  `updater:default` only if some JS ever calls the plugin directly.
- **Styling is ported from dotori's launcher** (`dotori_rcs/gui/src/styles/app.css`,
  spec `docs/gui_design.md`) so the two apps read as one toolchain: flat
  surfaces separated by lines, accent blue means *interactive* and never status,
  status uses go/warn/stop, shadows only on things that dismiss.
- **The settings window's CSS viewport can be narrow** (~450 px on a 200 % DPI
  display), so the form has a stacking breakpoint at 620 px. Check both widths
  when adding rows.
