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

            let payload_bytes = sample.payload().to_bytes();
            let payload = match serde_json::from_slice::<serde_json::Value>(&payload_bytes) {
                Ok(json) => MessagePayload::Json(json),
                Err(_) => MessagePayload::Raw {
                    bytes_len: payload_bytes.len(),
                },
            };

            let timestamp = sample
                .timestamp()
                .map(|ts| ts.to_string());

            let attachment = sample.attachment().map(|att| {
                let att_bytes = att.to_bytes();
                match serde_json::from_slice::<serde_json::Value>(&att_bytes) {
                    Ok(json) => MessagePayload::Json(json),
                    Err(_) => match att.try_to_string() {
                        Ok(s) => MessagePayload::Json(serde_json::Value::String(s.into_owned())),
                        Err(_) => MessagePayload::Raw { bytes_len: att_bytes.len() },
                    },
                }
            });

            let msg = ZenohMessage {
                key_expr: key,
                payload,
                timestamp,
                kind,
                attachment,
            };

            if tx.send(msg).is_err() {
                break; // receiver dropped
            }
        }
    });

    Ok(handle)
}
