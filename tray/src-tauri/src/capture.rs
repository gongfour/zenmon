//! The capture loop, ported from `zenmon-cli`'s `Command::Capture { dir: Some(_), .. }`
//! arm. Differences from the CLI reference (both deliberate, for an unattended
//! background recorder rather than a short interactive session):
//!
//! - Stop condition is a `watch<bool>` flipped by the tray toggle, not `ctrl_c()`.
//! - `enforce_retention` is throttled to once per [`RETENTION_CHECK_INTERVAL`]
//!   instead of once per message — the CLI's every-write call is cheap for a
//!   short run, but a `read_dir` per message adds up over hours.

use std::time::{Duration, Instant, SystemTime};

use serde::Serialize;
use tauri::async_runtime::JoinHandle;
use tokio::sync::watch;

use zenmon_core::capture::CaptureRecord;
use zenmon_core::trace::{enforce_retention, SegmentWriter};
use zenmon_core::{open_session, subscriber, ZenmonConfig};

use crate::config::Profile;

const RETENTION_CHECK_INTERVAL: Duration = Duration::from_secs(60);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum CaptureState {
    Starting,
    Running,
    Idle,
    Failed,
}

#[derive(Debug, Clone, Serialize)]
pub struct CaptureStatus {
    pub state: CaptureState,
    pub profile_name: String,
    pub messages_written: u64,
    /// Approximate total bytes written this session (NDJSON line lengths
    /// summed client-side; `SegmentWriter` doesn't expose its own counter).
    pub bytes_written: u64,
    /// Milliseconds since the Unix epoch, or `None` — JS-friendly, since
    /// `SystemTime` has no natural JSON representation.
    pub started_at_ms: Option<u64>,
    pub last_message_at_ms: Option<u64>,
    pub last_error: Option<String>,
}

fn epoch_millis(t: SystemTime) -> Option<u64> {
    t.duration_since(SystemTime::UNIX_EPOCH)
        .ok()
        .map(|d| d.as_millis() as u64)
}

impl CaptureStatus {
    pub fn idle(profile_name: &str) -> Self {
        Self {
            state: CaptureState::Idle,
            profile_name: profile_name.to_string(),
            messages_written: 0,
            bytes_written: 0,
            started_at_ms: None,
            last_message_at_ms: None,
            last_error: None,
        }
    }

    fn starting(profile_name: &str) -> Self {
        Self {
            state: CaptureState::Starting,
            ..Self::idle(profile_name)
        }
    }
}

/// Owns a spawned capture task.
pub struct CaptureHandle {
    stop_tx: watch::Sender<bool>,
    pub status_rx: watch::Receiver<CaptureStatus>,
    join: JoinHandle<()>,
}

impl CaptureHandle {
    /// Signal the capture task to flush and tear down. Synchronous,
    /// non-blocking, idempotent — safe to call from a menu-event handler.
    pub fn stop(&self) {
        let _ = self.stop_tx.send(true);
    }

    pub fn status(&self) -> CaptureStatus {
        self.status_rx.borrow().clone()
    }

    /// Await the task's teardown. Only meant to be called after `stop()`,
    /// from the Quit handler.
    pub async fn join(self) {
        let _ = self.join.await;
    }
}

/// Spawn the capture loop onto Tauri's async runtime (which is tokio).
/// Synchronous — returns immediately with a handle. A panic inside the
/// spawned task fails its `JoinHandle` but does not take down the app.
pub fn spawn(zenmon_config: ZenmonConfig, profile: Profile) -> CaptureHandle {
    let (stop_tx, stop_rx) = watch::channel(false);
    let (status_tx, status_rx) = watch::channel(CaptureStatus::starting(&profile.name));
    let join = tauri::async_runtime::spawn(run_capture(zenmon_config, profile, stop_rx, status_tx));
    CaptureHandle {
        stop_tx,
        status_rx,
        join,
    }
}

/// Trim an error down to the sentence a tray user can act on.
///
/// Zenoh error strings append their own source location ("… at
/// C:\…\zenoh-1.9.0\src\net\runtime\orchestrator.rs:386.") — a pointer into
/// zenoh's internals, not at the user's router or endpoint — and can run to
/// several lines. Keep the first line and drop the location suffix; callers
/// log the raw error so nothing is lost, just not shown in the status line.
fn humanize_error(message: &str) -> String {
    let first = message.lines().next().unwrap_or(message).trim();
    match first.rfind(" at ") {
        // Only strip when the tail is a source location — " at " can also be
        // ordinary message content.
        Some(idx) if first[idx..].contains(".rs:") => first[..idx].trim_end().to_string(),
        _ => first.to_string(),
    }
}

fn fail(status_tx: &watch::Sender<CaptureStatus>, message: impl ToString) {
    let raw = message.to_string();
    tracing::warn!(error = %raw, "capture failed");
    let shown = humanize_error(&raw);
    status_tx.send_modify(|s| {
        s.state = CaptureState::Failed;
        s.last_error = Some(shown);
    });
}

async fn run_capture(
    zenmon_config: ZenmonConfig,
    profile: Profile,
    mut stop_rx: watch::Receiver<bool>,
    status_tx: watch::Sender<CaptureStatus>,
) {
    let session = match open_session(&zenmon_config).await {
        Ok(session) => session,
        Err(err) => return fail(&status_tx, err),
    };

    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let subscription = match subscriber::subscribe(&session, &profile.key_expr, tx).await {
        Ok(subscription) => subscription,
        Err(err) => {
            fail(&status_tx, err);
            let _ = session.close().await;
            return;
        }
    };

    let mut writer = match SegmentWriter::open(
        profile.output_dir.clone(),
        profile.rotate_size_bytes,
        profile.rotate_interval(),
    ) {
        Ok(writer) => writer,
        Err(err) => {
            fail(&status_tx, err);
            let _ = subscription.stop().await;
            let _ = session.close().await;
            return;
        }
    };

    let start = Instant::now();
    status_tx.send_modify(|s| {
        s.state = CaptureState::Running;
        s.started_at_ms = epoch_millis(SystemTime::now());
    });

    let mut last_retention_check = Instant::now();

    loop {
        tokio::select! {
            biased;
            _ = stop_rx.changed() => {
                if *stop_rx.borrow() {
                    break;
                }
            }
            item = rx.recv() => match item {
                Some(msg) => {
                    let now = SystemTime::now();
                    let record = CaptureRecord::from_message(&msg, start.elapsed(), now);
                    let line = match serde_json::to_string(&record) {
                        Ok(line) => line,
                        Err(err) => {
                            tracing::warn!(error = %err, "capture record serialization failed");
                            let shown = humanize_error(&err.to_string());
                            status_tx.send_modify(|s| s.last_error = Some(shown));
                            continue;
                        }
                    };
                    if let Err(err) = writer.write_line(&line, now) {
                        tracing::warn!(error = %err, "capture write failed");
                        let shown = humanize_error(&err.to_string());
                        status_tx.send_modify(|s| s.last_error = Some(shown));
                        continue;
                    }

                    if last_retention_check.elapsed() >= RETENTION_CHECK_INTERVAL {
                        let _ = enforce_retention(
                            &profile.output_dir,
                            Some(profile.max_total_size_bytes),
                            Some(profile.max_age()),
                            now,
                        );
                        last_retention_check = Instant::now();
                    }

                    let written_bytes = line.len() as u64 + 1; // + newline
                    status_tx.send_modify(|s| {
                        s.messages_written += 1;
                        s.bytes_written += written_bytes;
                        s.last_message_at_ms = epoch_millis(now);
                    });
                }
                None => break,
            }
        }
    }

    let _ = writer.flush();
    let _ = enforce_retention(
        &profile.output_dir,
        Some(profile.max_total_size_bytes),
        Some(profile.max_age()),
        SystemTime::now(),
    );
    let end = subscription.stop().await;
    let _ = session.close().await;

    status_tx.send_modify(|s| {
        if let Some(err) = end.error() {
            tracing::warn!(error = %err, "capture subscription ended with error");
            s.last_error = Some(humanize_error(&err.to_string()));
        }
        s.state = if end.is_error() {
            CaptureState::Failed
        } else {
            CaptureState::Idle
        };
    });
}

#[cfg(test)]
mod tests {
    use super::humanize_error;

    #[test]
    fn strips_zenoh_source_location_suffix() {
        // Verbatim shape of a live failure (router down, client mode).
        let raw = concat!(
            "failed to open Zenoh session: Unable to connect to any of [tcp/localhost:7447]!",
            " at C:\\Users\\runneradmin\\.cargo\\registry\\src\\index.crates.io-1949cf8c6b5b557f",
            "\\zenoh-1.9.0\\src\\net\\runtime\\orchestrator.rs:386."
        );
        assert_eq!(
            humanize_error(raw),
            "failed to open Zenoh session: Unable to connect to any of [tcp/localhost:7447]!"
        );
    }

    #[test]
    fn keeps_messages_without_a_source_location() {
        assert_eq!(humanize_error("disk full"), "disk full");
        // " at " can be ordinary content, not a location suffix
        assert_eq!(
            humanize_error("no segment at index 3"),
            "no segment at index 3"
        );
    }

    #[test]
    fn keeps_only_the_first_line() {
        assert_eq!(humanize_error("top level\ncaused by: deeper"), "top level");
    }
}
