# zenmon

Zenoh network monitor and debugger. CLI + TUI tool built with Rust.

Lightweight terminal-based alternative to web dashboards for monitoring Zenoh networks. Uses native Zenoh API directly (not REST), so features like attachments are fully supported.

## Install

```bash
cargo install --path crates/zenmon-cli
```

Or build from source:

```bash
cargo build --release
# Binary at ./target/release/zenmon
```

Requires a Rust toolchain (1.75+).

## CLI Usage

```bash
# Subscribe to topics (real-time stream)
zenmon sub "sensor/**" --pretty --timestamp

# Publish a message
zenmon pub test/hello '{"msg":"world"}'

# Publish with attachment metadata
zenmon pub task/goal '{"action":"move","x":5}' --att '{"request_id":"001","client_id":"zenmon"}'

# List discovered nodes
zenmon nodes

# Query (Zenoh GET — requires queryable responder)
zenmon query "@/*/router"

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

# Consume a contract (topic types, schemas, encodings)
zenmon contract lint mynet.contract.yaml
zenmon -n myfleet --contract mynet.contract.yaml sub "topic/**"

# Validate and inspect the merged configuration without connecting
zenmon config validate
zenmon config show --effective
zenmon --json config show --effective
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

## Architecture

Cargo workspace with 3 crates:

```
crates/
  zenmon-core/    # Zenoh session, subscribe, query, registry (library)
  zenmon-cli/     # clap subcommands, produces `zenmon` binary
  zenmon-tui/     # ratatui views and event loop (library)
```

### Tech Stack

- [zenoh](https://zenoh.io/) — Pub/sub/query protocol
- [tokio](https://tokio.rs/) — Async runtime
- [ratatui](https://ratatui.rs/) + [crossterm](https://github.com/crossterm-rs/crossterm) — Terminal UI
- [clap](https://clap.rs/) — CLI argument parsing

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

## License

MIT
