use crate::app::App;
use ratatui::layout::{Constraint, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, Paragraph};
use ratatui::Frame;

pub fn render(app: &App, frame: &mut Frame, area: ratatui::layout::Rect) {
    let [filter_area, list_area] =
        Layout::vertical([Constraint::Length(3), Constraint::Fill(1)]).areas(area);

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

    let filtered = app.filtered_topics();
    let items: Vec<ListItem> = filtered
        .iter()
        .enumerate()
        .map(|(i, topic)| {
            let style = if i == app.topic_selected {
                Style::default()
                    .fg(Color::Black)
                    .bg(Color::Cyan)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::White)
            };
            let prefix = if i == app.topic_selected {
                ">> "
            } else {
                "   "
            };
            let line = Line::from(vec![
                Span::raw(prefix),
                Span::styled(&topic.key_expr, style),
            ]);
            ListItem::new(line)
        })
        .collect();

    let list = List::new(items).block(
        Block::default()
            .borders(Borders::ALL)
            .title(format!(
                " Topics ({}) — j/k:navigate  Enter:subscribe ",
                filtered.len()
            )),
    );
    frame.render_widget(list, list_area);
}
