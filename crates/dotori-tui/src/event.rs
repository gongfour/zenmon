use color_eyre::Result;
use crossterm::event::{EventStream, KeyEvent, KeyEventKind, MouseEvent};
use dotori_core::types::{NodeInfo, ZenohMessage};
use futures::{FutureExt, StreamExt};
use tokio::sync::mpsc;

#[derive(Clone, Debug)]
pub enum AppEvent {
    Key(KeyEvent),
    Mouse(MouseEvent),
    Zenoh(ZenohMessage),
    Tick,
    AdminNodes(Vec<NodeInfo>),
    ScoutStarted,
    ScoutNodes(Vec<NodeInfo>),
}

pub struct EventHandler {
    tx: mpsc::UnboundedSender<AppEvent>,
    rx: mpsc::UnboundedReceiver<AppEvent>,
    _task: tokio::task::JoinHandle<()>,
}

impl EventHandler {
    pub fn new(tick_rate_ms: u64, zenoh_rx: mpsc::UnboundedReceiver<ZenohMessage>) -> Self {
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

        let tick_delay = std::time::Duration::from_millis(tick_rate_ms);
        let key_tx = tx.clone();
        let task = tokio::spawn(async move {
            let mut reader = EventStream::new();
            let mut tick_interval = tokio::time::interval(tick_delay);

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
}
