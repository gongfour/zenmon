use crate::types::{MessagePayload, ZenohMessage};
use color_eyre::eyre::eyre;
use color_eyre::Result;
use std::time::Duration;
use zenoh::Session;

/// Send a Zenoh GET query and collect all replies.
pub async fn get(
    session: &Session,
    key_expr: &str,
    payload: Option<&str>,
    timeout: Duration,
) -> Result<Vec<ZenohMessage>> {
    let mut builder = session.get(key_expr).timeout(timeout);

    if let Some(p) = payload {
        builder = builder.payload(p.to_string());
    }

    let replies = builder.await.map_err(|e| eyre!(e))?;
    let mut results = Vec::new();

    while let Ok(reply) = replies.recv_async().await {
        match reply.result() {
            Ok(sample) => {
                let key = sample.key_expr().as_str().to_string();
                let kind = format!("{}", sample.kind());
                let msg_payload = MessagePayload::from_zbytes(sample.payload());
                let timestamp = sample.timestamp().map(|ts| ts.to_string());
                let attachment = sample.attachment().map(|att| MessagePayload::from_zbytes(&att));

                results.push(ZenohMessage {
                    key_expr: key,
                    payload: msg_payload,
                    timestamp,
                    kind,
                    attachment,
                });
            }
            Err(err) => {
                let payload_str = err
                    .payload()
                    .try_to_string()
                    .unwrap_or_else(|e| e.to_string().into());
                tracing::warn!(error = %payload_str, "Query error reply");
            }
        }
    }

    Ok(results)
}
