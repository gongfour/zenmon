use crate::app::{App, ConnectionState};
use ratatui::layout::{Constraint, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, Paragraph};
use ratatui::Frame;

pub fn render(app: &mut App, frame: &mut Frame, area: ratatui::layout::Rect) {
    app.list_rect = None;
    let [info_area, body_area] = Layout::vertical([Constraint::Length(5), Constraint::Fill(1)])
        .areas(area);

    let [left_area, right_area] =
        Layout::horizontal([Constraint::Percentage(50), Constraint::Percentage(50)])
            .areas(body_area);

    let (conn_str, conn_color) = match &app.connection_state {
        ConnectionState::Connected(zid) => (format!("Connected ({})", &zid[..zid.len().min(16)]), Color::Green),
        ConnectionState::Connecting => ("Connecting...".to_string(), Color::Yellow),
        ConnectionState::Disconnected(reason) => (format!("Disconnected — {}", reason), Color::Red),
    };

    let info_text = vec![
        Line::from(vec![
            Span::styled("Status: ", Style::default().fg(Color::Gray)),
            Span::styled(conn_str, Style::default().fg(conn_color)),
        ]),
        Line::from(vec![
            Span::styled("Endpoint: ", Style::default().fg(Color::Gray)),
            Span::styled(&app.endpoint, Style::default().fg(Color::White)),
        ]),
        Line::from(vec![
            Span::styled("Topics: ", Style::default().fg(Color::Gray)),
            Span::styled(
                format!("{}", app.topics.len()),
                Style::default().fg(Color::Cyan),
            ),
            Span::raw("  "),
            Span::styled("Nodes: ", Style::default().fg(Color::Gray)),
            Span::styled(
                format!("{}", app.nodes.len()),
                Style::default().fg(Color::Cyan),
            ),
            Span::raw("  "),
            Span::styled("Messages: ", Style::default().fg(Color::Gray)),
            Span::styled(
                format!("{}", app.recent_messages.len()),
                Style::default().fg(Color::Yellow),
            ),
        ]),
        Line::from(vec![
            Span::styled("Throughput: ", Style::default().fg(Color::Gray)),
            Span::styled(
                format!("{:.1} msg/s", app.total_hz),
                Style::default().fg(Color::Green),
            ),
            Span::raw("  "),
            Span::styled("Active topics: ", Style::default().fg(Color::Gray)),
            Span::styled(
                format!("{}", app.topic_hz.values().filter(|&&hz| hz > 0.0).count()),
                Style::default().fg(Color::Cyan),
            ),
        ]),
    ];
    let info = Paragraph::new(info_text)
        .block(Block::default().borders(Borders::ALL).title(" Overview "));
    frame.render_widget(info, info_area);

    let node_items: Vec<ListItem> = app
        .nodes
        .iter()
        .map(|node| {
            let kind_style = match node.kind.as_str() {
                "router" => Style::default().fg(Color::Green),
                "peer" => Style::default().fg(Color::Blue),
                "client" => Style::default().fg(Color::Gray),
                _ => Style::default().fg(Color::White),
            };
            let locator_text = if node.locators.is_empty() {
                "-".to_string()
            } else {
                node.locators.join(", ")
            };
            ListItem::new(Line::from(vec![
                Span::styled(
                    &node.zid[..node.zid.len().min(16)],
                    Style::default()
                        .fg(Color::Yellow)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::raw("  "),
                Span::styled(&node.kind, kind_style),
                Span::raw("  "),
                Span::styled(locator_text, Style::default().fg(Color::DarkGray)),
            ]))
        })
        .collect();
    let node_list = List::new(node_items).block(
        Block::default()
            .borders(Borders::ALL)
            .title(format!(" Nodes ({}) ", app.nodes.len())),
    );
    frame.render_widget(node_list, left_area);

    let topic_items: Vec<ListItem> = app
        .topics
        .iter()
        .map(|topic| {
            let has_data = app.topic_latest.contains_key(&topic.key_expr);
            let hz = app.topic_hz.get(&topic.key_expr).copied().unwrap_or(0.0);
            let topic_style = if has_data {
                Style::default().fg(Color::White)
            } else {
                Style::default().fg(Color::DarkGray)
            };
            let hz_str = if hz > 0.0 {
                format!("{:.1} Hz", hz)
            } else {
                "-".to_string()
            };
            ListItem::new(Line::from(vec![
                Span::styled(
                    &topic.key_expr,
                    topic_style.add_modifier(Modifier::BOLD),
                ),
                Span::raw("  "),
                Span::styled(hz_str, Style::default().fg(Color::Green)),
            ]))
        })
        .collect();
    let topic_list = List::new(topic_items).block(
        Block::default()
            .borders(Borders::ALL)
            .title(format!(" Topics ({}) ", app.topics.len())),
    );
    frame.render_widget(topic_list, right_area);
}
