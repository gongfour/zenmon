# zenmon

[![CI](https://github.com/gongfour/zenmon/actions/workflows/ci.yml/badge.svg)](https://github.com/gongfour/zenmon/actions/workflows/ci.yml)
[![Latest release](https://img.shields.io/github/v/release/gongfour/zenmon)](https://github.com/gongfour/zenmon/releases/latest)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](https://opensource.org/licenses/MIT)

**Zenoh network monitor and debugger.** One `zenmon` binary with CLI
subcommands and an interactive TUI dashboard, plus **zenmon-tray**, a desktop
app that records traffic from the system tray.

A lightweight terminal-based alternative to web dashboards: zenmon speaks the
native Zenoh API directly (not the REST plugin), so attachments and binary
payloads are first-class — and every command has a stable `--json` contract
designed to be driven by scripts and AI agents.

- **Live** — `sub` / `pub` / `query` / `nodes` with attachments, MessagePack
  auto-decode, and bounded runs (`--count`, `--duration`) that always terminate
- **TUI dashboard** — five views: overview, topics with live values, message
  stream, interactive query, node table
- **Time-shifted** — a rotating capture store (`capture --dir`, or the tray)
  plus pure offline readers (`trace stats` / `trace read`): see what happened
  while you were away, without a live session
- **Diagnosis episodes** — `scenario` triggers an actuation, observes a bounded
  window, and correlates it all into one episode JSON to reason over
- **Contract-aware** — annotate observed traffic against the
  `*.contract.yaml` your project declares
- **Self-updating** — `zenmon update` keeps the CLI *and* an installed tray
  current; on Windows the tray updates itself (and its bundled CLI) from
  *Settings → Updates*

## Install

Prebuilt binaries for every release are on the
[releases page](https://github.com/gongfour/zenmon/releases/latest) — no Rust
toolchain needed. Releases are built only by
[`release.yml`](.github/workflows/release.yml) from a `v<version>` tag; each one
carries a `zenmon.json` manifest naming every asset with its SHA-256.

### Windows

The quickest path is the tray installer — it bundles the CLI:

1. Download and run `zenmon-tray_<version>_x64-setup.exe` (per-user install,
   no admin prompt). zenmon-tray lands in the Start menu and the system tray.
2. Right-click the tray icon → **Settings…** → **Updates** → **Install CLI**.
   This puts the bundled `zenmon.exe` on your user `PATH`; newly opened
   terminals see it.

CLI only: download `zenmon-<version>-x86_64-pc-windows-msvc.zip`, unpack, and
put `zenmon.exe` in a folder on your `PATH` (for example
`%LOCALAPPDATA%\Programs\Zenmon\bin`). Avoid `%USERPROFILE%\.cargo\bin` —
`zenmon update` refuses to manage a binary cargo owns.

### Linux (x86_64)

```bash
# replace 0.3.0 with the latest release version
curl -LO https://github.com/gongfour/zenmon/releases/download/v0.3.0/zenmon-0.3.0-x86_64-unknown-linux-gnu.tar.gz
tar xzf zenmon-0.3.0-x86_64-unknown-linux-gnu.tar.gz -C ~/.local/bin zenmon
zenmon --version
```

### macOS (Apple Silicon)

```bash
# CLI — replace 0.3.0 with the latest release version
curl -LO https://github.com/gongfour/zenmon/releases/download/v0.3.0/zenmon-0.3.0-aarch64-apple-darwin.tar.gz
tar xzf zenmon-0.3.0-aarch64-apple-darwin.tar.gz -C ~/.local/bin zenmon

# tray (optional) — download with curl/gh, not a browser: the bundle is
# ad-hoc signed, not notarized, and a browser download gets quarantined
curl -LO https://github.com/gongfour/zenmon/releases/download/v0.3.0/zenmon-tray-0.3.0-aarch64-apple-darwin.app.tar.gz
tar xzf zenmon-tray-0.3.0-aarch64-apple-darwin.app.tar.gz -C /Applications
```

If a browser did fetch the tray:
`xattr -dr com.apple.quarantine /Applications/zenmon-tray.app`.

Other platforms — including Termux/Android — build from source; see
[Build features](#build-features).

### Verify a download (optional)

Every release's `zenmon.json` lists the SHA-256 of each asset:

```bash
sha256sum zenmon-0.3.0-x86_64-unknown-linux-gnu.tar.gz   # macOS: shasum -a 256
```

### Keeping it updated

One release carries both apps at one version, and either side updates both:

```bash
zenmon update check     # compare versions; downloads nothing
zenmon update apply     # download, verify, swap — CLI and an installed tray
```

On Windows the tray offers the same from **Settings → Updates**: it updates
itself with a signed installer (a running capture is stopped cleanly and
resumed after the restart) and refreshes its bundled CLI in the same step.
Details in [Self-update](#self-update-update) and
[`tray/README.md`](tray/README.md).

### Build from source

```bash
cargo install --path crates/zenmon-cli
```

Or:

```bash
cargo build --release
# Binary at ./target/release/zenmon
```

Requires a Rust toolchain (1.75+).

Both commands build the CLI and TUI only. The tray app has its own build —
see [Desktop tray app](#desktop-tray-app); do **not** build it with bare `cargo`.

### AI skill (optional)

`cargo install` installs only the binary. To let an AI agent (Claude Code) drive
zenmon for read-only Zenoh diagnostics, copy the bundled skill into your
Claude skills directory:

```bash
mkdir -p ~/.claude/skills/zenmon
cp skills/zenmon/SKILL.md ~/.claude/skills/zenmon/
```

The skill triggers on Zenoh debugging requests and is read-only by default
(`pub`/`replay`/`queryable` require explicit confirmation).

### Build features

The TUI's copy-to-clipboard action uses [`arboard`], whose Linux backend needs
X11/Wayland. The dashboard itself runs anywhere a terminal does, so clipboard is
split into its own feature to support display-less targets:

| Build | Dashboard TUI | Clipboard copy | Target |
|-------|:---:|:---:|--------|
| `cargo build --release` | ✅ | ✅ | macOS / Linux desktop |
| `cargo build --release --no-default-features --features tui` | ✅ | ❌ | Android / Termux |
| `cargo build --release --no-default-features` | ❌ | ❌ | Headless CLI only |

[`arboard`]: https://crates.io/crates/arboard

### Android (Termux)

zenmon runs on an Android phone/tablet under [Termux] (a big-screen tablet gets
the full dashboard). The phone joins the Zenoh network over Wi-Fi and observes
it directly — no cloud or tunnel involved.

```bash
pkg install rust git
git clone <repo-url> && cd zenmon
# Full dashboard, minus the copy action (no X11 on Termux):
cargo build --release --no-default-features --features tui
./target/release/zenmon tui
```

Notes:
- Building `zenoh` on-device is heavy. On low-RAM devices, cap parallelism to
  avoid the linker OOM-ing: `CARGO_BUILD_JOBS=2 cargo build ...`.
- Use the **F-Droid** build of Termux (the Play Store one is unmaintained).
- To skip the slow on-device compile, cross-compile from a workstation with
  [`cargo-ndk`] for the `aarch64-linux-android` target and copy over the binary.

[Termux]: https://termux.dev
[`cargo-ndk`]: https://github.com/bbqsrc/cargo-ndk

## Quick start

```bash
zenohd                                    # a Zenoh router to talk to (brew install zenoh)
zenmon tui                                # interactive dashboard
zenmon sub "demo/**"                      # stream matching traffic
zenmon pub demo/hello '{"msg":"world"}'   # publish from another terminal
```

The full command surface is under [CLI Usage](#cli-usage); TUI keys are under
[TUI Dashboard](#tui-dashboard).

## CLI Usage

```bash
# Subscribe to topics (real-time stream)
zenmon sub "sensor/**" --pretty --timestamp

# Publish a message
zenmon pub test/hello '{"msg":"world"}'

# Publish with attachment metadata
zenmon pub task/goal '{"action":"move","x":5}' --att '{"request_id":"001","client_id":"zenmon"}'

# Payload arguments accept a literal, @<path> (read from file), or - (stdin)
zenmon pub test/hello @payload.json

# Serve a fixed reply to GET queries (same payload syntax; binary files OK)
zenmon queryable serve "call/params/get" --reply @reply.json

# List discovered nodes
zenmon nodes

# Query (Zenoh GET — requires queryable responder)
zenmon query "@/*/router"

# See every reply when multiple queryables share a key
# (default consolidation keeps only one reply per key)
zenmon query "call/system/params/get" --consolidation none

# Bounded stream/watch (safe for agent tool calls)
zenmon --json sub "sensor/**" --count 10        # stop after 10 messages
zenmon --json sub "sensor/**" --duration 5s     # stop after 5s
zenmon --json nodes --watch --count 1           # one snapshot then exit

# Test how two key expressions relate (pure, no network)
zenmon --json keyexpr "a/*" "a/b"

# JSON output (pipe to jq, etc.)
zenmon --json nodes
zenmon --json sub "sensor/**"

# Publish repeatedly at a fixed rate (bounded — safe for agent tool calls)
zenmon pub cmd/drive '{"v":0.3}' --rate 10 --duration 5s

# Record a correlated diagnostic session → one episode JSON to reason over
zenmon --json scenario --pub cmd/drive '{"v":0.3}' --pub-rate 10 --pub-for 5s \
  --observe state/pose --track state/pose:x --for 6s

# Continuously record a rotating store (run under a supervisor for always-on)
zenmon capture "sensor/**" --dir ./trace --rotate-size 64MB --rotate-interval 1h \
  --max-total-size 1GB --max-age 7d

# Read the store WITHOUT a live subscription (pure, offline) — inspect the past
zenmon --json trace stats ./trace --since 1h --top 20        # per-topic rollup
zenmon --json trace read  ./trace --key "sensor/**" --since 10m --limit 100
zenmon --json trace read  ./trace --last-per-key             # latest value per topic

# Consume a contract (topic types, schemas, encodings)
zenmon contract lint mynet.contract.yaml
zenmon -n myfleet --contract mynet.contract.yaml sub "topic/**"

# Validate and inspect the merged configuration without connecting
zenmon config validate
zenmon config show --effective
zenmon --json config show --effective

# Manage where releases are fetched from (no network access; edits a local file)
zenmon remote list
zenmon remote add gh --github gongfour/zenmon
zenmon remote add usb --path E:/zenmon_releases --default
zenmon remote default gh
zenmon remote remove usb

# Update zenmon itself
zenmon update check                     # compare versions; downloads nothing
zenmon update apply                     # download, verify, replace in place
zenmon update apply --remote usb        # from a specific remote
zenmon update apply --reinstall         # same version again, or roll back
```

### Self-update (`update`)

`zenmon update apply` downloads the release archive for your platform, checks
it against the SHA-256 in `zenmon.json`, runs the new binary once to confirm it
reports the version the manifest promised, and only then swaps it in. When the
command returns the new binary is already installed — there is no background
helper and nothing to wait for.

The replacement is rename-only, so it works while zenmon is running: the old
binary is moved to `zenmon.exe.old-N` and the new one takes its place. Windows
refuses to *delete* a running executable but allows *renaming* it. Old files
still being executed by a running process are left alone and swept by a later
update; `apply` says so when that happens.

`update` refuses to touch a binary in cargo's `bin` directory — cargo owns that
file, and overwriting it would leave `.crates2.json` wrong and let the next
`cargo install` silently revert the update. Install a release binary elsewhere
on your `PATH` and update that one, or keep using `cargo install`.

| Environment variable | Effect |
|---|---|
| `ZENMON_UPDATE_TOKEN` | Bearer token for the GitHub API. Only needed for a private fork, or when the 60-requests-per-hour anonymous limit is exhausted by others sharing your IP. |
| `ZENMON_REMOTES` | Use a different remote registry file. |

Builds made with `--no-default-features` have no updater (the `self-update`
feature is off): releases are published for windows/linux/macos only, so a
source-built Termux install could never have an artifact to update to.

### Release remotes (`remote`)

A *remote* names a place holding release manifests and artifacts — a GitHub
repository, or a directory with the same file layout (local disk, a USB stick,
a UNC share). The registry is a per-user TOML file, kept outside any checkout:

| Platform | Registry |
|---|---|
| Windows | `%APPDATA%\zenmon\remotes.toml` |
| Linux | `~/.config/zenmon/remotes.toml` |
| macOS | `~/Library/Application Support/zenmon/remotes.toml` |

`ZENMON_REMOTES` overrides the path outright, which is how to try things
without touching your real registry.

The first remote added becomes the default. With none configured, zenmon falls
back to the built-in `gongfour/zenmon` — a fresh install can update itself
before anything has been set up. Once you *have* configured remotes, zenmon
never guesses: with no default set it asks rather than picking one.

A remote whose `kind` this build does not recognise (added by a newer zenmon)
is listed and preserved byte-for-byte rather than rejected, so an older binary
cannot destroy a newer one's configuration — it fails only if you ask to use
that remote.

### Global Options

| Option | Description | Default |
|--------|-------------|---------|
| `-e, --endpoint` | Zenoh connection endpoint | `tcp/localhost:7447` (effective default) |
| `-m, --mode` | Connection mode: `peer` or `client` | `client` (effective default) |
| `-n, --namespace` | Zenoh namespace (native prefix isolation) | - |
| `-c, --config` | Path to Zenoh JSON5 config file | - |
| `--connect-timeout` | Connect deadline (e.g. `5s`); client fails if no router in the window | - |
| `--json` | Output in JSON format | - |

### Key expression testing (`keyexpr`)

`zenmon keyexpr <A> <B>` reports how two key expressions relate, with no
network. `a_includes_b` means **A contains every key of B** (A ⊇ B); it is
directional, so order matters:

```bash
$ zenmon --json keyexpr "a/*" "a/b"
{"a":"a/*","b":"a/b","intersects":true,"a_includes_b":true,"b_includes_a":false,"equal":false,"relation":"a_includes_b"}
```

Here `a/*` includes `a/b` (every `a/b` is an `a/*`), but not vice-versa. The
`relation` field summarizes direction as one of `equal`, `a_includes_b`,
`b_includes_a`, `overlaps`, or `disjoint`.

### Reply consolidation (`query --consolidation`)

Zenoh consolidates GET replies per key: with the default strategy only one
reply per key survives, chosen by arrival order. When **several queryables
serve the same key expression** — e.g. an RPC-style `call/**` key that every
service in a fleet answers — this is a debugging trap: the fastest responder
masks all others, so a fast error reply (`"Unknown parameter"`) can hide a
slower success reply even though the operation actually applied.

`--consolidation` selects the strategy:

| Mode | Behavior |
|------|----------|
| `auto` (default) | Zenoh picks; effectively one reply per key |
| `none` | No consolidation — **every** queryable's reply is returned |
| `monotonic` | Forward replies immediately, drop timestamp regressions per key |
| `latest` | Hold back and return only the newest reply per key |

```bash
# Two services answer test/consol. Default shows only the fastest:
$ zenmon --json query test/consol
{"count":1,"items":[{"key_expr":"test/consol","payload":"{svc:a}",...}]}

# --consolidation none shows both:
$ zenmon --json query test/consol --consolidation none
{"count":2,"items":[{"key_expr":"test/consol","payload":"{svc:b}",...},
                    {"key_expr":"test/consol","payload":"{svc:a}",...}]}
```

Use `none` whenever you need to know **who** replied (fan-out RPC keys,
counting responders, spotting a service that answers with an error), and pair
it with `--limit` if the fan-out is large. `sub`-side consolidation is not
affected; this flag only applies to `query`.

### Agent-friendly output contracts

- **Duration options** use unit strings (`--timeout 5s`, `--refresh 100ms`,
  `--duration 500ms`), not bare integers.
- **Finite queries** (`discover`, `query`, `nodes`, `liveliness`, `scout`,
  `info`) emit `{"count":N,"items":[...]}` in `--json` mode. A successful empty
  result is exactly `{"count":0,"items":[]}` and exits `0`.
- **Streaming/watch** (`sub`, `--watch`) emit NDJSON (one object per line, no
  ANSI) in `--json` mode.
- **`pub`** emits `{"ok":true,"status":"accepted","key_expr":...,"bytes":N}`;
  `--rate` adds a `{...,"published":N,"rate_hz":R}` summary after the run.
- **`query`** reply errors returned by a queryable are surfaced under an
  `"errors":[...]` array (present only when non-empty), not silently dropped —
  so an endpoint that exists but rejects a request is distinguishable from one
  that never replied.
- **`query --consolidation none`** disables Zenoh reply consolidation so every
  queryable's reply is returned. The default (`auto`) keeps one reply per key,
  so when several services serve the same key expression the fastest reply
  masks the rest (a fast error can hide a slow success). Also accepts
  `monotonic` and `latest`.
- **Errors** in `--json` mode are a single line on stderr,
  `{"error":{"kind":"...","message":"..."}}`, with a stable non-zero exit code
  per kind (`invalid_input`=2, `connection`=3, `timeout`=4, `not_found`=5,
  `internal`=1).

Options can also be set via environment variables: `ZENMON_ENDPOINT`, `ZENMON_MODE`,
`ZENMON_NAMESPACE`, `ZENMON_CONFIG`, `ZENMON_SCOUT_PORT`, `ZENMON_CONNECT_TIMEOUT`.

Configuration is resolved in this order, with later sources overriding earlier ones:

1. Built-in defaults
2. Zenoh config file (`ZENMON_CONFIG` or `--config`)
3. Environment variables
4. Explicit CLI flags

Use `zenmon config show --effective` to see the resolved value and source for each
zenmon-managed setting. The command prints only an allow-list of settings and never dumps
the raw Zenoh config, so plugin credentials and private keys are not exposed. `zenmon
config validate` performs the same merge and validation without opening a network session.

## Payload decoding

`sub`, `query`, `scenario`, and the TUI decode each payload for display: JSON is
shown as-is, valid UTF-8 as text, and **MessagePack is auto-decoded to JSON** — a
conservative content-based fallback, accepted only when it consumes the whole
buffer and the top level is a map/array, so arbitrary binary still falls back to
base64. The original wire bytes are preserved, so `capture`/`replay` round-trips
stay byte-exact.

## Time-shifted inspection (capture store + trace reader)

Live bounded commands (`sub --count/--duration`, `scenario --for`) answer *"what
is happening now"*; they structurally cannot answer *"what happened while I was
away"* — an agent operates in discrete turns and cannot hold a subscription open
between them. The fix is to decouple collection from reading:

- **Collector** — `capture --dir` is a plain long-lived foreground process
  (supervise it with the OS: Windows Task Scheduler / `nssm` / a terminal). It
  appends records to rotating NDJSON segments and enforces retention, so the
  store stays bounded: rotate at `--rotate-size` (64MB) / `--rotate-interval`
  (1h), prune oldest closed segments over `--max-total-size` (1GB) or older
  than `--max-age` (7d). The active segment is never pruned.
- **Reader** — `trace stats` / `trace read` are pure file readers: no Zenoh
  session, safe to call from any agent turn. `trace read` is bounded by
  `--limit` (default 100) and reports `{returned, matched, truncated, cursor}`
  so truncation is never silent — pass `--cursor` to page through the rest.
  `--last-per-key` collapses to the latest record per topic; `--every N`
  samples large windows.

## Contract-aware monitoring

A **contract** (`*.contract.yaml`) declares the Zenoh protocol a project speaks —
per-topic key expression, messaging pattern, encoding, producers/consumers, and a
payload schema. Zenoh itself is schema-less; the contract is that missing layer.

```bash
zenmon contract lint mynet.contract.yaml      # parse + structural warnings
zenmon contract list mynet.contract.yaml      # key  pattern  encoding, per topic
zenmon contract show topic/navigation/pose    # full entry, $ref expanded
```

With `--contract <path>` (or `ZENMON_CONTRACT`), `sub`/`discover` annotate each
message with its declared type/description, expected-vs-observed encoding, and an
"undeclared topic" warning. Enrichment is additive — with no contract, output is
unchanged.

```bash
zenmon -n myfleet --contract mynet.contract.yaml --json sub "topic/**"
```

> Contract keys are relative to the fleet namespace, so pass `-n <fleet>` — the
> observed keys are then relative and match the contract's keys.

## Scenario — correlated diagnostic sessions

`zenmon scenario` records a correlated, multi-topic session and emits **one episode
JSON** that an AI (or you) can read to reason about cause and effect. It optionally
triggers an actuation or a task first, then observes a bounded window. It correlates;
it does not diagnose.

```bash
# Trigger a sustained actuation and capture the effect, in one command
zenmon --json scenario \
  --pub topic/drive/cmd '{"linear":{"x":0.3,"y":0,"z":0},"angular":{"x":0,"y":0,"z":0}}' \
  --pub-rate 10 --pub-for 8s \
  --observe topic/nav/pose \
  --track topic/nav/pose:x \
  --for 9s

# Trigger a long-running task from a file; keep the episode small
zenmon -n myfleet --contract mynet.contract.yaml --json scenario \
  --task task/nav/route @mission.json \
  --preset stall --track 'topic/safety/policy/*:level' \
  --for 15s --settle 1s --no-timeline

# Preview the resolved plan without running (dry run)
zenmon scenario --preset stall --prefix myfleet --for 15s --explain
```

- **Trigger** — `--pub KEY VALUE` (one-shot, or sustained with `--pub-rate` +
  `--pub-for`/`--pub-count`), or `--task PREFIX REQUEST_JSON` (publishes to
  `PREFIX/request`, auto-observes `PREFIX/feedback` + `PREFIX/response`, ends on
  the response). A large `VALUE`/`REQUEST_JSON` may be `@<file>` or `-` (stdin).
  With a contract, `--task` prints and validates the request schema (missing/
  unknown fields, and `A|B|C` enum values).
- **Observe** — `--observe KEY` (repeatable), or `--preset stall` (a built-in
  mission-diagnosis set: safety state/policies, obstacles, mission state, pose,
  robot state, behavior tree, task feedback/response).
- **Track** — `--track KEY:FIELD` extracts a payload field over time: `series`,
  `delta` (numeric), and `transitions` (for discrete fields). A wildcard `KEY`
  (e.g. `topic/safety/policy/*:level`) expands to one track per matching concrete
  key.
- **Episode** — `{ meta, topics, correlations, timeline, tracks }`. Each `topics`
  entry carries `count`, `first`/`last_t_rel_ms`, `rate_hz`, and `latest` (the last
  decoded payload); `correlations` groups events by attachment `correlation_id`.
  Always bounded by `--for`/`--settle`, so it terminates (agent-safe).
- **Size / preview** — `--no-timeline` drops the per-event timeline (keeps the
  summaries; much smaller for long/high-rate sessions); `--explain` prints the
  resolved plan and exits without touching the network.

## TUI Dashboard

```bash
zenmon tui
```

Interactive terminal dashboard with 5 views:

| Key | View | Description |
|-----|------|-------------|
| `1` | Dashboard | Connection status, recent messages, node summary |
| `2` | Topics | Topic list + real-time latest value detail panel |
| `3` | Subscribe | Live message stream with pause/resume |
| `4` | Query | Interactive Zenoh GET with status feedback |
| `5` | Nodes | Discovered Zenoh nodes table |

### Key Bindings

| Key | Action |
|-----|--------|
| `1`-`5` | Switch views |
| `q` | Quit |
| `Esc` | Back to Dashboard |
| `j`/`k` | Navigate lists |
| `/` | Filter (Topics) / Edit query (Query) |
| `i` | Enter query input (Query view) |
| `Space` | Pause/resume (Subscribe view) |
| `Shift+J`/`Shift+K` | Scroll detail panel (Topics view) |
| `Enter` | Subscribe to selected topic (Topics view) |

### Features

- **Graceful connection** — TUI starts even without zenohd, auto-reconnects every 5s
- **Real-time topic monitoring** — Topics view shows latest value updating in place with age indicator
- **Attachment display** — Zenoh attachments shown in magenta across all views
- **Non-blocking** — Reconnection and queries run in background, UI stays responsive

## Desktop tray app

`zenmon-tray` records Zenoh traffic from the system tray — the same rotating
segment store `zenmon capture --dir` writes and `zenmon trace` reads — so a
session can be captured for post-incident debugging without keeping a terminal
open. Left click toggles capture, right click opens the menu.

To *install* it, use the release installer — see [Install](#install). To build
it from source:

```bash
cd tray
npm install                            # first time only
npm run tauri build -- --no-bundle     # → ../target/release/zenmon-tray.exe
```

Requires Node 20+. **Build it with the Tauri CLI, never with bare `cargo`** —
`cargo build -p zenmon-tray` writes a *dev* binary to the same path, which shows
a blank page when run standalone. Full notes, dev workflow and layout in
[`tray/README.md`](tray/README.md).

Updates flow **both ways through one release** (single version, single tag):
`zenmon update apply` updates an installed tray alongside the CLI on every
platform, and on Windows the tray returns the favor — the release installer
bundles the `zenmon` CLI, and *Settings → Updates* updates the tray in place
(capture is stopped cleanly and resumed after the restart), refreshing the
bundled CLI in the same step. The same section puts the bundled CLI on your
`PATH` ("Install CLI") and drives `zenmon update apply` for a CLI installed
elsewhere. Whichever app you drive, both end up current.

## Architecture

Cargo workspace with 3 crates plus the tray app:

```
crates/
  zenmon-core/    # Zenoh session, subscribe, query, registry (library)
  zenmon-cli/     # clap subcommands, produces `zenmon` binary
  zenmon-tui/     # ratatui views and event loop (library)
tray/             # Tauri app: React frontend + `zenmon-tray` Rust backend
```

`zenmon-tray` is excluded from the workspace `default-members`, so a bare
`cargo build --release` at the root does not touch it.

### Tech Stack

- [zenoh](https://zenoh.io/) — Pub/sub/query protocol
- [tokio](https://tokio.rs/) — Async runtime
- [ratatui](https://ratatui.rs/) + [crossterm](https://github.com/crossterm-rs/crossterm) — Terminal UI
- [clap](https://clap.rs/) — CLI argument parsing
- [tauri](https://tauri.app/) + [React](https://react.dev/) — Desktop tray app

## Roadmap

### Phase 1 — Network Visibility
1. [ ] `zenmon scout` — discover all Zenoh nodes on the network (ZID, type, locators)
2. [ ] `zenmon info` — show current session info, connected peers/routers, locators
3. [ ] Topic Hz/throughput — display message rate (msgs/sec) per topic in TUI Topics view

### Phase 2 — Message Metadata
4. [ ] Encoding display — show payload encoding (`application/json`, `text/plain`, etc.) in sub/TUI
5. [ ] QoS display — show Priority, Reliability, Congestion control per message (`--qos` flag)
6. [ ] HLC timestamp parsing — human-readable time + source node ID instead of raw HLC

### Phase 3 — Liveliness & Events
7. [ ] Liveliness subscription — real-time node online/offline detection in TUI Nodes view
8. [ ] Transport events — connect/disconnect notifications in TUI
9. [ ] Pub matching — show whether subscribers exist when publishing

### Phase 4 — Debugging Utilities
10. [x] `zenmon keyexpr <A> <B>` — test intersection/inclusion between key expressions
11. [x] `zenmon pub --rate <HZ>` — repeated publish at fixed frequency for testing
12. [ ] `zenmon pub --congestion block|drop` — congestion control mode selection
13. [ ] DELETE message display — color-code PUT vs DELETE, filter by kind

### Phase 5 — Advanced Inspection
14. [ ] Admin space explorer — browse `@/**` for router/plugin/storage state
15. [ ] Storage/history query — fetch historical data from zenoh storage backends
16. [ ] Downsampling display — show rate-limiting configuration from router
17. [ ] Advanced pub/sub miss detection — detect dropped messages via `zenoh-ext`

### Phase 6 — AI-assisted diagnosis
18. [x] MessagePack payload auto-decode — read cross-language binary payloads as JSON
19. [x] Contract consumption — enrich `sub`/`discover`; `contract` inspect subcommand
20. [x] `zenmon scenario` — correlated diagnostic sessions (trigger, observe, track → episode JSON)
21. [ ] Automatic `events` in the episode (safety transitions, stalls) beyond `--track`
22. [ ] Strict contract payload validation (field/type checks against the schema)
23. [x] Rotating capture store + `trace` reader — time-shifted inspection (see what happened while the agent was away)
24. [x] `zenmon-tray` — always-on capture from the system tray, no terminal required

## License

MIT
