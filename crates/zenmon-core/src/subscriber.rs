//! Subscribing to a key expression.
//!
//! [`subscribe`] declares a Zenoh subscriber and spawns a pump task that decodes
//! samples into [`ZenohMessage`] and forwards them to a [`MessageSink`]. It
//! hands back a [`Subscription`], which owns that task:
//!
//! - **Stopping is explicit.** [`Subscription::stop`] signals the pump, waits for
//!   it, and undeclares the subscriber — no need to kill the consumer to stop the
//!   producer. Dropping a `Subscription` aborts the pump instead, so a forgotten
//!   subscription cannot outlive its handle.
//! - **The end reason reaches the caller.** `stop()` returns a
//!   [`SubscriptionEnd`] that distinguishes an ordinary shutdown from a
//!   subscription that died on its own, and [`SubscriptionEnd::is_error`] splits
//!   normal endings from failures. [`Subscription::is_finished`] lets a UI notice
//!   the death on its existing refresh tick without awaiting anything.
//! - **Backpressure is available.** A `tokio::sync::mpsc::Sender` sink makes the
//!   pump wait for capacity, which pushes back into Zenoh's own receive queue; an
//!   `UnboundedSender` keeps the previous unbounded behaviour.

use crate::error::ZenmonError;
use crate::types::{MessagePayload, ZenohMessage};
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;
use zenoh::Session;

/// Where a [`Subscription`] delivers decoded messages.
///
/// Both channel flavours are accepted because the right overflow policy belongs
/// to the consumer, not to this crate: a bounded `mpsc::channel(n)` makes the
/// pump await capacity (so a slow reader throttles the subscriber instead of
/// growing a queue), while an `mpsc::unbounded_channel()` never blocks and never
/// drops but can grow without limit on a high-rate key.
///
/// Backpressure from a bounded sink propagates: once the pump parks waiting for
/// room it stops draining Zenoh's own subscriber queue, a 256-slot FIFO that
/// blocks the Zenoh thread when full. So a sink of capacity `n` buffers roughly
/// `n + 256` samples before publishers feel it.
///
/// Construct it by passing either sender straight to [`subscribe`]; the `From`
/// impls do the rest.
#[derive(Debug, Clone)]
pub enum MessageSink {
    /// Applies backpressure: the pump waits for room.
    Bounded(mpsc::Sender<ZenohMessage>),
    /// Never blocks; the queue can grow without limit.
    Unbounded(mpsc::UnboundedSender<ZenohMessage>),
}

impl From<mpsc::Sender<ZenohMessage>> for MessageSink {
    fn from(tx: mpsc::Sender<ZenohMessage>) -> Self {
        MessageSink::Bounded(tx)
    }
}

impl From<mpsc::UnboundedSender<ZenohMessage>> for MessageSink {
    fn from(tx: mpsc::UnboundedSender<ZenohMessage>) -> Self {
        MessageSink::Unbounded(tx)
    }
}

/// Marker for "the receiving half is gone".
struct SinkClosed;

impl MessageSink {
    /// Deliver one message, waiting for capacity on a bounded sink.
    ///
    /// Cancel-safe: if the returned future is dropped the message is not sent.
    async fn send(&self, msg: ZenohMessage) -> Result<(), SinkClosed> {
        match self {
            MessageSink::Bounded(tx) => tx.send(msg).await.map_err(|_| SinkClosed),
            MessageSink::Unbounded(tx) => tx.send(msg).map_err(|_| SinkClosed),
        }
    }
}

/// Why a subscription stopped delivering messages.
///
/// The first three are ordinary endings; only [`SubscriptionEnd::Failed`] means
/// something went wrong. Use [`SubscriptionEnd::is_error`] rather than matching
/// if that is the only distinction you need.
#[derive(Debug)]
pub enum SubscriptionEnd {
    /// [`Subscription::stop`] was called.
    Stopped,
    /// The receiving half of the sink was dropped, so there is nobody left to
    /// deliver to.
    ReceiverDropped,
    /// Zenoh stopped delivering samples — normally because the session was
    /// closed underneath the subscription.
    Closed,
    /// The subscription ended abnormally.
    Failed(ZenmonError),
}

impl SubscriptionEnd {
    /// `true` only for [`SubscriptionEnd::Failed`].
    pub fn is_error(&self) -> bool {
        matches!(self, SubscriptionEnd::Failed(_))
    }

    /// The error behind a [`SubscriptionEnd::Failed`], if any.
    pub fn error(&self) -> Option<&ZenmonError> {
        match self {
            SubscriptionEnd::Failed(e) => Some(e),
            _ => None,
        }
    }
}

/// An active subscription and the pump task feeding its sink.
///
/// Owning this value owns the subscriber's lifetime: call [`Subscription::stop`]
/// to shut it down and learn why it ended, or drop it to abort the pump (which
/// undeclares the subscriber as the task unwinds, but reports nothing).
#[derive(Debug)]
pub struct Subscription {
    key_expr: String,
    stop: Option<oneshot::Sender<()>>,
    task: Option<JoinHandle<SubscriptionEnd>>,
}

impl Subscription {
    /// The key expression this subscription was declared on.
    pub fn key_expr(&self) -> &str {
        &self.key_expr
    }

    /// Whether the pump has already ended. Cheap and non-blocking, so a UI can
    /// poll it on its redraw tick and call [`Subscription::stop`] to collect the
    /// reason once it flips.
    pub fn is_finished(&self) -> bool {
        self.task.as_ref().is_none_or(JoinHandle::is_finished)
    }

    /// Stop the subscription and report why it ended.
    ///
    /// Signals the pump, waits for it to drain out, and undeclares the Zenoh
    /// subscriber. If the subscription had already ended on its own, that
    /// original reason is returned instead of [`SubscriptionEnd::Stopped`], so a
    /// caller polling [`Subscription::is_finished`] still learns the cause.
    ///
    /// Returns promptly even when a bounded sink is full: the stop signal is
    /// selected against the send, not queued behind it.
    pub async fn stop(mut self) -> SubscriptionEnd {
        if let Some(stop) = self.stop.take() {
            // Fails only when the pump already returned; the join below then
            // reports its real end reason.
            let _ = stop.send(());
        }
        let Some(task) = self.task.take() else {
            return SubscriptionEnd::Stopped;
        };
        match task.await {
            Ok(end) => end,
            Err(e) if e.is_cancelled() => SubscriptionEnd::Stopped,
            Err(e) => SubscriptionEnd::Failed(ZenmonError::internal(format!(
                "subscription pump for '{}' panicked: {}",
                self.key_expr, e
            ))),
        }
    }
}

impl Drop for Subscription {
    fn drop(&mut self) {
        // A dropped handle must not leave a live subscriber behind. Aborting
        // drops the Zenoh subscriber, which undeclares it; callers that need the
        // end reason (or a synchronous undeclare) must use `stop()`.
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }
}

/// Subscribe to a key expression, pumping decoded messages into `sink`.
///
/// `sink` accepts either `mpsc::Sender<ZenohMessage>` (backpressure) or
/// `mpsc::UnboundedSender<ZenohMessage>` (unbounded) — see [`MessageSink`].
///
/// The returned [`Subscription`] owns the pump task: stop it with
/// [`Subscription::stop`] to shut down deterministically and learn why it ended,
/// or drop it to abort.
pub async fn subscribe(
    session: &Session,
    key_expr: &str,
    sink: impl Into<MessageSink>,
) -> Result<Subscription, ZenmonError> {
    let sink = sink.into();
    let subscriber = session.declare_subscriber(key_expr).await?;
    tracing::info!(key_expr = %key_expr, "Subscribed");

    let (stop_tx, mut stop_rx) = oneshot::channel();
    let key = key_expr.to_string();
    let task_key = key.clone();

    let task = tokio::spawn(async move {
        let end = loop {
            tokio::select! {
                // Stop first: a firehose on the key must not starve the signal.
                biased;
                _ = &mut stop_rx => break SubscriptionEnd::Stopped,
                received = subscriber.recv_async() => {
                    // The only error flume reports here is "disconnected",
                    // i.e. the subscriber went away with its session.
                    let Ok(sample) = received else {
                        break SubscriptionEnd::Closed;
                    };
                    let msg = decode_sample(&sample);
                    // Selected against the stop signal so a full bounded sink
                    // cannot wedge `stop()`.
                    tokio::select! {
                        biased;
                        _ = &mut stop_rx => break SubscriptionEnd::Stopped,
                        sent = sink.send(msg) => {
                            if sent.is_err() {
                                break SubscriptionEnd::ReceiverDropped;
                            }
                        }
                    }
                }
            }
        };

        // Undeclare explicitly rather than leaving it to the drop glue, so the
        // subscriber is gone by the time `stop()` returns.
        match subscriber.undeclare().await {
            Ok(()) => {
                tracing::debug!(key_expr = %task_key, ?end, "Subscription ended");
                end
            }
            // `end` is never `Failed` here, so nothing is being masked.
            Err(e) => SubscriptionEnd::Failed(ZenmonError::internal(format!(
                "failed to undeclare subscriber on '{}': {}",
                task_key, e
            ))),
        }
    });

    Ok(Subscription {
        key_expr: key,
        stop: Some(stop_tx),
        task: Some(task),
    })
}

fn decode_sample(sample: &zenoh::sample::Sample) -> ZenohMessage {
    let key_expr = sample.key_expr().as_str().to_string();
    let kind = format!("{}", sample.kind());
    let payload = MessagePayload::from_zbytes(sample.payload());
    let encoding = sample.encoding().to_string();
    let timestamp = sample.timestamp().map(|ts| ts.to_string());
    let attachment = sample.attachment().map(MessagePayload::from_zbytes);

    let payload_bytes = payload.len();
    let attachment_bytes = attachment.as_ref().map(|a| a.len());
    ZenohMessage {
        key_expr,
        payload,
        encoding,
        payload_bytes,
        timestamp,
        kind,
        attachment,
        attachment_bytes,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use std::time::Duration;

    const STEP: Duration = Duration::from_millis(300);

    fn message(key: &str) -> ZenohMessage {
        ZenohMessage {
            key_expr: key.to_string(),
            payload: MessagePayload::from_bytes(b"x".to_vec()),
            encoding: String::new(),
            payload_bytes: 1,
            timestamp: None,
            kind: "PUT".to_string(),
            attachment: None,
            attachment_bytes: None,
        }
    }

    /// A peer session with no endpoints and no scouting: it never talks to the
    /// network, but Zenoh still routes a session's own publications to its own
    /// subscribers, which is enough to drive the whole pump offline.
    async fn local_session() -> Session {
        let mut cfg = zenoh::Config::default();
        cfg.insert_json5("mode", "\"peer\"").unwrap();
        cfg.insert_json5("connect/endpoints", "[]").unwrap();
        cfg.insert_json5("listen/endpoints", "[]").unwrap();
        cfg.insert_json5("scouting/multicast/enabled", "false")
            .unwrap();
        cfg.insert_json5("scouting/gossip/enabled", "false")
            .unwrap();
        zenoh::open(cfg).await.unwrap()
    }

    async fn recv_one(rx: &mut mpsc::UnboundedReceiver<ZenohMessage>) -> ZenohMessage {
        tokio::time::timeout(Duration::from_secs(5), rx.recv())
            .await
            .expect("timed out waiting for a message")
            .expect("sink closed")
    }

    // Zenoh's runtime refuses tokio's current-thread scheduler.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn stop_is_an_ordinary_ending_not_an_error() {
        let session = local_session().await;
        let (tx, mut rx) = mpsc::unbounded_channel();
        let sub = subscribe(&session, "test/stop/**", tx).await.unwrap();
        assert_eq!(sub.key_expr(), "test/stop/**");

        session.put("test/stop/a", "1").await.unwrap();
        assert_eq!(recv_one(&mut rx).await.key_expr, "test/stop/a");

        let end = sub.stop().await;
        assert!(matches!(end, SubscriptionEnd::Stopped), "{end:?}");
        assert!(!end.is_error());
        assert!(end.error().is_none());

        // Deterministically stopped: nothing arrives after `stop()` returned.
        session.put("test/stop/a", "2").await.unwrap();
        tokio::time::sleep(STEP).await;
        assert!(rx.try_recv().is_err());
        session.close().await.unwrap();
    }

    #[tokio::test]
    async fn a_failing_pump_is_reported_as_failed() {
        // The pump's own failure paths (undeclare, panic) can't be induced
        // against a live session, so drive `stop()`'s join handling directly.
        let (stop_tx, _stop_rx) = oneshot::channel::<()>();
        let sub = Subscription {
            key_expr: "test/boom/**".to_string(),
            stop: Some(stop_tx),
            task: Some(tokio::spawn(async { panic!("pump exploded") })),
        };

        let end = sub.stop().await;
        assert!(end.is_error(), "{end:?}");
        let err = end.error().expect("a failure carries its error");
        assert!(err.message.contains("test/boom/**"), "{}", err.message);
        assert_eq!(err.kind, crate::error::ErrorKind::Internal);
    }

    // Zenoh's runtime refuses tokio's current-thread scheduler.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn dropping_the_receiver_ends_the_subscription_without_an_error() {
        let session = local_session().await;
        let (tx, mut rx) = mpsc::unbounded_channel();
        let sub = subscribe(&session, "test/dropped/**", tx).await.unwrap();

        session.put("test/dropped/a", "1").await.unwrap();
        recv_one(&mut rx).await;
        drop(rx);

        // The pump only notices on its next delivery attempt.
        session.put("test/dropped/a", "2").await.unwrap();
        let end = tokio::time::timeout(Duration::from_secs(5), sub.stop())
            .await
            .expect("stop timed out");
        assert!(
            matches!(
                end,
                SubscriptionEnd::ReceiverDropped | SubscriptionEnd::Stopped
            ),
            "{end:?}"
        );
        assert!(!end.is_error());
        session.close().await.unwrap();
    }

    // Zenoh's runtime refuses tokio's current-thread scheduler.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn closing_the_session_ends_the_subscription_as_closed() {
        let session = local_session().await;
        let (tx, _rx) = mpsc::unbounded_channel();
        let sub = subscribe(&session, "test/closed/**", tx).await.unwrap();

        session.close().await.unwrap();
        tokio::time::sleep(STEP).await;
        assert!(sub.is_finished(), "a closed session must end the pump");

        let end = tokio::time::timeout(Duration::from_secs(5), sub.stop())
            .await
            .expect("stop timed out");
        // A closed session is an ordinary ending, never a failure.
        assert!(!end.is_error(), "{end:?}");
    }

    #[tokio::test]
    async fn bounded_sink_blocks_the_pump_until_the_reader_catches_up() {
        let (tx, mut rx) = mpsc::channel(1);
        let sink = MessageSink::from(tx);
        let accepted = Arc::new(AtomicUsize::new(0));

        let writer = {
            let accepted = accepted.clone();
            tokio::spawn(async move {
                for i in 0..5 {
                    sink.send(message(&format!("k/{i}"))).await.ok();
                    accepted.fetch_add(1, Ordering::SeqCst);
                }
            })
        };

        tokio::time::sleep(STEP).await;
        // One message sits in the channel and one send is parked waiting for
        // room; the remaining three are still upstream, unread.
        assert!(
            accepted.load(Ordering::SeqCst) <= 2,
            "bounded sink accepted {} messages without a reader",
            accepted.load(Ordering::SeqCst)
        );

        for _ in 0..5 {
            rx.recv().await.unwrap();
        }
        writer.await.unwrap();
        assert_eq!(accepted.load(Ordering::SeqCst), 5);
    }

    #[tokio::test]
    async fn unbounded_sink_accepts_everything_without_a_reader() {
        let (tx, _rx) = mpsc::unbounded_channel();
        let sink = MessageSink::from(tx);
        for i in 0..5 {
            sink.send(message(&format!("k/{i}"))).await.ok();
        }
        // Reaching here without a reader is the point: no backpressure at all.
    }

    // Zenoh's runtime refuses tokio's current-thread scheduler.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn stop_returns_promptly_while_a_bounded_sink_is_full() {
        let session = local_session().await;
        let (tx, _rx) = mpsc::channel(1);
        let sub = subscribe(&session, "test/full/**", tx).await.unwrap();

        // Never read: the sink fills and the pump parks inside `send`.
        for i in 0..20 {
            session.put("test/full/a", format!("{i}")).await.unwrap();
        }
        tokio::time::sleep(STEP).await;

        let end = tokio::time::timeout(Duration::from_secs(2), sub.stop())
            .await
            .expect("stop wedged behind a full sink");
        assert!(matches!(end, SubscriptionEnd::Stopped), "{end:?}");
        session.close().await.unwrap();
    }
}
