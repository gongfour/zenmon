//! Pointer input: wheel, clicks, and the geometry helpers that turn a
//! screen position back into a list index.

use super::*;
use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::Rect;

/// Return the space-tab index hit by a click at `(col, row)`, or `None`.
pub(crate) fn space_tab_hit(rects: &[Option<Rect>; 2], col: u16, row: u16) -> Option<usize> {
    for (i, maybe) in rects.iter().enumerate() {
        if let Some(r) = maybe {
            if col >= r.x && col < r.x + r.width && row >= r.y && row < r.y + r.height {
                return Some(i);
            }
        }
    }
    None
}

/// Return the list item index hit by a click row, or `None`.
///
/// `first_item_row` is the absolute screen row of item 0 (typically `rect.y + 1`
/// to skip the top border). `scroll_offset` is the number of items skipped before
/// rendering. `total_items` rejects clicks past the end of the list.
pub(crate) fn list_hit(
    rect: Rect,
    click_row: u16,
    scroll_offset: usize,
    total_items: usize,
    first_item_row: u16,
) -> Option<usize> {
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

impl App {
    pub(crate) fn handle_mouse(&mut self, ev: MouseEvent) {
        // No mouse routing while a text input or any overlay is active — the
        // overlays own their own keyboard-driven scroll and selection.
        if self.is_text_input_active() || self.overlay != Overlay::None {
            return;
        }
        match ev.kind {
            MouseEventKind::Down(MouseButton::Left) => self.handle_click(ev.column, ev.row),
            MouseEventKind::ScrollUp => self.wheel(-1),
            MouseEventKind::ScrollDown => self.wheel(1),
            _ => {}
        }
    }

    /// Move the active space's master selection by `delta` (mouse-wheel parity
    /// with `j`/`k`). Both movers clamp and reset the detail scroll.
    pub(crate) fn wheel(&mut self, delta: isize) {
        match self.space {
            Space::Traffic => self.move_tree_selection(delta),
            Space::Network => self.move_network_selection(delta),
        }
    }

    pub(crate) fn handle_click(&mut self, col: u16, row: u16) {
        if let Some(idx) = space_tab_hit(&self.space_tab_rects, col, row) {
            self.space = match idx {
                0 => Space::Traffic,
                _ => Space::Network,
            };
            self.pane_focus = PaneFocus::Master;
            return;
        }

        // A click inside the master list selects the row under the cursor. Each
        // space maps a display row to a selection differently: Traffic rows are
        // 1:1 with the flattened tree, while Network interleaves non-selectable
        // headers, so its click goes through `network_click_map`.
        let Some(rect) = self.list_rect else {
            return;
        };
        let inside = col >= rect.x
            && col < rect.x + rect.width
            && row >= rect.y
            && row < rect.y + rect.height;
        if !inside {
            return;
        }

        match self.space {
            Space::Traffic => {
                self.refresh_tree_rows();
                if let Some(idx) = list_hit(
                    rect,
                    row,
                    self.list_scroll_offset,
                    self.tree_rows().len(),
                    self.list_first_item_row,
                ) {
                    self.tree_selected = idx;
                    self.topic_detail_scroll = 0;
                    // Clicking a branch opens it — a click on a closed folder is
                    // how every tree behaves, and drilling into the detail pane
                    // would show a subtree summary the user did not ask for.
                    match self.selected_row().map(|r| r.kind) {
                        Some(RowKind::Leaf) => self.pane_focus = PaneFocus::Detail,
                        Some(_) => self.tree_toggle_at_cursor(),
                        None => {}
                    }
                }
            }
            Space::Network => {
                if let Some(disp) = list_hit(
                    rect,
                    row,
                    self.list_scroll_offset,
                    self.network_click_map.len(),
                    self.list_first_item_row,
                ) {
                    // A header row maps to `Some(&None)` → leave selection alone.
                    if let Some(&Some(sel)) = self.network_click_map.get(disp) {
                        self.network_selected = sel;
                        self.node_detail_scroll = 0;
                        self.pane_focus = PaneFocus::Detail;
                    }
                }
            }
        }
    }
}
