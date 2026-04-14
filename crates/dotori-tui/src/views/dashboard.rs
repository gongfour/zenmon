use crate::app::App;
use ratatui::layout::{Constraint, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, Paragraph};
use ratatui::Frame;

pub fn render(app: &App, frame: &mut Frame, area: ratatui::layout::Rect) {
    let [info_area, body_area] = Layout::vertical([Constraint::Length(5), Constraint::Fill(1)])
        .areas(area);

    let [left_area, right_area] =
        Layout::horizontal([Constraint::Percentage(50), Constraint::Percentage(50)])
            .areas(body_area);

    let info_text = vec![
        Line::from(vec![
            Span::styled("Connection: ", Style::default().fg(Color::Gray)),
            Span::styled(&app.connection_info, Style::default().fg(Color::Green)),
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
    ];
    let info = Paragraph::new(info_text)
        .block(Block::default().borders(Borders::ALL).title(" Overview "));
    frame.render_widget(info, info_area);

    let msg_items: Vec<ListItem> = app
        .recent_messages
        .iter()
        .take(50)
        .map(|msg| {
            let line = Line::from(vec![
                Span::styled(
                    &msg.key_expr,
                    Style::default()
                        .fg(Color::Cyan)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::raw(" | "),
                Span::styled(format!("{}", msg.payload), Style::default().fg(Color::White)),
            ]);
            ListItem::new(line)
        })
        .collect();
    let msg_list = List::new(msg_items).block(
        Block::default()
            .borders(Borders::ALL)
            .title(" Recent Messages "),
    );
    frame.render_widget(msg_list, left_area);

    let node_items: Vec<ListItem> = app
        .nodes
        .iter()
        .map(|node| {
            let line = Line::from(vec![
                Span::styled(
                    &node.zid[..node.zid.len().min(16)],
                    Style::default().fg(Color::Yellow),
                ),
                Span::raw(" "),
                Span::styled(&node.kind, Style::default().fg(Color::Green)),
                Span::raw(" "),
                Span::styled(node.locators.join(", "), Style::default().fg(Color::Gray)),
            ]);
            ListItem::new(line)
        })
        .collect();
    let node_list = List::new(node_items)
        .block(Block::default().borders(Borders::ALL).title(" Nodes "));
    frame.render_widget(node_list, right_area);
}
