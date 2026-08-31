//! Traffic space — master/detail view over live key expressions.
//!
//! Left (master): a flat, sorted key list with Hz + a tiny bandwidth sparkbar.
//! Right (detail): the selected key in **Live** mode (latest payload + scrolling
//! history scoped to that key) or **Query** mode (results of a `get`).

use crate::app::{format_bytes_per_sec, App, BodyLayout, DetailMode, PaneFocus, QueryStatus};
use crate::views::fmt::format_stream_timestamp;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, ListState, Paragraph, Wrap};
use ratatui::Frame;

/// Unicode block glyphs (low→high) for the master sparkbar.
const SPARK_GLYPHS: [char; 8] = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];

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

/// Render a compact sparkbar from the tail of a bandwidth series. Empty when the
/// series is empty or all-zero (an idle key shows no bar rather than a flat one).
fn sparkbar(series: &[u64], width: usize) -> String {
    if series.is_empty() || width == 0 {
        return String::new();
    }
    let tail = if series.len() > width {
        &series[series.len() - width..]
    } else {
        series
    };
    let max = tail.iter().copied().max().unwrap_or(0);
    if max == 0 {
        return String::new();
    }
    tail.iter()
        .map(|&v| {
            let idx = ((v as f64 / max as f64) * (SPARK_GLYPHS.len() - 1) as f64).round() as usize;
            SPARK_GLYPHS[idx.min(SPARK_GLYPHS.len() - 1)]
        })
        .collect()
}

fn render_master(app: &mut App, frame: &mut Frame, area: Rect) {
    let filtered = app.filtered_topics();
    let n = filtered.len();

    let title = if app.topics_filtering {
        format!(" Traffic — keys ({}) · /{}_ ", n, app.topic_filter)
    } else if !app.topic_filter.is_empty() {
        format!(" Traffic — keys ({}) · /{} ", n, app.topic_filter)
    } else {
        format!(" Traffic — keys ({}) ", n)
    };

    let items: Vec<ListItem> = filtered
        .iter()
        .map(|topic| {
            let key = &topic.key_expr;
            let has_data = app.topic_latest.contains_key(key);
            let hz = app.topic_hz.get(key).copied().unwrap_or(0.0);
            let hz_str = if hz > 0.0 {
                format!("{:>6.1} Hz", hz)
            } else {
                "      — ".to_string()
            };
            let spark = sparkbar(&app.topic_rate_series(key), 8);
            let key_style = if has_data {
                Style::default().fg(Color::White)
            } else {
                Style::default().fg(Color::DarkGray)
            };
            ListItem::new(Line::from(vec![
                Span::styled(key.clone(), key_style),
                Span::raw("  "),
                Span::styled(hz_str, Style::default().fg(Color::Green)),
                Span::raw(" "),
                Span::styled(spark, Style::default().fg(Color::Cyan)),
            ]))
        })
        .collect();

    let block = Block::default().borders(Borders::ALL).title(title);
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
    app.list_scroll_offset = if visible > 0 && app.topic_selected >= visible {
        app.topic_selected + 1 - visible
    } else {
        0
    };

    let mut state = ListState::default();
    if n > 0 {
        state.select(Some(app.topic_selected.min(n - 1)));
    }
    frame.render_stateful_widget(list, area, &mut state);

    if n == 0 {
        let inner = Rect {
            x: area.x + 2,
            y: area.y + 1,
            width: area.width.saturating_sub(4),
            height: area.height.saturating_sub(2),
        };
        super::render_empty_state(frame, inner, app.topics_empty_reason());
    }
}

fn render_detail(app: &App, frame: &mut Frame, area: Rect) {
    let key = app
        .filtered_topics()
        .get(app.topic_selected)
        .map(|t| t.key_expr.clone());

    let Some(key) = key else {
        let block = Block::default().borders(Borders::ALL).title(" Detail ");
        let inner = block.inner(area);
        frame.render_widget(block, area);
        super::render_empty_state(frame, inner, app.topics_empty_reason());
        return;
    };

    let title = format!(" {} ", key);
    let block = Block::default().borders(Borders::ALL).title(title);
    let inner = block.inner(area);
    frame.render_widget(block, area);

    if inner.height < 3 {
        return;
    }

    // Fixed header (stats + mode indicator), then the scrollable body.
    let [header_area, body_area] =
        Layout::vertical([Constraint::Length(2), Constraint::Fill(1)]).areas(inner);

    let history: Vec<&zenmon_core::types::ZenohMessage> = app
        .sub_messages
        .iter()
        .filter(|m| m.key_expr == key)
        .collect();

    // Header line: {hz} Hz · {bw} · {encoding?} · {n} msgs
    let hz = app.topic_hz.get(&key).copied().unwrap_or(0.0);
    let bw = format_bytes_per_sec(app.topic_bytes_per_sec(&key));
    let mut header = format!("{:.1} Hz · {}", hz, bw);
    if let Some(enc) = app
        .topic_latest
        .get(&key)
        .map(|(m, _)| m.encoding.as_str())
        .filter(|e| !e.is_empty())
    {
        header.push_str(&format!(" · {}", enc));
    }
    header.push_str(&format!(" · {} msgs", history.len()));

    let active = Style::default()
        .fg(Color::Black)
        .bg(Color::Cyan)
        .add_modifier(Modifier::BOLD);
    let inactive = Style::default().fg(Color::DarkGray);
    let mode_line = Line::from(vec![
        Span::styled(
            "[L]ive",
            if app.detail_mode == DetailMode::Live {
                active
            } else {
                inactive
            },
        ),
        Span::raw("  "),
        Span::styled(
            "[Q]uery",
            if app.detail_mode == DetailMode::Query {
                active
            } else {
                inactive
            },
        ),
    ]);
    frame.render_widget(
        Paragraph::new(vec![
            Line::from(Span::styled(header, Style::default().fg(Color::Gray))),
            mode_line,
        ]),
        header_area,
    );

    let body_lines = match app.detail_mode {
        DetailMode::Live => live_body(app, &key, &history),
        DetailMode::Query => query_body(app),
    };

    let para = Paragraph::new(body_lines)
        .wrap(Wrap { trim: false })
        .scroll((app.topic_detail_scroll, 0));
    frame.render_widget(para, body_area);
}

/// Truncate a single-line payload snippet for the history/results list.
fn snippet(text: &str, max: usize) -> String {
    let one_line: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if one_line.chars().count() > max {
        let cut: String = one_line.chars().take(max.saturating_sub(1)).collect();
        format!("{}…", cut)
    } else {
        one_line
    }
}

fn live_body<'a>(
    app: &'a App,
    key: &str,
    history: &[&'a zenmon_core::types::ZenohMessage],
) -> Vec<Line<'a>> {
    let mut lines: Vec<Line> = Vec::new();

    lines.push(section("─ latest ─"));
    match app.topic_latest.get(key) {
        Some((msg, _)) => {
            for line in msg.payload.pretty().lines() {
                lines.push(Line::from(Span::styled(
                    format!("  {}", line),
                    Style::default().fg(Color::White),
                )));
            }
            if let Some(att) = &msg.attachment {
                lines.push(Line::from(Span::styled(
                    "  attachment:",
                    Style::default().fg(Color::Magenta),
                )));
                for line in att.pretty().lines() {
                    lines.push(Line::from(Span::styled(
                        format!("    {}", line),
                        Style::default().fg(Color::Magenta),
                    )));
                }
            }
        }
        None => lines.push(Line::from(Span::styled(
            "  (no payload received yet)",
            Style::default().fg(Color::DarkGray),
        ))),
    }

    lines.push(Line::from(""));
    let hist_header = if app.sub_paused {
        "─ history (paused, space=resume) ─"
    } else {
        "─ history (live, space=pause) ─"
    };
    lines.push(section(hist_header));

    if history.is_empty() {
        lines.push(Line::from(Span::styled(
            "  (no messages buffered for this key)",
            Style::default().fg(Color::DarkGray),
        )));
    } else {
        for msg in history {
            let ts = format_stream_timestamp(msg.timestamp.as_deref().unwrap_or(""));
            let mut spans = vec![
                Span::styled(format!("{}  ", ts), Style::default().fg(Color::DarkGray)),
                Span::styled(
                    snippet(&msg.payload.to_string(), 60),
                    Style::default().fg(Color::White),
                ),
            ];
            if msg.attachment.is_some() {
                spans.push(Span::styled("  att", Style::default().fg(Color::Magenta)));
            }
            lines.push(Line::from(spans));
        }
    }

    lines
}

fn query_body(app: &App) -> Vec<Line<'_>> {
    let mut lines: Vec<Line> = Vec::new();

    let status = match &app.query_status {
        QueryStatus::Idle => Span::styled(
            "Idle — press Q to run a get on this key",
            Style::default().fg(Color::DarkGray),
        ),
        QueryStatus::Running => Span::styled("Running…", Style::default().fg(Color::Yellow)),
        QueryStatus::Done(n) => Span::styled(
            format!("Done — {} result(s)", n),
            Style::default().fg(Color::Green),
        ),
        QueryStatus::Error(e) => {
            Span::styled(format!("Error: {}", e), Style::default().fg(Color::Red))
        }
    };
    lines.push(Line::from(status));
    lines.push(Line::from(""));
    lines.push(section("─ results ─"));

    if app.query_results.is_empty() {
        lines.push(Line::from(Span::styled(
            "  (no results)",
            Style::default().fg(Color::DarkGray),
        )));
    } else {
        for msg in &app.query_results {
            let ts = format_stream_timestamp(msg.timestamp.as_deref().unwrap_or(""));
            let mut spans = vec![
                Span::styled(
                    msg.key_expr.clone(),
                    Style::default()
                        .fg(Color::Cyan)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::raw("  "),
                Span::styled(
                    snippet(&msg.payload.to_string(), 60),
                    Style::default().fg(Color::White),
                ),
            ];
            if !ts.is_empty() {
                spans.push(Span::styled(
                    format!("  {}", ts),
                    Style::default().fg(Color::DarkGray),
                ));
            }
            lines.push(Line::from(spans));
        }
    }

    lines
}

fn section(label: &str) -> Line<'_> {
    Line::from(Span::styled(
        label,
        Style::default().fg(Color::Gray).add_modifier(Modifier::DIM),
    ))
}
