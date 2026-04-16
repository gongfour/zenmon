use crate::types::{LivelinessEvent, LivelinessToken, TopicInfo};
use color_eyre::eyre::eyre;
use color_eyre::Result;
use tokio::sync::mpsc;
use zenoh::sample::SampleKind;
use zenoh::Session;

/// Discover active keys matching the given key expression.
/// Uses Zenoh admin space to list subscribers and publishers.
/// Falls back to a plain GET if admin space returns nothing.
pub async fn discover(session: &Session, key_expr: &str) -> Result<Vec<TopicInfo>> {
    let mut topics = Vec::new();

    // Query admin space for subscriber/publisher info
    let admin_key = format!("@/router/local/**");
    let replies = session.get(&admin_key).await.map_err(|e| eyre!(e))?;

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
        .await
        .map_err(|e| eyre!(e))?;

    while let Ok(reply) = replies.recv_async().await {
        if let Ok(sample) = reply.result() {
            let key = sample.key_expr().as_str().to_string();
            if !topics.iter().any(|t| t.key_expr == key) {
                topics.push(TopicInfo { key_expr: key });
            }
        }
    }

    // Also use liveliness to discover active tokens
    let replies = session.liveliness().get(key_expr).await.map_err(|e| eyre!(e))?;
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
) -> Result<Vec<LivelinessToken>> {
    let mut tokens = Vec::new();
    let replies = session
        .liveliness()
        .get(key_expr)
        .timeout(std::time::Duration::from_secs(3))
        .await
        .map_err(|e| eyre!(e))?;

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

/// Subscribe to liveliness changes. Sends join/leave events through the channel.
/// Returns Ok(()) when the subscriber is set up; events arrive on `tx`.
pub async fn subscribe_liveliness(
    session: &Session,
    key_expr: &str,
    tx: mpsc::UnboundedSender<LivelinessEvent>,
) -> Result<()> {
    let subscriber = session
        .liveliness()
        .declare_subscriber(key_expr)
        .history(true)
        .await
        .map_err(|e| eyre!(e))?;

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
