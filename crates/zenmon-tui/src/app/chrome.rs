//! Everything painted around the active space: the header, the hint bar, and
//! the modal overlays (help, doctor, scout-port, command palette).

use super::*;
use crate::views;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Wrap};
use ratatui::Frame;
use zenmon_core::doctor::CheckStatus;

/// Human label for a scout/multicast port. Ports in the Zenoh domain range
/// (7446..=7546, i.e. domains 0..=100) are shown as their domain id; anything
/// else is a custom port and is labeled as a port, not a domain — so an
/// arbitrary port below 7446 isn't misreported as "domain 0".
pub(crate) fn domain_port_label(port: u16) -> String {
    if (7446..=7546).contains(&port) {
        format!("domain {} (port {})", port - 7446, port)
    } else {
        format!("port {} (custom)", port)
    }
}

impl App {
    pub fn render(&mut self, frame: &mut Frame) {
        // Expire stale toasts each frame.
        let toast_expired = self
            .toast
            .as_ref()
            .map(|(_, t)| t.elapsed().as_secs() >= 2)
            .unwrap_or(true);
        if toast_expired {
            self.toast = None;
        }

        let compact = frame.area().width < TWO_PANE_MIN_WIDTH;
        let header_h = if compact { 1 } else { 2 };
        let [header_area, body_area, hint_area] = Layout::vertical([
            Constraint::Length(header_h),
            Constraint::Fill(1),
            Constraint::Length(1),
        ])
        .areas(frame.area());

        self.render_header(frame, header_area, compact);
        match self.space {
            Space::Traffic => views::traffic::render(self, frame, body_area),
            Space::Network => views::network::render(self, frame, body_area),
        }
        self.render_hint_bar(frame, hint_area);

        if self.overlay == Overlay::Help {
            self.render_help_overlay(frame, body_area);
        }
        if self.overlay == Overlay::Doctor {
            self.render_doctor_overlay(frame, body_area);
        }
        if self.overlay == Overlay::Palette {
            self.render_palette_overlay(frame, body_area);
        }
        if self.overlay == Overlay::ScoutPort {
            self.render_scout_port_modal(frame, body_area);
        }
        if self.overlay == Overlay::PlotPicker {
            self.render_plot_picker(frame, body_area);
        }
        if self.overlay == Overlay::Publish {
            self.render_publish_editor(frame, body_area);
        }
        if self.overlay == Overlay::ProfileSave {
            self.render_profile_save(frame, body_area);
        }
        if self.overlay == Overlay::ProfileLoad {
            self.render_profile_load(frame, body_area);
        }
    }

    fn render_header(&mut self, frame: &mut Frame, area: Rect, compact: bool) {
        let conn_text = match &self.connection_state {
            ConnectionState::Connected(zid) => {
                format!("Connected zid:{}", &zid[..zid.len().min(16)])
            }
            ConnectionState::Connecting => "Connecting...".to_string(),
            ConnectionState::Disconnected(reason) => format!("Disconnected: {}", reason),
        };

        // Health dot: reflects the last `doctor` run's overall status when a
        // report exists, otherwise falls back to socket connection state.
        let (dot_glyph, dot_color, health_label) = match &self.doctor_report {
            Some(report) => match report.status {
                CheckStatus::Pass => ("●", Color::Green, "OK".to_string()),
                CheckStatus::Warn => {
                    let n = report
                        .checks
                        .iter()
                        .filter(|c| c.status != CheckStatus::Pass)
                        .count();
                    ("⚠", Color::Yellow, format!("{} warn", n))
                }
                CheckStatus::Fail => {
                    let n = report
                        .checks
                        .iter()
                        .filter(|c| c.status == CheckStatus::Fail)
                        .count();
                    ("✖", Color::Red, format!("{} fail", n))
                }
            },
            None => match &self.connection_state {
                ConnectionState::Connected(_) => ("●", Color::Green, "OK".to_string()),
                ConnectionState::Connecting => ("●", Color::Yellow, "connecting".to_string()),
                ConnectionState::Disconnected(_) => ("●", Color::Red, "offline".to_string()),
            },
        };

        let [line0, line1] = Layout::vertical([
            Constraint::Length(1),
            Constraint::Length(if compact { 0 } else { 1 }),
        ])
        .areas(area);

        // Line 0: health dot + label, connection string, then the two space tabs.
        let mut spans = vec![
            Span::styled(format!("{} ", dot_glyph), Style::default().fg(dot_color)),
            Span::styled(
                format!("{}  ", health_label),
                Style::default().fg(dot_color),
            ),
        ];
        if self.doctor_running {
            spans.push(Span::styled(
                "checking…  ",
                Style::default().fg(Color::DarkGray),
            ));
        }
        spans.push(Span::styled(conn_text, Style::default().fg(Color::Gray)));
        spans.push(Span::raw("  "));
        // Guard 3 of 4: a session that can write says so on every frame. There
        // must never be a moment where the user does not know which kind of
        // session they are looking at.
        if self.allow_publish {
            spans.push(Span::styled(
                " WRITE ",
                Style::default()
                    .fg(Color::Black)
                    .bg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            ));
            spans.push(Span::raw("  "));
        }
        let prefix_width: u16 = spans.iter().map(|s| s.content.chars().count() as u16).sum();
        frame.render_widget(Paragraph::new(Line::from(spans)), line0);

        // Space tabs — record each label rect for mouse hit-testing.
        let mut x = line0.x + prefix_width;
        for (i, title) in SPACE_TITLES.iter().enumerate() {
            let label = format!("[{}]", title);
            let label_width = label.chars().count() as u16;
            if x + label_width > line0.x + line0.width {
                self.space_tab_rects[i] = None;
                continue;
            }
            let rect = Rect::new(x, line0.y, label_width, 1);
            self.space_tab_rects[i] = Some(rect);
            let style = if i == self.space.index() {
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD | Modifier::UNDERLINED)
            } else {
                Style::default().fg(Color::White)
            };
            frame.render_widget(Paragraph::new(Span::styled(label, style)), rect);
            x += label_width + 1;
        }

        // Line 1 (skipped when compact): counts summary.
        if !compact {
            let mut counts = format!(
                "{} sessions · {} services · {} keys · {:.0} msg/s · {}",
                self.nodes.len(),
                self.liveliness_tokens.len(),
                self.key_tree.len(),
                self.total_hz,
                format_bytes_per_sec(self.total_bytes_per_sec()),
            );
            // Say so when the session has forgotten keys. A monitoring tool
            // that quietly drops what it observed is worse than one that
            // admits the ceiling.
            if self.keys_aged_out > 0 {
                counts.push_str(&format!(" · {} aged out", self.keys_aged_out));
            }
            frame.render_widget(
                Paragraph::new(Span::styled(counts, Style::default().fg(Color::DarkGray))),
                line1,
            );
        }
    }

    /// The field picker: every numeric field in the selected key's latest
    /// payload, with its current value so the choice is informed.
    fn render_plot_picker(&self, frame: &mut Frame, content_area: Rect) {
        let fields = self.plottable_fields();
        let width = 46.min(content_area.width.saturating_sub(2));
        let height = (fields.len() as u16 + 4).min(content_area.height.saturating_sub(2));
        if width < 20 || height < 5 {
            return;
        }
        let popup = Rect::new(
            content_area.x + (content_area.width - width) / 2,
            content_area.y + (content_area.height - height) / 2,
            width,
            height,
        );

        frame.render_widget(Clear, popup);
        let block = Block::default()
            .borders(Borders::ALL)
            .title(" Plot field ")
            .style(Style::default().fg(Color::White).bg(Color::Black));
        let inner = block.inner(popup);
        frame.render_widget(block, popup);

        let latest = self
            .selected_topic_key()
            .and_then(|k| self.history.get(&k).and_then(|h| h.latest()).cloned());

        let mut lines: Vec<Line> = fields
            .iter()
            .enumerate()
            .map(|(i, pointer)| {
                let value = latest
                    .as_ref()
                    .and_then(|e| e.view.pointer(pointer))
                    .and_then(serde_json::Value::as_f64)
                    .map(crate::plot::format_value)
                    .unwrap_or_default();
                let style = if i == self.plot_picker_selected {
                    Style::default()
                        .fg(Color::Black)
                        .bg(Color::Cyan)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(Color::White)
                };
                Line::from(Span::styled(
                    format!(" {:<24} {:>14} ", pointer.trim_start_matches('/'), value),
                    style,
                ))
            })
            .collect();
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            "[Enter] plot   [Esc] cancel   [P] clear",
            Style::default().fg(Color::DarkGray),
        )));

        frame.render_widget(Paragraph::new(lines), inner);
    }

    /// The publish editor.
    fn render_publish_editor(&self, frame: &mut Frame, content_area: Rect) {
        let width = 72.min(content_area.width.saturating_sub(2));
        let height = 12.min(content_area.height.saturating_sub(2));
        if width < 32 || height < 8 {
            return;
        }
        let popup = Rect::new(
            content_area.x + (content_area.width - width) / 2,
            content_area.y + (content_area.height - height) / 2,
            width,
            height,
        );

        frame.render_widget(Clear, popup);
        let block = Block::default()
            .borders(Borders::ALL)
            .title(" Publish ")
            .border_style(Style::default().fg(Color::Yellow))
            .style(Style::default().fg(Color::White).bg(Color::Black));
        let inner = block.inner(popup);
        frame.render_widget(block, popup);

        let field = |label: &str, value: &str, active: bool| {
            let marker = if active { "▸ " } else { "  " };
            let style = if active {
                Style::default()
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::Gray)
            };
            Line::from(vec![
                Span::styled(
                    format!("{marker}{label:<9}"),
                    Style::default().fg(Color::DarkGray),
                ),
                Span::styled(format!("{value}{}", if active { "_" } else { "" }), style),
            ])
        };

        let mut lines = vec![
            field(
                "key",
                &self.publish_key,
                self.publish_field == PublishField::Key,
            ),
            field(
                "payload",
                &self.publish_payload,
                self.publish_field == PublishField::Payload,
            ),
            Line::from(""),
        ];

        // Guard 4 of 4: name the target again right before the send key, so
        // committing is never a blind action on whatever was prefilled.
        match &self.publish_result {
            Some(Ok(key)) => lines.push(Line::from(Span::styled(
                format!("  sent to {key}"),
                Style::default().fg(Color::Green),
            ))),
            Some(Err(reason)) => lines.push(Line::from(Span::styled(
                format!("  {reason}"),
                Style::default().fg(Color::Red),
            ))),
            None => lines.push(Line::from(vec![
                Span::styled("  will write to  ", Style::default().fg(Color::DarkGray)),
                Span::styled(
                    if self.publish_key.trim().is_empty() {
                        "(no key)".to_string()
                    } else {
                        self.publish_key.trim().to_string()
                    },
                    Style::default()
                        .fg(Color::Yellow)
                        .add_modifier(Modifier::BOLD),
                ),
            ])),
        }

        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            "  [Ctrl+Enter] send   [Tab] field   [Esc] cancel",
            Style::default().fg(Color::DarkGray),
        )));

        frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), inner);
    }

    /// Centre a popup of `w` x `h` inside `area`, or `None` if it will not fit.
    fn popup_rect(area: Rect, w: u16, h: u16, min_w: u16, min_h: u16) -> Option<Rect> {
        let width = w.min(area.width.saturating_sub(2));
        let height = h.min(area.height.saturating_sub(2));
        if width < min_w || height < min_h {
            return None;
        }
        Some(Rect::new(
            area.x + (area.width - width) / 2,
            area.y + (area.height - height) / 2,
            width,
            height,
        ))
    }

    fn render_profile_save(&self, frame: &mut Frame, content_area: Rect) {
        let Some(popup) = Self::popup_rect(content_area, 60, 7, 28, 5) else {
            return;
        };
        frame.render_widget(Clear, popup);
        let block = Block::default()
            .borders(Borders::ALL)
            .title(" Save this view ")
            .style(Style::default().fg(Color::White).bg(Color::Black));
        let inner = block.inner(popup);
        frame.render_widget(block, popup);

        // Say what is being captured, so "view" is not a word the user has to
        // guess the meaning of.
        let plotted = self.plot_field.len();
        let summary = format!(
            "  filter {}  ·  {} open  ·  {} plotted",
            if self.topic_filter.is_empty() {
                "(none)".into()
            } else {
                format!("'{}'", self.topic_filter)
            },
            self.tree_expanded.len(),
            plotted,
        );

        frame.render_widget(
            Paragraph::new(vec![
                Line::from(vec![
                    Span::styled("  name  ", Style::default().fg(Color::DarkGray)),
                    Span::styled(
                        format!("{}_", self.profile_name_input),
                        Style::default()
                            .fg(Color::White)
                            .add_modifier(Modifier::BOLD),
                    ),
                ]),
                Line::from(""),
                Line::from(Span::styled(summary, Style::default().fg(Color::DarkGray))),
                Line::from(""),
                Line::from(Span::styled(
                    "  [Enter] save   [Esc] cancel",
                    Style::default().fg(Color::DarkGray),
                )),
            ]),
            inner,
        );
    }

    fn render_profile_load(&self, frame: &mut Frame, content_area: Rect) {
        let rows = self.profiles.profiles.len() as u16;
        let Some(popup) = Self::popup_rect(content_area, 60, rows + 4, 28, 5) else {
            return;
        };
        frame.render_widget(Clear, popup);
        let block = Block::default()
            .borders(Borders::ALL)
            .title(" Load a saved view ")
            .style(Style::default().fg(Color::White).bg(Color::Black));
        let inner = block.inner(popup);
        frame.render_widget(block, popup);

        let mut lines: Vec<Line> = self
            .profiles
            .profiles
            .iter()
            .enumerate()
            .map(|(i, p)| {
                let style = if i == self.profile_selected {
                    Style::default()
                        .fg(Color::Black)
                        .bg(Color::Cyan)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(Color::White)
                };
                let detail = format!(
                    "{} open, {} plotted{}",
                    p.expanded.len(),
                    p.plot_fields.len(),
                    if p.filter.is_empty() {
                        String::new()
                    } else {
                        format!(", /{}", p.filter)
                    }
                );
                Line::from(Span::styled(
                    format!(" {:<20} {:>30} ", p.name, detail),
                    style,
                ))
            })
            .collect();
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            " [Enter] load   [Esc] cancel",
            Style::default().fg(Color::DarkGray),
        )));

        frame.render_widget(Paragraph::new(lines), inner);
    }

    fn render_hint_bar(&self, frame: &mut Frame, area: Rect) {
        let hint = match self.space {
            Space::Traffic => {
                "Tab space  j/k move  h/l fold  / filter  D diff  p plot  L live  Q query  y copy  : cmds  ? help  q quit"
            }
            Space::Network => {
                "Tab space  j/k move  Enter drill  s scout  y copy  : cmds  d doctor  ? help  q quit"
            }
        };
        frame.render_widget(
            Paragraph::new(Span::styled(hint, Style::default().fg(Color::DarkGray))),
            area,
        );
    }

    fn render_help_overlay(&self, frame: &mut Frame, content_area: Rect) {
        let width = 60.min(content_area.width.saturating_sub(2));
        let height = 16.min(content_area.height.saturating_sub(2));
        if width < 24 || height < 6 {
            return;
        }
        let x = content_area.x + (content_area.width - width) / 2;
        let y = content_area.y + (content_area.height - height) / 2;
        let popup = Rect::new(x, y, width, height);

        frame.render_widget(Clear, popup);
        let block = Block::default()
            .borders(Borders::ALL)
            .title(" Keybindings ")
            .style(Style::default().fg(Color::White).bg(Color::Black));
        let inner = block.inner(popup);
        frame.render_widget(block, popup);

        // Generated from the same command table the palette uses, so help and
        // the `:` palette can never drift apart.
        let mut lines: Vec<Line> = palette_commands()
            .iter()
            .map(|cmd| {
                Line::from(vec![
                    Span::styled(
                        format!("  {:<6}", cmd.key_hint),
                        Style::default().fg(Color::Yellow),
                    ),
                    Span::styled(cmd.label.to_string(), Style::default().fg(Color::White)),
                ])
            })
            .collect();
        // The tree keys have no palette entry (they act on the cursor, not the
        // app), so they are listed here explicitly.
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            "  Traffic tree",
            Style::default().fg(Color::Cyan),
        )));
        for (keys, what) in [
            ("h/←", "collapse, or go to the parent"),
            ("l/→", "expand, descend, or open a folded group"),
            ("z", "toggle the branch under the cursor"),
            ("/", "filter (searches inside folded groups)"),
            ("E/C", "expand / collapse everything"),
            ("D", "highlight what changed since the previous message"),
            ("p/P", "plot a numeric field / clear the plot"),
            ("space", "freeze the payload and history being read"),
        ] {
            lines.push(Line::from(vec![
                Span::styled(format!("  {:<6}", keys), Style::default().fg(Color::Yellow)),
                Span::styled(what.to_string(), Style::default().fg(Color::White)),
            ]));
        }
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            "`:` opens the command palette · j/k or ↑↓ to scroll · Esc/q/? to close",
            Style::default().fg(Color::DarkGray),
        )));

        let max_scroll = (lines.len() as u16).saturating_sub(inner.height);
        let scroll = self.help_scroll.min(max_scroll);
        let para = Paragraph::new(lines)
            .wrap(Wrap { trim: false })
            .scroll((scroll, 0));
        frame.render_widget(para, inner);
    }

    fn render_doctor_overlay(&self, frame: &mut Frame, content_area: Rect) {
        let width = 72.min(content_area.width.saturating_sub(2));
        let height = 20.min(content_area.height.saturating_sub(2));
        if width < 24 || height < 6 {
            return;
        }
        let x = content_area.x + (content_area.width - width) / 2;
        let y = content_area.y + (content_area.height - height) / 2;
        let popup = Rect::new(x, y, width, height);

        frame.render_widget(Clear, popup);
        let block = Block::default()
            .borders(Borders::ALL)
            .title(" Doctor ")
            .style(Style::default().fg(Color::White).bg(Color::Black));
        let inner = block.inner(popup);
        frame.render_widget(block, popup);

        let mut lines: Vec<Line> = Vec::new();
        match &self.doctor_report {
            None => {
                if self.doctor_running {
                    lines.push(Line::from(Span::styled(
                        "Running diagnostics…",
                        Style::default().fg(Color::Yellow),
                    )));
                } else {
                    lines.push(Line::from(Span::styled(
                        "No diagnostics yet.",
                        Style::default().fg(Color::DarkGray),
                    )));
                }
            }
            Some(report) => {
                if self.doctor_running {
                    lines.push(Line::from(Span::styled(
                        "Re-running diagnostics…",
                        Style::default().fg(Color::DarkGray),
                    )));
                }
                for check in &report.checks {
                    let (icon, color) = match check.status {
                        CheckStatus::Pass => ("✓", Color::Green),
                        CheckStatus::Warn => ("⚠", Color::Yellow),
                        CheckStatus::Fail => ("✖", Color::Red),
                    };
                    let mut spans = vec![
                        Span::styled(format!("{} ", icon), Style::default().fg(color)),
                        Span::styled(
                            format!("{:<12}", check.name),
                            Style::default().fg(Color::White),
                        ),
                        Span::styled(
                            format!("{:>6}ms  ", check.latency_ms),
                            Style::default().fg(Color::DarkGray),
                        ),
                    ];
                    if let Some(msg) = &check.message {
                        spans.push(Span::styled(msg.clone(), Style::default().fg(Color::Gray)));
                    }
                    lines.push(Line::from(spans));
                    if check.status != CheckStatus::Pass {
                        if let Some(hint) = &check.hint {
                            lines.push(Line::from(Span::styled(
                                format!("      hint: {}", hint),
                                Style::default().fg(Color::DarkGray),
                            )));
                        }
                    }
                }
            }
        }
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            "[r] re-run   [j/k] scroll   [Esc] close",
            Style::default().fg(Color::DarkGray),
        )));

        let max_scroll = (lines.len() as u16).saturating_sub(inner.height);
        let scroll = self.doctor_scroll.min(max_scroll);
        let para = Paragraph::new(lines)
            .wrap(Wrap { trim: false })
            .scroll((scroll, 0));
        frame.render_widget(para, inner);
    }

    fn render_scout_port_modal(&self, frame: &mut Frame, content_area: Rect) {
        let width = 58.min(content_area.width.saturating_sub(2));
        let height = 16.min(content_area.height.saturating_sub(2));
        if width < 20 || height < 8 {
            return;
        }
        let x = content_area.x + (content_area.width - width) / 2;
        let y = content_area.y + (content_area.height - height) / 2;
        let popup = Rect::new(x, y, width, height);

        frame.render_widget(Clear, popup);
        let block = Block::default()
            .borders(Borders::ALL)
            .title(" Domain Scan & Switch ")
            .style(
                Style::default()
                    .fg(Color::White)
                    .bg(Color::Black)
                    .add_modifier(Modifier::BOLD),
            );
        let inner = block.inner(popup);
        frame.render_widget(block, popup);

        let [current_row, input_row, _gap, list_area, hint_row] = Layout::vertical([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Fill(1),
            Constraint::Length(1),
        ])
        .areas(inner);

        let current_text = match self.scout_port_current {
            Some(p) => format!("Current: {}", domain_port_label(p)),
            None => "Current: domain 0 (port 7446, default)".to_string(),
        };
        frame.render_widget(
            Paragraph::new(current_text).style(Style::default().fg(Color::Gray)),
            current_row,
        );

        let input_text = if self.scout_port_input.is_empty() {
            "Custom port: _".to_string()
        } else {
            format!("Custom port: {}_", self.scout_port_input)
        };
        frame.render_widget(
            Paragraph::new(input_text).style(Style::default().fg(Color::Cyan)),
            input_row,
        );

        if self.port_scan_in_progress {
            frame.render_widget(
                Paragraph::new("Scanning domains 0-100 (ports 7446-7546) ...")
                    .style(Style::default().fg(Color::Yellow)),
                list_area,
            );
        } else {
            let hits: Vec<&PortScoutResult> = self
                .port_scan_results
                .iter()
                .filter(|r| !r.nodes.is_empty())
                .collect();
            if hits.is_empty() && self.port_scan_results.is_empty() {
                frame.render_widget(
                    Paragraph::new("Press 's' to scan domains 0-100 for nodes")
                        .style(Style::default().fg(Color::DarkGray)),
                    list_area,
                );
            } else if hits.is_empty() {
                frame.render_widget(
                    Paragraph::new("No nodes found in domains 0-100")
                        .style(Style::default().fg(Color::Red)),
                    list_area,
                );
            } else {
                let lines: Vec<Line> = hits
                    .iter()
                    .enumerate()
                    .map(|(i, r)| {
                        let selected = i == self.port_scan_selected;
                        let marker = if selected { "> " } else { "  " };
                        let is_self = matches!(
                            &self.connection_state,
                            ConnectionState::Connected(zid) if r.nodes.iter().any(|n| n.zid == *zid)
                        );
                        let base_text = format!(
                            "{}{}  {} node(s)",
                            marker,
                            domain_port_label(r.port),
                            r.nodes.len()
                        );
                        let mut spans = vec![Span::styled(
                            base_text,
                            if selected {
                                Style::default()
                                    .fg(Color::Black)
                                    .bg(Color::Cyan)
                                    .add_modifier(Modifier::BOLD)
                            } else {
                                Style::default().fg(Color::White)
                            },
                        )];
                        if is_self {
                            spans.push(Span::styled(
                                "  (self)",
                                Style::default()
                                    .fg(Color::Green)
                                    .add_modifier(Modifier::BOLD),
                            ));
                        }
                        Line::from(spans)
                    })
                    .collect();
                frame.render_widget(Paragraph::new(lines), list_area);
            }
        }

        frame.render_widget(
            Paragraph::new(" s:scan domains  Enter:switch  jk/↑↓:select  Esc:close ")
                .style(Style::default().fg(Color::DarkGray)),
            hint_row,
        );
    }

    fn render_palette_overlay(&self, frame: &mut Frame, content_area: Rect) {
        let width = 54.min(content_area.width.saturating_sub(2));
        let height = 16.min(content_area.height.saturating_sub(2));
        if width < 24 || height < 6 {
            return;
        }
        let x = content_area.x + (content_area.width - width) / 2;
        let y = content_area.y + (content_area.height - height) / 2;
        let popup = Rect::new(x, y, width, height);

        frame.render_widget(Clear, popup);
        let block = Block::default()
            .borders(Borders::ALL)
            .title(" Commands ")
            .style(Style::default().fg(Color::White).bg(Color::Black));
        let inner = block.inner(popup);
        frame.render_widget(block, popup);

        let [input_row, list_area, hint_row] = Layout::vertical([
            Constraint::Length(1),
            Constraint::Fill(1),
            Constraint::Length(1),
        ])
        .areas(inner);

        frame.render_widget(
            Paragraph::new(format!("> {}▏", self.palette_input))
                .style(Style::default().fg(Color::Cyan)),
            input_row,
        );

        let commands = palette_commands();
        let filtered = self.filtered_palette_commands();
        let label_width = list_area.width.saturating_sub(2) as usize;
        let lines: Vec<Line> = filtered
            .iter()
            .enumerate()
            .map(|(row, &idx)| {
                let cmd = &commands[idx];
                let selected = row == self.palette_selected;
                let marker = if selected { "> " } else { "  " };
                let hint_w = cmd.key_hint.chars().count();
                let label_w = cmd.label.chars().count();
                let pad = label_width
                    .saturating_sub(2)
                    .saturating_sub(label_w)
                    .saturating_sub(hint_w);
                let label_style = if selected {
                    Style::default()
                        .fg(Color::Black)
                        .bg(Color::Cyan)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(Color::White)
                };
                Line::from(vec![
                    Span::styled(format!("{}{}", marker, cmd.label), label_style),
                    Span::raw(" ".repeat(pad)),
                    Span::styled(
                        cmd.key_hint.to_string(),
                        Style::default().fg(Color::DarkGray),
                    ),
                ])
            })
            .collect();
        frame.render_widget(Paragraph::new(lines), list_area);

        frame.render_widget(
            Paragraph::new("[Enter] run   [Esc] close").style(Style::default().fg(Color::DarkGray)),
            hint_row,
        );
    }
}
