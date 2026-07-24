use crate::error::ZenmonError;
use crate::types::{MessagePayload, ZenohMessage};
use serde::Serialize;
use std::time::Duration;
use zenoh::Session;

pub use zenoh::query::ConsolidationMode;

/// A Zenoh reply error returned by a queryable (e.g. a `call/*` RPC server that
/// rejected the request). Surfaced instead of silently dropped so that an
/// endpoint which exists but errors is distinguishable from one that never
/// replied.
#[derive(Debug, Clone, Serialize)]
pub struct QueryReplyError {
    pub message: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub encoding: String,
}

/// The outcome of a GET query: successful replies plus any reply errors.
pub struct QueryOutcome {
    pub replies: Vec<ZenohMessage>,
    pub errors: Vec<QueryReplyError>,
}

/// Send a Zenoh GET query and collect replies.
///
/// When `limit` is `Some(n)`, collection of successful replies stops after `n`
/// (the output budget is bounded at the network level); the caller detects the
/// cap by comparing `replies.len()` to the limit. Reply errors are collected
/// separately and never counted against the limit.
///
/// `consolidation` controls reply filtering: with the default (`Auto`) only
/// one reply per key survives, so when several queryables share a key
/// expression the fastest reply masks the rest. `None` delivers every reply.
pub async fn get(
    session: &Session,
    key_expr: &str,
    payload: Option<&str>,
    timeout: Duration,
    limit: Option<usize>,
    consolidation: ConsolidationMode,
) -> Result<QueryOutcome, ZenmonError> {
    let mut builder = session
        .get(key_expr)
        .timeout(timeout)
        .consolidation(consolidation);

    if let Some(p) = payload {
        builder = builder.payload(p.to_string());
    }

    let replies = builder.await?;
    let mut results = Vec::new();
    let mut errors = Vec::new();

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
                let message = err
                    .payload()
                    .try_to_string()
                    .map(|s| s.into_owned())
                    .unwrap_or_else(|e| e.to_string());
                let encoding = err.encoding().to_string();
                errors.push(QueryReplyError { message, encoding });
            }
        }
    }

    Ok(QueryOutcome {
        replies: results,
        errors,
    })
}
