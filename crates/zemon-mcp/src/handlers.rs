//! Per-tool logic, deliberately free of rmcp types so it is unit-testable.
//! Every function returns a JSON string on success (the exact shape the CLI
//! `--json` mode produces) or a `ZemonError` on failure.

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
    fn config_show_json_is_allow_listed_effective_config() {
        let json = config_show_json().unwrap();
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        // Effective view exposes the allow-list only; never raw Zenoh secrets.
        assert!(v.get("endpoint").is_some());
        assert!(v.get("connect_timeout").is_some());
        assert!(v.get("password").is_none());
    }
}
