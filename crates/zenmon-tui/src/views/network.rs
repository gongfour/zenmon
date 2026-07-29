//! Network space — master/detail over the unified participant list.
//!
//! Left (master): one scrollable list with two labeled sections — **Sessions**
//! (transport nodes from `app.nodes`) and **Services** (liveliness tokens from
//! `app.liveliness_tokens`, grouped by a generic `/`-split prefix). Right
//! (detail): the selected participant — a session's kind/locators/links or a
//! service's key/group/status plus its join/leave history.

use crate::app::{App, BodyLayout, NetworkRow, PaneFocus};
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, ListState, Paragraph, Wrap};
use ratatui::Frame;
use std::collections::HashMap;
use std::time::{Duration, Instant, SystemTime};
use zenmon_core::topology::build_topology;
use zenmon_core::types::{LivelinessToken, NodeInfo};

/// A scout-only node older than this reads as stale (`○`) rather than live.
const STALE_THRESHOLD: Duration = Duration::from_secs(30);

pub fn render(app: &mut App, frame: &mut Frame, area: Rect) {
    match app.body_layout(area) {
        BodyLayout::Split { master, detail } => {
            render_master(app, frame, master);
            render_detail(app, frame, detail);
        }
        BodyLayout::Single {
            pane,
            focus: PaneFocus::Master,
        } => render_master(app, frame, pane),
        BodyLayout::Single {
            pane,
            focus: PaneFocus::Detail,
        } => render_detail(app, frame, pane),
    }
}

fn kind_color(kind: &str) -> Color {
    match kind {
        "router" => Color::Green,
        "peer" => Color::Blue,
        "client" => Color::Gray,
        _ => Color::White,
    }
}

fn zid_short(zid: &str) -> &str {
    &zid[..zid.len().min(16)]
}

fn header_item(label: String) -> ListItem<'static> {
    ListItem::new(Line::from(Span::styled(
        label,
        Style::default()
            .fg(Color::DarkGray)
            .add_modifier(Modifier::DIM),
    )))
}

fn render_master(app: &mut App, frame: &mut Frame, area: Rect) {
    let block = Block::default()
        .borders(Borders::ALL)
        .title(" Network — participants ");

    // Both sections empty → a single contextual empty state, no list chrome.
    if app.nodes.is_empty() && app.liveliness_tokens.is_empty() {
        let inner = block.inner(area);
        frame.render_widget(block, area);
        super::render_empty_state(frame, inner, app.nodes_empty_reason());
        app.list_rect = Some(area);
        app.list_first_item_row = area.y + 1;
        app.list_scroll_offset = 0;
        app.network_click_map = Vec::new();
        return;
    }

    let selected_row = app.selected_network_row();
    let rows = app.network_rows();
    let now = SystemTime::now();
    let self_zid = app.self_zid.clone();

    // Per-group alive/total for the Service sub-headers.
    let mut group_stats: HashMap<String, (usize, usize)> = HashMap::new();
    for token in &app.liveliness_tokens {
        let group = token
            .group_prefix()
            .unwrap_or_else(|| "(ungrouped)".to_string());
        let entry = group_stats.entry(group).or_insert((0, 0));
        entry.1 += 1;
        if token.alive {
            entry.0 += 1;
        }
    }
    let total_tokens = app.liveliness_tokens.len();
    let total_alive = app.liveliness_tokens.iter().filter(|t| t.alive).count();
    let n_sessions = app.nodes.len();

    let mut items: Vec<ListItem> = Vec::new();
    // One entry per display row (parallel to `items`): `Some(selectable_index)`
    // for a Session/Service row, `None` for a header. `selectable_index` is the
    // row's position in `network_rows()` order (== the `network_selected` value).
    let mut click_map: Vec<Option<usize>> = Vec::new();
    let mut selected_display: Option<usize> = None;
    let mut last_section: Option<u8> = None;
    let mut last_group: Option<String> = None;

    for (sel_idx, row) in rows.iter().enumerate() {
        match *row {
            NetworkRow::Session(i) => {
                if last_section != Some(0) {
                    items.push(header_item(format!("── Sessions ({}) ──", n_sessions)));
                    click_map.push(None);
                    last_section = Some(0);
                }
                if selected_row == Some(NetworkRow::Session(i)) {
                    selected_display = Some(items.len());
                }
                items.push(node_item(&app.nodes[i], now, self_zid.as_deref()));
                click_map.push(Some(sel_idx));
            }
            NetworkRow::Service(i) => {
                if last_section != Some(1) {
                    items.push(header_item(format!(
                        "── Services ({}/{}) ──",
                        total_alive, total_tokens
                    )));
                    click_map.push(None);
                    last_section = Some(1);
                    last_group = None;
                }
                let token = &app.liveliness_tokens[i];
                let group = token
                    .group_prefix()
                    .unwrap_or_else(|| "(ungrouped)".to_string());
                if last_group.as_deref() != Some(group.as_str()) {
                    let (alive, total) = group_stats.get(&group).copied().unwrap_or((0, 0));
                    items.push(header_item(format!(
                        "── {} ({}/{}) ──",
                        group, alive, total
                    )));
                    click_map.push(None);
                    last_group = Some(group);
                }
                if selected_row == Some(NetworkRow::Service(i)) {
                    selected_display = Some(items.len());
                }
                items.push(token_item(token));
                click_map.push(Some(sel_idx));
            }
        }
    }

    debug_assert_eq!(click_map.len(), items.len());
    app.network_click_map = click_map;

    let list = List::new(items).block(block).highlight_style(
        Style::default()
            .fg(Color::Black)
            .bg(Color::Cyan)
            .add_modifier(Modifier::BOLD),
    );

    // Record list geometry for phase-6 mouse hit-testing.
    app.list_rect = Some(area);
    app.list_first_item_row = area.y + 1;
    let visible = area.height.saturating_sub(2) as usize;
    let sel = selected_display.unwrap_or(0);
    app.list_scroll_offset = if visible > 0 && sel >= visible {
        sel + 1 - visible
    } else {
        0
    };

    let mut state = ListState::default();
    state.select(selected_display);
    frame.render_stateful_widget(list, area, &mut state);
}

fn node_item(node: &NodeInfo, now: SystemTime, self_zid: Option<&str>) -> ListItem<'static> {
    let stale = node.is_scout_stale(now, STALE_THRESHOLD);
    let (dot, dot_color) = if stale {
        ("○", Color::DarkGray)
    } else {
        ("●", Color::Green)
    };
    let is_self = self_zid.is_some_and(|z| z == node.zid);

    let mut spans = vec![
        Span::styled(format!("{} ", dot), Style::default().fg(dot_color)),
        Span::styled(
            format!("{} ", node.kind),
            Style::default().fg(kind_color(&node.kind)),
        ),
        Span::styled(
            zid_short(&node.zid).to_string(),
            Style::default().fg(Color::Yellow),
        ),
    ];
    if let Some(loc) = node.locators.first() {
        spans.push(Span::styled(
            format!("  {}", loc),
            Style::default().fg(Color::DarkGray),
        ));
    }
    if is_self {
        spans.push(Span::styled("  (self)", Style::default().fg(Color::Cyan)));
    }
    ListItem::new(Line::from(spans))
}

fn token_item(token: &LivelinessToken) -> ListItem<'static> {
    let (dot, color) = if token.alive {
        ("●", Color::Green)
    } else {
        ("○", Color::Red)
    };
    let name = token.node_name().unwrap_or_else(|| token.key_expr.clone());
    let group = token.group_prefix().unwrap_or_default();

    let mut spans = vec![
        Span::styled(format!("{} ", dot), Style::default().fg(color)),
        Span::styled(name, Style::default().fg(Color::White)),
    ];
    if !group.is_empty() {
        spans.push(Span::styled(
            format!("  {}", group),
            Style::default().fg(Color::DarkGray),
        ));
    }
    ListItem::new(Line::from(spans))
}

fn render_detail(app: &App, frame: &mut Frame, area: Rect) {
    match app.selected_network_row() {
        Some(NetworkRow::Session(i)) => render_session_detail(app, frame, area, i),
        Some(NetworkRow::Service(i)) => render_service_detail(app, frame, area, i),
        None => {
            let block = Block::default().borders(Borders::ALL).title(" Detail ");
            let inner = block.inner(area);
            frame.render_widget(block, area);
            super::render_empty_state(frame, inner, app.nodes_empty_reason());
        }
    }
}

fn section(label: &str) -> Line<'static> {
    Line::from(Span::styled(
        label.to_string(),
        Style::default().fg(Color::Gray).add_modifier(Modifier::DIM),
    ))
}

fn render_session_detail(app: &App, frame: &mut Frame, area: Rect, i: usize) {
    let node = &app.nodes[i];
    let is_self = app.self_zid.as_deref().is_some_and(|z| z == node.zid);

    let title = format!(" {} {} ", node.kind, zid_short(&node.zid));
    let block = Block::default().borders(Borders::ALL).title(title);
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let mut lines: Vec<Line> = Vec::new();

    let mut kind_spans = vec![
        Span::styled("kind: ", Style::default().fg(Color::Gray)),
        Span::styled(
            node.kind.clone(),
            Style::default().fg(kind_color(&node.kind)),
        ),
    ];
    if is_self {
        kind_spans.push(Span::styled(
            "  (self)",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ));
    }
    lines.push(Line::from(kind_spans));
    lines.push(Line::from(vec![
        Span::styled("zid: ", Style::default().fg(Color::Gray)),
        Span::styled(node.zid.clone(), Style::default().fg(Color::Yellow)),
    ]));

    lines.push(Line::from(""));
    lines.push(section("─ locators ─"));
    if node.locators.is_empty() {
        lines.push(Line::from(Span::styled(
            "  (none)",
            Style::default().fg(Color::DarkGray),
        )));
    } else {
        for loc in &node.locators {
            lines.push(Line::from(Span::styled(
                format!("  {}", loc),
                Style::default().fg(Color::White),
            )));
        }
    }

    // Links: edges from the observed topology that touch this node's zid.
    lines.push(Line::from(""));
    lines.push(section("─ links ─"));
    let topo = build_topology(&app.nodes);
    let related: Vec<_> = topo
        .edges
        .iter()
        .filter(|e| e.from_zid == node.zid || e.to_zid == node.zid)
        .collect();
    if related.is_empty() {
        lines.push(Line::from(Span::styled(
            "  (no links observed)",
            Style::default().fg(Color::DarkGray),
        )));
    } else {
        for e in related {
            let (arrow, other) = if e.from_zid == node.zid {
                ("→", &e.to_zid)
            } else {
                ("←", &e.from_zid)
            };
            let mut spans = vec![
                Span::styled(
                    format!("  {} ", arrow),
                    Style::default().fg(Color::DarkGray),
                ),
                Span::styled(
                    zid_short(other).to_string(),
                    Style::default().fg(Color::White),
                ),
            ];
            if let Some(dst) = &e.link_dst {
                spans.push(Span::styled(
                    format!("  {}", dst),
                    Style::default().fg(Color::DarkGray),
                ));
            }
            lines.push(Line::from(spans));
        }
    }

    let para = Paragraph::new(lines)
        .wrap(Wrap { trim: false })
        .scroll((app.node_detail_scroll, 0));
    frame.render_widget(para, inner);
}

fn render_service_detail(app: &App, frame: &mut Frame, area: Rect, i: usize) {
    let token = &app.liveliness_tokens[i];
    let name = token.node_name().unwrap_or_else(|| token.key_expr.clone());
    let group = token.group_prefix().unwrap_or_default();

    let title = format!(" {} ", name);
    let block = Block::default().borders(Borders::ALL).title(title);
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let (dot, status_text, status_color) = if token.alive {
        ("●", "alive", Color::Green)
    } else {
        ("○", "dead", Color::Red)
    };

    let mut lines: Vec<Line> = vec![
        Line::from(vec![
            Span::styled("status: ", Style::default().fg(Color::Gray)),
            Span::styled(
                format!("{} {}", dot, status_text),
                Style::default().fg(status_color),
            ),
        ]),
        Line::from(vec![
            Span::styled("key: ", Style::default().fg(Color::Gray)),
            Span::styled(token.key_expr.clone(), Style::default().fg(Color::White)),
        ]),
        Line::from(vec![
            Span::styled("group: ", Style::default().fg(Color::Gray)),
            Span::styled(
                if group.is_empty() {
                    "(ungrouped)".to_string()
                } else {
                    group
                },
                Style::default().fg(Color::White),
            ),
        ]),
    ];

    lines.push(Line::from(""));
    lines.push(section("─ join/leave history ─"));
    let now = Instant::now();
    let events: Vec<_> = app
        .liveliness_events
        .iter()
        .filter(|e| e.key_expr == token.key_expr)
        .collect();
    if events.is_empty() {
        lines.push(Line::from(Span::styled(
            "  (no events recorded)",
            Style::default().fg(Color::DarkGray),
        )));
    } else {
        for evt in events {
            let ago = now.duration_since(evt.timestamp);
            let time_str = if ago.as_secs() < 60 {
                format!("{:>3}s ago", ago.as_secs())
            } else {
                format!("{:>2}m {:02}s", ago.as_secs() / 60, ago.as_secs() % 60)
            };
            let (kind_text, kind_color) = if evt.is_join {
                ("JOIN ", Color::Green)
            } else {
                ("LEAVE", Color::Red)
            };
            lines.push(Line::from(vec![
                Span::styled(
                    format!("  {} ", time_str),
                    Style::default().fg(Color::DarkGray),
                ),
                Span::styled(kind_text, Style::default().fg(kind_color)),
            ]));
        }
    }

    let para = Paragraph::new(lines)
        .wrap(Wrap { trim: false })
        .scroll((app.node_detail_scroll, 0));
    frame.render_widget(para, inner);
}
