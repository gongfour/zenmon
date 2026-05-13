# TUI Connection Mode Switch — Design

**Date:** 2026-05-13
**Scope:** Allow switching Zenoh connection mode (peer ↔ client) from inside the running TUI without restarting the binary.

## Motivation

The TUI currently locks the connection mode to whatever was passed via `--mode` (default `client`). Users who want to operate without `zenohd` must quit, restart with `--mode peer`, and reattach — losing scrollback and any in-session context. Making the mode switchable inside the TUI lowers the friction of moving between zenohd-backed and zenohd-less environments while debugging.

## Goals

- Switch `peer ↔ client` from within the TUI in a few keystrokes
- Show the active mode at all times in the status bar
- Reuse the existing `pending_reconnect_port` reconnect plumbing — no new connection-management primitives
- Clean the in-memory network snapshot on switch so stale topics/nodes from the previous environment don't confuse the user

## Non-goals

- Editing the `--endpoint` value from inside the TUI (out of scope; users can already shift multicast domains via the scout port modal in peer mode, and remote endpoints still require a restart)
- Persisting the mode choice across TUI sessions
- Adding a generic "settings" view

## Decisions (recorded from brainstorming)

| Question | Decision |
|---|---|
| Trigger UX | **B** — global hotkey `m` opens a small modal with peer/client choices |
| Endpoint coupling | **A** — modal only changes mode; endpoint untouched |
| State cleanup on switch | **C** — clear topics/messages/nodes; preserve query history and filters |
| Mode display | **A** — status bar badge `mode:peer` next to existing `scout:` badge |

## Design

### Data model

Additions to `App` (`crates/zemon-tui/src/app.rs`):

```rust
pub current_mode: ConnectMode,        // mirrors the live config.mode
pub mode_modal_open: bool,
pub mode_modal_selection: ConnectMode, // transient selection inside the modal
pub pending_reconnect_mode: Option<ConnectMode>, // signal picked up by main loop
```

`ConnectMode` is already exported from `zemon-core::config`. `zemon-tui` already depends on `zemon-core` via `ZemonConfig`, so no new crate boundary is crossed.

`App::new` is updated to accept the initial mode (or to read it from a passed-in `ZemonConfig`) so `current_mode` is initialized correctly. The CLI entry point (`crates/zemon-cli/src/main.rs`) wires the resolved `cfg.mode` through.

### Reconnect plumbing

In the main loop (`crates/zemon-tui/src/lib.rs`), add a block adjacent to the existing `pending_reconnect_port` handler:

```rust
if let Some(new_mode) = app.pending_reconnect_mode.take() {
    config.mode = new_mode;
    app.current_mode = new_mode;
    *session.lock().await = None;
    app.connection_state = ConnectionState::Connecting;
    reconnect_pending = true;
    app.clear_network_state();
    spawn_connect(config.clone(), conn_tx.clone());
    needs_redraw = true;
}
```

The existing `ConnectResult::Connected` branch already clears liveliness state and re-subscribes, so liveliness handling needs no changes.

### State cleanup

New helper `App::clear_network_state(&mut self)` clears fields that represent observations of a specific network and resets associated UI selections. Fields preserved are those that represent user input or session-scoped configuration.

**Cleared:**
- `topics`, `topic_latest`, `topic_msg_counts`, `topic_hz`, `total_msg_count`, `total_hz`
- `topic_selected`, `topic_detail_scroll`
- `sub_messages`, `recent_messages`, `sub_selected`
- `admin_nodes`, `scout_nodes`, `nodes`, `node_selected`, `node_detail_scroll`

**Preserved:**
- `query_input`, `query_history`, `query_results`, `query_status`, `query_selected`
- `topic_filter`, `stream_filter`, `topics_filtering`, `stream_filtering`
- `stream_follow`, `sub_paused`
- `scout_port_current` (multicast domain choice is orthogonal to mode)

Liveliness fields are not cleared here because the `ConnectResult::Connected` handler in `lib.rs:303-307` already clears them after the new session lands.

### UI: modal

`render_mode_modal()` mirrors `render_scout_port_modal` in placement, sizing, and styling. Approximate dimensions: 40 cols × 7 rows, centered.

```
╭──────── Mode ────────╮
│                      │
│  > [●] Peer          │
│    [ ] Client        │
│                      │
│  current: peer       │
│ ↑↓:select Enter:apply Esc │
╰──────────────────────╯
```

The cursor (`>`) and filled radio (`[●]`) follow `mode_modal_selection`. The `current:` line shows `current_mode` so the user can confirm what they're switching from.

### UI: keybinding

In `App::handle_view_key`, when no text input is active:

```rust
KeyCode::Char('m') => {
    self.mode_modal_open = true;
    self.mode_modal_selection = self.current_mode;
}
```

In `App::handle_key`, add a routing guard right after the existing scout-port modal guard:

```rust
if self.mode_modal_open {
    self.handle_mode_modal_key(key);
    return;
}
```

`handle_mode_modal_key`:

| Key | Action |
|---|---|
| `↑` / `k` | `mode_modal_selection = Peer` |
| `↓` / `j` | `mode_modal_selection = Client` |
| `Enter` | If `selection == current_mode`: toast `"Already in <mode> mode"`, close modal. Else: `pending_reconnect_mode = Some(selection)`, toast `"Switching to <mode> mode..."`, close modal. |
| `Esc` | Close modal, no change |
| `m` | Close modal (toggle behavior) |

Pre-existing `m` usages were checked against `handle_view_key`: no conflict.

### UI: status bar badge

In the status line construction (`app.rs` ~974, just after `port_text`):

```rust
let mode_text = match self.current_mode {
    ConnectMode::Peer => " mode:peer ",
    ConnectMode::Client => " mode:client ",
};
```

Insert the corresponding `Span` immediately after the `scout:` span, using a distinct Blue background badge (`fg:Black bg:Blue` — scout 배지는 Magenta). Order in the status line:

```
[ NORMAL ] [ scout:7446 ] [ mode:peer ] [ Connected zid:... ]
```

### Edge cases

| Scenario | Behavior |
|---|---|
| Modal open during `Connecting` | Allowed. Applying mid-connect overwrites the in-flight target — when the previous `spawn_connect` resolves, the main loop sees `pending_reconnect_mode` is `None` and proceeds; the next loop iteration's `pending_reconnect_mode` block (if user reapplies) re-spawns with the latest mode. Practically: last `Enter` wins. |
| User selects same mode | No-op, toast only |
| Mode switch lands on a network with no nodes | Status reaches `Connected` (Zenoh runtime succeeds even with no peers); topic/node lists stay empty; status bar `mode:` badge reflects the new mode |
| Connection fails after switch | `ConnectionState::Disconnected(reason)` shown as today; 5s auto-retry kicks in; the new mode persists in `config` so retries use the new mode |
| `m` pressed while modal already open | Closes modal (toggle) |
| `m` pressed while text input is active (e.g. query editing) | Ignored, treated as text input as today |

## Tests

Unit tests in `crates/zemon-tui/src/app.rs` `#[cfg(test)]` block:

- `clear_network_state_clears_topics_and_nodes_only`
- `clear_network_state_preserves_query_history_and_filters`
- `mode_modal_enter_same_mode_does_not_set_pending`
- `mode_modal_enter_different_mode_sets_pending_and_closes`
- `mode_modal_esc_closes_without_setting_pending`

No integration test for the reconnect flow — that path is exercised today by the scout port modal and reuses the same plumbing.

## Out-of-scope follow-ups

- TUI-internal endpoint editing (separate spec when needed)
- Persisting last-used mode to a config file
- Combined "Connection Settings" modal that consolidates mode, scout port, endpoint
