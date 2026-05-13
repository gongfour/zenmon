# Nodes Discovery Merge Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make the zemon TUI Nodes view reflect the real Zenoh network by merging router admin-space data (live, 2s polling) with multicast scout results (manual trigger), with per-source tracking and scout-staleness visualization.

**Architecture:** Two independent background tokio tasks in the TUI produce `Vec<NodeInfo>` — one polling `@/*/router` admin space every 2 seconds, one running multicast `scout` on demand. A pure `merge_nodes` function combines both sides in `zemon-core`; results flow into `App` via new `AppEvent` variants and are rendered in a 4-column Nodes view with source badges and stale dimming.

**Tech Stack:** Rust 1.75+, Zenoh 1.7, tokio, ratatui, `bitflags = "2"` (new dep), existing `color_eyre`.

**Spec reference:** `docs/superpowers/specs/2026-04-15-nodes-discovery-merge-design.md` (commits `9375843` + `98e317a`)

**Worktree assumption:** The user will create an isolated git worktree + branch before executing this plan. All paths below are relative to the worktree root, which mirrors `D:\project\hdx\zemon`. If you start on `master`, create a branch first:

```bash
git checkout -b feat/nodes-discovery-merge
```

All commits in this plan land on that branch.

---

## File Structure

**Created:**
- `crates/zemon-core/src/merge.rs` — pure merge function + unit tests

**Modified:**
- `Cargo.toml` — add `bitflags` to `[workspace.dependencies]`
- `crates/zemon-core/Cargo.toml` — depend on workspace `bitflags`
- `crates/zemon-core/src/lib.rs` — register new `merge` module
- `crates/zemon-core/src/types.rs` — add `NodeSources` bitflags, modify `NodeInfo` fields, add `is_scout_stale()` + unit tests
- `crates/zemon-core/src/registry.rs` — rename `list_nodes` → `query_admin_nodes`, implement `@/*/router` JSON parsing with `sessions[]` expansion
- `crates/zemon-core/src/scout.rs` — add `ScoutInfo::to_node_info()` helper
- `crates/zemon-cli/src/main.rs` — update two call sites from `list_nodes` → `query_admin_nodes`
- `crates/zemon-tui/src/event.rs` — add `AdminNodes` / `ScoutStarted` / `ScoutNodes` variants; store `tx` in `EventHandler`; expose `sender()`
- `crates/zemon-tui/src/app.rs` — add `admin_nodes`, `scout_nodes`, `scout_in_progress`, `last_scout_at`, `pending_scout_request`; add event handlers; remove `node_selected` range bugs; wire `s` key binding for Nodes view
- `crates/zemon-tui/src/lib.rs` — remove old `list_nodes` call paths, spawn admin polling task + scout trigger helper, use `events.sender()`
- `crates/zemon-tui/src/views/nodes.rs` — 4-column layout with Source column, footer counts, stale dimming

---

## Task 1: Add `bitflags` workspace dependency

**Files:**
- Modify: `Cargo.toml`
- Modify: `crates/zemon-core/Cargo.toml`

- [ ] **Step 1: Add bitflags to workspace dependencies**

Edit `Cargo.toml`. Add one line inside `[workspace.dependencies]`:

```toml
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
bitflags = "2"
```

- [ ] **Step 2: Add bitflags to zemon-core**

Edit `crates/zemon-core/Cargo.toml`. Add one line:

```toml
[dependencies]
zenoh.workspace = true
tokio.workspace = true
serde.workspace = true
serde_json.workspace = true
tracing.workspace = true
color-eyre.workspace = true
bitflags.workspace = true
```

- [ ] **Step 3: Verify workspace builds**

Run: `cargo check -p zemon-core`
Expected: PASS with no errors. (May emit warnings — ignore.)

- [ ] **Step 4: Commit**

```bash
git add Cargo.toml crates/zemon-core/Cargo.toml
git commit -m "chore(deps): add bitflags to workspace"
```

---

## Task 2: Add `NodeSources` bitflags + modify `NodeInfo`

**Files:**
- Modify: `crates/zemon-core/src/types.rs:64-72`
- Test: same file (module-inline `#[cfg(test)] mod tests`)

- [ ] **Step 1: Replace the existing `NodeInfo` struct and add `NodeSources`**

In `crates/zemon-core/src/types.rs`, replace lines 64-72 (the existing `NodeInfo` block) with:

```rust
bitflags::bitflags! {
    /// Which discovery source produced or last confirmed a node entry.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
    pub struct NodeSources: u8 {
        const ADMIN = 0b01;
        const SCOUT = 0b10;
    }
}

impl serde::Serialize for NodeSources {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_u8(self.bits())
    }
}

/// Information about a discovered Zenoh node/session.
#[derive(Debug, Clone, Serialize)]
pub struct NodeInfo {
    pub zid: String,
    pub kind: String,
    pub locators: Vec<String>,
    pub metadata: Option<serde_json::Value>,
    pub sources: NodeSources,
    pub admin_last_seen: Option<SystemTime>,
    pub scout_last_seen: Option<SystemTime>,
}

impl NodeInfo {
    /// A node is stale only if it is scout-only (no admin confirmation) AND
    /// its last scout observation is older than `threshold`.
    pub fn is_scout_stale(&self, now: SystemTime, threshold: std::time::Duration) -> bool {
        if self.sources.contains(NodeSources::ADMIN) {
            return false;
        }
        self.scout_last_seen
            .and_then(|t| now.duration_since(t).ok())
            .map(|d| d > threshold)
            .unwrap_or(false)
    }
}
```

Note: `bitflags::bitflags!` needs the macro imported — since we added it as a workspace dep and it's in scope via `bitflags::bitflags!` path prefix, no `use` statement is required at the top of the file.

- [ ] **Step 2: Add unit tests below the new code**

Append to the same file `crates/zemon-core/src/types.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, SystemTime};

    fn node_with(sources: NodeSources, scout_last_seen: Option<SystemTime>) -> NodeInfo {
        NodeInfo {
            zid: "z1".into(),
            kind: "peer".into(),
            locators: vec![],
            metadata: None,
            sources,
            admin_last_seen: None,
            scout_last_seen,
        }
    }

    #[test]
    fn stale_false_when_admin_flag_set() {
        let old = SystemTime::now() - Duration::from_secs(600);
        let n = node_with(NodeSources::ADMIN | NodeSources::SCOUT, Some(old));
        assert!(!n.is_scout_stale(SystemTime::now(), Duration::from_secs(30)));
    }

    #[test]
    fn stale_false_when_scout_recent() {
        let recent = SystemTime::now() - Duration::from_secs(5);
        let n = node_with(NodeSources::SCOUT, Some(recent));
        assert!(!n.is_scout_stale(SystemTime::now(), Duration::from_secs(30)));
    }

    #[test]
    fn stale_true_when_scout_exceeds_threshold() {
        let old = SystemTime::now() - Duration::from_secs(120);
        let n = node_with(NodeSources::SCOUT, Some(old));
        assert!(n.is_scout_stale(SystemTime::now(), Duration::from_secs(30)));
    }
}
```

- [ ] **Step 3: Run tests — expected to FAIL to build first**

Run: `cargo test -p zemon-core types::tests`

Expected: the three test functions compile and fail/pass, BUT the rest of the crate fails to compile because `registry.rs` still references `last_seen` on `NodeInfo`. This is expected — Task 3 fixes it. For now, verify the **test module itself parses** by checking the error message mentions `registry.rs`, not `types.rs`.

If errors reference `types.rs`, stop and fix the struct definition before proceeding.

- [ ] **Step 4: Commit (even though crate doesn't build yet)**

```bash
git add crates/zemon-core/src/types.rs
git commit -m "feat(core): NodeSources bitflags + NodeInfo source tracking"
```

This is a WIP commit — the crate is broken and will be fixed in Task 3. Committing here keeps the history bite-sized.

---

## Task 3: Rewrite `query_admin_nodes` in registry.rs

**Files:**
- Modify: `crates/zemon-core/src/registry.rs` (full replacement)

- [ ] **Step 1: Replace the entire contents of `crates/zemon-core/src/registry.rs`**

```rust
use crate::types::{NodeInfo, NodeSources};
use color_eyre::eyre::eyre;
use color_eyre::Result;
use std::collections::HashMap;
use std::time::SystemTime;
use zenoh::Session;

/// Discover Zenoh nodes by querying the admin space.
///
/// Queries `@/*/router` to discover each reachable router, parses its JSON
/// payload, and emits one `NodeInfo` for the router itself plus one per
/// entry in its `sessions[]` array (the router's connected peers/clients).
/// Also queries `@/**` to pick up the local session's admin data so the
/// current peer appears in the list.
///
/// All entries are tagged with `NodeSources::ADMIN` and the current time.
/// Peer/client entries derived from `sessions[]` have empty `locators` —
/// transport link endpoints are intentionally not merged into the Locators
/// column (see design spec Non-Goals).
pub async fn query_admin_nodes(session: &Session) -> Result<Vec<NodeInfo>> {
    let now = SystemTime::now();
    let mut by_zid: HashMap<String, NodeInfo> = HashMap::new();

    // 1) Routers and the peers/clients they see.
    let replies = session.get("@/*/router").await.map_err(|e| eyre!(e))?;
    while let Ok(reply) = replies.recv_async().await {
        if let Ok(sample) = reply.result() {
            let key = sample.key_expr().as_str().to_string();
            let payload_str = sample
                .payload()
                .try_to_string()
                .unwrap_or_else(|e| e.to_string().into());
            let json: serde_json::Value = match serde_json::from_str(&payload_str) {
                Ok(v) => v,
                Err(e) => {
                    tracing::warn!("admin reply {} not JSON: {}", key, e);
                    continue;
                }
            };

            // Router itself.
            let router_zid = json
                .get("zid")
                .and_then(|v| v.as_str())
                .map(String::from)
                .unwrap_or_else(|| {
                    // Fall back to parsing from the key: @/<zid>/router
                    key.split('/').nth(1).unwrap_or("").to_string()
                });

            if !router_zid.is_empty() {
                let router_locators = json
                    .get("locators")
                    .and_then(|v| v.as_array())
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|v| v.as_str().map(String::from))
                            .collect()
                    })
                    .unwrap_or_default();

                by_zid
                    .entry(router_zid.clone())
                    .and_modify(|n| {
                        for loc in &router_locators {
                            if !n.locators.contains(loc) {
                                n.locators.push(loc.clone());
                            }
                        }
                        n.sources |= NodeSources::ADMIN;
                        n.admin_last_seen = Some(now);
                    })
                    .or_insert_with(|| NodeInfo {
                        zid: router_zid.clone(),
                        kind: "router".into(),
                        locators: router_locators.clone(),
                        metadata: Some(json.clone()),
                        sources: NodeSources::ADMIN,
                        admin_last_seen: Some(now),
                        scout_last_seen: None,
                    });
            }

            // Connected peers/clients from sessions[].
            if let Some(sessions) = json.get("sessions").and_then(|v| v.as_array()) {
                for s in sessions {
                    let peer_zid = match s.get("peer").and_then(|v| v.as_str()) {
                        Some(z) => z.to_string(),
                        None => continue,
                    };
                    let whatami = s
                        .get("whatami")
                        .and_then(|v| v.as_str())
                        .unwrap_or("unknown")
                        .to_string();

                    by_zid
                        .entry(peer_zid.clone())
                        .and_modify(|n| {
                            n.sources |= NodeSources::ADMIN;
                            n.admin_last_seen = Some(now);
                            // Keep first-seen kind; do NOT overwrite if already set.
                        })
                        .or_insert_with(|| NodeInfo {
                            zid: peer_zid,
                            kind: whatami,
                            locators: Vec::new(), // spec: no links[].dst here
                            metadata: None,
                            sources: NodeSources::ADMIN,
                            admin_last_seen: Some(now),
                            scout_last_seen: None,
                        });
                }
            }
        }
    }

    // 2) Local session admin data (so the current peer appears).
    if let Ok(replies) = session.get("@/**").await {
        while let Ok(reply) = replies.recv_async().await {
            if let Ok(sample) = reply.result() {
                let key = sample.key_expr().as_str().to_string();
                let parts: Vec<&str> = key.split('/').collect();
                if parts.len() < 3 {
                    continue;
                }
                let zid = parts[1].to_string();
                let kind = parts[2].to_string();
                if by_zid.contains_key(&zid) {
                    // Already covered by a router reply — tag the source only.
                    if let Some(n) = by_zid.get_mut(&zid) {
                        n.sources |= NodeSources::ADMIN;
                        n.admin_last_seen = Some(now);
                    }
                    continue;
                }
                let payload_str = sample
                    .payload()
                    .try_to_string()
                    .unwrap_or_else(|e| e.to_string().into());
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
                by_zid.insert(
                    zid.clone(),
                    NodeInfo {
                        zid,
                        kind,
                        locators,
                        metadata,
                        sources: NodeSources::ADMIN,
                        admin_last_seen: Some(now),
                        scout_last_seen: None,
                    },
                );
            }
        }
    }

    let mut out: Vec<NodeInfo> = by_zid.into_values().collect();
    out.sort_by(|a, b| a.zid.cmp(&b.zid));
    Ok(out)
}
```

- [ ] **Step 2: Verify zemon-core now builds**

Run: `cargo check -p zemon-core`
Expected: PASS (types.rs test block and registry.rs both compile).

- [ ] **Step 3: Run the type unit tests**

Run: `cargo test -p zemon-core types::tests`
Expected: PASS — three tests green.

- [ ] **Step 4: Commit**

```bash
git add crates/zemon-core/src/registry.rs
git commit -m "feat(core): query_admin_nodes parses router sessions[]"
```

---

## Task 4: Update CLI callers of the renamed function

**Files:**
- Modify: `crates/zemon-cli/src/main.rs:167` and `:193`

- [ ] **Step 1: Confirm the call sites**

Run: `grep -n "list_nodes" crates/zemon-cli/src/main.rs`
Expected output:
```
167:            let nodes = zemon_core::registry::list_nodes(&session).await?;
193:                            let updated = zemon_core::registry::list_nodes(&session).await?;
```

- [ ] **Step 2: Rename both call sites**

In `crates/zemon-cli/src/main.rs`, replace both occurrences:

- Line 167: `list_nodes(&session)` → `query_admin_nodes(&session)`
- Line 193: `list_nodes(&session)` → `query_admin_nodes(&session)`

Use find-and-replace for `zemon_core::registry::list_nodes` → `zemon_core::registry::query_admin_nodes`.

- [ ] **Step 3: Build the CLI**

Run: `cargo check -p zemon-cli`
Expected: PASS. If the compile error mentions `NodeInfo::last_seen`, the CLI is reading that field somewhere — search with `grep -n "last_seen" crates/zemon-cli/src/main.rs`. Remove any such references (the CLI nodes printer at lines 170-184 only uses `.zid`, `.kind`, `.locators`, so should be clean).

- [ ] **Step 4: Commit**

```bash
git add crates/zemon-cli/src/main.rs
git commit -m "refactor(cli): rename list_nodes callers to query_admin_nodes"
```

---

## Task 5: Create pure `merge_nodes` function with unit tests

**Files:**
- Create: `crates/zemon-core/src/merge.rs`
- Modify: `crates/zemon-core/src/lib.rs` (register module)

- [ ] **Step 1: Write the file with implementation + tests together**

Create `crates/zemon-core/src/merge.rs`:

```rust
use crate::types::{NodeInfo, NodeSources};
use std::collections::HashMap;

/// Merge admin-derived and scout-derived node lists into a single deduped,
/// sorted list. Admin entries seed the map (so their `kind`/`metadata` win
/// on collision). Scout entries add the `SCOUT` flag and their timestamp;
/// locators are unioned. Output is sorted by `zid`.
pub fn merge_nodes(admin: &[NodeInfo], scout: &[NodeInfo]) -> Vec<NodeInfo> {
    let mut by_zid: HashMap<String, NodeInfo> = HashMap::new();

    for n in admin {
        by_zid.insert(n.zid.clone(), n.clone());
    }
    for s in scout {
        by_zid
            .entry(s.zid.clone())
            .and_modify(|existing| {
                existing.sources |= NodeSources::SCOUT;
                existing.scout_last_seen = s.scout_last_seen;
                for loc in &s.locators {
                    if !existing.locators.contains(loc) {
                        existing.locators.push(loc.clone());
                    }
                }
            })
            .or_insert_with(|| s.clone());
    }

    let mut out: Vec<NodeInfo> = by_zid.into_values().collect();
    out.sort_by(|a, b| a.zid.cmp(&b.zid));
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::SystemTime;

    fn admin_node(zid: &str, kind: &str, locators: &[&str]) -> NodeInfo {
        NodeInfo {
            zid: zid.into(),
            kind: kind.into(),
            locators: locators.iter().map(|s| s.to_string()).collect(),
            metadata: None,
            sources: NodeSources::ADMIN,
            admin_last_seen: Some(SystemTime::now()),
            scout_last_seen: None,
        }
    }

    fn scout_node(zid: &str, kind: &str, locators: &[&str]) -> NodeInfo {
        NodeInfo {
            zid: zid.into(),
            kind: kind.into(),
            locators: locators.iter().map(|s| s.to_string()).collect(),
            metadata: None,
            sources: NodeSources::SCOUT,
            admin_last_seen: None,
            scout_last_seen: Some(SystemTime::now()),
        }
    }

    #[test]
    fn merge_admin_only_passes_through_sorted() {
        let admin = vec![
            admin_node("z2", "peer", &["tcp/1.1.1.1:7447"]),
            admin_node("z1", "router", &["tcp/2.2.2.2:7447"]),
        ];
        let out = merge_nodes(&admin, &[]);
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].zid, "z1");
        assert_eq!(out[1].zid, "z2");
        assert_eq!(out[0].sources, NodeSources::ADMIN);
    }

    #[test]
    fn merge_scout_only_passes_through_sorted() {
        let scout = vec![
            scout_node("z2", "peer", &["tcp/3.3.3.3:7447"]),
            scout_node("z1", "router", &["tcp/4.4.4.4:7447"]),
        ];
        let out = merge_nodes(&[], &scout);
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].zid, "z1");
        assert_eq!(out[0].sources, NodeSources::SCOUT);
    }

    #[test]
    fn merge_overlapping_zid_unions_sources_and_locators() {
        let admin = vec![admin_node("z1", "router", &["tcp/a:7447"])];
        let scout = vec![scout_node("z1", "peer", &["tcp/b:7447"])];
        let out = merge_nodes(&admin, &scout);
        assert_eq!(out.len(), 1);
        let n = &out[0];
        assert_eq!(n.zid, "z1");
        assert_eq!(n.kind, "router"); // admin wins
        assert!(n.sources.contains(NodeSources::ADMIN));
        assert!(n.sources.contains(NodeSources::SCOUT));
        assert!(n.admin_last_seen.is_some());
        assert!(n.scout_last_seen.is_some());
        assert!(n.locators.contains(&"tcp/a:7447".to_string()));
        assert!(n.locators.contains(&"tcp/b:7447".to_string()));
    }

    #[test]
    fn merge_disjoint_zids_produces_sorted_union() {
        let admin = vec![admin_node("z3", "router", &[])];
        let scout = vec![
            scout_node("z1", "peer", &[]),
            scout_node("z2", "peer", &[]),
        ];
        let out = merge_nodes(&admin, &scout);
        assert_eq!(out.len(), 3);
        assert_eq!(out[0].zid, "z1");
        assert_eq!(out[1].zid, "z2");
        assert_eq!(out[2].zid, "z3");
    }
}
```

- [ ] **Step 2: Register the module in `lib.rs`**

Edit `crates/zemon-core/src/lib.rs`. Add `pub mod merge;` after `pub mod registry;`:

```rust
pub mod config;
pub mod session;
pub mod types;
pub mod discover;
pub mod subscriber;
pub mod query;
pub mod registry;
pub mod merge;
pub mod scout;
pub mod info;
```

- [ ] **Step 3: Run the merge tests — expected to PASS**

Run: `cargo test -p zemon-core merge::tests`
Expected: all four tests PASS.

- [ ] **Step 4: Commit**

```bash
git add crates/zemon-core/src/merge.rs crates/zemon-core/src/lib.rs
git commit -m "feat(core): add pure merge_nodes with unit tests"
```

---

## Task 6: Add `ScoutInfo::to_node_info` helper

**Files:**
- Modify: `crates/zemon-core/src/types.rs` (append below existing `ScoutInfo`)

- [ ] **Step 1: Add the conversion method**

In `crates/zemon-core/src/types.rs`, immediately after the `pub struct ScoutInfo { ... }` block, add:

```rust
impl ScoutInfo {
    /// Convert a scout hello into a `NodeInfo` tagged with `SCOUT` source.
    pub fn to_node_info(&self, now: std::time::SystemTime) -> NodeInfo {
        NodeInfo {
            zid: self.zid.clone(),
            kind: self.whatami.clone(),
            locators: self.locators.clone(),
            metadata: None,
            sources: NodeSources::SCOUT,
            admin_last_seen: None,
            scout_last_seen: Some(now),
        }
    }
}
```

- [ ] **Step 2: Build**

Run: `cargo check -p zemon-core`
Expected: PASS.

- [ ] **Step 3: Commit**

```bash
git add crates/zemon-core/src/types.rs
git commit -m "feat(core): ScoutInfo::to_node_info conversion"
```

---

## Task 7: Expose `EventHandler::sender()` and add new `AppEvent` variants

**Files:**
- Modify: `crates/zemon-tui/src/event.rs`

- [ ] **Step 1: Add new variants to `AppEvent`**

Edit `crates/zemon-tui/src/event.rs`. Replace the existing enum at lines 7-12 with:

```rust
#[derive(Clone, Debug)]
pub enum AppEvent {
    Key(KeyEvent),
    Zenoh(ZenohMessage),
    Tick,
    AdminNodes(Vec<zemon_core::types::NodeInfo>),
    ScoutStarted,
    ScoutNodes(Vec<zemon_core::types::NodeInfo>),
}
```

- [ ] **Step 2: Store `tx` in `EventHandler` and expose it**

Replace the `EventHandler` struct (lines 14-17) and its `impl` block (lines 19-76) with the following. The existing spawned tasks inside `new()` keep using clones; the difference is that we assign `tx` to `self.tx` instead of letting it drop.

```rust
pub struct EventHandler {
    tx: mpsc::UnboundedSender<AppEvent>,
    rx: mpsc::UnboundedReceiver<AppEvent>,
    _task: tokio::task::JoinHandle<()>,
}

impl EventHandler {
    pub fn new(tick_rate_ms: u64, zenoh_rx: mpsc::UnboundedReceiver<ZenohMessage>) -> Self {
        let (tx, rx) = mpsc::unbounded_channel();

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
        let key_tx = tx.clone();
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
                                        if key_tx.send(AppEvent::Key(key)).is_err() {
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
                        if key_tx.send(AppEvent::Tick).is_err() {
                            break;
                        }
                    },
                }
            }
        });

        Self { tx, rx, _task: task }
    }

    pub fn sender(&self) -> mpsc::UnboundedSender<AppEvent> {
        self.tx.clone()
    }

    pub async fn next(&mut self) -> Result<AppEvent> {
        self.rx
            .recv()
            .await
            .ok_or_else(|| color_eyre::eyre::eyre!("Event channel closed"))
    }
}
```

- [ ] **Step 2.5: Add `NodeInfo` re-export usage check**

The new variants use `zemon_core::types::NodeInfo`. The file already has `use zemon_core::types::ZenohMessage;` at the top. Change that line to:

```rust
use zemon_core::types::{NodeInfo, ZenohMessage};
```

Then simplify the variants to just `AdminNodes(Vec<NodeInfo>)` and `ScoutNodes(Vec<NodeInfo>)`.

- [ ] **Step 3: Build TUI crate**

Run: `cargo check -p zemon-tui`
Expected: FAIL with errors about missing `AdminNodes`/`ScoutNodes` match arms in `app.rs::handle_event` and/or unused `scout_in_progress` etc. This is expected — later tasks fix `app.rs` and `lib.rs`. The event module itself should compile without errors; if the errors are inside `event.rs`, stop and fix before proceeding.

- [ ] **Step 4: Commit**

```bash
git add crates/zemon-tui/src/event.rs
git commit -m "feat(tui): EventHandler::sender and new node-discovery events"
```

---

## Task 8: Add new state fields + event handlers in `App`

**Files:**
- Modify: `crates/zemon-tui/src/app.rs`

- [ ] **Step 1: Add new `use` for merge + NodeSources + Duration**

In `crates/zemon-tui/src/app.rs` line 4, replace:

```rust
use zemon_core::types::{NodeInfo, TopicInfo, ZenohMessage};
```

with:

```rust
use zemon_core::merge::merge_nodes;
use zemon_core::types::{NodeInfo, NodeSources, TopicInfo, ZenohMessage};
use std::time::{Duration, SystemTime};
```

Note: `std::time::Instant` is already imported at line 11 — keep that line unchanged.

- [ ] **Step 2: Replace the existing `nodes` field with three fields in the `App` struct**

In `crates/zemon-tui/src/app.rs`, change line 59 from:

```rust
    pub nodes: Vec<NodeInfo>,
```

to:

```rust
    pub admin_nodes: Vec<NodeInfo>,
    pub scout_nodes: Vec<NodeInfo>,
    pub nodes: Vec<NodeInfo>, // cached merge of admin_nodes + scout_nodes
```

Then, immediately after line 84 (`pub node_selected: usize,`), add:

```rust
    pub scout_in_progress: bool,
    pub last_scout_at: Option<SystemTime>,
    pub pending_scout_request: bool,
```

- [ ] **Step 3: Initialize the new fields in `App::new`**

In the `App::new` body (around lines 88-118), change line 96 from:

```rust
            nodes: Vec::new(),
```

to:

```rust
            admin_nodes: Vec::new(),
            scout_nodes: Vec::new(),
            nodes: Vec::new(),
```

Then, immediately after line 116 (`node_selected: 0,`), add:

```rust
            scout_in_progress: false,
            last_scout_at: None,
            pending_scout_request: false,
```

- [ ] **Step 4: Add event handlers for the new variants**

Replace the existing `handle_event` method (lines 124-130) with:

```rust
    pub fn handle_event(&mut self, event: AppEvent) {
        match event {
            AppEvent::Key(key) => self.handle_key(key),
            AppEvent::Zenoh(msg) => self.handle_zenoh_message(msg),
            AppEvent::Tick => { self.update_hz(); }
            AppEvent::AdminNodes(nodes) => self.handle_admin_nodes(nodes),
            AppEvent::ScoutStarted => { self.scout_in_progress = true; }
            AppEvent::ScoutNodes(nodes) => self.handle_scout_nodes(nodes),
        }
    }

    fn handle_admin_nodes(&mut self, nodes: Vec<NodeInfo>) {
        self.admin_nodes = nodes;
        self.nodes = merge_nodes(&self.admin_nodes, &self.scout_nodes);
        self.clamp_node_selection();
    }

    fn handle_scout_nodes(&mut self, nodes: Vec<NodeInfo>) {
        self.scout_nodes = nodes;
        self.last_scout_at = Some(SystemTime::now());
        self.scout_in_progress = false;
        self.nodes = merge_nodes(&self.admin_nodes, &self.scout_nodes);
        self.clamp_node_selection();
    }

    fn clamp_node_selection(&mut self) {
        if self.nodes.is_empty() {
            self.node_selected = 0;
        } else if self.node_selected >= self.nodes.len() {
            self.node_selected = self.nodes.len() - 1;
        }
    }
```

- [ ] **Step 5: Wire `s` key in Nodes view**

In `handle_view_key` (around lines 241-252), modify the `ActiveView::Nodes` match arm. Replace:

```rust
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
```

with:

```rust
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
                KeyCode::Char('s') => {
                    if !self.scout_in_progress {
                        self.pending_scout_request = true;
                    }
                }
                _ => {}
            },
```

- [ ] **Step 6: Confirm `s` is not already bound globally**

Run: `grep -n "Char('s')\|Char('S')" crates/zemon-tui/src/app.rs`

Expected: only the new line you just added. If any other match arm claims `'s'` globally (outside `ActiveView::Nodes`), stop and report back — the design says to fall back to `'S'` (shift-s). Today the codebase has no other `'s'` binding.

- [ ] **Step 7: Build — App will still fail because `views/nodes.rs` and `lib.rs` still reference old paths**

Run: `cargo check -p zemon-tui`
Expected: FAIL with errors about `node.last_seen`, stale `list_nodes` calls in `lib.rs`, or missing imports. Those are fixed in Tasks 9 and 10. If the errors reference `app.rs` itself (e.g. unresolved imports in app.rs), stop and fix.

- [ ] **Step 8: Commit**

```bash
git add crates/zemon-tui/src/app.rs
git commit -m "feat(tui): App state + handlers for admin/scout node events"
```

---

## Task 9: Rewire `lib.rs` — spawn admin polling + scout trigger

**Files:**
- Modify: `crates/zemon-tui/src/lib.rs`

- [ ] **Step 1: Add imports**

At the top of `crates/zemon-tui/src/lib.rs`, after the existing imports (lines 1-13), add:

```rust
use event::AppEvent;
use std::time::Duration;
```

- [ ] **Step 2: Add scout helper function**

After `spawn_connect` (around line 76), append:

```rust
fn spawn_scout_task(
    config: ZemonConfig,
    tx: mpsc::UnboundedSender<AppEvent>,
    timeout: Duration,
) {
    tokio::spawn(async move {
        let _ = tx.send(AppEvent::ScoutStarted);
        let now = std::time::SystemTime::now();
        match zemon_core::scout::scout(&config, timeout).await {
            Ok(scouts) => {
                let nodes: Vec<_> = scouts.iter().map(|s| s.to_node_info(now)).collect();
                let _ = tx.send(AppEvent::ScoutNodes(nodes));
            }
            Err(e) => {
                tracing::warn!("scout failed: {}", e);
                // Send empty so scout_in_progress clears.
                let _ = tx.send(AppEvent::ScoutNodes(Vec::new()));
            }
        }
    });
}

fn spawn_admin_polling_task(
    session: Arc<Mutex<Option<Session>>>,
    tx: mpsc::UnboundedSender<AppEvent>,
) {
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(Duration::from_secs(2));
        loop {
            ticker.tick().await;
            let sess = {
                let guard = session.lock().await;
                guard.as_ref().cloned()
            };
            let Some(sess) = sess else { continue };
            match zemon_core::registry::query_admin_nodes(&sess).await {
                Ok(nodes) => {
                    if tx.send(AppEvent::AdminNodes(nodes)).is_err() {
                        break;
                    }
                }
                Err(e) => {
                    tracing::warn!("admin query failed: {}", e);
                    // Skip — preserve previous admin_nodes snapshot.
                }
            }
        }
    });
}
```

- [ ] **Step 3: Start the admin polling task from `run_loop`**

In `run_loop` (around lines 78-93), immediately after `let mut reconnect_pending = true;` (line 91), add:

```rust
    let tx = events.sender();
    spawn_admin_polling_task(session.clone(), tx.clone());
    spawn_scout_task(config.clone(), tx.clone(), Duration::from_secs(3));
```

- [ ] **Step 4: Remove the old `list_nodes` call paths and dispatch pending scout**

Inside the main loop body, find the block at lines 93-114 that dispatches pending queries. Keep that block. Then, immediately after it (before the `tokio::select!`), add a pending-scout dispatch:

```rust
        // Dispatch pending scout request
        if app.pending_scout_request {
            app.pending_scout_request = false;
            app.scout_in_progress = true;
            spawn_scout_task(config.clone(), tx.clone(), Duration::from_secs(3));
        }
```

Then inside the `tokio::select!`, **remove** the two old `list_nodes` call paths:

- Line 139 (inside `ConnectResult::Connected`): delete the line `app.nodes = zemon_core::registry::list_nodes(&s).await.unwrap_or_default();`. The admin polling task already populates `app.admin_nodes` on its 2-second cadence.
- Lines 148-159 (`_ = refresh_interval.tick()` arm): replace the body that calls `list_nodes` inside `if let Some(s) = session.lock().await.as_ref()` with just the reconnect branch. New body:

```rust
            _ = refresh_interval.tick() => {
                if !app.is_connected() && !reconnect_pending {
                    app.connection_state = ConnectionState::Connecting;
                    reconnect_pending = true;
                    spawn_connect(config.clone(), conn_tx.clone());
                }
            }
```

- [ ] **Step 5: Confirm no `list_nodes` references remain**

Run: `grep -n "list_nodes" crates/zemon-tui/src/lib.rs`
Expected: no output.

- [ ] **Step 6: Build**

Run: `cargo check -p zemon-tui`
Expected: FAIL only on `crates/zemon-tui/src/views/nodes.rs` (which still uses `node.kind.as_str()` patterns that will still work, plus potentially stale field reads). If `lib.rs` itself errors, stop and fix.

Specifically expect errors like "no field `nodes` on App" — NO, the field still exists. Expected errors are instead around unused variables or similar. If the project compiles fully at this step, even better.

- [ ] **Step 7: Commit**

```bash
git add crates/zemon-tui/src/lib.rs
git commit -m "feat(tui): spawn admin polling + scout trigger tasks"
```

---

## Task 10: Rewrite Nodes view with 4 columns + source badge + footer

**Files:**
- Modify: `crates/zemon-tui/src/views/nodes.rs` (full replacement)

- [ ] **Step 1: Replace the file contents**

Replace `crates/zemon-tui/src/views/nodes.rs` with:

```rust
use crate::app::App;
use zemon_core::types::{NodeInfo, NodeSources};
use ratatui::layout::Constraint;
use ratatui::style::{Color, Modifier, Style};
use ratatui::widgets::{Block, Borders, Cell, Row, Table};
use ratatui::Frame;
use std::time::{Duration, SystemTime};

const STALE_THRESHOLD: Duration = Duration::from_secs(30);

pub fn render(app: &App, frame: &mut Frame, area: ratatui::layout::Rect) {
    let now = SystemTime::now();

    let header = Row::new(vec![
        Cell::from("ZID"),
        Cell::from("Kind"),
        Cell::from("Source"),
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
        .map(|(i, node)| build_row(node, i == app.node_selected, now))
        .collect();

    let widths = [
        Constraint::Percentage(30),
        Constraint::Percentage(10),
        Constraint::Percentage(15),
        Constraint::Percentage(45),
    ];

    let (n_admin, n_scout, n_both) = count_by_source(&app.nodes);
    let scout_status = if app.scout_in_progress {
        " [scouting…]"
    } else {
        ""
    };
    let title = format!(
        " Nodes ({}) — admin:{} scout:{} both:{}{} — j/k:nav  s:scout ",
        app.nodes.len(),
        n_admin,
        n_scout,
        n_both,
        scout_status
    );

    let table = Table::new(rows, widths)
        .header(header)
        .block(Block::default().borders(Borders::ALL).title(title))
        .row_highlight_style(Style::default().bg(Color::DarkGray));

    frame.render_widget(table, area);
}

fn build_row<'a>(node: &'a NodeInfo, selected: bool, now: SystemTime) -> Row<'a> {
    let stale = node.is_scout_stale(now, STALE_THRESHOLD);

    let base_style = if selected {
        Style::default()
            .fg(Color::Black)
            .bg(Color::Cyan)
            .add_modifier(Modifier::BOLD)
    } else if stale {
        Style::default().fg(Color::DarkGray)
    } else {
        Style::default()
    };

    let kind_style = match node.kind.as_str() {
        "router" => Style::default().fg(Color::Green),
        "peer" => Style::default().fg(Color::Blue),
        "client" => Style::default().fg(Color::Gray),
        _ => Style::default(),
    };

    let (source_text, source_color) = source_badge(node.sources, stale);
    let source_style = Style::default().fg(source_color);

    Row::new(vec![
        Cell::from(node.zid.clone()),
        Cell::from(node.kind.clone()).style(kind_style),
        Cell::from(source_text).style(source_style),
        Cell::from(node.locators.join(", ")),
    ])
    .style(base_style)
}

fn source_badge(sources: NodeSources, stale: bool) -> (String, Color) {
    let both = NodeSources::ADMIN | NodeSources::SCOUT;
    if sources == both {
        ("both".to_string(), Color::Cyan)
    } else if sources.contains(NodeSources::ADMIN) {
        ("admin".to_string(), Color::Green)
    } else if sources.contains(NodeSources::SCOUT) {
        if stale {
            ("scout·stale".to_string(), Color::DarkGray)
        } else {
            ("scout".to_string(), Color::Magenta)
        }
    } else {
        ("-".to_string(), Color::DarkGray)
    }
}

fn count_by_source(nodes: &[NodeInfo]) -> (usize, usize, usize) {
    let both = NodeSources::ADMIN | NodeSources::SCOUT;
    let mut n_admin = 0;
    let mut n_scout = 0;
    let mut n_both = 0;
    for n in nodes {
        if n.sources == both {
            n_both += 1;
        } else if n.sources.contains(NodeSources::ADMIN) {
            n_admin += 1;
        } else if n.sources.contains(NodeSources::SCOUT) {
            n_scout += 1;
        }
    }
    (n_admin, n_scout, n_both)
}
```

- [ ] **Step 2: Build entire workspace**

Run: `cargo check --workspace`
Expected: PASS across all three crates.

- [ ] **Step 3: Run the full test suite**

Run: `cargo test --workspace`
Expected: PASS on the 4 merge tests and 3 types tests (7 tests total green, no failures).

- [ ] **Step 4: Commit**

```bash
git add crates/zemon-tui/src/views/nodes.rs
git commit -m "feat(tui): 4-column Nodes view with source badge and stale dim"
```

---

## Task 11: Verify no stale `last_seen` references remain

**Files:**
- Grep only

- [ ] **Step 1: Grep the workspace**

Run: `grep -rn "last_seen" crates/ Cargo.toml`
Expected output: only matches for `admin_last_seen` and `scout_last_seen` (the new fields). If any bare `last_seen` remains, remove it and rebuild.

- [ ] **Step 2: Grep for old function name**

Run: `grep -rn "list_nodes" crates/`
Expected: no output.

- [ ] **Step 3: Release-profile build sanity check**

Run: `cargo build --release`
Expected: PASS. (This takes longer; run it at least once before manual testing.)

- [ ] **Step 4: No commit needed — verification only**

If Step 1 or Step 2 printed anything, fix it in a follow-up commit:

```bash
git add -u
git commit -m "chore: remove remaining last_seen/list_nodes references"
```

---

## Task 12: Manual integration testing

**Files:**
- None — runtime testing against a live zenohd

- [ ] **Step 1: Start a router**

In terminal 1:

```bash
zenohd
```

Expected: router starts and listens on the default `tcp/[::]:7447`.

- [ ] **Step 2: Start the TUI**

In terminal 2:

```bash
./target/release/zemon tui
```

Expected: TUI launches. Press `5` to switch to Nodes view. Within 2 seconds (first admin poll tick), the view should show:
- One row for the local session (`Source: admin`, kind `peer` or `client`)
- One row for the router (`Source: admin`, kind `router`, non-empty Locators)
- Title footer reads something like `Nodes (2) — admin:2 scout:0 both:0 — j/k:nav  s:scout`

- [ ] **Step 3: Connect a second peer and watch it appear**

In terminal 3:

```bash
./target/release/zemon sub "**"
```

Expected: within 2-4 seconds the TUI Nodes view gains a third row with `Source: admin`, kind `peer`, empty Locators column (per spec: peer/client locators are intentionally empty from admin data).

- [ ] **Step 4: Trigger a scout**

In the TUI, with the Nodes view active, press `s`.

Expected:
- Title footer flips to `... [scouting…]` briefly
- After ≤3 seconds, at least one row flips from `Source: admin` to `Source: both` (color Cyan). Any nodes only reachable via multicast scout but not via the router appear as `Source: scout` (color Magenta).

- [ ] **Step 5: Kill the subscribing peer**

In terminal 3, press Ctrl+C.

Expected: within 2-4 seconds the row disappears from the admin-only side. If the last scout had seen it, the row persists as `Source: scout` and, after 30 seconds, renders in DarkGray with `scout·stale`.

- [ ] **Step 6: Kill the router**

Stop `zenohd` (terminal 1: Ctrl+C).

Expected: TUI status bar shows "Disconnected" or similar. The admin polling task logs a warning but **does not** clear the previous `admin_nodes` snapshot — the Nodes view briefly retains its last state, then (on reconnect attempts) the state may update.

- [ ] **Step 7: Record results**

If any of steps 2-6 diverge from "Expected", capture the deviation in a new scratchpad or issue. Do not commit — manual test results do not go into the repo.

---

## Execution Handoff

Plan complete. Two execution options:

**1. Subagent-Driven (recommended)** — dispatch a fresh subagent per task, review between tasks, fast iteration

**2. Inline Execution** — execute tasks in this session using executing-plans, batch execution with checkpoints
