use crate::types::{MessagePayload, ZenohMessage};
use color_eyre::eyre::eyre;
use color_eyre::Result;
use std::time::Duration;
use zenoh::Session;

/// Send a Zenoh GET query and collect replies.
///
/// When `limit` is `Some(n)`, collection stops after `n` replies (the output
/// budget is bounded at the network level); the caller detects the cap by
/// comparing the returned length to the limit.
pub async fn get(
    session: &Session,
    key_expr: &str,
    payload: Option<&str>,
    timeout: Duration,
    limit: Option<usize>,
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
                let encoding = sample.encoding().to_string();
                let timestamp = sample.timestamp().map(|ts| ts.to_string());
                let attachment = sample.attachment().map(|att| MessagePayload::from_zbytes(&att));

                let payload_bytes = msg_payload.len();
                let attachment_bytes = attachment.as_ref().map(|a| a.len());
                results.push(ZenohMessage {
                    key_expr: key,
                    payload: msg_payload,
                    encoding,
                    payload_bytes,
                    timestamp,
                    kind,
                    attachment,
                    attachment_bytes,
                });

                if let Some(l) = limit {
                    if results.len() >= l {
                        break;
                    }
                }
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
