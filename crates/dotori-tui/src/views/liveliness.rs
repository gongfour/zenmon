use crate::app::App;
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::Frame;

pub fn render(app: &mut App, frame: &mut Frame, area: ratatui::layout::Rect) {
    let placeholder = Paragraph::new(Line::from(Span::styled(
        "Liveliness view — coming soon",
        Style::default().fg(Color::DarkGray),
    )))
    .block(Block::default().borders(Borders::ALL).title(" Liveliness "));
    frame.render_widget(placeholder, area);
}
