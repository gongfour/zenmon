# dotori-zenoh

Zenoh network monitor and debugger. CLI + TUI tool built with Rust.

Lightweight terminal-based alternative to web dashboards for monitoring Zenoh networks. Uses native Zenoh API directly (not REST), so features like attachments are fully supported.

## Install

```bash
cargo install --path crates/dotori-cli
```

Or build from source:

```bash
cargo build --release
# Binary at ./target/release/dotori
```

Requires a Rust toolchain (1.75+).

## CLI Usage

```bash
# Subscribe to topics (real-time stream)
dotori sub "forklift/**" --pretty --timestamp

# Publish a message
dotori pub test/hello '{"msg":"world"}'

# Publish with attachment metadata
dotori pub task/goal '{"action":"move","x":5}' --att '{"request_id":"001","client_id":"dotori"}'

# List discovered nodes
dotori nodes

# Query (Zenoh GET — requires queryable responder)
dotori query "@/*/router"

# JSON output (pipe to jq, etc.)
dotori --json nodes
dotori --json sub "sensor/**"
```

### Global Options

| Option | Description | Default |
|--------|-------------|---------|
| `-e, --endpoint` | Zenoh connection endpoint | `tcp/localhost:7447` |
| `-m, --mode` | Connection mode: `peer` or `client` | `client` |
| `-n, --namespace` | Zenoh namespace (native prefix isolation) | - |
| `-c, --config` | Path to Zenoh JSON5 config file | - |
| `--json` | Output in JSON format | - |

Options can also be set via environment variables: `DOTORI_ENDPOINT`, `DOTORI_MODE`, `DOTORI_NAMESPACE`, `DOTORI_CONFIG`.

## TUI Dashboard

```bash
dotori tui
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

## Architecture

Cargo workspace with 3 crates:

```
crates/
  dotori-core/    # Zenoh session, subscribe, query, registry (library)
  dotori-cli/     # clap subcommands, produces `dotori` binary
  dotori-tui/     # ratatui views and event loop (library)
```

### Tech Stack

- [zenoh](https://zenoh.io/) — Pub/sub/query protocol
- [tokio](https://tokio.rs/) — Async runtime
- [ratatui](https://ratatui.rs/) + [crossterm](https://github.com/crossterm-rs/crossterm) — Terminal UI
- [clap](https://clap.rs/) — CLI argument parsing

## License

MIT
