use crate::event::AppEvent;
use crate::views;
use crossterm::event::{KeyCode, KeyEvent, MouseButton, MouseEvent, MouseEventKind};
use zemon_core::config::ConnectMode;
use zemon_core::merge::merge_nodes;
use zemon_core::types::{LivelinessToken, MessagePayload, NodeInfo, PortScoutResult, TopicInfo, ZenohMessage};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Wrap};
use ratatui::Frame;
use std::collections::{HashMap, VecDeque};
use std::time::{Instant, SystemTime};

/// Return the tab index hit by a click at `(col, row)`, or `None`.
pub(crate) fn tab_hit(rects: &[Option<Rect>; 7], col: u16, row: u16) -> Option<usize> {
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
    p.pretty()
}

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

/// Human label for a scout/multicast port. Ports in the Zenoh domain range
/// (7446..=7546, i.e. domains 0..=100) are shown as their domain id; anything
/// else is a custom port and is labeled as a port, not a domain — so an
/// arbitrary port below 7446 isn't misreported as "domain 0".
pub(crate) fn domain_port_label(port: u16) -> String {
    if (7446..=7546).contains(&port) {
        format!("domain {} (port {})", port - 7446, port)
    } else {
        format!("port {} (custom)", port)
    }
}

const TAB_TITLES: [&str; 7] =
    ["Dashboard", "Topics", "Stream", "Query", "Nodes", "Liveliness", "Network"];

/// One key binding, used as the single source of truth for both the compact
/// status-bar hint and the `?` help overlay so they can't drift apart.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct KeyHint {
    pub keys: &'static str,
    pub desc: &'static str,
}

impl KeyHint {
    const fn new(keys: &'static str, desc: &'static str) -> Self {
        Self { keys, desc }
    }
}

/// Bindings available in every view.
pub(crate) fn global_hints() -> &'static [KeyHint] {
    const HINTS: &[KeyHint] = &[
        KeyHint::new("1-6", "switch view"),
        KeyHint::new("?", "toggle this help"),
        KeyHint::new("m", "connection mode"),
        KeyHint::new("P", "domain scan & switch"),
        KeyHint::new("Esc", "back to Dashboard"),
        KeyHint::new("q", "quit"),
    ];
    HINTS
}

/// Bindings specific to the given view.
pub(crate) fn view_hints(view: ActiveView) -> &'static [KeyHint] {
    const DASHBOARD: &[KeyHint] = &[KeyHint::new("click", "open item in its view")];
    const TOPICS: &[KeyHint] = &[
        KeyHint::new("j/k", "select topic"),
        KeyHint::new("J/K", "scroll detail"),
        KeyHint::new("Enter", "open in Stream"),
        KeyHint::new("/", "filter"),
        KeyHint::new("y/Y", "copy payload / key"),
    ];
    const STREAM: &[KeyHint] = &[
        KeyHint::new("j/k", "scroll messages"),
        KeyHint::new("Space", "pause / resume"),
        KeyHint::new("f", "follow latest"),
        KeyHint::new("/", "filter"),
        KeyHint::new("y/Y", "copy payload / key"),
    ];
    const QUERY: &[KeyHint] = &[
        KeyHint::new("i or /", "edit query"),
        KeyHint::new("Enter", "run query"),
        KeyHint::new("j/k", "select result"),
        KeyHint::new("y", "copy payload"),
    ];
    const NODES: &[KeyHint] = &[
        KeyHint::new("j/k", "select node"),
        KeyHint::new("J/K", "scroll detail"),
        KeyHint::new("s", "discover / refresh nodes"),
        KeyHint::new("y", "copy ZID"),
    ];
    const LIVELINESS: &[KeyHint] = &[
        KeyHint::new("j/k", "select token"),
        KeyHint::new("J/K", "scroll event log"),
        KeyHint::new("y", "copy key"),
    ];
    const NETWORK: &[KeyHint] = &[
        KeyHint::new("J/K", "scroll graph"),
        KeyHint::new("5", "node details"),
    ];
    match view {
        ActiveView::Dashboard => DASHBOARD,
        ActiveView::Topics => TOPICS,
        ActiveView::Stream => STREAM,
        ActiveView::Query => QUERY,
        ActiveView::Nodes => NODES,
        ActiveView::Liveliness => LIVELINESS,
        ActiveView::Network => NETWORK,
    }
}

/// Why a view is showing nothing — so empty states explain the cause and the
/// next action instead of an ambiguous blank panel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EmptyReason {
    Connecting,
    Disconnected,
    NoDataYet,
    FilteredOut,
}

/// (reason, next-action) text for an empty state. Understandable without color.
pub(crate) fn empty_state_text(reason: EmptyReason) -> (&'static str, &'static str) {
    match reason {
        EmptyReason::Connecting => (
            "Connecting to the network…",
            "Waiting for the session to come up.",
        ),
        EmptyReason::Disconnected => (
            "Not connected.",
            "Check the endpoint; press m to change mode or P to scan domains.",
        ),
        EmptyReason::NoDataYet => (
            "Connected, but no messages observed yet.",
            "Topics appear as messages arrive. Try Query (4) or Nodes (5), or ? for help.",
        ),
        EmptyReason::FilteredOut => (
            "Nothing matches the current filter.",
            "Press / to edit or clear the filter.",
        ),
    }
}

/// Compact status-bar hint: quit + view switch + the top few view bindings,
/// always ending with `?:help`. Wide terminals show more; the rest live in `?`.
pub(crate) fn compact_hint(view: ActiveView) -> String {
    let mut parts: Vec<String> = vec!["q:quit".into(), "1-7:view".into()];
    for h in view_hints(view).iter().take(3) {
        parts.push(format!("{}:{}", h.keys, h.desc.split(['/', ' ']).next().unwrap_or(h.desc)));
    }
    parts.push("?:help".into());
    parts.join("  ")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActiveView {
    Dashboard,
    Topics,
    Stream,
    Query,
    Nodes,
    Liveliness,
    Network,
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
            ActiveView::Network => 6,
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

#[derive(Debug, Clone)]
pub struct LivelinessEventRecord {
    pub timestamp: Instant,
    pub is_join: bool,
    pub key_expr: String,
    pub node_name: String,
    pub group: String,
}

const LIVELINESS_EVENT_CAP: usize = 200;

pub struct App {
    pub active_view: ActiveView,
    pub should_quit: bool,
    pub connection_state: ConnectionState,
    pub endpoint: String,
    pub tab_rects: [Option<ratatui::layout::Rect>; 7],

    pub topics: Vec<TopicInfo>,
    pub topic_latest: HashMap<String, (ZenohMessage, Instant)>,
    pub admin_nodes: Vec<NodeInfo>,
    pub scout_nodes: Vec<NodeInfo>,
    pub nodes: Vec<NodeInfo>,
    pub recent_messages: VecDeque<ZenohMessage>,

    pub sub_messages: VecDeque<ZenohMessage>,
    pub sub_paused: bool,
    pub sub_selected: usize,
    pub stream_follow: bool,
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
    pub node_detail_scroll: u16,
    pub scout_in_progress: bool,
    pub last_scout_at: Option<SystemTime>,
    pub pending_scout_request: bool,

    pub scout_port_modal_open: bool,
    pub scout_port_input: String,
    pub scout_port_current: Option<u16>,
    pub current_mode: ConnectMode,
    pub mode_modal_open: bool,
    pub mode_modal_selection: ConnectMode,
    pub pending_reconnect_mode: Option<ConnectMode>,
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

    pub self_zid: Option<String>,

    pub liveliness_tokens: Vec<LivelinessToken>,
    pub liveliness_selected: usize,
    pub liveliness_events: VecDeque<LivelinessEventRecord>,
    pub liveliness_log_scroll: u16,

    pub help_open: bool,
    pub help_scroll: u16,

    /// Dashboard summary-panel rects, for click-to-navigate.
    pub dash_node_rect: Option<ratatui::layout::Rect>,
    pub dash_topic_rect: Option<ratatui::layout::Rect>,

    pub network_scroll: u16,
}

impl App {
    pub fn new(endpoint: String) -> Self {
        Self {
            active_view: ActiveView::Dashboard,
            should_quit: false,
            connection_state: ConnectionState::Connecting,
            endpoint,
            tab_rects: [None; 7],
            topics: Vec::new(),
            topic_latest: HashMap::new(),
            admin_nodes: Vec::new(),
            scout_nodes: Vec::new(),
            nodes: Vec::new(),
            recent_messages: VecDeque::with_capacity(100),
            sub_messages: VecDeque::with_capacity(500),
            sub_paused: false,
            sub_selected: 0,
            stream_follow: true,
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
            node_detail_scroll: 0,
            scout_in_progress: false,
            last_scout_at: None,
            pending_scout_request: false,
            scout_port_modal_open: false,
            scout_port_input: String::new(),
            scout_port_current: None,
            current_mode: ConnectMode::Client,
            mode_modal_open: false,
            mode_modal_selection: ConnectMode::Client,
            pending_reconnect_mode: None,
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
            self_zid: None,
            liveliness_tokens: Vec::new(),
            liveliness_selected: 0,
            liveliness_events: VecDeque::with_capacity(LIVELINESS_EVENT_CAP),
            liveliness_log_scroll: 0,
            help_open: false,
            help_scroll: 0,
            dash_node_rect: None,
            dash_topic_rect: None,
            network_scroll: 0,
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

    /// Wipes all network-observation state (topics, messages, nodes) and resets
    /// associated UI selection indices. Called before reconnecting with a new
    /// mode so the previous session's data does not bleed into the new one.
    ///
    /// Does NOT clear liveliness state — the `ConnectResult::Connected` handler
    /// in `lib.rs` clears those fields after the new session is established.
    /// Does NOT clear query results, history, or user-entered filters, which
    /// are session-scoped user inputs that should survive a reconnect.
    pub fn clear_network_state(&mut self) {
        self.topics.clear();
        self.topic_latest.clear();
        self.topic_msg_counts.clear();
        self.topic_hz.clear();
        self.total_msg_count = 0;
        self.total_hz = 0.0;
        self.topic_selected = 0;
        self.topic_detail_scroll = 0;
        self.list_scroll_offset = 0;

        self.sub_messages.clear();
        self.recent_messages.clear();
        self.sub_selected = 0;

        self.admin_nodes.clear();
        self.scout_nodes.clear();
        self.nodes.clear();
        self.node_selected = 0;
        self.node_detail_scroll = 0;
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
            AppEvent::Liveliness(event) => self.handle_liveliness(event),
        }
    }

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
        if self.help_open {
            self.handle_help_key(key);
            return;
        }
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
                KeyCode::Char('?') => {
                    self.help_open = true;
                    self.help_scroll = 0;
                    return;
                }
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
                KeyCode::Char('1') => self.active_view = ActiveView::Dashboard,
                KeyCode::Char('2') => self.active_view = ActiveView::Topics,
                KeyCode::Char('3') => self.active_view = ActiveView::Stream,
                KeyCode::Char('4') => self.active_view = ActiveView::Query,
                KeyCode::Char('5') => self.active_view = ActiveView::Nodes,
                KeyCode::Char('6') => self.active_view = ActiveView::Liveliness,
                KeyCode::Char('7') => self.active_view = ActiveView::Network,
                KeyCode::Esc => {
                    self.active_view = ActiveView::Dashboard;
                }
                _ => self.handle_view_key(key),
            }
        } else {
            self.handle_text_input_key(key);
        }
    }

    fn handle_help_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') | KeyCode::Char('?') => {
                self.help_open = false;
                self.help_scroll = 0;
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.help_scroll = self.help_scroll.saturating_add(1);
            }
            KeyCode::Up | KeyCode::Char('k') => {
                self.help_scroll = self.help_scroll.saturating_sub(1);
            }
            _ => {}
        }
    }

    fn is_text_input_active(&self) -> bool {
        self.topics_filtering
            || self.stream_filtering
            || self.query_editing
            || self.scout_port_modal_open
            || self.mode_modal_open
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
                let label = match target {
                    ConnectMode::Peer => "peer",
                    ConnectMode::Client => "client",
                };
                if target == self.current_mode {
                    self.set_toast(format!("Already in {} mode", label));
                } else {
                    self.pending_reconnect_mode = Some(target);
                    self.set_toast(format!("Switching to {} mode...", label));
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

    /// Which Dashboard summary row (if any) a click landed on. Returns the
    /// target view and the item index; `None` for borders / empty rows / clicks
    /// outside both panels.
    pub(crate) fn dashboard_click_target(&self, col: u16, row: u16) -> Option<(ActiveView, usize)> {
        let in_rect = |rect: ratatui::layout::Rect| {
            col >= rect.x && col < rect.x + rect.width
        };
        if let Some(rect) = self.dash_node_rect {
            if in_rect(rect) {
                if let Some(idx) = list_hit(rect, row, 0, self.nodes.len(), rect.y + 1) {
                    return Some((ActiveView::Nodes, idx));
                }
            }
        }
        if let Some(rect) = self.dash_topic_rect {
            if in_rect(rect) {
                if let Some(idx) = list_hit(rect, row, 0, self.topics.len(), rect.y + 1) {
                    return Some((ActiveView::Topics, idx));
                }
            }
        }
        None
    }

    fn handle_click(&mut self, col: u16, row: u16) {
        if let Some(idx) = tab_hit(&self.tab_rects, col, row) {
            self.active_view = match idx {
                0 => ActiveView::Dashboard,
                1 => ActiveView::Topics,
                2 => ActiveView::Stream,
                3 => ActiveView::Query,
                4 => ActiveView::Nodes,
                5 => ActiveView::Liveliness,
                6 => ActiveView::Network,
                _ => self.active_view,
            };
            return;
        }

        // Dashboard summary panels are the entry point into the detailed views.
        if self.active_view == ActiveView::Dashboard {
            if let Some((view, idx)) = self.dashboard_click_target(col, row) {
                match view {
                    ActiveView::Nodes => {
                        self.active_view = ActiveView::Nodes;
                        self.node_selected = idx;
                        self.node_detail_scroll = 0;
                    }
                    ActiveView::Topics => {
                        // Clear any filter so the clicked index maps directly.
                        self.topic_filter.clear();
                        self.active_view = ActiveView::Topics;
                        self.topic_selected = idx;
                        self.topic_detail_scroll = 0;
                    }
                    _ => {}
                }
            }
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
            ActiveView::Liveliness => self.liveliness_tokens.len(),
            ActiveView::Dashboard | ActiveView::Network => return,
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
            ActiveView::Stream => self.pin_stream_at(idx),
            ActiveView::Query => self.query_selected = idx,
            ActiveView::Nodes => self.node_selected = idx,
            ActiveView::Liveliness => {
                self.liveliness_selected = idx;
                self.liveliness_log_scroll = 0;
            }
            ActiveView::Dashboard | ActiveView::Network => {}
        }
    }

    fn handle_wheel_up(&mut self) {
        match self.active_view {
            ActiveView::Topics => {
                self.topic_selected = self.topic_selected.saturating_sub(1);
                self.topic_detail_scroll = 0;
            }
            ActiveView::Stream => {
                self.pin_stream_at(self.sub_selected.saturating_sub(1));
            }
            ActiveView::Query => {
                self.query_selected = self.query_selected.saturating_sub(1);
            }
            ActiveView::Nodes => {
                self.node_selected = self.node_selected.saturating_sub(1);
            }
            ActiveView::Liveliness => {
                self.liveliness_selected = self.liveliness_selected.saturating_sub(1);
            }
            ActiveView::Network => {
                self.network_scroll = self.network_scroll.saturating_sub(1);
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
                    self.pin_stream_at(self.sub_selected + 1);
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
            ActiveView::Liveliness => {
                let max = self.liveliness_tokens.len().saturating_sub(1);
                if self.liveliness_selected < max {
                    self.liveliness_selected += 1;
                }
            }
            ActiveView::Network => {
                self.network_scroll = self.network_scroll.saturating_add(1);
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
        // Detail-panel scroll (uppercase J/K) is uniform across views.
        if let Some(action) = detail_scroll_action(key) {
            match self.active_view {
                ActiveView::Topics => {
                    self.topic_detail_scroll = apply_detail_scroll(self.topic_detail_scroll, action)
                }
                ActiveView::Nodes => {
                    self.node_detail_scroll = apply_detail_scroll(self.node_detail_scroll, action)
                }
                ActiveView::Liveliness => {
                    self.liveliness_log_scroll =
                        apply_detail_scroll(self.liveliness_log_scroll, action)
                }
                ActiveView::Network => {
                    self.network_scroll = apply_detail_scroll(self.network_scroll, action)
                }
                _ => {}
            }
            return;
        }

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
                KeyCode::Char('f') => self.follow_stream(),
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
                    self.pin_stream_at(self.sub_selected.saturating_sub(1));
                }
                KeyCode::Down | KeyCode::Char('j') => {
                    let max = self.filtered_sub_messages().len().saturating_sub(1);
                    if self.sub_selected < max {
                        self.pin_stream_at(self.sub_selected + 1);
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
                    self.node_detail_scroll = 0;
                }
                KeyCode::Down | KeyCode::Char('j') => {
                    let max = self.nodes.len().saturating_sub(1);
                    if self.node_selected < max {
                        self.node_selected += 1;
                        self.node_detail_scroll = 0;
                    }
                }
                KeyCode::Char('s') => {
                    if !self.scout_in_progress {
                        self.pending_scout_request = true;
                    }
                }
                _ => {}
            },
            ActiveView::Liveliness => match key.code {
                KeyCode::Char('y') => {
                    if let Some(token) = self.liveliness_tokens.get(self.liveliness_selected).cloned() {
                        self.copy_to_clipboard(token.key_expr, "key_expr");
                    } else {
                        self.set_error_toast("No token selected");
                    }
                }
                KeyCode::Up | KeyCode::Char('k') => {
                    self.liveliness_selected = self.liveliness_selected.saturating_sub(1);
                }
                KeyCode::Down | KeyCode::Char('j') => {
                    let max = self.liveliness_tokens.len().saturating_sub(1);
                    if self.liveliness_selected < max {
                        self.liveliness_selected += 1;
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
            if !self.stream_follow && matches_stream_filter && self.sub_selected > 0 {
                self.sub_selected += 1;
            }
            self.clamp_stream_selection();
            if self.stream_follow {
                self.sub_selected = 0;
            }
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

    /// Connection-level empty reason (connecting/disconnected), or `None` when
    /// the session is up and the emptiness is view-specific.
    fn connection_empty_reason(&self) -> Option<EmptyReason> {
        match &self.connection_state {
            ConnectionState::Connecting => Some(EmptyReason::Connecting),
            ConnectionState::Disconnected(_) => Some(EmptyReason::Disconnected),
            ConnectionState::Connected(_) => None,
        }
    }

    /// Why the Stream message list is empty (only meaningful when it is).
    pub(crate) fn stream_empty_reason(&self) -> EmptyReason {
        if let Some(r) = self.connection_empty_reason() {
            return r;
        }
        if !self.stream_filter.is_empty() && !self.sub_messages.is_empty() {
            EmptyReason::FilteredOut
        } else {
            EmptyReason::NoDataYet
        }
    }

    /// Why the Topics list is empty (only meaningful when it is).
    pub(crate) fn topics_empty_reason(&self) -> EmptyReason {
        if let Some(r) = self.connection_empty_reason() {
            return r;
        }
        if !self.topic_filter.is_empty() && !self.topics.is_empty() {
            EmptyReason::FilteredOut
        } else {
            EmptyReason::NoDataYet
        }
    }

    /// Why the Nodes list is empty (only meaningful when it is).
    pub(crate) fn nodes_empty_reason(&self) -> EmptyReason {
        if let Some(r) = self.connection_empty_reason() {
            return r;
        }
        EmptyReason::NoDataYet
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

    fn follow_stream(&mut self) {
        self.stream_follow = true;
        self.sub_selected = 0;
    }

    fn pin_stream_at(&mut self, idx: usize) {
        self.stream_follow = false;
        self.sub_selected = idx;
        self.clamp_stream_selection();
    }

    pub fn render(&mut self, frame: &mut Frame) {
        let [tabs_area, content_area, status_area] = Layout::vertical([
            Constraint::Length(3),
            Constraint::Fill(1),
            Constraint::Length(1),
        ])
        .areas(frame.area());

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

        match self.active_view {
            ActiveView::Dashboard => views::dashboard::render(self, frame, content_area),
            ActiveView::Topics => views::topics::render(self, frame, content_area),
            ActiveView::Stream => views::stream::render(self, frame, content_area),
            ActiveView::Query => views::query::render(self, frame, content_area),
            ActiveView::Nodes => views::nodes::render(self, frame, content_area),
            ActiveView::Liveliness => views::liveliness::render(self, frame, content_area),
            ActiveView::Network => views::network::render(self, frame, content_area),
        }

        if self.scout_port_modal_open {
            self.render_scout_port_modal(frame, content_area);
        }
        if self.mode_modal_open {
            self.render_mode_modal(frame, content_area);
        }
        if self.help_open {
            self.render_help_overlay(frame, content_area);
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
            Some(p) => format!(" {} ", domain_port_label(p)),
            None => " domain 0 (port 7446) ".to_string(),
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
                format!(" {} ", compact_hint(self.active_view)),
                Style::default().fg(Color::DarkGray),
            ),
        ]);
        frame.render_widget(status, status_area);
    }

    fn render_help_overlay(&self, frame: &mut Frame, content_area: Rect) {
        let width = 60.min(content_area.width.saturating_sub(2));
        let height = 24.min(content_area.height.saturating_sub(2));
        if width < 24 || height < 6 {
            return;
        }
        let x = content_area.x + (content_area.width - width) / 2;
        let y = content_area.y + (content_area.height - height) / 2;
        let popup = Rect::new(x, y, width, height);

        frame.render_widget(Clear, popup);
        let view_name = TAB_TITLES[self.active_view.index()];
        let block = Block::default()
            .borders(Borders::ALL)
            .title(format!(" Keybindings — {} ", view_name))
            .style(Style::default().fg(Color::White).bg(Color::Black));
        let inner = block.inner(popup);
        frame.render_widget(block, popup);

        let mut lines: Vec<Line> = Vec::new();
        let section = |lines: &mut Vec<Line>, name: &str, hints: &[KeyHint]| {
            lines.push(Line::from(Span::styled(
                name.to_string(),
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            )));
            for h in hints {
                lines.push(Line::from(vec![
                    Span::styled(
                        format!("  {:<9}", h.keys),
                        Style::default().fg(Color::Yellow),
                    ),
                    Span::styled(h.desc.to_string(), Style::default().fg(Color::White)),
                ]));
            }
            lines.push(Line::from(""));
        };
        section(&mut lines, &format!("{} view", view_name), view_hints(self.active_view));
        section(&mut lines, "Global", global_hints());
        lines.push(Line::from(Span::styled(
            "j/k or ↑↓ to scroll · Esc/q/? to close",
            Style::default().fg(Color::DarkGray),
        )));

        let max_scroll = (lines.len() as u16).saturating_sub(inner.height);
        let scroll = self.help_scroll.min(max_scroll);
        let para = Paragraph::new(lines)
            .wrap(Wrap { trim: false })
            .scroll((scroll, 0));
        frame.render_widget(para, inner);
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
            .title(" Domain Scan & Switch ")
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
            Some(p) => format!("Current: {}", domain_port_label(p)),
            None => "Current: domain 0 (port 7446, default)".to_string(),
        };
        frame.render_widget(
            Paragraph::new(current_text).style(Style::default().fg(Color::Gray)),
            current_row,
        );

        let input_text = if self.scout_port_input.is_empty() {
            "Custom port: _".to_string()
        } else {
            format!("Custom port: {}_", self.scout_port_input)
        };
        frame.render_widget(
            Paragraph::new(input_text).style(Style::default().fg(Color::Cyan)),
            input_row,
        );

        if self.port_scan_in_progress {
            frame.render_widget(
                Paragraph::new("Scanning domains 0-100 (ports 7446-7546) ...")
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
                    Paragraph::new("Press 's' to scan domains 0-100 for nodes")
                        .style(Style::default().fg(Color::DarkGray)),
                    list_area,
                );
            } else if hits.is_empty() {
                frame.render_widget(
                    Paragraph::new("No nodes found in domains 0-100")
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
                        let is_self = matches!(
                            &self.connection_state,
                            ConnectionState::Connected(zid) if r.nodes.iter().any(|n| n.zid == *zid)
                        );
                        let base_text = format!(
                            "{}{}  {} node(s)",
                            marker,
                            domain_port_label(r.port),
                            r.nodes.len()
                        );
                        let mut spans = vec![Span::styled(
                            base_text,
                            if selected {
                                Style::default()
                                    .fg(Color::Black)
                                    .bg(Color::Cyan)
                                    .add_modifier(Modifier::BOLD)
                            } else {
                                Style::default().fg(Color::White)
                            },
                        )];
                        if is_self {
                            spans.push(Span::styled(
                                "  (self)",
                                Style::default()
                                    .fg(Color::Green)
                                    .add_modifier(Modifier::BOLD),
                            ));
                        }
                        Line::from(spans)
                    })
                    .collect();
                frame.render_widget(Paragraph::new(lines), list_area);
            }
        }

        frame.render_widget(
            Paragraph::new(" s:scan domains  Enter:switch  jk/↑↓:select  Esc:close ")
                .style(Style::default().fg(Color::DarkGray)),
            hint_row,
        );
    }

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

        let (peer_marker, client_marker) = match self.mode_modal_selection {
            ConnectMode::Peer => ("> [*] Peer", "  [ ] Client"),
            ConnectMode::Client => ("  [ ] Peer", "> [*] Client"),
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::KeyModifiers;
    use ratatui::layout::Rect;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    #[test]
    fn uppercase_j_k_map_to_detail_scroll_without_shift() {
        // Uppercase J/K scroll regardless of whether Shift is reported.
        assert_eq!(detail_scroll_action(key(KeyCode::Char('J'))), Some(DetailScroll::Down));
        assert_eq!(detail_scroll_action(key(KeyCode::Char('K'))), Some(DetailScroll::Up));
        assert_eq!(
            detail_scroll_action(KeyEvent::new(KeyCode::Char('J'), KeyModifiers::SHIFT)),
            Some(DetailScroll::Down)
        );
    }

    #[test]
    fn lowercase_j_k_are_not_detail_scroll() {
        assert_eq!(detail_scroll_action(key(KeyCode::Char('j'))), None);
        assert_eq!(detail_scroll_action(key(KeyCode::Char('k'))), None);
    }

    fn node(zid: &str) -> NodeInfo {
        NodeInfo {
            zid: zid.into(),
            kind: "peer".into(),
            locators: vec![],
            metadata: None,
            sources: zemon_core::types::NodeSources::ADMIN,
            admin_last_seen: None,
            scout_last_seen: None,
        }
    }

    #[test]
    fn key_7_switches_to_network_view() {
        let mut app = App::new("test".into());
        app.handle_key(key(KeyCode::Char('7')));
        assert_eq!(app.active_view, ActiveView::Network);
        // J/K scroll the graph in the Network view.
        app.handle_key(key(KeyCode::Char('J')));
        assert_eq!(app.network_scroll, 3);
    }

    #[test]
    fn dashboard_click_targets_node_and_topic_rows() {
        let mut app = App::new("t".into());
        app.nodes = vec![node("a"), node("b")];
        app.topics = vec![
            TopicInfo { key_expr: "x".into() },
            TopicInfo { key_expr: "y".into() },
        ];
        app.dash_node_rect = Some(Rect::new(0, 5, 40, 10));
        app.dash_topic_rect = Some(Rect::new(40, 5, 40, 10));

        // First item row is rect.y + 1 = 6.
        assert_eq!(app.dashboard_click_target(5, 6), Some((ActiveView::Nodes, 0)));
        assert_eq!(app.dashboard_click_target(5, 7), Some((ActiveView::Nodes, 1)));
        assert_eq!(app.dashboard_click_target(45, 6), Some((ActiveView::Topics, 0)));
        // Border row → no-op.
        assert_eq!(app.dashboard_click_target(5, 5), None);
        // Past the last item → no-op.
        assert_eq!(app.dashboard_click_target(5, 8), None);
    }

    #[test]
    fn empty_reason_reflects_connection_state() {
        let mut app = App::new("test".into());
        app.connection_state = ConnectionState::Connecting;
        assert_eq!(app.stream_empty_reason(), EmptyReason::Connecting);
        app.connection_state = ConnectionState::Disconnected("x".into());
        assert_eq!(app.nodes_empty_reason(), EmptyReason::Disconnected);
    }

    #[test]
    fn empty_reason_distinguishes_filter_from_no_data() {
        let mut app = App::new("test".into());
        app.connection_state = ConnectionState::Connected("zid".into());
        // No data at all → NoDataYet.
        assert_eq!(app.topics_empty_reason(), EmptyReason::NoDataYet);
        // Data exists but the filter hides it → FilteredOut.
        app.topics.push(TopicInfo { key_expr: "a/b".into() });
        app.topic_filter = "zzz".into();
        assert_eq!(app.topics_empty_reason(), EmptyReason::FilteredOut);
    }

    #[test]
    fn question_mark_toggles_help_in_normal_mode() {
        let mut app = App::new("test".into());
        assert!(!app.help_open);
        app.handle_key(key(KeyCode::Char('?')));
        assert!(app.help_open);
        // q closes the overlay (does not quit)
        app.handle_key(key(KeyCode::Char('q')));
        assert!(!app.help_open);
        assert!(!app.should_quit);
    }

    #[test]
    fn question_mark_is_literal_during_text_input() {
        let mut app = App::new("test".into());
        app.active_view = ActiveView::Query;
        app.query_editing = true; // text input active
        app.handle_key(key(KeyCode::Char('?')));
        assert!(!app.help_open, "? must not open help while editing");
    }

    #[test]
    fn help_scrolls_and_esc_closes() {
        let mut app = App::new("test".into());
        app.handle_key(key(KeyCode::Char('?')));
        app.handle_key(key(KeyCode::Char('j')));
        assert_eq!(app.help_scroll, 1);
        app.handle_key(key(KeyCode::Char('k')));
        assert_eq!(app.help_scroll, 0);
        app.handle_key(key(KeyCode::Esc));
        assert!(!app.help_open);
    }

    #[test]
    fn every_view_has_hints_and_compact_ends_with_help() {
        for v in [
            ActiveView::Dashboard,
            ActiveView::Topics,
            ActiveView::Stream,
            ActiveView::Query,
            ActiveView::Nodes,
            ActiveView::Liveliness,
        ] {
            assert!(!view_hints(v).is_empty(), "{v:?} has no hints");
            assert!(compact_hint(v).ends_with("?:help"));
        }
        assert!(global_hints().iter().any(|h| h.keys == "q"));
    }

    #[test]
    fn domain_port_label_maps_domain_range() {
        assert_eq!(domain_port_label(7446), "domain 0 (port 7446)");
        assert_eq!(domain_port_label(7450), "domain 4 (port 7450)");
        assert_eq!(domain_port_label(7546), "domain 100 (port 7546)");
    }

    #[test]
    fn domain_port_label_treats_out_of_range_as_custom_port() {
        // Below 7446 must not be misreported as "domain 0".
        assert_eq!(domain_port_label(7000), "port 7000 (custom)");
        assert_eq!(domain_port_label(8000), "port 8000 (custom)");
    }

    #[test]
    fn apply_detail_scroll_saturates_at_zero() {
        assert_eq!(apply_detail_scroll(0, DetailScroll::Up), 0);
        assert_eq!(apply_detail_scroll(0, DetailScroll::Down), 3);
        assert_eq!(apply_detail_scroll(5, DetailScroll::Up), 2);
    }

    #[test]
    fn topics_detail_scroll_works_without_shift_modifier() {
        let mut app = App::new("test".into());
        app.active_view = ActiveView::Topics;
        app.handle_key(key(KeyCode::Char('J')));
        assert_eq!(app.topic_detail_scroll, 3);
        app.handle_key(key(KeyCode::Char('K')));
        assert_eq!(app.topic_detail_scroll, 0);
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

    #[test]
    fn tab_hit_inside_rect_returns_index() {
        let rects = [
            Some(Rect::new(1, 0, 14, 3)),
            Some(Rect::new(16, 0, 10, 3)),
            Some(Rect::new(28, 0, 12, 3)),
            None,
            None,
            None,
            None,
        ];
        assert_eq!(tab_hit(&rects, 2, 1), Some(0));
        assert_eq!(tab_hit(&rects, 20, 1), Some(1));
        assert_eq!(tab_hit(&rects, 30, 2), Some(2));
    }

    #[test]
    fn tab_hit_outside_returns_none() {
        let rects = [Some(Rect::new(1, 0, 14, 3)), None, None, None, None, None, None];
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
            payload: zemon_core::types::MessagePayload::from_json(&serde_json::json!(null)),
            encoding: String::new(),
            payload_bytes: 0,
            timestamp: None,
            kind: "put".into(),
            attachment: None,
            attachment_bytes: None,
        };
        app.handle_zenoh_message(msg);
        assert_eq!(app.sub_selected, 0);
    }

    #[test]
    fn sub_selected_nonzero_follows_message_through_shift() {
        let mut app = App::new("test".into());
        let make = |k: &str| ZenohMessage {
            key_expr: k.into(),
            payload: zemon_core::types::MessagePayload::from_json(&serde_json::json!(null)),
            encoding: String::new(),
            payload_bytes: 0,
            timestamp: None,
            kind: "put".into(),
            attachment: None,
            attachment_bytes: None,
        };
        app.handle_zenoh_message(make("a"));
        app.handle_zenoh_message(make("b"));
        app.handle_zenoh_message(make("c"));
        app.pin_stream_at(1);
        app.handle_zenoh_message(make("d"));
        assert!(!app.stream_follow);
        assert_eq!(app.sub_selected, 2);
    }

    #[test]
    fn filtered_sub_messages_match_key_and_payload() {
        let mut app = App::new("test".into());
        app.handle_zenoh_message(ZenohMessage {
            key_expr: "robot/pose".into(),
            payload: zemon_core::types::MessagePayload::from_json(&serde_json::json!({"x": 1})),
            encoding: String::new(),
            payload_bytes: 0,
            timestamp: None,
            kind: "put".into(),
            attachment: None,
            attachment_bytes: None,
        });
        app.handle_zenoh_message(ZenohMessage {
            key_expr: "robot/status".into(),
            payload: zemon_core::types::MessagePayload::from_json(&serde_json::json!("idle")),
            encoding: String::new(),
            payload_bytes: 0,
            timestamp: None,
            kind: "put".into(),
            attachment: None,
            attachment_bytes: None,
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
            payload: zemon_core::types::MessagePayload::from_json(&serde_json::json!(null)),
            encoding: String::new(),
            payload_bytes: 0,
            timestamp: None,
            kind: "put".into(),
            attachment: None,
            attachment_bytes: None,
        };
        app.handle_zenoh_message(make("alpha/1"));
        app.handle_zenoh_message(make("beta/1"));
        app.handle_zenoh_message(make("alpha/2"));

        app.stream_filter = "alpha".into();
        app.pin_stream_at(1);

        app.handle_zenoh_message(make("beta/2"));
        assert_eq!(app.sub_selected, 1);

        app.handle_zenoh_message(make("alpha/3"));
        assert_eq!(app.sub_selected, 2);
    }

    #[test]
    fn follow_stream_resets_selection_to_latest() {
        let mut app = App::new("test".into());
        app.stream_follow = false;
        app.sub_selected = 3;
        app.follow_stream();
        assert!(app.stream_follow);
        assert_eq!(app.sub_selected, 0);
    }

    #[test]
    fn pin_stream_disables_follow() {
        let mut app = App::new("test".into());
        app.pin_stream_at(2);
        assert!(!app.stream_follow);
        assert_eq!(app.sub_selected, 0);
    }

    #[test]
    fn clear_network_state_clears_topics_messages_and_nodes() {
        let mut app = App::new("test".into());
        let make = |k: &str| ZenohMessage {
            key_expr: k.into(),
            payload: zemon_core::types::MessagePayload::from_json(&serde_json::json!(null)),
            encoding: String::new(),
            payload_bytes: 0,
            timestamp: None,
            kind: "put".into(),
            attachment: None,
            attachment_bytes: None,
        };
        app.handle_zenoh_message(make("a"));
        app.handle_zenoh_message(make("b"));
        app.total_msg_count = 7;
        app.total_hz = 3.5;
        app.topic_selected = 1;
        app.topic_detail_scroll = 4;
        app.list_scroll_offset = 5;
        app.sub_selected = 1;
        app.admin_nodes.push(zemon_core::types::NodeInfo {
            zid: "z1".into(),
            kind: "router".into(),
            locators: vec![],
            metadata: None,
            sources: zemon_core::types::NodeSources::default(),
            admin_last_seen: None,
            scout_last_seen: None,
        });
        app.scout_nodes.push(zemon_core::types::NodeInfo {
            zid: "z2".into(),
            kind: "peer".into(),
            locators: vec![],
            metadata: None,
            sources: zemon_core::types::NodeSources::default(),
            admin_last_seen: None,
            scout_last_seen: None,
        });
        app.nodes = zemon_core::merge::merge_nodes(&app.admin_nodes, &app.scout_nodes);
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
        assert_eq!(app.list_scroll_offset, 0);

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
            payload: zemon_core::types::MessagePayload::from_json(&serde_json::json!(1)),
            encoding: String::new(),
            payload_bytes: 0,
            timestamp: None,
            kind: "get".into(),
            attachment: None,
            attachment_bytes: None,
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
}
