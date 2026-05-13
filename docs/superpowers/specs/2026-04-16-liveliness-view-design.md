# Liveliness View Design

## Summary

Add a new `[6] Liveliness` TUI tab for monitoring Zenoh liveliness tokens, and clean up the existing `[5] Nodes` view to focus purely on transport-level node information.

## Background

Zenoh liveliness tokens provide application-level presence detection. Each application declares a token on a key expression (e.g. `hdx/forky001/node/action_executor_ec98a701`), and observers can detect when tokens appear (join) or disappear (leave).

Key constraint: liveliness events do not carry `source_info` — the declaring session's ZID is not available. Tokens are identified solely by their `key_expr`. This means liveliness tokens cannot be mapped to transport-level nodes (which are identified by ZID).

This makes liveliness and transport nodes two fundamentally different data sources:
- **Transport nodes** = infrastructure level (ZID, kind, locators, router topology)
- **Liveliness tokens** = application level (service name, group, alive/dead status)

They belong in separate views.

## Changes

### 1. New `[6] Liveliness` View

Three-panel layout:

```
┌─ Token List ──────────┐┌─ Token Detail ─────────────┐
│ Liveliness (7 alive)  ││ action_executor             │
│ ── hdx/forky001 (6/6) ││ Key: hdx/forky001/node/...  │
│ > ● action_executor   ││ Group: hdx/forky001          │
│   ● eb_controller     ││ Status: ● alive              │
│   ● topic_recorder    ││                              │
│   ● trajectory_fol... ││                              │
│ ── hdx (1/1)          ││                              │
│   ● system_monitor    ││                              │
├─ Event Log ───────────┤├──────────────────────────────┤
│ 02:03:32 JOIN action.. │ 02:03:31 JOIN system_monitor │
└────────────────────────┘└─────────────────────────────┘
```

#### Token List (top-left, 45% width)
- Flat list of all liveliness tokens
- Grouped by prefix with section headers: `── hdx/forky001 (6/6) ──`
  - Format: `── {group_prefix} ({alive}/{total}) ──`
  - Group prefix extracted from key_expr (everything before the last two path segments)
- Each token row: `{status_icon} {node_name}`
  - `●` green = alive, `○` red = dead
  - Node name extracted from key_expr last segment with hash stripped
- Navigation: `j/k` or arrow keys
- Selected row highlighted with cyan background

#### Token Detail (top-right, 55% width)
- **Name**: extracted node name (bold, yellow)
- **Key**: full key_expr
- **Group**: group prefix
- **Status**: alive/dead with colored indicator

#### Event Log (bottom, full width, ~30% height)
- Shows all JOIN/LEAVE events across the network, newest first
- Format: `{time} {JOIN|LEAVE} {node_name} {group_prefix}`
  - JOIN = green, LEAVE = red
- Scrollable with `Shift+J/K`
- Events stored in a `VecDeque` with a cap (e.g. 200 entries)

#### Keyboard
- `j/k`: navigate token list
- `Shift+J/K`: scroll event log
- `y`: copy selected token's key_expr to clipboard

### 2. Clean Up `[5] Nodes` View

Remove all liveliness-related code from the Nodes view:
- Remove `Name` column from the node list table
- Remove `Network Liveliness` section from the node detail panel
- Remove liveliness token matching logic from `build_row()`
- Keep `(self)` label on ZID (this is session-level info, not liveliness)
- Restore the original 4-column layout: ZID, Kind, Source, Locators

### 3. App State Changes

Keep existing liveliness state in `App`:
- `liveliness_tokens: Vec<LivelinessToken>` — current token state
- `handle_liveliness()` — join/leave event handling

Add new state:
- `liveliness_selected: usize` — selected token index in the list
- `liveliness_events: VecDeque<LivelinessEventRecord>` — event log history
- `liveliness_log_scroll: u16` — event log scroll position
- `ActiveView::Liveliness` variant — new tab

New type for event log:
```rust
struct LivelinessEventRecord {
    timestamp: Instant,
    kind: LivelinessEventKind, // Join or Leave
    key_expr: String,
    node_name: String,
    group: String,
}
```

### 4. Tab Bar

Update tab bar from 5 to 6 tabs:
```
[1] Dashboard  [2] Topics  [3] Stream  [4] Query  [5] Nodes  [6] Liveliness
```

Key `6` switches to the Liveliness view.

## Non-Goals

- Mapping liveliness tokens to transport nodes by ZID (not possible with current Zenoh API)
- Tree collapse/expand (flat list with group headers is sufficient)
- Liveliness token declaration from zemon (zemon is an observer, not a participant)

## Files to Change

- `crates/zemon-tui/src/views/nodes.rs` — remove liveliness code
- `crates/zemon-tui/src/views/liveliness.rs` — new file, liveliness view rendering
- `crates/zemon-tui/src/views/mod.rs` — add liveliness module
- `crates/zemon-tui/src/app.rs` — add liveliness state, ActiveView variant, key handlers, tab rendering
- `crates/zemon-tui/src/event.rs` — no changes needed (LivelinessEvent already exists)
- `crates/zemon-core/src/types.rs` — add LivelinessEventRecord type (or keep in TUI)
