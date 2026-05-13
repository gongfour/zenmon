# TUI Connection Mode Switch — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add an in-TUI modal to switch the Zenoh connection mode (peer ↔ client) without restarting the binary, reusing the existing reconnect plumbing.

**Architecture:** New `App` fields hold the live mode and modal state; an `m` keystroke opens a small modal whose `Enter` sets `pending_reconnect_mode`; the main loop picks it up, mutates `config.mode`, drops the session, calls `clear_network_state()`, and re-enters the existing reconnect path. The active mode is shown in the status bar.

**Tech Stack:** Rust, ratatui, tokio, Zenoh.

**Spec:** `docs/superpowers/specs/2026-05-13-tui-mode-switch-design.md`

---

## File Structure

| File | Change |
|---|---|
| `crates/dotori-tui/src/app.rs` | Modify — add fields, `clear_network_state` helper, `handle_mode_modal_key`, `render_mode_modal`, `'m'` key binding, status bar badge, unit tests |
| `crates/dotori-tui/src/lib.rs` | Modify — initialize `app.current_mode` from config, add `pending_reconnect_mode` block in main loop |

No new files. CLI (`crates/dotori-cli/src/main.rs`) is untouched: it already sets `cfg.mode` from the `--mode` flag and passes the full `DotoriConfig` to `dotori_tui::run`.

---

## Task 1: Add mode state fields and `clear_network_state` helper

**Files:**
- Modify: `crates/dotori-tui/src/app.rs`

- [ ] **Step 1.1: Write failing tests for `clear_network_state`**

Append to the `#[cfg(test)] mod tests` block in `crates/dotori-tui/src/app.rs` (just before the closing `}` at line 1276):

```rust
    #[test]
    fn clear_network_state_clears_topics_messages_and_nodes() {
        let mut app = App::new("test".into());
        let make = |k: &str| ZenohMessage {
            key_expr: k.into(),
            payload: dotori_core::types::MessagePayload::Json(serde_json::json!(null)),
            timestamp: None,
            kind: "put".into(),
            attachment: None,
        };
        app.handle_zenoh_message(make("a"));
        app.handle_zenoh_message(make("b"));
        app.total_msg_count = 7;
        app.total_hz = 3.5;
        app.topic_selected = 1;
        app.topic_detail_scroll = 4;
        app.sub_selected = 1;
        app.admin_nodes.push(dotori_core::types::NodeInfo {
            zid: "z1".into(),
            kind: "router".into(),
            locators: vec![],
            metadata: None,
            sources: dotori_core::types::NodeSources::default(),
            admin_last_seen: None,
            scout_last_seen: None,
        });
        app.scout_nodes.push(dotori_core::types::NodeInfo {
            zid: "z2".into(),
            kind: "peer".into(),
            locators: vec![],
            metadata: None,
            sources: dotori_core::types::NodeSources::default(),
            admin_last_seen: None,
            scout_last_seen: None,
        });
        app.nodes = dotori_core::merge::merge_nodes(&app.admin_nodes, &app.scout_nodes);
        app.node_selected = 1;
        app.node_detail_scroll = 2;

        app.clear_network_state();

        assert!(app.topics.is_empty());
        assert!(app.topic_latest.is_empty());
        assert!(app.topic_msg_counts.is_empty());
        assert!(app.topic_hz.is_empty());
        assert_eq!(app.total_msg_count, 0);
        assert_eq!(app.total_hz, 0.0);
        assert_eq!(app.topic_selected, 0);
        assert_eq!(app.topic_detail_scroll, 0);

        assert!(app.sub_messages.is_empty());
        assert!(app.recent_messages.is_empty());
        assert_eq!(app.sub_selected, 0);

        assert!(app.admin_nodes.is_empty());
        assert!(app.scout_nodes.is_empty());
        assert!(app.nodes.is_empty());
        assert_eq!(app.node_selected, 0);
        assert_eq!(app.node_detail_scroll, 0);
    }

    #[test]
    fn clear_network_state_preserves_query_history_and_filters() {
        let mut app = App::new("test".into());
        app.query_input = "demo/**".into();
        app.query_history.push("demo/**".into());
        app.query_results.push(ZenohMessage {
            key_expr: "demo/x".into(),
            payload: dotori_core::types::MessagePayload::Json(serde_json::json!(1)),
            timestamp: None,
            kind: "get".into(),
            attachment: None,
        });
        app.topic_filter = "abc".into();
        app.stream_filter = "xyz".into();
        app.stream_follow = false;
        app.sub_paused = true;

        app.clear_network_state();

        assert_eq!(app.query_input, "demo/**");
        assert_eq!(app.query_history, vec!["demo/**".to_string()]);
        assert_eq!(app.query_results.len(), 1);
        assert_eq!(app.topic_filter, "abc");
        assert_eq!(app.stream_filter, "xyz");
        assert!(!app.stream_follow);
        assert!(app.sub_paused);
    }
```

- [ ] **Step 1.2: Run tests to verify they fail**

Run: `cargo test -p dotori-tui clear_network_state -- --nocapture`
Expected: FAIL with `no method named 'clear_network_state' found for struct 'App'`

- [ ] **Step 1.3: Add `ConnectMode` import and new `App` fields**

In `crates/dotori-tui/src/app.rs`, modify the import block at line 5 from:

```rust
use dotori_core::types::{LivelinessToken, MessagePayload, NodeInfo, PortScoutResult, TopicInfo, ZenohMessage};
```

to:

```rust
use dotori_core::config::ConnectMode;
use dotori_core::types::{LivelinessToken, MessagePayload, NodeInfo, PortScoutResult, TopicInfo, ZenohMessage};
```

In the `pub struct App { ... }` block (starts at line 109), add these fields right after `pub scout_port_current: Option<u16>,` (line 157):

```rust
    pub current_mode: ConnectMode,
    pub mode_modal_open: bool,
    pub mode_modal_selection: ConnectMode,
    pub pending_reconnect_mode: Option<ConnectMode>,
```

In `App::new` (starts at line 180), add these initializers right after `scout_port_current: None,` (line 222):

```rust
            current_mode: ConnectMode::Client,
            mode_modal_open: false,
            mode_modal_selection: ConnectMode::Client,
            pending_reconnect_mode: None,
```

- [ ] **Step 1.4: Add `clear_network_state` method**

In `crates/dotori-tui/src/app.rs`, add this method to the `impl App { ... }` block immediately after `set_error_toast` (before the existing `fn copy_to_clipboard` at line 255):

```rust
    pub fn clear_network_state(&mut self) {
        self.topics.clear();
        self.topic_latest.clear();
        self.topic_msg_counts.clear();
        self.topic_hz.clear();
        self.total_msg_count = 0;
        self.total_hz = 0.0;
        self.topic_selected = 0;
        self.topic_detail_scroll = 0;

        self.sub_messages.clear();
        self.recent_messages.clear();
        self.sub_selected = 0;

        self.admin_nodes.clear();
        self.scout_nodes.clear();
        self.nodes.clear();
        self.node_selected = 0;
        self.node_detail_scroll = 0;
    }
```

- [ ] **Step 1.5: Run tests to verify they pass**

Run: `cargo test -p dotori-tui clear_network_state -- --nocapture`
Expected: PASS (both tests)

Also run the full test suite to confirm no regressions:

Run: `cargo test -p dotori-tui`
Expected: all tests PASS

- [ ] **Step 1.6: Commit**

```bash
git add crates/dotori-tui/src/app.rs
git commit -m "$(cat <<'EOF'
feat(tui): add mode state fields and clear_network_state helper

Adds current_mode/mode_modal_open/mode_modal_selection/pending_reconnect_mode
to App and a clear_network_state helper that wipes topics, messages, and
nodes while preserving query history and user-entered filters. Used by the
upcoming mode-switch modal.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 2: Mode modal key handler and `m` keybinding

**Files:**
- Modify: `crates/dotori-tui/src/app.rs`

- [ ] **Step 2.1: Write failing tests for modal behavior**

Append to the `#[cfg(test)] mod tests` block in `crates/dotori-tui/src/app.rs`:

```rust
    use crossterm::event::KeyModifiers;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    #[test]
    fn pressing_m_opens_mode_modal_with_current_mode_selected() {
        let mut app = App::new("test".into());
        app.current_mode = ConnectMode::Peer;
        app.mode_modal_selection = ConnectMode::Client; // stale prior value

        app.handle_key(key(KeyCode::Char('m')));

        assert!(app.mode_modal_open);
        assert_eq!(app.mode_modal_selection, ConnectMode::Peer);
    }

    #[test]
    fn mode_modal_arrow_keys_change_selection() {
        let mut app = App::new("test".into());
        app.mode_modal_open = true;
        app.mode_modal_selection = ConnectMode::Client;

        app.handle_key(key(KeyCode::Up));
        assert_eq!(app.mode_modal_selection, ConnectMode::Peer);

        app.handle_key(key(KeyCode::Down));
        assert_eq!(app.mode_modal_selection, ConnectMode::Client);

        app.handle_key(key(KeyCode::Char('k')));
        assert_eq!(app.mode_modal_selection, ConnectMode::Peer);

        app.handle_key(key(KeyCode::Char('j')));
        assert_eq!(app.mode_modal_selection, ConnectMode::Client);
    }

    #[test]
    fn mode_modal_enter_same_mode_does_not_set_pending() {
        let mut app = App::new("test".into());
        app.current_mode = ConnectMode::Peer;
        app.mode_modal_open = true;
        app.mode_modal_selection = ConnectMode::Peer;

        app.handle_key(key(KeyCode::Enter));

        assert!(app.pending_reconnect_mode.is_none());
        assert!(!app.mode_modal_open);
    }

    #[test]
    fn mode_modal_enter_different_mode_sets_pending_and_closes() {
        let mut app = App::new("test".into());
        app.current_mode = ConnectMode::Client;
        app.mode_modal_open = true;
        app.mode_modal_selection = ConnectMode::Peer;

        app.handle_key(key(KeyCode::Enter));

        assert_eq!(app.pending_reconnect_mode, Some(ConnectMode::Peer));
        assert!(!app.mode_modal_open);
    }

    #[test]
    fn mode_modal_esc_closes_without_setting_pending() {
        let mut app = App::new("test".into());
        app.current_mode = ConnectMode::Client;
        app.mode_modal_open = true;
        app.mode_modal_selection = ConnectMode::Peer;

        app.handle_key(key(KeyCode::Esc));

        assert!(app.pending_reconnect_mode.is_none());
        assert!(!app.mode_modal_open);
    }

    #[test]
    fn pressing_m_again_closes_mode_modal() {
        let mut app = App::new("test".into());
        app.handle_key(key(KeyCode::Char('m')));
        assert!(app.mode_modal_open);
        app.handle_key(key(KeyCode::Char('m')));
        assert!(!app.mode_modal_open);
    }
```

This requires `ConnectMode` to derive `PartialEq` and `Eq` for the `assert_eq!` calls — it already does (`crates/dotori-core/src/config.rs:16`).

- [ ] **Step 2.2: Run tests to verify they fail**

Run: `cargo test -p dotori-tui mode_modal -- --nocapture` and `cargo test -p dotori-tui pressing_m -- --nocapture`
Expected: FAIL — modal handler does not exist; the `m` key falls through to `handle_view_key` and is ignored.

- [ ] **Step 2.3: Add modal routing and `m` key in `handle_key`**

In `crates/dotori-tui/src/app.rs`, modify `handle_key` (starts at line 352). Replace:

```rust
    fn handle_key(&mut self, key: KeyEvent) {
        if self.scout_port_modal_open {
            self.handle_scout_modal_key(key);
            return;
        }
        if !self.is_text_input_active() {
            match key.code {
                KeyCode::Char('q') => {
                    self.should_quit = true;
                    return;
                }
                KeyCode::Char('P') => {
                    self.scout_port_modal_open = true;
                    self.scout_port_input.clear();
                    return;
                }
```

with:

```rust
    fn handle_key(&mut self, key: KeyEvent) {
        if self.scout_port_modal_open {
            self.handle_scout_modal_key(key);
            return;
        }
        if self.mode_modal_open {
            self.handle_mode_modal_key(key);
            return;
        }
        if !self.is_text_input_active() {
            match key.code {
                KeyCode::Char('q') => {
                    self.should_quit = true;
                    return;
                }
                KeyCode::Char('P') => {
                    self.scout_port_modal_open = true;
                    self.scout_port_input.clear();
                    return;
                }
                KeyCode::Char('m') => {
                    self.mode_modal_open = true;
                    self.mode_modal_selection = self.current_mode;
                    return;
                }
```

- [ ] **Step 2.4: Update `is_text_input_active` to include the mode modal**

In `crates/dotori-tui/src/app.rs`, modify `is_text_input_active` (starts at line 384). Replace:

```rust
    fn is_text_input_active(&self) -> bool {
        self.topics_filtering
            || self.stream_filtering
            || self.query_editing
            || self.scout_port_modal_open
    }
```

with:

```rust
    fn is_text_input_active(&self) -> bool {
        self.topics_filtering
            || self.stream_filtering
            || self.query_editing
            || self.scout_port_modal_open
            || self.mode_modal_open
    }
```

- [ ] **Step 2.5: Add `handle_mode_modal_key`**

In `crates/dotori-tui/src/app.rs`, add this method to the `impl App { ... }` block immediately after `handle_scout_modal_key` (before `fn handle_mouse` at line 454):

```rust
    fn handle_mode_modal_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc | KeyCode::Char('m') => {
                self.mode_modal_open = false;
            }
            KeyCode::Up | KeyCode::Char('k') => {
                self.mode_modal_selection = ConnectMode::Peer;
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.mode_modal_selection = ConnectMode::Client;
            }
            KeyCode::Enter => {
                let target = self.mode_modal_selection;
                self.mode_modal_open = false;
                if target == self.current_mode {
                    let label = match target {
                        ConnectMode::Peer => "peer",
                        ConnectMode::Client => "client",
                    };
                    self.set_toast(format!("Already in {} mode", label));
                } else {
                    self.pending_reconnect_mode = Some(target);
                    let label = match target {
                        ConnectMode::Peer => "peer",
                        ConnectMode::Client => "client",
                    };
                    self.set_toast(format!("Switching to {} mode...", label));
                }
            }
            _ => {}
        }
    }
```

- [ ] **Step 2.6: Run tests to verify they pass**

Run: `cargo test -p dotori-tui`
Expected: all tests PASS, including the new modal tests.

- [ ] **Step 2.7: Commit**

```bash
git add crates/dotori-tui/src/app.rs
git commit -m "$(cat <<'EOF'
feat(tui): add 'm' keybinding and mode modal key handler

Pressing 'm' opens a modal whose selection defaults to the current mode.
Up/k and Down/j move between Peer and Client; Enter applies (no-op toast
if same mode, otherwise sets pending_reconnect_mode); Esc or another 'm'
closes without applying.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 3: Render the mode modal

**Files:**
- Modify: `crates/dotori-tui/src/app.rs`

This task is rendering only; it has no unit tests. We verify visually in the manual smoke test (Task 6).

- [ ] **Step 3.1: Add `render_mode_modal` method**

In `crates/dotori-tui/src/app.rs`, add this method to the `impl App { ... }` block immediately after `render_scout_port_modal` (i.e. after the closing `}` of that function, before the `}` that closes the `impl App` block — find it just below the existing modal renderer near line ~1130):

```rust
    fn render_mode_modal(&self, frame: &mut Frame, content_area: Rect) {
        let width = 36.min(content_area.width.saturating_sub(2));
        let height = 9.min(content_area.height.saturating_sub(2));
        if width < 24 || height < 7 {
            return;
        }
        let x = content_area.x + (content_area.width - width) / 2;
        let y = content_area.y + (content_area.height - height) / 2;
        let popup = Rect::new(x, y, width, height);

        frame.render_widget(Clear, popup);
        let block = Block::default()
            .borders(Borders::ALL)
            .title(" Mode ")
            .style(
                Style::default()
                    .fg(Color::White)
                    .bg(Color::Black)
                    .add_modifier(Modifier::BOLD),
            );
        let inner = block.inner(popup);
        frame.render_widget(block, popup);

        let [_pad, peer_row, client_row, _gap, current_row, hint_row] = Layout::vertical([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
        ])
        .areas(inner);

        let peer_marker = if matches!(self.mode_modal_selection, ConnectMode::Peer) {
            "> [*] Peer"
        } else {
            "  [ ] Peer"
        };
        let client_marker = if matches!(self.mode_modal_selection, ConnectMode::Client) {
            "> [*] Client"
        } else {
            "  [ ] Client"
        };

        frame.render_widget(
            Paragraph::new(peer_marker).style(Style::default().fg(Color::Cyan)),
            peer_row,
        );
        frame.render_widget(
            Paragraph::new(client_marker).style(Style::default().fg(Color::Cyan)),
            client_row,
        );

        let current_label = match self.current_mode {
            ConnectMode::Peer => "current: peer",
            ConnectMode::Client => "current: client",
        };
        frame.render_widget(
            Paragraph::new(current_label).style(Style::default().fg(Color::Gray)),
            current_row,
        );

        frame.render_widget(
            Paragraph::new(" jk/UpDn:select  Enter:apply  Esc:close ")
                .style(Style::default().fg(Color::DarkGray)),
            hint_row,
        );
    }
```

- [ ] **Step 3.2: Dispatch the modal render in `App::render`**

In `crates/dotori-tui/src/app.rs`, find the existing block in `pub fn render` (around line 933):

```rust
        if self.scout_port_modal_open {
            self.render_scout_port_modal(frame, content_area);
        }
```

Change it to:

```rust
        if self.scout_port_modal_open {
            self.render_scout_port_modal(frame, content_area);
        }
        if self.mode_modal_open {
            self.render_mode_modal(frame, content_area);
        }
```

- [ ] **Step 3.3: Build to verify**

Run: `cargo build -p dotori-tui`
Expected: compiles cleanly (no warnings about unused imports — `Clear`, `Borders`, `Layout`, `Constraint`, `Paragraph`, `Block`, `Modifier` are already in scope from line 6-9).

- [ ] **Step 3.4: Commit**

```bash
git add crates/dotori-tui/src/app.rs
git commit -m "$(cat <<'EOF'
feat(tui): render mode-switch modal

Centered 36x9 modal with Peer/Client radio rows, current-mode line, and
key-hint footer. Mirrors the layout style of the existing scout port
modal.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 4: Status bar mode badge and updated key hint

**Files:**
- Modify: `crates/dotori-tui/src/app.rs`

- [ ] **Step 4.1: Add the `mode:` badge and update the hint string**

In `crates/dotori-tui/src/app.rs`, find the `port_text` block in `pub fn render` (around line 974) and the status `Line::from(...)` immediately after it (around line 979). Currently:

```rust
        let port_text = match self.scout_port_current {
            Some(p) => format!(" scout:{} ", p),
            None => " scout:7446 ".to_string(),
        };

        let status = Line::from(vec![
            Span::styled(conn_text, conn_style),
            Span::styled(
                format!(" {} ", self.endpoint),
                Style::default().fg(Color::Gray),
            ),
            Span::styled(
                port_text,
                Style::default().fg(Color::Black).bg(Color::Magenta),
            ),
            middle_span,
            Span::styled(
                " q:quit  1-6:view  /:filter  y:copy  P:port ",
                Style::default().fg(Color::DarkGray),
            ),
        ]);
```

Replace with:

```rust
        let port_text = match self.scout_port_current {
            Some(p) => format!(" scout:{} ", p),
            None => " scout:7446 ".to_string(),
        };

        let mode_text = match self.current_mode {
            ConnectMode::Peer => " mode:peer ",
            ConnectMode::Client => " mode:client ",
        };

        let status = Line::from(vec![
            Span::styled(conn_text, conn_style),
            Span::styled(
                format!(" {} ", self.endpoint),
                Style::default().fg(Color::Gray),
            ),
            Span::styled(
                port_text,
                Style::default().fg(Color::Black).bg(Color::Magenta),
            ),
            Span::styled(
                mode_text,
                Style::default().fg(Color::Black).bg(Color::Blue),
            ),
            middle_span,
            Span::styled(
                " q:quit  1-6:view  /:filter  y:copy  P:port  m:mode ",
                Style::default().fg(Color::DarkGray),
            ),
        ]);
```

- [ ] **Step 4.2: Build to verify**

Run: `cargo build -p dotori-tui`
Expected: compiles cleanly.

- [ ] **Step 4.3: Commit**

```bash
git add crates/dotori-tui/src/app.rs
git commit -m "$(cat <<'EOF'
feat(tui): show active mode in status bar and add m:mode hint

Adds a 'mode:peer' / 'mode:client' badge next to the scout-port badge
and includes 'm:mode' in the keybinding hint line.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 5: Wire mode reconnect into the main loop

**Files:**
- Modify: `crates/dotori-tui/src/lib.rs`

- [ ] **Step 5.1: Initialize `app.current_mode` from config in `run`**

In `crates/dotori-tui/src/lib.rs`, find the top of `pub async fn run` (line 23-27):

```rust
pub async fn run(mut config: DotoriConfig, tick_rate_ms: u64) -> Result<()> {
    let endpoint = config.endpoint.clone();
    let mut app = App::new(endpoint);
    app.scout_port_current = config.scout_port;
```

Change it to:

```rust
pub async fn run(mut config: DotoriConfig, tick_rate_ms: u64) -> Result<()> {
    let endpoint = config.endpoint.clone();
    let mut app = App::new(endpoint);
    app.scout_port_current = config.scout_port;
    app.current_mode = config.mode;
    app.mode_modal_selection = config.mode;
```

- [ ] **Step 5.2: Add the `pending_reconnect_mode` block in the main loop**

In `crates/dotori-tui/src/lib.rs`, find the existing `pending_reconnect_port` block (line 263-270):

```rust
        if let Some(new_port) = app.pending_reconnect_port.take() {
            config.scout_port = Some(new_port);
            *session.lock().await = None;
            app.connection_state = ConnectionState::Connecting;
            reconnect_pending = true;
            spawn_connect(config.clone(), conn_tx.clone());
            needs_redraw = true;
        }
```

Add this block immediately after it:

```rust
        if let Some(new_mode) = app.pending_reconnect_mode.take() {
            config.mode = new_mode;
            app.current_mode = new_mode;
            app.clear_network_state();
            *session.lock().await = None;
            app.connection_state = ConnectionState::Connecting;
            reconnect_pending = true;
            spawn_connect(config.clone(), conn_tx.clone());
            needs_redraw = true;
        }
```

- [ ] **Step 5.3: Build the workspace**

Run: `cargo build`
Expected: clean build (no errors, no new warnings).

- [ ] **Step 5.4: Run all tests**

Run: `cargo test`
Expected: all tests PASS.

- [ ] **Step 5.5: Commit**

```bash
git add crates/dotori-tui/src/lib.rs
git commit -m "$(cat <<'EOF'
feat(tui): wire mode reconnect into the main loop

Initializes app.current_mode from the resolved DotoriConfig and adds a
pending_reconnect_mode handler next to the existing scout-port one.
On switch: updates config.mode, clears network state, drops the session,
and re-enters the connect path.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 6: Manual smoke test

**Files:**
- None (verification only)

This task confirms the feature works end-to-end and the UI renders correctly. It is not optional — rendering changes (Tasks 3 and 4) have no unit coverage.

- [ ] **Step 6.1: Build the release binary**

Run: `cargo build --release`
Expected: produces `./target/release/dotori`.

- [ ] **Step 6.2: Smoke test — peer mode without zenohd**

In Terminal A (no zenohd running anywhere):

Run: `./target/release/dotori --mode peer tui`

Verify:
- Status bar bottom-right shows ` mode:peer ` badge with blue background
- Status bar shows `Connected zid:...` (peer mode opens a session even with no peers)
- Help hint includes `m:mode`

- [ ] **Step 6.3: Smoke test — modal interactions**

While the TUI from Step 6.2 is running:

1. Press `m` — verify modal `╭──── Mode ────╮` appears centered
2. Verify cursor `>` is on `Peer` (current mode)
3. Press `j` — verify cursor moves to `Client`
4. Press `k` — verify cursor moves back to `Peer`
5. Press `Enter` — verify modal closes and toast says `Already in peer mode`
6. Press `m`, then `j`, then `Enter` — verify modal closes, toast says `Switching to client mode...`, status badge changes to ` mode:client `, status briefly shows `Connecting...`, and any previously visible topics/nodes are gone (clean slate)
7. With no zenohd running, status will eventually show `Disconnected: ...` — this is expected
8. Press `m`, then `k` (or `Up`), `Enter` — switches back to peer; should reconnect

- [ ] **Step 6.4: Smoke test — Esc and toggle**

1. Press `m` to open the modal
2. Press `Esc` — modal closes, no toast, no reconnect
3. Press `m` to open again
4. Press `m` again — modal closes (toggle behavior)

- [ ] **Step 6.5: Smoke test — with publisher running**

Start a publisher in Terminal B (peer mode, so no zenohd needed):

Run: `./target/release/dotori --mode peer pub test/hello '{"msg":"world"}'` (repeat or wrap in a loop)

In the TUI from Step 6.2 (still in peer mode), verify:
- `test/hello` appears in the Topics view
- Switch to Stream view ([3]), see messages flowing
- Press `m`, switch to client mode, Enter
- Verify topic list and stream are cleared
- Switch back to peer mode (`m`, `k`, `Enter`)
- Verify topic reappears once messages arrive again

- [ ] **Step 6.6: Record results and commit any fixes**

If any step fails, fix the underlying code, re-run `cargo test`, and amend or add a fix commit. If all pass, no commit needed for this task.

---

## Self-review notes

**Spec coverage:**

| Spec section | Implementing task |
|---|---|
| Data model — App fields | Task 1 (Step 1.3) |
| Reconnect plumbing | Task 5 (Step 5.2) |
| State cleanup helper | Task 1 (Steps 1.4) + invoked in Task 5 (Step 5.2) |
| UI: modal | Task 3 |
| UI: keybinding | Task 2 (Steps 2.3, 2.5) |
| UI: status bar badge | Task 4 |
| Edge cases | Task 2 (handler covers all key paths); Task 5 (reconnect plumbing for fail/retry); Task 6 (manual verification) |
| Tests listed in spec | Task 1 (Step 1.1) covers `clear_network_state_*`; Task 2 (Step 2.1) covers `mode_modal_*` |

**Type consistency:** `ConnectMode`, `current_mode`, `mode_modal_selection`, `pending_reconnect_mode`, `clear_network_state` are spelled identically across all tasks. `ConnectMode` is `Copy` (already derives `Copy` in `crates/dotori-core/src/config.rs:16`), so the assignments and `==` comparisons in this plan are valid.

**No placeholders.** Every code-change step shows the exact code.
