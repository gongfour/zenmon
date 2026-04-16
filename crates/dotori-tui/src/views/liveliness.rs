use crate::app::{App, LivelinessEventRecord};
use ratatui::layout::{Constraint, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};
use ratatui::Frame;

pub fn render(app: &mut App, frame: &mut Frame, area: ratatui::layout::Rect) {
    let [top_area, log_area] =
        Layout::vertical([Constraint::Percentage(70), Constraint::Percentage(30)])
            .areas(area);

    let [list_area, detail_area] =
        Layout::horizontal([Constraint::Percentage(45), Constraint::Percentage(55)])
            .areas(top_area);

    render_token_list(app, frame, list_area);
    render_token_detail(app, frame, detail_area);
    render_event_log(app, frame, log_area);
}

/// Build a flat list of display rows: group headers + token rows.
fn build_grouped_rows(app: &App) -> Vec<GroupedRow> {
    let tokens = &app.liveliness_tokens;
    if tokens.is_empty() {
        return Vec::new();
    }

    // Group tokens by prefix
    let mut groups: Vec<(String, Vec<(usize, &dotori_core::types::LivelinessToken)>)> = Vec::new();
    for (i, token) in tokens.iter().enumerate() {
        let group = token.group_prefix().unwrap_or_else(|| "(ungrouped)".to_string());
        if let Some(g) = groups.iter_mut().find(|(k, _)| *k == group) {
            g.1.push((i, token));
        } else {
            groups.push((group, vec![(i, token)]));
        }
    }
    groups.sort_by(|a, b| a.0.cmp(&b.0));

    let mut rows = Vec::new();
    for (group_name, group_tokens) in &groups {
        let alive = group_tokens.iter().filter(|(_, t)| t.alive).count();
        let total = group_tokens.len();
        rows.push(GroupedRow::Header {
            label: format!("── {} ({}/{}) ──", group_name, alive, total),
        });
        for (token_idx, token) in group_tokens {
            rows.push(GroupedRow::Token {
                token_idx: *token_idx,
                name: token.node_name().unwrap_or_else(|| token.key_expr.clone()),
                alive: token.alive,
            });
        }
    }
    rows
}

enum GroupedRow {
    Header { label: String },
    Token { token_idx: usize, name: String, alive: bool },
}

fn render_token_list(app: &mut App, frame: &mut Frame, area: ratatui::layout::Rect) {
    app.list_rect = Some(area);
    app.list_first_item_row = area.y + 1;

    let rows = build_grouped_rows(app);

    let alive_count = app.liveliness_tokens.iter().filter(|t| t.alive).count();
    let dead_count = app.liveliness_tokens.iter().filter(|t| !t.alive).count();
    let title = if dead_count > 0 {
        format!(" Liveliness ({} alive, {} dead) ", alive_count, dead_count)
    } else {
        format!(" Liveliness ({} alive) ", alive_count)
    };

    let lines: Vec<Line> = rows
        .iter()
        .map(|row| match row {
            GroupedRow::Header { label } => {
                Line::from(Span::styled(label, Style::default().fg(Color::DarkGray)))
            }
            GroupedRow::Token { token_idx, name, alive } => {
                let selected = *token_idx == app.liveliness_selected;
                let (icon, icon_style) = if *alive {
                    ("● ", Style::default().fg(Color::Green))
                } else {
                    ("○ ", Style::default().fg(Color::Red))
                };
                let name_style = if selected {
                    Style::default()
                        .fg(Color::Black)
                        .bg(Color::Cyan)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(Color::White)
                };
                let prefix = if selected { "> " } else { "  " };
                Line::from(vec![
                    Span::raw(prefix),
                    Span::styled(icon, if selected { name_style } else { icon_style }),
                    Span::styled(name, name_style),
                ])
            }
        })
        .collect();

    let paragraph = Paragraph::new(lines)
        .block(Block::default().borders(Borders::ALL).title(title));
    frame.render_widget(paragraph, area);
}

fn render_token_detail(app: &App, frame: &mut Frame, area: ratatui::layout::Rect) {
    let selected = app.liveliness_tokens.get(app.liveliness_selected);

    let Some(token) = selected else {
        let empty = Paragraph::new(Line::from(Span::styled(
            "No token selected",
            Style::default().fg(Color::DarkGray),
        )))
        .block(Block::default().borders(Borders::ALL).title(" Detail "));
        frame.render_widget(empty, area);
        return;
    };

    let name = token.node_name().unwrap_or_else(|| token.key_expr.clone());
    let group = token.group_prefix().unwrap_or_default();
    let (status_icon, status_text, status_color) = if token.alive {
        ("●", "alive", Color::Green)
    } else {
        ("○", "dead", Color::Red)
    };

    let lines = vec![
        Line::from(Span::styled(
            &name,
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        Line::from(vec![
            Span::styled("Key: ", Style::default().fg(Color::Gray)),
            Span::styled(&token.key_expr, Style::default().fg(Color::White)),
        ]),
        Line::from(vec![
            Span::styled("Group: ", Style::default().fg(Color::Gray)),
            Span::styled(&group, Style::default().fg(Color::White)),
        ]),
        Line::from(vec![
            Span::styled("Status: ", Style::default().fg(Color::Gray)),
            Span::styled(
                format!("{} {}", status_icon, status_text),
                Style::default().fg(status_color),
            ),
        ]),
    ];

    let title = format!(" {} ", name);
    let detail = Paragraph::new(lines)
        .block(Block::default().borders(Borders::ALL).title(title))
        .wrap(Wrap { trim: false });
    frame.render_widget(detail, area);
}

fn render_event_log(app: &App, frame: &mut Frame, area: ratatui::layout::Rect) {
    let title = format!(" Event Log ({}) - Shift+J/K:scroll ", app.liveliness_events.len());

    if app.liveliness_events.is_empty() {
        let empty = Paragraph::new(Line::from(Span::styled(
            "  No events yet — waiting for liveliness changes...",
            Style::default().fg(Color::DarkGray),
        )))
        .block(Block::default().borders(Borders::ALL).title(title));
        frame.render_widget(empty, area);
        return;
    }

    let now = std::time::Instant::now();
    let lines: Vec<Line> = app
        .liveliness_events
        .iter()
        .map(|evt| format_event_line(evt, now))
        .collect();

    let log = Paragraph::new(lines)
        .block(Block::default().borders(Borders::ALL).title(title))
        .scroll((app.liveliness_log_scroll, 0));
    frame.render_widget(log, area);
}

fn format_event_line<'a>(evt: &LivelinessEventRecord, now: std::time::Instant) -> Line<'a> {
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

    let group_suffix = if evt.group.is_empty() {
        String::new()
    } else {
        format!("  ({})", evt.group)
    };

    Line::from(vec![
        Span::styled(
            format!("  {} ", time_str),
            Style::default().fg(Color::DarkGray),
        ),
        Span::styled(
            format!("{} ", kind_text),
            Style::default().fg(kind_color),
        ),
        Span::styled(
            evt.node_name.clone(),
            Style::default().fg(Color::White),
        ),
        Span::styled(group_suffix, Style::default().fg(Color::DarkGray)),
    ])
}
