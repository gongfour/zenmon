use crate::app::App;
use dotori_core::types::MessagePayload;
use ratatui::layout::{Constraint, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, Paragraph, Wrap};
use ratatui::Frame;

pub fn render(app: &App, frame: &mut Frame, area: ratatui::layout::Rect) {
    let [filter_area, body_area] =
        Layout::vertical([Constraint::Length(3), Constraint::Fill(1)]).areas(area);

    let [list_area, detail_area] =
        Layout::horizontal([Constraint::Percentage(35), Constraint::Percentage(65)])
            .areas(body_area);

    // Filter bar
    let filter_text = if app.topics_filtering {
        format!("/{}_", app.topic_filter)
    } else if app.topic_filter.is_empty() {
        "Press / to filter".to_string()
    } else {
        format!("Filter: {} (/ to edit)", app.topic_filter)
    };
    let filter_style = if app.topics_filtering {
        Style::default().fg(Color::Yellow)
    } else {
        Style::default().fg(Color::Gray)
    };
    let filter = Paragraph::new(filter_text)
        .style(filter_style)
        .block(Block::default().borders(Borders::ALL).title(" Filter "));
    frame.render_widget(filter, filter_area);

    // Topic list (left)
    let filtered = app.filtered_topics();
    let items: Vec<ListItem> = filtered
        .iter()
        .enumerate()
        .map(|(i, topic)| {
            let is_selected = i == app.topic_selected;
            let has_data = app.topic_latest.contains_key(&topic.key_expr);
            let hz = app.topic_hz.get(&topic.key_expr).copied().unwrap_or(0.0);
            let style = if is_selected {
                Style::default()
                    .fg(Color::Black)
                    .bg(Color::Cyan)
                    .add_modifier(Modifier::BOLD)
            } else if has_data {
                Style::default().fg(Color::White)
            } else {
                Style::default().fg(Color::DarkGray)
            };
            let prefix = if is_selected { ">> " } else { "   " };
            let hz_str = if hz > 0.0 {
                format!(" {:.1} Hz", hz)
            } else {
                String::new()
            };
            ListItem::new(Line::from(vec![
                Span::raw(prefix),
                Span::styled(&topic.key_expr, style),
                Span::styled(hz_str, Style::default().fg(Color::Green)),
            ]))
        })
        .collect();

    let list = List::new(items).block(
        Block::default().borders(Borders::ALL).title(format!(
            " Topics ({}) j/k:nav ",
            filtered.len()
        )),
    );
    frame.render_widget(list, list_area);

    // Detail panel (right) — latest value of selected topic
    let selected_key = filtered.get(app.topic_selected).map(|t| &t.key_expr);

    if let Some(key) = selected_key {
        if let Some((msg, received_at)) = app.topic_latest.get(key.as_str()) {
            let age = received_at.elapsed();
            let age_str = if age.as_secs() >= 60 {
                format!("{}m {}s ago", age.as_secs() / 60, age.as_secs() % 60)
            } else {
                format!("{:.1}s ago", age.as_secs_f64())
            };

            let payload_str = match &msg.payload {
                MessagePayload::Json(v) => {
                    serde_json::to_string_pretty(v).unwrap_or_else(|_| format!("{}", v))
                }
                other => format!("{}", other),
            };

            let mut lines = vec![
                Line::from(vec![
                    Span::styled("Topic: ", Style::default().fg(Color::Gray)),
                    Span::styled(key, Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)),
                ]),
                Line::from(vec![
                    Span::styled("Updated: ", Style::default().fg(Color::Gray)),
                    Span::styled(
                        &age_str,
                        Style::default().fg(if age.as_secs() < 5 {
                            Color::Green
                        } else if age.as_secs() < 30 {
                            Color::Yellow
                        } else {
                            Color::Red
                        }),
                    ),
                ]),
                Line::from(vec![
                    Span::styled("Kind: ", Style::default().fg(Color::Gray)),
                    Span::styled(&msg.kind, Style::default().fg(Color::White)),
                ]),
                Line::from(vec![
                    Span::styled("Rate: ", Style::default().fg(Color::Gray)),
                    Span::styled(
                        format!("{:.1} Hz", app.topic_hz.get(key.as_str()).copied().unwrap_or(0.0)),
                        Style::default().fg(Color::Green),
                    ),
                ]),
                Line::from(""),
                Line::from(Span::styled("Payload:", Style::default().fg(Color::Gray))),
            ];

            for line in payload_str.lines() {
                lines.push(Line::from(Span::styled(
                    format!("  {}", line),
                    Style::default().fg(Color::White),
                )));
            }

            if let Some(att) = &msg.attachment {
                let att_str = match att {
                    MessagePayload::Json(v) => {
                        serde_json::to_string_pretty(v).unwrap_or_else(|_| format!("{}", v))
                    }
                    other => format!("{}", other),
                };
                lines.push(Line::from(""));
                lines.push(Line::from(Span::styled(
                    "Attachment:",
                    Style::default().fg(Color::Magenta),
                )));
                for line in att_str.lines() {
                    lines.push(Line::from(Span::styled(
                        format!("  {}", line),
                        Style::default().fg(Color::Magenta),
                    )));
                }
            }

            let scroll_hint = if app.topic_detail_scroll > 0 {
                format!(" Latest Value (Shift+J/K scroll, line {}) ", app.topic_detail_scroll)
            } else {
                " Latest Value (Shift+J/K scroll) ".to_string()
            };
            let detail = Paragraph::new(lines)
                .block(Block::default().borders(Borders::ALL).title(scroll_hint))
                .wrap(Wrap { trim: false })
                .scroll((app.topic_detail_scroll, 0));
            frame.render_widget(detail, detail_area);
        } else {
            let detail = Paragraph::new(Line::from(Span::styled(
                "No data received yet",
                Style::default().fg(Color::DarkGray),
            )))
            .block(Block::default().borders(Borders::ALL).title(" Latest Value "));
            frame.render_widget(detail, detail_area);
        }
    } else {
        let detail = Paragraph::new(Line::from(Span::styled(
            "No topic selected",
            Style::default().fg(Color::DarkGray),
        )))
        .block(Block::default().borders(Borders::ALL).title(" Latest Value "));
        frame.render_widget(detail, detail_area);
    }
}
