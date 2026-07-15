# Split `app.rs` into Submodules — Design

**Date:** 2026-05-14
**Scope:** Split the 1604-line `crates/zenmon-tui/src/app.rs` into a focused `app/` directory of submodules organized by concern (state, events, input, render). Apply two opportunistic cleanups (`is_text_input_active` rename, workspace-wide `cargo fmt`) in the same effort.

## Motivation

Code reviewers flagged during the TUI mode-switch and project-rename efforts that `app.rs` had grown too large to reason about reliably. At 1604 lines, it bundles type definitions, the App struct, the constructor, simple accessors, event dispatch, every key/mouse handler, every modal handler, every domain mutation, the render orchestrator, both modal renderers, and ~322 lines of tests. The file currently shoulders four distinct concerns simultaneously, which makes additions risky and review tedious. Splitting along concern lines makes future feature work safer and PRs easier to read. The two opportunistic cleanups (`is_text_input_active` is now misleading because it includes both modals; `cargo fmt --check` is failing on pre-existing long lines) are folded in because the split touches every relevant file anyway.

## Goals

- `crates/zenmon-tui/src/app.rs` becomes `crates/zenmon-tui/src/app/` with `mod.rs` + 3 sub-files, each ~300–600 lines.
- Public API observable from `crates/zenmon-tui/src/lib.rs` is unchanged. `App`, `ConnectionState`, `QueryStatus` remain accessible as `app::App`, etc.
- All 30 existing tests continue to pass with no behavior change.
- `is_text_input_active` is renamed to `is_key_capture_active` to reflect that it includes modal flags as well.
- `cargo fmt --check` passes after the work.

## Non-goals

- No new features.
- No new tests beyond what already exists.
- No introduction of `TestBackend` or any rendering test infrastructure.
- No changes to the `views/` directory beyond what existing imports require.
- No changes to `crates/zenmon-cli/` or `crates/zenmon-core/` other than what `cargo fmt` produces.

## Decisions (recorded from brainstorming)

| Question | Decision |
|---|---|
| Decomposition axis | **A** — concern type (state / events / input / render), aligning with the existing `views/` feature-split |
| Test placement | **A** — distribute tests to the submodule whose code they exercise |
| Cleanup scope | **A** — `cargo fmt`, the `is_text_input_active` rename, and any unused imports surfaced during the move are all in scope |

## Decomposition

`app.rs` is replaced by `app/mod.rs` and three siblings. All `App` methods stay inside `impl App` blocks; Rust permits the same struct to have multiple `impl` blocks across files in the same module tree, so the split is purely an organization change.

### `app/mod.rs` (~360 lines)

- Module declarations: `mod events; mod input; mod render;`
- Free helpers (`pub(crate)` so `input::handle_click` can call them):
  - `tab_hit`
  - `list_hit`
  - `payload_to_string`
- Constants (`pub(super)` so siblings can read them):
  - `TAB_TITLES`
  - `LIVELINESS_EVENT_CAP`
- Type definitions:
  - `ActiveView` (with `index` impl)
  - `ConnectionState`
  - `QueryStatus`
  - `LivelinessEventRecord`
- `App` struct definition (the field block).
- `impl App { ... }` for the small/general methods:
  - `new`
  - `is_connected`
  - `set_toast`
  - `set_error_toast`
  - `clear_network_state`
  - `copy_to_clipboard`
- Tests (`#[cfg(test)] mod tests`):
  - `tab_hit_inside_rect_returns_index`
  - `tab_hit_outside_returns_none`
  - `list_hit_converts_row_to_index`
  - `list_hit_respects_scroll_offset`

### `app/events.rs` (~370 lines)

- `impl App { ... }` for event dispatch and domain state:
  - `handle_event` (top-level dispatcher)
  - `handle_liveliness`
  - `handle_admin_nodes`
  - `handle_scout_nodes`
  - `clamp_node_selection`
  - `handle_zenoh_message`
  - `update_hz`
  - `filtered_topics`
  - `filtered_sub_messages`
  - `stream_message_matches`
  - `clamp_stream_selection`
  - `follow_stream`
  - `pin_stream_at`
- Tests:
  - `clear_network_state_clears_topics_messages_and_nodes`
  - `clear_network_state_preserves_query_history_and_filters`
  - `sub_selected_zero_stays_on_new_message`
  - `sub_selected_nonzero_follows_message_through_shift`
  - `filtered_sub_messages_match_key_and_payload`
  - `sub_selected_only_shifts_for_matching_filtered_message`
  - `follow_stream_resets_selection_to_latest`
  - `pin_stream_disables_follow`

The `clear_network_state_*` tests live here even though `clear_network_state` is defined in `mod.rs`, because they exercise it together with `handle_zenoh_message`, which is here. Tests have access to `super::*` regardless of which submodule they're in.

### `app/input.rs` (~580 lines)

- `impl App { ... }` for all input handling:
  - `handle_key`
  - `is_key_capture_active` (renamed from `is_text_input_active`)
  - `handle_scout_modal_key`
  - `handle_mode_modal_key`
  - `handle_text_input_key`
  - `handle_view_key`
  - `handle_mouse`
  - `handle_click`
  - `handle_wheel_up`
  - `handle_wheel_down`
- Tests:
  - `pressing_m_opens_mode_modal_with_current_mode_selected`
  - `mode_modal_arrow_keys_change_selection`
  - `mode_modal_enter_same_mode_does_not_set_pending`
  - `mode_modal_enter_different_mode_sets_pending_and_closes`
  - `mode_modal_esc_closes_without_setting_pending`
  - `pressing_m_again_closes_mode_modal`

### `app/render.rs` (~340 lines)

- `impl App { ... }` for rendering:
  - `render` (top-level orchestrator)
  - `render_scout_port_modal`
  - `render_mode_modal`
- No tests.

## Visibility changes

The current `app.rs` uses Rust's "private by default" for everything not explicitly `pub`. After the split, the following items need wider visibility because callers now live in sibling modules:

| Item | Before | After |
|---|---|---|
| `tab_hit` | `pub(crate)` | unchanged |
| `list_hit` | `pub(crate)` | unchanged |
| `payload_to_string` | private | `pub(super)` (used by `input.rs` and possibly `render.rs`) |
| `TAB_TITLES` | private | `pub(super)` (used by `render.rs`) |
| `LIVELINESS_EVENT_CAP` | private | `pub(super)` (used by `events.rs`) |

External API (`pub` items observable from `lib.rs`):
- `App`, `ConnectionState`, `QueryStatus`, `LivelinessEventRecord`, `ActiveView` remain `pub` and reachable as `app::App`, etc., because the directory module re-exports them implicitly through `mod.rs`.

## Cleanups in scope

**a. `cargo fmt --all`**

Executed once after the split lands. The fmt fixes that already exist on master (long lines in `zenmon-cli/src/main.rs`, `zenmon-core/src/config.rs`, `zenmon-core/src/discover.rs`) plus any new lines introduced or relocated by the split are all corrected by a single `cargo fmt --all` run.

**b. `is_text_input_active` → `is_key_capture_active`**

The function is currently:

```rust
fn is_text_input_active(&self) -> bool {
    self.topics_filtering
        || self.stream_filtering
        || self.query_editing
        || self.scout_port_modal_open
        || self.mode_modal_open
}
```

The first three flags are text inputs. The last two are modals that capture every keypress but accept no character input. The current name is misleading — a reader expecting a function called `is_text_input_active` to gate text-only behavior would be wrong about the modal cases. `is_key_capture_active` correctly describes the contract: "global keybindings should be suppressed because something is intercepting key events."

The rename is mechanical: the function moves to `input.rs` during the split. Its only call site is `handle_key` in the same file (line 401 in current `app.rs`).

**c. Unused imports / minor warnings**

If the move surfaces any unused imports (likely, because each submodule will have a focused import set), they get removed in the same commit. If pre-existing warnings unrelated to the split are surfaced, they get a separate fix commit (or are noted as out-of-scope follow-ups). No behavior changes.

## Commit structure

The work lands as **two commits**:

1. **`refactor(tui): split app.rs into app/ submodules`**
   - `git mv crates/zenmon-tui/src/app.rs crates/zenmon-tui/src/app/mod.rs`
   - Create `app/events.rs`, `app/input.rs`, `app/render.rs` and move methods/tests into them
   - Apply visibility changes
   - Rename `is_text_input_active` → `is_key_capture_active`
   - Remove unused imports surfaced by the move
   - Build clean, all 30 tests pass
2. **`style: cargo fmt --all`**
   - Pure formatting changes only
   - Build clean, all 30 tests pass
   - `cargo fmt --check` passes after this commit

## Verification

After commit 1:
- `cargo build` clean (no errors)
- `cargo test` shows 30 passing (8 zenmon-core + 22 zenmon-tui), 0 failures
- `wc -l crates/zenmon-tui/src/app/*.rs` shows each file ~300–600 lines
- `grep -rn "is_text_input_active" crates/` returns zero results
- `grep -rn "fn is_key_capture_active" crates/` returns exactly one definition
- The `lib.rs` import block (`use app::{App, ConnectionState, QueryStatus};`) still compiles unchanged

After commit 2:
- Same build and test verifications still pass
- `cargo fmt --all --check` exits 0

## Risks and mitigations

| Risk | Mitigation |
|---|---|
| Visibility omissions cause non-buildable intermediate state | All moves happen inside one commit; the implementer runs `cargo build` after each move and adjusts visibility before committing. |
| Tests cannot reach private items from a different submodule | Tests stay in the submodule that owns the code they primarily exercise. Cross-submodule access works because `mod tests { use super::*; }` reaches its parent submodule's `pub(crate)`/`pub(super)` items, and Rust's visibility rules from inside the same crate are usually permissive. If a test fails to reach an item, raise that item's visibility to `pub(super)` or `pub(crate)` rather than moving the test. |
| `git log --follow` chain breaks for relocated code | Use `git mv` for the rename of `app.rs` → `app/mod.rs`. New files (`events.rs`, `input.rs`, `render.rs`) start fresh in git history; this is unavoidable when splitting a single file into multiple. The blame chain for the relocated content can be recovered with `git log -L`. |
| `cargo fmt` commit becomes too large to review | The commit is intentionally pure-formatting (no semantic changes) and should be reviewable as "trust cargo fmt". A code reviewer can spot-check that no semantic edits slipped in by running `cargo fmt --all` themselves on the pre-commit state and diffing. |
| Public-API consumers (`lib.rs`) break | The directory module mechanically re-exports its `pub` items; `App`, `ConnectionState`, `QueryStatus` continue to be `app::App` etc. CI will catch any accidental visibility regression because `lib.rs` won't compile. |
| Behavior changes silently slip into the move | The split commit must produce zero diff in compiled behavior. The 30-test suite acts as the regression gate. The implementer should not refactor logic during the move — only relocate. |

## Tests

This refactor adds no new tests. The existing 30 tests (8 in `zenmon-core`, 22 in `zenmon-tui`) act as the regression gate. Tests are redistributed across submodules as listed above. After the split:

- `cargo test -p zenmon-tui` → 22 passed
- `cargo test` (workspace) → 30 passed total

## Out-of-scope follow-ups

- Introducing `ratatui::backend::TestBackend` for snapshot testing of `render` and the modal renderers.
- Splitting `views/<view>.rs` to also own per-view input handlers (decomposition axis B from the brainstorming).
- Restructuring `crates/zenmon-cli/src/main.rs` (currently the largest file in the workspace at ~480 lines and with its own pre-existing fmt issues — out of scope for this work, separate spec if desired).
