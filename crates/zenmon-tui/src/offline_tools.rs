//! Commands belonging to the offline timeline, kept out of the live command palette.
use crate::{
    app::App,
    event::{AppEvent, EventHandler},
    recording::Recording,
};
use color_eyre::{eyre::eyre, Result};
use crossterm::event::{KeyCode, KeyModifiers};
use ratatui::{
    backend::CrosstermBackend,
    layout::Rect,
    widgets::{Block, Borders, Clear, Paragraph},
    Terminal,
};
use std::{
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::Duration,
};
use zenmon_core::{ErrorKind, ZenmonError};

type Screen = Terminal<CrosstermBackend<std::io::Stdout>>;
#[derive(Default)]
pub(crate) struct Tools {
    pub input: Option<String>,
    pub start: Option<usize>,
    pub end: Option<usize>,
    pub message: String,
}
impl Tools {
    pub fn render(&self, frame: &mut ratatui::Frame) {
        if let Some(input) = &self.input {
            let area = frame.area();
            let h = 9.min(area.height);
            let popup = Rect::new(area.x, area.y + area.height - h, area.width, h);
            frame.render_widget(Clear, popup);
            let marks = format!(
                "Marks: {} .. {} (inclusive message positions)",
                self.start
                    .map(|p| (p + 1).to_string())
                    .unwrap_or_else(|| "unset".into()),
                self.end
                    .map(|p| (p + 1).to_string())
                    .unwrap_or_else(|| "unset".into())
            );
            frame.render_widget(Paragraph::new(format!(
                "goto <RFC3339 | +10s from window start>\nfind / find-prev <text> (selected key; find-all / find-all-prev for all keys)\nchange / change-prev </field | attachment:/field> (selected key)\nmark-start | mark-end | export <path> | export-all <path>\n{marks}\n{}\n> {input}", self.message))
                .block(Block::default().borders(Borders::ALL).title(" Offline tools · Enter run · Esc close ")), popup);
        }
    }
    pub fn take_command(&mut self, event: &AppEvent) -> Option<String> {
        let input = self.input.as_mut()?;
        if let AppEvent::Key(k) = event {
            match k.code {
                KeyCode::Esc => self.input = None,
                KeyCode::Char('c') if k.modifiers.contains(KeyModifiers::CONTROL) => {
                    self.input = None
                }
                KeyCode::Enter => return self.input.take(),
                KeyCode::Backspace => {
                    input.pop();
                }
                KeyCode::Char(c)
                    if !k
                        .modifiers
                        .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
                {
                    input.push(c);
                }
                _ => {}
            }
        }
        None
    }
}
pub(crate) fn cancelled(flag: &AtomicBool) -> zenmon_core::Result<()> {
    if flag.load(Ordering::Relaxed) {
        Err(ZenmonError::new(
            ErrorKind::Interrupted,
            "operation cancelled",
        ))
    } else {
        Ok(())
    }
}

/// Run searches and exports away from the input loop. On cancellation we await
/// cleanup before returning the spool to the viewer.
pub(crate) async fn command(
    mut recording: Recording,
    tools: &mut Tools,
    text: String,
    position: usize,
    app: &mut App,
    terminal: &mut Screen,
    events: &mut EventHandler,
) -> Result<(Recording, Option<usize>)> {
    let text = text.trim();
    let (verb, argument) = text.split_once(char::is_whitespace).unwrap_or((text, ""));
    let argument = argument.trim();
    let immediate: Option<Result<Option<usize>>> = match verb {
        "goto" => Some(recording.seek_to(argument).map(Some)),
        "mark-start" => {
            tools.start = Some(position);
            Some(Ok(None))
        }
        "mark-end" => {
            tools.end = Some(position);
            Some(Ok(None))
        }
        "find" | "find-prev" | "find-all" | "find-all-prev" | "change" | "change-prev"
        | "export" | "export-all" => None,
        _ => Some(Err(eyre!("unknown command; press F2 for offline tools"))),
    };
    if let Some(result) = immediate {
        match result {
            Ok(target) => {
                tools.message = format!("{text}: done");
                app.set_toast(&tools.message);
                return Ok((recording, target));
            }
            Err(e) => {
                tools.message = e.to_string();
                app.set_error_toast(&tools.message);
                return Ok((recording, None));
            }
        }
    }
    let selected = app.selected_topic_key();
    let all = verb.contains("all");
    let export = verb.starts_with("export");
    let pointer = verb.starts_with("change");
    let forward = !verb.ends_with("prev");
    // Explicit -all is required to avoid silently searching/exporting all keys
    // when a branch rather than a leaf happens to be selected.
    if !all && selected.is_none() {
        tools.message = "Select a concrete key, or use find-all/export-all".into();
        app.set_error_toast(&tools.message);
        return Ok((recording, None));
    }
    if argument.is_empty() {
        tools.message = "This command needs an argument".into();
        app.set_error_toast(&tools.message);
        return Ok((recording, None));
    }
    let range = tools.start.zip(tools.end);
    if export && range.is_none() {
        tools.message = "Set mark-start and mark-end before exporting".into();
        app.set_error_toast(&tools.message);
        return Ok((recording, None));
    }
    let key = if all { None } else { selected };
    let argument = argument.to_string();
    let cancel = Arc::new(AtomicBool::new(false));
    let worker_cancel = cancel.clone();
    let mut worker = tokio::task::spawn_blocking(move || {
        let result: Result<(Option<usize>, String)> = (|| {
            if export {
                let (start, end) = range.unwrap();
                let count = recording.export_range(
                    &PathBuf::from(&argument),
                    start,
                    end,
                    key.as_deref(),
                    || cancelled(&worker_cancel),
                )?;
                Ok((None, format!("Exported {count} records to {argument}")))
            } else {
                let found = recording.find(
                    position,
                    key.as_deref(),
                    &argument,
                    forward,
                    pointer,
                    || Ok(cancelled(&worker_cancel)?),
                )?;
                Ok((
                    found,
                    found
                        .map(|p| format!("Matched message {}", p + 1))
                        .unwrap_or_else(|| "No further match in loaded window".into()),
                ))
            }
        })();
        (recording, result)
    });
    loop {
        tokio::select! {
            result = &mut worker => {
                let (recording, result) = result?;
                return Ok(match result {
                    Ok((target, message)) => { tools.message = message; app.set_toast(&tools.message); (recording, target) }
                    Err(e) => { tools.message = e.to_string(); app.set_error_toast(&tools.message); (recording, None) }
                });
            }
            event = events.next() => {
                match event {
                    Ok(AppEvent::Key(k)) if k.code == KeyCode::Esc || (k.code == KeyCode::Char('c') && k.modifiers.contains(KeyModifiers::CONTROL)) => { cancel.store(true, Ordering::Relaxed); }
                    Err(_) => { cancel.store(true, Ordering::Relaxed); }
                    _ => {}
                }
            }
            _ = tokio::time::sleep(Duration::from_millis(100)) => {}
        }
        if terminal
            .draw(|frame| {
                app.render(frame);
                let area = frame.area();
                let popup = Rect::new(
                    area.x,
                    area.y + area.height.saturating_sub(3),
                    area.width,
                    area.height.min(3),
                );
                frame.render_widget(Clear, popup);
                frame.render_widget(
                    Paragraph::new(if cancel.load(Ordering::Relaxed) {
                        "Cancelling…"
                    } else {
                        "Working on loaded recording… Esc / Ctrl+C cancel"
                    })
                    .block(Block::default().borders(Borders::ALL)),
                    popup,
                );
            })
            .is_err()
        {
            cancel.store(true, Ordering::Relaxed);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::KeyEvent;
    #[test]
    fn input_cancellation_does_not_submit_and_unicode_backspace_is_safe() {
        let mut tools = Tools {
            input: Some("find 온도".into()),
            ..Tools::default()
        };
        let key = |code, mods| AppEvent::Key(KeyEvent::new(code, mods));
        assert!(tools
            .take_command(&key(KeyCode::Backspace, KeyModifiers::NONE))
            .is_none());
        assert_eq!(tools.input.as_deref(), Some("find 온"));
        assert!(tools
            .take_command(&key(KeyCode::Char('c'), KeyModifiers::CONTROL))
            .is_none());
        assert!(tools.input.is_none());
        tools.input = Some("goto +1s".into());
        assert_eq!(
            tools
                .take_command(&key(KeyCode::Enter, KeyModifiers::NONE))
                .as_deref(),
            Some("goto +1s")
        );
        assert!(tools.input.is_none());
    }
}
