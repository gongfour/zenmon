use crate::app::App;
use dotori_core::types::{NodeInfo, NodeSources};
use ratatui::layout::{Constraint, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Cell, Paragraph, Row, Table, Wrap};
use ratatui::Frame;
use std::time::{Duration, SystemTime};

const STALE_THRESHOLD: Duration = Duration::from_secs(30);
const BOTH_SOURCES: NodeSources = NodeSources::from_bits_retain(
    NodeSources::ADMIN.bits() | NodeSources::SCOUT.bits(),
);

pub fn render(app: &mut App, frame: &mut Frame, area: ratatui::layout::Rect) {
    let [list_area, detail_area] =
        Layout::horizontal([Constraint::Percentage(45), Constraint::Percentage(55)])
            .areas(area);

    render_node_list(app, frame, list_area);
    render_node_detail(app, frame, detail_area);
}

fn render_node_list(app: &mut App, frame: &mut Frame, area: ratatui::layout::Rect) {
    app.list_rect = Some(area);
    app.list_first_item_row = area.y + 3;
    let visible = area.height.saturating_sub(4) as usize;
    app.list_scroll_offset = if visible > 0 && app.node_selected >= visible {
        app.node_selected + 1 - visible
    } else {
        0
    };

    let now = SystemTime::now();
    let header = Row::new(vec![
        Cell::from("ZID"),
        Cell::from("Kind"),
        Cell::from("Source"),
    ])
    .style(
        Style::default()
            .fg(Color::Yellow)
            .add_modifier(Modifier::BOLD),
    )
    .bottom_margin(1);

    let self_zid = app.self_zid.as_deref();
    let rows: Vec<Row> = app
        .nodes
        .iter()
        .enumerate()
        .skip(app.list_scroll_offset)
        .take(visible)
        .map(|(i, node)| build_row(node, i == app.node_selected, now, self_zid))
        .collect();

    let widths = [
        Constraint::Percentage(50),
        Constraint::Percentage(15),
        Constraint::Percentage(35),
    ];

    let (n_admin, n_scout, n_both) = count_by_source(&app.nodes);
    let scout_status = if app.scout_in_progress {
        " [scouting...]"
    } else {
        ""
    };
    let title = format!(
        " Nodes ({}) a:{} s:{} b:{}{} ",
        app.nodes.len(),
        n_admin,
        n_scout,
        n_both,
        scout_status
    );

    let table = Table::new(rows, widths)
        .header(header)
        .block(Block::default().borders(Borders::ALL).title(title));

    frame.render_widget(table, area);
}

fn render_node_detail(app: &App, frame: &mut Frame, area: ratatui::layout::Rect) {
    let selected = app.nodes.get(app.node_selected);

    let Some(node) = selected else {
        let empty = Paragraph::new(Line::from(Span::styled(
            "No node selected",
            Style::default().fg(Color::DarkGray),
        )))
        .block(Block::default().borders(Borders::ALL).title(" Detail "));
        frame.render_widget(empty, area);
        return;
    };

    let is_self = app.self_zid.as_deref().is_some_and(|z| z == node.zid);
    let now = SystemTime::now();
    let stale = node.is_scout_stale(now, STALE_THRESHOLD);
    let (source_text, _) = source_badge(node.sources, stale);

    let mut lines: Vec<Line> = Vec::new();

    // ZID
    let mut zid_spans = vec![
        Span::styled("ZID: ", Style::default().fg(Color::Gray)),
        Span::styled(&node.zid, Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)),
    ];
    if is_self {
        zid_spans.push(Span::styled(" (self)", Style::default().fg(Color::DarkGray)));
    }
    lines.push(Line::from(zid_spans));

    // Kind
    let kind_color = match node.kind.as_str() {
        "router" => Color::Green,
        "peer" => Color::Blue,
        "client" => Color::Gray,
        _ => Color::White,
    };
    lines.push(Line::from(vec![
        Span::styled("Kind: ", Style::default().fg(Color::Gray)),
        Span::styled(&node.kind, Style::default().fg(kind_color)),
    ]));

    // Source
    lines.push(Line::from(vec![
        Span::styled("Source: ", Style::default().fg(Color::Gray)),
        Span::styled(source_text, Style::default().fg(Color::White)),
    ]));

    // Locators
    if node.locators.is_empty() {
        lines.push(Line::from(vec![
            Span::styled("Locators: ", Style::default().fg(Color::Gray)),
            Span::styled("(none)", Style::default().fg(Color::DarkGray)),
        ]));
    } else {
        lines.push(Line::from(Span::styled("Locators:", Style::default().fg(Color::Gray))));
        for loc in &node.locators {
            lines.push(Line::from(Span::styled(
                format!("  {}", loc),
                Style::default().fg(Color::White),
            )));
        }
    }

    // Parse metadata for extra info
    if let Some(meta) = &node.metadata {
        lines.push(Line::from(""));

        // Version
        if let Some(version) = meta.get("version").and_then(|v| v.as_str()) {
            let short_ver = version.split(' ').next().unwrap_or(version);
            lines.push(Line::from(vec![
                Span::styled("Version: ", Style::default().fg(Color::Gray)),
                Span::styled(short_ver, Style::default().fg(Color::Cyan)),
            ]));
        }

        // Plugins
        if let Some(plugins) = meta.get("plugins").and_then(|v| v.as_object()) {
            let plugin_names: Vec<&str> = plugins.keys().map(|k| k.as_str()).collect();
            if !plugin_names.is_empty() {
                lines.push(Line::from(vec![
                    Span::styled("Plugins: ", Style::default().fg(Color::Gray)),
                    Span::styled(plugin_names.join(", "), Style::default().fg(Color::Magenta)),
                ]));
            }
        }

        // Sessions (connected peers/clients)
        if let Some(sessions) = meta.get("sessions").and_then(|v| v.as_array()) {
            if !sessions.is_empty() {
                lines.push(Line::from(""));
                lines.push(Line::from(Span::styled(
                    format!("Sessions ({}):", sessions.len()),
                    Style::default().fg(Color::Gray),
                )));
                for s in sessions {
                    let peer_zid = s.get("peer").and_then(|v| v.as_str()).unwrap_or("?");
                    let whatami = s.get("whatami").and_then(|v| v.as_str()).unwrap_or("?");

                    // Extract link dst address
                    let link_addr = s
                        .get("links")
                        .and_then(|v| v.as_array())
                        .and_then(|links| links.first())
                        .and_then(|link| link.get("dst"))
                        .and_then(|v| v.as_str())
                        .unwrap_or("");

                    let zid_short = &peer_zid[..peer_zid.len().min(16)];
                    let is_session_self = app.self_zid.as_deref().is_some_and(|z| z == peer_zid);

                    let mut spans = vec![
                        Span::styled("  ", Style::default()),
                        Span::styled(
                            zid_short,
                            Style::default().fg(Color::Yellow),
                        ),
                    ];
                    if is_session_self {
                        spans.push(Span::styled("(self)", Style::default().fg(Color::DarkGray)));
                    }
                    spans.push(Span::styled(
                        format!(" {}", whatami),
                        Style::default().fg(Color::Blue),
                    ));
                    if !link_addr.is_empty() {
                        spans.push(Span::styled(
                            format!(" {}", link_addr),
                            Style::default().fg(Color::DarkGray),
                        ));
                    }
                    lines.push(Line::from(spans));
                }
            }
        }
    }

    // Last seen timestamps
    lines.push(Line::from(""));
    if let Some(admin_ts) = node.admin_last_seen {
        let ago = now.duration_since(admin_ts).unwrap_or_default();
        lines.push(Line::from(vec![
            Span::styled("Admin seen: ", Style::default().fg(Color::Gray)),
            Span::styled(format_ago(ago), Style::default().fg(Color::Green)),
        ]));
    }
    if let Some(scout_ts) = node.scout_last_seen {
        let ago = now.duration_since(scout_ts).unwrap_or_default();
        lines.push(Line::from(vec![
            Span::styled("Scout seen: ", Style::default().fg(Color::Gray)),
            Span::styled(
                format_ago(ago),
                Style::default().fg(if stale { Color::Red } else { Color::Green }),
            ),
        ]));
    }

    let title = format!(
        " {} - {} ",
        &node.zid[..node.zid.len().min(16)],
        node.kind,
    );
    let detail = Paragraph::new(lines)
        .block(Block::default().borders(Borders::ALL).title(title))
        .wrap(Wrap { trim: false })
        .scroll((app.node_detail_scroll, 0));
    frame.render_widget(detail, area);
}

fn format_ago(d: Duration) -> String {
    let secs = d.as_secs();
    if secs < 2 {
        "just now".to_string()
    } else if secs < 60 {
        format!("{}s ago", secs)
    } else {
        format!("{}m {}s ago", secs / 60, secs % 60)
    }
}

fn build_row<'a>(
    node: &'a NodeInfo,
    selected: bool,
    now: SystemTime,
    self_zid: Option<&str>,
) -> Row<'a> {
    let stale = node.is_scout_stale(now, STALE_THRESHOLD);

    let base_style = if selected {
        Style::default()
            .fg(Color::Black)
            .bg(Color::Cyan)
            .add_modifier(Modifier::BOLD)
    } else if stale {
        Style::default().fg(Color::DarkGray)
    } else {
        Style::default()
    };

    let kind_cell = if selected {
        Cell::from(node.kind.clone())
    } else {
        let kind_style = match node.kind.as_str() {
            "router" => Style::default().fg(Color::Green),
            "peer" => Style::default().fg(Color::Blue),
            "client" => Style::default().fg(Color::Gray),
            _ => Style::default(),
        };
        Cell::from(node.kind.clone()).style(kind_style)
    };

    let (source_text, source_color) = source_badge(node.sources, stale);
    let source_cell = if selected {
        Cell::from(source_text)
    } else {
        Cell::from(source_text).style(Style::default().fg(source_color))
    };

    let is_self = self_zid.is_some_and(|z| z == node.zid);
    let zid_text = if is_self {
        format!("{} (self)", node.zid)
    } else {
        node.zid.clone()
    };

    Row::new(vec![
        Cell::from(zid_text),
        kind_cell,
        source_cell,
    ])
    .style(base_style)
}

fn source_badge(sources: NodeSources, stale: bool) -> (String, Color) {
    if sources == BOTH_SOURCES {
        ("both".to_string(), Color::Cyan)
    } else if sources.contains(NodeSources::ADMIN) {
        ("admin".to_string(), Color::Green)
    } else if sources.contains(NodeSources::SCOUT) {
        if stale {
            ("scout-stale".to_string(), Color::DarkGray)
        } else {
            ("scout".to_string(), Color::Magenta)
        }
    } else {
        ("-".to_string(), Color::DarkGray)
    }
}

fn count_by_source(nodes: &[NodeInfo]) -> (usize, usize, usize) {
    let mut n_admin = 0;
    let mut n_scout = 0;
    let mut n_both = 0;
    for n in nodes {
        if n.sources == BOTH_SOURCES {
            n_both += 1;
        } else if n.sources.contains(NodeSources::ADMIN) {
            n_admin += 1;
        } else if n.sources.contains(NodeSources::SCOUT) {
            n_scout += 1;
        }
    }
    (n_admin, n_scout, n_both)
}
