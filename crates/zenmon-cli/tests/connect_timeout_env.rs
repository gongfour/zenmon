//! Integration tests for `ZENMON_CONNECT_TIMEOUT` validation.
//!
//! The env var used to bypass the CLI's positive/precision validation: `0s`
//! became `Duration::ZERO`, sub-millisecond values silently rounded to `0ms`,
//! and malformed values were ignored (behaving like the default). It must now
//! be validated identically to the `--connect-timeout` flag: an invalid value
//! is a hard `invalid_input` error (exit code 2).

use std::process::Command;

/// Run `zenmon --json info` with the given `ZENMON_CONNECT_TIMEOUT`. An *invalid*
/// value is rejected at validation as `invalid_input` (exit code 2) before any
/// connection is attempted, regardless of whether a router is reachable.
fn run_with_timeout(value: &str) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_zenmon"))
        .env("ZENMON_CONNECT_TIMEOUT", value)
        .args(["--json", "info"])
        .output()
        .expect("failed to run zenmon")
}

fn error_kind(out: &std::process::Output) -> String {
    let stderr = String::from_utf8(out.stderr.clone()).expect("stderr utf8");
    let v: serde_json::Value =
        serde_json::from_str(stderr.trim_end_matches('\n')).expect("stderr must be JSON");
    v["error"]["kind"].as_str().unwrap_or_default().to_string()
}

#[test]
fn zero_env_timeout_is_rejected() {
    let out = run_with_timeout("0s");
    assert_eq!(out.status.code(), Some(2), "invalid_input exit code");
    assert_eq!(error_kind(&out), "invalid_input");
}

#[test]
fn sub_millisecond_env_timeout_is_rejected() {
    let out = run_with_timeout("1ns");
    assert_eq!(out.status.code(), Some(2));
    assert_eq!(error_kind(&out), "invalid_input");
}

#[test]
fn malformed_env_timeout_is_rejected_not_ignored() {
    let out = run_with_timeout("not-a-duration");
    assert_eq!(out.status.code(), Some(2));
    assert_eq!(error_kind(&out), "invalid_input");
}

#[test]
fn oversized_env_timeout_is_rejected() {
    let out = run_with_timeout("100days");
    assert_eq!(out.status.code(), Some(2));
    assert_eq!(error_kind(&out), "invalid_input");
}

#[test]
fn valid_env_timeout_passes_validation() {
    // A valid timeout must not be rejected as invalid_input (exit code 2).
    // Whether `info` then connects (exit 0, if a router is reachable) or fails
    // to connect (exit 3, if not) is environment-dependent and irrelevant here —
    // both outcomes prove the value passed validation.
    let out = run_with_timeout("5s");
    assert_ne!(
        out.status.code(),
        Some(2),
        "a valid timeout must not be rejected as invalid_input (got exit {:?}, stderr: {})",
        out.status.code(),
        String::from_utf8_lossy(&out.stderr),
    );
}
