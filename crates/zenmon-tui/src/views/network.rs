use crate::app::App;
use crate::views::topology::{
    build_topology_rows, node_row_count, visual_index_of_node, TopoRow,
};
use zenmon_core::types::NodeSources;
use ratatui::layout::{Constraint, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};
use ratatui::Frame;
use std::time::{Duration, SystemTime};

const STALE_THRESHOLD: Duration = Duration::from_secs(30);
const BOTH_SOURCES: NodeSources = NodeSources::from_bits_retain(
    NodeSources::ADMIN.bits() | NodeSources::SCOUT.bits(),
);

pub fn render(app: &mut App, frame: &mut Frame, area: ratatui::layout::Rect) {
    let [list_area, detail_area] =
        Layout::horizontal([Constraint::Percentage(55), Constraint::Percentage(45)])
            .areas(area);

    render_topology(app, frame, list_area);
    render_node_detail(app, frame, detail_area);
}

fn render_topology(app: &mut App, frame: &mut Frame, area: ratatui::layout::Rect) {
    let now = SystemTime::now();
    let rows = build_topology_rows(&app.nodes, app.self_zid.as_deref(), now);
    let total_nodes = node_row_count(&rows);

    if rows.is_empty() {
        app.list_rect = Some(area);
        let hint = Paragraph::new(Line::from(Span::styled(
            "No nodes yet — press r to scout",
            Style::default().fg(Color::DarkGray),
        )))
        .block(Block::default().borders(Borders::ALL).title(" Topology "));
        frame.render_widget(hint, area);
        return;
    }

    // Clamp selection and compute a visual scroll offset that keeps the
    // selected node visible.
    if total_nodes == 0 {
        app.node_selected = 0;
    } else if app.node_selected >= total_nodes {
        app.node_selected = total_nodes - 1;
    }
    app.list_rect = Some(area);
    app.list_first_item_row = area.y + 1;
    let visible = area.height.saturating_sub(2) as usize;
    let sel_visual = visual_index_of_node(&rows, app.node_selected).unwrap_or(0);
    app.list_scroll_offset = if visible > 0 && sel_visual >= visible {
        sel_visual + 1 - visible
    } else {
        0
    };

    let scout_status = if app.scout_in_progress { " [scouting...]" } else { "" };
    let port = app.scout_port_current.unwrap_or(7446);
    let title = format!(" Topology — scout:{} · {} nodes{} ", port, total_nodes, scout_status);

    let node_idx_before = rows
        .iter()
        .take(app.list_scroll_offset)
        .filter(|r| matches!(r, TopoRow::Node(_)))
        .count();
    let mut node_idx = node_idx_before;
    let lines: Vec<Line> = rows
        .iter()
        .skip(app.list_scroll_offset)
        .take(visible)
        .map(|row| match row {
            TopoRow::Header(label) => {
                Line::from(Span::styled(label.clone(), Style::default().fg(Color::DarkGray)))
            }
            TopoRow::Node(n) => {
                let this = node_idx;
                node_idx += 1;
                topo_node_line(n, this == app.node_selected)
            }
        })
        .collect();

    let para = Paragraph::new(lines)
        .block(Block::default().borders(Borders::ALL).title(title));
    frame.render_widget(para, area);
}

fn topo_node_line<'a>(n: &crate::views::topology::TopoNode, selected: bool) -> Line<'a> {
    let icon = if n.alive { "● " } else { "○ " };
    let icon_style = if n.alive {
        Style::default().fg(Color::Green)
    } else {
        Style::default().fg(Color::Red)
    };
    let kind_color = match n.kind.as_str() {
        "router" => Color::Green,
        "peer" => Color::Blue,
        "client" => Color::Gray,
        _ => Color::White,
    };
    let name_style = if selected {
        Style::default().fg(Color::Black).bg(Color::Cyan).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(Color::White)
    };
    let branch = if n.is_child { " ├─ " } else { "" };
    let cursor = if selected { ">" } else { " " };
    let zid_short = &n.zid[..n.zid.len().min(16)];
    let mut spans = vec![
        Span::raw(format!("{}{}", cursor, branch)),
        Span::styled(icon, if selected { name_style } else { icon_style }),
        Span::styled(format!("{:<7}", n.kind), if selected { name_style } else { Style::default().fg(kind_color) }),
        Span::raw(" "),
        Span::styled(zid_short.to_string(), name_style),
        Span::raw("  "),
        Span::styled(n.locator.clone(), Style::default().fg(Color::DarkGray)),
    ];
    if n.is_self {
        spans.push(Span::styled(" (self)", Style::default().fg(Color::DarkGray)));
    }
    if !n.alive {
        spans.push(Span::styled(" stale", Style::default().fg(Color::Red)));
    }
    Line::from(spans)
}

fn render_node_detail(app: &App, frame: &mut Frame, area: ratatui::layout::Rect) {
    let rows = build_topology_rows(&app.nodes, app.self_zid.as_deref(), SystemTime::now());
    let selected_zid = crate::views::topology::nth_node_zid(&rows, app.node_selected);
    let selected = selected_zid.and_then(|z| app.nodes.iter().find(|n| n.zid == z));

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
