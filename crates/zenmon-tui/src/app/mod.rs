//! Application state and the pieces that read it directly.
//!
//! The behaviour is split across sibling modules rather than one file: `keys`
//! (keyboard dispatch), `mouse` (pointer), `ingest` (inbound data and rates),
//! and `chrome` (header, hint bar, overlays). They are separate `impl App`
//! blocks over the state defined here, so moving a method between them is a
//! cut-and-paste with no signature change.

mod chrome;
mod ingest;
mod keys;
mod mouse;

#[cfg(test)]
mod tests;

// `RateWindow` is defined next to the rate accounting that fills it, but the
// `App` fields that hold it are declared here.
use ingest::{RateWindow, RATE_WINDOW_SECS};

use crate::history::History;
use crate::tree::{FlattenOpts, KeyTree, RowKind, TreeRow};
use ratatui::layout::{Constraint, Layout, Rect};
use std::collections::{HashMap, HashSet, VecDeque};
use std::time::{Instant, SystemTime};
use zenmon_core::config::ConnectMode;
use zenmon_core::doctor::DoctorReport;
use zenmon_core::types::{
    LivelinessToken, MessagePayload, NodeInfo, PortScoutResult, ZenohMessage,
};

/// How many children an expanded branch may list before it folds to a summary.
pub(crate) const DEFAULT_FOLD_THRESHOLD: usize = 12;

/// A key's row dims after this long without a message.
pub(crate) const IDLE_DIM_SECS: u64 = 300;

/// Above this many keys the longest-idle ones are dropped.
///
/// This is a memory bound, not a staleness policy. A key that stopped
/// publishing is itself a finding — often *the* finding — so going quiet never
/// gets a key evicted on its own; it only decides who goes first when the
/// session has accumulated more keys than it can hold.
pub(crate) const MAX_KEYS: usize = 10_000;

/// Evict down to this fraction of [`MAX_KEYS`], so the sort behind an eviction
/// runs occasionally rather than once per key past the cap.
pub(crate) const EVICT_TO_FRACTION: f64 = 0.9;

/// How a key's row should read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum KeyFreshness {
    /// Publishing recently.
    Live,
    /// Seen this session, silent for at least [`IDLE_DIM_SECS`].
    Idle,
    /// In the tree but no payload recorded — only reachable transiently.
    NoData,
}

/// Coarse "how long ago" for an idle key: minutes, then hours.
///
/// Deliberately blunt. The number answers "did this stop just now or hours
/// ago", and a second-accurate figure next to a dimmed row would invite
/// reading precision into something the 1 Hz sampling cannot support.
pub(crate) fn format_idle(secs: u64) -> String {
    if secs < 3600 {
        format!("{}m", secs / 60)
    } else {
        format!("{}h{}m", secs / 3600, (secs % 3600) / 60)
    }
}

/// The flattened rows, kept until something they depend on changes.
///
/// Flattening is cheap per node but runs over every visible row, and the frame
/// gate only helps if the work behind a frame is bounded. A steady stream on
/// known keys changes none of these three, so it costs nothing.
#[derive(Debug, Default, Clone)]
struct TreeViewCache {
    rows: Vec<TreeRow>,
    tree_version: u64,
    expanded_version: u64,
    filter: String,
    /// Distinguishes "cached an empty tree" from "never built".
    built: bool,
}

fn payload_to_string(p: &MessagePayload) -> String {
    p.pretty()
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

/// A modal overlay drawn on top of the active space. `Palette` opens the `:`
/// command list; `ScoutPort` is the numeric scout-port/domain switcher reached
/// through the palette.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Overlay {
    None,
    Help,
    Doctor,
    Palette,
    ScoutPort,
}

/// An effect the command palette can trigger. Each entry in [`palette_commands`]
/// maps a human label to one of these; [`App::run_palette_action`] performs it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PaletteAction {
    SwitchSpace(Space),
    RunDoctor,
    ScoutRefresh,
    SetMode(ConnectMode),
    OpenScoutPort,
    OpenHelp,
    Quit,
}

/// One row in the command palette / help overlay: a `label`, a short `key_hint`
/// (the direct keybinding, if any), and the [`PaletteAction`] it runs.
pub struct PaletteCommand {
    pub label: &'static str,
    pub key_hint: &'static str,
    pub action: PaletteAction,
}

/// The static command table. The palette and help overlay both render from this
/// list so the two can never drift out of sync.
pub fn palette_commands() -> &'static [PaletteCommand] {
    &[
        PaletteCommand {
            label: "Go to Traffic",
            key_hint: "1",
            action: PaletteAction::SwitchSpace(Space::Traffic),
        },
        PaletteCommand {
            label: "Go to Network",
            key_hint: "2",
            action: PaletteAction::SwitchSpace(Space::Network),
        },
        PaletteCommand {
            label: "Run doctor",
            key_hint: "d",
            action: PaletteAction::RunDoctor,
        },
        PaletteCommand {
            label: "Refresh nodes (scout)",
            key_hint: "s",
            action: PaletteAction::ScoutRefresh,
        },
        PaletteCommand {
            label: "Switch to peer mode",
            key_hint: "",
            action: PaletteAction::SetMode(ConnectMode::Peer),
        },
        PaletteCommand {
            label: "Switch to client mode",
            key_hint: "",
            action: PaletteAction::SetMode(ConnectMode::Client),
        },
        PaletteCommand {
            label: "Set scout domain / port…",
            key_hint: "",
            action: PaletteAction::OpenScoutPort,
        },
        PaletteCommand {
            label: "Help",
            key_hint: "?",
            action: PaletteAction::OpenHelp,
        },
        PaletteCommand {
            label: "Quit",
            key_hint: "q",
            action: PaletteAction::Quit,
        },
    ]
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

    /// The key hierarchy shown in the Traffic master pane.
    pub key_tree: KeyTree,
    /// Branches the user has opened.
    pub tree_expanded: HashSet<String>,
    /// Over-threshold branches the user has asked to list in full.
    pub tree_unfolded: HashSet<String>,
    /// Bumped on every change to `tree_expanded`/`tree_unfolded` so the row
    /// cache can distinguish "same tree, different expansion" from no change.
    pub tree_expanded_version: u64,
    /// Set once the user expands or collapses anything, which switches off the
    /// automatic opening in [`App::auto_expand`] — after that the tree is
    /// theirs and new keys must not reopen what they closed.
    pub tree_user_touched: bool,
    /// Children above which an expanded branch shows a summary row instead.
    pub fold_threshold: usize,
    /// Keys dropped to stay under [`MAX_KEYS`]. Surfaced in the header, because
    /// a monitoring tool that silently forgets what it saw is worse than one
    /// that admits it.
    pub keys_aged_out: usize,
    tree_cache: TreeViewCache,

    pub topic_latest: HashMap<String, (ZenohMessage, Instant)>,
    /// Per-key history, replacing the global ring the detail pane used to
    /// filter. See [`crate::history`] for why the global ring could not work.
    pub history: History,
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
    /// Cursor over the visible rows of [`App::tree_rows`].
    pub tree_selected: usize,
    pub topics_filtering: bool,
    /// Highlight what changed since the previous message on the selected key.
    /// On by default: a repeating status blob is unreadable without it, and the
    /// cost when nothing changed is that everything renders dim.
    pub diff_enabled: bool,
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

    pub scout_port_input: String,
    pub scout_port_current: Option<u16>,
    pub current_mode: ConnectMode,
    pub pending_reconnect_mode: Option<ConnectMode>,

    /// Command-palette filter text and selected filtered-row index.
    pub palette_input: String,
    pub palette_selected: usize,
    pub port_scan_results: Vec<PortScoutResult>,
    pub port_scan_selected: usize,
    pub port_scan_in_progress: bool,
    pub pending_port_scan_request: bool,
    pub pending_reconnect_port: Option<u16>,

    pub list_rect: Option<ratatui::layout::Rect>,
    pub list_first_item_row: u16,
    pub list_scroll_offset: usize,

    /// One entry per rendered Network-master display row: `Some(selectable_index)`
    /// (an index into [`App::network_rows`] / the `network_selected` space) for a
    /// Session/Service row, or `None` for a non-selectable section/group header.
    /// Rebuilt every Network render so clicks map back to the right participant.
    pub network_click_map: Vec<Option<usize>>,

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

    /// Latest one-shot diagnostics report (drives the header health dot and the
    /// Doctor overlay). `None` until the first `doctor` run completes.
    pub doctor_report: Option<DoctorReport>,
    /// A doctor run is in flight (set on `DoctorStarted`, cleared on report).
    pub doctor_running: bool,
    /// Scroll offset for the Doctor overlay check list.
    pub doctor_scroll: u16,
    /// Set to request a doctor run; consumed by the run-loop in `lib.rs`.
    pub pending_doctor_request: bool,
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
            key_tree: KeyTree::new(),
            history: History::default(),
            tree_expanded: HashSet::new(),
            tree_unfolded: HashSet::new(),
            tree_expanded_version: 0,
            tree_user_touched: false,
            fold_threshold: DEFAULT_FOLD_THRESHOLD,
            keys_aged_out: 0,
            tree_cache: TreeViewCache::default(),
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
            tree_selected: 0,
            topics_filtering: false,
            diff_enabled: true,
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
            scout_port_input: String::new(),
            scout_port_current: None,
            current_mode: ConnectMode::Client,
            pending_reconnect_mode: None,
            palette_input: String::new(),
            palette_selected: 0,
            port_scan_results: Vec::new(),
            port_scan_selected: 0,
            port_scan_in_progress: false,
            pending_port_scan_request: false,
            pending_reconnect_port: None,
            list_rect: None,
            list_first_item_row: 0,
            list_scroll_offset: 0,
            network_click_map: Vec::new(),
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
            doctor_report: None,
            doctor_running: false,
            doctor_scroll: 0,
            pending_doctor_request: false,
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
        self.key_tree = KeyTree::new();
        self.tree_expanded.clear();
        self.tree_unfolded.clear();
        self.tree_expanded_version = self.tree_expanded_version.wrapping_add(1);
        // A reconnect rebuilds the namespace from scratch, so the automatic
        // opening should get another go at it.
        self.tree_user_touched = false;
        self.topic_latest.clear();
        self.history.clear();
        self.topic_msg_counts.clear();
        self.topic_hz.clear();
        self.total_msg_count = 0;
        self.total_hz = 0.0;
        self.tree_selected = 0;
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

    /// Connection-level empty reason (connecting/disconnected), or `None` when
    /// the session is up and the emptiness is view-specific.
    fn connection_empty_reason(&self) -> Option<EmptyReason> {
        match &self.connection_state {
            ConnectionState::Connecting => Some(EmptyReason::Connecting),
            ConnectionState::Disconnected(_) => Some(EmptyReason::Disconnected),
            ConnectionState::Connected(_) => None,
        }
    }

    /// Why the Topics list is empty (only meaningful when it is).
    pub(crate) fn topics_empty_reason(&self) -> EmptyReason {
        if let Some(r) = self.connection_empty_reason() {
            return r;
        }
        if !self.topic_filter.is_empty() && !self.key_tree.is_empty() {
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

    /// Rebuild the flattened rows if the tree, the expansion state or the filter
    /// changed. Cheap and idempotent, so callers that need current rows can just
    /// call it rather than reason about who rendered last.
    pub(crate) fn refresh_tree_rows(&mut self) {
        let tree_version = self.key_tree.version();
        let expanded_version = self.tree_expanded_version;
        if self.tree_cache.built
            && self.tree_cache.tree_version == tree_version
            && self.tree_cache.expanded_version == expanded_version
            && self.tree_cache.filter == self.topic_filter
        {
            return;
        }
        let rows = self.key_tree.flatten(&FlattenOpts {
            expanded: &self.tree_expanded,
            unfolded: &self.tree_unfolded,
            filter: &self.topic_filter,
            fold_threshold: self.fold_threshold,
        });
        self.tree_cache = TreeViewCache {
            rows,
            tree_version,
            expanded_version,
            filter: self.topic_filter.clone(),
            built: true,
        };
        self.clamp_tree_selection();
    }

    /// The visible rows as of the last [`App::refresh_tree_rows`].
    pub(crate) fn tree_rows(&self) -> &[TreeRow] {
        &self.tree_cache.rows
    }

    pub(crate) fn selected_row(&self) -> Option<&TreeRow> {
        self.tree_cache.rows.get(self.tree_selected)
    }

    fn clamp_tree_selection(&mut self) {
        let len = self.tree_cache.rows.len();
        self.tree_selected = if len == 0 {
            0
        } else {
            self.tree_selected.min(len - 1)
        };
    }

    /// Note that the expansion state changed, so the row cache rebuilds and the
    /// automatic opening stands down.
    pub(crate) fn mark_tree_touched(&mut self) {
        self.tree_expanded_version = self.tree_expanded_version.wrapping_add(1);
        self.tree_user_touched = true;
    }

    /// Open the tree far enough to be useful on a namespace the user has not
    /// touched yet.
    ///
    /// A tree that opens fully collapsed shows one row and looks broken; one
    /// that opens fully expanded is the flat list this replaced. So: a small
    /// namespace opens completely, and a large one opens through whatever
    /// single-child stem it shares. Stops for good at the first manual
    /// expand or collapse — new keys must never reopen what the user closed.
    pub(crate) fn auto_expand(&mut self) {
        if self.tree_user_touched {
            return;
        }
        let paths = if self.key_tree.len() <= self.fold_threshold {
            self.key_tree.branch_paths()
        } else {
            self.key_tree.single_child_chain()
        };
        let added = paths
            .into_iter()
            .filter(|p| self.tree_expanded.insert(p.clone()))
            .count();
        if added > 0 {
            // Not `mark_tree_touched`: this is not the user's doing.
            self.tree_expanded_version = self.tree_expanded_version.wrapping_add(1);
        }
    }

    /// Whether a key is publishing, has gone quiet, or has no payload yet.
    pub(crate) fn key_freshness(&self, key: &str) -> KeyFreshness {
        match self.topic_latest.get(key) {
            None => KeyFreshness::NoData,
            Some((_, at)) if at.elapsed().as_secs() >= IDLE_DIM_SECS => KeyFreshness::Idle,
            Some(_) => KeyFreshness::Live,
        }
    }

    /// Seconds since the last message on `key`, if one was ever recorded.
    pub(crate) fn idle_secs(&self, key: &str) -> Option<u64> {
        self.topic_latest
            .get(key)
            .map(|(_, at)| at.elapsed().as_secs())
    }

    /// Aggregate rate and bandwidth for every key at or below `path`.
    ///
    /// Scans `topic_hz`, so it is O(keys) and is only ever called for the one
    /// selected branch — never per visible row.
    pub(crate) fn subtree_totals(&self, path: &str) -> (f64, u64) {
        let mut hz = 0.0;
        let mut bytes = 0;
        for (key, rate) in &self.topic_hz {
            if key == path || key.starts_with(&format!("{path}/")) {
                hz += rate;
                bytes += self.topic_bytes_per_sec(key);
            }
        }
        (hz, bytes)
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
}
