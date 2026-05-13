# zemon TUI Mouse Support Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add mouse-based navigation (tab click, list click, wheel scroll) and `y`/`Y` yank keys that copy payload/key_expr/zid to the system clipboard in the zemon TUI.

**Architecture:** Switch from `ratatui::init()` to manual crossterm setup so mouse capture can be enabled. Extend `AppEvent` with a `Mouse` variant. App stores per-frame layout rects set during `render()`, which `handle_click` uses for hit-testing. Replace `sub_scroll` with `sub_selected` cursor. Yank via `arboard` writes to the system clipboard and surfaces success/failure through a 2-second status-bar toast.

**Tech Stack:** Rust 1.75+, `ratatui`, `crossterm`, `arboard` 3.x (new), `tokio`.

**Design spec:** `docs/superpowers/specs/2026-04-15-tui-mouse-support-design.md`

---

## File Structure

**Modify:**
- `crates/zemon-tui/Cargo.toml` — add `arboard = "3"`
- `crates/zemon-tui/src/lib.rs` — manual terminal init/teardown with mouse capture + panic hook
- `crates/zemon-tui/src/event.rs` — `AppEvent::Mouse` variant, forward mouse events
- `crates/zemon-tui/src/app.rs` — rect fields, hit-test helpers, `handle_mouse`, `handle_click`, wheel routing, `sub_selected`, toast, yank logic, unit tests
- `crates/zemon-tui/src/views/topics.rs` — record `list_rect`
- `crates/zemon-tui/src/views/subscribe.rs` — `ListState::with_selected`, drop `sub_scroll`, record `list_rect`
- `crates/zemon-tui/src/views/query.rs` — record `list_rect`
- `crates/zemon-tui/src/views/nodes.rs` — record `list_rect`

No new files.

---

## Task 1: Add `arboard` Dependency

**Files:**
- Modify: `crates/zemon-tui/Cargo.toml`

- [ ] **Step 1: Add the dependency**

Open `crates/zemon-tui/Cargo.toml` and add under `[dependencies]`:

```toml
arboard = "3"
```

- [ ] **Step 2: Verify it compiles**

Run: `cargo check -p zemon-tui`
Expected: clean build, no errors. Warnings about unused import are fine since we don't use it yet.

- [ ] **Step 3: Commit**

```bash
git add crates/zemon-tui/Cargo.toml Cargo.lock
git commit -m "chore(tui): add arboard dependency for clipboard"
```

---

## Task 2: Switch to Manual Terminal Init with Mouse Capture

**Files:**
- Modify: `crates/zemon-tui/src/lib.rs`

- [ ] **Step 1: Replace `ratatui::init()` with manual setup**

In `crates/zemon-tui/src/lib.rs`, add imports near the top (after existing `use` statements):

```rust
use crossterm::event::{DisableMouseCapture, EnableMouseCapture};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;
```

Replace the `let mut terminal = ratatui::init();` line in `run()` with:

```rust
enable_raw_mode()?;
let mut stdout = std::io::stdout();
execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
let backend = CrosstermBackend::new(stdout);
let mut terminal = Terminal::new(backend)?;

// Restore terminal on panic
let original_hook = std::panic::take_hook();
std::panic::set_hook(Box::new(move |info| {
    let _ = disable_raw_mode();
    let _ = execute!(
        std::io::stdout(),
        LeaveAlternateScreen,
        DisableMouseCapture
    );
    original_hook(info);
}));
```

Replace `ratatui::restore();` with:

```rust
disable_raw_mode()?;
execute!(
    std::io::stdout(),
    LeaveAlternateScreen,
    DisableMouseCapture
)?;
```

Also update the `run_loop` signature from `terminal: &mut ratatui::DefaultTerminal` to:

```rust
terminal: &mut Terminal<CrosstermBackend<std::io::Stdout>>,
```

- [ ] **Step 2: Verify compilation**

Run: `cargo build -p zemon-tui`
Expected: clean build.

- [ ] **Step 3: Smoke test**

Run: `cargo run -- tui` in one terminal, press `q` to quit.
Expected: TUI opens, alternate screen shows, `q` quits cleanly and terminal returns to normal. Mouse cursor behavior in the terminal may differ (cells highlight instead of text selection) — this is expected.

- [ ] **Step 4: Commit**

```bash
git add crates/zemon-tui/src/lib.rs
git commit -m "feat(tui): enable mouse capture with manual terminal init"
```

---

## Task 3: Add `AppEvent::Mouse` Variant

**Files:**
- Modify: `crates/zemon-tui/src/event.rs`

- [ ] **Step 1: Add `Mouse` variant and forward events**

In `crates/zemon-tui/src/event.rs`, update imports:

```rust
use crossterm::event::{EventStream, KeyEvent, KeyEventKind, MouseEvent};
```

Add a `Mouse` variant to the enum:

```rust
#[derive(Clone, Debug)]
pub enum AppEvent {
    Key(KeyEvent),
    Mouse(MouseEvent),
    Zenoh(ZenohMessage),
    Tick,
}
```

In the `EventStream` branch inside `tokio::spawn`, extend the event dispatch:

```rust
Some(Ok(evt)) => {
    match evt {
        crossterm::event::Event::Key(key) => {
            if key.kind == KeyEventKind::Press
                && tx.send(AppEvent::Key(key)).is_err()
            {
                break;
            }
        }
        crossterm::event::Event::Mouse(m) => {
            if tx.send(AppEvent::Mouse(m)).is_err() {
                break;
            }
        }
        _ => {}
    }
}
```

- [ ] **Step 2: Add a no-op handler in `App::handle_event`**

In `crates/zemon-tui/src/app.rs`, update `handle_event`:

```rust
pub fn handle_event(&mut self, event: AppEvent) {
    match event {
        AppEvent::Key(key) => self.handle_key(key),
        AppEvent::Mouse(_) => {} // wired in Task 5
        AppEvent::Zenoh(msg) => self.handle_zenoh_message(msg),
        AppEvent::Tick => {
            self.update_hz();
        }
    }
}
```

- [ ] **Step 3: Verify compilation**

Run: `cargo build -p zemon-tui`
Expected: clean build.

- [ ] **Step 4: Commit**

```bash
git add crates/zemon-tui/src/event.rs crates/zemon-tui/src/app.rs
git commit -m "feat(tui): forward mouse events through AppEvent"
```

---

## Task 4: Hit-test Helpers with Unit Tests

This task introduces pure functions for hit-testing, fully unit-tested before any UI wiring.

**Files:**
- Modify: `crates/zemon-tui/src/app.rs`
- Test: `crates/zemon-tui/src/app.rs` (inline `#[cfg(test)]` module)

- [ ] **Step 1: Write the failing tests**

At the end of `crates/zemon-tui/src/app.rs`, add:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::layout::Rect;

    #[test]
    fn tab_hit_inside_rect_returns_index() {
        let rects = [
            Some(Rect::new(1, 0, 14, 3)),
            Some(Rect::new(16, 0, 10, 3)),
            Some(Rect::new(28, 0, 12, 3)),
            None,
            None,
        ];
        assert_eq!(tab_hit(&rects, 2, 1), Some(0));
        assert_eq!(tab_hit(&rects, 20, 1), Some(1));
        assert_eq!(tab_hit(&rects, 30, 2), Some(2));
    }

    #[test]
    fn tab_hit_outside_returns_none() {
        let rects = [
            Some(Rect::new(1, 0, 14, 3)),
            None,
            None,
            None,
            None,
        ];
        assert_eq!(tab_hit(&rects, 50, 1), None);
        assert_eq!(tab_hit(&rects, 2, 5), None);
    }

    #[test]
    fn list_hit_converts_row_to_index() {
        // list_rect at (0,5) 20x10, first item row 6 (border), 0 scroll
        let rect = Rect::new(0, 5, 20, 10);
        // Click on row 6 → index 0
        assert_eq!(list_hit(rect, 6, 0, 10, 5), Some(0));
        // Click on row 8 → index 2
        assert_eq!(list_hit(rect, 8, 0, 10, 5), Some(2));
        // Click on border row 5 → None (above first item)
        assert_eq!(list_hit(rect, 5, 0, 10, 5), None);
        // Click outside rect → None
        assert_eq!(list_hit(rect, 20, 0, 10, 5), None);
        // Click beyond item count → None
        assert_eq!(list_hit(rect, 13, 0, 10, 5), None);
    }

    #[test]
    fn list_hit_respects_scroll_offset() {
        let rect = Rect::new(0, 5, 20, 10);
        // first item row 6, scroll offset 4, 20 items total
        assert_eq!(list_hit(rect, 6, 4, 20, 5), Some(4));
        assert_eq!(list_hit(rect, 9, 4, 20, 5), Some(7));
    }
}
```

- [ ] **Step 2: Run tests — verify they fail with "cannot find function"**

Run: `cargo test -p zemon-tui`
Expected: compile errors — `tab_hit` and `list_hit` not found.

- [ ] **Step 3: Implement the hit-test functions**

Near the top of `crates/zemon-tui/src/app.rs` (below the `use` statements), add:

```rust
use ratatui::layout::Rect;

/// Return the tab index hit by a click at `(col, row)`, or `None`.
pub(crate) fn tab_hit(rects: &[Option<Rect>; 5], col: u16, row: u16) -> Option<usize> {
    for (i, maybe) in rects.iter().enumerate() {
        if let Some(r) = maybe {
            if col >= r.x && col < r.x + r.width && row >= r.y && row < r.y + r.height {
                return Some(i);
            }
        }
    }
    None
}

/// Return the list item index hit by a click, or `None`.
/// `first_item_row` is the absolute screen row where item 0 is rendered
/// (typically `rect.y + 1` to skip the top border).
/// `scroll_offset` is the number of items skipped before rendering (for scrolled lists).
/// `total_items` is the current item count (used to reject clicks past the end).
pub(crate) fn list_hit(
    rect: Rect,
    click_row: u16,
    scroll_offset: usize,
    total_items: usize,
    first_item_row: u16,
) -> Option<usize> {
    // Note: signature exposes rect for outside-rect checks; we still need col check via caller
    // but here we treat any row inside [first_item_row, rect.bottom()) as a hit row.
    if click_row < first_item_row || click_row >= rect.y + rect.height {
        return None;
    }
    let row_in_list = (click_row - first_item_row) as usize;
    let idx = row_in_list + scroll_offset;
    if idx >= total_items {
        return None;
    }
    Some(idx)
}
```

Note: the column check for lists is handled by the caller (using `rect.x`/`rect.width`) before calling `list_hit`. Adjust the test if you prefer adding a column parameter; as written, the tests pass clicks within a single column which is sufficient for these pure tests.

- [ ] **Step 4: Run tests — verify they pass**

Run: `cargo test -p zemon-tui`
Expected: all 4 tests pass.

- [ ] **Step 5: Commit**

```bash
git add crates/zemon-tui/src/app.rs
git commit -m "feat(tui): add pure hit-test helpers with unit tests"
```

---

## Task 5: Tab Click Wiring

**Files:**
- Modify: `crates/zemon-tui/src/app.rs`

- [ ] **Step 1: Add tab rect fields to `App`**

In `crates/zemon-tui/src/app.rs`, add to the `App` struct (below `endpoint` field):

```rust
pub tab_rects: [Option<ratatui::layout::Rect>; 5],
```

And in `App::new`, initialize:

```rust
tab_rects: [None; 5],
```

- [ ] **Step 2: Record tab rects in `render()`**

Replace the `tabs` widget construction in `render()` (currently using `Tabs::new(...)`) with manual rendering that tracks per-tab rects. Replace the block that builds and renders `tabs` with:

```rust
// Render tabs manually so we can record per-tab rects for hit-testing.
let tabs_block = Block::default().borders(Borders::ALL).title(" zemon ");
let inner = tabs_block.inner(tabs_area);
frame.render_widget(tabs_block, tabs_area);

let divider_width: u16 = 2;
let mut x = inner.x;
for (i, title) in TAB_TITLES.iter().enumerate() {
    let label = format!("[{}] {}", i + 1, title);
    let label_width = label.chars().count() as u16;
    if x + label_width > inner.x + inner.width {
        self.tab_rects[i] = None;
        continue;
    }
    let rect = ratatui::layout::Rect::new(x, inner.y, label_width, inner.height);
    self.tab_rects[i] = Some(rect);
    let style = if i == self.active_view.index() {
        Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD | Modifier::UNDERLINED)
    } else {
        Style::default().fg(Color::White)
    };
    let para = ratatui::widgets::Paragraph::new(Span::styled(label, style));
    frame.render_widget(para, rect);
    x += label_width + divider_width;
}
```

Remove the old `let tabs = Tabs::new(...)` block and its `frame.render_widget(tabs, tabs_area);` call.

- [ ] **Step 3: Wire `handle_mouse` and `handle_click`**

Add imports at the top of `app.rs`:

```rust
use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
```

Update `handle_event` to dispatch:

```rust
pub fn handle_event(&mut self, event: AppEvent) {
    match event {
        AppEvent::Key(key) => self.handle_key(key),
        AppEvent::Mouse(m) => self.handle_mouse(m),
        AppEvent::Zenoh(msg) => self.handle_zenoh_message(msg),
        AppEvent::Tick => {
            self.update_hz();
        }
    }
}
```

Add methods to `impl App`:

```rust
fn handle_mouse(&mut self, ev: MouseEvent) {
    if self.is_text_input_active() {
        return;
    }
    match ev.kind {
        MouseEventKind::Down(MouseButton::Left) => self.handle_click(ev.column, ev.row),
        MouseEventKind::ScrollUp => self.handle_wheel_up(),
        MouseEventKind::ScrollDown => self.handle_wheel_down(),
        _ => {}
    }
}

fn handle_click(&mut self, col: u16, row: u16) {
    if let Some(idx) = tab_hit(&self.tab_rects, col, row) {
        self.active_view = match idx {
            0 => ActiveView::Dashboard,
            1 => ActiveView::Topics,
            2 => ActiveView::Subscribe,
            3 => ActiveView::Query,
            4 => ActiveView::Nodes,
            _ => self.active_view,
        };
    }
    // list-area clicks added in Task 7
}

fn handle_wheel_up(&mut self) {
    // wired in Task 8
}

fn handle_wheel_down(&mut self) {
    // wired in Task 8
}
```

- [ ] **Step 4: Manual test**

Run: `cargo run -- tui`
Click each tab header: expected — view switches. Press `q` to quit.

- [ ] **Step 5: Commit**

```bash
git add crates/zemon-tui/src/app.rs
git commit -m "feat(tui): wire mouse clicks to tab switching"
```

---

## Task 6: Replace `sub_scroll` with `sub_selected` (with Tests)

**Files:**
- Modify: `crates/zemon-tui/src/app.rs`
- Modify: `crates/zemon-tui/src/views/subscribe.rs`

- [ ] **Step 1: Write failing tests for cursor maintenance**

Append to the `#[cfg(test)] mod tests` block in `app.rs`:

```rust
#[test]
fn sub_selected_zero_stays_on_new_message() {
    let mut app = App::new("test".into());
    app.sub_selected = 0;
    let msg = ZenohMessage {
        key_expr: "a".into(),
        payload: zemon_core::types::MessagePayload::Json(serde_json::json!(null)),
        timestamp: None,
        kind: "put".into(),
        attachment: None,
    };
    app.handle_zenoh_message(msg);
    assert_eq!(app.sub_selected, 0, "cursor at top should stay at top");
}

#[test]
fn sub_selected_nonzero_follows_message_through_shift() {
    let mut app = App::new("test".into());
    let make = |k: &str| ZenohMessage {
        key_expr: k.into(),
        payload: zemon_core::types::MessagePayload::Json(serde_json::json!(null)),
        timestamp: None,
        kind: "put".into(),
        attachment: None,
    };
    app.handle_zenoh_message(make("a"));
    app.handle_zenoh_message(make("b"));
    app.handle_zenoh_message(make("c"));
    // messages stored with push_front: [c, b, a]
    app.sub_selected = 1; // pointing at "b"
    app.handle_zenoh_message(make("d")); // now [d, c, b, a]
    assert_eq!(app.sub_selected, 2, "cursor follows 'b' to new index 2");
}
```

- [ ] **Step 2: Run tests — verify they fail**

Run: `cargo test -p zemon-tui`
Expected: test failures — `sub_selected` field doesn't exist or logic is wrong.

- [ ] **Step 3: Replace `sub_scroll` with `sub_selected`**

In `crates/zemon-tui/src/app.rs`:

Remove the field `pub sub_scroll: u16,` and add:

```rust
pub sub_selected: usize,
```

In `App::new`, replace `sub_scroll: 0,` with:

```rust
sub_selected: 0,
```

In `handle_zenoh_message`, update the subscribe branch:

```rust
if !self.sub_paused {
    self.sub_messages.push_front(msg);
    if self.sub_messages.len() > 500 {
        self.sub_messages.pop_back();
    }
    // Maintain cursor: if we're not at the top, follow the message as it shifts down.
    if self.sub_selected > 0 && self.sub_selected < self.sub_messages.len() {
        self.sub_selected += 1;
    }
    // Clamp to current length
    if self.sub_selected >= self.sub_messages.len() && !self.sub_messages.is_empty() {
        self.sub_selected = self.sub_messages.len() - 1;
    }
}
```

In `handle_view_key`, replace the `ActiveView::Subscribe` match arm:

```rust
ActiveView::Subscribe => match key.code {
    KeyCode::Char(' ') => self.sub_paused = !self.sub_paused,
    KeyCode::Up | KeyCode::Char('k') => {
        self.sub_selected = self.sub_selected.saturating_sub(1);
    }
    KeyCode::Down | KeyCode::Char('j') => {
        let max = self.sub_messages.len().saturating_sub(1);
        if self.sub_selected < max {
            self.sub_selected += 1;
        }
    }
    _ => {}
},
```

- [ ] **Step 4: Update `subscribe.rs` to use `ListState::with_selected`**

In `crates/zemon-tui/src/views/subscribe.rs`:

Change the import line to include `ListState` and make the function take `&mut App` (so we can render stateful):

```rust
use crate::app::App;
use zemon_core::types::MessagePayload;
use ratatui::layout::{Constraint, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, ListState, Paragraph};
use ratatui::Frame;

pub fn render(app: &mut App, frame: &mut Frame, area: ratatui::layout::Rect) {
```

Replace the body of the items construction. Delete the `let scroll = app.sub_scroll as usize;` line and the `.skip(scroll).take(messages_area.height as usize)` chain. Replace with:

```rust
    let items: Vec<ListItem> = app
        .sub_messages
        .iter()
        .map(|msg| {
            let payload_str = match &msg.payload {
                MessagePayload::Json(v) => {
                    serde_json::to_string_pretty(v).unwrap_or_else(|_| format!("{}", v))
                }
                other => format!("{}", other),
            };
            let att_str = msg.attachment.as_ref().map(|a| format!(" att:{}", a));
            let ts = msg.timestamp.as_deref().unwrap_or("");
            let mut spans = vec![
                Span::styled(
                    &msg.key_expr,
                    Style::default()
                        .fg(Color::Cyan)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(format!(" [{}]", ts), Style::default().fg(Color::DarkGray)),
                Span::raw(" "),
                Span::styled(payload_str, Style::default().fg(Color::White)),
            ];
            if let Some(att) = att_str {
                spans.push(Span::styled(att, Style::default().fg(Color::Magenta)));
            }
            ListItem::new(Line::from(spans))
        })
        .collect();

    let list = List::new(items)
        .block(Block::default().borders(Borders::ALL).title(" Messages "))
        .highlight_style(
            Style::default()
                .bg(Color::DarkGray)
                .add_modifier(Modifier::BOLD),
        );

    let mut state = ListState::default().with_selected(Some(app.sub_selected));
    frame.render_stateful_widget(list, messages_area, &mut state);
```

- [ ] **Step 5: Update callers to pass `&mut App`**

In `crates/zemon-tui/src/app.rs`, `App::render` currently calls:

```rust
ActiveView::Subscribe => views::subscribe::render(self, frame, content_area),
```

`self` is already `&mut self` in `render`, so the call is fine. Verify the other view renders (`topics::render`, etc.) take `&App` — those are unchanged for now.

- [ ] **Step 6: Run tests and build**

Run: `cargo test -p zemon-tui && cargo build -p zemon-tui`
Expected: all tests pass, clean build.

- [ ] **Step 7: Manual test**

Run: `cargo run -- tui`, switch to Subscribe (press `3`), publish messages from another terminal with `./target/release/zemon pub test/hello '{"msg":"1"}'` a few times, use j/k to move cursor.
Expected: highlighted row moves with j/k, new messages push the cursor down if not at top.

- [ ] **Step 8: Commit**

```bash
git add crates/zemon-tui/src/app.rs crates/zemon-tui/src/views/subscribe.rs
git commit -m "feat(tui): replace sub_scroll with sub_selected cursor"
```

---

## Task 7: Record List Rects and Wire List Clicks

**Files:**
- Modify: `crates/zemon-tui/src/app.rs`
- Modify: `crates/zemon-tui/src/views/topics.rs`
- Modify: `crates/zemon-tui/src/views/subscribe.rs`
- Modify: `crates/zemon-tui/src/views/query.rs`
- Modify: `crates/zemon-tui/src/views/nodes.rs`

- [ ] **Step 1: Add list rect fields**

In `crates/zemon-tui/src/app.rs`, add to the `App` struct:

```rust
pub list_rect: Option<ratatui::layout::Rect>,
pub list_first_item_row: u16,
pub list_scroll_offset: usize,
```

Initialize in `App::new`:

```rust
list_rect: None,
list_first_item_row: 0,
list_scroll_offset: 0,
```

- [ ] **Step 2: Change view render signatures to `&mut App`**

Each view's `render` must be able to write to `app.list_rect`. Update signatures:

`topics.rs`:
```rust
pub fn render(app: &mut App, frame: &mut Frame, area: ratatui::layout::Rect) {
```

`query.rs`:
```rust
pub fn render(app: &mut App, frame: &mut Frame, area: ratatui::layout::Rect) {
```

`nodes.rs`:
```rust
pub fn render(app: &mut App, frame: &mut Frame, area: ratatui::layout::Rect) {
```

`subscribe.rs` already is `&mut App` from Task 6.

`dashboard.rs` — leave as `&App` (no list).

- [ ] **Step 3: Record `list_rect` in each view**

In `topics.rs` at the start of `render`, after `let [list_area, detail_area] = ...` but before rendering the list, reset and record:

```rust
    app.list_rect = Some(list_area);
    app.list_first_item_row = list_area.y + 1; // border
    app.list_scroll_offset = 0;
```

In `subscribe.rs`, after `let [status_area, messages_area] = ...` and before rendering the list:

```rust
    app.list_rect = Some(messages_area);
    app.list_first_item_row = messages_area.y + 1;
    app.list_scroll_offset = 0; // List renders from top; ListState handles selection
```

In `query.rs`, before rendering `results`:

```rust
    app.list_rect = Some(results_area);
    app.list_first_item_row = results_area.y + 1;
    app.list_scroll_offset = 0;
```

In `nodes.rs`, at the start:

```rust
    app.list_rect = Some(area);
    // Nodes table has header (1 row) + bottom margin (1 row) + border, so first data row is y + 3
    app.list_first_item_row = area.y + 3;
    app.list_scroll_offset = 0;
```

And in `dashboard.rs`, at the start:

```rust
    app.list_rect = None;
```

(This requires `dashboard.rs` to also take `&mut App` — update its signature too.)

- [ ] **Step 4: Update `App::render` to pass `&mut self` to view renders**

In `crates/zemon-tui/src/app.rs`, the match in `render()` should call each view:

```rust
match self.active_view {
    ActiveView::Dashboard => views::dashboard::render(self, frame, content_area),
    ActiveView::Topics => views::topics::render(self, frame, content_area),
    ActiveView::Subscribe => views::subscribe::render(self, frame, content_area),
    ActiveView::Query => views::query::render(self, frame, content_area),
    ActiveView::Nodes => views::nodes::render(self, frame, content_area),
}
```

All calls already use `self` — just confirm `self: &mut self` in `render` (it already is).

- [ ] **Step 5: Extend `handle_click` with list hit-testing**

In `app.rs`, update `handle_click`:

```rust
fn handle_click(&mut self, col: u16, row: u16) {
    if let Some(idx) = tab_hit(&self.tab_rects, col, row) {
        self.active_view = match idx {
            0 => ActiveView::Dashboard,
            1 => ActiveView::Topics,
            2 => ActiveView::Subscribe,
            3 => ActiveView::Query,
            4 => ActiveView::Nodes,
            _ => self.active_view,
        };
        return;
    }

    // List-area click
    let Some(rect) = self.list_rect else { return };
    if col < rect.x || col >= rect.x + rect.width {
        return;
    }
    let total = match self.active_view {
        ActiveView::Topics => self.filtered_topics().len(),
        ActiveView::Subscribe => self.sub_messages.len(),
        ActiveView::Query => self.query_results.len(),
        ActiveView::Nodes => self.nodes.len(),
        ActiveView::Dashboard => return,
    };
    let Some(idx) = list_hit(
        rect,
        row,
        self.list_scroll_offset,
        total,
        self.list_first_item_row,
    ) else {
        return;
    };
    match self.active_view {
        ActiveView::Topics => {
            self.topic_selected = idx;
            self.topic_detail_scroll = 0;
        }
        ActiveView::Subscribe => self.sub_selected = idx,
        ActiveView::Query => {
            // query_results currently has no selection cursor — add one in Task 9
            self.query_selected = idx;
        }
        ActiveView::Nodes => self.node_selected = idx,
        ActiveView::Dashboard => {}
    }
}
```

- [ ] **Step 6: Add `query_selected` field**

Because the handler above references `self.query_selected`, add to `App` struct:

```rust
pub query_selected: usize,
```

Initialize in `App::new`:

```rust
query_selected: 0,
```

Also update `handle_view_key`'s `ActiveView::Query` arm to move the cursor with j/k. Replace it with:

```rust
ActiveView::Query => match key.code {
    KeyCode::Char('/') | KeyCode::Char('i') => self.query_editing = true,
    KeyCode::Down | KeyCode::Char('j') => {
        let max = self.query_results.len().saturating_sub(1);
        if self.query_selected < max {
            self.query_selected += 1;
        }
    }
    KeyCode::Up | KeyCode::Char('k') => {
        self.query_selected = self.query_selected.saturating_sub(1);
    }
    _ => {}
},
```

(Note: the old behavior of `k` to recall last query is removed — query recall now happens only inside the input mode via Up key, which is not yet implemented. If the user relies on `k`-recall, that can be restored later; for now the simpler j/k navigation is preferred because it's consistent with the other views.)

In `query.rs`, render the results with `ListState::with_selected(Some(app.query_selected))`:

Change the `List::new(result_items)` rendering block to:

```rust
    let result_count = result_items.len();
    let results = List::new(result_items)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(format!(" Results ({}) ", result_count)),
        )
        .highlight_style(
            Style::default()
                .bg(Color::DarkGray)
                .add_modifier(Modifier::BOLD),
        );
    let mut results_state = ListState::default().with_selected(Some(app.query_selected));
    frame.render_stateful_widget(results, results_area, &mut results_state);
```

And add `ListState` to the imports at the top of `query.rs`.

- [ ] **Step 7: Build and run**

Run: `cargo build -p zemon-tui`
Expected: clean build.

Run: `cargo run -- tui`
Click on topic list items, node rows, subscribe messages, query results (after running a query).
Expected: clicked item becomes highlighted/selected.

- [ ] **Step 8: Commit**

```bash
git add crates/zemon-tui/src/app.rs crates/zemon-tui/src/views/
git commit -m "feat(tui): wire list-area clicks to item selection"
```

---

## Task 8: Wheel Scroll

**Files:**
- Modify: `crates/zemon-tui/src/app.rs`

- [ ] **Step 1: Implement wheel handlers**

In `crates/zemon-tui/src/app.rs`, replace the stub `handle_wheel_up` / `handle_wheel_down` with:

```rust
fn handle_wheel_up(&mut self) {
    match self.active_view {
        ActiveView::Topics => {
            self.topic_selected = self.topic_selected.saturating_sub(1);
            self.topic_detail_scroll = 0;
        }
        ActiveView::Subscribe => {
            self.sub_selected = self.sub_selected.saturating_sub(1);
        }
        ActiveView::Query => {
            self.query_selected = self.query_selected.saturating_sub(1);
        }
        ActiveView::Nodes => {
            self.node_selected = self.node_selected.saturating_sub(1);
        }
        ActiveView::Dashboard => {}
    }
}

fn handle_wheel_down(&mut self) {
    match self.active_view {
        ActiveView::Topics => {
            let max = self.filtered_topics().len().saturating_sub(1);
            if self.topic_selected < max {
                self.topic_selected += 1;
                self.topic_detail_scroll = 0;
            }
        }
        ActiveView::Subscribe => {
            let max = self.sub_messages.len().saturating_sub(1);
            if self.sub_selected < max {
                self.sub_selected += 1;
            }
        }
        ActiveView::Query => {
            let max = self.query_results.len().saturating_sub(1);
            if self.query_selected < max {
                self.query_selected += 1;
            }
        }
        ActiveView::Nodes => {
            let max = self.nodes.len().saturating_sub(1);
            if self.node_selected < max {
                self.node_selected += 1;
            }
        }
        ActiveView::Dashboard => {}
    }
}
```

- [ ] **Step 2: Manual test**

Run: `cargo run -- tui`. Open Topics / Nodes / Subscribe / Query, scroll with the mouse wheel.
Expected: cursor moves up/down by one item per wheel click.

- [ ] **Step 3: Commit**

```bash
git add crates/zemon-tui/src/app.rs
git commit -m "feat(tui): handle mouse wheel scroll in list views"
```

---

## Task 9: Toast Notification Infrastructure

**Files:**
- Modify: `crates/zemon-tui/src/app.rs`

- [ ] **Step 1: Add toast field**

In the `App` struct, add:

```rust
pub toast: Option<(String, Instant)>,
pub toast_is_error: bool,
```

Initialize in `App::new`:

```rust
toast: None,
toast_is_error: false,
```

- [ ] **Step 2: Add helper methods**

In `impl App`:

```rust
pub fn set_toast(&mut self, msg: impl Into<String>) {
    self.toast = Some((msg.into(), Instant::now()));
    self.toast_is_error = false;
}

pub fn set_error_toast(&mut self, msg: impl Into<String>) {
    self.toast = Some((msg.into(), Instant::now()));
    self.toast_is_error = true;
}
```

- [ ] **Step 3: Render toast in the status bar**

In `App::render()`, replace the status bar composition block. Currently it computes `mode_text` and builds a `status` Line. Replace with:

```rust
    let toast_expired = self
        .toast
        .as_ref()
        .map(|(_, t)| t.elapsed().as_secs() >= 2)
        .unwrap_or(true);
    if toast_expired {
        self.toast = None;
    }

    let middle_span = if let Some((msg, _)) = &self.toast {
        let style = if self.toast_is_error {
            Style::default().fg(Color::White).bg(Color::Red)
        } else {
            Style::default().fg(Color::Black).bg(Color::Green)
        };
        Span::styled(format!(" {} ", msg), style)
    } else if self.is_text_input_active() {
        Span::styled(" INPUT ", Style::default().fg(Color::Cyan))
    } else {
        Span::styled(" NORMAL ", Style::default().fg(Color::Cyan))
    };

    let status = Line::from(vec![
        Span::styled(conn_text, conn_style),
        Span::styled(
            format!(" {} ", self.endpoint),
            Style::default().fg(Color::Gray),
        ),
        middle_span,
        Span::styled(
            " q:quit  1-5:view  /:filter  y:copy ",
            Style::default().fg(Color::DarkGray),
        ),
    ]);
    frame.render_widget(status, status_area);
```

- [ ] **Step 4: Build**

Run: `cargo build -p zemon-tui`
Expected: clean build.

- [ ] **Step 5: Commit**

```bash
git add crates/zemon-tui/src/app.rs
git commit -m "feat(tui): add 2s toast notifications in status bar"
```

---

## Task 10: Yank (Clipboard Copy) Keys

**Files:**
- Modify: `crates/zemon-tui/src/app.rs`

- [ ] **Step 1: Add yank helpers**

In `crates/zemon-tui/src/app.rs`, add imports:

```rust
use zemon_core::types::MessagePayload;
```

Add a pure serialization helper:

```rust
fn payload_to_string(p: &MessagePayload) -> String {
    match p {
        MessagePayload::Json(v) => {
            serde_json::to_string_pretty(v).unwrap_or_else(|_| v.to_string())
        }
        MessagePayload::Raw { bytes_len } => format!("<{} bytes>", bytes_len),
    }
}
```

Add a copy helper that talks to arboard and sets a toast:

```rust
fn copy_to_clipboard(&mut self, text: String, label: &str) {
    let byte_len = text.len();
    match arboard::Clipboard::new() {
        Ok(mut cb) => match cb.set_text(text) {
            Ok(()) => self.set_toast(format!("Copied {} ({}B)", label, byte_len)),
            Err(e) => self.set_error_toast(format!("Copy failed: {}", e)),
        },
        Err(e) => self.set_error_toast(format!("Clipboard unavailable: {}", e)),
    }
}
```

- [ ] **Step 2: Add y / Y handlers in `handle_view_key`**

Extend each view's match arm in `handle_view_key`. For `ActiveView::Topics`:

```rust
ActiveView::Topics => match (key.modifiers, key.code) {
    (_, KeyCode::Char('/')) => self.topics_filtering = true,
    (_, KeyCode::Char('y')) => {
        let filtered = self.filtered_topics();
        if let Some(topic) = filtered.get(self.topic_selected) {
            let key = topic.key_expr.clone();
            drop(filtered);
            if let Some((msg, _)) = self.topic_latest.get(&key).cloned() {
                let text = payload_to_string(&msg.payload);
                self.copy_to_clipboard(text, "payload");
            } else {
                self.set_error_toast("No data for selected topic");
            }
        }
    }
    (_, KeyCode::Char('Y')) => {
        let filtered = self.filtered_topics();
        if let Some(topic) = filtered.get(self.topic_selected) {
            let text = topic.key_expr.clone();
            drop(filtered);
            self.copy_to_clipboard(text, "key_expr");
        }
    }
    (m, KeyCode::Char('J')) if m.contains(crossterm::event::KeyModifiers::SHIFT) => {
        self.topic_detail_scroll = self.topic_detail_scroll.saturating_add(3);
    }
    (m, KeyCode::Char('K')) if m.contains(crossterm::event::KeyModifiers::SHIFT) => {
        self.topic_detail_scroll = self.topic_detail_scroll.saturating_sub(3);
    }
    (_, KeyCode::Up) | (_, KeyCode::Char('k')) => {
        self.topic_selected = self.topic_selected.saturating_sub(1);
        self.topic_detail_scroll = 0;
    }
    (_, KeyCode::Down) | (_, KeyCode::Char('j')) => {
        let max = self.filtered_topics().len().saturating_sub(1);
        if self.topic_selected < max {
            self.topic_selected += 1;
        }
        self.topic_detail_scroll = 0;
    }
    (_, KeyCode::Enter) => {
        self.active_view = ActiveView::Subscribe;
    }
    _ => {}
},
```

For `ActiveView::Subscribe`:

```rust
ActiveView::Subscribe => match key.code {
    KeyCode::Char(' ') => self.sub_paused = !self.sub_paused,
    KeyCode::Char('y') => {
        if let Some(msg) = self.sub_messages.get(self.sub_selected).cloned() {
            let text = payload_to_string(&msg.payload);
            self.copy_to_clipboard(text, "payload");
        } else {
            self.set_error_toast("No message selected");
        }
    }
    KeyCode::Char('Y') => {
        if let Some(msg) = self.sub_messages.get(self.sub_selected).cloned() {
            self.copy_to_clipboard(msg.key_expr, "key_expr");
        }
    }
    KeyCode::Up | KeyCode::Char('k') => {
        self.sub_selected = self.sub_selected.saturating_sub(1);
    }
    KeyCode::Down | KeyCode::Char('j') => {
        let max = self.sub_messages.len().saturating_sub(1);
        if self.sub_selected < max {
            self.sub_selected += 1;
        }
    }
    _ => {}
},
```

For `ActiveView::Query`:

```rust
ActiveView::Query => match key.code {
    KeyCode::Char('/') | KeyCode::Char('i') => self.query_editing = true,
    KeyCode::Char('y') => {
        if let Some(msg) = self.query_results.get(self.query_selected).cloned() {
            let text = payload_to_string(&msg.payload);
            self.copy_to_clipboard(text, "payload");
        } else {
            self.set_error_toast("No result selected");
        }
    }
    KeyCode::Down | KeyCode::Char('j') => {
        let max = self.query_results.len().saturating_sub(1);
        if self.query_selected < max {
            self.query_selected += 1;
        }
    }
    KeyCode::Up | KeyCode::Char('k') => {
        self.query_selected = self.query_selected.saturating_sub(1);
    }
    _ => {}
},
```

For `ActiveView::Nodes`:

```rust
ActiveView::Nodes => match key.code {
    KeyCode::Char('y') => {
        if let Some(node) = self.nodes.get(self.node_selected).cloned() {
            self.copy_to_clipboard(node.zid, "zid");
        } else {
            self.set_error_toast("No node selected");
        }
    }
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

- [ ] **Step 3: Build and test**

Run: `cargo build -p zemon-tui`
Expected: clean build.

- [ ] **Step 4: Manual test**

Run: `cargo run -- tui`, publish a few test messages from another terminal:

```bash
./target/release/zemon pub test/hello '{"msg":"world"}'
```

- Switch to Topics (`2`), select a topic, press `y` → status bar shows "Copied payload (14B)". Paste in another app to verify.
- Press `Y` → "Copied key_expr (10B)".
- Switch to Subscribe (`3`), use j/k to select a message, press `y` → payload copied.
- Switch to Nodes (`5`), press `y` → zid copied.

- [ ] **Step 5: Commit**

```bash
git add crates/zemon-tui/src/app.rs
git commit -m "feat(tui): add y/Y yank keys for clipboard copy"
```

---

## Task 11: Final Integration Test and Polish

**Files:**
- Modify: `crates/zemon-tui/src/app.rs` (status bar hint text update only if needed)

- [ ] **Step 1: Full manual verification checklist**

Run: `zenohd` in terminal 1, `cargo run --release -- tui` in terminal 2, `./target/release/zemon pub test/a '{"x":1}' && ./target/release/zemon pub test/b '{"y":2}'` in terminal 3 (repeat a few times).

Verify:
- [ ] Click each of the 5 tab labels — view switches
- [ ] In Topics: click on a topic row — row highlights and detail updates
- [ ] In Topics: wheel up/down — selection moves
- [ ] In Topics: press `y` — status shows "Copied payload (NB)"
- [ ] In Topics: press `Y` — status shows "Copied key_expr (NB)"
- [ ] In Subscribe: j/k moves cursor, new messages while cursor is non-zero keep cursor on its message
- [ ] In Subscribe: click a row selects it
- [ ] In Subscribe: wheel scrolls selection
- [ ] In Subscribe: `y` copies selected payload
- [ ] In Query: type a key and run query, click results, `y` copies
- [ ] In Nodes: click row selects, `y` copies zid
- [ ] Press `q` — terminal restores cleanly (no mouse cursor residue, can run `ls` normally)
- [ ] Force a panic (e.g. temporarily add `panic!()` to a handler, run, trigger it, remove) — terminal state restores

- [ ] **Step 2: Run the full test suite**

Run: `cargo test --workspace`
Expected: all tests pass, including the hit-test and sub_selected tests added earlier.

- [ ] **Step 3: Run `cargo clippy`**

Run: `cargo clippy -p zemon-tui --all-targets`
Expected: no new warnings introduced by this work. Fix any that appear.

- [ ] **Step 4: Commit any polish**

```bash
git add -u
git commit -m "chore(tui): polish after mouse support integration test"
```

(Skip if there were no polish changes.)

---

## Self-Review Notes

1. **Spec coverage:** Every item in the spec has a task — terminal init (Task 2), AppEvent::Mouse (Task 3), hit-testing (Task 4), tab click (Task 5), sub_selected (Task 6), list click (Task 7), wheel (Task 8), toast (Task 9), yank (Task 10), integration check (Task 11).

2. **Type consistency:** `tab_hit` / `list_hit` signatures match across tests and call site. `sub_selected` / `query_selected` / `node_selected` / `topic_selected` all `usize`. `toast: Option<(String, Instant)>` consistent. `payload_to_string` defined once, called from multiple yank handlers.

3. **Placeholders:** None. Every step shows concrete code or exact commands.

4. **Known simplification:** The previous Query view's `k` binding to recall last query is removed in Task 7 Step 6 in favor of j/k navigation. If the user reports this as a regression, a follow-up can restore recall with a different key (e.g. `r`).
