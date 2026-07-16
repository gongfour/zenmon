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
