use crate::app::{App, QueryStatus};
use dotori_core::types::MessagePayload;
use ratatui::layout::{Constraint, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, Paragraph};
use ratatui::Frame;

pub fn render(app: &App, frame: &mut Frame, area: ratatui::layout::Rect) {
    let [input_area, status_area, results_area, history_area] = Layout::vertical([
        Constraint::Length(3),
        Constraint::Length(1),
        Constraint::Fill(1),
        Constraint::Length(6),
    ])
    .areas(area);

    let input_text = if app.query_editing {
        format!("GET > {}_", app.query_input)
    } else if app.query_input.is_empty() {
        "Press / or i to enter key expression".to_string()
    } else {
        format!(
            "GET > {}  (Enter to execute, / to edit)",
            app.query_input
        )
    };
    let input_style = if app.query_editing {
        Style::default().fg(Color::Yellow)
    } else {
        Style::default().fg(Color::Gray)
    };
    let input = Paragraph::new(input_text)
        .style(input_style)
        .block(Block::default().borders(Borders::ALL).title(" Query "));
    frame.render_widget(input, input_area);

    // Query status line
    let status_line = match &app.query_status {
        QueryStatus::Idle => Line::from(Span::styled(
            " Ready — enter a key expression to query",
            Style::default().fg(Color::DarkGray),
        )),
        QueryStatus::Running => Line::from(Span::styled(
            " Querying...",
            Style::default().fg(Color::Yellow),
        )),
        QueryStatus::Done(count) => Line::from(Span::styled(
            format!(" {} result(s) returned", count),
            Style::default().fg(Color::Green),
        )),
        QueryStatus::Error(e) => Line::from(Span::styled(
            format!(" Error: {}", e),
            Style::default().fg(Color::Red),
        )),
    };
    frame.render_widget(status_line, status_area);

    let result_items: Vec<ListItem> = app
        .query_results
        .iter()
        .map(|msg| {
            let payload_str = match &msg.payload {
                MessagePayload::Json(v) => {
                    serde_json::to_string_pretty(v).unwrap_or_else(|_| format!("{}", v))
                }
                other => format!("{}", other),
            };
            let line = Line::from(vec![
                Span::styled(
                    &msg.key_expr,
                    Style::default()
                        .fg(Color::Cyan)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::raw(" | "),
                Span::styled(payload_str, Style::default().fg(Color::White)),
            ]);
            ListItem::new(line)
        })
        .collect();
    let result_count = result_items.len();
    let results = List::new(result_items).block(
        Block::default()
            .borders(Borders::ALL)
            .title(format!(" Results ({}) ", result_count)),
    );
    frame.render_widget(results, results_area);

    let history_items: Vec<ListItem> = app
        .query_history
        .iter()
        .rev()
        .take(4)
        .map(|q| {
            ListItem::new(Line::from(Span::styled(
                q,
                Style::default().fg(Color::DarkGray),
            )))
        })
        .collect();
    let history = List::new(history_items).block(
        Block::default()
            .borders(Borders::ALL)
            .title(" History (k:recall) "),
    );
    frame.render_widget(history, history_area);
}
