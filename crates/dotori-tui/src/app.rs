use crate::event::AppEvent;
use crate::views;
use crossterm::event::{KeyCode, KeyEvent};
use dotori_core::types::{NodeInfo, TopicInfo, ZenohMessage};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Tabs};
use ratatui::Frame;
use std::collections::{HashMap, VecDeque};
use std::time::Instant;

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

const TAB_TITLES: [&str; 5] = ["Dashboard", "Topics", "Subscribe", "Query", "Nodes"];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActiveView {
    Dashboard,
    Topics,
    Subscribe,
    Query,
    Nodes,
}

impl ActiveView {
    pub fn index(&self) -> usize {
        match self {
            ActiveView::Dashboard => 0,
            ActiveView::Topics => 1,
            ActiveView::Subscribe => 2,
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

    pub topics: Vec<TopicInfo>,
    pub topic_latest: HashMap<String, (ZenohMessage, Instant)>,
    pub nodes: Vec<NodeInfo>,
    pub recent_messages: VecDeque<ZenohMessage>,

    pub sub_messages: VecDeque<ZenohMessage>,
    pub sub_paused: bool,
    pub sub_scroll: u16,

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

    pub node_selected: usize,
}

impl App {
    pub fn new(endpoint: String) -> Self {
        Self {
            active_view: ActiveView::Dashboard,
            should_quit: false,
            connection_state: ConnectionState::Connecting,
            endpoint,
            topics: Vec::new(),
            topic_latest: HashMap::new(),
            nodes: Vec::new(),
            recent_messages: VecDeque::with_capacity(100),
            sub_messages: VecDeque::with_capacity(500),
            sub_paused: false,
            sub_scroll: 0,
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
            node_selected: 0,
        }
    }

    pub fn is_connected(&self) -> bool {
        matches!(self.connection_state, ConnectionState::Connected(_))
    }

    pub fn handle_event(&mut self, event: AppEvent) {
        match event {
            AppEvent::Key(key) => self.handle_key(key),
            AppEvent::Mouse(_) => {} // wired in Task 5
            AppEvent::Zenoh(msg) => self.handle_zenoh_message(msg),
            AppEvent::Tick => {
                self.update_hz();
            }
        }
    }

    fn handle_key(&mut self, key: KeyEvent) {
        if !self.is_text_input_active() {
            match key.code {
                KeyCode::Char('q') => {
                    self.should_quit = true;
                    return;
                }
                KeyCode::Char('1') => self.active_view = ActiveView::Dashboard,
                KeyCode::Char('2') => self.active_view = ActiveView::Topics,
                KeyCode::Char('3') => self.active_view = ActiveView::Subscribe,
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
        self.topics_filtering || self.query_editing
    }

    fn handle_text_input_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc => {
                self.topics_filtering = false;
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
            }
            KeyCode::Char(c) => {
                if self.topics_filtering {
                    self.topic_filter.push(c);
                } else if self.query_editing {
                    self.query_input.push(c);
                }
            }
            KeyCode::Backspace => {
                if self.topics_filtering {
                    self.topic_filter.pop();
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
                // Shift+J/K or Ctrl+D/U: scroll detail panel
                (m, KeyCode::Char('J')) if m.contains(crossterm::event::KeyModifiers::SHIFT) => {
                    self.topic_detail_scroll = self.topic_detail_scroll.saturating_add(3);
                }
                (m, KeyCode::Char('K')) if m.contains(crossterm::event::KeyModifiers::SHIFT) => {
                    self.topic_detail_scroll = self.topic_detail_scroll.saturating_sub(3);
                }
                // j/k: navigate topic list (reset detail scroll)
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
                    self.active_view = ActiveView::Subscribe;
                }
                _ => {}
            },
            ActiveView::Subscribe => match key.code {
                KeyCode::Char(' ') => self.sub_paused = !self.sub_paused,
                KeyCode::Up | KeyCode::Char('k') => {
                    self.sub_scroll = self.sub_scroll.saturating_add(1);
                }
                KeyCode::Down | KeyCode::Char('j') => {
                    self.sub_scroll = self.sub_scroll.saturating_sub(1);
                }
                _ => {}
            },
            ActiveView::Query => match key.code {
                KeyCode::Char('/') | KeyCode::Char('i') => self.query_editing = true,
                KeyCode::Up | KeyCode::Char('k') => {
                    if let Some(prev) = self.query_history.last() {
                        self.query_input = prev.clone();
                    }
                }
                _ => {}
            },
            ActiveView::Nodes => match key.code {
                KeyCode::Up | KeyCode::Char('k') => {
                    self.node_selected = self.node_selected.saturating_sub(1);
                }
                KeyCode::Down | KeyCode::Char('j') => {
                    let max = self.nodes.len().saturating_sub(1);
                    if self.node_selected < max {
                        self.node_selected += 1;
                    }
                }
                _ => {}
            },
            _ => {}
        }
    }

    fn handle_zenoh_message(&mut self, msg: ZenohMessage) {
        // Auto-collect topic from received message
        if !self.topics.iter().any(|t| t.key_expr == msg.key_expr) {
            self.topics.push(TopicInfo {
                key_expr: msg.key_expr.clone(),
            });
            self.topics.sort_by(|a, b| a.key_expr.cmp(&b.key_expr));
        }

        // Store latest value per topic
        self.topic_latest
            .insert(msg.key_expr.clone(), (msg.clone(), Instant::now()));

        // Count messages for Hz calculation
        *self.topic_msg_counts.entry(msg.key_expr.clone()).or_insert(0) += 1;
        self.total_msg_count += 1;

        self.recent_messages.push_front(msg.clone());
        if self.recent_messages.len() > 100 {
            self.recent_messages.pop_back();
        }

        if !self.sub_paused {
            self.sub_messages.push_front(msg);
            if self.sub_messages.len() > 500 {
                self.sub_messages.pop_back();
            }
        }
    }

    /// Recalculate Hz rates. Call this periodically (e.g. every tick).
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

    pub fn render(&mut self, frame: &mut Frame) {
        let [tabs_area, content_area, status_area] = Layout::vertical([
            Constraint::Length(3),
            Constraint::Fill(1),
            Constraint::Length(1),
        ])
        .areas(frame.area());

        let tabs = Tabs::new(
            TAB_TITLES
                .iter()
                .enumerate()
                .map(|(i, t)| format!("[{}] {}", i + 1, t)),
        )
        .block(Block::default().borders(Borders::ALL).title(" dotori "))
        .select(self.active_view.index())
        .style(Style::default().fg(Color::White))
        .highlight_style(
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD | Modifier::UNDERLINED),
        )
        .divider("  ");
        frame.render_widget(tabs, tabs_area);

        match self.active_view {
            ActiveView::Dashboard => views::dashboard::render(self, frame, content_area),
            ActiveView::Topics => views::topics::render(self, frame, content_area),
            ActiveView::Subscribe => views::subscribe::render(self, frame, content_area),
            ActiveView::Query => views::query::render(self, frame, content_area),
            ActiveView::Nodes => views::nodes::render(self, frame, content_area),
        }

        // Status bar with connection state
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

        let mode_text = if self.is_text_input_active() {
            " INPUT "
        } else {
            " NORMAL "
        };

        let status = Line::from(vec![
            Span::styled(conn_text, conn_style),
            Span::styled(
                format!(" {} ", self.endpoint),
                Style::default().fg(Color::Gray),
            ),
            Span::styled(mode_text, Style::default().fg(Color::Cyan)),
            Span::styled(
                " q:quit  1-5:view  /:filter ",
                Style::default().fg(Color::DarkGray),
            ),
        ]);
        frame.render_widget(status, status_area);
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
        let rects = [
            Some(Rect::new(1, 0, 14, 3)),
            None,
            None,
            None,
            None,
        ];
        assert_eq!(tab_hit(&rects, 50, 1), None);
        assert_eq!(tab_hit(&rects, 2, 5), None);
    }

    #[test]
    fn list_hit_converts_row_to_index() {
        // list_rect at (0,5) size 20x10 → rows 5..15
        // first_item_row = 6 (border at row 5)
        // total_items = 8
        let rect = Rect::new(0, 5, 20, 10);
        assert_eq!(list_hit(rect, 6, 0, 8, 6), Some(0));
        assert_eq!(list_hit(rect, 8, 0, 8, 6), Some(2));
        assert_eq!(list_hit(rect, 5, 0, 8, 6), None, "border row");
        assert_eq!(list_hit(rect, 15, 0, 8, 6), None, "at rect.bottom(), exclusive");
        assert_eq!(list_hit(rect, 20, 0, 8, 6), None, "past rect");
        assert_eq!(list_hit(rect, 14, 0, 8, 6), None, "row 14 → index 8, past total");
    }

    #[test]
    fn list_hit_respects_scroll_offset() {
        let rect = Rect::new(0, 5, 20, 10);
        // scroll_offset 4, total 20, first_item_row 6
        assert_eq!(list_hit(rect, 6, 4, 20, 6), Some(4));
        assert_eq!(list_hit(rect, 9, 4, 20, 6), Some(7));
    }
}
