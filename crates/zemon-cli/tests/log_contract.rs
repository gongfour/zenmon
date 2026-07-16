//! Integration test for the `--json` logging contract under `RUST_LOG`.
//!
//! `info` opens a Zenoh session; with no router it fails deterministically while
//! still driving the code paths that Zenoh logs from (including its full `Config`
//! debug output, which carries auth fields). Setting `RUST_LOG=trace` must NOT
//! leak any of that into stdout/stderr in `--json` mode: logs are forced off.

use std::process::Command;

fn zemon() -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_zemon"));
    // Would enable trace logging for every crate if the filter were not forced
    // off. This is exactly the reviewer's reproduction.
    cmd.env("RUST_LOG", "trace");
    cmd
}

#[test]
fn json_mode_suppresses_logs_even_with_rust_log_trace() {
    let out = zemon()
        .args(["--json", "info"])
        .output()
        .expect("failed to run zemon");

    let stdout = String::from_utf8(out.stdout).expect("stdout utf8");
    let stderr = String::from_utf8(out.stderr).expect("stderr utf8");

    for (stream, text) in [("stdout", &stdout), ("stderr", &stderr)] {
        // Tracing's fmt layer emits ANSI-colored, multi-line output; a clean
        // JSON contract never contains escape codes or level tokens.
        assert!(
            !text.contains('\u{1b}'),
            "{stream} must contain no ANSI escapes (log leak): {text:?}"
        );
        assert!(
            !text.contains("TRACE") && !text.contains("DEBUG"),
            "{stream} must contain no tracing level output (log leak): {text:?}"
        );
        // The specific sensitive leak: Zenoh dumping its Config (auth included).
        assert!(
            !text.contains("Config("),
            "{stream} must not expose Zenoh Config debug output: {text:?}"
        );
    }

    // stderr is at most the single structured JSON error line.
    let trimmed_err = stderr.trim_end_matches('\n');
    if !trimmed_err.is_empty() {
        assert_eq!(
            trimmed_err.lines().count(),
            1,
            "stderr must be a single JSON line: {trimmed_err:?}"
        );
        let v: serde_json::Value =
            serde_json::from_str(trimmed_err).expect("stderr must be valid JSON");
        assert!(v["error"]["kind"].is_string(), "expected an error envelope");
    }
}
