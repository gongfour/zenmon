# TUI Network View & Scout Separation Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the flat "Nodes" tab with a "Network" tab that renders a router→peer topology tree, and split the overloaded "scout" into `r`:refresh vs `P`:Switch Scouting Port using accurate native-Zenoh terminology.

**Architecture:** A pure model function `build_topology_rows` turns the existing merged `NodeInfo` list (with `metadata.sessions`) into a flat list of display rows (headers + nodes); the Network view renders that list as a tree on the left and the existing node detail on the right. All logic lives in `crates/zenmon-tui`; core/CLI are untouched.

**Tech Stack:** Rust, ratatui, crossterm, serde_json. Tests via `cargo test` (`zenmon-tui`).

**Spec:** `docs/superpowers/specs/2026-07-16-tui-network-view-design.md`

## Global Constraints

- TUI-only: no behavior change to `zenmon-core` or `zenmon-cli`. `ZenmonConfig.scout_port` stays as-is; domain↔port math (if any) lives only in the TUI layer.
- Terminology: no "domain" label anywhere in new/modified TUI code. Use "scouting port" / "discovery network".
- Reuse the existing node detail rendering; do not rewrite it.
- Test only pure logic (model + helpers). Rendering is not unit-tested, matching the existing convention.
- English in code; commit messages `feat(tui):` / `refactor(tui):`.
- Branch: `feat/tui-network-view` (already checked out, based on the rename branch).
- Baseline before starting: `cargo build` clean, `cargo test` = 30 passing (8 core + 22 tui).

---

## Task 1: Rename Nodes → Network (mechanical, no behavior change)

**Files:**
- Rename: `crates/zenmon-tui/src/views/nodes.rs` → `crates/zenmon-tui/src/views/network.rs`
- Modify: `crates/zenmon-tui/src/views/mod.rs`
- Modify: `crates/zenmon-tui/src/app.rs` (enum variant, TAB_TITLES, dispatch, all match arms)

**Interfaces:**
- Produces: `ActiveView::Network` (was `ActiveView::Nodes`), `views::network::render(app, frame, area)`.

- [ ] **Step 1: Rename the view file**

```bash
git mv crates/zenmon-tui/src/views/nodes.rs crates/zenmon-tui/src/views/network.rs
```

- [ ] **Step 2: Update the module declaration**

In `crates/zenmon-tui/src/views/mod.rs`, change line 3:

```rust
pub mod network;
```

(replacing `pub mod nodes;`)

- [ ] **Step 3: Rename the enum variant and dispatch across app.rs**

Run these substitutions on `crates/zenmon-tui/src/app.rs`:

```bash
sed -i '' \
  -e 's/ActiveView::Nodes/ActiveView::Network/g' \
  -e 's/views::nodes::render/views::network::render/g' \
  crates/zenmon-tui/src/app.rs
```

- [ ] **Step 4: Update the tab title**

In `crates/zenmon-tui/src/app.rs`, change the `TAB_TITLES` constant (line ~59):

```rust
const TAB_TITLES: [&str; 6] = ["Dashboard", "Topics", "Stream", "Query", "Network", "Liveliness"];
```

- [ ] **Step 5: Build and test**

Run: `cargo build -p zenmon-tui && cargo test -p zenmon-tui`
Expected: build clean; `test result: ok. 22 passed`.

- [ ] **Step 6: Commit**

```bash
git add -A
git commit -m "refactor(tui): rename Nodes tab/view to Network"
```

---

## Task 2: Topology model — `build_topology_rows` + helpers (TDD)

**Files:**
- Create: `crates/zenmon-tui/src/views/topology.rs`
- Modify: `crates/zenmon-tui/src/views/mod.rs` (add `pub mod topology;`)

**Interfaces:**
- Produces:
  - `enum TopoRow { Header(String), Node(TopoNode) }`
  - `struct TopoNode { zid: String, kind: String, locator: String, is_child: bool, alive: bool, is_self: bool, in_registry: bool }`
  - `fn build_topology_rows(nodes: &[NodeInfo], self_zid: Option<&str>, now: SystemTime) -> Vec<TopoRow>`
  - `fn node_row_count(rows: &[TopoRow]) -> usize`
  - `fn nth_node_zid(rows: &[TopoRow], n: usize) -> Option<&str>`
  - `fn node_index_at_visual(rows: &[TopoRow], visual: usize) -> Option<usize>`
  - `fn visual_index_of_node(rows: &[TopoRow], node_idx: usize) -> Option<usize>`

- [ ] **Step 1: Add the module declaration**

In `crates/zenmon-tui/src/views/mod.rs`, add:

```rust
pub mod topology;
```

- [ ] **Step 2: Write the failing tests**

Create `crates/zenmon-tui/src/views/topology.rs` with ONLY the test module first:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use zenmon_core::types::{NodeInfo, NodeSources};
    use std::time::{Duration, SystemTime};

    fn node(zid: &str, kind: &str, sources: NodeSources) -> NodeInfo {
        NodeInfo {
            zid: zid.into(),
            kind: kind.into(),
            locators: vec!["tcp/1.2.3.4:7447".into()],
            metadata: None,
            sources,
            admin_last_seen: None,
            scout_last_seen: None,
        }
    }

    fn router_with_sessions(zid: &str, peers: &[&str]) -> NodeInfo {
        let sessions: Vec<_> = peers
            .iter()
            .map(|p| serde_json::json!({
                "peer": p, "whatami": "peer",
                "links": [{"dst": "tcp/9.9.9.9:41000"}]
            }))
            .collect();
        let mut n = node(zid, "router", NodeSources::ADMIN);
        n.metadata = Some(serde_json::json!({ "sessions": sessions }));
        n
    }

    fn node_zids(rows: &[TopoRow]) -> Vec<&str> {
        rows.iter()
            .filter_map(|r| match r {
                TopoRow::Node(n) => Some(n.zid.as_str()),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn router_lists_its_sessions_as_children() {
        let nodes = vec![
            router_with_sessions("r1", &["p1", "p2"]),
            node("p1", "peer", NodeSources::ADMIN),
            node("p2", "peer", NodeSources::ADMIN),
        ];
        let rows = build_topology_rows(&nodes, None, SystemTime::now());
        assert_eq!(node_row_count(&rows), 3);
        match (&rows[0], &rows[1], &rows[2]) {
            (TopoRow::Node(r), TopoRow::Node(a), TopoRow::Node(b)) => {
                assert_eq!(r.zid, "r1");
                assert!(!r.is_child);
                assert_eq!(a.zid, "p1");
                assert!(a.is_child);
                assert_eq!(b.zid, "p2");
                assert!(b.is_child);
            }
            _ => panic!("expected 3 node rows"),
        }
    }

    #[test]
    fn peer_under_two_routers_appears_twice() {
        let nodes = vec![
            router_with_sessions("r1", &["p1"]),
            router_with_sessions("r2", &["p1"]),
            node("p1", "peer", NodeSources::ADMIN),
        ];
        let rows = build_topology_rows(&nodes, None, SystemTime::now());
        let p1_count = node_zids(&rows).iter().filter(|z| **z == "p1").count();
        assert_eq!(p1_count, 2);
    }

    #[test]
    fn orphan_non_router_goes_to_unlinked_group() {
        let nodes = vec![
            router_with_sessions("r1", &[]),
            node("p9", "peer", NodeSources::SCOUT),
        ];
        let rows = build_topology_rows(&nodes, None, SystemTime::now());
        let header_pos = rows
            .iter()
            .position(|r| matches!(r, TopoRow::Header(h) if h.contains("unlinked")));
        assert!(header_pos.is_some(), "expected an unlinked header");
        let p9_pos = node_zids(&rows).iter().position(|z| **z == "p9");
        assert!(p9_pos.is_some());
    }

    #[test]
    fn no_router_produces_flat_list_under_header() {
        let nodes = vec![
            node("p1", "peer", NodeSources::ADMIN),
            node("p2", "peer", NodeSources::ADMIN),
        ];
        let rows = build_topology_rows(&nodes, None, SystemTime::now());
        assert!(matches!(&rows[0], TopoRow::Header(h) if h.contains("no router")));
        assert_eq!(node_zids(&rows), vec!["p1", "p2"]);
    }

    #[test]
    fn empty_nodes_produce_no_rows() {
        let rows = build_topology_rows(&[], None, SystemTime::now());
        assert!(rows.is_empty());
    }

    #[test]
    fn self_node_is_marked() {
        let nodes = vec![node("me", "peer", NodeSources::ADMIN)];
        let rows = build_topology_rows(&nodes, Some("me"), SystemTime::now());
        match &rows[1] {
            TopoRow::Node(n) => assert!(n.is_self),
            _ => panic!("expected node row after header"),
        }
    }

    #[test]
    fn scout_only_stale_node_is_not_alive() {
        let now = SystemTime::now();
        let mut n = node("s1", "peer", NodeSources::SCOUT);
        n.scout_last_seen = Some(now - Duration::from_secs(60));
        let rows = build_topology_rows(&[n], None, now);
        match &rows[1] {
            TopoRow::Node(node) => assert!(!node.alive),
            _ => panic!("expected node row after header"),
        }
    }

    #[test]
    fn node_index_at_visual_skips_headers() {
        let rows = vec![
            TopoRow::Header("h".into()),
            TopoRow::Node(TopoNode {
                zid: "a".into(), kind: "peer".into(), locator: "-".into(),
                is_child: false, alive: true, is_self: false, in_registry: true,
            }),
        ];
        assert_eq!(node_index_at_visual(&rows, 0), None); // header
        assert_eq!(node_index_at_visual(&rows, 1), Some(0)); // first node
        assert_eq!(nth_node_zid(&rows, 0), Some("a"));
        assert_eq!(visual_index_of_node(&rows, 0), Some(1));
    }
}
```

- [ ] **Step 3: Run tests to verify they fail to compile**

Run: `cargo test -p zenmon-tui topology 2>&1 | tail -5`
Expected: compile error — `build_topology_rows` / `TopoRow` not found.

- [ ] **Step 4: Implement the model above the test module**

Insert this at the TOP of `crates/zenmon-tui/src/views/topology.rs` (before the `#[cfg(test)]` block):

```rust
use zenmon_core::types::NodeInfo;
use std::collections::HashSet;
use std::time::{Duration, SystemTime};

const STALE_THRESHOLD: Duration = Duration::from_secs(30);

#[derive(Debug, Clone, PartialEq)]
pub enum TopoRow {
    Header(String),
    Node(TopoNode),
}

#[derive(Debug, Clone, PartialEq)]
pub struct TopoNode {
    pub zid: String,
    pub kind: String,
    pub locator: String,
    pub is_child: bool,
    pub alive: bool,
    pub is_self: bool,
    /// True when a full `NodeInfo` backs this row (detail lookup will succeed).
    pub in_registry: bool,
}

fn best_locator(node: &NodeInfo) -> String {
    node.locators.first().cloned().unwrap_or_else(|| "-".to_string())
}

/// Parse a router's admin `metadata.sessions` into (peer_zid, whatami, link_dst).
fn parse_sessions(node: &NodeInfo) -> Vec<(String, String, String)> {
    let Some(meta) = &node.metadata else { return Vec::new() };
    let Some(sessions) = meta.get("sessions").and_then(|v| v.as_array()) else {
        return Vec::new();
    };
    sessions
        .iter()
        .filter_map(|s| {
            let peer = s.get("peer").and_then(|v| v.as_str())?.to_string();
            let whatami = s
                .get("whatami")
                .and_then(|v| v.as_str())
                .unwrap_or("peer")
                .to_string();
            let link = s
                .get("links")
                .and_then(|v| v.as_array())
                .and_then(|l| l.first())
                .and_then(|l| l.get("dst"))
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            Some((peer, whatami, link))
        })
        .collect()
}

/// Build the flat topology row list: routers as roots, their non-router
/// sessions as children, remaining non-router nodes under an "unlinked" group.
/// When no routers exist, every node is listed flat under a "no router" header.
pub fn build_topology_rows(
    nodes: &[NodeInfo],
    self_zid: Option<&str>,
    now: SystemTime,
) -> Vec<TopoRow> {
    let is_self = |zid: &str| self_zid == Some(zid);
    let mk = |n: &NodeInfo, is_child: bool| {
        TopoRow::Node(TopoNode {
            zid: n.zid.clone(),
            kind: n.kind.clone(),
            locator: best_locator(n),
            is_child,
            alive: !n.is_scout_stale(now, STALE_THRESHOLD),
            is_self: is_self(&n.zid),
            in_registry: true,
        })
    };

    let mut routers: Vec<&NodeInfo> = nodes.iter().filter(|n| n.kind == "router").collect();
    routers.sort_by(|a, b| a.zid.cmp(&b.zid));

    if routers.is_empty() {
        if nodes.is_empty() {
            return Vec::new();
        }
        let mut rows = vec![TopoRow::Header("── nodes (no router) ──".to_string())];
        let mut sorted: Vec<&NodeInfo> = nodes.iter().collect();
        sorted.sort_by(|a, b| a.zid.cmp(&b.zid));
        rows.extend(sorted.into_iter().map(|n| mk(n, false)));
        return rows;
    }

    let mut rows = Vec::new();
    let mut child_zids: HashSet<String> = HashSet::new();

    for router in &routers {
        rows.push(mk(router, false));
        let mut seen = HashSet::new();
        for (peer_zid, whatami, link) in parse_sessions(router) {
            if !seen.insert(peer_zid.clone()) {
                continue; // dedup within a single router
            }
            // Routers are shown only as their own roots, never as children.
            if nodes.iter().any(|n| n.zid == peer_zid && n.kind == "router") {
                continue;
            }
            child_zids.insert(peer_zid.clone());
            match nodes.iter().find(|n| n.zid == peer_zid) {
                Some(n) => rows.push(mk(n, true)),
                None => rows.push(TopoRow::Node(TopoNode {
                    zid: peer_zid.clone(),
                    kind: whatami,
                    locator: if link.is_empty() { "-".to_string() } else { link },
                    is_child: true,
                    alive: true,
                    is_self: is_self(&peer_zid),
                    in_registry: false,
                })),
            }
        }
    }

    let mut unlinked: Vec<&NodeInfo> = nodes
        .iter()
        .filter(|n| n.kind != "router" && !child_zids.contains(&n.zid))
        .collect();
    unlinked.sort_by(|a, b| a.zid.cmp(&b.zid));
    if !unlinked.is_empty() {
        rows.push(TopoRow::Header("── unlinked (scouted) ──".to_string()));
        rows.extend(unlinked.into_iter().map(|n| mk(n, false)));
    }

    rows
}

pub fn node_row_count(rows: &[TopoRow]) -> usize {
    rows.iter().filter(|r| matches!(r, TopoRow::Node(_))).count()
}

pub fn nth_node_zid(rows: &[TopoRow], n: usize) -> Option<&str> {
    rows.iter()
        .filter_map(|r| match r {
            TopoRow::Node(x) => Some(x.zid.as_str()),
            _ => None,
        })
        .nth(n)
}

/// Map a visual row index (headers included) to a selectable node index.
/// Returns `None` if the visual row is a header or out of range.
pub fn node_index_at_visual(rows: &[TopoRow], visual: usize) -> Option<usize> {
    let mut node_idx = 0;
    for (i, r) in rows.iter().enumerate() {
        match r {
            TopoRow::Node(_) => {
                if i == visual {
                    return Some(node_idx);
                }
                node_idx += 1;
            }
            TopoRow::Header(_) => {
                if i == visual {
                    return None;
                }
            }
        }
    }
    None
}

/// Inverse of `node_index_at_visual`: the visual row index of the n-th node.
pub fn visual_index_of_node(rows: &[TopoRow], node_idx: usize) -> Option<usize> {
    let mut seen = 0;
    for (i, r) in rows.iter().enumerate() {
        if let TopoRow::Node(_) = r {
            if seen == node_idx {
                return Some(i);
            }
            seen += 1;
        }
    }
    None
}
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p zenmon-tui topology 2>&1 | tail -5`
Expected: `test result: ok. 8 passed`.

- [ ] **Step 6: Commit**

```bash
git add -A
git commit -m "feat(tui): add topology row model and selection helpers"
```

---

## Task 3: Network view — tree render + selection/nav wiring

**Files:**
- Modify: `crates/zenmon-tui/src/views/network.rs` (replace list rendering with tree; detail lookup by zid)
- Modify: `crates/zenmon-tui/src/app.rs` (Network key/click/wheel handlers; `s`→`r`)

**Interfaces:**
- Consumes: `topology::{build_topology_rows, node_row_count, nth_node_zid, node_index_at_visual, visual_index_of_node, TopoRow}`.
- `app.node_selected` now indexes selectable **node rows** (not `app.nodes`).

- [ ] **Step 1: Replace the list renderer in network.rs with a topology tree**

In `crates/zenmon-tui/src/views/network.rs`, replace the whole `render` fn and `render_node_list` fn with:

```rust
use crate::views::topology::{
    build_topology_rows, node_row_count, visual_index_of_node, TopoRow,
};

pub fn render(app: &mut App, frame: &mut Frame, area: ratatui::layout::Rect) {
    let [list_area, detail_area] =
        Layout::horizontal([Constraint::Percentage(55), Constraint::Percentage(45)])
            .areas(area);

    render_topology(app, frame, list_area);
    render_node_detail(app, frame, detail_area);
}

fn render_topology(app: &mut App, frame: &mut Frame, area: ratatui::layout::Rect) {
    let now = SystemTime::now();
    let rows = build_topology_rows(&app.nodes, app.self_zid.as_deref(), now);
    let total_nodes = node_row_count(&rows);

    // Clamp selection and compute a visual scroll offset that keeps the
    // selected node visible.
    if total_nodes == 0 {
        app.node_selected = 0;
    } else if app.node_selected >= total_nodes {
        app.node_selected = total_nodes - 1;
    }
    app.list_rect = Some(area);
    app.list_first_item_row = area.y + 1;
    let visible = area.height.saturating_sub(2) as usize;
    let sel_visual = visual_index_of_node(&rows, app.node_selected).unwrap_or(0);
    app.list_scroll_offset = if visible > 0 && sel_visual >= visible {
        sel_visual + 1 - visible
    } else {
        0
    };

    let scout_status = if app.scout_in_progress { " [scouting...]" } else { "" };
    let port = app.scout_port_current.unwrap_or(7446);
    let title = format!(" Topology — scout:{} · {} nodes{} ", port, total_nodes, scout_status);

    let mut node_idx = 0usize;
    let lines: Vec<Line> = rows
        .iter()
        .skip(app.list_scroll_offset)
        .take(visible)
        .map(|row| match row {
            TopoRow::Header(label) => {
                Line::from(Span::styled(label.clone(), Style::default().fg(Color::DarkGray)))
            }
            TopoRow::Node(n) => {
                let this = node_idx;
                node_idx += 1;
                topo_node_line(n, this == app.node_selected)
            }
        })
        .collect();

    // Advance node_idx for rows skipped by scroll so selection highlight lines up.
    // (skip() above already dropped rows; recompute selection relative to slice.)
    let para = Paragraph::new(lines)
        .block(Block::default().borders(Borders::ALL).title(title));
    frame.render_widget(para, area);
}

fn topo_node_line<'a>(n: &crate::views::topology::TopoNode, selected: bool) -> Line<'a> {
    let icon = if n.alive { "● " } else { "○ " };
    let icon_style = if n.alive {
        Style::default().fg(Color::Green)
    } else {
        Style::default().fg(Color::Red)
    };
    let kind_color = match n.kind.as_str() {
        "router" => Color::Green,
        "peer" => Color::Blue,
        "client" => Color::Gray,
        _ => Color::White,
    };
    let name_style = if selected {
        Style::default().fg(Color::Black).bg(Color::Cyan).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(Color::White)
    };
    let branch = if n.is_child { " ├─ " } else { "" };
    let cursor = if selected { ">" } else { " " };
    let zid_short = &n.zid[..n.zid.len().min(16)];
    let mut spans = vec![
        Span::raw(format!("{}{}", cursor, branch)),
        Span::styled(icon, if selected { name_style } else { icon_style }),
        Span::styled(format!("{:<7}", n.kind), if selected { name_style } else { Style::default().fg(kind_color) }),
        Span::raw(" "),
        Span::styled(zid_short.to_string(), name_style),
        Span::raw("  "),
        Span::styled(n.locator.clone(), Style::default().fg(Color::DarkGray)),
    ];
    if n.is_self {
        spans.push(Span::styled(" (self)", Style::default().fg(Color::DarkGray)));
    }
    if !n.alive {
        spans.push(Span::styled(" stale", Style::default().fg(Color::Red)));
    }
    Line::from(spans)
}
```

Note: the selection-highlight index above assumes `node_idx` counts from the first *visible* node. Because `skip(list_scroll_offset)` may drop node rows, initialize `node_idx` to the number of node rows before the scroll offset. Add this immediately before the `let mut node_idx = 0usize;` line and use it as the initial value:

```rust
let node_idx_before = rows
    .iter()
    .take(app.list_scroll_offset)
    .filter(|r| matches!(r, TopoRow::Node(_)))
    .count();
let mut node_idx = node_idx_before;
```

(Replace `let mut node_idx = 0usize;` with `let mut node_idx = node_idx_before;`.)

- [ ] **Step 2: Point the detail panel at the selected zid**

In `crates/zenmon-tui/src/views/network.rs`, change the first line of `render_node_detail` from the index lookup to a zid lookup. Replace:

```rust
    let selected = app.nodes.get(app.node_selected);
```

with:

```rust
    let rows = build_topology_rows(&app.nodes, app.self_zid.as_deref(), SystemTime::now());
    let selected_zid = crate::views::topology::nth_node_zid(&rows, app.node_selected);
    let selected = selected_zid.and_then(|z| app.nodes.iter().find(|n| n.zid == z));
```

The existing "No node selected" empty-state branch and the rest of the detail rendering stay unchanged. (Session-only children not present in `app.nodes` fall through to the empty state, which is acceptable for this pass; richer minimal-detail is a follow-up.)

- [ ] **Step 3: Update Network key handling in app.rs (`s`→`r`, nav over node rows)**

In `crates/zenmon-tui/src/app.rs`, replace the entire `ActiveView::Network => match key.code { ... }` arm inside `handle_view_key` with:

```rust
            ActiveView::Network => {
                let rows = crate::views::topology::build_topology_rows(
                    &self.nodes,
                    self.self_zid.as_deref(),
                    SystemTime::now(),
                );
                let total = crate::views::topology::node_row_count(&rows);
                match key.code {
                    KeyCode::Char('y') => {
                        if let Some(z) =
                            crate::views::topology::nth_node_zid(&rows, self.node_selected)
                        {
                            self.copy_to_clipboard(z.to_string(), "zid");
                        } else {
                            self.set_error_toast("No node selected");
                        }
                    }
                    KeyCode::Up | KeyCode::Char('k') => {
                        self.node_selected = self.node_selected.saturating_sub(1);
                        self.node_detail_scroll = 0;
                    }
                    KeyCode::Down | KeyCode::Char('j') => {
                        if self.node_selected + 1 < total {
                            self.node_selected += 1;
                            self.node_detail_scroll = 0;
                        }
                    }
                    KeyCode::Char('J') => {
                        self.node_detail_scroll = self.node_detail_scroll.saturating_add(3);
                    }
                    KeyCode::Char('K') => {
                        self.node_detail_scroll = self.node_detail_scroll.saturating_sub(3);
                    }
                    KeyCode::Char('r') => {
                        if !self.scout_in_progress {
                            self.pending_scout_request = true;
                        }
                    }
                    _ => {}
                }
            }
```

- [ ] **Step 4: Update Network click + wheel handling in app.rs**

In `handle_click`, the total-items match arm for Network currently reads `ActiveView::Network => self.nodes.len()`. Replace that single arm with a visual-row count, and replace the selection assignment.

Change the `total` computation arm:

```rust
            ActiveView::Network => crate::views::topology::build_topology_rows(
                &self.nodes,
                self.self_zid.as_deref(),
                SystemTime::now(),
            )
            .len(),
```

Change the click-selection arm from `ActiveView::Network => self.node_selected = idx,` to:

```rust
            ActiveView::Network => {
                let rows = crate::views::topology::build_topology_rows(
                    &self.nodes,
                    self.self_zid.as_deref(),
                    SystemTime::now(),
                );
                if let Some(n) = crate::views::topology::node_index_at_visual(&rows, idx) {
                    self.node_selected = n;
                    self.node_detail_scroll = 0;
                }
            }
```

In `handle_wheel_up`, the `ActiveView::Network` arm stays `self.node_selected = self.node_selected.saturating_sub(1);` (already correct).

In `handle_wheel_down`, replace the `ActiveView::Network` arm with:

```rust
            ActiveView::Network => {
                let total = crate::views::topology::node_row_count(
                    &crate::views::topology::build_topology_rows(
                        &self.nodes,
                        self.self_zid.as_deref(),
                        SystemTime::now(),
                    ),
                );
                if self.node_selected + 1 < total {
                    self.node_selected += 1;
                }
            }
```

- [ ] **Step 5: Add the empty-state hint to the tree**

In `render_topology` (network.rs), after computing `rows`, handle the empty case before rendering lines. Insert right after `let total_nodes = node_row_count(&rows);`:

```rust
    if rows.is_empty() {
        app.list_rect = Some(area);
        let hint = Paragraph::new(Line::from(Span::styled(
            "No nodes yet — press r to scout",
            Style::default().fg(Color::DarkGray),
        )))
        .block(Block::default().borders(Borders::ALL).title(" Topology "));
        frame.render_widget(hint, area);
        return;
    }
```

- [ ] **Step 6: Build and test**

Run: `cargo build -p zenmon-tui && cargo test -p zenmon-tui 2>&1 | grep "test result"`
Expected: build clean; `test result: ok. 30 passed` (22 existing + 8 topology).

- [ ] **Step 7: Manual smoke (optional if a router is running)**

Run: `cargo run -p zenmon-cli -- tui` in one terminal with `zenohd` running; press `5` (Network), confirm the tree renders and `j`/`k` moves selection, `r` triggers a scout. Press `q` to quit.

- [ ] **Step 8: Commit**

```bash
git add -A
git commit -m "feat(tui): render Network topology tree with r:refresh"
```

---

## Task 4: Switch Scouting Port modal + status-bar terminology

**Files:**
- Modify: `crates/zenmon-tui/src/app.rs` (`render_scout_port_modal`, `handle_scout_modal_key`, status bar, scout modal title)

**Interfaces:** none new; UI text + one behavior tweak (`s` always scans).

- [ ] **Step 1: Retitle the modal and drop "domain" language**

In `render_scout_port_modal` (app.rs), change the block title and the "Current" / list text.

Change the title:

```rust
            .title(" Switch Scouting Port ")
```

Change the `current_text` block to (no domain math):

```rust
        let current_text = match self.scout_port_current {
            Some(p) => format!("Current: {}", p),
            None => "Current: 7446 (default)".to_string(),
        };
```

Add a subtitle line clarifying the multicast address. Replace the `current_row` render with two stacked lines by rendering this before `input_row`:

```rust
        frame.render_widget(
            Paragraph::new("Zenoh multicast discovery: 224.0.0.224:<port>")
                .style(Style::default().fg(Color::DarkGray)),
            current_row,
        );
```

and move the `Current:` text into the `input_row`? No — keep layout simple: render the subtitle in `current_row` and the `Current:`/input in `input_row`. Concretely, replace the existing `current_row` and `input_row` render blocks with:

```rust
        frame.render_widget(
            Paragraph::new(current_text).style(Style::default().fg(Color::Gray)),
            current_row,
        );

        let input_text = if self.scout_port_input.is_empty() {
            "Go to port: _".to_string()
        } else {
            format!("Go to port: {}_", self.scout_port_input)
        };
        frame.render_widget(
            Paragraph::new(input_text).style(Style::default().fg(Color::Cyan)),
            input_row,
        );
```

- [ ] **Step 2: Drop "(domain N)" from the scanned-port list rows**

In `render_scout_port_modal`, the per-hit line currently formats `"(domain {})"`. Replace the `base_text` format with:

```rust
                        let base_text = format!(
                            "{}{:>5}   {} node(s)",
                            marker,
                            r.port,
                            r.nodes.len()
                        );
```

- [ ] **Step 3: Make `s` always scan (remove the empty-input coupling)**

In `handle_scout_modal_key`, replace the `KeyCode::Char('s')` arm with:

```rust
            KeyCode::Char('s') => {
                if !self.port_scan_in_progress {
                    self.pending_port_scan_request = true;
                }
            }
```

- [ ] **Step 4: Update the scanning-progress and hint text**

In `render_scout_port_modal`, change the "Scanning ports" and "Press 's'" strings to keep them port-framed (they already are) and update the hint row:

```rust
        frame.render_widget(
            Paragraph::new(" s:scan  jk/↑↓:select  Enter:switch  Esc:close ")
                .style(Style::default().fg(Color::DarkGray)),
            hint_row,
        );
```

- [ ] **Step 5: Update the status bar hint (`r`/`P`)**

In `render`, change the status-bar hint span text from `" q:quit  1-6:view  /:filter  y:copy  P:port  m:mode "` to:

```rust
                " q:quit  1-6:view  /:filter  y:copy  r:refresh  P:port  m:mode ",
```

- [ ] **Step 6: Build and test**

Run: `cargo build -p zenmon-tui && cargo test -p zenmon-tui 2>&1 | grep "test result"`
Expected: build clean; `test result: ok. 30 passed`.

- [ ] **Step 7: Commit**

```bash
git add -A
git commit -m "feat(tui): reframe scout-port modal as Switch Scouting Port (no domain)"
```

---

## Task 5: Final verification

**Files:** none unless a fix is needed.

- [ ] **Step 1: Full workspace build and test**

Run: `cargo build --release 2>&1 | tail -2 && cargo test 2>&1 | grep "test result"`
Expected: release build finishes; all test buckets pass (30 total: 8 core + 30 tui... i.e. 22 existing tui + 8 new topology = 30 tui; core 8). Confirm no `FAILED`.

- [ ] **Step 2: Terminology gate — no "domain" in TUI code**

Run: `grep -rni "domain" crates/zenmon-tui/src && echo "FOUND" || echo "CLEAN"`
Expected: `CLEAN` (no matches). If any appear, fix them (they violate the Global Constraint).

- [ ] **Step 3: Smoke — TUI launches and Network tab responds**

With `zenohd` running:

```bash
cat > /tmp/zenmon_network_smoke.exp <<'EOF'
#!/usr/bin/expect -f
set timeout 10
set env(TERM) "xterm-256color"
spawn -noecho cargo run -p zenmon-cli -- --mode peer tui
sleep 4
send "5"
sleep 1
send "r"
sleep 2
send "j"
sleep 1
send "q"
expect eof
EOF
chmod +x /tmp/zenmon_network_smoke.exp
/tmp/zenmon_network_smoke.exp > /dev/null 2>/tmp/zenmon_network_smoke.err
echo "EXIT=$?"; cat /tmp/zenmon_network_smoke.err
```

Expected: `EXIT=0`, no stderr.

- [ ] **Step 4: No commit unless fixes were applied**

If steps 1–3 pass cleanly, no new commit. If fixes were needed, commit them:

```bash
git add -A
git commit -m "fix(tui): resolve verification findings for Network view"
```

---

## Self-Review Notes

**Spec coverage:**

| Spec section | Task |
|---|---|
| Nodes→Network 탭 승격 | Task 1 |
| 토폴로지 트리 모델 (build rules, edge cases) | Task 2 |
| 트리 렌더 (좌) + 상세 (우) | Task 3 (Steps 1–2) |
| 탐색·선택 (헤더 건너뛰기, 클릭 매핑) | Task 2 (helpers) + Task 3 (Steps 3–4) |
| scout 분리 `r`:refresh | Task 3 (Step 3) |
| Switch Scouting Port 모달 + 용어 | Task 4 |
| 빈 상태 안내 | Task 3 (Step 5) |
| 상태바 `scout:` / 힌트 | Task 3 (Step 1 title) + Task 4 (Step 5) |
| no-router / unlinked / self / stale | Task 2 (tests + impl) |
| 용어 정확성(no "domain") | Task 5 (Step 2 gate) |

**Type consistency:** `TopoRow`, `TopoNode`, `build_topology_rows`, `node_row_count`, `nth_node_zid`, `node_index_at_visual`, `visual_index_of_node` are spelled identically across Task 2 (definition) and Task 3 (use). `app.node_selected` semantics change (node-row index) is applied consistently in render, key, click, and wheel handlers.

**Known simplifications (documented, not gaps):** router↔router links are not drawn (routers appear only as roots); session-only children absent from `app.nodes` fall to the detail empty-state. Both are noted as acceptable for this pass; richer detail is a follow-up.
