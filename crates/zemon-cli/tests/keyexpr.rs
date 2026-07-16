//! Deterministic (no-router) integration tests for the `keyexpr` command,
//! which opens no session and runs entirely offline.

use std::process::Command;

fn zemon() -> Command {
    Command::new(env!("CARGO_BIN_EXE_zemon"))
}

#[test]
fn keyexpr_reports_inclusion_direction() {
    let out = zemon()
        .args(["--json", "keyexpr", "a/*", "a/b"])
        .output()
        .expect("failed to run zemon");

    assert!(out.status.success(), "expected exit 0");
    let v: serde_json::Value =
        serde_json::from_slice(&out.stdout).expect("stdout must be valid JSON");
    assert_eq!(v["a_includes_b"], true);
    assert_eq!(v["b_includes_a"], false);
    assert_eq!(v["intersects"], true);
    assert_eq!(v["relation"], "a_includes_b");
}

#[test]
fn keyexpr_invalid_input_exits_two() {
    let out = zemon()
        .args(["--json", "keyexpr", "a//b", "x"])
        .output()
        .expect("failed to run zemon");

    assert_eq!(out.status.code(), Some(2), "invalid_input should exit 2");
    let stderr = String::from_utf8(out.stderr).unwrap();
    let v: serde_json::Value =
        serde_json::from_str(stderr.trim()).expect("stderr must be JSON error");
    assert_eq!(v["error"]["kind"], "invalid_input");
}
