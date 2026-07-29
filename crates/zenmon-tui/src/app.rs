use crate::event::AppEvent;
use crate::views;
use crossterm::event::{KeyCode, KeyEvent, MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Wrap};
use ratatui::Frame;
use std::collections::{HashMap, VecDeque};
use std::time::{Instant, SystemTime};
use zenmon_core::config::ConnectMode;
use zenmon_core::merge::merge_nodes;
use zenmon_core::types::{
    LivelinessToken, MessagePayload, NodeInfo, PortScoutResult, TopicInfo, ZenohMessage,
};

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
#[allow(dead_code)]
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

/// A top-level navigation space. The redesign folds the old seven tabs into two
/// drill-down spaces (see the TUI redesign spec).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Space {
    Traffic,
    Network,
}

impl Space {
    pub fn index(self) -> usize {
        match self {
            Space::Traffic => 0,
            Space::Network => 1,
        }
    }
    pub fn next(self) -> Self {
        match self {
            Space::Traffic => Space::Network,
            Space::Network => Space::Traffic,
        }
    }
}

pub const SPACE_TITLES: [&str; 2] = ["Traffic", "Network"];

/// A modal overlay drawn on top of the active space. `Palette` (phase 2) and
/// `Doctor` (phase 5) are added in later phases.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Overlay {
    None,
    Help,
}

/// Which pane holds focus when the terminal is too narrow to show both.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PaneFocus {
    Master,
    Detail,
}

/// Which mode the Traffic detail pane shows for the selected key: `Live` (latest
/// payload + scrolling history) or `Query` (results of a `get` on the key).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DetailMode {
    Live,
    Query,
}

/// A selectable row in the unified Network participant list: either a transport
/// **Session** (index into `nodes`) or a liveliness **Service** (index into
/// `liveliness_tokens`). Section/group headers are drawn by the view and are not
/// part of this list — `network_selected` is a cursor over these rows only.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NetworkRow {
    Session(usize),
    Service(usize),
}

/// Minimum body width (columns) at which master and detail render side by side.
/// Below this the space shows one pane at a time (Termux/tablet target).
pub(crate) const TWO_PANE_MIN_WIDTH: u16 = 90;

pub(crate) fn is_narrow(width: u16) -> bool {
    width < TWO_PANE_MIN_WIDTH
}

/// How the active space splits its body area, given terminal width and focus.
pub(crate) enum BodyLayout {
    Split { master: Rect, detail: Rect },
    Single { pane: Rect, focus: PaneFocus },
}

/// How many 1-second buckets of rate history to keep for sparklines.
const RATE_WINDOW_SECS: usize = 30;

/// A bounded ring buffer of per-second byte counts, used for bandwidth
/// sparklines. Idle seconds are recorded as `0` buckets so a topic that goes
/// quiet shows a real dip rather than a stale value.
#[derive(Debug, Clone)]
pub(crate) struct RateWindow {
    samples: std::collections::VecDeque<u64>,
    cap: usize,
}

impl RateWindow {
    pub(crate) fn new(cap: usize) -> Self {
        Self {
            samples: std::collections::VecDeque::new(),
            cap,
        }
    }

    /// Record one completed 1-second bucket, evicting the oldest beyond `cap`.
    pub(crate) fn push(&mut self, bytes: u64) {
        self.samples.push_back(bytes);
        while self.samples.len() > self.cap {
            self.samples.pop_front();
        }
    }

    pub(crate) fn latest(&self) -> u64 {
        self.samples.back().copied().unwrap_or(0)
    }

    pub(crate) fn is_all_zero(&self) -> bool {
        self.samples.iter().all(|&b| b == 0)
    }

    pub(crate) fn series(&self) -> Vec<u64> {
        self.samples.iter().copied().collect()
    }
}

/// Human-readable application-payload bandwidth (not protocol overhead).
pub(crate) fn format_bytes_per_sec(bytes: u64) -> String {
    let b = bytes as f64;
    if b >= 1_048_576.0 {
        format!("{:.1} MB/s", b / 1_048_576.0)
    } else if b >= 1024.0 {
        format!("{:.1} KB/s", b / 1024.0)
    } else {
        format!("{} B/s", bytes)
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
    pub space: Space,
    pub overlay: Overlay,
    pub pane_focus: PaneFocus,
    pub detail_mode: DetailMode,
    pub should_quit: bool,
    pub connection_state: ConnectionState,
    pub endpoint: String,
    pub space_tab_rects: [Option<ratatui::layout::Rect>; 2],

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
    /// Exact key selected through Topics → Stream navigation. When present,
    /// this takes precedence over the general substring filter.
    pub stream_key_filter: Option<String>,
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

    // Application-payload bandwidth accounting (bytes since last bucket) and
    // per-second history for sparklines.
    pub(crate) topic_byte_counts: HashMap<String, u64>,
    pub(crate) total_byte_count: u64,
    pub(crate) total_rate: RateWindow,
    pub(crate) topic_rates: HashMap<String, RateWindow>,

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

    pub help_scroll: u16,

    /// Dashboard summary-panel rects, for click-to-navigate.
    pub dash_node_rect: Option<ratatui::layout::Rect>,
    pub dash_topic_rect: Option<ratatui::layout::Rect>,

    pub network_scroll: u16,

    /// Cursor over the unified Network participant list (see [`NetworkRow`]).
    pub network_selected: usize,
}

impl App {
    pub fn new(endpoint: String) -> Self {
        Self {
            space: Space::Traffic,
            overlay: Overlay::None,
            pane_focus: PaneFocus::Master,
            detail_mode: DetailMode::Live,
            should_quit: false,
            connection_state: ConnectionState::Connecting,
            endpoint,
            space_tab_rects: [None; 2],
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
            stream_key_filter: None,
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
            topic_byte_counts: HashMap::new(),
            total_byte_count: 0,
            total_rate: RateWindow::new(RATE_WINDOW_SECS),
            topic_rates: HashMap::new(),
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
            help_scroll: 0,
            dash_node_rect: None,
            dash_topic_rect: None,
            network_scroll: 0,
            network_selected: 0,
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

    #[cfg(feature = "clipboard")]
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

    // Built without the `clipboard` feature (e.g. Termux/Android, no X11): keep
    // the keybinding responsive but tell the user copy is unavailable here.
    #[cfg(not(feature = "clipboard"))]
    fn copy_to_clipboard(&mut self, _text: String, _label: &str) {
        self.set_error_toast("Clipboard not supported in this build".to_string());
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

    fn handle_liveliness(&mut self, event: zenmon_core::types::LivelinessEvent) {
        use zenmon_core::types::LivelinessEvent;
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
            KeyCode::Tab => self.switch_space(self.space.next()),
            KeyCode::Char('1') => self.switch_space(Space::Traffic),
            KeyCode::Char('2') => self.switch_space(Space::Network),
            KeyCode::Enter | KeyCode::Right => self.pane_focus = PaneFocus::Detail,
            KeyCode::Esc | KeyCode::Left => self.pane_focus = PaneFocus::Master,
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
        match key.code {
            KeyCode::Char('j') | KeyCode::Down => self.move_topic_selection(1),
            KeyCode::Char('k') | KeyCode::Up => self.move_topic_selection(-1),
            KeyCode::Char('/') => self.topics_filtering = true,
            KeyCode::Char('L') => self.detail_mode = DetailMode::Live,
            KeyCode::Char('Q') => {
                self.detail_mode = DetailMode::Query;
                if let Some(key_expr) = self.selected_topic_key() {
                    self.query_history.push(key_expr.clone());
                    self.pending_query = Some(key_expr);
                }
            }
            KeyCode::Char(' ') => self.sub_paused = !self.sub_paused,
            KeyCode::Char('f') => {
                self.follow_stream();
                self.topic_detail_scroll = 0;
            }
            KeyCode::Char('y') => self.copy_selected_payload(),
            KeyCode::Char('Y') => self.copy_selected_key(),
            _ => {}
        }
    }

    /// Move the master cursor within `filtered_topics()`, clamped to bounds, and
    /// reset the detail scroll so the new selection starts at the top.
    fn move_topic_selection(&mut self, delta: isize) {
        let len = self.filtered_topics().len();
        if len == 0 {
            return;
        }
        let cur = self.topic_selected.min(len - 1) as isize;
        self.topic_selected = (cur + delta).clamp(0, len as isize - 1) as usize;
        self.topic_detail_scroll = 0;
    }

    /// The key expression currently selected in the Traffic master, if any.
    fn selected_topic_key(&self) -> Option<String> {
        self.filtered_topics()
            .get(self.topic_selected)
            .map(|t| t.key_expr.clone())
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
            KeyCode::Char('s') => {
                if !self.scout_in_progress {
                    self.pending_scout_request = true;
                }
            }
            KeyCode::Char('y') => self.copy_selected_participant(),
            _ => {}
        }
    }

    /// Move the unified cursor within `network_rows()`, clamped to bounds, and
    /// reset the detail scroll so the new selection starts at the top.
    fn move_network_selection(&mut self, delta: isize) {
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
            Some(NetworkRow::Service(i)) => {
                let key = self.liveliness_tokens[i].key_expr.clone();
                self.copy_to_clipboard(key, "key");
            }
            None => self.set_error_toast("No participant selected to copy"),
        }
    }

    /// The unified list of selectable participant rows, in display order: every
    /// transport session (in node order) followed by every liveliness service.
    /// Services are ordered so tokens of the same group are contiguous (groups in
    /// first-appearance order), which is exactly the order the view draws them —
    /// so `network_selected` maps 1:1 onto the rendered rows.
    pub(crate) fn network_rows(&self) -> Vec<NetworkRow> {
        let mut rows: Vec<NetworkRow> = (0..self.nodes.len()).map(NetworkRow::Session).collect();

        let mut group_order: Vec<String> = Vec::new();
        let mut by_group: HashMap<String, Vec<usize>> = HashMap::new();
        for (i, token) in self.liveliness_tokens.iter().enumerate() {
            let group = token
                .group_prefix()
                .unwrap_or_else(|| "(ungrouped)".to_string());
            if !by_group.contains_key(&group) {
                group_order.push(group.clone());
            }
            by_group.entry(group).or_default().push(i);
        }
        for group in &group_order {
            for &i in &by_group[group] {
                rows.push(NetworkRow::Service(i));
            }
        }
        rows
    }

    /// The participant row under the unified cursor, if any.
    pub(crate) fn selected_network_row(&self) -> Option<NetworkRow> {
        self.network_rows().get(self.network_selected).copied()
    }

    fn switch_space(&mut self, space: Space) {
        self.space = space;
        self.pane_focus = PaneFocus::Master;
    }

    fn is_text_input_active(&self) -> bool {
        self.topics_filtering
            || self.stream_filtering
            || self.query_editing
            || self.scout_port_modal_open
            || self.mode_modal_open
    }

    fn handle_mouse(&mut self, ev: MouseEvent) {
        if self.is_text_input_active() {
            return;
        }
        if let MouseEventKind::Down(MouseButton::Left) = ev.kind {
            self.handle_click(ev.column, ev.row);
        }
    }

    fn handle_click(&mut self, col: u16, row: u16) {
        if let Some(idx) = space_tab_hit(&self.space_tab_rects, col, row) {
            self.space = match idx {
                0 => Space::Traffic,
                _ => Space::Network,
            };
            self.pane_focus = PaneFocus::Master;
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
                    self.stream_key_filter = None;
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
                    self.stream_key_filter = None;
                    self.stream_filter.pop();
                    self.clamp_stream_selection();
                } else if self.query_editing {
                    self.query_input.pop();
                }
            }
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

        *self
            .topic_msg_counts
            .entry(msg.key_expr.clone())
            .or_insert(0) += 1;
        self.total_msg_count += 1;

        // Application payload bytes (not protocol overhead), from the lossless
        // wire byte counts captured at receive time.
        let bytes = (msg.payload_bytes + msg.attachment_bytes.unwrap_or(0)) as u64;
        *self
            .topic_byte_counts
            .entry(msg.key_expr.clone())
            .or_insert(0) += bytes;
        self.total_byte_count += bytes;

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
        if elapsed < 1.0 {
            return;
        }

        // Every topic we already track a rate for gets a bucket this interval —
        // even if it received nothing (a 0 bucket) — so sparklines show real
        // dips and idle Hz decays to 0 instead of sticking at its last value.
        let mut keys: std::collections::HashSet<String> =
            self.topic_rates.keys().cloned().collect();
        keys.extend(self.topic_msg_counts.keys().cloned());

        for key in keys {
            let msgs = self.topic_msg_counts.get(&key).copied().unwrap_or(0);
            let bytes = self.topic_byte_counts.get(&key).copied().unwrap_or(0);
            self.topic_hz.insert(key.clone(), msgs as f64 / elapsed);
            self.topic_rates
                .entry(key.clone())
                .or_insert_with(|| RateWindow::new(RATE_WINDOW_SECS))
                .push(bytes);
            // Evict topics idle for the whole window to bound memory.
            if self.topic_rates.get(&key).is_some_and(|w| w.is_all_zero()) {
                self.topic_rates.remove(&key);
                self.topic_hz.remove(&key);
            }
        }

        self.topic_msg_counts.clear();
        self.topic_byte_counts.clear();

        self.total_rate.push(self.total_byte_count);
        self.total_hz = self.total_msg_count as f64 / elapsed;
        self.total_msg_count = 0;
        self.total_byte_count = 0;
        self.last_hz_update = Instant::now();
    }

    /// Latest total application bandwidth in bytes/sec (last completed bucket).
    pub(crate) fn total_bytes_per_sec(&self) -> u64 {
        self.total_rate.latest()
    }

    /// Latest bandwidth + history for a specific topic (empty if idle/evicted).
    pub(crate) fn topic_rate_series(&self, key: &str) -> Vec<u64> {
        self.topic_rates
            .get(key)
            .map(|w| w.series())
            .unwrap_or_default()
    }

    pub(crate) fn topic_bytes_per_sec(&self, key: &str) -> u64 {
        self.topic_rates.get(key).map(|w| w.latest()).unwrap_or(0)
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
        if let Some(key) = &self.stream_key_filter {
            return msg.key_expr == *key;
        }

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

    #[allow(dead_code)]
    fn open_selected_topic_in_stream(&mut self) {
        let key = self
            .filtered_topics()
            .get(self.topic_selected)
            .map(|topic| topic.key_expr.clone());
        let Some(key) = key else {
            return;
        };

        self.stream_filter.clear();
        self.stream_key_filter = Some(key.clone());
        self.stream_filtering = false;
        self.follow_stream();
        self.space = Space::Traffic;
        self.set_toast(format!("Stream filtered to exact topic: {}", key));
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

    #[allow(dead_code)]
    fn pin_stream_at(&mut self, idx: usize) {
        self.stream_follow = false;
        self.sub_selected = idx;
        self.clamp_stream_selection();
    }

    /// How the active space splits `body`: two side-by-side panes on wide
    /// terminals, or a single focused pane when narrow (Termux/tablet target).
    pub(crate) fn body_layout(&self, body: Rect) -> BodyLayout {
        if is_narrow(body.width) {
            BodyLayout::Single {
                pane: body,
                focus: self.pane_focus,
            }
        } else {
            let [master, detail] =
                Layout::horizontal([Constraint::Percentage(40), Constraint::Percentage(60)])
                    .areas(body);
            BodyLayout::Split { master, detail }
        }
    }

    pub fn render(&mut self, frame: &mut Frame) {
        // Expire stale toasts each frame.
        let toast_expired = self
            .toast
            .as_ref()
            .map(|(_, t)| t.elapsed().as_secs() >= 2)
            .unwrap_or(true);
        if toast_expired {
            self.toast = None;
        }

        let compact = frame.area().width < TWO_PANE_MIN_WIDTH;
        let header_h = if compact { 1 } else { 2 };
        let [header_area, body_area, hint_area] = Layout::vertical([
            Constraint::Length(header_h),
            Constraint::Fill(1),
            Constraint::Length(1),
        ])
        .areas(frame.area());

        self.render_header(frame, header_area, compact);
        match self.space {
            Space::Traffic => views::traffic::render(self, frame, body_area),
            Space::Network => views::network::render(self, frame, body_area),
        }
        self.render_hint_bar(frame, hint_area);

        if self.overlay == Overlay::Help {
            self.render_help_overlay(frame, body_area);
        }
    }

    fn render_header(&mut self, frame: &mut Frame, area: Rect, compact: bool) {
        // Health dot: a placeholder derived from socket state this phase; real
        // `doctor` wiring lands in phase 5.
        let (dot_color, conn_text) = match &self.connection_state {
            ConnectionState::Connected(zid) => (
                Color::Green,
                format!("Connected zid:{}", &zid[..zid.len().min(16)]),
            ),
            ConnectionState::Connecting => (Color::Yellow, "Connecting...".to_string()),
            ConnectionState::Disconnected(reason) => {
                (Color::Red, format!("Disconnected: {}", reason))
            }
        };

        let [line0, line1] = Layout::vertical([
            Constraint::Length(1),
            Constraint::Length(if compact { 0 } else { 1 }),
        ])
        .areas(area);

        // Line 0: dot + connection string, then the two space tabs (right side).
        let spans = vec![
            Span::styled("● ", Style::default().fg(dot_color)),
            Span::styled(conn_text, Style::default().fg(Color::Gray)),
            Span::raw("  "),
        ];
        let prefix_width: u16 = spans.iter().map(|s| s.content.chars().count() as u16).sum();
        frame.render_widget(Paragraph::new(Line::from(spans)), line0);

        // Space tabs — record each label rect for mouse hit-testing.
        let mut x = line0.x + prefix_width;
        for (i, title) in SPACE_TITLES.iter().enumerate() {
            let label = format!("[{}]", title);
            let label_width = label.chars().count() as u16;
            if x + label_width > line0.x + line0.width {
                self.space_tab_rects[i] = None;
                continue;
            }
            let rect = Rect::new(x, line0.y, label_width, 1);
            self.space_tab_rects[i] = Some(rect);
            let style = if i == self.space.index() {
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD | Modifier::UNDERLINED)
            } else {
                Style::default().fg(Color::White)
            };
            frame.render_widget(Paragraph::new(Span::styled(label, style)), rect);
            x += label_width + 1;
        }

        // Line 1 (skipped when compact): counts summary.
        if !compact {
            let counts = format!(
                "{} sessions · {} services · {} keys · {:.0} msg/s · {}",
                self.nodes.len(),
                self.liveliness_tokens.len(),
                self.topics.len(),
                self.total_hz,
                format_bytes_per_sec(self.total_bytes_per_sec()),
            );
            frame.render_widget(
                Paragraph::new(Span::styled(counts, Style::default().fg(Color::DarkGray))),
                line1,
            );
        }
    }

    fn render_hint_bar(&self, frame: &mut Frame, area: Rect) {
        let hint = match self.space {
            Space::Traffic => {
                "Tab space  / filter  j/k move  Enter open  L live  Q query  y/Y copy  ? help  q quit"
            }
            Space::Network => {
                "Tab space  j/k move  Enter drill  s scout  y copy  ? help  q quit"
            }
        };
        frame.render_widget(
            Paragraph::new(Span::styled(hint, Style::default().fg(Color::DarkGray))),
            area,
        );
    }

    fn render_help_overlay(&self, frame: &mut Frame, content_area: Rect) {
        let width = 60.min(content_area.width.saturating_sub(2));
        let height = 16.min(content_area.height.saturating_sub(2));
        if width < 24 || height < 6 {
            return;
        }
        let x = content_area.x + (content_area.width - width) / 2;
        let y = content_area.y + (content_area.height - height) / 2;
        let popup = Rect::new(x, y, width, height);

        frame.render_widget(Clear, popup);
        let block = Block::default()
            .borders(Borders::ALL)
            .title(" Keybindings ")
            .style(Style::default().fg(Color::White).bg(Color::Black));
        let inner = block.inner(popup);
        frame.render_widget(block, popup);

        let entries: [(&str, &str); 6] = [
            ("Tab / 1 / 2", "switch space"),
            ("j / k", "move"),
            ("Enter / →", "drill into detail"),
            ("Esc / ←", "back to list"),
            ("?", "help"),
            ("q", "quit"),
        ];
        let mut lines: Vec<Line> = entries
            .iter()
            .map(|(keys, desc)| {
                Line::from(vec![
                    Span::styled(
                        format!("  {:<12}", keys),
                        Style::default().fg(Color::Yellow),
                    ),
                    Span::styled(desc.to_string(), Style::default().fg(Color::White)),
                ])
            })
            .collect();
        lines.push(Line::from(""));
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

    #[allow(dead_code)]
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

    #[allow(dead_code)]
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
        assert_eq!(
            detail_scroll_action(key(KeyCode::Char('J'))),
            Some(DetailScroll::Down)
        );
        assert_eq!(
            detail_scroll_action(key(KeyCode::Char('K'))),
            Some(DetailScroll::Up)
        );
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

    #[test]
    fn rate_window_evicts_beyond_cap_and_detects_idle() {
        let mut w = RateWindow::new(3);
        w.push(10);
        w.push(20);
        w.push(30);
        w.push(40); // evicts the 10
        assert_eq!(w.series(), vec![20, 30, 40]);
        assert_eq!(w.latest(), 40);
        assert!(!w.is_all_zero());
        let mut idle = RateWindow::new(2);
        idle.push(0);
        idle.push(0);
        assert!(idle.is_all_zero());
    }

    #[test]
    fn format_bytes_per_sec_scales_units() {
        assert_eq!(format_bytes_per_sec(500), "500 B/s");
        assert_eq!(format_bytes_per_sec(2048), "2.0 KB/s");
        assert_eq!(format_bytes_per_sec(3_145_728), "3.0 MB/s");
    }

    #[test]
    fn narrow_below_threshold() {
        assert!(is_narrow(89));
        assert!(!is_narrow(90));
        assert!(!is_narrow(200));
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
        app.topics.push(TopicInfo {
            key_expr: "a/b".into(),
        });
        app.topic_filter = "zzz".into();
        assert_eq!(app.topics_empty_reason(), EmptyReason::FilteredOut);
    }

    #[test]
    fn question_mark_toggles_help_overlay() {
        let mut app = App::new("test".into());
        assert_eq!(app.overlay, Overlay::None);
        app.handle_key(key(KeyCode::Char('?')));
        assert_eq!(app.overlay, Overlay::Help);
        // q closes the overlay (does not quit)
        app.handle_key(key(KeyCode::Char('q')));
        assert_eq!(app.overlay, Overlay::None);
        assert!(!app.should_quit);
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
        assert_eq!(app.overlay, Overlay::None);
    }

    #[test]
    fn tab_and_number_keys_switch_space() {
        let mut app = App::new("test".into());
        assert_eq!(app.space, Space::Traffic);
        app.handle_key(key(KeyCode::Tab));
        assert_eq!(app.space, Space::Network);
        app.handle_key(key(KeyCode::Char('1')));
        assert_eq!(app.space, Space::Traffic);
        app.handle_key(key(KeyCode::Char('2')));
        assert_eq!(app.space, Space::Network);
    }

    #[test]
    fn enter_and_esc_move_pane_focus() {
        let mut app = App::new("test".into());
        assert_eq!(app.pane_focus, PaneFocus::Master);
        app.handle_key(key(KeyCode::Enter));
        assert_eq!(app.pane_focus, PaneFocus::Detail);
        app.handle_key(key(KeyCode::Esc));
        assert_eq!(app.pane_focus, PaneFocus::Master);
    }

    #[test]
    fn switching_space_resets_pane_focus_to_master() {
        let mut app = App::new("test".into());
        app.pane_focus = PaneFocus::Detail;
        app.handle_key(key(KeyCode::Char('2')));
        assert_eq!(app.space, Space::Network);
        assert_eq!(app.pane_focus, PaneFocus::Master);
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
    fn space_tab_hit_inside_rect_returns_index() {
        let rects = [Some(Rect::new(1, 0, 9, 1)), Some(Rect::new(12, 0, 9, 1))];
        assert_eq!(space_tab_hit(&rects, 2, 0), Some(0));
        assert_eq!(space_tab_hit(&rects, 14, 0), Some(1));
    }

    #[test]
    fn space_tab_hit_outside_returns_none() {
        let rects = [Some(Rect::new(1, 0, 9, 1)), None];
        assert_eq!(space_tab_hit(&rects, 50, 0), None);
        assert_eq!(space_tab_hit(&rects, 2, 5), None);
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
            payload: zenmon_core::types::MessagePayload::from_json(&serde_json::json!(null)),
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
            payload: zenmon_core::types::MessagePayload::from_json(&serde_json::json!(null)),
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
            payload: zenmon_core::types::MessagePayload::from_json(&serde_json::json!({"x": 1})),
            encoding: String::new(),
            payload_bytes: 0,
            timestamp: None,
            kind: "put".into(),
            attachment: None,
            attachment_bytes: None,
        });
        app.handle_zenoh_message(ZenohMessage {
            key_expr: "robot/status".into(),
            payload: zenmon_core::types::MessagePayload::from_json(&serde_json::json!("idle")),
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
            payload: zenmon_core::types::MessagePayload::from_json(&serde_json::json!(null)),
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
            payload: zenmon_core::types::MessagePayload::from_json(&serde_json::json!(null)),
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
        app.admin_nodes.push(zenmon_core::types::NodeInfo {
            zid: "z1".into(),
            kind: "router".into(),
            locators: vec![],
            metadata: None,
            sources: zenmon_core::types::NodeSources::default(),
            admin_last_seen: None,
            scout_last_seen: None,
        });
        app.scout_nodes.push(zenmon_core::types::NodeInfo {
            zid: "z2".into(),
            kind: "peer".into(),
            locators: vec![],
            metadata: None,
            sources: zenmon_core::types::NodeSources::default(),
            admin_last_seen: None,
            scout_last_seen: None,
        });
        app.nodes = zenmon_core::merge::merge_nodes(&app.admin_nodes, &app.scout_nodes);
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
            payload: zenmon_core::types::MessagePayload::from_json(&serde_json::json!(1)),
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

    fn seed_topics(app: &mut App, keys: &[&str]) {
        for k in keys {
            app.topics.push(TopicInfo {
                key_expr: (*k).into(),
            });
        }
        app.topics.sort_by(|a, b| a.key_expr.cmp(&b.key_expr));
    }

    #[test]
    fn traffic_q_runs_query_on_selected_key() {
        let mut app = App::new("test".into());
        app.space = Space::Traffic;
        seed_topics(&mut app, &["demo/a", "demo/b"]);
        app.topic_selected = 1; // demo/b
        app.handle_key(key(KeyCode::Char('Q')));
        assert_eq!(app.detail_mode, DetailMode::Query);
        assert_eq!(app.pending_query, Some("demo/b".to_string()));
        assert_eq!(app.query_history, vec!["demo/b".to_string()]);
    }

    #[test]
    fn traffic_l_sets_live_mode() {
        let mut app = App::new("test".into());
        app.space = Space::Traffic;
        app.detail_mode = DetailMode::Query;
        app.handle_key(key(KeyCode::Char('L')));
        assert_eq!(app.detail_mode, DetailMode::Live);
    }

    #[test]
    fn traffic_jk_move_and_clamp_selection() {
        let mut app = App::new("test".into());
        app.space = Space::Traffic;
        seed_topics(&mut app, &["a", "b", "c"]);
        assert_eq!(app.topic_selected, 0);
        app.handle_key(key(KeyCode::Char('j')));
        assert_eq!(app.topic_selected, 1);
        app.handle_key(key(KeyCode::Char('j')));
        assert_eq!(app.topic_selected, 2);
        // Clamp at the bottom.
        app.handle_key(key(KeyCode::Char('j')));
        assert_eq!(app.topic_selected, 2);
        app.handle_key(key(KeyCode::Char('k')));
        app.handle_key(key(KeyCode::Char('k')));
        assert_eq!(app.topic_selected, 0);
        // Clamp at the top.
        app.handle_key(key(KeyCode::Char('k')));
        assert_eq!(app.topic_selected, 0);
    }

    #[test]
    fn traffic_move_resets_detail_scroll() {
        let mut app = App::new("test".into());
        app.space = Space::Traffic;
        seed_topics(&mut app, &["a", "b"]);
        app.topic_detail_scroll = 9;
        app.handle_key(key(KeyCode::Char('j')));
        assert_eq!(app.topic_selected, 1);
        assert_eq!(app.topic_detail_scroll, 0);
    }

    #[test]
    fn render_traffic_with_data_wide_and_narrow_shows_key() {
        use ratatui::backend::TestBackend;
        use ratatui::Terminal;
        for (w, h) in [(120u16, 30u16), (60u16, 20u16)] {
            let mut app = App::new("tcp/127.0.0.1:7447".into());
            app.space = Space::Traffic;
            app.connection_state = ConnectionState::Connected("zid".into());
            app.topics.push(TopicInfo {
                key_expr: "demo/robot/pose".into(),
            });
            let msg = ZenohMessage {
                key_expr: "demo/robot/pose".into(),
                payload: MessagePayload::from_json(&serde_json::json!({"x": 1})),
                encoding: "application/json".into(),
                payload_bytes: 8,
                timestamp: None,
                kind: "put".into(),
                attachment: None,
                attachment_bytes: None,
            };
            app.topic_latest
                .insert("demo/robot/pose".into(), (msg.clone(), Instant::now()));
            app.sub_messages.push_front(msg);
            app.topic_hz.insert("demo/robot/pose".into(), 12.0);

            let mut terminal = Terminal::new(TestBackend::new(w, h)).unwrap();
            // Wide → master; narrow default focus is Master → master shows the key.
            terminal.draw(|f| app.render(f)).unwrap();
            let text = buffer_text(terminal.backend().buffer());
            assert!(
                text.contains("demo/robot/pose"),
                "key missing at {}x{}: {}",
                w,
                h,
                text
            );
        }
    }

    #[test]
    fn render_traffic_narrow_detail_pane_does_not_panic() {
        use ratatui::backend::TestBackend;
        use ratatui::Terminal;
        let mut app = App::new("tcp/127.0.0.1:7447".into());
        app.space = Space::Traffic;
        app.pane_focus = PaneFocus::Detail;
        app.connection_state = ConnectionState::Connected("zid".into());
        app.topics.push(TopicInfo {
            key_expr: "demo/robot/pose".into(),
        });
        let msg = ZenohMessage {
            key_expr: "demo/robot/pose".into(),
            payload: MessagePayload::from_json(&serde_json::json!({"x": 1})),
            encoding: "application/json".into(),
            payload_bytes: 8,
            timestamp: None,
            kind: "put".into(),
            attachment: None,
            attachment_bytes: None,
        };
        app.topic_latest
            .insert("demo/robot/pose".into(), (msg.clone(), Instant::now()));
        app.sub_messages.push_front(msg);

        let mut terminal = Terminal::new(TestBackend::new(60, 20)).unwrap();
        terminal.draw(|f| app.render(f)).unwrap();
        let text = buffer_text(terminal.backend().buffer());
        assert!(text.contains("demo/robot/pose"), "detail key missing");
    }

    /// Headless render smoke test: drive `render()` through ratatui's in-memory
    /// TestBackend at wide and narrow widths (either side of the two-pane
    /// breakpoint) to prove the layout builds without panicking and the header
    /// tab / space body actually paint. Substitutes for interactive TTY checks.
    fn render_at(width: u16, height: u16, space: Space) -> ratatui::buffer::Buffer {
        use ratatui::backend::TestBackend;
        use ratatui::Terminal;
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        let mut app = App::new("tcp/127.0.0.1:7447".into());
        app.space = space;
        terminal.draw(|f| app.render(f)).unwrap();
        terminal.backend().buffer().clone()
    }

    fn buffer_text(buf: &ratatui::buffer::Buffer) -> String {
        buf.content().iter().map(|c| c.symbol()).collect()
    }

    /// Render `buf` back into a row-by-row text grid (with borders visible), for
    /// eyeballing the real layout in a terminal.
    fn buffer_grid(buf: &ratatui::buffer::Buffer) -> String {
        let area = buf.area();
        let mut out = String::new();
        for y in 0..area.height {
            for x in 0..area.width {
                out.push_str(buf[(area.x + x, area.y + y)].symbol());
            }
            out.push('\n');
        }
        out
    }

    /// A dev harness (not run by default): seed a realistic network and dump the
    /// actual rendered frames to stdout for manual inspection.
    /// Run with: `cargo test -p zenmon-tui dump_frames -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn dump_frames() {
        use ratatui::backend::TestBackend;
        use ratatui::Terminal;

        let seed = |space: Space| {
            let mut app = App::new("tcp/127.0.0.1:7447".into());
            app.space = space;
            app.connection_state = ConnectionState::Connected("a3f19c0011223344".into());
            app.self_zid = Some("a3f19c0011223344".into());
            // Topics + rates.
            for (k, hz) in [
                ("demo/robot/pose", 12.0),
                ("demo/robot/battery", 1.0),
                ("demo/sensor/lidar", 30.0),
                ("sys/health", 0.0),
            ] {
                app.topics.push(TopicInfo { key_expr: k.into() });
                app.topic_hz.insert(k.into(), hz);
            }
            let msg = ZenohMessage {
                key_expr: "demo/robot/pose".into(),
                payload: MessagePayload::from_json(
                    &serde_json::json!({"x": 1.21, "y": 3.40, "theta": 0.11}),
                ),
                encoding: "application/json".into(),
                payload_bytes: 40,
                timestamp: None,
                kind: "put".into(),
                attachment: None,
                attachment_bytes: None,
            };
            app.topic_latest
                .insert("demo/robot/pose".into(), (msg.clone(), Instant::now()));
            app.sub_messages.push_front(msg);
            app.total_hz = 43.0;
            // Nodes + services.
            app.nodes.push(make_node("a3f19c0011223344", "router"));
            app.nodes.push(make_node("b2c4ffee00990011", "peer"));
            app.liveliness_tokens
                .push(make_token("demo/robot/node/action_executor", true));
            app.liveliness_tokens
                .push(make_token("demo/robot/node/topic_recorder", false));
            app.liveliness_tokens
                .push(make_token("sys/node/system_monitor", true));
            app
        };

        for (label, space, w, h) in [
            ("TRAFFIC (wide 110x24)", Space::Traffic, 110u16, 24u16),
            ("NETWORK (wide 110x24)", Space::Network, 110u16, 24u16),
            ("NETWORK (narrow 64x20)", Space::Network, 64u16, 20u16),
        ] {
            let mut app = seed(space);
            let mut terminal = Terminal::new(TestBackend::new(w, h)).unwrap();
            terminal.draw(|f| app.render(f)).unwrap();
            println!("\n===== {label} =====");
            print!("{}", buffer_grid(terminal.backend().buffer()));
        }
    }

    #[test]
    fn render_wide_paints_two_pane_traffic() {
        let buf = render_at(120, 30, Space::Traffic);
        let text = buffer_text(&buf);
        assert!(text.contains("Traffic"), "space tab / master title missing");
        assert!(
            text.contains("Detail"),
            "detail pane missing in wide layout"
        );
    }

    #[test]
    fn render_narrow_single_pane_does_not_panic() {
        // Below TWO_PANE_MIN_WIDTH: single pane, compact 1-line header.
        let buf = render_at(60, 20, Space::Network);
        let text = buffer_text(&buf);
        assert!(text.contains("Network"), "network master title missing");
    }

    fn make_node(zid: &str, kind: &str) -> NodeInfo {
        NodeInfo {
            zid: zid.into(),
            kind: kind.into(),
            locators: vec!["tcp/127.0.0.1:7447".into()],
            metadata: None,
            sources: zenmon_core::types::NodeSources::ADMIN,
            admin_last_seen: Some(SystemTime::now()),
            scout_last_seen: None,
        }
    }

    fn make_token(key_expr: &str, alive: bool) -> LivelinessToken {
        LivelinessToken {
            key_expr: key_expr.into(),
            source_zid: None,
            alive,
        }
    }

    #[test]
    fn network_jk_move_and_clamp_selection() {
        let mut app = App::new("test".into());
        app.space = Space::Network;
        app.nodes.push(make_node("z1", "router"));
        app.nodes.push(make_node("z2", "peer"));
        app.liveliness_tokens
            .push(make_token("fleet/r1/node/a", true));
        // 2 sessions + 1 service = 3 selectable rows.
        assert_eq!(app.network_rows().len(), 3);
        assert_eq!(app.network_selected, 0);
        app.handle_key(key(KeyCode::Char('j')));
        assert_eq!(app.network_selected, 1);
        app.handle_key(key(KeyCode::Char('j')));
        assert_eq!(app.network_selected, 2);
        // Clamp at the bottom.
        app.handle_key(key(KeyCode::Char('j')));
        assert_eq!(app.network_selected, 2);
        app.handle_key(key(KeyCode::Char('k')));
        app.handle_key(key(KeyCode::Char('k')));
        assert_eq!(app.network_selected, 0);
        // Clamp at the top.
        app.handle_key(key(KeyCode::Char('k')));
        assert_eq!(app.network_selected, 0);
    }

    #[test]
    fn network_move_resets_detail_scroll() {
        let mut app = App::new("test".into());
        app.space = Space::Network;
        app.nodes.push(make_node("z1", "router"));
        app.nodes.push(make_node("z2", "peer"));
        app.node_detail_scroll = 9;
        app.handle_key(key(KeyCode::Char('j')));
        assert_eq!(app.network_selected, 1);
        assert_eq!(app.node_detail_scroll, 0);
    }

    #[test]
    fn network_selection_resolves_session_then_service() {
        let mut app = App::new("test".into());
        app.nodes.push(make_node("z1", "router"));
        app.nodes.push(make_node("z2", "peer"));
        app.liveliness_tokens
            .push(make_token("fleet/r1/node/a", true));
        assert_eq!(app.selected_network_row(), Some(NetworkRow::Session(0)));
        app.network_selected = 1;
        assert_eq!(app.selected_network_row(), Some(NetworkRow::Session(1)));
        app.network_selected = 2;
        assert_eq!(app.selected_network_row(), Some(NetworkRow::Service(0)));
    }

    #[test]
    fn network_s_requests_scout_refresh() {
        let mut app = App::new("test".into());
        app.space = Space::Network;
        assert!(!app.pending_scout_request);
        app.handle_key(key(KeyCode::Char('s')));
        assert!(app.pending_scout_request);
    }

    #[test]
    fn network_s_does_not_request_while_scout_in_progress() {
        let mut app = App::new("test".into());
        app.space = Space::Network;
        app.scout_in_progress = true;
        app.handle_key(key(KeyCode::Char('s')));
        assert!(!app.pending_scout_request);
    }

    #[test]
    fn render_network_with_data_wide_and_narrow_shows_participant() {
        use ratatui::backend::TestBackend;
        use ratatui::Terminal;
        for (w, h) in [(120u16, 30u16), (60u16, 20u16)] {
            let mut app = App::new("tcp/127.0.0.1:7447".into());
            app.space = Space::Network;
            app.connection_state = ConnectionState::Connected("zidrouter0000000".into());
            app.nodes.push(make_node("zidrouter0000000", "router"));
            app.liveliness_tokens
                .push(make_token("fleet/r1/node/action_executor_ec98a701", true));

            let mut terminal = Terminal::new(TestBackend::new(w, h)).unwrap();
            terminal.draw(|f| app.render(f)).unwrap();
            let text = buffer_text(terminal.backend().buffer());
            // Master (wide → both panes; narrow default focus is Master) shows the
            // session zid and the service name.
            assert!(
                text.contains("zidrouter"),
                "session zid missing at {}x{}: {}",
                w,
                h,
                text
            );
            assert!(
                text.contains("action_executor"),
                "service name missing at {}x{}: {}",
                w,
                h,
                text
            );
        }
    }

    #[test]
    fn render_help_overlay_paints() {
        use ratatui::backend::TestBackend;
        use ratatui::Terminal;
        let mut terminal = Terminal::new(TestBackend::new(120, 30)).unwrap();
        let mut app = App::new("tcp/127.0.0.1:7447".into());
        app.overlay = Overlay::Help;
        terminal.draw(|f| app.render(f)).unwrap();
        // Overlay drew on top without panicking; buffer is non-empty.
        assert!(!buffer_text(terminal.backend().buffer()).trim().is_empty());
    }
}
