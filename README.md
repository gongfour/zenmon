# zenmon

[![CI](https://github.com/gongfour/zenmon/actions/workflows/ci.yml/badge.svg)](https://github.com/gongfour/zenmon/actions/workflows/ci.yml)
[![Latest release](https://img.shields.io/github/v/release/gongfour/zenmon)](https://github.com/gongfour/zenmon/releases/latest)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)

**See what your Zenoh network is doing. Try a message. Find what changed.**

zenmon makes Zenoh easier to explore, test, and debug from your terminal.
Browse live traffic in an interactive dashboard, publish and query from the
command line, or record messages and inspect them later.

One CLI binary. Native Zenoh connectivity. No web server or REST plugin to set up.
Works with your own key expressions and payloads, across sensors, services,
robotics, and other Zenoh applications. No project-specific contract required.

[Get started](#try-it) · [Download](https://github.com/gongfour/zenmon/releases/latest) · [User guide](docs/usage.md) · [Desktop recorder](tray/README.md)

## From “is it connected?” to “what changed?”

- **Explore live traffic.** Collapse key hierarchies, see message rates and
  bandwidth, and inspect JSON, text, binary payloads, and attachments.
- **Spot the change.** Browse per-key message history, highlight payload
  differences, pin a baseline, and plot numeric fields in the terminal.
- **Test both sides.** Publish a message, subscribe to a stream, send a query,
  or serve a fixed query reply without writing a test application.
- **Understand the network.** Inspect sessions, liveliness tokens, router
  declarations, and connection diagnostics.
- **Look back in time.** Record a rotating capture store and read it offline.
  The optional tray app keeps recording without an open terminal.
- **Automate the investigation.** Use JSON/NDJSON output, time-bounded
  subscriptions, and an optional MCP interface for agent workflows.

## Install

Download a prebuilt CLI for **Linux x86_64**, **macOS Apple Silicon**, or
**Windows x64** from the [latest release](https://github.com/gongfour/zenmon/releases/latest).
Extract `zenmon` (`zenmon.exe` on Windows) into a directory on your `PATH`.

The Windows tray installer also bundles the CLI: open **Settings → Updates →
Install CLI** after installation.

Or build from this checkout with a current stable Rust toolchain:

```bash
cargo install --locked --path crates/zenmon-cli
zenmon --version
```

[Platform-specific installation and updates →](docs/usage.md#install)

## Try it

With a Zenoh router running locally on `tcp/localhost:7447`, open the dashboard:

```bash
zenmon tui
```

In another terminal, publish some demo traffic:

```bash
zenmon pub demo/temperature '{"celsius":23.5}' --rate 2 --duration 30s
```

Expand `demo`, select `temperature`, and press `p` to plot its numeric field.
Publish a different value to see the payload change:

```bash
zenmon pub demo/temperature '{"celsius":26.0}'
```

**No router?** Use `zenmon --mode peer tui` and add `--mode peer` before
`pub` in both commands. Peer discovery must be available on your network.

**Connecting elsewhere?** Use `zenmon -e tcp/192.168.1.10:7447 tui`.
Connection flags go before the subcommand; the default mode is `client`.

## A few commands go a long way

```bash
# Receive matching messages for five seconds
zenmon sub "demo/**" --pretty --duration 5s

# Ask a queryable for its current value (requires a responder)
zenmon query demo/temperature

# Check your connection and see connected nodes
zenmon doctor
zenmon nodes

# Record traffic; stop with Ctrl+C
zenmon capture "demo/**" --dir ./trace --max-total-size 1GB --max-age 7d

# Inspect the recording offline
zenmon trace stats ./trace
zenmon trace read ./trace --key "demo/**" --limit 20

# Return machine-readable messages, ending after 10 messages or 5 seconds
zenmon --json sub "demo/**" --count 10 --duration 5s

# Check a wildcard without connecting to a network
zenmon keyexpr "demo/*" "demo/temperature"
```

A subscription sees traffic published while it is listening. A query gets
replies from queryables or storage; it does not automatically retrieve a
publisher's last message. `discover` finds keys from observed traffic, so quiet
keys may not appear.

For a larger demo, `python3 scripts/load.py --keys 100 --duration 30` publishes
bounded sample traffic to a local router. It stops only the processes it starts.

Use `zenmon --help` or `zenmon <command> --help` for all options.

## Your network, in two views

| Traffic | Network |
|---|---|
| Hierarchical key tree with aggregate rates | Connected sessions and discovered nodes |
| Per-key history and payload differences | Application liveliness tokens |
| Numeric field plots and pinned baselines | Dead-token filtering and rescanning |
| Live messages and a query editor | Connection diagnostics |

Use `1` / `2` to switch views, arrow keys to navigate, `/` to filter,
`Tab` to switch views, and `?` for help. Save filters, expanded branches, and
plots as named views through the `:` command palette.

Publishing from the dashboard is opt-in: `zenmon tui --allow-publish`.
[Dashboard controls and behavior →](docs/usage.md#tui-dashboard)

## Go further when you need to

| Need | Next step |
|---|---|
| Record in the background | [Desktop tray app](tray/README.md) |
| Replay captured messages or inspect past traffic | [Capture and trace](docs/usage.md#time-shifted-inspection-capture-store--trace-reader) · `zenmon replay --help` |
| Annotate keys and validate expected payloads | [Optional contracts](docs/usage.md#contract-aware-monitoring) |
| Observe several topics in one diagnostic window | [Scenarios](docs/usage.md#scenario--correlated-diagnostic-sessions) |
| Integrate an AI agent | [MCP server](docs/usage.md#mcp-server-optional) · [Bundled skill](skills/zenmon/SKILL.md) |
| Use a headless machine or Android/Termux | [Build features](docs/usage.md#build-features) |
| Configure endpoints, namespaces, or JSON5 | [Connection options](docs/usage.md#global-options) · `zenmon config --help` |

## Development

```bash
cargo build --locked
cargo test --locked --workspace --exclude zenmon-tray
```

| Component | Purpose |
|---|---|
| `crates/zenmon-core` | Shared Zenoh operations, payloads, capture, and configuration |
| `crates/zenmon-cli` | CLI commands and the `zenmon` binary |
| `crates/zenmon-tui` | Interactive terminal dashboard |
| `tray` | Optional Tauri + React desktop recorder |

Build the tray with the Tauri CLI; see the [tray development guide](tray/README.md).
The default Cargo build includes the CLI and TUI.

## License

[MIT](LICENSE)
