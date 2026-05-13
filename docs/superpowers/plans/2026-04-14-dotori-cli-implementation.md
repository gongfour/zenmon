# zemon-cli Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build a Rust CLI+TUI tool for Zenoh network monitoring and debugging, with headless CLI subcommands and an interactive ratatui TUI dashboard.

**Architecture:** Cargo workspace with 3 crates — `zemon-core` (library: Zenoh session, discover, subscribe, query, registry), `zemon-cli` (binary: clap subcommands), `zemon-tui` (library: ratatui views). Single `zemon` binary produced by zemon-cli. Async via tokio, events via mpsc channels.

**Tech Stack:** Rust 2021, zenoh 1.7 (unstable feature), tokio 1, clap 4 (derive), ratatui 0.30, crossterm 0.29 (event-stream), serde/serde_json, tracing, color-eyre

**Design Spec:** `docs/superpowers/specs/2026-04-14-zemon-cli-design.md`

---

### Task 1: Workspace Scaffold + Git Init

**Files:**
- Create: `Cargo.toml` (workspace root)
- Create: `crates/zemon-core/Cargo.toml`
- Create: `crates/zemon-core/src/lib.rs`
- Create: `crates/zemon-cli/Cargo.toml`
- Create: `crates/zemon-cli/src/main.rs`
- Create: `crates/zemon-tui/Cargo.toml`
- Create: `crates/zemon-tui/src/lib.rs`
- Create: `.gitignore`

- [ ] **Step 1: Initialize git repo**

```bash
cd /Users/kang/Project/hdx/zemon_cli
git init
```

- [ ] **Step 2: Create .gitignore**

Create `.gitignore`:
```
/target
Cargo.lock
```

- [ ] **Step 3: Create workspace root Cargo.toml**

Create `Cargo.toml`:
```toml
[workspace]
resolver = "2"
members = [
    "crates/zemon-core",
    "crates/zemon-cli",
    "crates/zemon-tui",
]

[workspace.package]
version = "0.1.0"
edition = "2021"
license = "MIT"

[workspace.dependencies]
zemon-core = { path = "crates/zemon-core" }
zemon-tui = { path = "crates/zemon-tui" }
zenoh = { version = "1.7", features = ["unstable"] }
tokio = { version = "1", features = ["rt", "rt-multi-thread", "macros", "time", "sync", "signal"] }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
tracing = "0.1"
tracing-subscriber = { version = "0.3", features = ["env-filter"] }
color-eyre = "0.6"
```

- [ ] **Step 4: Create zemon-core crate**

Create `crates/zemon-core/Cargo.toml`:
```toml
[package]
name = "zemon-core"
version.workspace = true
edition.workspace = true

[dependencies]
zenoh.workspace = true
tokio.workspace = true
serde.workspace = true
serde_json.workspace = true
tracing.workspace = true
color-eyre.workspace = true
```

Create `crates/zemon-core/src/lib.rs`:
```rust
pub mod config;
pub mod session;
pub mod types;
pub mod discover;
pub mod subscriber;
pub mod query;
pub mod registry;
```

- [ ] **Step 5: Create zemon-tui crate**

Create `crates/zemon-tui/Cargo.toml`:
```toml
[package]
name = "zemon-tui"
version.workspace = true
edition.workspace = true

[dependencies]
zemon-core.workspace = true
tokio.workspace = true
serde_json.workspace = true
tracing.workspace = true
color-eyre.workspace = true
ratatui = "0.30"
crossterm = { version = "0.29", features = ["event-stream"] }
futures = "0.3"
```

Create `crates/zemon-tui/src/lib.rs`:
```rust
pub mod app;
pub mod event;
pub mod views;

pub use app::run;
```

- [ ] **Step 6: Create zemon-cli crate**

Create `crates/zemon-cli/Cargo.toml`:
```toml
[package]
name = "zemon-cli"
version.workspace = true
edition.workspace = true

[[bin]]
name = "zemon"
path = "src/main.rs"

[dependencies]
zemon-core.workspace = true
zemon-tui.workspace = true
tokio.workspace = true
serde_json.workspace = true
tracing.workspace = true
tracing-subscriber.workspace = true
color-eyre.workspace = true
clap = { version = "4", features = ["derive"] }
```

Create `crates/zemon-cli/src/main.rs`:
```rust
fn main() {
    println!("zemon - Zenoh network monitor");
}
```

- [ ] **Step 7: Verify workspace builds**

```bash
cd /Users/kang/Project/hdx/zemon_cli
cargo check
```

Expected: compiles with no errors (may have warnings about unused modules).

- [ ] **Step 8: Commit**

```bash
git add -A
git commit -m "feat: scaffold cargo workspace with core, cli, tui crates"
```

---

### Task 2: zemon-core — Config Module

**Files:**
- Create: `crates/zemon-core/src/config.rs`

- [ ] **Step 1: Write config module**

Create `crates/zemon-core/src/config.rs`:
```rust
use std::path::PathBuf;

/// Connection configuration for a Zenoh session.
#[derive(Debug, Clone)]
pub struct ZemonConfig {
    pub endpoint: String,
    pub mode: ConnectMode,
    pub namespace: Option<String>,
    pub config_file: Option<PathBuf>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectMode {
    Peer,
    Client,
}

impl Default for ZemonConfig {
    fn default() -> Self {
        Self {
            endpoint: "tcp/localhost:7447".to_string(),
            mode: ConnectMode::Client,
            namespace: None,
            config_file: None,
        }
    }
}

impl ZemonConfig {
    /// Build a Zenoh Config from ZemonConfig.
    pub fn to_zenoh_config(&self) -> color_eyre::Result<zenoh::Config> {
        let mut config = match &self.config_file {
            Some(path) => zenoh::Config::from_file(path)?,
            None => zenoh::Config::default(),
        };

        let mode_str = match self.mode {
            ConnectMode::Peer => "\"peer\"",
            ConnectMode::Client => "\"client\"",
        };
        config.insert_json5("mode", mode_str)?;

        let endpoint_json = format!("[\"{}\"]", self.endpoint);
        config.insert_json5("connect/endpoints", &endpoint_json)?;

        if let Some(ns) = &self.namespace {
            config.insert_json5("namespace", &format!("\"{}\"", ns))?;
        }

        Ok(config)
    }

    /// Create config from environment variables, falling back to defaults.
    pub fn from_env() -> Self {
        let mut cfg = Self::default();

        if let Ok(endpoint) = std::env::var("ZEMON_ENDPOINT") {
            cfg.endpoint = endpoint;
        }
        if let Ok(mode) = std::env::var("ZEMON_MODE") {
            cfg.mode = match mode.to_lowercase().as_str() {
                "peer" => ConnectMode::Peer,
                _ => ConnectMode::Client,
            };
        }
        if let Ok(ns) = std::env::var("ZEMON_NAMESPACE") {
            cfg.namespace = Some(ns);
        }
        if let Ok(config_path) = std::env::var("ZEMON_CONFIG") {
            cfg.config_file = Some(PathBuf::from(config_path));
        }

        cfg
    }
}
```

- [ ] **Step 2: Verify it compiles**

```bash
cargo check -p zemon-core
```

Expected: success.

- [ ] **Step 3: Commit**

```bash
git add crates/zemon-core/src/config.rs
git commit -m "feat(core): add config module with ZemonConfig and env var support"
```

---

### Task 3: zemon-core — Types Module

**Files:**
- Create: `crates/zemon-core/src/types.rs`

- [ ] **Step 1: Write types module**

Create `crates/zemon-core/src/types.rs`:
```rust
use serde::Serialize;
use std::time::SystemTime;

/// Information about a discovered Zenoh key/topic.
#[derive(Debug, Clone, Serialize)]
pub struct TopicInfo {
    pub key_expr: String,
}

/// A received Zenoh message.
#[derive(Debug, Clone, Serialize)]
pub struct ZenohMessage {
    pub key_expr: String,
    pub payload: MessagePayload,
    pub timestamp: Option<String>,
    pub kind: String,
}

/// Payload of a message — either parsed JSON or raw bytes info.
#[derive(Debug, Clone, Serialize)]
#[serde(untagged)]
pub enum MessagePayload {
    Json(serde_json::Value),
    Raw { bytes_len: usize },
}

impl std::fmt::Display for MessagePayload {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MessagePayload::Json(v) => write!(f, "{}", v),
            MessagePayload::Raw { bytes_len } => write!(f, "<{} bytes>", bytes_len),
        }
    }
}

/// Information about a discovered Zenoh node/session.
#[derive(Debug, Clone, Serialize)]
pub struct NodeInfo {
    pub zid: String,
    pub kind: String,
    pub locators: Vec<String>,
    pub metadata: Option<serde_json::Value>,
    pub last_seen: Option<SystemTime>,
}
```

- [ ] **Step 2: Verify it compiles**

```bash
cargo check -p zemon-core
```

Expected: success.

- [ ] **Step 3: Commit**

```bash
git add crates/zemon-core/src/types.rs
git commit -m "feat(core): add shared types — TopicInfo, ZenohMessage, NodeInfo"
```

---

### Task 4: zemon-core — Session Module

**Files:**
- Create: `crates/zemon-core/src/session.rs`

- [ ] **Step 1: Write session module**

Create `crates/zemon-core/src/session.rs`:
```rust
use crate::config::ZemonConfig;
use color_eyre::Result;
use zenoh::Session;

/// Open a Zenoh session from ZemonConfig.
pub async fn open_session(config: &ZemonConfig) -> Result<Session> {
    let zenoh_config = config.to_zenoh_config()?;
    let session = zenoh::open(zenoh_config).await?;
    tracing::info!(zid = %session.zid(), "Zenoh session opened");
    Ok(session)
}
```

- [ ] **Step 2: Verify it compiles**

```bash
cargo check -p zemon-core
```

Expected: success.

- [ ] **Step 3: Commit**

```bash
git add crates/zemon-core/src/session.rs
git commit -m "feat(core): add session module — open_session helper"
```

---

### Task 5: zemon-core — Discover Module

**Files:**
- Create: `crates/zemon-core/src/discover.rs`

- [ ] **Step 1: Write discover module**

Create `crates/zemon-core/src/discover.rs`:
```rust
use crate::types::TopicInfo;
use color_eyre::Result;
use zenoh::Session;

/// Discover active keys matching the given key expression.
/// Uses Zenoh admin space to list subscribers and publishers.
/// Falls back to a plain GET if admin space returns nothing.
pub async fn discover(session: &Session, key_expr: &str) -> Result<Vec<TopicInfo>> {
    let mut topics = Vec::new();

    // Query admin space for subscriber/publisher info
    let admin_key = format!("@/router/local/**");
    let replies = session.get(&admin_key).await?;

    while let Ok(reply) = replies.recv_async().await {
        if let Ok(sample) = reply.result() {
            let key = sample.key_expr().as_str().to_string();
            let payload_str = sample
                .payload()
                .try_to_string()
                .unwrap_or_else(|e| e.to_string().into());

            // Try to parse the admin response for key expressions
            if let Ok(value) = serde_json::from_str::<serde_json::Value>(&payload_str) {
                // Admin space responses vary — extract what we can
                tracing::debug!(key = %key, "admin response: {}", value);
            }

            topics.push(TopicInfo { key_expr: key });
        }
    }

    // Also try a direct GET on the user-provided key expression
    // to find queryables that respond
    let replies = session
        .get(key_expr)
        .timeout(std::time::Duration::from_secs(2))
        .await?;

    while let Ok(reply) = replies.recv_async().await {
        if let Ok(sample) = reply.result() {
            let key = sample.key_expr().as_str().to_string();
            if !topics.iter().any(|t| t.key_expr == key) {
                topics.push(TopicInfo { key_expr: key });
            }
        }
    }

    // Also use liveliness to discover active tokens
    let replies = session.liveliness().get(key_expr).await?;
    while let Ok(reply) = replies.recv_async().await {
        if let Ok(sample) = reply.result() {
            let key = sample.key_expr().as_str().to_string();
            if !topics.iter().any(|t| t.key_expr == key) {
                topics.push(TopicInfo { key_expr: key });
            }
        }
    }

    topics.sort_by(|a, b| a.key_expr.cmp(&b.key_expr));
    Ok(topics)
}
```

- [ ] **Step 2: Verify it compiles**

```bash
cargo check -p zemon-core
```

Expected: success.

- [ ] **Step 3: Commit**

```bash
git add crates/zemon-core/src/discover.rs
git commit -m "feat(core): add discover module — admin space + liveliness discovery"
```

---

### Task 6: zemon-core — Subscriber Module

**Files:**
- Create: `crates/zemon-core/src/subscriber.rs`

- [ ] **Step 1: Write subscriber module**

Create `crates/zemon-core/src/subscriber.rs`:
```rust
use crate::types::{MessagePayload, ZenohMessage};
use color_eyre::Result;
use tokio::sync::mpsc;
use zenoh::Session;

/// Subscribe to a key expression and send messages to the provided channel.
/// Returns a JoinHandle that runs until the session is closed or an error occurs.
pub async fn subscribe(
    session: &Session,
    key_expr: &str,
    tx: mpsc::UnboundedSender<ZenohMessage>,
) -> Result<tokio::task::JoinHandle<()>> {
    let subscriber = session.declare_subscriber(key_expr).await?;
    tracing::info!(key_expr = %key_expr, "Subscribed");

    let handle = tokio::spawn(async move {
        while let Ok(sample) = subscriber.recv_async().await {
            let key = sample.key_expr().as_str().to_string();
            let kind = format!("{}", sample.kind());

            let payload_bytes = sample.payload().to_bytes();
            let payload = match serde_json::from_slice::<serde_json::Value>(&payload_bytes) {
                Ok(json) => MessagePayload::Json(json),
                Err(_) => MessagePayload::Raw {
                    bytes_len: payload_bytes.len(),
                },
            };

            let timestamp = sample
                .timestamp()
                .map(|ts| ts.to_string());

            let msg = ZenohMessage {
                key_expr: key,
                payload,
                timestamp,
                kind,
            };

            if tx.send(msg).is_err() {
                break; // receiver dropped
            }
        }
    });

    Ok(handle)
}
```

- [ ] **Step 2: Verify it compiles**

```bash
cargo check -p zemon-core
```

Expected: success.

- [ ] **Step 3: Commit**

```bash
git add crates/zemon-core/src/subscriber.rs
git commit -m "feat(core): add subscriber module — async subscription with mpsc channel"
```

---

### Task 7: zemon-core — Query Module

**Files:**
- Create: `crates/zemon-core/src/query.rs`

- [ ] **Step 1: Write query module**

Create `crates/zemon-core/src/query.rs`:
```rust
use crate::types::{MessagePayload, ZenohMessage};
use color_eyre::Result;
use std::time::Duration;
use zenoh::Session;

/// Send a Zenoh GET query and collect all replies.
pub async fn get(
    session: &Session,
    key_expr: &str,
    payload: Option<&str>,
    timeout: Duration,
) -> Result<Vec<ZenohMessage>> {
    let mut builder = session.get(key_expr).timeout(timeout);

    if let Some(p) = payload {
        builder = builder.payload(p.to_string());
    }

    let replies = builder.await?;
    let mut results = Vec::new();

    while let Ok(reply) = replies.recv_async().await {
        match reply.result() {
            Ok(sample) => {
                let key = sample.key_expr().as_str().to_string();
                let kind = format!("{}", sample.kind());

                let payload_bytes = sample.payload().to_bytes();
                let msg_payload =
                    match serde_json::from_slice::<serde_json::Value>(&payload_bytes) {
                        Ok(json) => MessagePayload::Json(json),
                        Err(_) => MessagePayload::Raw {
                            bytes_len: payload_bytes.len(),
                        },
                    };

                let timestamp = sample.timestamp().map(|ts| ts.to_string());

                results.push(ZenohMessage {
                    key_expr: key,
                    payload: msg_payload,
                    timestamp,
                    kind,
                });
            }
            Err(err) => {
                let payload_str = err
                    .payload()
                    .try_to_string()
                    .unwrap_or_else(|e| e.to_string().into());
                tracing::warn!(error = %payload_str, "Query error reply");
            }
        }
    }

    Ok(results)
}
```

- [ ] **Step 2: Verify it compiles**

```bash
cargo check -p zemon-core
```

Expected: success.

- [ ] **Step 3: Commit**

```bash
git add crates/zemon-core/src/query.rs
git commit -m "feat(core): add query module — Zenoh GET with timeout and payload"
```

---

### Task 8: zemon-core — Registry Module

**Files:**
- Create: `crates/zemon-core/src/registry.rs`

- [ ] **Step 1: Write registry module**

Create `crates/zemon-core/src/registry.rs`:
```rust
use crate::types::NodeInfo;
use color_eyre::Result;
use zenoh::Session;

/// Discover Zenoh nodes by querying the admin space.
pub async fn list_nodes(session: &Session) -> Result<Vec<NodeInfo>> {
    let mut nodes = Vec::new();

    // Query admin space for sessions
    let replies = session.get("@/**").await?;

    while let Ok(reply) = replies.recv_async().await {
        if let Ok(sample) = reply.result() {
            let key = sample.key_expr().as_str().to_string();
            let payload_str = sample
                .payload()
                .try_to_string()
                .unwrap_or_else(|e| e.to_string().into());

            // Admin keys look like: @/<zid>/router or @/<zid>/peer or @/<zid>/client
            let parts: Vec<&str> = key.split('/').collect();
            if parts.len() >= 3 {
                let zid = parts[1].to_string();
                let kind = parts[2].to_string();

                // Parse metadata from payload (JSON)
                let metadata = serde_json::from_str::<serde_json::Value>(&payload_str).ok();

                let locators = metadata
                    .as_ref()
                    .and_then(|m| m.get("locators"))
                    .and_then(|l| l.as_array())
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|v| v.as_str().map(String::from))
                            .collect()
                    })
                    .unwrap_or_default();

                // Avoid duplicate entries for the same zid
                if !nodes.iter().any(|n: &NodeInfo| n.zid == zid) {
                    nodes.push(NodeInfo {
                        zid,
                        kind,
                        locators,
                        metadata,
                        last_seen: Some(std::time::SystemTime::now()),
                    });
                }
            }
        }
    }

    nodes.sort_by(|a, b| a.zid.cmp(&b.zid));
    Ok(nodes)
}
```

- [ ] **Step 2: Verify it compiles**

```bash
cargo check -p zemon-core
```

Expected: success.

- [ ] **Step 3: Commit**

```bash
git add crates/zemon-core/src/registry.rs
git commit -m "feat(core): add registry module — node discovery via admin space"
```

---

### Task 9: zemon-cli — Clap CLI with All Subcommands

**Files:**
- Create: `crates/zemon-cli/src/cli.rs`
- Modify: `crates/zemon-cli/src/main.rs`

- [ ] **Step 1: Write CLI argument definitions**

Create `crates/zemon-cli/src/cli.rs`:
```rust
use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Parser, Debug)]
#[command(name = "zemon", about = "Zenoh network monitor and debugger")]
pub struct Cli {
    /// Zenoh connection endpoint
    #[arg(short, long, default_value = "tcp/localhost:7447")]
    pub endpoint: String,

    /// Connection mode: peer or client
    #[arg(short, long, default_value = "client")]
    pub mode: String,

    /// Zenoh namespace for key expression isolation
    #[arg(short, long)]
    pub namespace: Option<String>,

    /// Path to Zenoh JSON5 config file
    #[arg(short, long)]
    pub config: Option<PathBuf>,

    /// Output in JSON format
    #[arg(long)]
    pub json: bool,

    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand, Debug)]
pub enum Command {
    /// Discover active keys/topics
    Discover {
        /// Key expression to filter (default: "**")
        #[arg(default_value = "**")]
        key_expr: String,
    },

    /// Subscribe to a topic and stream messages
    Sub {
        /// Key expression to subscribe
        key_expr: String,

        /// Pretty-print JSON output
        #[arg(long)]
        pretty: bool,

        /// Show timestamps
        #[arg(long)]
        timestamp: bool,
    },

    /// Send a Zenoh GET query
    Query {
        /// Key expression to query
        key_expr: String,

        /// JSON payload to include in query
        #[arg(long)]
        payload: Option<String>,

        /// Query timeout in milliseconds
        #[arg(long, default_value = "5000")]
        timeout: u64,
    },

    /// List discovered Zenoh nodes
    Nodes {
        /// Watch for changes (live update)
        #[arg(long)]
        watch: bool,
    },

    /// Launch interactive TUI dashboard
    Tui {
        /// UI refresh interval in milliseconds
        #[arg(long, default_value = "100")]
        refresh: u64,
    },
}
```

- [ ] **Step 2: Write main.rs with command dispatch**

Replace `crates/zemon-cli/src/main.rs`:
```rust
mod cli;

use clap::Parser;
use cli::{Cli, Command};
use color_eyre::Result;
use zemon_core::config::{ConnectMode, ZemonConfig};
use std::path::PathBuf;
use std::time::Duration;

fn build_config(cli: &Cli) -> ZemonConfig {
    let mut cfg = ZemonConfig::from_env();

    // CLI flags override env
    cfg.endpoint = cli.endpoint.clone();
    cfg.mode = match cli.mode.as_str() {
        "peer" => ConnectMode::Peer,
        _ => ConnectMode::Client,
    };
    if cli.namespace.is_some() {
        cfg.namespace = cli.namespace.clone();
    }
    if cli.config.is_some() {
        cfg.config_file = cli.config.clone();
    }

    cfg
}

#[tokio::main]
async fn main() -> Result<()> {
    color_eyre::install()?;
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "zemon=info,zenoh=warn".into()),
        )
        .init();

    let cli = Cli::parse();
    let config = build_config(&cli);

    match cli.command {
        Command::Discover { key_expr } => {
            let session = zemon_core::session::open_session(&config).await?;
            let topics = zemon_core::discover::discover(&session, &key_expr).await?;

            if cli.json {
                println!("{}", serde_json::to_string_pretty(&topics)?);
            } else {
                if topics.is_empty() {
                    println!("No active keys found for '{}'", key_expr);
                } else {
                    for topic in &topics {
                        println!("{}", topic.key_expr);
                    }
                    println!("\n{} key(s) found", topics.len());
                }
            }
            session.close().await?;
        }

        Command::Sub {
            key_expr,
            pretty,
            timestamp,
        } => {
            let session = zemon_core::session::open_session(&config).await?;
            let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
            let _handle = zemon_core::subscriber::subscribe(&session, &key_expr, tx).await?;

            eprintln!("Subscribing to '{}' ... (Ctrl+C to stop)", key_expr);

            loop {
                tokio::select! {
                    Some(msg) = rx.recv() => {
                        if cli.json {
                            println!("{}", serde_json::to_string(&msg)?);
                        } else {
                            let ts = if timestamp {
                                msg.timestamp.as_deref().unwrap_or("--")
                            } else {
                                ""
                            };
                            let payload_str = if pretty {
                                match &msg.payload {
                                    zemon_core::types::MessagePayload::Json(v) => {
                                        serde_json::to_string_pretty(v)?
                                    }
                                    other => format!("{}", other),
                                }
                            } else {
                                format!("{}", msg.payload)
                            };

                            if timestamp {
                                println!("[{}] {} | {}", ts, msg.key_expr, payload_str);
                            } else {
                                println!("{} | {}", msg.key_expr, payload_str);
                            }
                        }
                    }
                    _ = tokio::signal::ctrl_c() => {
                        eprintln!("\nStopped.");
                        break;
                    }
                }
            }
            session.close().await?;
        }

        Command::Query {
            key_expr,
            payload,
            timeout,
        } => {
            let session = zemon_core::session::open_session(&config).await?;
            let results = zemon_core::query::get(
                &session,
                &key_expr,
                payload.as_deref(),
                Duration::from_millis(timeout),
            )
            .await?;

            if cli.json {
                println!("{}", serde_json::to_string_pretty(&results)?);
            } else {
                if results.is_empty() {
                    println!("No replies for '{}'", key_expr);
                } else {
                    for msg in &results {
                        println!("{} | {}", msg.key_expr, msg.payload);
                    }
                    println!("\n{} reply(ies)", results.len());
                }
            }
            session.close().await?;
        }

        Command::Nodes { watch } => {
            let session = zemon_core::session::open_session(&config).await?;
            let nodes = zemon_core::registry::list_nodes(&session).await?;

            if cli.json {
                println!("{}", serde_json::to_string_pretty(&nodes)?);
            } else {
                if nodes.is_empty() {
                    println!("No nodes discovered");
                } else {
                    println!("{:<40} {:<10} {}", "ZID", "KIND", "LOCATORS");
                    println!("{}", "-".repeat(70));
                    for node in &nodes {
                        println!(
                            "{:<40} {:<10} {}",
                            node.zid,
                            node.kind,
                            node.locators.join(", ")
                        );
                    }
                    println!("\n{} node(s)", nodes.len());
                }
            }

            if watch {
                eprintln!("Watching for changes... (Ctrl+C to stop)");
                let mut interval = tokio::time::interval(Duration::from_secs(3));
                loop {
                    tokio::select! {
                        _ = interval.tick() => {
                            let updated = zemon_core::registry::list_nodes(&session).await?;
                            // Clear screen and reprint
                            print!("\x1B[2J\x1B[H");
                            println!("{:<40} {:<10} {}", "ZID", "KIND", "LOCATORS");
                            println!("{}", "-".repeat(70));
                            for node in &updated {
                                println!(
                                    "{:<40} {:<10} {}",
                                    node.zid,
                                    node.kind,
                                    node.locators.join(", ")
                                );
                            }
                            println!("\n{} node(s) — refreshing every 3s", updated.len());
                        }
                        _ = tokio::signal::ctrl_c() => {
                            eprintln!("\nStopped.");
                            break;
                        }
                    }
                }
            }
            session.close().await?;
        }

        Command::Tui { refresh } => {
            let session = zemon_core::session::open_session(&config).await?;
            zemon_tui::run(session, refresh).await?;
        }
    }

    Ok(())
}
```

- [ ] **Step 3: Verify it compiles**

```bash
cargo check -p zemon-cli
```

Expected: may fail because `zemon_tui::run` doesn't exist yet. That's fine — we'll fix it in the next task.

- [ ] **Step 4: Create stub zemon_tui::run**

Update `crates/zemon-tui/src/lib.rs`:
```rust
pub mod app;
pub mod event;
pub mod views;

use color_eyre::Result;
use zenoh::Session;

pub async fn run(session: Session, tick_rate_ms: u64) -> Result<()> {
    tracing::info!("TUI not yet implemented");
    Ok(())
}
```

Create stubs for TUI modules:

`crates/zemon-tui/src/app.rs`:
```rust
// App state — implemented in Task 12
```

`crates/zemon-tui/src/event.rs`:
```rust
// Event handler — implemented in Task 11
```

`crates/zemon-tui/src/views/mod.rs`:
```rust
// TUI views — implemented in Tasks 13-17
```

Create directory:
```bash
mkdir -p crates/zemon-tui/src/views
```

- [ ] **Step 5: Verify full workspace compiles**

```bash
cargo check
```

Expected: success.

- [ ] **Step 6: Run the binary to verify CLI parsing**

```bash
cargo run -- --help
```

Expected: help text showing all global options and subcommands.

- [ ] **Step 7: Commit**

```bash
git add crates/zemon-cli/ crates/zemon-tui/
git commit -m "feat(cli): add clap CLI with discover, sub, query, nodes, tui subcommands"
```

---

### Task 10: zemon-tui — Event Handler

**Files:**
- Modify: `crates/zemon-tui/src/event.rs`

- [ ] **Step 1: Write event handler**

Replace `crates/zemon-tui/src/event.rs`:
```rust
use color_eyre::Result;
use crossterm::event::{EventStream, KeyEvent, KeyEventKind};
use zemon_core::types::ZenohMessage;
use futures::{FutureExt, StreamExt};
use tokio::sync::mpsc;

#[derive(Clone, Debug)]
pub enum AppEvent {
    Key(KeyEvent),
    Zenoh(ZenohMessage),
    Tick,
}

pub struct EventHandler {
    rx: mpsc::UnboundedReceiver<AppEvent>,
    _task: tokio::task::JoinHandle<()>,
}

impl EventHandler {
    pub fn new(tick_rate_ms: u64, zenoh_rx: mpsc::UnboundedReceiver<ZenohMessage>) -> Self {
        let (tx, rx) = mpsc::unbounded_channel();

        // Forward Zenoh messages to unified event channel
        let zenoh_tx = tx.clone();
        tokio::spawn(async move {
            let mut zenoh_rx = zenoh_rx;
            while let Some(msg) = zenoh_rx.recv().await {
                if zenoh_tx.send(AppEvent::Zenoh(msg)).is_err() {
                    break;
                }
            }
        });

        let tick_delay = std::time::Duration::from_millis(tick_rate_ms);
        let task = tokio::spawn(async move {
            let mut reader = EventStream::new();
            let mut tick_interval = tokio::time::interval(tick_delay);

            loop {
                let tick = tick_interval.tick();
                let crossterm_event = reader.next().fuse();

                tokio::select! {
                    maybe_event = crossterm_event => {
                        match maybe_event {
                            Some(Ok(evt)) => {
                                if let crossterm::event::Event::Key(key) = evt {
                                    if key.kind == KeyEventKind::Press {
                                        if tx.send(AppEvent::Key(key)).is_err() {
                                            break;
                                        }
                                    }
                                }
                            }
                            Some(Err(_)) => break,
                            None => break,
                        }
                    },
                    _ = tick => {
                        if tx.send(AppEvent::Tick).is_err() {
                            break;
                        }
                    },
                }
            }
        });

        Self { rx, _task: task }
    }

    pub async fn next(&mut self) -> Result<AppEvent> {
        self.rx
            .recv()
            .await
            .ok_or_else(|| color_eyre::eyre::eyre!("Event channel closed"))
    }
}
```

- [ ] **Step 2: Verify it compiles**

```bash
cargo check -p zemon-tui
```

Expected: success.

- [ ] **Step 3: Commit**

```bash
git add crates/zemon-tui/src/event.rs
git commit -m "feat(tui): add async event handler — keyboard, zenoh, tick multiplexing"
```

---

### Task 11: zemon-tui — App Structure + Tab Switching

**Files:**
- Modify: `crates/zemon-tui/src/app.rs`
- Modify: `crates/zemon-tui/src/lib.rs`

- [ ] **Step 1: Write app state and rendering**

Replace `crates/zemon-tui/src/app.rs`:
```rust
use crate::event::AppEvent;
use crate::views;
use crossterm::event::{KeyCode, KeyEvent};
use zemon_core::types::{NodeInfo, TopicInfo, ZenohMessage};
use ratatui::layout::{Constraint, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Line;
use ratatui::widgets::{Block, Borders, Tabs};
use ratatui::Frame;
use std::collections::VecDeque;

const TAB_TITLES: [&str; 5] = ["Dashboard", "Topics", "Subscribe", "Query", "Nodes"];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActiveView {
    Dashboard,
    Topics,
    Subscribe,
    Query,
    Nodes,
}

impl ActiveView {
    pub fn index(&self) -> usize {
        match self {
            ActiveView::Dashboard => 0,
            ActiveView::Topics => 1,
            ActiveView::Subscribe => 2,
            ActiveView::Query => 3,
            ActiveView::Nodes => 4,
        }
    }

    pub fn from_index(i: usize) -> Self {
        match i {
            0 => ActiveView::Dashboard,
            1 => ActiveView::Topics,
            2 => ActiveView::Subscribe,
            3 => ActiveView::Query,
            4 => ActiveView::Nodes,
            _ => ActiveView::Dashboard,
        }
    }
}

pub struct App {
    pub active_view: ActiveView,
    pub should_quit: bool,
    pub connection_info: String,

    // Dashboard state
    pub topics: Vec<TopicInfo>,
    pub nodes: Vec<NodeInfo>,
    pub recent_messages: VecDeque<ZenohMessage>,

    // Subscribe state
    pub sub_messages: VecDeque<ZenohMessage>,
    pub sub_paused: bool,
    pub sub_scroll: u16,

    // Topics state
    pub topic_filter: String,
    pub topic_selected: usize,
    pub topics_filtering: bool,

    // Query state
    pub query_input: String,
    pub query_results: Vec<ZenohMessage>,
    pub query_history: Vec<String>,
    pub query_editing: bool,
    pub pending_query: Option<String>,

    // Nodes state
    pub node_selected: usize,
}

impl App {
    pub fn new(connection_info: String) -> Self {
        Self {
            active_view: ActiveView::Dashboard,
            should_quit: false,
            connection_info,
            topics: Vec::new(),
            nodes: Vec::new(),
            recent_messages: VecDeque::with_capacity(100),
            sub_messages: VecDeque::with_capacity(500),
            sub_paused: false,
            sub_scroll: 0,
            topic_filter: String::new(),
            topic_selected: 0,
            topics_filtering: false,
            query_input: String::new(),
            query_results: Vec::new(),
            query_history: Vec::new(),
            query_editing: false,
            pending_query: None,
        }
    }

    pub fn handle_event(&mut self, event: AppEvent) {
        match event {
            AppEvent::Key(key) => self.handle_key(key),
            AppEvent::Zenoh(msg) => self.handle_zenoh_message(msg),
            AppEvent::Tick => {} // State refresh handled externally
        }
    }

    fn handle_key(&mut self, key: KeyEvent) {
        // Global keys (when not in text input mode)
        if !self.is_text_input_active() {
            match key.code {
                KeyCode::Char('q') => {
                    self.should_quit = true;
                    return;
                }
                KeyCode::Char('1') => self.active_view = ActiveView::Dashboard,
                KeyCode::Char('2') => self.active_view = ActiveView::Topics,
                KeyCode::Char('3') => self.active_view = ActiveView::Subscribe,
                KeyCode::Char('4') => self.active_view = ActiveView::Query,
                KeyCode::Char('5') => self.active_view = ActiveView::Nodes,
                KeyCode::Esc => {
                    self.active_view = ActiveView::Dashboard;
                }
                _ => self.handle_view_key(key),
            }
        } else {
            self.handle_text_input_key(key);
        }
    }

    fn is_text_input_active(&self) -> bool {
        self.topics_filtering || self.query_editing
    }

    fn handle_text_input_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc => {
                self.topics_filtering = false;
                self.query_editing = false;
            }
            KeyCode::Enter => {
                if self.query_editing {
                    self.query_editing = false;
                    if !self.query_input.is_empty() {
                        self.query_history.push(self.query_input.clone());
                        self.pending_query = Some(self.query_input.clone());
                    }
                }
                if self.topics_filtering {
                    self.topics_filtering = false;
                }
            }
            KeyCode::Char(c) => {
                if self.topics_filtering {
                    self.topic_filter.push(c);
                } else if self.query_editing {
                    self.query_input.push(c);
                }
            }
            KeyCode::Backspace => {
                if self.topics_filtering {
                    self.topic_filter.pop();
                } else if self.query_editing {
                    self.query_input.pop();
                }
            }
            _ => {}
        }
    }

    fn handle_view_key(&mut self, key: KeyEvent) {
        match self.active_view {
            ActiveView::Topics => match key.code {
                KeyCode::Char('/') => self.topics_filtering = true,
                KeyCode::Up | KeyCode::Char('k') => {
                    self.topic_selected = self.topic_selected.saturating_sub(1);
                }
                KeyCode::Down | KeyCode::Char('j') => {
                    let max = self.filtered_topics().len().saturating_sub(1);
                    if self.topic_selected < max {
                        self.topic_selected += 1;
                    }
                }
                KeyCode::Enter => {
                    self.active_view = ActiveView::Subscribe;
                }
                _ => {}
            },
            ActiveView::Subscribe => match key.code {
                KeyCode::Char(' ') => self.sub_paused = !self.sub_paused,
                KeyCode::Up | KeyCode::Char('k') => {
                    self.sub_scroll = self.sub_scroll.saturating_add(1);
                }
                KeyCode::Down | KeyCode::Char('j') => {
                    self.sub_scroll = self.sub_scroll.saturating_sub(1);
                }
                _ => {}
            },
            ActiveView::Query => match key.code {
                KeyCode::Char('/') | KeyCode::Char('i') => self.query_editing = true,
                KeyCode::Up | KeyCode::Char('k') => {
                    // Navigate query history
                    if let Some(prev) = self.query_history.last() {
                        self.query_input = prev.clone();
                    }
                }
                _ => {}
            },
            ActiveView::Nodes => match key.code {
                KeyCode::Up | KeyCode::Char('k') => {
                    self.node_selected = self.node_selected.saturating_sub(1);
                }
                KeyCode::Down | KeyCode::Char('j') => {
                    let max = self.nodes.len().saturating_sub(1);
                    if self.node_selected < max {
                        self.node_selected += 1;
                    }
                }
                _ => {}
            },
            _ => {}
        }
    }

    fn handle_zenoh_message(&mut self, msg: ZenohMessage) {
        // Update recent messages for dashboard
        self.recent_messages.push_front(msg.clone());
        if self.recent_messages.len() > 100 {
            self.recent_messages.pop_back();
        }

        // Update subscribe view
        if !self.sub_paused {
            self.sub_messages.push_front(msg);
            if self.sub_messages.len() > 500 {
                self.sub_messages.pop_back();
            }
        }
    }

    pub fn filtered_topics(&self) -> Vec<&TopicInfo> {
        if self.topic_filter.is_empty() {
            self.topics.iter().collect()
        } else {
            self.topics
                .iter()
                .filter(|t| t.key_expr.contains(&self.topic_filter))
                .collect()
        }
    }

    pub fn render(&mut self, frame: &mut Frame) {
        let [tabs_area, content_area, status_area] = Layout::vertical([
            Constraint::Length(3),
            Constraint::Fill(1),
            Constraint::Length(1),
        ])
        .areas(frame.area());

        // Tab bar
        let tabs = Tabs::new(TAB_TITLES.iter().enumerate().map(|(i, t)| {
            format!("[{}] {}", i + 1, t)
        }))
        .block(Block::default().borders(Borders::ALL).title(" zemon "))
        .select(self.active_view.index())
        .style(Style::default().fg(Color::White))
        .highlight_style(
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD | Modifier::UNDERLINED),
        )
        .divider("  ");
        frame.render_widget(tabs, tabs_area);

        // Content
        match self.active_view {
            ActiveView::Dashboard => views::dashboard::render(self, frame, content_area),
            ActiveView::Topics => views::topics::render(self, frame, content_area),
            ActiveView::Subscribe => views::subscribe::render(self, frame, content_area),
            ActiveView::Query => views::query::render(self, frame, content_area),
            ActiveView::Nodes => views::nodes::render(self, frame, content_area),
        }

        // Status bar
        let status = Line::from(format!(
            " {} | {} | q:quit  1-5:switch view  /:filter",
            self.connection_info,
            if self.is_text_input_active() {
                "INPUT MODE (Esc to cancel)"
            } else {
                "NORMAL"
            }
        ))
        .style(Style::default().fg(Color::Black).bg(Color::Cyan));
        frame.render_widget(status, status_area);
    }
}
```

- [ ] **Step 2: Update lib.rs with the run function**

Replace `crates/zemon-tui/src/lib.rs`:
```rust
pub mod app;
pub mod event;
pub mod views;

use app::App;
use color_eyre::Result;
use zemon_core::types::ZenohMessage;
use event::EventHandler;
use tokio::sync::mpsc;
use zenoh::Session;

pub async fn run(session: Session, tick_rate_ms: u64) -> Result<()> {
    // Channel for Zenoh messages → TUI
    let (zenoh_tx, zenoh_rx) = mpsc::unbounded_channel::<ZenohMessage>();

    // Subscribe to everything to populate the TUI
    let _sub_handle = zemon_core::subscriber::subscribe(&session, "**", zenoh_tx.clone()).await?;

    let connection_info = format!("zid:{}", session.zid());
    let mut app = App::new(connection_info);

    // Initial data load
    app.topics = zemon_core::discover::discover(&session, "**").await.unwrap_or_default();
    app.nodes = zemon_core::registry::list_nodes(&session).await.unwrap_or_default();

    let mut terminal = ratatui::init();
    let mut events = EventHandler::new(tick_rate_ms, zenoh_rx);

    let result = run_loop(&mut terminal, &mut app, &mut events, &session).await;

    ratatui::restore();
    session.close().await?;
    result
}

async fn run_loop(
    terminal: &mut ratatui::DefaultTerminal,
    app: &mut App,
    events: &mut EventHandler,
    session: &Session,
) -> Result<()> {
    let mut refresh_interval = tokio::time::interval(std::time::Duration::from_secs(5));

    loop {
        terminal.draw(|frame| app.render(frame))?;

        // Execute pending query if any
        if let Some(key_expr) = app.pending_query.take() {
            match zemon_core::query::get(
                session,
                &key_expr,
                None,
                std::time::Duration::from_secs(5),
            )
            .await
            {
                Ok(results) => app.query_results = results,
                Err(e) => tracing::warn!(error = %e, "Query failed"),
            }
        }

        tokio::select! {
            event = events.next() => {
                app.handle_event(event?);
            }
            _ = refresh_interval.tick() => {
                // Periodically refresh topics and nodes
                if let Ok(topics) = zemon_core::discover::discover(session, "**").await {
                    app.topics = topics;
                }
                if let Ok(nodes) = zemon_core::registry::list_nodes(session).await {
                    app.nodes = nodes;
                }
            }
        }

        if app.should_quit {
            break;
        }
    }
    Ok(())
}
```

- [ ] **Step 3: Verify it compiles**

```bash
cargo check -p zemon-tui
```

Expected: may fail on missing view modules — fixed in next tasks.

- [ ] **Step 4: Commit**

```bash
git add crates/zemon-tui/src/app.rs crates/zemon-tui/src/lib.rs
git commit -m "feat(tui): add app state, tab switching, event handling, and run loop"
```

---

### Task 12: zemon-tui — Views Module + Dashboard View

**Files:**
- Modify: `crates/zemon-tui/src/views/mod.rs`
- Create: `crates/zemon-tui/src/views/dashboard.rs`

- [ ] **Step 1: Update views/mod.rs**

Replace `crates/zemon-tui/src/views/mod.rs`:
```rust
pub mod dashboard;
pub mod topics;
pub mod subscribe;
pub mod query;
pub mod nodes;
```

- [ ] **Step 2: Write dashboard view**

Create `crates/zemon-tui/src/views/dashboard.rs`:
```rust
use crate::app::App;
use ratatui::layout::{Constraint, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, Paragraph};
use ratatui::Frame;

pub fn render(app: &App, frame: &mut Frame, area: ratatui::layout::Rect) {
    let [info_area, body_area] = Layout::vertical([
        Constraint::Length(5),
        Constraint::Fill(1),
    ])
    .areas(area);

    let [left_area, right_area] = Layout::horizontal([
        Constraint::Percentage(50),
        Constraint::Percentage(50),
    ])
    .areas(body_area);

    // Connection info panel
    let info_text = vec![
        Line::from(vec![
            Span::styled("Connection: ", Style::default().fg(Color::Gray)),
            Span::styled(&app.connection_info, Style::default().fg(Color::Green)),
        ]),
        Line::from(vec![
            Span::styled("Topics: ", Style::default().fg(Color::Gray)),
            Span::styled(
                format!("{}", app.topics.len()),
                Style::default().fg(Color::Cyan),
            ),
            Span::raw("  "),
            Span::styled("Nodes: ", Style::default().fg(Color::Gray)),
            Span::styled(
                format!("{}", app.nodes.len()),
                Style::default().fg(Color::Cyan),
            ),
            Span::raw("  "),
            Span::styled("Messages: ", Style::default().fg(Color::Gray)),
            Span::styled(
                format!("{}", app.recent_messages.len()),
                Style::default().fg(Color::Yellow),
            ),
        ]),
    ];
    let info = Paragraph::new(info_text)
        .block(Block::default().borders(Borders::ALL).title(" Overview "));
    frame.render_widget(info, info_area);

    // Recent messages
    let msg_items: Vec<ListItem> = app
        .recent_messages
        .iter()
        .take(50)
        .map(|msg| {
            let line = Line::from(vec![
                Span::styled(
                    &msg.key_expr,
                    Style::default()
                        .fg(Color::Cyan)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::raw(" | "),
                Span::styled(
                    format!("{}", msg.payload),
                    Style::default().fg(Color::White),
                ),
            ]);
            ListItem::new(line)
        })
        .collect();
    let msg_list = List::new(msg_items)
        .block(Block::default().borders(Borders::ALL).title(" Recent Messages "));
    frame.render_widget(msg_list, left_area);

    // Nodes summary
    let node_items: Vec<ListItem> = app
        .nodes
        .iter()
        .map(|node| {
            let line = Line::from(vec![
                Span::styled(
                    &node.zid[..node.zid.len().min(16)],
                    Style::default().fg(Color::Yellow),
                ),
                Span::raw(" "),
                Span::styled(&node.kind, Style::default().fg(Color::Green)),
                Span::raw(" "),
                Span::styled(
                    node.locators.join(", "),
                    Style::default().fg(Color::Gray),
                ),
            ]);
            ListItem::new(line)
        })
        .collect();
    let node_list = List::new(node_items)
        .block(Block::default().borders(Borders::ALL).title(" Nodes "));
    frame.render_widget(node_list, right_area);
}
```

- [ ] **Step 3: Commit**

```bash
git add crates/zemon-tui/src/views/
git commit -m "feat(tui): add views module and dashboard view"
```

---

### Task 13: zemon-tui — Topics View

**Files:**
- Create: `crates/zemon-tui/src/views/topics.rs`

- [ ] **Step 1: Write topics view**

Create `crates/zemon-tui/src/views/topics.rs`:
```rust
use crate::app::App;
use ratatui::layout::{Constraint, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, Paragraph};
use ratatui::Frame;

pub fn render(app: &App, frame: &mut Frame, area: ratatui::layout::Rect) {
    let [filter_area, list_area] = Layout::vertical([
        Constraint::Length(3),
        Constraint::Fill(1),
    ])
    .areas(area);

    // Filter bar
    let filter_text = if app.topics_filtering {
        format!("/{}_", app.topic_filter)
    } else if app.topic_filter.is_empty() {
        "Press / to filter".to_string()
    } else {
        format!("Filter: {} (/ to edit)", app.topic_filter)
    };
    let filter_style = if app.topics_filtering {
        Style::default().fg(Color::Yellow)
    } else {
        Style::default().fg(Color::Gray)
    };
    let filter = Paragraph::new(filter_text)
        .style(filter_style)
        .block(Block::default().borders(Borders::ALL).title(" Filter "));
    frame.render_widget(filter, filter_area);

    // Topic list
    let filtered = app.filtered_topics();
    let items: Vec<ListItem> = filtered
        .iter()
        .enumerate()
        .map(|(i, topic)| {
            let style = if i == app.topic_selected {
                Style::default()
                    .fg(Color::Black)
                    .bg(Color::Cyan)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::White)
            };
            let prefix = if i == app.topic_selected { ">> " } else { "   " };
            let line = Line::from(vec![
                Span::raw(prefix),
                Span::styled(&topic.key_expr, style),
            ]);
            ListItem::new(line)
        })
        .collect();

    let list = List::new(items).block(
        Block::default().borders(Borders::ALL).title(format!(
            " Topics ({}) — j/k:navigate  Enter:subscribe ",
            filtered.len()
        )),
    );
    frame.render_widget(list, list_area);
}
```

- [ ] **Step 2: Commit**

```bash
git add crates/zemon-tui/src/views/topics.rs
git commit -m "feat(tui): add topics view with filtering and selection"
```

---

### Task 14: zemon-tui — Subscribe View

**Files:**
- Create: `crates/zemon-tui/src/views/subscribe.rs`

- [ ] **Step 1: Write subscribe view**

Create `crates/zemon-tui/src/views/subscribe.rs`:
```rust
use crate::app::App;
use zemon_core::types::MessagePayload;
use ratatui::layout::{Constraint, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, Paragraph};
use ratatui::Frame;

pub fn render(app: &App, frame: &mut Frame, area: ratatui::layout::Rect) {
    let [status_area, messages_area] = Layout::vertical([
        Constraint::Length(3),
        Constraint::Fill(1),
    ])
    .areas(area);

    // Status bar
    let status_text = if app.sub_paused {
        Line::from(vec![
            Span::styled(" PAUSED ", Style::default().fg(Color::Black).bg(Color::Yellow)),
            Span::raw(format!("  {} messages buffered  ", app.sub_messages.len())),
            Span::styled("Space:resume  j/k:scroll", Style::default().fg(Color::Gray)),
        ])
    } else {
        Line::from(vec![
            Span::styled(" LIVE ", Style::default().fg(Color::Black).bg(Color::Green)),
            Span::raw(format!("  {} messages  ", app.sub_messages.len())),
            Span::styled("Space:pause", Style::default().fg(Color::Gray)),
        ])
    };
    let status = Paragraph::new(status_text)
        .block(Block::default().borders(Borders::ALL).title(" Subscribe "));
    frame.render_widget(status, status_area);

    // Messages
    let scroll = app.sub_scroll as usize;
    let items: Vec<ListItem> = app
        .sub_messages
        .iter()
        .skip(scroll)
        .take(messages_area.height as usize)
        .map(|msg| {
            let payload_str = match &msg.payload {
                MessagePayload::Json(v) => {
                    serde_json::to_string_pretty(v).unwrap_or_else(|_| format!("{}", v))
                }
                other => format!("{}", other),
            };

            let ts = msg.timestamp.as_deref().unwrap_or("");
            let line = Line::from(vec![
                Span::styled(
                    &msg.key_expr,
                    Style::default()
                        .fg(Color::Cyan)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(format!(" [{}]", ts), Style::default().fg(Color::DarkGray)),
                Span::raw(" "),
                Span::styled(payload_str, Style::default().fg(Color::White)),
            ]);
            ListItem::new(line)
        })
        .collect();

    let list = List::new(items)
        .block(Block::default().borders(Borders::ALL).title(" Messages "));
    frame.render_widget(list, messages_area);
}
```

- [ ] **Step 2: Commit**

```bash
git add crates/zemon-tui/src/views/subscribe.rs
git commit -m "feat(tui): add subscribe view with pause/resume and scrollback"
```

---

### Task 15: zemon-tui — Query View

**Files:**
- Create: `crates/zemon-tui/src/views/query.rs`

- [ ] **Step 1: Write query view**

Create `crates/zemon-tui/src/views/query.rs`:
```rust
use crate::app::App;
use zemon_core::types::MessagePayload;
use ratatui::layout::{Constraint, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, Paragraph};
use ratatui::Frame;

pub fn render(app: &App, frame: &mut Frame, area: ratatui::layout::Rect) {
    let [input_area, results_area, history_area] = Layout::vertical([
        Constraint::Length(3),
        Constraint::Fill(1),
        Constraint::Length(6),
    ])
    .areas(area);

    // Query input
    let input_text = if app.query_editing {
        format!("GET > {}_", app.query_input)
    } else if app.query_input.is_empty() {
        "Press / or i to enter key expression".to_string()
    } else {
        format!("GET > {}  (Enter to execute, / to edit)", app.query_input)
    };
    let input_style = if app.query_editing {
        Style::default().fg(Color::Yellow)
    } else {
        Style::default().fg(Color::Gray)
    };
    let input = Paragraph::new(input_text)
        .style(input_style)
        .block(Block::default().borders(Borders::ALL).title(" Query "));
    frame.render_widget(input, input_area);

    // Results
    let result_items: Vec<ListItem> = app
        .query_results
        .iter()
        .map(|msg| {
            let payload_str = match &msg.payload {
                MessagePayload::Json(v) => {
                    serde_json::to_string_pretty(v).unwrap_or_else(|_| format!("{}", v))
                }
                other => format!("{}", other),
            };
            let line = Line::from(vec![
                Span::styled(
                    &msg.key_expr,
                    Style::default()
                        .fg(Color::Cyan)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::raw(" | "),
                Span::styled(payload_str, Style::default().fg(Color::White)),
            ]);
            ListItem::new(line)
        })
        .collect();
    let result_count = result_items.len();
    let results = List::new(result_items).block(
        Block::default()
            .borders(Borders::ALL)
            .title(format!(" Results ({}) ", result_count)),
    );
    frame.render_widget(results, results_area);

    // History
    let history_items: Vec<ListItem> = app
        .query_history
        .iter()
        .rev()
        .take(4)
        .map(|q| ListItem::new(Line::from(Span::styled(q, Style::default().fg(Color::DarkGray)))))
        .collect();
    let history = List::new(history_items)
        .block(Block::default().borders(Borders::ALL).title(" History (k:recall) "));
    frame.render_widget(history, history_area);
}
```

- [ ] **Step 2: Commit**

```bash
git add crates/zemon-tui/src/views/query.rs
git commit -m "feat(tui): add query view with input, results, and history"
```

---

### Task 16: zemon-tui — Nodes View

**Files:**
- Create: `crates/zemon-tui/src/views/nodes.rs`

- [ ] **Step 1: Write nodes view**

Create `crates/zemon-tui/src/views/nodes.rs`:
```rust
use crate::app::App;
use ratatui::layout::Constraint;
use ratatui::style::{Color, Modifier, Style};
use ratatui::widgets::{Block, Borders, Cell, Row, Table};
use ratatui::Frame;

pub fn render(app: &App, frame: &mut Frame, area: ratatui::layout::Rect) {
    let header = Row::new(vec![
        Cell::from("ZID"),
        Cell::from("Kind"),
        Cell::from("Locators"),
    ])
    .style(
        Style::default()
            .fg(Color::Yellow)
            .add_modifier(Modifier::BOLD),
    )
    .bottom_margin(1);

    let rows: Vec<Row> = app
        .nodes
        .iter()
        .enumerate()
        .map(|(i, node)| {
            let style = if i == app.node_selected {
                Style::default()
                    .fg(Color::Black)
                    .bg(Color::Cyan)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            };

            let kind_style = match node.kind.as_str() {
                "router" => Style::default().fg(Color::Green),
                "peer" => Style::default().fg(Color::Blue),
                "client" => Style::default().fg(Color::Gray),
                _ => Style::default(),
            };

            Row::new(vec![
                Cell::from(node.zid.clone()),
                Cell::from(node.kind.clone()).style(kind_style),
                Cell::from(node.locators.join(", ")),
            ])
            .style(style)
        })
        .collect();

    let widths = [
        Constraint::Percentage(40),
        Constraint::Percentage(15),
        Constraint::Percentage(45),
    ];

    let table = Table::new(rows, widths)
        .header(header)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(format!(" Nodes ({}) — j/k:navigate ", app.nodes.len())),
        )
        .row_highlight_style(Style::default().bg(Color::DarkGray));

    frame.render_widget(table, area);
}
```

- [ ] **Step 2: Verify full workspace compiles**

```bash
cargo check
```

Expected: success — all modules now have implementations.

- [ ] **Step 3: Commit**

```bash
git add crates/zemon-tui/src/views/nodes.rs
git commit -m "feat(tui): add nodes view with table display"
```

---

### Task 17: Build & Smoke Test

**Files:** (no new files)

- [ ] **Step 1: Build release binary**

```bash
cargo build --release
```

Expected: compiles successfully. Binary at `target/release/zemon`.

- [ ] **Step 2: Test CLI help**

```bash
./target/release/zemon --help
./target/release/zemon discover --help
./target/release/zemon sub --help
./target/release/zemon query --help
./target/release/zemon nodes --help
./target/release/zemon tui --help
```

Expected: all help texts display correctly with proper descriptions.

- [ ] **Step 3: Test CLI without Zenoh (expect connection error)**

```bash
./target/release/zemon discover 2>&1 || true
```

Expected: error message about Zenoh connection failure (no zenohd running). This confirms the binary runs and attempts to connect.

- [ ] **Step 4: Commit any fixes**

If any compilation or runtime issues were found and fixed, commit them:
```bash
git add -A
git commit -m "fix: resolve build issues from smoke testing"
```

---

### Task 18: Integration Test with Zenoh (Manual)

**Files:** (no new files)

This task requires a running zenohd router. Run these commands manually.

- [ ] **Step 1: Start zenohd (if available)**

```bash
zenohd --cfg "adminspace/enabled:true"
```

- [ ] **Step 2: Test discover**

In another terminal:
```bash
./target/release/zemon discover
./target/release/zemon discover --json
```

- [ ] **Step 3: Test subscribe**

Terminal 1 — subscribe:
```bash
./target/release/zemon sub "test/**" --pretty --timestamp
```

Terminal 2 — publish (using zenoh CLI tool `z_put` or similar):
```bash
# If z_pub is available:
z_put -k "test/hello" -v '{"msg": "world"}'
```

Verify messages appear in the subscriber terminal.

- [ ] **Step 4: Test query**

```bash
./target/release/zemon query "test/**" --timeout 2000
./target/release/zemon query "test/**" --json
```

- [ ] **Step 5: Test nodes**

```bash
./target/release/zemon nodes
./target/release/zemon nodes --json
```

- [ ] **Step 6: Test TUI**

```bash
./target/release/zemon tui
```

Verify:
- Tab bar displays at top with 5 tabs
- Number keys 1-5 switch views
- Dashboard shows connection info, message count, node list
- q exits cleanly

- [ ] **Step 7: Final commit**

```bash
git add -A
git commit -m "chore: complete MVP v0.1 — CLI + TUI for Zenoh monitoring"
```
