use crate::event::AppEvent;
use crate::views;
use crossterm::event::{KeyCode, KeyEvent, MouseButton, MouseEvent, MouseEventKind};
use dotori_core::merge::merge_nodes;
use dotori_core::types::{MessagePayload, NodeInfo, PortScoutResult, TopicInfo, ZenohMessage};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};
use ratatui::Frame;
use std::collections::{HashMap, VecDeque};
use std::time::{Instant, SystemTime};

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

fn payload_to_string(p: &MessagePayload) -> String {
    match p {
        MessagePayload::Json(v) => {
            serde_json::to_string_pretty(v).unwrap_or_else(|_| v.to_string())
        }
        MessagePayload::Raw { bytes_len } => format!("<{} bytes>", bytes_len),
    }
}

const TAB_TITLES: [&str; 5] = ["Dashboard", "Topics", "Stream", "Query", "Nodes"];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActiveView {
    Dashboard,
    Topics,
    Stream,
    Query,
    Nodes,
}

impl ActiveView {
    pub fn index(&self) -> usize {
        match self {
            ActiveView::Dashboard => 0,
            ActiveView::Topics => 1,
            ActiveView::Stream => 2,
            ActiveView::Query => 3,
            ActiveView::Nodes => 4,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConnectionState {
    Disconnected(String),
    Connecting,
    Connected(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QueryStatus {
    Idle,
    Running,
    Done(usize),
    Error(String),
}

pub struct App {
    pub active_view: ActiveView,
    pub should_quit: bool,
    pub connection_state: ConnectionState,
    pub endpoint: String,
    pub tab_rects: [Option<ratatui::layout::Rect>; 5],

    pub topics: Vec<TopicInfo>,
    pub topic_latest: HashMap<String, (ZenohMessage, Instant)>,
    pub admin_nodes: Vec<NodeInfo>,
    pub scout_nodes: Vec<NodeInfo>,
    pub nodes: Vec<NodeInfo>,
    pub recent_messages: VecDeque<ZenohMessage>,

    pub sub_messages: VecDeque<ZenohMessage>,
    pub sub_paused: bool,
    pub sub_selected: usize,
    pub stream_filter: String,
    pub stream_filtering: bool,

    pub topic_filter: String,
    pub topic_selected: usize,
    pub topics_filtering: bool,
    pub topic_detail_scroll: u16,

    pub topic_msg_counts: HashMap<String, u32>,
    pub topic_hz: HashMap<String, f64>,
    pub last_hz_update: Instant,
    pub total_msg_count: u32,
    pub total_hz: f64,

    pub query_input: String,
    pub query_results: Vec<ZenohMessage>,
    pub query_history: Vec<String>,
    pub query_editing: bool,
    pub pending_query: Option<String>,
    pub query_status: QueryStatus,
    pub query_selected: usize,

    pub node_selected: usize,
    pub scout_in_progress: bool,
    pub last_scout_at: Option<SystemTime>,
    pub pending_scout_request: bool,

    pub scout_port_modal_open: bool,
    pub scout_port_input: String,
    pub scout_port_current: Option<u16>,
    pub port_scan_results: Vec<PortScoutResult>,
    pub port_scan_selected: usize,
    pub port_scan_in_progress: bool,
    pub pending_port_scan_request: bool,
    pub pending_reconnect_port: Option<u16>,

    pub list_rect: Option<ratatui::layout::Rect>,
    pub list_first_item_row: u16,
    pub list_scroll_offset: usize,

    pub toast: Option<(String, std::time::Instant)>,
    pub toast_is_error: bool,
}

impl App {
    pub fn new(endpoint: String) -> Self {
        Self {
            active_view: ActiveView::Dashboard,
            should_quit: false,
            connection_state: ConnectionState::Connecting,
            endpoint,
            tab_rects: [None; 5],
            topics: Vec::new(),
            topic_latest: HashMap::new(),
            admin_nodes: Vec::new(),
            scout_nodes: Vec::new(),
            nodes: Vec::new(),
            recent_messages: VecDeque::with_capacity(100),
            sub_messages: VecDeque::with_capacity(500),
            sub_paused: false,
            sub_selected: 0,
            stream_filter: String::new(),
            stream_filtering: false,
            topic_filter: String::new(),
            topic_selected: 0,
            topics_filtering: false,
            topic_detail_scroll: 0,
            topic_msg_counts: HashMap::new(),
            topic_hz: HashMap::new(),
            last_hz_update: Instant::now(),
            total_msg_count: 0,
            total_hz: 0.0,
            query_input: String::new(),
            query_results: Vec::new(),
            query_history: Vec::new(),
            query_editing: false,
            pending_query: None,
            query_status: QueryStatus::Idle,
            query_selected: 0,
            node_selected: 0,
            scout_in_progress: false,
            last_scout_at: None,
            pending_scout_request: false,
            scout_port_modal_open: false,
            scout_port_input: String::new(),
            scout_port_current: None,
            port_scan_results: Vec::new(),
            port_scan_selected: 0,
            port_scan_in_progress: false,
            pending_port_scan_request: false,
            pending_reconnect_port: None,
            list_rect: None,
            list_first_item_row: 0,
            list_scroll_offset: 0,
            toast: None,
            toast_is_error: false,
        }
    }

    pub fn is_connected(&self) -> bool {
        matches!(self.connection_state, ConnectionState::Connected(_))
    }

    pub fn set_toast(&mut self, msg: impl Into<String>) {
        self.toast = Some((msg.into(), std::time::Instant::now()));
        self.toast_is_error = false;
    }

    pub fn set_error_toast(&mut self, msg: impl Into<String>) {
        self.toast = Some((msg.into(), std::time::Instant::now()));
        self.toast_is_error = true;
    }

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

    pub fn handle_event(&mut self, event: AppEvent) {
        match event {
            AppEvent::Key(key) => self.handle_key(key),
            AppEvent::Mouse(m) => self.handle_mouse(m),
            AppEvent::Zenoh(msg) => self.handle_zenoh_message(msg),
            AppEvent::Tick => self.update_hz(),
            AppEvent::AdminNodes(nodes) => self.handle_admin_nodes(nodes),
            AppEvent::ScoutStarted => {
                self.scout_in_progress = true;
            }
            AppEvent::ScoutNodes(nodes) => self.handle_scout_nodes(nodes),
            AppEvent::PortScanStarted => {
                self.port_scan_in_progress = true;
            }
            AppEvent::PortScanResults(results) => {
                self.port_scan_results = results;
                self.port_scan_selected = 0;
                self.port_scan_in_progress = false;
            }
        }
    }

    fn handle_admin_nodes(&mut self, nodes: Vec<NodeInfo>) {
        self.admin_nodes = nodes;
        self.nodes = merge_nodes(&self.admin_nodes, &self.scout_nodes);
        self.clamp_node_selection();
    }

    fn handle_scout_nodes(&mut self, nodes: Vec<NodeInfo>) {
        self.scout_nodes = nodes;
        self.last_scout_at = Some(SystemTime::now());
        self.scout_in_progress = false;
        self.nodes = merge_nodes(&self.admin_nodes, &self.scout_nodes);
        self.clamp_node_selection();
    }

    fn clamp_node_selection(&mut self) {
        if self.nodes.is_empty() {
            self.node_selected = 0;
        } else if self.node_selected >= self.nodes.len() {
            self.node_selected = self.nodes.len() - 1;
        }
    }

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
                KeyCode::Char('1') => self.active_view = ActiveView::Dashboard,
                KeyCode::Char('2') => self.active_view = ActiveView::Topics,
                KeyCode::Char('3') => self.active_view = ActiveView::Stream,
                KeyCode::Char('4') => self.active_view = ActiveView::Query,
                KeyCode::Char('5') => self.active_view = ActiveView::Nodes,
                KeyCode::Esc => {
                    self.active_view = ActiveView::Dashboard;
                }
                _ => self.handle_view_key(key),
            }
        } else {
            self.handle_text_input_key(key);
        }
    }

    fn is_text_input_active(&self) -> bool {
        self.topics_filtering
            || self.stream_filtering
            || self.query_editing
            || self.scout_port_modal_open
    }

    fn handle_scout_modal_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc => {
                self.scout_port_modal_open = false;
                self.scout_port_input.clear();
            }
            KeyCode::Char('s') => {
                if self.scout_port_input.is_empty() && !self.port_scan_in_progress {
                    self.pending_port_scan_request = true;
                }
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
                    self.scout_port_modal_open = false;
                    self.scout_port_input.clear();
                    self.set_toast(format!("Reconnecting with scout port {}", port));
                } else {
                    self.set_error_toast("Type a port or scan and select one");
                }
            }
            KeyCode::Char(c) if c.is_ascii_digit() => {
                if self.scout_port_input.len() < 5 {
                    self.scout_port_input.push(c);
                }
            }
            KeyCode::Backspace => {
                self.scout_port_input.pop();
            }
            KeyCode::Up | KeyCode::Char('k') => {
                self.port_scan_selected = self.port_scan_selected.saturating_sub(1);
            }
            KeyCode::Down | KeyCode::Char('j') => {
                let hits = self
                    .port_scan_results
                    .iter()
                    .filter(|r| !r.nodes.is_empty())
                    .count();
                let max = hits.saturating_sub(1);
                if self.port_scan_selected < max {
                    self.port_scan_selected += 1;
                }
            }
            _ => {}
        }
    }

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
                2 => ActiveView::Stream,
                3 => ActiveView::Query,
                4 => ActiveView::Nodes,
                _ => self.active_view,
            };
            return;
        }

        let Some(rect) = self.list_rect else {
            return;
        };
        if col < rect.x || col >= rect.x + rect.width {
            return;
        }
        let total = match self.active_view {
            ActiveView::Topics => self.filtered_topics().len(),
            ActiveView::Stream => self.filtered_sub_messages().len(),
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
            ActiveView::Stream => self.sub_selected = idx,
            ActiveView::Query => self.query_selected = idx,
            ActiveView::Nodes => self.node_selected = idx,
            ActiveView::Dashboard => {}
        }
    }

    fn handle_wheel_up(&mut self) {
        match self.active_view {
            ActiveView::Topics => {
                self.topic_selected = self.topic_selected.saturating_sub(1);
                self.topic_detail_scroll = 0;
            }
            ActiveView::Stream => {
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
            ActiveView::Stream => {
                let max = self.filtered_sub_messages().len().saturating_sub(1);
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

    fn handle_text_input_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc => {
                self.topics_filtering = false;
                self.stream_filtering = false;
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
                if self.stream_filtering {
                    self.stream_filtering = false;
                    self.clamp_stream_selection();
                }
            }
            KeyCode::Char(c) => {
                if self.topics_filtering {
                    self.topic_filter.push(c);
                } else if self.stream_filtering {
                    self.stream_filter.push(c);
                    self.clamp_stream_selection();
                } else if self.query_editing {
                    self.query_input.push(c);
                }
            }
            KeyCode::Backspace => {
                if self.topics_filtering {
                    self.topic_filter.pop();
                } else if self.stream_filtering {
                    self.stream_filter.pop();
                    self.clamp_stream_selection();
                } else if self.query_editing {
                    self.query_input.pop();
                }
            }
            _ => {}
        }
    }

    fn handle_view_key(&mut self, key: KeyEvent) {
        match self.active_view {
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
                    self.active_view = ActiveView::Stream;
                }
                _ => {}
            },
            ActiveView::Stream => match key.code {
                KeyCode::Char('/') => self.stream_filtering = true,
                KeyCode::Char(' ') => self.sub_paused = !self.sub_paused,
                KeyCode::Char('y') => {
                    if let Some(msg) = self
                        .filtered_sub_messages()
                        .get(self.sub_selected)
                        .map(|msg| (*msg).clone())
                    {
                        let text = payload_to_string(&msg.payload);
                        self.copy_to_clipboard(text, "payload");
                    } else {
                        self.set_error_toast("No message selected");
                    }
                }
                KeyCode::Char('Y') => {
                    if let Some(msg) = self
                        .filtered_sub_messages()
                        .get(self.sub_selected)
                        .map(|msg| (*msg).clone())
                    {
                        self.copy_to_clipboard(msg.key_expr, "key_expr");
                    }
                }
                KeyCode::Up | KeyCode::Char('k') => {
                    self.sub_selected = self.sub_selected.saturating_sub(1);
                }
                KeyCode::Down | KeyCode::Char('j') => {
                    let max = self.filtered_sub_messages().len().saturating_sub(1);
                    if self.sub_selected < max {
                        self.sub_selected += 1;
                    }
                }
                _ => {}
            },
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
                KeyCode::Char('s') => {
                    if !self.scout_in_progress {
                        self.pending_scout_request = true;
                    }
                }
                _ => {}
            },
            _ => {}
        }
    }

    fn handle_zenoh_message(&mut self, msg: ZenohMessage) {
        if !self.topics.iter().any(|t| t.key_expr == msg.key_expr) {
            self.topics.push(TopicInfo {
                key_expr: msg.key_expr.clone(),
            });
            self.topics.sort_by(|a, b| a.key_expr.cmp(&b.key_expr));
        }

        self.topic_latest
            .insert(msg.key_expr.clone(), (msg.clone(), Instant::now()));

        *self.topic_msg_counts.entry(msg.key_expr.clone()).or_insert(0) += 1;
        self.total_msg_count += 1;

        self.recent_messages.push_front(msg.clone());
        if self.recent_messages.len() > 100 {
            self.recent_messages.pop_back();
        }

        if !self.sub_paused {
            let matches_stream_filter = self.stream_message_matches(&msg);
            self.sub_messages.push_front(msg);
            if self.sub_messages.len() > 500 {
                self.sub_messages.pop_back();
            }
            if matches_stream_filter && self.sub_selected > 0 {
                self.sub_selected += 1;
            }
            self.clamp_stream_selection();
        }
    }

    pub fn update_hz(&mut self) {
        let elapsed = self.last_hz_update.elapsed().as_secs_f64();
        if elapsed >= 1.0 {
            for (key, count) in self.topic_msg_counts.drain() {
                self.topic_hz.insert(key, count as f64 / elapsed);
            }
            self.total_hz = self.total_msg_count as f64 / elapsed;
            self.total_msg_count = 0;
            self.last_hz_update = Instant::now();
        }
    }

    pub fn filtered_topics(&self) -> Vec<&TopicInfo> {
        if self.topic_filter.is_empty() {
            self.topics.iter().collect()
        } else {
            self.topics
                .iter()
                .filter(|t| t.key_expr.contains(&self.topic_filter))
                .collect()
        }
    }

    pub fn filtered_sub_messages(&self) -> Vec<&ZenohMessage> {
        self.sub_messages
            .iter()
            .filter(|msg| self.stream_message_matches(msg))
            .collect()
    }

    fn stream_message_matches(&self, msg: &ZenohMessage) -> bool {
        if self.stream_filter.is_empty() {
            return true;
        }

        msg.key_expr.contains(&self.stream_filter)
            || payload_to_string(&msg.payload).contains(&self.stream_filter)
            || msg
                .attachment
                .as_ref()
                .map(|att| payload_to_string(att).contains(&self.stream_filter))
                .unwrap_or(false)
    }

    fn clamp_stream_selection(&mut self) {
        let filtered_len = self.filtered_sub_messages().len();
        if filtered_len == 0 {
            self.sub_selected = 0;
        } else if self.sub_selected >= filtered_len {
            self.sub_selected = filtered_len - 1;
        }
    }

    pub fn render(&mut self, frame: &mut Frame) {
        let [tabs_area, content_area, status_area] = Layout::vertical([
            Constraint::Length(3),
            Constraint::Fill(1),
            Constraint::Length(1),
        ])
        .areas(frame.area());

        let tabs_block = Block::default().borders(Borders::ALL).title(" dotori ");
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

        match self.active_view {
            ActiveView::Dashboard => views::dashboard::render(self, frame, content_area),
            ActiveView::Topics => views::topics::render(self, frame, content_area),
            ActiveView::Stream => views::stream::render(self, frame, content_area),
            ActiveView::Query => views::query::render(self, frame, content_area),
            ActiveView::Nodes => views::nodes::render(self, frame, content_area),
        }

        if self.scout_port_modal_open {
            self.render_scout_port_modal(frame, content_area);
        }

        let (conn_text, conn_style) = match &self.connection_state {
            ConnectionState::Connected(zid) => (
                format!(" Connected zid:{} ", &zid[..zid.len().min(16)]),
                Style::default().fg(Color::Black).bg(Color::Green),
            ),
            ConnectionState::Connecting => (
                " Connecting... ".to_string(),
                Style::default().fg(Color::Black).bg(Color::Yellow),
            ),
            ConnectionState::Disconnected(reason) => (
                format!(" Disconnected: {} ", reason),
                Style::default().fg(Color::White).bg(Color::Red),
            ),
        };

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
                " q:quit  1-5:view  /:filter  y:copy  P:port ",
                Style::default().fg(Color::DarkGray),
            ),
        ]);
        frame.render_widget(status, status_area);
    }

    fn render_scout_port_modal(&self, frame: &mut Frame, content_area: Rect) {
        let width = 58.min(content_area.width.saturating_sub(2));
        let height = 16.min(content_area.height.saturating_sub(2));
        if width < 20 || height < 8 {
            return;
        }
        let x = content_area.x + (content_area.width - width) / 2;
        let y = content_area.y + (content_area.height - height) / 2;
        let popup = Rect::new(x, y, width, height);

        frame.render_widget(Clear, popup);
        let block = Block::default()
            .borders(Borders::ALL)
            .title(" Scout Port ")
            .style(
                Style::default()
                    .fg(Color::White)
                    .bg(Color::Black)
                    .add_modifier(Modifier::BOLD),
            );
        let inner = block.inner(popup);
        frame.render_widget(block, popup);

        let [current_row, input_row, _gap, list_area, hint_row] = Layout::vertical([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Fill(1),
            Constraint::Length(1),
        ])
        .areas(inner);

        let current_text = match self.scout_port_current {
            Some(p) => format!(
                "Current: {} (domain {})",
                p,
                p.saturating_sub(7446)
            ),
            None => "Current: 7446 (default, domain 0)".to_string(),
        };
        frame.render_widget(
            Paragraph::new(current_text).style(Style::default().fg(Color::Gray)),
            current_row,
        );

        let input_text = if self.scout_port_input.is_empty() {
            "New port: _".to_string()
        } else {
            format!("New port: {}_", self.scout_port_input)
        };
        frame.render_widget(
            Paragraph::new(input_text).style(Style::default().fg(Color::Cyan)),
            input_row,
        );

        if self.port_scan_in_progress {
            frame.render_widget(
                Paragraph::new("Scanning ports 7446-7546 ...")
                    .style(Style::default().fg(Color::Yellow)),
                list_area,
            );
        } else {
            let hits: Vec<&PortScoutResult> = self
                .port_scan_results
                .iter()
                .filter(|r| !r.nodes.is_empty())
                .collect();
            if hits.is_empty() && self.port_scan_results.is_empty() {
                frame.render_widget(
                    Paragraph::new("Press 's' to scan ports 7446-7546")
                        .style(Style::default().fg(Color::DarkGray)),
                    list_area,
                );
            } else if hits.is_empty() {
                frame.render_widget(
                    Paragraph::new("No nodes found in 7446-7546")
                        .style(Style::default().fg(Color::Red)),
                    list_area,
                );
            } else {
                let lines: Vec<Line> = hits
                    .iter()
                    .enumerate()
                    .map(|(i, r)| {
                        let selected = i == self.port_scan_selected;
                        let marker = if selected { "> " } else { "  " };
                        let style = if selected {
                            Style::default()
                                .fg(Color::Black)
                                .bg(Color::Cyan)
                                .add_modifier(Modifier::BOLD)
                        } else {
                            Style::default().fg(Color::White)
                        };
                        Line::from(vec![Span::styled(
                            format!(
                                "{}{:>5}  (domain {:<3})  {} node(s)",
                                marker,
                                r.port,
                                r.port.saturating_sub(7446),
                                r.nodes.len()
                            ),
                            style,
                        )])
                    })
                    .collect();
                frame.render_widget(Paragraph::new(lines), list_area);
            }
        }

        frame.render_widget(
            Paragraph::new(" s:scan  Enter:reconnect  jk/↑↓:select  Esc:close ")
                .style(Style::default().fg(Color::DarkGray)),
            hint_row,
        );
    }
}

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
        let rects = [Some(Rect::new(1, 0, 14, 3)), None, None, None, None];
        assert_eq!(tab_hit(&rects, 50, 1), None);
        assert_eq!(tab_hit(&rects, 2, 5), None);
    }

    #[test]
    fn list_hit_converts_row_to_index() {
        let rect = Rect::new(0, 5, 20, 10);
        assert_eq!(list_hit(rect, 6, 0, 8, 6), Some(0));
        assert_eq!(list_hit(rect, 8, 0, 8, 6), Some(2));
        assert_eq!(list_hit(rect, 5, 0, 8, 6), None);
        assert_eq!(list_hit(rect, 15, 0, 8, 6), None);
        assert_eq!(list_hit(rect, 20, 0, 8, 6), None);
        assert_eq!(list_hit(rect, 14, 0, 8, 6), None);
    }

    #[test]
    fn list_hit_respects_scroll_offset() {
        let rect = Rect::new(0, 5, 20, 10);
        assert_eq!(list_hit(rect, 6, 4, 20, 6), Some(4));
        assert_eq!(list_hit(rect, 9, 4, 20, 6), Some(7));
    }

    #[test]
    fn sub_selected_zero_stays_on_new_message() {
        let mut app = App::new("test".into());
        app.sub_selected = 0;
        let msg = ZenohMessage {
            key_expr: "a".into(),
            payload: dotori_core::types::MessagePayload::Json(serde_json::json!(null)),
            timestamp: None,
            kind: "put".into(),
            attachment: None,
        };
        app.handle_zenoh_message(msg);
        assert_eq!(app.sub_selected, 0);
    }

    #[test]
    fn sub_selected_nonzero_follows_message_through_shift() {
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
        app.handle_zenoh_message(make("c"));
        app.sub_selected = 1;
        app.handle_zenoh_message(make("d"));
        assert_eq!(app.sub_selected, 2);
    }

    #[test]
    fn filtered_sub_messages_match_key_and_payload() {
        let mut app = App::new("test".into());
        app.handle_zenoh_message(ZenohMessage {
            key_expr: "robot/pose".into(),
            payload: dotori_core::types::MessagePayload::Json(serde_json::json!({"x": 1})),
            timestamp: None,
            kind: "put".into(),
            attachment: None,
        });
        app.handle_zenoh_message(ZenohMessage {
            key_expr: "robot/status".into(),
            payload: dotori_core::types::MessagePayload::Json(serde_json::json!("idle")),
            timestamp: None,
            kind: "put".into(),
            attachment: None,
        });

        app.stream_filter = "pose".into();
        assert_eq!(app.filtered_sub_messages().len(), 1);
        assert_eq!(app.filtered_sub_messages()[0].key_expr, "robot/pose");

        app.stream_filter = "idle".into();
        assert_eq!(app.filtered_sub_messages().len(), 1);
        assert_eq!(app.filtered_sub_messages()[0].key_expr, "robot/status");
    }

    #[test]
    fn sub_selected_only_shifts_for_matching_filtered_message() {
        let mut app = App::new("test".into());
        let make = |k: &str| ZenohMessage {
            key_expr: k.into(),
            payload: dotori_core::types::MessagePayload::Json(serde_json::json!(null)),
            timestamp: None,
            kind: "put".into(),
            attachment: None,
        };
        app.handle_zenoh_message(make("alpha/1"));
        app.handle_zenoh_message(make("beta/1"));
        app.handle_zenoh_message(make("alpha/2"));

        app.stream_filter = "alpha".into();
        app.sub_selected = 1;

        app.handle_zenoh_message(make("beta/2"));
        assert_eq!(app.sub_selected, 1);

        app.handle_zenoh_message(make("alpha/3"));
        assert_eq!(app.sub_selected, 2);
    }
}
