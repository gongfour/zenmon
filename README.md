# zemon

Zenoh network monitor and debugger. CLI + TUI tool built with Rust.

Lightweight terminal-based alternative to web dashboards for monitoring Zenoh networks. Uses native Zenoh API directly (not REST), so features like attachments are fully supported.

## Install

```bash
cargo install --path crates/zemon-cli
```

Or build from source:

```bash
cargo build --release
# Binary at ./target/release/zemon
```

Requires a Rust toolchain (1.75+).

## CLI Usage

```bash
# Subscribe to topics (real-time stream)
zemon sub "forklift/**" --pretty --timestamp

# Publish a message
zemon pub test/hello '{"msg":"world"}'

# Publish with attachment metadata
zemon pub task/goal '{"action":"move","x":5}' --att '{"request_id":"001","client_id":"zemon"}'

# List discovered nodes
zemon nodes

# Query (Zenoh GET — requires queryable responder)
zemon query "@/*/router"

# Bounded stream/watch (safe for agent tool calls)
zemon --json sub "sensor/**" --count 10        # stop after 10 messages
zemon --json sub "sensor/**" --duration 5s     # stop after 5s
zemon --json nodes --watch --count 1           # one snapshot then exit

# Test how two key expressions relate (pure, no network)
zemon --json keyexpr "a/*" "a/b"

# JSON output (pipe to jq, etc.)
zemon --json nodes
zemon --json sub "sensor/**"

# Validate and inspect the merged configuration without connecting
zemon config validate
zemon config show --effective
zemon --json config show --effective
```

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

`zemon keyexpr <A> <B>` reports how two key expressions relate, with no
network. `a_includes_b` means **A contains every key of B** (A ⊇ B); it is
directional, so order matters:

```bash
$ zemon --json keyexpr "a/*" "a/b"
{"a":"a/*","b":"a/b","intersects":true,"a_includes_b":true,"b_includes_a":false,"equal":false,"relation":"a_includes_b"}
```

Here `a/*` includes `a/b` (every `a/b` is an `a/*`), but not vice-versa. The
`relation` field summarizes direction as one of `equal`, `a_includes_b`,
`b_includes_a`, `overlaps`, or `disjoint`.

### Agent-friendly output contracts

- **Duration options** use unit strings (`--timeout 5s`, `--refresh 100ms`,
  `--duration 500ms`), not bare integers.
- **Finite queries** (`discover`, `query`, `nodes`, `liveliness`, `scout`,
  `info`) emit `{"count":N,"items":[...]}` in `--json` mode. A successful empty
  result is exactly `{"count":0,"items":[]}` and exits `0`.
- **Streaming/watch** (`sub`, `--watch`) emit NDJSON (one object per line, no
  ANSI) in `--json` mode.
- **`pub`** emits `{"ok":true,"status":"accepted","key_expr":...,"bytes":N}`.
- **Errors** in `--json` mode are a single line on stderr,
  `{"error":{"kind":"...","message":"..."}}`, with a stable non-zero exit code
  per kind (`invalid_input`=2, `connection`=3, `timeout`=4, `not_found`=5,
  `internal`=1).

Options can also be set via environment variables: `ZEMON_ENDPOINT`, `ZEMON_MODE`,
`ZEMON_NAMESPACE`, `ZEMON_CONFIG`, `ZEMON_SCOUT_PORT`, `ZEMON_CONNECT_TIMEOUT`.

Configuration is resolved in this order, with later sources overriding earlier ones:

1. Built-in defaults
2. Zenoh config file (`ZEMON_CONFIG` or `--config`)
3. Environment variables
4. Explicit CLI flags

Use `zemon config show --effective` to see the resolved value and source for each
zemon-managed setting. The command prints only an allow-list of settings and never dumps
the raw Zenoh config, so plugin credentials and private keys are not exposed. `zemon
config validate` performs the same merge and validation without opening a network session.

## TUI Dashboard

```bash
zemon tui
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
  zemon-core/    # Zenoh session, subscribe, query, registry (library)
  zemon-cli/     # clap subcommands, produces `zemon` binary
  zemon-tui/     # ratatui views and event loop (library)
```

### Tech Stack

- [zenoh](https://zenoh.io/) — Pub/sub/query protocol
- [tokio](https://tokio.rs/) — Async runtime
- [ratatui](https://ratatui.rs/) + [crossterm](https://github.com/crossterm-rs/crossterm) — Terminal UI
- [clap](https://clap.rs/) — CLI argument parsing

## Roadmap

### Phase 1 — Network Visibility
1. [ ] `zemon scout` — discover all Zenoh nodes on the network (ZID, type, locators)
2. [ ] `zemon info` — show current session info, connected peers/routers, locators
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
10. [x] `zemon keyexpr <A> <B>` — test intersection/inclusion between key expressions
11. [ ] `zemon pub --rate <HZ>` — repeated publish at fixed frequency for testing
12. [ ] `zemon pub --congestion block|drop` — congestion control mode selection
13. [ ] DELETE message display — color-code PUT vs DELETE, filter by kind

### Phase 5 — Advanced Inspection
14. [ ] Admin space explorer — browse `@/**` for router/plugin/storage state
15. [ ] Storage/history query — fetch historical data from zenoh storage backends
16. [ ] Downsampling display — show rate-limiting configuration from router
17. [ ] Advanced pub/sub miss detection — detect dropped messages via `zenoh-ext`

## License

MIT
