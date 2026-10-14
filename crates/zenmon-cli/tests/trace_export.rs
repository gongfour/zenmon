//! File-only export uses the real CLI and never needs a reachable router.
use std::{
    path::PathBuf,
    process::Command,
    sync::atomic::{AtomicU64, Ordering},
};
struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        static N: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "zenmon-export-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&path).unwrap();
        let mut lines = String::new();
        for n in 0..3 {
            lines.push_str(&serde_json::json!({"schema_version":2,"key_expr":"sensor/state","payload_base64":"AP8=","attachment_base64":"/wA=","encoding":"zenoh/bytes","kind":"PUT","received_at":format!("2026-08-06T00:00:0{n}Z"),"received_offset_ms":n*1000}).to_string());
            lines.push('\n');
        }
        std::fs::write(path.join("source.ndjson"), lines).unwrap();
        Self(path)
    }
    fn command(&self) -> Command {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_zenmon"));
        cmd.current_dir(&self.0)
            .args(["-e", "tcp/127.0.0.1:1", "--json"]);
        cmd
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
#[test]
fn exports_verify_and_no_clobber_through_cli() {
    let f = Fixture::new();
    let out = f
        .command()
        .args([
            "trace",
            "export",
            "source.ndjson",
            "--output",
            "slice.ndjson",
            "--since",
            "2026-08-06T00:00:01Z",
            "--until",
            "2026-08-06T00:00:02Z",
        ])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(out.stderr.is_empty());
    let report: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(report["records"], 1);
    let bytes = std::fs::read(f.0.join("slice.ndjson")).unwrap();
    let out = f
        .command()
        .args(["trace", "verify", "slice.ndjson"])
        .output()
        .unwrap();
    assert!(out.status.success());
    let report: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(report["valid"], true);
    assert_eq!(report["records"], 1);
    let out = f
        .command()
        .args([
            "trace",
            "export",
            "source.ndjson",
            "--output",
            "slice.ndjson",
        ])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
    assert!(out.stdout.is_empty());
    let error: serde_json::Value = serde_json::from_slice(&out.stderr).unwrap();
    assert_eq!(error["error"]["kind"], "invalid_input");
    assert_eq!(std::fs::read(f.0.join("slice.ndjson")).unwrap(), bytes);
}
#[test]
fn invalid_windows_fail_before_creating_output() {
    let f = Fixture::new();
    for bounds in [
        ["+1s", "+2s"],
        ["2026-08-06T00:00:02Z", "2026-08-06T00:00:01Z"],
    ] {
        let out = f
            .command()
            .args([
                "trace",
                "export",
                "source.ndjson",
                "--output",
                "slice.ndjson",
                "--since",
                bounds[0],
                "--until",
                bounds[1],
            ])
            .output()
            .unwrap();
        assert_eq!(out.status.code(), Some(2));
        assert!(!f.0.join("slice.ndjson").exists());
    }
}
#[cfg(feature = "tui")]
#[test]
fn offline_filters_require_trace_and_validate_before_terminal_setup() {
    let f = Fixture::new();
    assert_eq!(
        f.command()
            .args(["tui", "--key", "sensor/**"])
            .output()
            .unwrap()
            .status
            .code(),
        Some(2)
    );
    let out = f
        .command()
        .args([
            "tui",
            "--trace",
            "source.ndjson",
            "--since",
            "+2s",
            "--until",
            "+1s",
        ])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
    let error: serde_json::Value = serde_json::from_slice(&out.stderr).unwrap();
    assert_eq!(error["error"]["kind"], "invalid_input");
}
