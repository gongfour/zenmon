use crate::app::App;
use ratatui::layout::Constraint;
use ratatui::style::{Color, Modifier, Style};
use ratatui::widgets::{Block, Borders, Cell, Row, Table};
use ratatui::Frame;

pub fn render(app: &App, frame: &mut Frame, area: ratatui::layout::Rect) {
    let header = Row::new(vec![
        Cell::from("ZID"),
        Cell::from("Kind"),
        Cell::from("Locators"),
    ])
    .style(
        Style::default()
            .fg(Color::Yellow)
            .add_modifier(Modifier::BOLD),
    )
    .bottom_margin(1);

    let rows: Vec<Row> = app
        .nodes
        .iter()
        .enumerate()
        .map(|(i, node)| {
            let style = if i == app.node_selected {
                Style::default()
                    .fg(Color::Black)
                    .bg(Color::Cyan)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            };
            let kind_style = match node.kind.as_str() {
                "router" => Style::default().fg(Color::Green),
                "peer" => Style::default().fg(Color::Blue),
                "client" => Style::default().fg(Color::Gray),
                _ => Style::default(),
            };
            Row::new(vec![
                Cell::from(node.zid.clone()),
                Cell::from(node.kind.clone()).style(kind_style),
                Cell::from(node.locators.join(", ")),
            ])
            .style(style)
        })
        .collect();

    let widths = [
        Constraint::Percentage(40),
        Constraint::Percentage(15),
        Constraint::Percentage(45),
    ];

    let table = Table::new(rows, widths)
        .header(header)
        .block(
            Block::default().borders(Borders::ALL).title(format!(
                " Nodes ({}) — j/k:navigate ",
                app.nodes.len()
            )),
        )
        .row_highlight_style(Style::default().bg(Color::DarkGray));

    frame.render_widget(table, area);
}
