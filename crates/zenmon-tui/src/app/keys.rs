//! Keyboard dispatch: global chrome keys, per-space keys, overlay keys, and
//! the text-input mode that filters and editors run in.

use super::*;
use crossterm::event::{KeyCode, KeyEvent};

/// A detail-panel scroll request. Uppercase `J`/`K` scroll the detail panel in
/// every view; we accept the uppercase char regardless of how the terminal
/// reports the Shift modifier, so the contract is portable and consistent.
/// Lowercase `j`/`k` remain list navigation and are not handled here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DetailScroll {
    Down,
    Up,
}

pub(crate) fn detail_scroll_action(key: KeyEvent) -> Option<DetailScroll> {
    match key.code {
        KeyCode::Char('J') => Some(DetailScroll::Down),
        KeyCode::Char('K') => Some(DetailScroll::Up),
        _ => None,
    }
}

/// Apply a detail-scroll action to a scroll offset (3 lines per step,
/// saturating at 0).
pub(crate) fn apply_detail_scroll(scroll: u16, action: DetailScroll) -> u16 {
    match action {
        DetailScroll::Down => scroll.saturating_add(3),
        DetailScroll::Up => scroll.saturating_sub(3),
    }
}

impl App {
    pub(crate) fn handle_key(&mut self, key: KeyEvent) {
        if self.overlay == Overlay::Palette {
            self.handle_palette_key(key);
            return;
        }
        if self.overlay == Overlay::ScoutPort {
            self.handle_scout_modal_key(key);
            return;
        }
        if self.overlay == Overlay::PlotPicker {
            self.handle_plot_picker_key(key);
            return;
        }
        if self.overlay == Overlay::Publish {
            self.handle_publish_key(key);
            return;
        }
        if self.overlay == Overlay::ProfileSave {
            self.handle_profile_save_key(key);
            return;
        }
        if self.overlay == Overlay::ProfileLoad {
            self.handle_profile_load_key(key);
            return;
        }
        if self.overlay == Overlay::Doctor {
            match key.code {
                KeyCode::Esc | KeyCode::Char('q') | KeyCode::Char('d') => {
                    self.overlay = Overlay::None;
                }
                KeyCode::Char('r') => self.pending_doctor_request = true,
                KeyCode::Down | KeyCode::Char('j') => {
                    self.doctor_scroll = self.doctor_scroll.saturating_add(1)
                }
                KeyCode::Up | KeyCode::Char('k') => {
                    self.doctor_scroll = self.doctor_scroll.saturating_sub(1)
                }
                _ => {}
            }
            return;
        }
        if self.overlay != Overlay::None {
            match key.code {
                KeyCode::Esc | KeyCode::Char('q') | KeyCode::Char('?') => {
                    self.overlay = Overlay::None;
                    self.help_scroll = 0;
                }
                KeyCode::Down | KeyCode::Char('j') => {
                    self.help_scroll = self.help_scroll.saturating_add(1)
                }
                KeyCode::Up | KeyCode::Char('k') => {
                    self.help_scroll = self.help_scroll.saturating_sub(1)
                }
                _ => {}
            }
            return;
        }
        // Text input (filters, query editor, modals) swallows every key so that
        // typing `q`/`1`/`2`/`?` edits text instead of triggering global actions.
        if self.is_text_input_active() {
            self.handle_text_input_key(key);
            return;
        }
        match key.code {
            KeyCode::Char('q') => self.should_quit = true,
            KeyCode::Char('?') => {
                self.overlay = Overlay::Help;
                self.help_scroll = 0;
            }
            KeyCode::Char('d') => {
                self.overlay = Overlay::Doctor;
                self.doctor_scroll = 0;
                self.pending_doctor_request = true;
            }
            KeyCode::Char(':') => {
                self.overlay = Overlay::Palette;
                self.palette_input.clear();
                self.palette_selected = 0;
            }
            KeyCode::Tab => self.switch_space(self.space.next()),
            KeyCode::Char('1') => self.switch_space(Space::Traffic),
            KeyCode::Char('2') => self.switch_space(Space::Network),
            // Left/Right are NOT global: in Traffic they walk the tree, where
            // that reading is much stronger than "move pane focus". Enter/Esc
            // move focus in both spaces, and Network keeps the arrows too.
            KeyCode::Enter => self.pane_focus = PaneFocus::Detail,
            KeyCode::Esc => self.pane_focus = PaneFocus::Master,
            _ => self.handle_space_key(key),
        }
    }

    /// Dispatch a key not claimed by the global chrome to the active space.
    fn handle_space_key(&mut self, key: KeyEvent) {
        match self.space {
            Space::Traffic => self.handle_traffic_key(key),
            Space::Network => self.handle_network_key(key),
        }
    }

    /// Traffic-space normal-mode keys (master navigation + detail actions).
    /// Global keys (`q`, `?`, `Tab`, `1`/`2`, `Enter`/`Esc`) are consumed by
    /// `handle_key` before this runs.
    fn handle_traffic_key(&mut self, key: KeyEvent) {
        if let Some(action) = detail_scroll_action(key) {
            self.topic_detail_scroll = apply_detail_scroll(self.topic_detail_scroll, action);
            return;
        }
        // Every key below reads or moves within the visible rows, and the last
        // key may have changed them, so bring the cache up to date once here.
        self.refresh_tree_rows();
        match key.code {
            KeyCode::Char('j') | KeyCode::Down => self.move_tree_selection(1),
            KeyCode::Char('k') | KeyCode::Up => self.move_tree_selection(-1),
            KeyCode::Char('l') | KeyCode::Right => self.tree_expand_or_descend(),
            KeyCode::Char('h') | KeyCode::Left => self.tree_collapse_or_ascend(),
            KeyCode::Char('z') => self.tree_toggle_at_cursor(),
            KeyCode::Char('E') => self.tree_expand_all(),
            KeyCode::Char('C') => self.tree_collapse_all(),
            KeyCode::Char('/') => self.topics_filtering = true,
            KeyCode::Char('L') => self.detail_mode = DetailMode::Live,
            KeyCode::Char('Q') => {
                self.detail_mode = DetailMode::Query;
                if let Some(key_expr) = self.selected_topic_key() {
                    self.query_history.push(key_expr.clone());
                    self.pending_query = Some(key_expr);
                }
            }
            KeyCode::Char(' ') => self.history_paused = !self.history_paused,
            KeyCode::Char('D') => self.diff_enabled = !self.diff_enabled,
            KeyCode::Char('p') => self.open_plot_picker(),
            KeyCode::Char('P') => self.clear_plot_field(),
            KeyCode::Char('y') => self.copy_selected_payload(),
            KeyCode::Char('Y') => self.copy_selected_key(),
            _ => {}
        }
    }

    fn handle_profile_save_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc => {
                self.overlay = Overlay::None;
                self.profile_name_input.clear();
            }
            KeyCode::Enter => {
                let name = self.profile_name_input.trim().to_string();
                if name.is_empty() {
                    self.set_error_toast("Give the view a name");
                    return;
                }
                let profile = self.snapshot_profile(&name);
                self.profiles.upsert(profile);
                match zenmon_core::profile::save(&self.profiles) {
                    // Say where it went: a saved thing the user cannot find is
                    // barely saved.
                    Ok(()) => {
                        let where_ = zenmon_core::profile::config_path()
                            .map(|p| p.display().to_string())
                            .unwrap_or_default();
                        self.set_toast(format!("Saved '{name}' to {where_}"));
                    }
                    Err(e) => self.set_error_toast(format!("Could not save: {e}")),
                }
                self.overlay = Overlay::None;
                self.profile_name_input.clear();
            }
            KeyCode::Backspace => {
                self.profile_name_input.pop();
            }
            KeyCode::Char(c) => self.profile_name_input.push(c),
            _ => {}
        }
    }

    fn handle_profile_load_key(&mut self, key: KeyEvent) {
        let count = self.profiles.profiles.len();
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => self.overlay = Overlay::None,
            KeyCode::Down | KeyCode::Char('j') => {
                if count > 0 {
                    self.profile_selected = (self.profile_selected + 1).min(count - 1);
                }
            }
            KeyCode::Up | KeyCode::Char('k') => {
                self.profile_selected = self.profile_selected.saturating_sub(1);
            }
            KeyCode::Enter => {
                if let Some(p) = self.profiles.profiles.get(self.profile_selected).cloned() {
                    self.apply_profile(&p);
                    self.set_toast(format!("Loaded '{}'", p.name));
                }
                self.overlay = Overlay::None;
            }
            _ => {}
        }
    }

    /// Open the publish editor, prefilled with the selected key.
    ///
    /// Guard 1 of 4: without `--allow-publish` this refuses and says how to
    /// enable it, rather than the command being absent and leaving the user
    /// wondering whether the tool can do it at all.
    fn open_publish_editor(&mut self) {
        if !self.allow_publish {
            self.set_error_toast("Publishing is off — restart with --allow-publish");
            return;
        }
        self.publish_key = self.selected_topic_key().unwrap_or_default();
        self.publish_payload.clear();
        self.publish_field = if self.publish_key.is_empty() {
            PublishField::Key
        } else {
            PublishField::Payload
        };
        self.publish_result = None;
        self.overlay = Overlay::Publish;
    }

    /// Guard 4 of 4: `Ctrl+Enter` sends, plain `Enter` does not.
    ///
    /// Every other editor in this app commits on `Enter`, so this one
    /// deliberately does not: the muscle memory that dismisses a dialog must
    /// not be the thing that writes to a live network.
    fn handle_publish_key(&mut self, key: KeyEvent) {
        use crossterm::event::KeyModifiers;
        match key.code {
            KeyCode::Esc => {
                self.overlay = Overlay::None;
                self.publish_result = None;
            }
            KeyCode::Tab | KeyCode::Down | KeyCode::Up => {
                self.publish_field = match self.publish_field {
                    PublishField::Key => PublishField::Payload,
                    PublishField::Payload => PublishField::Key,
                };
            }
            KeyCode::Enter if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.arm_publish();
            }
            KeyCode::Backspace => match self.publish_field {
                PublishField::Key => {
                    self.publish_key.pop();
                }
                PublishField::Payload => {
                    self.publish_payload.pop();
                }
            },
            KeyCode::Char(c) => match self.publish_field {
                PublishField::Key => self.publish_key.push(c),
                PublishField::Payload => self.publish_payload.push(c),
            },
            _ => {}
        }
    }

    /// Hand the composed message to the run loop, which owns the session.
    fn arm_publish(&mut self) {
        if !self.allow_publish {
            // Unreachable through the UI, but the check is repeated here so the
            // guarantee does not depend on every caller of this method.
            self.publish_result = Some(Err("publishing is not enabled".into()));
            return;
        }
        let key = self.publish_key.trim().to_string();
        if key.is_empty() {
            self.publish_result = Some(Err("key expression is empty".into()));
            return;
        }
        if key.contains('*') {
            // A wildcard put fans out to every matching subscriber, which on a
            // fleet is every vehicle at once. Nothing about the editor makes
            // that intent visible, so it is refused rather than confirmed.
            self.publish_result = Some(Err("wildcards are not allowed here — name one key".into()));
            return;
        }
        self.pending_publish = Some((key, self.publish_payload.clone()));
        self.publish_result = None;
    }

    /// Open the field picker for the selected key.
    ///
    /// Only keys have plottable fields; a branch names a prefix, and a key with
    /// nothing numeric in its latest payload says so rather than opening an
    /// empty list the user has to dismiss.
    fn open_plot_picker(&mut self) {
        let Some(key) = self.selected_topic_key() else {
            self.set_error_toast("Select a key to plot a field from");
            return;
        };
        if self.plottable_fields().is_empty() {
            self.set_error_toast(format!("No numeric fields in {key}"));
            return;
        }
        // Start on the field already plotted, so reopening the picker does not
        // silently move the selection off it.
        let current = self.plot_field.get(&key).cloned();
        self.plot_picker_selected = current
            .and_then(|c| self.plottable_fields().iter().position(|f| *f == c))
            .unwrap_or(0);
        self.overlay = Overlay::PlotPicker;
    }

    fn clear_plot_field(&mut self) {
        if let Some(key) = self.selected_topic_key() {
            if self.plot_field.remove(&key).is_some() {
                self.set_toast("Plot cleared");
            }
        }
    }

    fn handle_plot_picker_key(&mut self, key: KeyEvent) {
        let fields = self.plottable_fields();
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') | KeyCode::Char('p') => {
                self.overlay = Overlay::None;
            }
            KeyCode::Down | KeyCode::Char('j') => {
                if !fields.is_empty() {
                    self.plot_picker_selected =
                        (self.plot_picker_selected + 1).min(fields.len() - 1);
                }
            }
            KeyCode::Up | KeyCode::Char('k') => {
                self.plot_picker_selected = self.plot_picker_selected.saturating_sub(1);
            }
            KeyCode::Enter => {
                if let (Some(k), Some(field)) = (
                    self.selected_topic_key(),
                    fields.get(self.plot_picker_selected).cloned(),
                ) {
                    self.plot_field.insert(k, field);
                }
                self.overlay = Overlay::None;
            }
            _ => {}
        }
    }

    /// Move the master cursor over the visible rows, clamped, resetting the
    /// detail scroll so the new selection starts at the top.
    pub(crate) fn move_tree_selection(&mut self, delta: isize) {
        self.refresh_tree_rows();
        let len = self.tree_rows().len();
        if len == 0 {
            return;
        }
        let cur = self.tree_selected.min(len - 1) as isize;
        self.tree_selected = (cur + delta).clamp(0, len as isize - 1) as usize;
        self.topic_detail_scroll = 0;
    }

    /// `l` / `→`: open a closed branch, step into an open one, list a folded
    /// group, or — on a key, where there is nothing left to open — hand focus
    /// to the detail pane, which is what the arrow used to do everywhere.
    fn tree_expand_or_descend(&mut self) {
        let Some(row) = self.selected_row() else {
            return;
        };
        match row.kind {
            RowKind::Branch { expanded: false } => {
                let path = row.path.clone();
                self.tree_expanded.insert(path);
                self.mark_tree_touched();
                self.refresh_tree_rows();
            }
            RowKind::Branch { expanded: true } => self.move_tree_selection(1),
            RowKind::FoldSummary { .. } => {
                let path = row.path.clone();
                self.tree_unfolded.insert(path);
                self.mark_tree_touched();
                self.refresh_tree_rows();
            }
            RowKind::Leaf => self.pane_focus = PaneFocus::Detail,
        }
    }

    /// `h` / `←`: close an open branch, else climb to the parent row.
    fn tree_collapse_or_ascend(&mut self) {
        let Some(row) = self.selected_row() else {
            return;
        };
        if let RowKind::Branch { expanded: true } = row.kind {
            let path = row.path.clone();
            self.tree_expanded.remove(&path);
            // Re-folding on collapse means reopening a huge branch does not dump
            // every child back on screen because it was unfolded once before.
            self.tree_unfolded.remove(&path);
            self.mark_tree_touched();
            self.refresh_tree_rows();
            return;
        }
        self.select_parent_row();
    }

    /// Move the cursor to the nearest row above that is shallower than this one.
    fn select_parent_row(&mut self) {
        let Some(depth) = self.selected_row().map(|r| r.depth) else {
            return;
        };
        if depth == 0 {
            return;
        }
        let rows = self.tree_rows();
        if let Some(idx) = rows[..self.tree_selected]
            .iter()
            .rposition(|r| r.depth < depth)
        {
            self.tree_selected = idx;
            self.topic_detail_scroll = 0;
        }
    }

    pub(crate) fn tree_toggle_at_cursor(&mut self) {
        let Some(row) = self.selected_row() else {
            return;
        };
        match row.kind {
            RowKind::Branch { expanded: true } => self.tree_collapse_or_ascend(),
            RowKind::Branch { expanded: false } | RowKind::FoldSummary { .. } => {
                self.tree_expand_or_descend()
            }
            RowKind::Leaf => {}
        }
    }

    /// `E`: open everything, folded groups included.
    ///
    /// Leaving the fold in place would make "expand all" stop at a summary row
    /// saying there is more — which is not what the key says it does. The row
    /// count can get large; `C` is right there.
    fn tree_expand_all(&mut self) {
        for path in self.key_tree.branch_paths() {
            self.tree_expanded.insert(path.clone());
            self.tree_unfolded.insert(path);
        }
        self.mark_tree_touched();
        self.refresh_tree_rows();
    }

    fn tree_collapse_all(&mut self) {
        self.tree_expanded.clear();
        self.tree_unfolded.clear();
        self.tree_selected = 0;
        self.mark_tree_touched();
        self.refresh_tree_rows();
    }

    /// The key expression currently selected, if the selection is a key.
    ///
    /// A branch row names a prefix, not a key: querying or copying it would act
    /// on something the network never published.
    pub(crate) fn selected_topic_key(&self) -> Option<String> {
        self.selected_row()
            .filter(|r| r.kind == RowKind::Leaf)
            .map(|r| r.path.clone())
    }

    fn copy_selected_payload(&mut self) {
        let Some(key) = self.selected_topic_key() else {
            self.set_error_toast("No key selected to copy");
            return;
        };
        match self.topic_latest.get(&key).map(|(m, _)| m.payload.pretty()) {
            Some(text) => self.copy_to_clipboard(text, "payload"),
            None => self.set_error_toast("No payload received for selected key"),
        }
    }

    fn copy_selected_key(&mut self) {
        match self.selected_topic_key() {
            Some(key) => self.copy_to_clipboard(key, "key"),
            None => self.set_error_toast("No key selected to copy"),
        }
    }

    /// Network-space normal-mode keys (unified participant navigation + detail
    /// actions). Global keys are consumed by `handle_key` before this runs.
    fn handle_network_key(&mut self, key: KeyEvent) {
        if let Some(action) = detail_scroll_action(key) {
            self.node_detail_scroll = apply_detail_scroll(self.node_detail_scroll, action);
            return;
        }
        match key.code {
            KeyCode::Char('j') | KeyCode::Down => self.move_network_selection(1),
            KeyCode::Char('k') | KeyCode::Up => self.move_network_selection(-1),
            // Network has no hierarchy, so the arrows keep their old
            // pane-focus meaning here even though Traffic reassigned them.
            KeyCode::Right => self.pane_focus = PaneFocus::Detail,
            KeyCode::Left => self.pane_focus = PaneFocus::Master,
            KeyCode::Char('s') => {
                if !self.scout_in_progress {
                    self.pending_scout_request = true;
                }
            }
            KeyCode::Char('/') => self.network_filtering = true,
            KeyCode::Char('D') => {
                self.network_dead_only = !self.network_dead_only;
                self.clamp_network_selection();
                self.node_detail_scroll = 0;
            }
            KeyCode::Char('y') => self.copy_selected_participant(),
            _ => {}
        }
    }

    /// Move the unified cursor within `network_rows()`, clamped to bounds, and
    /// reset the detail scroll so the new selection starts at the top.
    pub(crate) fn move_network_selection(&mut self, delta: isize) {
        let len = self.network_rows().len();
        if len == 0 {
            return;
        }
        let cur = self.network_selected.min(len - 1) as isize;
        self.network_selected = (cur + delta).clamp(0, len as isize - 1) as usize;
        self.node_detail_scroll = 0;
    }

    fn copy_selected_participant(&mut self) {
        match self.selected_network_row() {
            Some(NetworkRow::Session(i)) => {
                let zid = self.nodes[i].zid.clone();
                self.copy_to_clipboard(zid, "zid");
            }
            Some(NetworkRow::Liveliness(i)) => {
                let key = self.liveliness_tokens[i].key_expr.clone();
                self.copy_to_clipboard(key, "key");
            }
            None => self.set_error_toast("No participant selected to copy"),
        }
    }

    /// The unified list of selectable participant rows, in display order: every
    /// transport session (in node order) followed by every liveliness token.
    ///
    /// Tokens sort by key expression. That is the order the keys themselves
    /// impose, so related tokens land together without the view having to
    /// decide what "related" means.
    pub(crate) fn network_rows(&self) -> Vec<NetworkRow> {
        let needle = self.network_filter.to_lowercase();
        let matches = |hay: &str| needle.is_empty() || hay.to_lowercase().contains(&needle);

        // Sessions are hidden entirely by the dead-only toggle: it exists to
        // answer "which token died", and a transport session has no such state
        // to be filtered on.
        let mut rows: Vec<NetworkRow> = if self.network_dead_only {
            Vec::new()
        } else {
            (0..self.nodes.len())
                .filter(|&i| {
                    let n = &self.nodes[i];
                    matches(&n.zid) || matches(&n.kind)
                })
                .map(NetworkRow::Session)
                .collect()
        };

        let mut token_idx: Vec<usize> = (0..self.liveliness_tokens.len())
            .filter(|&i| {
                let t = &self.liveliness_tokens[i];
                (!self.network_dead_only || !t.alive) && matches(&t.key_expr)
            })
            .collect();
        // Dead first, then by key. A token that stopped is the finding; on a
        // fleet it is a handful of rows among thousands, and scrolling to them
        // is the work this ordering removes.
        token_idx.sort_by(|&a, &b| {
            let (ta, tb) = (&self.liveliness_tokens[a], &self.liveliness_tokens[b]);
            ta.alive.cmp(&tb.alive).then(ta.key_expr.cmp(&tb.key_expr))
        });
        rows.extend(token_idx.into_iter().map(NetworkRow::Liveliness));
        rows
    }

    /// Keep the cursor inside the row list after a filter changes it.
    pub(crate) fn clamp_network_selection(&mut self) {
        let len = self.network_rows().len();
        self.network_selected = if len == 0 {
            0
        } else {
            self.network_selected.min(len - 1)
        };
    }

    /// The participant row under the unified cursor, if any.
    pub(crate) fn selected_network_row(&self) -> Option<NetworkRow> {
        self.network_rows().get(self.network_selected).copied()
    }

    fn switch_space(&mut self, space: Space) {
        self.space = space;
        self.pane_focus = PaneFocus::Master;
    }

    pub(crate) fn is_text_input_active(&self) -> bool {
        self.topics_filtering
            || self.network_filtering
            || self.query_editing
            || self.overlay == Overlay::Palette
            || self.overlay == Overlay::ScoutPort
    }

    /// Indices into [`palette_commands`] whose label contains `palette_input`
    /// (case-insensitive substring). Empty input matches everything.
    pub(crate) fn filtered_palette_commands(&self) -> Vec<usize> {
        let needle = self.palette_input.to_ascii_lowercase();
        palette_commands()
            .iter()
            .enumerate()
            .filter(|(_, cmd)| {
                needle.is_empty() || cmd.label.to_ascii_lowercase().contains(&needle)
            })
            .map(|(i, _)| i)
            .collect()
    }

    /// Command-palette key handling: printable chars filter, arrows/Tab move the
    /// cursor within the filtered list, Enter runs, Esc closes.
    fn handle_palette_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc => {
                self.overlay = Overlay::None;
            }
            KeyCode::Enter => {
                let filtered = self.filtered_palette_commands();
                if let Some(&idx) = filtered.get(self.palette_selected) {
                    let action = palette_commands()[idx].action;
                    self.run_palette_action(action);
                }
            }
            KeyCode::Down | KeyCode::Tab => {
                let len = self.filtered_palette_commands().len();
                if len > 0 {
                    self.palette_selected = (self.palette_selected + 1).min(len - 1);
                }
            }
            KeyCode::Up => {
                self.palette_selected = self.palette_selected.saturating_sub(1);
            }
            KeyCode::Backspace => {
                self.palette_input.pop();
                self.palette_selected = 0;
            }
            KeyCode::Char(c) => {
                self.palette_input.push(c);
                self.palette_selected = 0;
            }
            _ => {}
        }
    }

    /// Perform a palette command's effect. Every action closes the palette,
    /// except `OpenScoutPort`, which transitions to the ScoutPort overlay.
    pub(crate) fn run_palette_action(&mut self, action: PaletteAction) {
        match action {
            PaletteAction::SwitchSpace(space) => {
                self.switch_space(space);
                self.overlay = Overlay::None;
            }
            PaletteAction::RunDoctor => {
                self.overlay = Overlay::Doctor;
                self.doctor_scroll = 0;
                self.pending_doctor_request = true;
            }
            PaletteAction::ScoutRefresh => {
                if !self.scout_in_progress {
                    self.pending_scout_request = true;
                }
                self.overlay = Overlay::None;
            }
            PaletteAction::SetMode(mode) => {
                if mode != self.current_mode {
                    self.pending_reconnect_mode = Some(mode);
                    self.set_toast(format!("Switching to {} mode…", mode));
                } else {
                    self.set_toast(format!("Already in {} mode", mode));
                }
                self.overlay = Overlay::None;
            }
            PaletteAction::OpenScoutPort => {
                self.overlay = Overlay::ScoutPort;
                self.scout_port_input.clear();
            }
            PaletteAction::OpenPublish => self.open_publish_editor(),
            PaletteAction::SaveProfile => {
                self.profile_name_input.clear();
                self.overlay = Overlay::ProfileSave;
            }
            PaletteAction::LoadProfile => {
                if self.profiles.profiles.is_empty() {
                    self.set_error_toast("No saved views yet — save one first");
                } else {
                    self.profile_selected = 0;
                    self.overlay = Overlay::ProfileLoad;
                }
            }
            PaletteAction::OpenHelp => {
                self.overlay = Overlay::Help;
                self.help_scroll = 0;
            }
            PaletteAction::Quit => {
                self.should_quit = true;
            }
        }
    }

    /// ScoutPort modal key handling: type a custom port, scan domains, or pick a
    /// scanned result, then reconnect on that scout port.
    fn handle_scout_modal_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc => {
                self.overlay = Overlay::None;
                self.scout_port_input.clear();
            }
            KeyCode::Char('s')
                if self.scout_port_input.is_empty() && !self.port_scan_in_progress =>
            {
                self.pending_port_scan_request = true;
            }
            KeyCode::Enter => {
                let from_input = self
                    .scout_port_input
                    .trim()
                    .parse::<u16>()
                    .ok()
                    .filter(|p| *p > 0);
                let from_list = if self.scout_port_input.is_empty() {
                    self.port_scan_results
                        .iter()
                        .filter(|r| !r.nodes.is_empty())
                        .nth(self.port_scan_selected)
                        .map(|r| r.port)
                } else {
                    None
                };
                if let Some(port) = from_input.or(from_list) {
                    self.pending_reconnect_port = Some(port);
                    self.scout_port_current = Some(port);
                    self.overlay = Overlay::None;
                    self.scout_port_input.clear();
                    self.set_toast(format!("Reconnecting with scout port {}", port));
                } else {
                    self.set_error_toast("Type a port or scan and select one");
                }
            }
            KeyCode::Char(c) if c.is_ascii_digit() && self.scout_port_input.len() < 5 => {
                self.scout_port_input.push(c);
            }
            KeyCode::Backspace => {
                self.scout_port_input.pop();
            }
            KeyCode::Up | KeyCode::Char('k') => {
                self.port_scan_selected = self.port_scan_selected.saturating_sub(1);
            }
            KeyCode::Down | KeyCode::Char('j') => {
                let count = self
                    .port_scan_results
                    .iter()
                    .filter(|r| !r.nodes.is_empty())
                    .count();
                if count > 0 && self.port_scan_selected + 1 < count {
                    self.port_scan_selected += 1;
                }
            }
            _ => {}
        }
    }

    fn handle_text_input_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc => {
                self.topics_filtering = false;
                self.network_filtering = false;
                self.query_editing = false;
            }
            KeyCode::Enter => {
                if self.query_editing {
                    self.query_editing = false;
                    if !self.query_input.is_empty() {
                        self.query_history.push(self.query_input.clone());
                        self.pending_query = Some(self.query_input.clone());
                    }
                }
                if self.topics_filtering {
                    self.topics_filtering = false;
                }
                if self.network_filtering {
                    self.network_filtering = false;
                }
            }
            KeyCode::Char(c) => {
                if self.topics_filtering {
                    self.topic_filter.push(c);
                } else if self.network_filtering {
                    self.network_filter.push(c);
                    self.clamp_network_selection();
                } else if self.query_editing {
                    self.query_input.push(c);
                }
            }
            KeyCode::Backspace => {
                if self.topics_filtering {
                    self.topic_filter.pop();
                } else if self.network_filtering {
                    self.network_filter.pop();
                    self.clamp_network_selection();
                } else if self.query_editing {
                    self.query_input.pop();
                }
            }
            _ => {}
        }
    }
}
