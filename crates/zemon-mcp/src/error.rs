//! Map the core error taxonomy onto MCP tool errors, preserving `kind`.

use rmcp::ErrorData as McpError;
use zemon_core::error::ZemonError;

/// Convert a `ZemonError` into an MCP tool error. The human message is carried
/// verbatim and the stable `kind` is attached as structured data so agents see
/// the same taxonomy the CLI exposes.
pub fn to_mcp_error(e: ZemonError) -> McpError {
    let data = serde_json::json!({ "kind": e.kind_str() });
    McpError::internal_error(e.to_string(), Some(data))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preserves_kind_as_structured_data() {
        let err = to_mcp_error(ZemonError::invalid_input("bad key expr"));
        let data = err.data.expect("data must be present");
        assert_eq!(data["kind"], "invalid_input");
        assert_eq!(err.message.as_ref(), "bad key expr");
    }

    /// `rmcp::ErrorData` derives `Serialize` with `data: Option<Value>`
    /// (`skip_serializing_if = "Option::is_none"`), so the `kind` we attach
    /// rides along verbatim in the JSON actually sent over the wire. This
    /// covers that end-to-end serialization, distinct from the struct-field
    /// check above.
    #[test]
    fn maps_kind_into_error_data() {
        let mcp = to_mcp_error(ZemonError::invalid_input("bad".to_string()));
        let rendered = serde_json::to_value(&mcp).unwrap();
        assert!(rendered.to_string().contains("invalid_input"));
        assert_eq!(rendered["data"]["kind"], "invalid_input");
    }
}
