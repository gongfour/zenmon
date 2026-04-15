use crate::app::App;
use dotori_core::types::{NodeInfo, NodeSources};
use ratatui::layout::Constraint;
use ratatui::style::{Color, Modifier, Style};
use ratatui::widgets::{Block, Borders, Cell, Row, Table};
use ratatui::Frame;
use std::time::{Duration, SystemTime};

const STALE_THRESHOLD: Duration = Duration::from_secs(30);
const BOTH_SOURCES: NodeSources = NodeSources::from_bits_retain(
    NodeSources::ADMIN.bits() | NodeSources::SCOUT.bits(),
);

pub fn render(app: &mut App, frame: &mut Frame, area: ratatui::layout::Rect) {
    app.list_rect = Some(area);
    app.list_first_item_row = area.y + 3;
    let visible = area.height.saturating_sub(4) as usize;
    app.list_scroll_offset = if visible > 0 && app.node_selected >= visible {
        app.node_selected + 1 - visible
    } else {
        0
    };

    let now = SystemTime::now();
    let header = Row::new(vec![
        Cell::from("ZID"),
        Cell::from("Kind"),
        Cell::from("Source"),
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
        .skip(app.list_scroll_offset)
        .take(visible)
        .map(|(i, node)| build_row(node, i == app.node_selected, now))
        .collect();

    let widths = [
        Constraint::Percentage(30),
        Constraint::Percentage(10),
        Constraint::Percentage(15),
        Constraint::Percentage(45),
    ];

    let (n_admin, n_scout, n_both) = count_by_source(&app.nodes);
    let scout_status = if app.scout_in_progress {
        " [scouting...]"
    } else {
        ""
    };
    let title = format!(
        " Nodes ({}) - admin:{} scout:{} both:{}{} - j/k:nav  s:scout ",
        app.nodes.len(),
        n_admin,
        n_scout,
        n_both,
        scout_status
    );

    let table = Table::new(rows, widths)
        .header(header)
        .block(Block::default().borders(Borders::ALL).title(title));

    frame.render_widget(table, area);
}

fn build_row<'a>(node: &'a NodeInfo, selected: bool, now: SystemTime) -> Row<'a> {
    let stale = node.is_scout_stale(now, STALE_THRESHOLD);

    let base_style = if selected {
        Style::default()
            .fg(Color::Black)
            .bg(Color::Cyan)
            .add_modifier(Modifier::BOLD)
    } else if stale {
        Style::default().fg(Color::DarkGray)
    } else {
        Style::default()
    };

    let kind_cell = if selected {
        Cell::from(node.kind.clone())
    } else {
        let kind_style = match node.kind.as_str() {
            "router" => Style::default().fg(Color::Green),
            "peer" => Style::default().fg(Color::Blue),
            "client" => Style::default().fg(Color::Gray),
            _ => Style::default(),
        };
        Cell::from(node.kind.clone()).style(kind_style)
    };

    let (source_text, source_color) = source_badge(node.sources, stale);
    let source_cell = if selected {
        Cell::from(source_text)
    } else {
        Cell::from(source_text).style(Style::default().fg(source_color))
    };

    Row::new(vec![
        Cell::from(node.zid.clone()),
        kind_cell,
        source_cell,
        Cell::from(node.locators.join(", ")),
    ])
    .style(base_style)
}

fn source_badge(sources: NodeSources, stale: bool) -> (String, Color) {
    if sources == BOTH_SOURCES {
        ("both".to_string(), Color::Cyan)
    } else if sources.contains(NodeSources::ADMIN) {
        ("admin".to_string(), Color::Green)
    } else if sources.contains(NodeSources::SCOUT) {
        if stale {
            ("scout-stale".to_string(), Color::DarkGray)
        } else {
            ("scout".to_string(), Color::Magenta)
        }
    } else {
        ("-".to_string(), Color::DarkGray)
    }
}

fn count_by_source(nodes: &[NodeInfo]) -> (usize, usize, usize) {
    let mut n_admin = 0;
    let mut n_scout = 0;
    let mut n_both = 0;
    for n in nodes {
        if n.sources == BOTH_SOURCES {
            n_both += 1;
        } else if n.sources.contains(NodeSources::ADMIN) {
            n_admin += 1;
        } else if n.sources.contains(NodeSources::SCOUT) {
            n_scout += 1;
        }
    }
    (n_admin, n_scout, n_both)
}
