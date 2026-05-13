# Phase 1: Network Visibility Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add network discovery (`zemon scout`), session introspection (`zemon info`), and per-topic message rate (Hz) display to CLI and TUI.

**Architecture:** Two new core modules (`scout.rs`, `info.rs`) with corresponding CLI subcommands. Hz tracking is computed client-side in TUI app state by counting messages per topic per second. All three features are independent — scout doesn't need a session, info reads session metadata, Hz is purely local computation.

**Tech Stack:** zenoh 1.9 (scout API, SessionInfo API), tokio, clap, ratatui

---

## File Structure

| File | Action | Responsibility |
|------|--------|----------------|
| `crates/zemon-core/src/scout.rs` | Create | `zenoh::scout()` wrapper, returns `Vec<ScoutInfo>` |
| `crates/zemon-core/src/info.rs` | Create | Session info extraction (own ZID, routers, peers) |
| `crates/zemon-core/src/types.rs` | Modify | Add `ScoutInfo`, `SessionDetail` types |
| `crates/zemon-core/src/lib.rs` | Modify | Register new modules |
| `crates/zemon-cli/src/cli.rs` | Modify | Add `Scout`, `Info` subcommands |
| `crates/zemon-cli/src/main.rs` | Modify | Add command handlers for scout and info |
| `crates/zemon-tui/src/app.rs` | Modify | Add `topic_hz: HashMap<String, f64>`, `topic_msg_counts` for Hz calculation |
| `crates/zemon-tui/src/views/topics.rs` | Modify | Show Hz next to each topic in left panel |
| `crates/zemon-tui/src/views/dashboard.rs` | Modify | Show total msg/s in overview |

---

### Task 1: Core types — ScoutInfo and SessionDetail

**Files:**
- Modify: `crates/zemon-core/src/types.rs`

- [ ] **Step 1: Add ScoutInfo and SessionDetail types**

Append to `crates/zemon-core/src/types.rs` (after the existing `NodeInfo` struct):

```rust
/// Information about a Zenoh node discovered via scouting.
#[derive(Debug, Clone, Serialize)]
pub struct ScoutInfo {
    pub zid: String,
    pub whatami: String,
    pub locators: Vec<String>,
}

/// Detailed session information.
#[derive(Debug, Clone, Serialize)]
pub struct SessionDetail {
    pub zid: String,
    pub mode: String,
    pub routers: Vec<String>,
    pub peers: Vec<String>,
}
```

- [ ] **Step 2: Verify it compiles**

```bash
cargo check -p zemon-core
```

- [ ] **Step 3: Commit**

```bash
git add crates/zemon-core/src/types.rs
git commit -m "feat(core): add ScoutInfo and SessionDetail types"
```

---

### Task 2: Core — scout module

**Files:**
- Create: `crates/zemon-core/src/scout.rs`
- Modify: `crates/zemon-core/src/lib.rs`

- [ ] **Step 1: Register module in lib.rs**

Add `pub mod scout;` to `crates/zemon-core/src/lib.rs`:

```rust
pub mod config;
pub mod session;
pub mod types;
pub mod discover;
pub mod subscriber;
pub mod query;
pub mod registry;
pub mod scout;
```

- [ ] **Step 2: Write scout module**

Create `crates/zemon-core/src/scout.rs`:

```rust
use crate::config::ZemonConfig;
use crate::types::ScoutInfo;
use color_eyre::eyre::eyre;
use color_eyre::Result;
use std::time::Duration;
use zenoh::config::WhatAmI;

/// Scout the network for Zenoh nodes.
/// This does NOT require a session — it uses multicast scouting directly.
/// Returns after `timeout` duration.
pub async fn scout(config: &ZemonConfig, timeout: Duration) -> Result<Vec<ScoutInfo>> {
    let zenoh_config = config.to_zenoh_config()?;
    let receiver = zenoh::scout(WhatAmI::Router | WhatAmI::Peer | WhatAmI::Client, zenoh_config)
        .await
        .map_err(|e| eyre!(e))?;

    let mut nodes = Vec::new();

    let _ = tokio::time::timeout(timeout, async {
        while let Ok(hello) = receiver.recv_async().await {
            let zid = format!("{}", hello.zid());
            if !nodes.iter().any(|n: &ScoutInfo| n.zid == zid) {
                nodes.push(ScoutInfo {
                    zid,
                    whatami: format!("{}", hello.whatami()),
                    locators: hello.locators().iter().map(|l| format!("{}", l)).collect(),
                });
            }
        }
    })
    .await;

    receiver.stop();
    nodes.sort_by(|a, b| a.zid.cmp(&b.zid));
    Ok(nodes)
}
```

- [ ] **Step 3: Verify it compiles**

```bash
cargo check -p zemon-core
```

- [ ] **Step 4: Commit**

```bash
git add crates/zemon-core/src/scout.rs crates/zemon-core/src/lib.rs
git commit -m "feat(core): add scout module — multicast network discovery"
```

---

### Task 3: Core — info module

**Files:**
- Create: `crates/zemon-core/src/info.rs`
- Modify: `crates/zemon-core/src/lib.rs`

- [ ] **Step 1: Register module in lib.rs**

Add `pub mod info;` to `crates/zemon-core/src/lib.rs` (after `pub mod scout;`):

```rust
pub mod config;
pub mod session;
pub mod types;
pub mod discover;
pub mod subscriber;
pub mod query;
pub mod registry;
pub mod scout;
pub mod info;
```

- [ ] **Step 2: Write info module**

Create `crates/zemon-core/src/info.rs`:

```rust
use crate::types::SessionDetail;
use color_eyre::Result;
use zenoh::Session;

/// Get detailed information about the current session.
pub async fn session_info(session: &Session) -> Result<SessionDetail> {
    let zid = format!("{}", session.info().zid().await);

    let mut routers = Vec::new();
    let mut router_iter = session.info().routers_zid().await;
    while let Some(rid) = router_iter.next() {
        routers.push(format!("{}", rid));
    }

    let mut peers = Vec::new();
    let mut peer_iter = session.info().peers_zid().await;
    while let Some(pid) = peer_iter.next() {
        peers.push(format!("{}", pid));
    }

    let mode = if !routers.is_empty() {
        "client".to_string()
    } else {
        "peer".to_string()
    };

    Ok(SessionDetail {
        zid,
        mode,
        routers,
        peers,
    })
}
```

- [ ] **Step 3: Verify it compiles**

```bash
cargo check -p zemon-core
```

- [ ] **Step 4: Commit**

```bash
git add crates/zemon-core/src/info.rs crates/zemon-core/src/lib.rs
git commit -m "feat(core): add info module — session introspection"
```

---

### Task 4: CLI — scout and info subcommands

**Files:**
- Modify: `crates/zemon-cli/src/cli.rs`
- Modify: `crates/zemon-cli/src/main.rs`

- [ ] **Step 1: Add Scout and Info to Command enum**

In `crates/zemon-cli/src/cli.rs`, add two new variants to `Command` enum (before the `Tui` variant):

```rust
    /// Scout the network for Zenoh nodes (no router needed)
    Scout {
        /// Scouting timeout in seconds
        #[arg(long, default_value = "3")]
        timeout: u64,
    },

    /// Show current session information
    Info,
```

- [ ] **Step 2: Add scout handler to main.rs**

In `crates/zemon-cli/src/main.rs`, add the `Scout` match arm (before `Command::Tui`):

```rust
        Command::Scout { timeout } => {
            let nodes = zemon_core::scout::scout(
                &config,
                Duration::from_secs(timeout),
            )
            .await?;

            if cli.json {
                println!("{}", serde_json::to_string_pretty(&nodes)?);
            } else if nodes.is_empty() {
                println!("No Zenoh nodes found (scouted for {}s)", timeout);
            } else {
                println!("{:<40} {:<10} {}", "ZID", "TYPE", "LOCATORS");
                println!("{}", "-".repeat(70));
                for node in &nodes {
                    println!(
                        "{:<40} {:<10} {}",
                        node.zid,
                        node.whatami,
                        node.locators.join(", ")
                    );
                }
                println!("\n{} node(s) found", nodes.len());
            }
        }
```

- [ ] **Step 3: Add info handler to main.rs**

In `crates/zemon-cli/src/main.rs`, add the `Info` match arm (after `Command::Scout`):

```rust
        Command::Info => {
            let session = zemon_core::session::open_session(&config).await?;
            let detail = zemon_core::info::session_info(&session).await?;

            if cli.json {
                println!("{}", serde_json::to_string_pretty(&detail)?);
            } else {
                println!("Session ZID:  {}", detail.zid);
                println!("Mode:         {}", detail.mode);
                if detail.routers.is_empty() {
                    println!("Routers:      (none)");
                } else {
                    for (i, r) in detail.routers.iter().enumerate() {
                        if i == 0 {
                            println!("Routers:      {}", r);
                        } else {
                            println!("              {}", r);
                        }
                    }
                }
                if detail.peers.is_empty() {
                    println!("Peers:        (none)");
                } else {
                    for (i, p) in detail.peers.iter().enumerate() {
                        if i == 0 {
                            println!("Peers:        {}", p);
                        } else {
                            println!("              {}", p);
                        }
                    }
                }
            }
            session
                .close()
                .await
                .map_err(|e| color_eyre::eyre::eyre!(e))?;
        }
```

- [ ] **Step 4: Update is_tui check if needed**

The `is_tui` check in main.rs uses `matches!(cli.command, Command::Tui { .. })` which still works — no change needed.

- [ ] **Step 5: Verify it compiles**

```bash
cargo check
```

- [ ] **Step 6: Commit**

```bash
git add crates/zemon-cli/
git commit -m "feat(cli): add scout and info subcommands"
```

---

### Task 5: TUI — topic Hz calculation

**Files:**
- Modify: `crates/zemon-tui/src/app.rs`

- [ ] **Step 1: Add Hz tracking fields to App**

In `crates/zemon-tui/src/app.rs`, add these fields to the `App` struct (after `topic_detail_scroll`):

```rust
    pub topic_msg_counts: HashMap<String, u32>,
    pub topic_hz: HashMap<String, f64>,
    pub last_hz_update: Instant,
    pub total_msg_count: u32,
    pub total_hz: f64,
```

- [ ] **Step 2: Initialize new fields in App::new()**

Add to the `Self { ... }` block in `App::new()`:

```rust
            topic_msg_counts: HashMap::new(),
            topic_hz: HashMap::new(),
            last_hz_update: Instant::now(),
            total_msg_count: 0,
            total_hz: 0.0,
```

- [ ] **Step 3: Count messages in handle_zenoh_message()**

In the `handle_zenoh_message` method, add message counting after the `topic_latest.insert()` line:

```rust
        // Count messages for Hz calculation
        *self.topic_msg_counts.entry(msg.key_expr.clone()).or_insert(0) += 1;
        self.total_msg_count += 1;
```

- [ ] **Step 4: Add Hz update method**

Add a new method to `App`:

```rust
    /// Recalculate Hz rates. Call this periodically (e.g. every 1 second).
    pub fn update_hz(&mut self) {
        let elapsed = self.last_hz_update.elapsed().as_secs_f64();
        if elapsed >= 1.0 {
            for (key, count) in self.topic_msg_counts.drain() {
                self.topic_hz.insert(key, count as f64 / elapsed);
            }
            self.total_hz = self.total_msg_count as f64 / elapsed;
            self.total_msg_count = 0;
            self.last_hz_update = Instant::now();
        }
    }
```

- [ ] **Step 5: Call update_hz on Tick events**

Change the `AppEvent::Tick` handler in `handle_event`:

```rust
            AppEvent::Tick => { self.update_hz(); }
```

- [ ] **Step 6: Verify it compiles**

```bash
cargo check -p zemon-tui
```

- [ ] **Step 7: Commit**

```bash
git add crates/zemon-tui/src/app.rs
git commit -m "feat(tui): add per-topic Hz rate calculation"
```

---

### Task 6: TUI Topics view — show Hz

**Files:**
- Modify: `crates/zemon-tui/src/views/topics.rs`

- [ ] **Step 1: Add Hz display to topic list items**

In `crates/zemon-tui/src/views/topics.rs`, replace the topic list item rendering (the `.map(|(i, topic)| { ... })` closure inside the `items` Vec builder) with:

```rust
        .map(|(i, topic)| {
            let is_selected = i == app.topic_selected;
            let has_data = app.topic_latest.contains_key(&topic.key_expr);
            let hz = app.topic_hz.get(&topic.key_expr).copied().unwrap_or(0.0);
            let style = if is_selected {
                Style::default()
                    .fg(Color::Black)
                    .bg(Color::Cyan)
                    .add_modifier(Modifier::BOLD)
            } else if has_data {
                Style::default().fg(Color::White)
            } else {
                Style::default().fg(Color::DarkGray)
            };
            let prefix = if is_selected { ">> " } else { "   " };
            let hz_str = if hz > 0.0 {
                format!(" {:.1} Hz", hz)
            } else {
                String::new()
            };
            ListItem::new(Line::from(vec![
                Span::raw(prefix),
                Span::styled(&topic.key_expr, style),
                Span::styled(hz_str, Style::default().fg(Color::Green)),
            ]))
        })
```

- [ ] **Step 2: Add Hz to detail panel**

In the same file, in the detail panel section, add Hz display after the "Kind:" line. Insert this between the `Kind:` line and the empty line before `Payload:`:

```rust
                Line::from(vec![
                    Span::styled("Rate: ", Style::default().fg(Color::Gray)),
                    Span::styled(
                        format!("{:.1} Hz", app.topic_hz.get(key.as_str()).copied().unwrap_or(0.0)),
                        Style::default().fg(Color::Green),
                    ),
                ]),
```

- [ ] **Step 3: Verify it compiles**

```bash
cargo check -p zemon-tui
```

- [ ] **Step 4: Commit**

```bash
git add crates/zemon-tui/src/views/topics.rs
git commit -m "feat(tui): show Hz rate per topic in Topics view"
```

---

### Task 7: TUI Dashboard — show total msg/s

**Files:**
- Modify: `crates/zemon-tui/src/views/dashboard.rs`

- [ ] **Step 1: Add total Hz to dashboard overview**

In `crates/zemon-tui/src/views/dashboard.rs`, add a third line to `info_text` (after the Topics/Nodes/Messages line):

```rust
        Line::from(vec![
            Span::styled("Throughput: ", Style::default().fg(Color::Gray)),
            Span::styled(
                format!("{:.1} msg/s", app.total_hz),
                Style::default().fg(Color::Green),
            ),
            Span::raw("  "),
            Span::styled("Active topics: ", Style::default().fg(Color::Gray)),
            Span::styled(
                format!("{}", app.topic_hz.values().filter(|&&hz| hz > 0.0).count()),
                Style::default().fg(Color::Cyan),
            ),
        ]),
```

- [ ] **Step 2: Verify it compiles**

```bash
cargo check -p zemon-tui
```

- [ ] **Step 3: Commit**

```bash
git add crates/zemon-tui/src/views/dashboard.rs
git commit -m "feat(tui): show total throughput in Dashboard overview"
```

---

### Task 8: Build & Smoke Test

- [ ] **Step 1: Build release binary**

```bash
cargo build --release
```

- [ ] **Step 2: Test scout**

```bash
# With zenohd running:
./target/release/zemon scout
./target/release/zemon scout --timeout 2
./target/release/zemon --json scout

# Without zenohd (should return empty, not crash):
./target/release/zemon scout --timeout 1
```

- [ ] **Step 3: Test info**

```bash
# With zenohd running:
./target/release/zemon info
./target/release/zemon --json info
```

- [ ] **Step 4: Test Hz in TUI**

Start TUI, publish messages at ~1Hz, verify Topics view shows rate:

```bash
# Terminal 1:
./target/release/zemon tui

# Terminal 2:
for i in $(seq 1 20); do
  ./target/release/zemon pub test/hz '{"i":'$i'}' 2>/dev/null
  sleep 0.5
done
```

Topics view should show `test/hz ~2.0 Hz`. Dashboard should show throughput.

- [ ] **Step 5: Commit any fixes**

```bash
git add -A
git commit -m "fix: resolve build issues from Phase 1 smoke testing"
```

- [ ] **Step 6: Final commit and push**

```bash
git push
```
