use crate::error::ZenmonError;
use crate::types::{LivelinessEvent, LivelinessToken, TopicInfo, TransportChange};
use tokio::sync::mpsc;
use zenoh::sample::SampleKind;
use zenoh::Session;

/// Discover active keys matching the given key expression.
/// Uses Zenoh admin space to list subscribers and publishers.
/// Falls back to a plain GET if admin space returns nothing.
pub async fn discover(session: &Session, key_expr: &str) -> Result<Vec<TopicInfo>, ZenmonError> {
    let mut topics = Vec::new();

    // Query admin space for subscriber/publisher info
    let admin_key = "@/router/local/**".to_string();
    let replies = session.get(&admin_key).await?;

    while let Ok(reply) = replies.recv_async().await {
        if let Ok(sample) = reply.result() {
            let key = sample.key_expr().as_str().to_string();
            let payload_str = sample
                .payload()
                .try_to_string()
                .unwrap_or_else(|e| e.to_string().into());

            // Try to parse the admin response for key expressions
            if let Ok(value) = serde_json::from_str::<serde_json::Value>(&payload_str) {
                // Admin space responses vary — extract what we can
                tracing::debug!(key = %key, "admin response: {}", value);
            }

            topics.push(TopicInfo { key_expr: key });
        }
    }

    // Also try a direct GET on the user-provided key expression
    // to find queryables that respond
    let replies = session
        .get(key_expr)
        .timeout(std::time::Duration::from_secs(2))
        .await?;

    while let Ok(reply) = replies.recv_async().await {
        if let Ok(sample) = reply.result() {
            let key = sample.key_expr().as_str().to_string();
            if !topics.iter().any(|t| t.key_expr == key) {
                topics.push(TopicInfo { key_expr: key });
            }
        }
    }

    // Also use liveliness to discover active tokens
    let replies = session.liveliness().get(key_expr).await?;
    while let Ok(reply) = replies.recv_async().await {
        if let Ok(sample) = reply.result() {
            let key = sample.key_expr().as_str().to_string();
            if !topics.iter().any(|t| t.key_expr == key) {
                topics.push(TopicInfo { key_expr: key });
            }
        }
    }

    topics.sort_by(|a, b| a.key_expr.cmp(&b.key_expr));
    Ok(topics)
}

fn extract_source_zid(sample: &zenoh::sample::Sample) -> Option<String> {
    sample
        .source_info()
        .map(|info| {
            let gid = info.source_id();
            let zid = gid.zid();
            format!("{}", zid)
        })
        .filter(|s| s != "00000000000000000000000000000000")
}

/// Query liveliness tokens matching the given key expression.
pub async fn query_liveliness(
    session: &Session,
    key_expr: &str,
) -> Result<Vec<LivelinessToken>, ZenmonError> {
    let mut tokens = Vec::new();
    let replies = session
        .liveliness()
        .get(key_expr)
        .timeout(std::time::Duration::from_secs(3))
        .await?;

    while let Ok(reply) = replies.recv_async().await {
        match reply.result() {
            Ok(sample) => {
                let source_zid = extract_source_zid(sample);
                tokens.push(LivelinessToken {
                    key_expr: sample.key_expr().as_str().to_string(),
                    source_zid,
                    alive: true,
                });
            }
            Err(err) => {
                tracing::debug!("liveliness error reply: {:?}", err);
            }
        }
    }
    tokens.sort_by(|a, b| a.key_expr.cmp(&b.key_expr));
    Ok(tokens)
}

/// Report this session's transports opening and closing on `tx`, starting
/// with the ones already open. Runs until the session closes or `tx` is
/// dropped.
pub async fn subscribe_transport_events(
    session: &Session,
    tx: mpsc::UnboundedSender<TransportChange>,
) -> Result<(), ZenmonError> {
    let listener = session
        .info()
        .transport_events_listener()
        .history(true)
        .await?;
    tokio::spawn(async move {
        while let Ok(event) = listener.recv_async().await {
            let t = event.transport();
            let change = TransportChange {
                zid: t.zid().to_string(),
                whatami: t.whatami().to_string(),
                opened: matches!(event.kind(), SampleKind::Put),
            };
            if tx.send(change).is_err() {
                break;
            }
        }
    });
    Ok(())
}

/// Subscribe to liveliness changes. Sends join/leave events through the channel.
/// Returns Ok(()) when the subscriber is set up; events arrive on `tx`.
pub async fn subscribe_liveliness(
    session: &Session,
    key_expr: &str,
    tx: mpsc::UnboundedSender<LivelinessEvent>,
) -> Result<(), ZenmonError> {
    let subscriber = session
        .liveliness()
        .declare_subscriber(key_expr)
        .history(true)
        .await?;

    tokio::spawn(async move {
        while let Ok(sample) = subscriber.recv_async().await {
            let source_zid = extract_source_zid(&sample);
            tracing::debug!(
                "liveliness event: key={} kind={:?} source_zid={:?}",
                sample.key_expr(),
                sample.kind(),
                source_zid
            );
            let token = LivelinessToken {
                key_expr: sample.key_expr().as_str().to_string(),
                source_zid,
                alive: matches!(sample.kind(), SampleKind::Put),
            };
            let event = match sample.kind() {
                SampleKind::Put => LivelinessEvent::Join(token),
                SampleKind::Delete => LivelinessEvent::Leave(token),
            };
            if tx.send(event).is_err() {
                break;
            }
        }
    });

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    async fn peer(listen: &str, connect: &str) -> Session {
        let mut cfg = zenoh::Config::default();
        cfg.insert_json5("mode", "\"peer\"").unwrap();
        cfg.insert_json5("listen/endpoints", listen).unwrap();
        cfg.insert_json5("connect/endpoints", connect).unwrap();
        cfg.insert_json5("scouting/multicast/enabled", "false")
            .unwrap();
        cfg.insert_json5("scouting/gossip/enabled", "false")
            .unwrap();
        zenoh::open(cfg).await.unwrap()
    }

    async fn next(rx: &mut mpsc::UnboundedReceiver<TransportChange>) -> TransportChange {
        tokio::time::timeout(Duration::from_secs(10), rx.recv())
            .await
            .expect("timed out waiting for a transport event")
            .expect("channel closed")
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_peer_connecting_and_leaving_is_reported() {
        let hub = peer(r#"["tcp/127.0.0.1:0"]"#, "[]").await;
        let endpoint = hub
            .info()
            .locators()
            .await
            .into_iter()
            .map(|l| l.to_string())
            .find(|l| l.starts_with("tcp/"))
            .unwrap();
        let (tx, mut rx) = mpsc::unbounded_channel();
        subscribe_transport_events(&hub, tx).await.unwrap();

        let other = peer("[]", &format!(r#"["{endpoint}"]"#)).await;
        let other_zid = other.zid().to_string();
        let opened = next(&mut rx).await;
        assert_eq!(
            (opened.zid.as_str(), opened.opened),
            (other_zid.as_str(), true)
        );
        assert_eq!(opened.whatami, "peer");

        other.close().await.unwrap();
        let closed = next(&mut rx).await;
        assert_eq!(
            (closed.zid.as_str(), closed.opened),
            (other_zid.as_str(), false)
        );
    }
}
