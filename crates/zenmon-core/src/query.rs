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

/// One bound of a `_time` range: a relative age (`10m`, `1h`, `90s`) becomes
/// `now(-<secs>s)`, resolved where the storage evaluates it; an RFC3339
/// instant passes through.
fn time_expr(bound: &str) -> Result<String, ZenmonError> {
    let t = bound.trim();
    if let Ok(age) = humantime::parse_duration(t) {
        return Ok(format!("now(-{}s)", age.as_secs_f64()));
    }
    humantime::parse_rfc3339_weak(t).map_err(|e| {
        ZenmonError::invalid_input(format!(
            "invalid time '{bound}': {e} (try 10m or an RFC3339 timestamp)"
        ))
    })?;
    Ok(t.to_string())
}

/// `key_expr` with a Zenoh `_time` selector parameter for `[since..until]`,
/// which storages with history answer with every value in the window
/// instead of the latest. Either bound may be open. Existing parameters are
/// kept (Zenoh separates them with `;`).
pub fn with_time_range(
    key_expr: &str,
    since: Option<&str>,
    until: Option<&str>,
) -> Result<String, ZenmonError> {
    if since.is_none() && until.is_none() {
        return Ok(key_expr.to_string());
    }
    let start = since.map(time_expr).transpose()?.unwrap_or_default();
    let end = until.map(time_expr).transpose()?.unwrap_or_default();
    let param = format!("_time=[{start}..{end}]");
    Ok(match key_expr.split_once('?') {
        Some((_, params)) if !params.is_empty() => format!("{key_expr};{param}"),
        Some((key, _)) => format!("{key}?{param}"),
        None => format!("{key_expr}?{param}"),
    })
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
                let attachment = sample.attachment().map(MessagePayload::from_zbytes);

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
                    qos: None,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_time_range_becomes_a_selector_parameter() {
        assert_eq!(with_time_range("a/**", None, None).unwrap(), "a/**");
        assert_eq!(
            with_time_range("a/**", Some("10m"), None).unwrap(),
            "a/**?_time=[now(-600s)..]"
        );
        assert_eq!(
            with_time_range("a/b?x=1", Some("2026-10-11T10:00:00Z"), Some("90s")).unwrap(),
            "a/b?x=1;_time=[2026-10-11T10:00:00Z..now(-90s)]"
        );
        assert!(with_time_range("a", Some("yesterday"), None).is_err());
    }

    #[test]
    fn zenoh_parses_what_we_build() {
        let s = with_time_range("a/**", Some("1h"), Some("2026-10-11T10:00:00Z")).unwrap();
        let range = s.split_once("_time=").unwrap().1;
        assert!(range.parse::<zenoh::query::TimeRange>().is_ok(), "{range}");
    }
}
