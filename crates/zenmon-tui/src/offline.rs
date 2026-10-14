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
    // Validate and load before changing terminal state.
    eprintln!(
        "Indexing {} (offline, temporary disk spool)…",
        path.display()
    );
    let mut recording = Recording::load(path)?;
    let mut app = App::new(path.display().to_string());
    app.contract = contract;
    match zenmon_core::profile::load() {
        Ok(profiles) => app.profiles = profiles,
        Err(e) => app.set_error_toast(format!("Saved views unavailable: {e}")),
    }
    let mut position = recording.entries.len() - 1;
    recording.show(&mut app, position)?;
    enable_raw_mode()?;
    let _guard = TerminalGuard;
    execute!(std::io::stdout(), EnterAlternateScreen, EnableMouseCapture)?;
    let mut terminal = Terminal::new(CrosstermBackend::new(std::io::stdout()))?;
    let (_tx, rx) = tokio::sync::mpsc::unbounded_channel();
    let mut events = EventHandler::new(refresh, rx);
    loop {
        terminal.draw(|frame| app.render(frame))?;
        let event = events.next().await?;
        let mut target = position;
        match event {
            AppEvent::Key(k)
                if k.code == KeyCode::Char('c') && k.modifiers.contains(KeyModifiers::CONTROL) =>
            {
                break
            }
            AppEvent::Key(k) if app.overlay == Overlay::None && !app.is_text_input_active() => {
                match k.code {
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
            AppEvent::Tick => {} // A paused recording does not age into live 0 Hz buckets.
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
