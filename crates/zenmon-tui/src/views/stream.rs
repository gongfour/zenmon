use crate::app::App;
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
            let readable = rfc3339
                .strip_suffix('Z')
                .unwrap_or(&rfc3339)
                .replace('T', " ");
            // `to_string_rfc3339_lossy` fraction width varies by zenoh version
            // (micro- vs nanosecond). Cap to microseconds so the display is
            // deterministic, then drop any trailing zeros.
            trim_fractional_zeros(cap_fractional_digits(readable, 6))
        }
        Err(_) => raw.to_string(),
    }
}

/// Truncate the fractional-seconds part of a `HH:MM:SS.ffffff` string to at most
/// `max` digits. Strings with fewer (or no) fractional digits are unchanged.
/// Assumes no trailing timezone offset (the `Z` suffix is stripped beforehand).
fn cap_fractional_digits(mut ts: String, max: usize) -> String {
    if let Some(dot_idx) = ts.find('.') {
        let frac_end = dot_idx + 1 + max;
        if ts.len() > frac_end {
            ts.truncate(frac_end);
        }
    }
    ts
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
    } else if let Some(key) = &app.stream_key_filter {
        format!("Exact topic: {} (/ to edit)", key)
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

    let mode_badge = if app.stream_follow {
        Span::styled(
            " FOLLOW ",
            Style::default().fg(Color::Black).bg(Color::Cyan),
        )
    } else {
        Span::styled(
            " PINNED ",
            Style::default().fg(Color::Black).bg(Color::LightYellow),
        )
    };
    let follow_hint = if app.stream_follow { "" } else { "  f:follow" };

    let status_text = if app.sub_paused {
        Line::from(vec![
            Span::styled(
                " PAUSED ",
                Style::default().fg(Color::Black).bg(Color::Yellow),
            ),
            mode_badge.clone(),
            Span::raw(format!(
                "  showing {} / {} buffered  ",
                filtered_messages.len(),
                app.sub_messages.len()
            )),
            Span::styled(
                format!("Space:resume  /:filter  j/k:scroll{}", follow_hint),
                Style::default().fg(Color::Gray),
            ),
        ])
    } else {
        Line::from(vec![
            Span::styled(
                " LIVE ",
                Style::default().fg(Color::Black).bg(Color::Green),
            ),
            mode_badge,
            Span::raw(format!(
                "  showing {} / {} messages  ",
                filtered_messages.len(),
                app.sub_messages.len()
            )),
            Span::styled(
                format!("Space:pause  /:filter{}", follow_hint),
                Style::default().fg(Color::Gray),
            ),
        ])
    };
    let status = Paragraph::new(status_text)
        .block(Block::default().borders(Borders::ALL).title(" Stream "));
    frame.render_widget(status, status_area);

    let items: Vec<ListItem> = filtered_messages
        .iter()
        .map(|msg| {
            let payload_str = msg.payload.pretty();
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

    if filtered_messages.is_empty() {
        let inner = ratatui::layout::Rect {
            x: messages_area.x + 2,
            y: messages_area.y + 1,
            width: messages_area.width.saturating_sub(4),
            height: messages_area.height.saturating_sub(2),
        };
        super::render_empty_state(frame, inner, app.stream_empty_reason());
    }
}

#[cfg(test)]
mod tests {
    use super::{cap_fractional_digits, format_stream_timestamp, trim_fractional_zeros};

    #[test]
    fn formats_zenoh_timestamp_as_readable_datetime() {
        let formatted = format_stream_timestamp("7386690599959157260/33");
        // `to_string_rfc3339_lossy()` fraction width is zenoh-version dependent
        // (some emit microseconds, some nanoseconds). We cap it to microseconds
        // ourselves so the display is deterministic regardless of that.
        assert_eq!(formatted, "2024-07-01 15:32:06.860479");
    }

    #[test]
    fn caps_fraction_to_microseconds() {
        // Nanosecond fraction is truncated to 6 digits (version-independent).
        assert_eq!(
            cap_fractional_digits("2024-07-01 15:32:06.860479001".to_string(), 6),
            "2024-07-01 15:32:06.860479"
        );
        // Fewer than 6 fractional digits are left untouched.
        assert_eq!(
            cap_fractional_digits("2024-07-01 15:32:06.86".to_string(), 6),
            "2024-07-01 15:32:06.86"
        );
        // No fractional part: unchanged.
        assert_eq!(
            cap_fractional_digits("2024-07-01 15:32:06".to_string(), 6),
            "2024-07-01 15:32:06"
        );
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
