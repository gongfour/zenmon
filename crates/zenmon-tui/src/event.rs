use color_eyre::Result;
use crossterm::event::{EventStream, KeyEvent, KeyEventKind, MouseEvent};
use futures::{FutureExt, StreamExt};
use tokio::sync::mpsc;
use tokio::sync::mpsc::error::TryRecvError;
use zenmon_core::types::{
    LivelinessEvent, NodeInfo, PortScoutResult, TransportChange, ZenohMessage,
};

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
    DoctorStarted,
    DoctorReport(zenmon_core::doctor::DoctorReport),
    /// A background task failed in a way the user must see (the TUI has no
    /// log output — tracing is off so it cannot corrupt the screen).
    Error(String),
    /// This session opened or lost a transport.
    Transport(TransportChange),
    /// Outcome of a publish armed in the editor, or why it failed.
    Published(Result<crate::app::Sent, String>),
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

/// Merges the dashboard's event sources into one stream with a fixed
/// priority: terminal input (and ticks) first, then background-task results,
/// then Zenoh traffic.
///
/// Three queues rather than one so a traffic burst cannot bury a keypress:
/// with a single FIFO, a key pressed during a flood waited behind every
/// message already queued, and the dashboard stopped answering exactly when
/// someone was trying to use it.
pub struct EventHandler {
    tx: mpsc::UnboundedSender<AppEvent>,
    rx: mpsc::UnboundedReceiver<AppEvent>,
    input_rx: mpsc::UnboundedReceiver<AppEvent>,
    zenoh_rx: mpsc::UnboundedReceiver<ZenohMessage>,
    _task: tokio::task::JoinHandle<()>,
}

impl EventHandler {
    pub fn new(
        tick_delay: std::time::Duration,
        zenoh_rx: mpsc::UnboundedReceiver<ZenohMessage>,
    ) -> Self {
        let (tx, rx) = mpsc::unbounded_channel();
        let (key_tx, input_rx) = mpsc::unbounded_channel();

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
                                    crossterm::event::Event::Mouse(m)
                                        if key_tx.send(AppEvent::Mouse(m)).is_err() => {
                                            break;
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

        Self {
            tx,
            rx,
            input_rx,
            zenoh_rx,
            _task: task,
        }
    }

    /// Sender for background tasks' results (scout, doctor, publish, …).
    pub fn sender(&self) -> mpsc::UnboundedSender<AppEvent> {
        self.tx.clone()
    }

    pub async fn next(&mut self) -> Result<AppEvent> {
        tokio::select! {
            biased;
            Some(event) = self.input_rx.recv() => Ok(event),
            Some(event) = self.rx.recv() => Ok(event),
            Some(msg) = self.zenoh_rx.recv() => Ok(AppEvent::Zenoh(msg)),
            else => Err(color_eyre::eyre::eyre!("Event channel closed")),
        }
    }

    /// The next already-queued event, in the same priority order as
    /// [`EventHandler::next`], or `None` when every queue is empty.
    pub fn try_next(&mut self) -> Result<Option<AppEvent>> {
        if let Ok(event) = self.input_rx.try_recv() {
            return Ok(Some(event));
        }
        if let Ok(event) = self.rx.try_recv() {
            return Ok(Some(event));
        }
        match self.zenoh_rx.try_recv() {
            Ok(msg) => Ok(Some(AppEvent::Zenoh(msg))),
            Err(TryRecvError::Empty) | Err(TryRecvError::Disconnected) => Ok(None),
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

    fn message(key: &str) -> ZenohMessage {
        ZenohMessage {
            key_expr: key.to_string(),
            payload: zenmon_core::types::MessagePayload::from_bytes(b"x".to_vec()),
            encoding: String::new(),
            payload_bytes: 1,
            timestamp: None,
            kind: "PUT".to_string(),
            attachment: None,
            attachment_bytes: None,
            qos: None,
        }
    }

    /// A keypress queued behind a traffic backlog is handed out first.
    #[tokio::test]
    async fn input_jumps_ahead_of_queued_traffic() {
        let (tx, rx) = mpsc::unbounded_channel();
        let (key_tx, input_rx) = mpsc::unbounded_channel();
        let (zenoh_tx, zenoh_rx) = mpsc::unbounded_channel();
        let mut events = EventHandler {
            tx,
            rx,
            input_rx,
            zenoh_rx,
            _task: tokio::spawn(async {}),
        };

        for i in 0..1000 {
            zenoh_tx.send(message(&format!("k/{i}"))).unwrap();
        }
        events.sender().send(AppEvent::ScoutStarted).unwrap();
        key_tx.send(AppEvent::Tick).unwrap();

        assert!(matches!(events.next().await.unwrap(), AppEvent::Tick));
        assert!(matches!(
            events.try_next().unwrap(),
            Some(AppEvent::ScoutStarted)
        ));
        match events.next().await.unwrap() {
            AppEvent::Zenoh(m) => assert_eq!(m.key_expr, "k/0"),
            other => panic!("expected traffic, got {other:?}"),
        }
    }
}
