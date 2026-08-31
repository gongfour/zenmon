// The two spaces the app dispatches to, plus their shared formatting helpers.
pub mod fmt;
pub mod network;
pub mod traffic;

use crate::app::{empty_state_text, EmptyReason};
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Wrap};
use ratatui::Frame;

/// Render a contextual empty state (why it's empty + the next action) in `area`,
/// instead of leaving an ambiguous blank panel.
#[allow(dead_code)]
pub(crate) fn render_empty_state(frame: &mut Frame, area: Rect, reason: EmptyReason) {
    let (why, action) = empty_state_text(reason);
    let lines = vec![
        Line::from(""),
        Line::from(Span::styled(
            why,
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(Span::styled(action, Style::default().fg(Color::Gray))),
    ];
    let para = Paragraph::new(lines).wrap(Wrap { trim: true });
    frame.render_widget(para, area);
}
