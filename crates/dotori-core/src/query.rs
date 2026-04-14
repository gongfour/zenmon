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

                let payload_bytes = sample.payload().to_bytes();
                let msg_payload =
                    match serde_json::from_slice::<serde_json::Value>(&payload_bytes) {
                        Ok(json) => MessagePayload::Json(json),
                        Err(_) => MessagePayload::Raw {
                            bytes_len: payload_bytes.len(),
                        },
                    };

                let timestamp = sample.timestamp().map(|ts| ts.to_string());

                results.push(ZenohMessage {
                    key_expr: key,
                    payload: msg_payload,
                    timestamp,
                    kind,
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
