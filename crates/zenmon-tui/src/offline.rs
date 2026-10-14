//! A file-only event loop: no session, discovery, query, or publish tasks exist here.
use crate::{
    app::{App, ConnectionState, Overlay, Space},
    event::{AppEvent, EventHandler},
};
use color_eyre::{eyre::eyre, Result};
use crossterm::{
    event::{DisableMouseCapture, EnableMouseCapture, KeyCode, KeyModifiers},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{backend::CrosstermBackend, Terminal};
use std::{
    path::Path,
    time::{Duration, Instant, SystemTime},
};
use zenmon_core::types::{MessagePayload, ZenohMessage};

use crate::recording::Recording;

impl Recording {
    fn show(&mut self, app: &mut App, position: usize) -> Result<()> {
        let selected_key = app.selected_topic_key();
        let selected = app.tree_selected;
        let expanded = app.tree_expanded.clone();
        let unfolded = app.tree_unfolded.clone();
        let touched = app.tree_user_touched;
        app.clear_network_state();
        app.history_paused = false;
        app.total_byte_count = 0;
        app.topic_byte_counts.clear();
        let end = self.entries[position].time;
        let now = Instant::now();
        app.offline_now = Some(now);
        for index in self.snapshot(position)? {
            let time = self.entries[index].time;
            let r = self.read(index)?;
            let payload = r.payload_bytes()?;
            let attachment = r.attachment_bytes()?;
            let received = now
                .checked_sub(end.duration_since(time).unwrap_or_default())
                .ok_or_else(|| eyre!("capture time span is too large"))?;
            let msg = ZenohMessage {
                key_expr: r.key_expr.clone(),
                payload_bytes: payload.len(),
                payload: MessagePayload::from_bytes(payload),
                encoding: r.encoding.clone(),
                timestamp: r.source_timestamp.clone(),
                kind: r.kind.clone(),
                attachment_bytes: attachment.as_ref().map(Vec::len),
                attachment: attachment.map(MessagePayload::from_bytes),
                qos: None,
            };
            app.ingest_at(msg, received);
        }
        app.topic_msg_counts.clear();
        app.topic_byte_counts.clear();
        for (key, count, bytes) in self.counts(position) {
            app.topic_msg_counts
                .insert(self.keys[key].clone(), count as u32);
            app.topic_byte_counts.insert(self.keys[key].clone(), bytes);
        }
        app.total_msg_count = (position + 1) as u32;
        app.total_byte_count = self.entries[position].total_bytes;
        app.evict_excess_keys();
        let elapsed = end
            .duration_since(self.entries[0].time)
            .unwrap_or_default()
            .as_secs_f64();
        // These are capture-window averages, never ingestion-speed rates.
        if elapsed > 0.0 {
            app.total_hz = (position + 1) as f64 / elapsed;
            for (key, count) in &app.topic_msg_counts {
                app.topic_hz.insert(key.clone(), *count as f64 / elapsed);
            }
        }
        app.offline_bandwidth(elapsed);
        if touched {
            app.tree_expanded = expanded;
            app.tree_unfolded = unfolded;
            app.tree_user_touched = true;
        }
        app.tree_selected = selected;
        app.refresh_tree_rows();
        if let Some(key) = selected_key {
            if let Some(index) = app
                .tree_rows()
                .iter()
                .position(|r| r.path == key && r.kind == crate::tree::RowKind::Leaf)
            {
                app.tree_selected = index;
            }
        }
        app.connection_state = ConnectionState::Connected(String::new());
        app.offline_status = Some(format!(
            "{}/{} · {} · rates: window average",
            position + 1,
            self.entries.len(),
            self.read(position)?.received_at.unwrap_or_else(|| format!(
                "+{}ms",
                end.duration_since(SystemTime::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_millis()
            ))
        ));
        Ok(())
    }
}

fn focus_key(app: &mut App, key: &str) {
    if !key
        .to_lowercase()
        .contains(&app.topic_filter.to_lowercase())
    {
        app.topic_filter.clear();
    }
    app.tree_unfolded.insert(String::new());
    let mut prefix = String::new();
    for part in key.split('/') {
        if !prefix.is_empty() {
            prefix.push('/');
        }
        prefix.push_str(part);
        app.tree_expanded.insert(prefix.clone());
        app.tree_unfolded.insert(prefix.clone());
    }
    app.mark_tree_touched();
    app.refresh_tree_rows();
    if let Some(index) = app
        .tree_rows()
        .iter()
        .position(|r| r.path == key && r.kind == crate::tree::RowKind::Leaf)
    {
        app.tree_selected = index;
    }
}

/// Blocks every network intent even if a saved view or palette command requests one.
fn reject_network_actions(app: &mut App) {
    let requested = app.pending_query.take().is_some()
        | app.pending_publish.take().is_some()
        | std::mem::take(&mut app.pending_scout_request)
        | std::mem::take(&mut app.pending_doctor_request)
        | std::mem::take(&mut app.pending_port_scan_request)
        | app.pending_reconnect_port.take().is_some()
        | app.pending_reconnect_mode.take().is_some()
        | (app.space == Space::Network)
        | matches!(
            app.overlay,
            Overlay::Query | Overlay::Publish | Overlay::ScoutPort | Overlay::Doctor
        );
    if requested {
        app.space = Space::Traffic;
        app.detail_mode = crate::app::DetailMode::Live;
        app.overlay = Overlay::None;
        app.set_error_toast("Offline capture: network operations are unavailable");
    }
}

struct TerminalGuard;
impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        let _ = execute!(std::io::stdout(), LeaveAlternateScreen, DisableMouseCapture);
    }
}

pub async fn run(
    path: &Path,
    refresh: Duration,
    contract: Option<zenmon_core::contract::Contract>,
) -> Result<()> {
    run_selected(
        path,
        refresh,
        contract,
        zenmon_core::inspection::Selection::default(),
    )
    .await
}

pub async fn run_selected(
    path: &Path,
    refresh: Duration,
    contract: Option<zenmon_core::contract::Contract>,
    selection: zenmon_core::inspection::Selection,
) -> Result<()> {
    run_with_resume(path, refresh, contract, selection, true).await
}

pub async fn run_with_resume(
    path: &Path,
    refresh: Duration,
    contract: Option<zenmon_core::contract::Contract>,
    selection: zenmon_core::inspection::Selection,
    resume: bool,
) -> Result<()> {
    // Keep the terminal in normal mode during indexing; Ctrl+C can cancel it.
    use std::sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    };
    let cancel = Arc::new(AtomicBool::new(false));
    let worker_cancel = cancel.clone();
    let (progress_tx, mut progress_rx) =
        tokio::sync::watch::channel(zenmon_core::inspection::Progress::default());
    let source = path.to_path_buf();
    eprintln!("Indexing {} (offline) · Ctrl+C cancel", path.display());
    let mut worker = tokio::task::spawn_blocking(move || {
        let mut last = Instant::now();
        Recording::load_selected(
            &source,
            &selection,
            crate::recording::MAX_RECORDS,
            crate::recording::MAX_BYTES,
            |p| {
                crate::offline_tools::cancelled(&worker_cancel)?;
                if last.elapsed() >= Duration::from_millis(200) || p.files == p.total_files {
                    progress_tx.send_replace(p);
                    last = Instant::now();
                }
                Ok(())
            },
        )
    });
    let mut recording = loop {
        tokio::select! {
            result = &mut worker => { break result??; }
            result = tokio::signal::ctrl_c() => {
                cancel.store(true, Ordering::Relaxed);
                let _ = worker.await;
                result?;
                return Err(zenmon_core::ZenmonError::new(zenmon_core::ErrorKind::Interrupted, "indexing cancelled").into());
            }
            Ok(()) = progress_rx.changed() => {
                let p = *progress_rx.borrow_and_update();
                eprintln!("Files {}/{} · scanned {} · selected {} · {:.1} MiB decoded", p.files, p.total_files, p.scanned, p.matched, p.decoded_bytes as f64 / 1048576.0);
            }
        }
    };
    eprintln!(
        "Indexed {} messages across {} keys",
        recording.entries.len(),
        recording.keys.len()
    );
    let mut app = App::new(path.display().to_string());
    app.contract = contract;
    match zenmon_core::profile::load() {
        Ok(profiles) => app.profiles = profiles,
        Err(e) => app.set_error_toast(format!("Saved views unavailable: {e}")),
    }
    let (mut checkpoints, restored, persistence_message) =
        crate::investigation::Checkpoints::open(path, &recording, resume);
    let mut position = restored
        .as_ref()
        .map_or(recording.entries.len() - 1, |s| s.position);
    let mut tools = crate::offline_tools::Tools::default();
    recording.show(&mut app, position)?;
    if let Some(state) = restored {
        app.apply_profile(&state.view);
        if let Some(key) = &state.selected_key {
            // Restore only the visible selection; never clear the saved filter.
            if let Some(i) = app
                .tree_rows()
                .iter()
                .position(|r| r.path == *key && r.kind == crate::tree::RowKind::Leaf)
            {
                app.tree_selected = i;
            }
        }
        tools.start = state.start;
        tools.end = state.end;
        tools.last_search = state.search;
        tools.message = "Restored investigation (paused)".into();
        app.set_toast("Restored investigation (paused)");
    }
    if let Some(message) = persistence_message {
        app.set_error_toast(message);
    }
    enable_raw_mode()?;
    let _guard = TerminalGuard;
    execute!(std::io::stdout(), EnterAlternateScreen, EnableMouseCapture)?;
    let mut terminal = Terminal::new(CrosstermBackend::new(std::io::stdout()))?;
    let (_tx, rx) = tokio::sync::mpsc::unbounded_channel();
    let mut events = EventHandler::new(refresh.min(Duration::from_millis(100)), rx);
    let mut last_checkpoint = Instant::now();
    let mut playback = crate::playback::Playback::default();
    loop {
        if checkpoints.enabled() && last_checkpoint.elapsed() >= Duration::from_secs(5) {
            let state = crate::investigation::State::snapshot(&recording, position, &app, &tools);
            if let Err(e) = checkpoints.checkpoint(state, false).await {
                app.set_error_toast(format!("Could not save investigation: {e}"));
            }
            last_checkpoint = Instant::now();
        }
        terminal.draw(|frame| {
            let status = app.offline_status.take();
            app.offline_status = status
                .as_ref()
                .map(|s| format!("{} · {s}", playback.label()));
            app.render(frame);
            app.offline_status = status;
            tools.render(frame);
        })?;
        let event = events.next().await?;
        let mut target = position;
        let repeat = match &event {
            AppEvent::Key(k)
                if matches!(k.code, KeyCode::F(3) | KeyCode::F(4))
                    && tools.input.is_none()
                    && app.overlay == Overlay::None
                    && !app.is_text_input_active() =>
            {
                playback.pause();
                match tools
                    .repeat(k.code == KeyCode::F(4) || k.modifiers.contains(KeyModifiers::SHIFT))
                {
                    Some((command, key)) => {
                        if let Some(key) = key {
                            focus_key(&mut app, &key);
                            if app.selected_topic_key().as_deref() != Some(key.as_str()) {
                                app.set_error_toast(
                                    "The search key has no message at this position",
                                );
                                continue;
                            }
                        }
                        Some(command)
                    }
                    None => {
                        app.set_toast("Run a find/change search in F2 first");
                        None
                    }
                }
            }
            _ => None,
        };
        if tools.input.is_some() || repeat.is_some() {
            playback.pause();
            if let Some(command) = repeat.or_else(|| tools.take_command(&event)) {
                let focus_match = command.trim_start().starts_with("find")
                    || command.trim_start().starts_with("change");
                let (next, found) = crate::offline_tools::command(
                    recording,
                    &mut tools,
                    command,
                    position,
                    &mut app,
                    &mut terminal,
                    &mut events,
                )
                .await?;
                recording = next;
                if let Some(found) = found {
                    position = found;
                    recording.show(&mut app, position)?;
                    if focus_match {
                        focus_key(&mut app, recording.key_at(position));
                    }
                }
            }
            continue;
        }
        if let AppEvent::Key(k) = &event {
            if !matches!(k.code, KeyCode::Char(' ') | KeyCode::F(9) | KeyCode::F(10))
                || app.overlay != Overlay::None
                || app.is_text_input_active()
            {
                playback.pause();
            }
        } else if matches!(event, AppEvent::Mouse(_)) {
            playback.pause();
        }
        match event {
            AppEvent::Key(k)
                if k.code == KeyCode::Char('c') && k.modifiers.contains(KeyModifiers::CONTROL) =>
            {
                break
            }
            AppEvent::Key(k) if app.overlay == Overlay::None && !app.is_text_input_active() => {
                match k.code {
                    KeyCode::F(2) => tools.open(&app),
                    KeyCode::F(3) | KeyCode::F(4) => {}
                    KeyCode::Char(' ') => {
                        target = playback.toggle(&recording, position, Instant::now())
                    }
                    KeyCode::F(9) => playback.speed(false, Instant::now()),
                    KeyCode::F(10) => playback.speed(true, Instant::now()),
                    KeyCode::Char('[') => target = position.saturating_sub(1),
                    KeyCode::Char(']') => target = (position + 1).min(recording.entries.len() - 1),
                    KeyCode::F(5) | KeyCode::Char('{') => target = 0,
                    KeyCode::F(6) | KeyCode::Char('}') => target = recording.entries.len() - 1,
                    KeyCode::F(7) | KeyCode::Char(',') => {
                        target = recording.seek_time(position, false)
                    }
                    KeyCode::F(8) | KeyCode::Char('.') => {
                        target = recording.seek_time(position, true)
                    }
                    _ => app.handle_event(AppEvent::Key(k)),
                }
            }
            AppEvent::Tick => {
                target = playback.target(&recording, position, Instant::now());
            }
            other => app.handle_event(other),
        }
        reject_network_actions(&mut app);
        if app.should_quit {
            break;
        }
        if target != position {
            position = target;
            recording.show(&mut app, position)?;
        }
    }
    let state = crate::investigation::State::snapshot(&recording, position, &app, &tools);
    let saved = checkpoints.checkpoint(state, true).await;
    drop(terminal);
    drop(_guard);
    if let Err(e) = saved {
        eprintln!("Could not save investigation: {e}");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::recording::{MAX_BYTES, MAX_RECORDS};
    use zenmon_core::{capture::CaptureRecord, trace};
    fn records() -> Vec<CaptureRecord> {
        (0..3)
            .map(|i| CaptureRecord {
                schema_version: 1,
                key_expr: "demo/value".into(),
                payload_base64: ["MA==", "MQ==", "Mg=="][i as usize].into(),
                encoding: "application/json".into(),
                attachment_base64: Some("AP8=".into()),
                source_timestamp: None,
                received_at: None,
                received_offset_ms: i * 1000,
                kind: "PUT".into(),
            })
            .collect()
    }
    fn recording() -> Recording {
        use std::io::Write;
        let mut file = tempfile::NamedTempFile::new().unwrap();
        for record in records() {
            writeln!(file, "{}", serde_json::to_string(&record).unwrap()).unwrap();
        }
        Recording::load(file.path()).unwrap()
    }
    #[test]
    fn seeking_rebuilds_history_without_future_values_and_keeps_binary_attachment() {
        let mut r = recording();
        let mut app = App::new("test".into());
        r.show(&mut app, 2).unwrap();
        assert_eq!(app.history.get("demo/value").unwrap().len(), 3);
        assert_eq!(app.topic_latest["demo/value"].0.payload.as_bytes(), b"2");
        assert_eq!(
            app.topic_latest["demo/value"]
                .0
                .attachment
                .as_ref()
                .unwrap()
                .as_bytes(),
            &[0, 255]
        );
        assert_eq!(app.topic_hz["demo/value"], 1.5);
        r.show(&mut app, 0).unwrap();
        assert_eq!(app.history.get("demo/value").unwrap().len(), 1);
        assert_eq!(app.topic_latest["demo/value"].0.payload.as_bytes(), b"0");
        assert!(!app.allow_publish);
        assert_eq!(r.seek_time(0, true), 1);
        assert_eq!(r.seek_time(2, false), 1);
    }
    #[test]
    fn network_intents_are_rejected_and_offline_header_is_rendered() {
        let mut app = App::new("test".into());
        recording().show(&mut app, 2).unwrap();
        app.pending_scout_request = true;
        app.pending_doctor_request = true;
        app.pending_publish = Some(("demo/value".into(), "1".into()));
        app.space = Space::Network;
        reject_network_actions(&mut app);
        assert!(!app.pending_scout_request);
        assert!(!app.pending_doctor_request);
        assert!(app.pending_publish.is_none());
        assert_eq!(app.space, Space::Traffic);
        let backend = ratatui::backend::TestBackend::new(120, 30);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|f| app.render(f)).unwrap();
        let rendered: String = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|c| c.symbol())
            .collect();
        assert!(rendered.contains("OFFLINE"));
        assert!(!rendered.contains("Connected zid:"));
    }
    #[test]
    fn loader_checks_compressed_input_and_limits_before_display() {
        let path =
            std::env::temp_dir().join(format!("zenmon-offline-{}.ndjson.zst", std::process::id()));
        let mut writer = trace::CaptureFileWriter::create(&path, trace::Compression::Zstd).unwrap();
        for record in records() {
            writer
                .write_line(&serde_json::to_string(&record).unwrap(), SystemTime::now())
                .unwrap();
        }
        writer.finish().unwrap();
        drop(writer);
        assert_eq!(Recording::load(&path).unwrap().entries.len(), 3);
        assert!(Recording::load_with_limits(&path, 2, MAX_BYTES).is_err());
        assert!(Recording::load_with_limits(&path, MAX_RECORDS, 10).is_err());
        std::fs::write(&path, "broken\n").unwrap();
        assert!(Recording::load(&path).is_err());
        std::fs::remove_file(path).unwrap();
    }
    #[test]
    #[ignore = "manual render benchmark: set ZENMON_BENCH_TRACE to a capture file"]
    fn benchmark_recording_render() {
        let path = std::env::var("ZENMON_BENCH_TRACE").expect("capture path");
        let start = Instant::now();
        let mut recording = Recording::load(Path::new(&path)).unwrap();
        let mut app = App::new("render benchmark".into());
        recording
            .show(&mut app, recording.entries.len() - 1)
            .unwrap();
        eprintln!(
            "load_and_show_ms={:.3}",
            start.elapsed().as_secs_f64() * 1000.0
        );
        let largest = app
            .topic_latest
            .iter()
            .max_by_key(|(_, (m, _))| m.payload_bytes)
            .map(|(k, _)| k.clone())
            .unwrap();
        let smallest = app
            .topic_latest
            .iter()
            .min_by_key(|(_, (m, _))| m.payload_bytes)
            .map(|(k, _)| k.clone())
            .unwrap();
        let mut terminal = Terminal::new(ratatui::backend::TestBackend::new(120, 30)).unwrap();
        for (label, key) in [("largest", largest), ("smallest", smallest)] {
            focus_key(&mut app, &key);
            app.topic_detail_scroll = 0;
            let start = Instant::now();
            terminal.draw(|f| app.render(f)).unwrap();
            eprintln!(
                "{label} payload_bytes={} first_frame_ms={:.3}",
                app.topic_latest[&key].0.payload_bytes,
                start.elapsed().as_secs_f64() * 1000.0
            );
            let mut idle = Vec::new();
            for _ in 0..100 {
                let start = Instant::now();
                terminal.draw(|f| app.render(f)).unwrap();
                idle.push(start.elapsed().as_secs_f64() * 1000.0);
            }
            idle.sort_by(f64::total_cmp);
            let mut scroll = Vec::new();
            for offset in [3, 6, 9, 30, 100, 1000, 3, 0] {
                app.topic_detail_scroll = offset;
                let start = Instant::now();
                terminal.draw(|f| app.render(f)).unwrap();
                scroll.push(start.elapsed().as_secs_f64() * 1000.0);
            }
            scroll.sort_by(f64::total_cmp);
            eprintln!("{label} idle_p50_ms={:.3} idle_p95_ms={:.3} scroll_p50_ms={:.3} scroll_max_ms={:.3}",idle[50],idle[95],scroll[4],scroll[7]);
        }
    }

    #[test]
    #[ignore = "manual benchmark: set ZENMON_BENCH_TRACE to a capture file"]
    fn benchmark_recording_seek() {
        let path = std::env::var("ZENMON_BENCH_TRACE").expect("capture path");
        let start = Instant::now();
        let mut recording = Recording::load(Path::new(&path)).unwrap();
        eprintln!("index/load: {:?}", start.elapsed());
        let mut app = App::new("benchmark".into());
        let start = Instant::now();
        for delta in 1..=5 {
            recording
                .show(&mut app, recording.entries.len() - delta)
                .unwrap();
        }
        eprintln!("five backward seeks: {:?}", start.elapsed());
    }
}
