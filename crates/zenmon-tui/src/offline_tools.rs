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
    history: Vec<String>,
    history_index: Option<usize>,
    draft: String,
    fields: Vec<String>,
    completion: Option<(Vec<String>, usize)>,
    last_search: Option<(String, Option<String>)>,
}
impl Tools {
    pub fn open(&mut self, app: &App) {
        self.input = Some(String::new());
        self.history_index = None;
        self.draft.clear();
        self.completion = None;
        self.fields.clear();
        if let Some((msg, _)) = app
            .selected_topic_key()
            .and_then(|k| app.topic_latest.get(&k))
        {
            for (prefix, payload) in [
                ("", Some(&msg.payload)),
                ("attachment:", msg.attachment.as_ref()),
            ] {
                if let Some(payload) = payload.filter(|p| p.len() <= 64 * 1024) {
                    collect_fields(&payload.to_view(), prefix, 0, &mut self.fields);
                }
            }
        }
    }

    pub fn repeat(&self, backwards: bool) -> Option<(String, Option<String>)> {
        let (command, key) = self.last_search.as_ref()?;
        let (verb, arg) = command.split_once(' ')?;
        let verb = verb.trim_end_matches("-prev");
        Some((
            format!("{verb}{} {arg}", if backwards { "-prev" } else { "" }),
            key.clone(),
        ))
    }

    fn candidates(&self, input: &str) -> Vec<String> {
        if let Some((verb, prefix)) = input.split_once(' ') {
            if matches!(verb, "change" | "change-prev") {
                return self
                    .fields
                    .iter()
                    .filter(|p| p.starts_with(prefix))
                    .map(|p| format!("{verb} {p}"))
                    .collect();
            }
            return Vec::new();
        }
        [
            "goto",
            "find",
            "find-prev",
            "find-all",
            "find-all-prev",
            "change",
            "change-prev",
            "mark-start",
            "mark-end",
            "export",
            "export-all",
        ]
        .into_iter()
        .filter(|s| s.starts_with(input))
        .map(|s| {
            if s.starts_with("mark-") {
                s.into()
            } else {
                format!("{s} ")
            }
        })
        .collect()
    }

    pub fn render(&self, frame: &mut ratatui::Frame) {
        if let Some(input) = &self.input {
            let area = frame.area();
            let h = 10.min(area.height);
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
            let suggestions = self
                .completion
                .as_ref()
                .map(|(choices, _)| choices.clone())
                .unwrap_or_else(|| self.candidates(input))
                .into_iter()
                .take(3)
                .collect::<Vec<_>>()
                .join(" | ");
            frame.render_widget(Paragraph::new(format!(
                "goto <RFC3339 | +10s from window start>\nfind / find-prev <text> (selected key; find-all / find-all-prev for all keys)\nchange / change-prev </field | attachment:/field> (selected key)\nmark-start | mark-end | export <path> | export-all <path>\n{marks}\n{}\nTab: {suggestions}\n> {input}", self.message))
                .block(Block::default().borders(Borders::ALL).title(" Offline tools · Tab complete · ↑↓ history · Enter run · Esc close ")), popup);
        }
    }
    pub fn take_command(&mut self, event: &AppEvent) -> Option<String> {
        let AppEvent::Key(k) = event else {
            return None;
        };
        self.input.as_ref()?;
        if !matches!(k.code, KeyCode::Tab | KeyCode::BackTab) {
            self.completion = None;
        }
        match k.code {
            KeyCode::Esc => self.input = None,
            KeyCode::Char('c') if k.modifiers.contains(KeyModifiers::CONTROL) => self.input = None,
            KeyCode::Enter => {
                let text = self.input.take()?;
                if !text.trim().is_empty() && self.history.last() != Some(&text) {
                    self.history.push(text.clone());
                    if self.history.len() > 100 {
                        self.history.remove(0);
                    }
                }
                self.history_index = None;
                return Some(text);
            }
            KeyCode::Up => {
                if !self.history.is_empty() {
                    let i = match self.history_index {
                        Some(i) => i.saturating_sub(1),
                        None => {
                            self.draft = self.input.clone().unwrap();
                            self.history.len() - 1
                        }
                    };
                    self.history_index = Some(i);
                    self.input = Some(self.history[i].clone());
                }
            }
            KeyCode::Down => {
                if let Some(i) = self.history_index {
                    if i + 1 < self.history.len() {
                        self.history_index = Some(i + 1);
                        self.input = Some(self.history[i + 1].clone());
                    } else {
                        self.history_index = None;
                        self.input = Some(self.draft.clone());
                    }
                }
            }
            KeyCode::Tab | KeyCode::BackTab => {
                let reverse =
                    k.code == KeyCode::BackTab || k.modifiers.contains(KeyModifiers::SHIFT);
                if let Some((choices, i)) = &mut self.completion {
                    *i = if reverse {
                        (*i + choices.len() - 1) % choices.len()
                    } else {
                        (*i + 1) % choices.len()
                    };
                    self.input = Some(choices[*i].clone());
                } else {
                    let choices = self.candidates(self.input.as_ref().unwrap());
                    if !choices.is_empty() {
                        let i = if reverse { choices.len() - 1 } else { 0 };
                        self.input = Some(choices[i].clone());
                        self.completion = Some((choices, i));
                    }
                }
            }
            KeyCode::Backspace => {
                self.input.as_mut().unwrap().pop();
            }
            KeyCode::Char(c)
                if !k
                    .modifiers
                    .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
            {
                self.input.as_mut().unwrap().push(c);
            }
            _ => {}
        }
        None
    }
}

// Suggestions are deliberately bounded; manual JSON pointers remain unrestricted.
fn collect_fields(value: &serde_json::Value, prefix: &str, depth: usize, out: &mut Vec<String>) {
    if depth >= 16 || out.len() >= 256 {
        return;
    }
    let mut visit = |name: String, child: &serde_json::Value| {
        if out.len() >= 256 {
            return;
        }
        let path = format!("{prefix}/{}", name.replace('~', "~0").replace('/', "~1"));
        out.push(path.clone());
        collect_fields(child, &path, depth + 1, out);
    };
    match value {
        serde_json::Value::Object(map) => {
            for (name, child) in map.iter().take(256) {
                visit(name.clone(), child);
            }
        }
        serde_json::Value::Array(items) => {
            for (i, child) in items.iter().take(256).enumerate() {
                visit(i.to_string(), child);
            }
        }
        _ => {}
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
    let search = (!export).then(|| (format!("{verb} {argument}"), key.clone()));
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
                    Ok((target, message)) => { if let Some(search) = search { tools.last_search = Some(search); } tools.message = message; app.set_toast(&tools.message); (recording, target) }
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
    fn press(tools: &mut Tools, code: KeyCode) -> Option<String> {
        tools.take_command(&AppEvent::Key(KeyEvent::new(code, KeyModifiers::NONE)))
    }
    #[test]
    fn completion_cycles_escaped_fields_without_submitting() {
        let mut tools = Tools::default();
        collect_fields(
            &serde_json::json!({"a/b": {"~name": [true]}, "state": "ready"}),
            "",
            0,
            &mut tools.fields,
        );
        collect_fields(
            &serde_json::json!({"request_id": 1}),
            "attachment:",
            0,
            &mut tools.fields,
        );
        assert!(tools.fields.contains(&"/a~1b/~0name/0".into()));
        tools.input = Some("cha".into());
        assert!(press(&mut tools, KeyCode::Tab).is_none());
        assert_eq!(tools.input.as_deref(), Some("change "));
        press(&mut tools, KeyCode::Tab);
        assert_eq!(tools.input.as_deref(), Some("change-prev "));
        press(&mut tools, KeyCode::BackTab);
        assert_eq!(tools.input.as_deref(), Some("change "));
        press(&mut tools, KeyCode::Char('/'));
        press(&mut tools, KeyCode::Tab);
        assert_eq!(tools.input.as_deref(), Some("change /a~1b"));
        assert_eq!(
            tools.candidates("change attachment:"),
            vec!["change attachment:/request_id"]
        );
        assert!(tools.candidates("export /tmp/").is_empty());
    }
    #[test]
    fn history_restores_draft_and_is_bounded_in_memory() {
        let mut tools = Tools::default();
        for i in 0..105 {
            tools.input = Some(format!("find message-{i}"));
            press(&mut tools, KeyCode::Enter);
        }
        assert_eq!(tools.history.len(), 100);
        tools.open(&App::new("test".into()));
        tools.input = Some("find draft".into());
        press(&mut tools, KeyCode::Up);
        assert_eq!(tools.input.as_deref(), Some("find message-104"));
        press(&mut tools, KeyCode::Down);
        assert_eq!(tools.input.as_deref(), Some("find draft"));
        press(&mut tools, KeyCode::Esc);
        tools.open(&App::new("test".into()));
        press(&mut tools, KeyCode::Up);
        assert_eq!(tools.input.as_deref(), Some("find message-104"));
    }
    #[test]
    fn repeated_search_keeps_argument_scope_and_changes_direction() {
        let mut tools = Tools::default();
        assert!(tools.repeat(false).is_none());
        tools.last_search = Some((
            "find-prev warning with spaces".into(),
            Some("demo/status".into()),
        ));
        assert_eq!(
            tools.repeat(false),
            Some((
                "find warning with spaces".into(),
                Some("demo/status".into())
            ))
        );
        tools.last_search = Some(("find-all warning".into(), None));
        assert_eq!(
            tools.repeat(true),
            Some(("find-all-prev warning".into(), None))
        );
        let mut fields = Vec::new();
        collect_fields(&serde_json::json!(vec![0; 1000]), "", 0, &mut fields);
        assert_eq!(fields.len(), 256);
    }
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
