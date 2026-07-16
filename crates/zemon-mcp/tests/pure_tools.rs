//! End-to-end checks for tools that need no network session.

use zemon_mcp::handlers;

#[test]
fn keyexpr_and_config_show_work_without_a_session() {
    let ke = handlers::keyexpr_json("a/*", "a/b").unwrap();
    assert!(serde_json::from_str::<serde_json::Value>(&ke).unwrap()["a_includes_b"]
        .as_bool()
        .unwrap());

    let cfg = handlers::config_show_json().unwrap();
    assert!(serde_json::from_str::<serde_json::Value>(&cfg).unwrap()["endpoint"].is_object());
}

/// Asserts the full 11-tool surface is registered on the router, by
/// unit-constructing the server (no session, no zenohd needed) and reading
/// `ZemonMcpServer::list_tool_names()` (backed by `ToolRouter::list_all`).
///
/// This is the protocol-level-adjacent check; the actual `tools/list`
/// stdio round-trip was verified manually against a running client (see
/// Task 1 Step 11) and is not re-automated here since rmcp 2.2.0's
/// in-memory/duplex transport setup adds significant test-harness weight
/// for a check this unit test already covers structurally.
#[test]
fn all_expected_tools_are_registered() {
    use std::sync::Arc;
    let state = Arc::new(zemon_mcp::state::ServerState::new(Default::default()));
    let server = zemon_mcp::server::ZemonMcpServer::new(state);
    let names = server.list_tool_names();
    assert_eq!(names.len(), 11, "expected exactly 11 tools, got {names:?}");
    for expected in [
        "keyexpr",
        "config_show",
        "discover",
        "info",
        "query",
        "nodes",
        "liveliness",
        "scout",
        "doctor",
        "sub_snapshot",
        "pub",
    ] {
        assert!(names.iter().any(|n| n == expected), "missing tool {expected}");
    }
}
