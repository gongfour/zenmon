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
}
