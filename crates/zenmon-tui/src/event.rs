use color_eyre::Result;
use crossterm::event::{EventStream, KeyEvent, KeyEventKind, MouseEvent};
use zenmon_core::types::{LivelinessEvent, NodeInfo, PortScoutResult, ZenohMessage};
use futures::{FutureExt, StreamExt};
use tokio::sync::mpsc;
use tokio::sync::mpsc::error::TryRecvError;

#[derive(Clone, Debug)]
pub enum AppEvent {
    Key(KeyEvent),
    Mouse(MouseEvent),
    Zenoh(ZenohMessage),
    Tick,
    AdminNodes(Vec<NodeInfo>),
    ScoutStarted,
    ScoutNodes(Vec<NodeInfo>),
    PortScanStarted,
    PortScanResults(Vec<PortScoutResult>),
    Liveliness(LivelinessEvent),
}

/// Build the event-loop tick interval from the user's refresh `Duration`.
///
/// Kept separate (and taking a `Duration` directly, never a millisecond count)
/// so the non-zero-period invariant is unit-testable without a live terminal.
/// A previous `refresh.as_millis() as u64` conversion truncated sub-millisecond
/// refreshes (e.g. `--refresh 1ns`) to `0ms`, which made this panic with
/// "interval period must be non-zero" after the TTY was already initialized.
fn build_tick_interval(period: std::time::Duration) -> tokio::time::Interval {
    tokio::time::interval(period)
}

pub struct EventHandler {
    tx: mpsc::UnboundedSender<AppEvent>,
    rx: mpsc::UnboundedReceiver<AppEvent>,
    _task: tokio::task::JoinHandle<()>,
}

impl EventHandler {
    pub fn new(
        tick_delay: std::time::Duration,
        zenoh_rx: mpsc::UnboundedReceiver<ZenohMessage>,
    ) -> Self {
        let (tx, rx) = mpsc::unbounded_channel();

        let zenoh_tx = tx.clone();
        tokio::spawn(async move {
            let mut zenoh_rx = zenoh_rx;
            while let Some(msg) = zenoh_rx.recv().await {
                if zenoh_tx.send(AppEvent::Zenoh(msg)).is_err() {
                    break;
                }
            }
        });

        let key_tx = tx.clone();
        let task = tokio::spawn(async move {
            let mut reader = EventStream::new();
            let mut tick_interval = build_tick_interval(tick_delay);

            loop {
                let tick = tick_interval.tick();
                let crossterm_event = reader.next().fuse();

                tokio::select! {
                    maybe_event = crossterm_event => {
                        match maybe_event {
                            Some(Ok(evt)) => {
                                match evt {
                                    crossterm::event::Event::Key(key) => {
                                        if key.kind == KeyEventKind::Press
                                            && key_tx.send(AppEvent::Key(key)).is_err()
                                        {
                                            break;
                                        }
                                    }
                                    crossterm::event::Event::Mouse(m) => {
                                        if key_tx.send(AppEvent::Mouse(m)).is_err() {
                                            break;
                                        }
                                    }
                                    _ => {}
                                }
                            }
                            Some(Err(_)) => break,
                            None => break,
                        }
                    },
                    _ = tick => {
                        if key_tx.send(AppEvent::Tick).is_err() {
                            break;
                        }
                    },
                }
            }
        });

        Self { tx, rx, _task: task }
    }

    pub fn sender(&self) -> mpsc::UnboundedSender<AppEvent> {
        self.tx.clone()
    }

    pub async fn next(&mut self) -> Result<AppEvent> {
        self.rx
            .recv()
            .await
            .ok_or_else(|| color_eyre::eyre::eyre!("Event channel closed"))
    }

    pub fn try_next(&mut self) -> Result<Option<AppEvent>> {
        match self.rx.try_recv() {
            Ok(event) => Ok(Some(event)),
            Err(TryRecvError::Empty) => Ok(None),
            Err(TryRecvError::Disconnected) => {
                Err(color_eyre::eyre::eyre!("Event channel closed"))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    /// A sub-millisecond refresh (e.g. `--refresh 1ns`) used to be truncated to
    /// `0ms` via `as_millis() as u64`, and `tokio::time::interval(0)` panics with
    /// "interval period must be non-zero". Passing the `Duration` straight
    /// through keeps it a valid, non-zero period; the first tick fires
    /// immediately so awaiting it proves the interval was built without panic.
    #[tokio::test]
    async fn build_tick_interval_accepts_sub_ms_period() {
        // Sanity-check the old, unsafe conversion actually collapsed to zero.
        assert_eq!(Duration::from_nanos(1).as_millis() as u64, 0);

        let mut interval = build_tick_interval(Duration::from_nanos(1));
        interval.tick().await; // immediate first tick; would have panicked at 0ms
    }

    /// A large refresh previously funnelled through a lossy `u128 -> u64`
    /// millisecond cast could wrap. Kept as a `Duration`, it must build a usable
    /// interval whose immediate first tick fires without panicking.
    #[tokio::test]
    async fn build_tick_interval_accepts_large_period() {
        // ~1 year: within `Instant` range, far beyond any refresh a user sets.
        let mut interval = build_tick_interval(Duration::from_secs(365 * 24 * 3600));
        interval.tick().await;
    }
}
