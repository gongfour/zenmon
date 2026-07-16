//! Per-tool logic, deliberately free of rmcp types so it is unit-testable.
//! Every function returns a JSON string on success (the exact shape the CLI
//! `--json` mode produces) or a `ZemonError` on failure.

use std::time::Duration;
use zemon_core::error::ZemonError;

/// `keyexpr` tool: pure key-expression relationship comparison (no session).
pub fn keyexpr_json(a: &str, b: &str) -> Result<String, ZemonError> {
    let relation = zemon_core::keyexpr::compare(a, b)?;
    serde_json::to_string(&relation)
        .map_err(|e| ZemonError::internal(format!("serialize keyexpr result: {e}")))
}

/// `discover` tool: list active keys/topics for `key_expr`.
pub async fn discover_json(
    session: &zenoh::Session,
    key_expr: &str,
) -> Result<String, ZemonError> {
    let topics = zemon_core::discover::discover(session, key_expr)
        .await
        .map_err(ZemonError::from)?;
    zemon_core::output::to_collection_json(&topics)
        .map_err(|e| ZemonError::internal(format!("serialize discover result: {e}")))
}

/// `config_show` tool: the effective, allow-listed configuration (no
/// network). Serializes the `effective` view the server was actually started
/// with (threaded in from `ServerState`), rather than re-resolving from
/// env/config-file — a re-resolve would ignore the CLI global flags
/// (`-e/-m/-c/--scout-port/--connect-timeout`) the session was built from and
/// disagree with `info`.
pub fn config_show_json(
    effective: &zemon_core::config::EffectiveConfig,
) -> Result<String, ZemonError> {
    serde_json::to_string(effective)
        .map_err(|e| ZemonError::internal(format!("serialize effective config: {e}")))
}

/// `info` tool: current session details.
pub async fn info_json(
    session: &zenoh::Session,
    mode: zemon_core::config::ConnectMode,
) -> Result<String, ZemonError> {
    let detail = zemon_core::info::session_info(session, mode)
        .await
        .map_err(|e| ZemonError::internal(e.to_string()))?;
    serde_json::to_string(&detail)
        .map_err(|e| ZemonError::internal(format!("serialize info: {e}")))
}

/// `query` tool: send a GET and collect replies (bounded by `timeout`/`limit`).
pub async fn query_json(
    session: &zenoh::Session,
    key_expr: &str,
    payload: Option<&str>,
    timeout: Duration,
    limit: Option<usize>,
) -> Result<String, ZemonError> {
    let replies = zemon_core::query::get(session, key_expr, payload, timeout, limit)
        .await
        .map_err(|e| ZemonError::internal(e.to_string()))?;
    let limited = limit.is_some_and(|l| replies.len() >= l);
    zemon_core::output::to_collection_json_limited(&replies, limited)
        .map_err(|e| ZemonError::internal(format!("serialize query result: {e}")))
}

/// `nodes` tool: one snapshot of discovered Zenoh nodes (admin space).
pub async fn nodes_json(session: &zenoh::Session) -> Result<String, ZemonError> {
    let nodes = zemon_core::registry::query_admin_nodes(session)
        .await
        .map_err(|e| ZemonError::internal(e.to_string()))?;
    zemon_core::output::to_collection_json(&nodes)
        .map_err(|e| ZemonError::internal(format!("serialize nodes result: {e}")))
}

/// `liveliness` tool: query current liveliness tokens.
pub async fn liveliness_json(
    session: &zenoh::Session,
    key_expr: &str,
) -> Result<String, ZemonError> {
    let tokens = zemon_core::discover::query_liveliness(session, key_expr)
        .await
        .map_err(|e| ZemonError::internal(e.to_string()))?;
    zemon_core::output::to_collection_json(&tokens)
        .map_err(|e| ZemonError::internal(format!("serialize liveliness result: {e}")))
}

/// `scout` tool: multicast scan of a port range (does its own transient
/// sessions, no shared session). Mirrors the CLI's `scout --json` shape,
/// which filters to ports that found nodes rather than serializing every
/// scanned port.
pub async fn scout_json(
    config: &zemon_core::config::ZemonConfig,
    per_port_timeout: Duration,
    port_range: (u16, u16),
) -> Result<String, ZemonError> {
    let results = zemon_core::scout::scout_port_range(
        config,
        port_range.0,
        port_range.1,
        per_port_timeout,
    )
    .await
    .map_err(|e| ZemonError::internal(e.to_string()))?;
    let hits: Vec<_> = results.iter().filter(|r| !r.nodes.is_empty()).collect();
    zemon_core::output::to_collection_json(&hits)
        .map_err(|e| ZemonError::internal(format!("serialize scout result: {e}")))
}

/// `doctor` tool: connection diagnostics. Opens its own transient session(s);
/// always returns a serializable report, even when checks fail.
pub async fn doctor_json(
    config: &zemon_core::config::ZemonConfig,
    timeout: Duration,
) -> Result<String, ZemonError> {
    let report = zemon_core::doctor::run(config, timeout).await;
    serde_json::to_string(&report)
        .map_err(|e| ZemonError::internal(format!("serialize doctor report: {e}")))
}

/// Precondition for `sub_snapshot`: at least one of `count`/`duration` must be
/// set, or the collection loop would never terminate. Extracted so it is
/// unit-testable without opening a session.
pub(crate) fn sub_snapshot_bound_error(
    count: Option<usize>,
    duration: Option<Duration>,
) -> Option<ZemonError> {
    if count.is_none() && duration.is_none() {
        Some(ZemonError::invalid_input(
            "sub_snapshot requires at least one of `count` or `duration`".to_string(),
        ))
    } else {
        None
    }
}

/// `sub_snapshot` tool: subscribe and collect messages until `count` is
/// reached or `duration` elapses (at least one bound is required), then
/// return a `{count, items}` batch. Mirrors the CLI's `sub --json
/// --max-payload-bytes` per-message capping (see `zemon-cli/src/main.rs`):
/// `to_collection_json_limited`'s second argument is a `limited: bool` flag
/// (whether results were truncated by e.g. `--limit`), not a byte cap, so it
/// is never used here for capping.
pub async fn sub_snapshot_json(
    session: &zenoh::Session,
    key_expr: &str,
    count: Option<usize>,
    duration: Option<Duration>,
    max_payload_bytes: Option<usize>,
) -> Result<String, ZemonError> {
    if let Some(e) = sub_snapshot_bound_error(count, duration) {
        return Err(e);
    }

    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let handle = zemon_core::subscriber::subscribe(session, key_expr, tx)
        .await
        .map_err(|e| ZemonError::internal(e.to_string()))?;

    let deadline = duration.map(|d| tokio::time::Instant::now() + d);
    let mut collected: Vec<zemon_core::types::ZenohMessage> = Vec::new();
    loop {
        if let Some(max) = count {
            if collected.len() >= max {
                break;
            }
        }
        let recv = match deadline {
            Some(dl) => match tokio::time::timeout_at(dl, rx.recv()).await {
                Ok(msg) => msg,
                Err(_) => break, // duration elapsed
            },
            None => rx.recv().await,
        };
        match recv {
            Some(msg) => collected.push(msg),
            None => break, // subscriber closed
        }
    }
    handle.abort();

    match max_payload_bytes {
        Some(cap) => {
            let views: Vec<serde_json::Value> = collected
                .iter()
                .map(|msg| -> Result<serde_json::Value, ZemonError> {
                    let mut v = serde_json::to_value(msg).map_err(|e| {
                        ZemonError::internal(format!("serialize sub snapshot item: {e}"))
                    })?;
                    v["payload"] = msg.payload.to_view_capped(cap);
                    if let Some(att) = &msg.attachment {
                        v["attachment"] = att.to_view_capped(cap);
                    }
                    Ok(v)
                })
                .collect::<Result<_, _>>()?;
            zemon_core::output::to_collection_json(&views)
        }
        None => zemon_core::output::to_collection_json(&collected),
    }
    .map_err(|e| ZemonError::internal(format!("serialize sub snapshot: {e}")))
}

/// `pub` tool: publish a value to a key expression (test injection; mutates
/// the network). Mirrors the CLI's `Command::Pub` arm exactly: the
/// attachment, if present, is attached as raw bytes (not re-wrapped as a
/// String), and `publish_accepted_json` takes the payload byte length plus
/// an optional attachment byte length as its third argument.
pub async fn pub_json(
    session: &zenoh::Session,
    key_expr: &str,
    value: &str,
    att: Option<&str>,
) -> Result<String, ZemonError> {
    let mut builder = session.put(key_expr, value.to_string());
    if let Some(att) = att {
        builder = builder.attachment(att.as_bytes());
    }
    builder
        .await
        .map_err(|e| ZemonError::internal(format!("publish failed: {e}")))?;
    let attachment_bytes = att.map(|a| a.len());
    zemon_core::output::publish_accepted_json(key_expr, value.len(), attachment_bytes)
        .map_err(|e| ZemonError::internal(format!("serialize pub result: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keyexpr_json_reports_inclusion() {
        let json = keyexpr_json("a/*", "a/b").unwrap();
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(v["a_includes_b"], true);
        assert_eq!(v["b_includes_a"], false);
    }

    #[test]
    fn keyexpr_json_rejects_invalid_expr() {
        assert!(keyexpr_json("a/**/**/c", "b").is_err());
    }

    #[test]
    fn discover_json_uses_count_items_envelope() {
        let empty: Vec<zemon_core::types::TopicInfo> = vec![];
        let json = zemon_core::output::to_collection_json(&empty).unwrap();
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(v["count"], 0);
        assert!(v["items"].as_array().unwrap().is_empty());
    }

    #[test]
    fn query_result_uses_count_items_envelope() {
        let empty: Vec<zemon_core::types::ZenohMessage> = vec![];
        let json = zemon_core::output::to_collection_json(&empty).unwrap();
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(v["count"], 0);
    }

    #[test]
    fn config_show_json_is_allow_listed_effective_config() {
        let effective = zemon_core::config::resolve_config(Default::default())
            .unwrap()
            .effective;
        let json = config_show_json(&effective).unwrap();
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        // Effective view exposes the allow-list only; never raw Zenoh secrets.
        assert!(v.get("endpoint").is_some());
        assert!(v.get("connect_timeout").is_some());
        assert!(v.get("password").is_none());
    }

    #[tokio::test]
    async fn sub_snapshot_requires_a_bound() {
        // We cannot open a real session here, but the bound check happens
        // before any session use, so assert the pure precondition directly.
        let err = super::sub_snapshot_bound_error(None, None);
        assert!(err.is_some());
        assert!(super::sub_snapshot_bound_error(Some(1), None).is_none());
        assert!(super::sub_snapshot_bound_error(None, Some(Duration::from_millis(1))).is_none());
    }

    #[test]
    fn pub_accepted_envelope_shape() {
        let json = zemon_core::output::publish_accepted_json("test/x", 5, None).unwrap();
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(v["ok"], true);
        assert_eq!(v["key_expr"], "test/x");
        assert_eq!(v["bytes"], 5);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn doctor_json_is_serializable() {
        // Uses a bogus endpoint + tiny timeout so it fails fast without a
        // router, but still produces a serializable report object.
        let mut config = zemon_core::config::ZemonConfig::default();
        config.endpoint = "tcp/127.0.0.1:1".to_string();
        let json = doctor_json(&config, Duration::from_millis(200))
            .await
            .unwrap();
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert!(v.is_object());
    }
}
