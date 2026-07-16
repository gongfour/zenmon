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

/// `config_show` tool: the effective, allow-listed configuration (no session).
pub fn config_show_json() -> Result<String, ZemonError> {
    let resolved = zemon_core::config::resolve_config(Default::default())
        .map_err(|e| ZemonError::invalid_input(e.to_string()))?;
    serde_json::to_string(&resolved.effective)
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
    zemon_core::output::to_collection_json(&replies)
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
        let json = config_show_json().unwrap();
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        // Effective view exposes the allow-list only; never raw Zenoh secrets.
        assert!(v.get("endpoint").is_some());
        assert!(v.get("connect_timeout").is_some());
        assert!(v.get("password").is_none());
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
