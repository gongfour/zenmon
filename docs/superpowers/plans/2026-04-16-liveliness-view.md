# Liveliness View Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a `[6] Liveliness` TUI tab showing liveliness tokens grouped by prefix with a global event log, and clean up `[5] Nodes` to remove liveliness code.

**Architecture:** Split into 4 tasks: (1) add `LivelinessEventRecord` type, (2) update `App` state for 6 tabs + liveliness fields + event recording, (3) clean up Nodes view, (4) create Liveliness view. Each task produces a compilable, committable unit.

**Tech Stack:** Rust, ratatui, zemon-core types, zemon-tui views

---

### Task 1: Add LivelinessEventRecord type and record events in App

**Files:**
- Modify: `crates/zemon-tui/src/app.rs`

This task adds the event record type (kept in TUI since it's display-only), new App fields, and updates `handle_liveliness()` to record events.

- [ ] **Step 1: Add LivelinessEventRecord struct and new fields to App**

In `crates/zemon-tui/src/app.rs`, add after the `QueryStatus` enum (before `pub struct App`):

```rust
#[derive(Debug, Clone)]
pub struct LivelinessEventRecord {
    pub timestamp: Instant,
    pub is_join: bool,
    pub key_expr: String,
    pub node_name: String,
    pub group: String,
}

const LIVELINESS_EVENT_CAP: usize = 200;
```

Add these fields to `pub struct App` (after `liveliness_tokens`):

```rust
    pub liveliness_selected: usize,
    pub liveliness_events: VecDeque<LivelinessEventRecord>,
    pub liveliness_log_scroll: u16,
```

- [ ] **Step 2: Initialize new fields in App::new()**

In the `App::new()` initializer, after `liveliness_tokens: Vec::new(),` add:

```rust
            liveliness_selected: 0,
            liveliness_events: VecDeque::with_capacity(LIVELINESS_EVENT_CAP),
            liveliness_log_scroll: 0,
```

- [ ] **Step 3: Update handle_liveliness() to record events**

Replace the existing `handle_liveliness` method with:

```rust
    fn handle_liveliness(&mut self, event: zemon_core::types::LivelinessEvent) {
        use zemon_core::types::LivelinessEvent;
        let (token, is_join) = match event {
            LivelinessEvent::Join(t) => (t, true),
            LivelinessEvent::Leave(t) => (t, false),
        };

        // Record event
        let record = LivelinessEventRecord {
            timestamp: Instant::now(),
            is_join,
            key_expr: token.key_expr.clone(),
            node_name: token.node_name().unwrap_or_else(|| token.key_expr.clone()),
            group: token.group_prefix().unwrap_or_default(),
        };
        self.liveliness_events.push_front(record);
        if self.liveliness_events.len() > LIVELINESS_EVENT_CAP {
            self.liveliness_events.pop_back();
        }

        // Update token state
        if is_join {
            if let Some(existing) = self
                .liveliness_tokens
                .iter_mut()
                .find(|t| t.key_expr == token.key_expr)
            {
                existing.alive = true;
                existing.source_zid = token.source_zid.or(existing.source_zid.clone());
            } else {
                self.liveliness_tokens.push(token);
            }
        } else if let Some(existing) = self
            .liveliness_tokens
            .iter_mut()
            .find(|t| t.key_expr == token.key_expr)
        {
            existing.alive = false;
        }
    }
```

- [ ] **Step 4: Verify it compiles**

Run: `cargo check -p zemon-tui`
Expected: compiles with no errors (warnings about unused fields are OK)

- [ ] **Step 5: Commit**

```bash
git add crates/zemon-tui/src/app.rs
git commit -m "feat(tui): add LivelinessEventRecord and event logging to App state"
```

---

### Task 2: Expand App from 5 tabs to 6 — add ActiveView::Liveliness

**Files:**
- Modify: `crates/zemon-tui/src/app.rs`

This task adds the Liveliness variant to ActiveView, updates all tab-related code (TAB_TITLES, tab_rects, key handlers, click handlers, mouse handlers, render dispatch, status bar).

- [ ] **Step 1: Update TAB_TITLES and ActiveView**

Change the constant and enum:

```rust
const TAB_TITLES: [&str; 6] = ["Dashboard", "Topics", "Stream", "Query", "Nodes", "Liveliness"];
```

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActiveView {
    Dashboard,
    Topics,
    Stream,
    Query,
    Nodes,
    Liveliness,
}

impl ActiveView {
    pub fn index(&self) -> usize {
        match self {
            ActiveView::Dashboard => 0,
            ActiveView::Topics => 1,
            ActiveView::Stream => 2,
            ActiveView::Query => 3,
            ActiveView::Nodes => 4,
            ActiveView::Liveliness => 5,
        }
    }
}
```

- [ ] **Step 2: Update tab_rects array size**

In `pub struct App`, change:

```rust
    pub tab_rects: [Option<ratatui::layout::Rect>; 6],
```

In `App::new()`, change:

```rust
            tab_rects: [None; 6],
```

- [ ] **Step 3: Update tab_hit function signature**

Change the `tab_hit` function:

```rust
pub(crate) fn tab_hit(rects: &[Option<Rect>; 6], col: u16, row: u16) -> Option<usize> {
```

- [ ] **Step 4: Add key '6' to handle_key**

In `handle_key()`, after the `KeyCode::Char('5')` line, add:

```rust
                KeyCode::Char('6') => self.active_view = ActiveView::Liveliness,
```

- [ ] **Step 5: Add Liveliness to handle_click tab dispatch**

In `handle_click()`, change the tab match:

```rust
            self.active_view = match idx {
                0 => ActiveView::Dashboard,
                1 => ActiveView::Topics,
                2 => ActiveView::Stream,
                3 => ActiveView::Query,
                4 => ActiveView::Nodes,
                5 => ActiveView::Liveliness,
                _ => self.active_view,
            };
```

- [ ] **Step 6: Add Liveliness to handle_click list, handle_wheel_up, handle_wheel_down**

In `handle_click()`, in the `total` match, add:

```rust
            ActiveView::Liveliness => self.liveliness_tokens.len(),
```

In the selection match below it, add:

```rust
            ActiveView::Liveliness => {
                self.liveliness_selected = idx;
                self.liveliness_log_scroll = 0;
            }
```

In `handle_wheel_up()`, add:

```rust
            ActiveView::Liveliness => {
                self.liveliness_selected = self.liveliness_selected.saturating_sub(1);
            }
```

In `handle_wheel_down()`, add:

```rust
            ActiveView::Liveliness => {
                let max = self.liveliness_tokens.len().saturating_sub(1);
                if self.liveliness_selected < max {
                    self.liveliness_selected += 1;
                }
            }
```

- [ ] **Step 7: Add Liveliness key handler in handle_view_key**

In `handle_view_key()`, before the `_ => {}` catch-all, add:

```rust
            ActiveView::Liveliness => match key.code {
                KeyCode::Char('y') => {
                    if let Some(token) = self.liveliness_tokens.get(self.liveliness_selected) {
                        let text = token.key_expr.clone();
                        self.copy_to_clipboard(text, "key_expr");
                    }
                }
                KeyCode::Up | KeyCode::Char('k') => {
                    self.liveliness_selected = self.liveliness_selected.saturating_sub(1);
                    self.liveliness_log_scroll = 0;
                }
                KeyCode::Down | KeyCode::Char('j') => {
                    let max = self.liveliness_tokens.len().saturating_sub(1);
                    if self.liveliness_selected < max {
                        self.liveliness_selected += 1;
                        self.liveliness_log_scroll = 0;
                    }
                }
                KeyCode::Char('J') => {
                    self.liveliness_log_scroll = self.liveliness_log_scroll.saturating_add(3);
                }
                KeyCode::Char('K') => {
                    self.liveliness_log_scroll = self.liveliness_log_scroll.saturating_sub(3);
                }
                _ => {}
            },
```

- [ ] **Step 8: Add Liveliness to render dispatch**

In `render()`, add to the match:

```rust
            ActiveView::Liveliness => views::liveliness::render(self, frame, content_area),
```

- [ ] **Step 9: Update status bar hint text**

Change the status bar hint from `1-5:view` to `1-6:view`:

```rust
                " q:quit  1-6:view  /:filter  y:copy  P:port ",
```

- [ ] **Step 10: Create placeholder liveliness view module**

Create `crates/zemon-tui/src/views/liveliness.rs`:

```rust
use crate::app::App;
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::Frame;

pub fn render(app: &mut App, frame: &mut Frame, area: ratatui::layout::Rect) {
    let placeholder = Paragraph::new(Line::from(Span::styled(
        "Liveliness view — coming soon",
        Style::default().fg(Color::DarkGray),
    )))
    .block(Block::default().borders(Borders::ALL).title(" Liveliness "));
    frame.render_widget(placeholder, area);
}
```

Add to `crates/zemon-tui/src/views/mod.rs`:

```rust
pub mod liveliness;
```

- [ ] **Step 11: Verify it compiles**

Run: `cargo check`
Expected: compiles with no errors

- [ ] **Step 12: Commit**

```bash
git add crates/zemon-tui/src/app.rs crates/zemon-tui/src/views/liveliness.rs crates/zemon-tui/src/views/mod.rs
git commit -m "feat(tui): add [6] Liveliness tab with placeholder view"
```

---

### Task 3: Clean up Nodes view — remove liveliness code

**Files:**
- Modify: `crates/zemon-tui/src/views/nodes.rs`

Remove Name column, liveliness matching in build_row, Network Liveliness section in detail, render_liveliness_line helper. Keep `(self)` label.

- [ ] **Step 1: Restore node list header to 3 columns (no Name)**

In `render_node_list()`, change the header:

```rust
    let header = Row::new(vec![
        Cell::from("ZID"),
        Cell::from("Kind"),
        Cell::from("Source"),
    ])
```

Remove the `tokens` binding. Change the row mapping to remove `tokens` parameter:

```rust
    let rows: Vec<Row> = app
        .nodes
        .iter()
        .enumerate()
        .skip(app.list_scroll_offset)
        .take(visible)
        .map(|(i, node)| build_row(node, i == app.node_selected, now, self_zid))
        .collect();
```

Change widths back to 3 columns:

```rust
    let widths = [
        Constraint::Percentage(50),
        Constraint::Percentage(15),
        Constraint::Percentage(35),
    ];
```

- [ ] **Step 2: Simplify build_row — remove tokens parameter and Name cell**

Replace the entire `build_row` function with:

```rust
fn build_row<'a>(node: &'a NodeInfo, selected: bool, now: SystemTime, self_zid: Option<&str>) -> Row<'a> {
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

    let kind_cell = if selected {
        Cell::from(node.kind.clone())
    } else {
        let kind_style = match node.kind.as_str() {
            "router" => Style::default().fg(Color::Green),
            "peer" => Style::default().fg(Color::Blue),
            "client" => Style::default().fg(Color::Gray),
            _ => Style::default(),
        };
        Cell::from(node.kind.clone()).style(kind_style)
    };

    let (source_text, source_color) = source_badge(node.sources, stale);
    let source_cell = if selected {
        Cell::from(source_text)
    } else {
        Cell::from(source_text).style(Style::default().fg(source_color))
    };

    let is_self = self_zid.is_some_and(|z| z == node.zid);
    let zid_text = if is_self {
        format!("{} (self)", node.zid)
    } else {
        node.zid.clone()
    };

    Row::new(vec![
        Cell::from(zid_text),
        kind_cell,
        source_cell,
    ])
    .style(base_style)
}
```

- [ ] **Step 3: Remove liveliness sections from render_node_detail**

Remove the entire "Liveliness tokens" block (from `// Liveliness tokens` to the closing brace before `// Node name from liveliness`).

Remove the "Node name from liveliness" block. Replace the title logic with:

```rust
    let title = format!(
        " {} - {} ",
        &node.zid[..node.zid.len().min(16)],
        node.kind,
    );
```

- [ ] **Step 4: Remove render_liveliness_line helper function**

Delete the entire `render_liveliness_line` function.

- [ ] **Step 5: Remove unused import**

The `zemon_core::types::LivelinessToken` import in `build_row` is no longer needed. Verify there are no remaining references to liveliness types in nodes.rs. The file should only import from `zemon_core::types::{NodeInfo, NodeSources}`.

- [ ] **Step 6: Verify it compiles**

Run: `cargo check`
Expected: compiles with no errors

- [ ] **Step 7: Commit**

```bash
git add crates/zemon-tui/src/views/nodes.rs
git commit -m "refactor(tui): remove liveliness code from Nodes view, keep (self) label"
```

---

### Task 4: Implement Liveliness view rendering

**Files:**
- Modify: `crates/zemon-tui/src/views/liveliness.rs`

Three-panel layout: top-left token list with group headers, top-right token detail, bottom event log.

- [ ] **Step 1: Write the full liveliness view**

Replace `crates/zemon-tui/src/views/liveliness.rs` with:

```rust
use crate::app::{App, LivelinessEventRecord};
use ratatui::layout::{Constraint, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};
use ratatui::Frame;

pub fn render(app: &mut App, frame: &mut Frame, area: ratatui::layout::Rect) {
    let [top_area, log_area] =
        Layout::vertical([Constraint::Percentage(70), Constraint::Percentage(30)])
            .areas(area);

    let [list_area, detail_area] =
        Layout::horizontal([Constraint::Percentage(45), Constraint::Percentage(55)])
            .areas(top_area);

    render_token_list(app, frame, list_area);
    render_token_detail(app, frame, detail_area);
    render_event_log(app, frame, log_area);
}

/// Build a flat list of display rows: group headers + token rows.
/// Returns (rows, token_indices) where token_indices[visual_row] = Some(token_index) for token rows.
fn build_grouped_rows(app: &App) -> (Vec<GroupedRow>, usize) {
    let tokens = &app.liveliness_tokens;
    if tokens.is_empty() {
        return (Vec::new(), 0);
    }

    // Group tokens by prefix
    let mut groups: Vec<(String, Vec<(usize, &zemon_core::types::LivelinessToken)>)> = Vec::new();
    for (i, token) in tokens.iter().enumerate() {
        let group = token.group_prefix().unwrap_or_else(|| "(ungrouped)".to_string());
        if let Some(g) = groups.iter_mut().find(|(k, _)| *k == group) {
            g.1.push((i, token));
        } else {
            groups.push((group, vec![(i, token)]));
        }
    }
    groups.sort_by(|a, b| a.0.cmp(&b.0));

    let mut rows = Vec::new();
    let mut token_count = 0;
    for (group_name, group_tokens) in &groups {
        let alive = group_tokens.iter().filter(|(_, t)| t.alive).count();
        let total = group_tokens.len();
        rows.push(GroupedRow::Header {
            label: format!("── {} ({}/{}) ──", group_name, alive, total),
        });
        for (token_idx, token) in group_tokens {
            rows.push(GroupedRow::Token {
                token_idx: *token_idx,
                name: token.node_name().unwrap_or_else(|| token.key_expr.clone()),
                alive: token.alive,
            });
            token_count += 1;
        }
    }
    (rows, token_count)
}

enum GroupedRow {
    Header { label: String },
    Token { token_idx: usize, name: String, alive: bool },
}

fn render_token_list(app: &mut App, frame: &mut Frame, area: ratatui::layout::Rect) {
    app.list_rect = Some(area);
    app.list_first_item_row = area.y + 1;

    let (rows, _token_count) = build_grouped_rows(app);

    let alive_count = app.liveliness_tokens.iter().filter(|t| t.alive).count();
    let dead_count = app.liveliness_tokens.iter().filter(|t| !t.alive).count();
    let title = if dead_count > 0 {
        format!(" Liveliness ({} alive, {} dead) ", alive_count, dead_count)
    } else {
        format!(" Liveliness ({} alive) ", alive_count)
    };

    let visible_height = area.height.saturating_sub(2) as usize;
    let lines: Vec<Line> = rows
        .iter()
        .enumerate()
        .map(|(_i, row)| match row {
            GroupedRow::Header { label } => {
                Line::from(Span::styled(label, Style::default().fg(Color::DarkGray)))
            }
            GroupedRow::Token { token_idx, name, alive } => {
                let selected = *token_idx == app.liveliness_selected;
                let (icon, icon_style) = if *alive {
                    ("● ", Style::default().fg(Color::Green))
                } else {
                    ("○ ", Style::default().fg(Color::Red))
                };
                let name_style = if selected {
                    Style::default()
                        .fg(Color::Black)
                        .bg(Color::Cyan)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(Color::White)
                };
                let prefix = if selected { "> " } else { "  " };
                Line::from(vec![
                    Span::raw(prefix),
                    Span::styled(icon, if selected { name_style } else { icon_style }),
                    Span::styled(name, name_style),
                ])
            }
        })
        .collect();

    let paragraph = Paragraph::new(lines)
        .block(Block::default().borders(Borders::ALL).title(title));
    frame.render_widget(paragraph, area);
}

fn render_token_detail(app: &App, frame: &mut Frame, area: ratatui::layout::Rect) {
    let selected = app.liveliness_tokens.get(app.liveliness_selected);

    let Some(token) = selected else {
        let empty = Paragraph::new(Line::from(Span::styled(
            "No token selected",
            Style::default().fg(Color::DarkGray),
        )))
        .block(Block::default().borders(Borders::ALL).title(" Detail "));
        frame.render_widget(empty, area);
        return;
    };

    let name = token.node_name().unwrap_or_else(|| token.key_expr.clone());
    let group = token.group_prefix().unwrap_or_default();
    let (status_icon, status_text, status_color) = if token.alive {
        ("●", "alive", Color::Green)
    } else {
        ("○", "dead", Color::Red)
    };

    let lines = vec![
        Line::from(Span::styled(
            &name,
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        Line::from(vec![
            Span::styled("Key: ", Style::default().fg(Color::Gray)),
            Span::styled(&token.key_expr, Style::default().fg(Color::White)),
        ]),
        Line::from(vec![
            Span::styled("Group: ", Style::default().fg(Color::Gray)),
            Span::styled(&group, Style::default().fg(Color::White)),
        ]),
        Line::from(vec![
            Span::styled("Status: ", Style::default().fg(Color::Gray)),
            Span::styled(
                format!("{} {}", status_icon, status_text),
                Style::default().fg(status_color),
            ),
        ]),
    ];

    let title = format!(" {} ", name);
    let detail = Paragraph::new(lines)
        .block(Block::default().borders(Borders::ALL).title(title))
        .wrap(Wrap { trim: false });
    frame.render_widget(detail, area);
}

fn render_event_log(app: &App, frame: &mut Frame, area: ratatui::layout::Rect) {
    let title = format!(" Event Log ({}) - Shift+J/K:scroll ", app.liveliness_events.len());

    if app.liveliness_events.is_empty() {
        let empty = Paragraph::new(Line::from(Span::styled(
            "  No events yet — waiting for liveliness changes...",
            Style::default().fg(Color::DarkGray),
        )))
        .block(Block::default().borders(Borders::ALL).title(title));
        frame.render_widget(empty, area);
        return;
    }

    let now = std::time::Instant::now();
    let lines: Vec<Line> = app
        .liveliness_events
        .iter()
        .map(|evt| format_event_line(evt, now))
        .collect();

    let log = Paragraph::new(lines)
        .block(Block::default().borders(Borders::ALL).title(title))
        .scroll((app.liveliness_log_scroll, 0));
    frame.render_widget(log, area);
}

fn format_event_line<'a>(evt: &LivelinessEventRecord, now: std::time::Instant) -> Line<'a> {
    let ago = now.duration_since(evt.timestamp);
    let time_str = if ago.as_secs() < 60 {
        format!("{:>3}s ago", ago.as_secs())
    } else {
        format!("{:>2}m {:02}s", ago.as_secs() / 60, ago.as_secs() % 60)
    };

    let (kind_text, kind_color) = if evt.is_join {
        ("JOIN ", Color::Green)
    } else {
        ("LEAVE", Color::Red)
    };

    let group_suffix = if evt.group.is_empty() {
        String::new()
    } else {
        format!("  ({})", evt.group)
    };

    Line::from(vec![
        Span::styled(
            format!("  {} ", time_str),
            Style::default().fg(Color::DarkGray),
        ),
        Span::styled(
            format!("{} ", kind_text),
            Style::default().fg(kind_color),
        ),
        Span::styled(
            evt.node_name.clone(),
            Style::default().fg(Color::White),
        ),
        Span::styled(group_suffix, Style::default().fg(Color::DarkGray)),
    ])
}
```

- [ ] **Step 2: Verify it compiles**

Run: `cargo check`
Expected: compiles with no errors

- [ ] **Step 3: Build and manual test**

Run: `cargo build`

Then test manually:
```bash
# Terminal 1: ensure zenohd is running
# Terminal 2: start TUI
./target/debug/zemon tui
# Press '6' to switch to Liveliness tab
# Verify: token list shows grouped tokens with ● status
# Verify: selecting a token shows detail on right
# Verify: event log at bottom shows JOIN events
# Press j/k to navigate, Shift+J/K to scroll log
# Press y to copy key_expr
```

- [ ] **Step 4: Commit**

```bash
git add crates/zemon-tui/src/views/liveliness.rs
git commit -m "feat(tui): implement [6] Liveliness view with grouped list, detail, and event log"
```
