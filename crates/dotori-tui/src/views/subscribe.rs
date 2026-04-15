use crate::app::App;
use dotori_core::types::MessagePayload;
use ratatui::layout::{Constraint, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, ListState, Paragraph};
use ratatui::Frame;

pub fn render(app: &mut App, frame: &mut Frame, area: ratatui::layout::Rect) {
    let [status_area, messages_area] =
        Layout::vertical([Constraint::Length(3), Constraint::Fill(1)]).areas(area);

    app.list_rect = Some(messages_area);
    app.list_first_item_row = messages_area.y + 1;
    let visible = messages_area.height.saturating_sub(2) as usize;
    app.list_scroll_offset = if visible > 0 && app.sub_selected >= visible {
        app.sub_selected + 1 - visible
    } else {
        0
    };

    let status_text = if app.sub_paused {
        Line::from(vec![
            Span::styled(
                " PAUSED ",
                Style::default().fg(Color::Black).bg(Color::Yellow),
            ),
            Span::raw(format!("  {} messages buffered  ", app.sub_messages.len())),
            Span::styled(
                "Space:resume  j/k:scroll",
                Style::default().fg(Color::Gray),
            ),
        ])
    } else {
        Line::from(vec![
            Span::styled(
                " LIVE ",
                Style::default().fg(Color::Black).bg(Color::Green),
            ),
            Span::raw(format!("  {} messages  ", app.sub_messages.len())),
            Span::styled("Space:pause", Style::default().fg(Color::Gray)),
        ])
    };
    let status = Paragraph::new(status_text)
        .block(Block::default().borders(Borders::ALL).title(" Subscribe "));
    frame.render_widget(status, status_area);

    let items: Vec<ListItem> = app
        .sub_messages
        .iter()
        .map(|msg| {
            let payload_str = match &msg.payload {
                MessagePayload::Json(v) => {
                    serde_json::to_string_pretty(v).unwrap_or_else(|_| format!("{}", v))
                }
                other => format!("{}", other),
            };
            let att_str = msg.attachment.as_ref().map(|a| format!(" att:{}", a));
            let ts = msg.timestamp.as_deref().unwrap_or("");
            let mut spans = vec![
                Span::styled(
                    &msg.key_expr,
                    Style::default()
                        .fg(Color::Cyan)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(format!(" [{}]", ts), Style::default().fg(Color::DarkGray)),
                Span::raw(" "),
                Span::styled(payload_str, Style::default().fg(Color::White)),
            ];
            if let Some(att) = att_str {
                spans.push(Span::styled(att, Style::default().fg(Color::Magenta)));
            }
            ListItem::new(Line::from(spans))
        })
        .collect();

    let list = List::new(items)
        .block(Block::default().borders(Borders::ALL).title(" Messages "))
        .highlight_style(
            Style::default()
                .bg(Color::DarkGray)
                .add_modifier(Modifier::BOLD),
        );

    let mut state = ListState::default().with_selected(Some(app.sub_selected));
    frame.render_stateful_widget(list, messages_area, &mut state);
}
