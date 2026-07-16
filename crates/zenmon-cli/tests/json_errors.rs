//! Deterministic (no-router) integration test for the `--json` error contract.
//!
//! A bad `--port-range` fails in `parse_port_range` before any Zenoh session is
//! opened, so this exercises the structured-error path without needing `zenohd`.

use std::process::Command;

fn zenmon() -> Command {
    Command::new(env!("CARGO_BIN_EXE_zenmon"))
}

#[test]
fn json_mode_emits_single_structured_error_on_bad_input() {
    let out = zenmon()
        .args(["--json", "scout", "--port-range", "not-a-range"])
        .output()
        .expect("failed to run zenmon");

    // Non-zero exit on error.
    assert!(!out.status.success(), "expected non-zero exit");

    let stderr = String::from_utf8(out.stderr).expect("stderr utf8");
    let trimmed = stderr.trim_end_matches('\n');

    // No ANSI escapes.
    assert!(
        !trimmed.contains('\u{1b}'),
        "stderr must contain no ANSI escapes: {trimmed:?}"
    );
    // Exactly one line.
    assert_eq!(
        trimmed.lines().count(),
        1,
        "stderr must be a single JSON line: {trimmed:?}"
    );

    // Parses as the error envelope with a stable kind.
    let v: serde_json::Value =
        serde_json::from_str(trimmed).expect("stderr must be valid JSON");
    assert_eq!(v["error"]["kind"], "invalid_input");
    assert!(
        v["error"]["message"].is_string(),
        "error.message must be a string"
    );

    // stdout stays empty on failure.
    assert!(out.stdout.is_empty(), "stdout must be empty on error");
}
