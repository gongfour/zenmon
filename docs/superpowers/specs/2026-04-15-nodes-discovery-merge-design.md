# Nodes Discovery Merge — Design

**Status:** Draft
**Date:** 2026-04-15
**Author:** Kang WonJin (with Claude)

## Problem

The TUI Nodes view currently shows only the local session and one router, even when the Zenoh network has many connected peers. The cause is in `crates/zemon-core/src/registry.rs::list_nodes()`: it queries `@/**` from the local peer session, which returns only the local session's admin data — the router's `sessions[]` array (containing connected peer/client transports) is never fetched, and no multicast scouting is performed.

## Goal

Make the Nodes view reflect the real state of the Zenoh network by combining two discovery sources:

1. **Router admin space** — live, accurate "who is currently connected to a router I see"
2. **Multicast scout** — router-independent "who is announcing themselves on this subnet"

Each source compensates for the other's weaknesses: admin covers cross-subnet mesh via the router, scout covers pure P2P and admin-silent cases.

## Non-Goals

- No new `zemon` CLI subcommand in this scope. Core API is designed to be CLI-reachable later.
- No parsing of `@/<zid>/router/linkstate/**` (text-format HAT). JSON `sessions` field is sufficient.
- No merging of `sessions[].links[].dst` addresses into the Locators column. Deferred to a future Detail view.
- No changes to the Dashboard view beyond what already uses `app.nodes.len()`.
- No automatic periodic scout. Manual trigger only (plus one startup scout).

## Architecture

```
┌────────────────────┐     2s polling      ┌─────────────┐
│ query_admin_nodes  │ ──────────────────► │             │
│  (registry.rs)     │                     │             │
└────────────────────┘                     │ merge_nodes │ ──► app.nodes
┌────────────────────┐   on-demand (`s`)   │  (pure fn)  │     (Vec<NodeInfo>)
│    scout_nodes     │ ──────────────────► │             │
│   (scout.rs)       │                     │             │
└────────────────────┘                     └─────────────┘
```

Two independent data-gathering paths, a single pure merge function, one rendered result. The TUI event loop runs each source as its own background tokio task; results flow in through `AppEvent` variants; `merge_nodes` is invoked on each update.

### Component boundaries

- **`zemon-core::registry`** — owns admin-space querying. Returns `Vec<NodeInfo>` tagged with `NodeSources::ADMIN`.
- **`zemon-core::scout`** — already exists. Gets a small `to_node_info()` conversion helper. Tagged with `NodeSources::SCOUT`.
- **`zemon-core::merge`** — new module. Pure function only; no I/O, no `Session` dependency. Unit-testable.
- **`zemon-core::types`** — `NodeInfo` gains source tracking fields.
- **`zemon-tui::app`** — holds `admin_nodes` and `scout_nodes` separately; computes `nodes` (merged) on each update.
- **`zemon-tui::lib`** — spawns admin polling task and scout task; wires `AppEvent` handling.
- **`zemon-tui::event`** — adds `AdminNodes`, `ScoutStarted`, `ScoutNodes` variants; exposes the sender via a new `EventHandler::sender()` accessor so background tasks in `lib.rs` can emit events; handles `s` key in Nodes view.
- **`zemon-tui::views::nodes`** — renders new 4-column layout with Source column and footer counts.

## Data Model

### `NodeSources` bitflags

```rust
use bitflags::bitflags;

bitflags! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
    pub struct NodeSources: u8 {
        const ADMIN = 0b01;
        const SCOUT = 0b10;
    }
}
```

Added `bitflags` crate to `zemon-core/Cargo.toml`.

### `NodeInfo` (modified)

```rust
#[derive(Debug, Clone)]
pub struct NodeInfo {
    pub zid: String,
    pub kind: String,                      // "router" | "peer" | "client" | "unknown"
    pub locators: Vec<String>,
    pub metadata: Option<serde_json::Value>,
    pub sources: NodeSources,
    pub admin_last_seen: Option<SystemTime>,
    pub scout_last_seen: Option<SystemTime>,
}
```

The existing `last_seen: Option<SystemTime>` field is removed; it is replaced by the two source-specific timestamps.

### `is_scout_stale` helper

```rust
impl NodeInfo {
    pub fn is_scout_stale(&self, now: SystemTime, threshold: Duration) -> bool {
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

Rule: **a node is never stale if it is currently seen in admin**. Stale applies only to scout-only entries whose last scout observation exceeds the threshold (30 seconds).

## Core Logic

### `query_admin_nodes(session: &Session) -> Result<Vec<NodeInfo>>`

Replaces the existing `list_nodes()` function. Steps:

1. Query `@/*/router` to fetch each visible router's root admin payload.
2. For each reply, parse the JSON: `{zid, locators, sessions: [{peer, whatami, links: [{src, dst}], ...}, ...]}`.
3. Emit one `NodeInfo` for the router itself (`kind: "router"`, `locators` from root payload).
4. Emit one `NodeInfo` per entry in `sessions[]`:
   - `zid = sessions[i].peer`
   - `kind = sessions[i].whatami`
   - `locators = Vec::new()` — transport link endpoints (`sessions[i].links[].dst`) are **not** used here. Per Non-Goals, those belong in a future Detail view. Peer/client rows will simply show an empty Locators column for admin-derived entries unless scout also discovers them and contributes scout-advertised locators during merge.
5. Query `@/**` to include the local session's own admin data (so the current peer appears in the list).
6. Deduplicate by `zid` — on collision, take union of `locators` and keep the first-seen `kind`/`metadata`.
7. Tag every emitted node with `sources = NodeSources::ADMIN` and `admin_last_seen = Some(now)`.

Partial-failure tolerance: if a reply fails to parse, skip that reply and continue. A query that fails at the session level returns `Err`.

### `ScoutInfo::to_node_info(&self, now) -> NodeInfo`

Small conversion helper in `scout.rs`. Produces a `NodeInfo` tagged `NodeSources::SCOUT` with `scout_last_seen = Some(now)` and `admin_last_seen = None`. Field mapping mirrors the existing `ScoutInfo` structure; the plan phase will read `scout.rs` and confirm exact field names.

### `merge_nodes(admin: &[NodeInfo], scout: &[NodeInfo]) -> Vec<NodeInfo>`

Pure function in new `zemon-core/src/merge.rs`:

```rust
pub fn merge_nodes(admin: &[NodeInfo], scout: &[NodeInfo]) -> Vec<NodeInfo> {
    use std::collections::HashMap;
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
```

Merge rules:
- Admin-first: admin entries seed the map, so admin-derived `kind` and `metadata` win on collision.
- Scout adds its timestamp and `NodeSources::SCOUT` flag on collision.
- `locators` is a union (de-duplicated by string equality).
- Final order: sorted by `zid`.

## TUI Integration

### `App` state additions (`zemon-tui/src/app.rs`)

```rust
pub admin_nodes: Vec<NodeInfo>,
pub scout_nodes: Vec<NodeInfo>,
pub nodes: Vec<NodeInfo>,           // cached merge result, used by renderer
pub node_selected: usize,           // existing
pub scout_in_progress: bool,
pub last_scout_at: Option<SystemTime>,
```

The existing `nodes` field remains and continues to be what the renderer reads; it is now a derived cache, recomputed only when `admin_nodes` or `scout_nodes` changes (never per frame).

### `AppEvent` additions (`zemon-tui/src/event.rs`)

```rust
AdminNodes(Vec<NodeInfo>),
ScoutStarted,
ScoutNodes(Vec<NodeInfo>),
```

### `EventHandler` sender exposure (`zemon-tui/src/event.rs`)

Today, the `AppEvent` sender is created inside `EventHandler::new()` and moved into the spawned input/zenoh tasks — there is no way for `lib.rs` to obtain a clone. The admin polling and scout tasks in this design require a sender from outside the event handler, so `EventHandler::new()` must expose it.

**Change**: `EventHandler` stores the sender as a struct field and exposes a `sender()` accessor:

```rust
pub struct EventHandler {
    tx: mpsc::UnboundedSender<AppEvent>,        // NEW: retained for external cloning
    rx: mpsc::UnboundedReceiver<AppEvent>,
    _task: tokio::task::JoinHandle<()>,
}

impl EventHandler {
    pub fn new(tick_rate_ms: u64, zenoh_rx: mpsc::UnboundedReceiver<ZenohMessage>) -> Self {
        let (tx, rx) = mpsc::unbounded_channel();
        // ... existing zenoh-forwarding task uses tx.clone() ...
        // ... existing key/tick task uses tx.clone() (the clone moves into the task; the
        //     original `tx` is now stored in the struct instead of being dropped) ...
        Self { tx, rx, _task: task }
    }

    pub fn sender(&self) -> mpsc::UnboundedSender<AppEvent> {
        self.tx.clone()
    }

    // existing `next()` unchanged
}
```

`run_loop` in `lib.rs` calls `events.sender()` once after constructing `EventHandler`, then passes clones of the returned sender into each background task (admin polling + scout trigger helper).

**Note on the existing internal clones**: the existing code moves `tx` into both spawned tasks inside `EventHandler::new`. That pattern works because each spawned task needs to own its clone. The patch is to clone first (for the internal tasks) **and then store the original in `self.tx`** rather than letting it drop at the end of `new()`. No behavior change for existing senders — only the sender's lifetime is extended.

### Background tasks (`zemon-tui/src/lib.rs`)

Before spawning the tasks below, `run_loop` grabs the sender once:

```rust
let tx = events.sender();
```

**Admin polling task** — spawned once on startup with a connected session:

```rust
let admin_tx = tx.clone();
let session_a = session.clone();
tokio::spawn(async move {
    let mut ticker = tokio::time::interval(Duration::from_secs(2));
    loop {
        ticker.tick().await;
        match query_admin_nodes(&session_a).await {
            Ok(nodes) => {
                if admin_tx.send(AppEvent::AdminNodes(nodes)).is_err() {
                    break;
                }
            }
            Err(e) => {
                tracing::warn!("admin query failed: {e}");
                // Do not send — preserve previous admin_nodes snapshot.
            }
        }
    }
});
```

`tokio::time::interval`'s first tick fires immediately, so the first Nodes view render is already populated without needing a separate eager call. On a transient query error, the task logs and **skips** the send, preserving the previous `admin_nodes` state in `App` (matching the Error Handling section below).

**Startup scout task** — spawned once on startup:

```rust
spawn_scout_task(tx.clone(), Duration::from_secs(3));
```

**User-triggered scout** — on `s` key in Nodes view:

```rust
KeyCode::Char('s') if app.active_view == ActiveView::Nodes => {
    if !app.scout_in_progress {
        app.scout_in_progress = true;
        spawn_scout_task(tx.clone(), Duration::from_secs(3));
    }
}
```

**`spawn_scout_task`** — helper that sends `ScoutStarted`, runs the scout with a 3-second timeout, converts results via `to_node_info()`, then sends `ScoutNodes(Vec<NodeInfo>)`.

### Event handlers (in App)

```rust
AppEvent::AdminNodes(nodes) => {
    self.admin_nodes = nodes;
    self.nodes = merge_nodes(&self.admin_nodes, &self.scout_nodes);
    if self.node_selected >= self.nodes.len() {
        self.node_selected = self.nodes.len().saturating_sub(1);
    }
}
AppEvent::ScoutStarted => {
    self.scout_in_progress = true;
}
AppEvent::ScoutNodes(nodes) => {
    self.scout_nodes = nodes;
    self.last_scout_at = Some(SystemTime::now());
    self.scout_in_progress = false;
    self.nodes = merge_nodes(&self.admin_nodes, &self.scout_nodes);
    if self.node_selected >= self.nodes.len() {
        self.node_selected = self.nodes.len().saturating_sub(1);
    }
}
```

The existing `lib.rs:139` call path (`app.nodes = list_nodes(&s).await...`) is removed entirely. Admin polling task owns that responsibility now.

## UI Rendering

### Layout

Four-column table in `crates/zemon-tui/src/views/nodes.rs`:

| Column | Width | Content |
|--------|-------|---------|
| ZID | 30% | Existing |
| Kind | 10% | Existing (with existing color coding) |
| Source | 15% | New |
| Locators | 45% | Existing, now narrower |

### Source cell rendering

Computed per row using `node.sources` and `node.is_scout_stale(now, Duration::from_secs(30))`:

| `sources`          | stale | display       | color     |
|--------------------|-------|---------------|-----------|
| `ADMIN`            | —     | `admin`       | Green     |
| `SCOUT`            | false | `scout`       | Magenta   |
| `SCOUT`            | true  | `scout·stale` | DarkGray  |
| `ADMIN \| SCOUT`   | —     | `both`        | Cyan      |

A row whose scout entry is stale is additionally rendered with a dimmed (`DarkGray`) row style to make it visually recede.

### Block title / footer

```rust
let (n_admin, n_scout, n_both) = count_by_source(&app.nodes);
let scout_status = if app.scout_in_progress { " [scouting…]" } else { "" };
let title = format!(
    " Nodes ({}) — admin:{} scout:{} both:{}{} — j/k:nav  s:scout ",
    app.nodes.len(), n_admin, n_scout, n_both, scout_status
);
```

`count_by_source` is a small private helper in the same file.

### Key binding check

The plan will grep `crates/zemon-tui/src/event.rs` for existing `KeyCode::Char('s')` usage before claiming `s` is free. If taken, fall back to `S` (shift-s) with corresponding update to the help text.

## Testing Strategy

This feature ships the project's first unit tests. Scope is kept deliberately narrow to the pure, deterministic pieces.

### `zemon-core/src/merge.rs` unit tests

- `merge_admin_only_passes_through_sorted` — admin input with two entries, scout empty; output matches admin sorted by zid.
- `merge_scout_only_passes_through_sorted` — symmetric.
- `merge_overlapping_zid_unions_sources_and_locators` — one admin entry and one scout entry with the same zid but different locators; output has one entry with `ADMIN | SCOUT`, both timestamps, union of locators, admin's `kind`.
- `merge_disjoint_zids_produces_sorted_union` — no overlap; result contains all entries from both inputs sorted.

### `zemon-core/src/types.rs` unit tests

- `stale_false_when_admin_flag_set` — node with `ADMIN | SCOUT`, old `scout_last_seen`; returns false.
- `stale_false_when_scout_recent` — SCOUT only, timestamp within threshold; false.
- `stale_true_when_scout_exceeds_threshold` — SCOUT only, timestamp older than threshold; true.

### Manual integration test (documented checklist)

Run in `docs/superpowers/plans/...` final task. Steps:

1. Start `zenohd`.
2. Start `zemon tui`. Verify Nodes view shows at minimum the local session and the router, both with Source `admin`.
3. In another terminal: `zemon sub "**"`. Within 2–4 seconds, the new peer should appear in the Nodes view with Source `admin`.
4. Press `s` in the Nodes view. Title shows `[scouting…]`, then scout results populate. Nodes also seen via admin should flip to Source `both`.
5. Kill the `zemon sub` process. Within 2–4 seconds the peer should disappear from the Nodes view (admin no longer sees it, and if scout last saw it, it becomes scout-only).
6. After 30 seconds, scout-only stale nodes should render dimmed with `scout·stale`.

## Error Handling

- **Router admin query fails** (timeout, router gone): `query_admin_nodes` returns whatever partial results it has. The polling task treats `Err` as "keep previous state" — it does NOT clear `admin_nodes` on a transient failure. Implementation: `unwrap_or_default()` in the polling task is acceptable for MVP because `query_admin_nodes` only returns `Err` on catastrophic session-level failure, at which point the TUI is already broken.
- **Scout fails / times out with no results**: `scout_nodes = vec![]` is sent, clearing the scout side. This is intentional — a fresh empty scout result should supersede old scout results on explicit user trigger.
- **Malformed admin payload**: skip the reply, continue. Logged via `tracing::warn` (TUI filter is `off`, so this only shows in non-TUI runs).
- **Empty `sessions[]` array**: router appears alone with no connected transports. Not an error.

## Migration Notes

- `NodeInfo::last_seen` is removed. Any grep of the codebase for `last_seen` should return zero hits outside this spec and the plan. The plan includes a verification step.
- `list_nodes` is renamed to `query_admin_nodes`. Callers: `zemon-tui/src/lib.rs:139` (removed — polling task replaces it) and the dashboard indirectly through `app.nodes`. No CLI callers today.
- `zemon-core/Cargo.toml` gains `bitflags = "2"` (workspace version TBD by plan step).

## Open Questions

None at design time. Plan-phase verifications (file reads, grep checks) are annotated inline with the tasks.
