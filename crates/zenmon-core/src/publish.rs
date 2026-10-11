//! Publishing a payload to a key expression.
//!
//! Lifted out of the CLI so the TUI publishes through exactly the same call —
//! the same pairing `subscriber` already has with the `sub` command. Two
//! implementations of "put a value on a key" would eventually disagree about
//! attachments or error mapping, and the one behind a UI is the one nobody
//! would notice drifting.

use zenoh::Session;

use crate::error::{Result, ZenmonError};

/// How a put is sent. `Default` is Zenoh's own defaults (data priority,
/// drop when congested, batched), so a caller that sets nothing changes
/// nothing.
#[derive(Debug, Clone, Copy, Default)]
pub struct PutQos {
    pub priority: Option<zenoh::qos::Priority>,
    pub congestion_control: Option<zenoh::qos::CongestionControl>,
    pub express: bool,
}

/// Put `value` on `key_expr`, optionally with an attachment.
///
/// One shot. Repeating at a rate is the caller's business: the CLI spends a
/// count/duration budget, and the TUI publishes once per confirmation.
pub async fn put(
    session: &Session,
    key_expr: &str,
    value: Vec<u8>,
    attachment: Option<&[u8]>,
) -> Result<()> {
    put_with(session, key_expr, value, attachment, PutQos::default()).await
}

/// [`put`] with explicit QoS — to reproduce how a real publisher sends, since
/// a `Block` publisher and a `Drop` one behave differently under load.
pub async fn put_with(
    session: &Session,
    key_expr: &str,
    value: Vec<u8>,
    attachment: Option<&[u8]>,
    qos: PutQos,
) -> Result<()> {
    let mut builder = session.put(key_expr, value).express(qos.express);
    if let Some(p) = qos.priority {
        builder = builder.priority(p);
    }
    if let Some(cc) = qos.congestion_control {
        builder = builder.congestion_control(cc);
    }
    if let Some(bytes) = attachment {
        builder = builder.attachment(bytes.to_vec());
    }
    builder
        .await
        .map_err(|e| ZenmonError::internal(e.to_string()))
}

/// Whether any subscriber on the network matches `key_expr`.
///
/// A put with nobody listening succeeds just the same, so "published" alone
/// cannot tell a working command from one going nowhere — a typo in the key,
/// or the consumer not running. Routing reports matches asynchronously, so a
/// fresh session may need a moment: this waits up to `settle` for one before
/// answering `false`, and answers at once when a match is already known.
pub async fn has_subscribers(
    session: &Session,
    key_expr: &str,
    settle: std::time::Duration,
) -> Result<bool> {
    let internal = |e: zenoh::Error| ZenmonError::internal(e.to_string());
    let publisher = session
        .declare_publisher(key_expr.to_string())
        .await
        .map_err(internal)?;
    // Listen before reading the status, so a match landing in between is
    // not lost.
    let listener = publisher.matching_listener().await.map_err(internal)?;
    let matched = if publisher
        .matching_status()
        .await
        .map_err(internal)?
        .matching()
    {
        true
    } else {
        tokio::time::timeout(settle, async {
            while let Ok(status) = listener.recv_async().await {
                if status.matching() {
                    return true;
                }
            }
            false
        })
        .await
        .unwrap_or(false)
    };
    let _ = listener.undeclare().await;
    let _ = publisher.undeclare().await;
    Ok(matched)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    async fn local_session() -> Session {
        let mut cfg = zenoh::Config::default();
        cfg.insert_json5("mode", "\"peer\"").unwrap();
        cfg.insert_json5("listen/endpoints", "[]").unwrap();
        cfg.insert_json5("scouting/multicast/enabled", "false")
            .unwrap();
        cfg.insert_json5("scouting/gossip/enabled", "false")
            .unwrap();
        zenoh::open(cfg).await.unwrap()
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn matching_follows_whether_a_subscriber_exists() {
        let session = local_session().await;
        let settle = Duration::from_millis(300);
        assert!(!has_subscribers(&session, "test/match/a", settle)
            .await
            .unwrap());

        let _sub = session.declare_subscriber("test/match/**").await.unwrap();
        assert!(has_subscribers(&session, "test/match/a", settle)
            .await
            .unwrap());
        assert!(!has_subscribers(&session, "test/other", settle)
            .await
            .unwrap());
    }
}
