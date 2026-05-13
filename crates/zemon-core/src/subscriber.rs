use crate::types::{MessagePayload, ZenohMessage};
use color_eyre::eyre::eyre;
use color_eyre::Result;
use tokio::sync::mpsc;
use zenoh::Session;

/// Subscribe to a key expression and send messages to the provided channel.
/// Returns a JoinHandle that runs until the session is closed or an error occurs.
pub async fn subscribe(
    session: &Session,
    key_expr: &str,
    tx: mpsc::UnboundedSender<ZenohMessage>,
) -> Result<tokio::task::JoinHandle<()>> {
    let subscriber = session.declare_subscriber(key_expr).await.map_err(|e| eyre!(e))?;
    tracing::info!(key_expr = %key_expr, "Subscribed");

    let handle = tokio::spawn(async move {
        while let Ok(sample) = subscriber.recv_async().await {
            let key = sample.key_expr().as_str().to_string();
            let kind = format!("{}", sample.kind());
            let payload = MessagePayload::from_zbytes(sample.payload());
            let timestamp = sample.timestamp().map(|ts| ts.to_string());
            let attachment = sample.attachment().map(|att| MessagePayload::from_zbytes(&att));

            let msg = ZenohMessage {
                key_expr: key,
                payload,
                timestamp,
                kind,
                attachment,
            };

            if tx.send(msg).is_err() {
                break;
            }
        }
    });

    Ok(handle)
}
