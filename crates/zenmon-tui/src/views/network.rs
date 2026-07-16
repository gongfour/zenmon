use crate::app::App;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};
use ratatui::Frame;
use zenmon_core::topology::build_topology;

pub fn render(app: &mut App, frame: &mut Frame, area: Rect) {
    let topo = build_topology(&app.nodes);
    let adjacency = zenmon_core::topology::to_adjacency_lines(&topo);

    let title = format!(
        " Network ({} nodes, {} links){} ",
        topo.nodes.len(),
        topo.edges.len(),
        if topo.partial { " · partial" } else { "" }
    );

    let mut lines: Vec<Line> = Vec::new();
    if topo.partial {
        lines.push(Line::from(Span::styled(
            "Partial view — relationships come from admin metadata; some nodes may not report sessions.",
            Style::default().fg(Color::Yellow),
        )));
        lines.push(Line::from(""));
    }
    for l in &adjacency {
        // Indented branch lines are edges; flush-left lines are source nodes.
        let style = if l.starts_with(' ') {
            Style::default().fg(Color::White)
        } else {
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD)
        };
        lines.push(Line::from(Span::styled(l.clone(), style)));
    }

    let para = Paragraph::new(lines)
        .block(Block::default().borders(Borders::ALL).title(title))
        .wrap(Wrap { trim: false })
        .scroll((app.network_scroll, 0));
    frame.render_widget(para, area);
}
