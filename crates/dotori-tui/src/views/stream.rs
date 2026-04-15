use crate::app::App;
use dotori_core::types::MessagePayload;
use ratatui::layout::{Constraint, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, ListState, Paragraph};
use ratatui::Frame;
use std::str::FromStr;

fn format_stream_timestamp(raw: &str) -> String {
    if raw.is_empty() {
        return String::new();
    }

    match zenoh::time::Timestamp::from_str(raw) {
        Ok(ts) => {
            let rfc3339 = ts.get_time().to_string_rfc3339_lossy();
            trim_fractional_zeros(
                rfc3339
                    .strip_suffix('Z')
                    .unwrap_or(&rfc3339)
                    .replace('T', " "),
            )
        }
        Err(_) => raw.to_string(),
    }
}

fn trim_fractional_zeros(mut ts: String) -> String {
    if let Some(dot_idx) = ts.find('.') {
        let mut end = ts.len();
        while end > dot_idx + 1 && ts.as_bytes()[end - 1] == b'0' {
            end -= 1;
        }
        if end == dot_idx + 1 {
            end -= 1;
        }
        ts.truncate(end);
    }
    ts
}

pub fn render(app: &mut App, frame: &mut Frame, area: ratatui::layout::Rect) {
    let [filter_area, status_area, messages_area] = Layout::vertical([
        Constraint::Length(3),
        Constraint::Length(3),
        Constraint::Fill(1),
    ])
    .areas(area);

    let filter_text = if app.stream_filtering {
        format!("/{}_", app.stream_filter)
    } else if app.stream_filter.is_empty() {
        "Press / to filter stream".to_string()
    } else {
        format!("Filter: {} (/ to edit)", app.stream_filter)
    };
    let filter_style = if app.stream_filtering {
        Style::default().fg(Color::Yellow)
    } else {
        Style::default().fg(Color::Gray)
    };
    let filter = Paragraph::new(filter_text)
        .style(filter_style)
        .block(Block::default().borders(Borders::ALL).title(" Filter "));
    frame.render_widget(filter, filter_area);

    app.list_rect = Some(messages_area);
    app.list_first_item_row = messages_area.y + 1;
    let visible = messages_area.height.saturating_sub(2) as usize;
    app.list_scroll_offset = if visible > 0 && app.sub_selected >= visible {
        app.sub_selected + 1 - visible
    } else {
        0
    };
    let filtered_messages = app.filtered_sub_messages();

    let status_text = if app.sub_paused {
        Line::from(vec![
            Span::styled(
                " PAUSED ",
                Style::default().fg(Color::Black).bg(Color::Yellow),
            ),
            Span::raw(format!(
                "  showing {} / {} buffered  ",
                filtered_messages.len(),
                app.sub_messages.len()
            )),
            Span::styled(
                "Space:resume  /:filter  j/k:scroll",
                Style::default().fg(Color::Gray),
            ),
        ])
    } else {
        Line::from(vec![
            Span::styled(
                " LIVE ",
                Style::default().fg(Color::Black).bg(Color::Green),
            ),
            Span::raw(format!(
                "  showing {} / {} messages  ",
                filtered_messages.len(),
                app.sub_messages.len()
            )),
            Span::styled("Space:pause  /:filter", Style::default().fg(Color::Gray)),
        ])
    };
    let status = Paragraph::new(status_text)
        .block(Block::default().borders(Borders::ALL).title(" Stream "));
    frame.render_widget(status, status_area);

    let items: Vec<ListItem> = filtered_messages
        .iter()
        .map(|msg| {
            let payload_str = match &msg.payload {
                MessagePayload::Json(v) => {
                    serde_json::to_string_pretty(v).unwrap_or_else(|_| format!("{}", v))
                }
                other => format!("{}", other),
            };
            let att_str = msg.attachment.as_ref().map(|a| format!(" att:{}", a));
            let ts = format_stream_timestamp(msg.timestamp.as_deref().unwrap_or(""));
            let mut spans = vec![
                Span::styled(format!("[{}] ", ts), Style::default().fg(Color::DarkGray)),
                Span::styled(
                    &msg.key_expr,
                    Style::default()
                        .fg(Color::Cyan)
                        .add_modifier(Modifier::BOLD),
                ),
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
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(format!(" Messages ({}) ", filtered_messages.len())),
        )
        .highlight_style(
            Style::default()
                .bg(Color::DarkGray)
                .add_modifier(Modifier::BOLD),
        );

    let mut state = ListState::default().with_selected(Some(app.sub_selected));
    frame.render_stateful_widget(list, messages_area, &mut state);
}

#[cfg(test)]
mod tests {
    use super::{format_stream_timestamp, trim_fractional_zeros};

    #[test]
    fn formats_zenoh_timestamp_as_readable_datetime() {
        let formatted = format_stream_timestamp("7386690599959157260/33");
        assert_eq!(formatted, "2024-07-01 15:32:06.860479");
    }

    #[test]
    fn keeps_raw_timestamp_when_parsing_fails() {
        assert_eq!(format_stream_timestamp("not-a-timestamp"), "not-a-timestamp");
    }

    #[test]
    fn keeps_empty_timestamp_empty() {
        assert_eq!(format_stream_timestamp(""), "");
    }

    #[test]
    fn trims_trailing_fractional_zeros() {
        assert_eq!(
            trim_fractional_zeros("2024-07-01 15:32:06.860479000".to_string()),
            "2024-07-01 15:32:06.860479"
        );
        assert_eq!(
            trim_fractional_zeros("2024-07-01 15:32:06.000000000".to_string()),
            "2024-07-01 15:32:06"
        );
    }
}
