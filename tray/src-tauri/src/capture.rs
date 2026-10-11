//! The tray's capture task. What lands on disk — exclusion, records,
//! rotation, retention — is zenmon-core's [`Recorder`], the same pipeline
//! `zenmon capture --dir` runs. What is the tray's own:
//!
//! - The stop condition is a `watch<bool>` flipped by the tray toggle, not
//!   `ctrl_c()`.
//! - A failed write does not end the capture (a full disk may free up), but
//!   it shows: `last_error` is set while writes fail and cleared by the next
//!   one that succeeds, so the tray never says "capturing" over a silent gap.

use std::time::SystemTime;

use serde::Serialize;
use tauri::async_runtime::JoinHandle;
use tokio::sync::watch;

use zenmon_core::capture::{CaptureExclude, CaptureSink, Recorded, Recorder, Retention};
use zenmon_core::trace::SegmentWriter;
use zenmon_core::{open_session, subscriber, ZenmonConfig};

use crate::config::Profile;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum CaptureState {
    Starting,
    Running,
    Idle,
    Failed,
}

impl CaptureState {
    /// Whether a capture is under way — i.e. a click should *stop* it.
    /// `Failed` and `Idle` are both terminal: the task has ended, so a click
    /// should start a fresh one.
    pub fn is_active(self) -> bool {
        matches!(self, CaptureState::Starting | CaptureState::Running)
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct CaptureStatus {
    pub state: CaptureState,
    pub profile_name: String,
    pub messages_written: u64,
    /// Messages dropped by the profile's `exclude` list this session — shown
    /// so an exclusion is visible rather than a silent gap in the capture.
    pub messages_excluded: u64,
    /// Approximate total bytes recorded this session — uncompressed NDJSON
    /// line lengths summed client-side, so with zstd the disk use is smaller.
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
            messages_excluded: 0,
            bytes_written: 0,
            started_at_ms: None,
            last_message_at_ms: None,
            last_error: None,
        }
    }

    /// Running, but the latest write failed — the capture is up and
    /// nothing is landing on disk.
    pub fn is_degraded(&self) -> bool {
        self.state == CaptureState::Running && self.last_error.is_some()
    }

    /// What the tray visibly shows. A change here is pushed at once instead
    /// of waiting out the 1 Hz coalescing.
    pub fn visual(&self) -> (CaptureState, bool) {
        (self.state, self.is_degraded())
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

    /// See [`CaptureState::is_active`]. A failed handle is kept around only so
    /// its error stays visible; it no longer counts as capturing.
    pub fn is_active(&self) -> bool {
        self.status_rx.borrow().state.is_active()
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

fn fail(status_tx: &watch::Sender<CaptureStatus>, message: impl ToString) {
    status_tx.send_modify(|s| {
        s.state = CaptureState::Failed;
        s.last_error = Some(message.to_string());
    });
}

async fn run_capture(
    zenmon_config: ZenmonConfig,
    profile: Profile,
    mut stop_rx: watch::Receiver<bool>,
    status_tx: watch::Sender<CaptureStatus>,
) {
    // Validate before connecting, so a typo fails the start instead of
    // recording everything (or nothing) without a word.
    // Blank lines come from the settings textarea (one expression per line).
    let exclude_exprs: Vec<&str> = profile
        .exclude
        .iter()
        .map(|e| e.trim())
        .filter(|e| !e.is_empty())
        .collect();
    let exclude = match CaptureExclude::parse(&exclude_exprs) {
        Ok(exclude) => exclude,
        Err(err) => return fail(&status_tx, err),
    };

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

    let writer = match SegmentWriter::open_with(
        profile.output_dir.clone(),
        profile.rotate_size_bytes,
        profile.rotate_interval(),
        profile.compression,
    ) {
        Ok(writer) => writer,
        Err(err) => {
            fail(&status_tx, err);
            let _ = subscription.stop().await;
            let _ = session.close().await;
            return;
        }
    };
    let mut recorder = Recorder::new(
        exclude,
        CaptureSink::Store {
            writer,
            dir: profile.output_dir.clone(),
            retention: Retention {
                max_total_size: profile.max_total_size_bytes,
                max_age: profile.max_age(),
            },
        },
    );

    status_tx.send_modify(|s| {
        s.state = CaptureState::Running;
        s.started_at_ms = epoch_millis(SystemTime::now());
    });

    let mut failing = false;
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
                    match recorder.record(&msg, now) {
                        // Not pushed per message: the status channel ticks
                        // once per *written* message already.
                        Ok(Recorded::Excluded) => {}
                        Ok(Recorded::Written { .. }) => {
                            let recovered = std::mem::take(&mut failing);
                            let (written, excluded, bytes) =
                                (recorder.written(), recorder.excluded(), recorder.bytes());
                            status_tx.send_modify(|s| {
                                s.messages_written = written;
                                s.messages_excluded = excluded;
                                s.bytes_written = bytes;
                                s.last_message_at_ms = epoch_millis(now);
                                if recovered {
                                    s.last_error = None;
                                }
                            });
                        }
                        Err(err) => {
                            failing = true;
                            status_tx.send_modify(|s| s.last_error = Some(err.to_string()));
                        }
                    }
                }
                None => break,
            }
        }
    }

    // Finish, not just flush: a zstd segment is only complete once its frame
    // is closed.
    let finished = recorder.finish();
    let end = subscription.stop().await;
    let _ = session.close().await;

    let excluded = recorder.excluded();
    status_tx.send_modify(|s| {
        s.messages_excluded = excluded;
        if let Err(err) = finished {
            s.last_error = Some(err.to_string());
        }
        if let Some(err) = end.error() {
            s.last_error = Some(err.to_string());
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
    use super::*;

    #[test]
    fn a_write_error_while_running_is_a_visible_change() {
        let mut s = CaptureStatus::idle("p");
        s.state = CaptureState::Running;
        let healthy = s.visual();
        s.last_error = Some("disk full".into());
        assert!(s.is_degraded());
        assert_ne!(s.visual(), healthy, "must bypass the 1 Hz coalescing");
    }

    #[test]
    fn an_error_on_a_stopped_capture_is_not_degraded() {
        let mut s = CaptureStatus::idle("p");
        s.state = CaptureState::Failed;
        s.last_error = Some("connect refused".into());
        assert!(!s.is_degraded());
    }
}
