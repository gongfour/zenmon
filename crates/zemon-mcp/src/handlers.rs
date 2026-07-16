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
}
