//! One-shot network diagnostics.
//!
//! `doctor` runs a fixed sequence of checks (config → session → connection →
//! admin) under a single overall deadline and reports each as `pass | warn |
//! fail` with a stable machine `code` and a human `hint`. It reuses the typed
//! errors (#6), the configured connection mode (#10), and never mutates network
//! state.

use crate::config::{ConnectMode, ZemonConfig};
use crate::error::ErrorKind;
use crate::session::open_session;
use serde::Serialize;
use std::time::{Duration, Instant};

/// Result of a single diagnostic check.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum CheckStatus {
    Pass,
    Warn,
    Fail,
}

/// A single diagnostic step.
#[derive(Debug, Clone, Serialize)]
pub struct Check {
    pub name: String,
    pub status: CheckStatus,
    pub latency_ms: u64,
    /// Stable machine-readable code (safe for tests / branching).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hint: Option<String>,
}

impl Check {
    fn ms(latency: Duration) -> u64 {
        latency.as_millis() as u64
    }

    pub fn pass(name: &str, latency: Duration, message: Option<String>) -> Self {
        Self {
            name: name.to_string(),
            status: CheckStatus::Pass,
            latency_ms: Self::ms(latency),
            code: None,
            message,
            hint: None,
        }
    }

    pub fn warn(name: &str, latency: Duration, code: &str, message: &str, hint: &str) -> Self {
        Self {
            name: name.to_string(),
            status: CheckStatus::Warn,
            latency_ms: Self::ms(latency),
            code: Some(code.to_string()),
            message: Some(message.to_string()),
            hint: Some(hint.to_string()),
        }
    }

    pub fn fail(name: &str, latency: Duration, code: &str, message: &str, hint: &str) -> Self {
        Self {
            name: name.to_string(),
            status: CheckStatus::Fail,
            latency_ms: Self::ms(latency),
            code: Some(code.to_string()),
            message: Some(message.to_string()),
            hint: Some(hint.to_string()),
        }
    }
}

/// The full diagnostic report.
#[derive(Debug, Clone, Serialize)]
pub struct DoctorReport {
    pub status: CheckStatus,
    pub checks: Vec<Check>,
}

impl DoctorReport {
    pub fn new(checks: Vec<Check>) -> Self {
        Self {
            status: overall_status(&checks),
            checks,
        }
    }

    /// Exit code: `0` for pass/warn (warnings alone are not failures); for a
    /// failing report, the first failing check's `code` maps to a stable
    /// non-zero code consistent with the typed-error exit codes (#10).
    pub fn exit_code(&self) -> i32 {
        if self.status != CheckStatus::Fail {
            return 0;
        }
        for c in &self.checks {
            if c.status == CheckStatus::Fail {
                return match c.code.as_deref() {
                    Some("config_invalid") => 2,
                    Some("router_unreachable") | Some("session_connection") => 3,
                    Some("deadline_exceeded") => 4,
                    _ => 1,
                };
            }
        }
        1
    }
}

/// Overall status: fail if any check failed, else warn if any warned, else pass.
pub fn overall_status(checks: &[Check]) -> CheckStatus {
    if checks.iter().any(|c| c.status == CheckStatus::Fail) {
        CheckStatus::Fail
    } else if checks.iter().any(|c| c.status == CheckStatus::Warn) {
        CheckStatus::Warn
    } else {
        CheckStatus::Pass
    }
}

/// Judge the connection health for the *configured* mode (not a guess from
/// router presence). A client needs a router; a peer with zero routers is fine.
pub fn evaluate_connection(
    mode: ConnectMode,
    routers: usize,
    peers: usize,
    latency: Duration,
) -> Check {
    let summary = format!("{} router(s), {} peer(s)", routers, peers);
    match mode {
        ConnectMode::Client => {
            if routers > 0 {
                Check::pass("connection", latency, Some(summary))
            } else {
                Check::fail(
                    "connection",
                    latency,
                    "router_unreachable",
                    "no router connected in client mode",
                    "start a router (zenohd) and check --endpoint",
                )
            }
        }
        // A peer is healthy with zero routers.
        ConnectMode::Peer => Check::pass("connection", latency, Some(summary)),
    }
}

fn session_fail_code(kind: ErrorKind) -> &'static str {
    match kind {
        ErrorKind::Timeout => "deadline_exceeded",
        _ => "session_connection",
    }
}

/// Run all diagnostics under a single overall deadline.
pub async fn run(config: &ZemonConfig, timeout: Duration) -> DoctorReport {
    let deadline = Instant::now() + timeout;
    let mut checks = Vec::new();

    // 1. Config resolution / validity.
    let t = Instant::now();
    if let Err(e) = config.to_zenoh_config() {
        checks.push(Check::fail(
            "config",
            t.elapsed(),
            "config_invalid",
            &e.to_string(),
            "check --config, --endpoint, --mode and namespace",
        ));
        return DoctorReport::new(checks);
    }
    checks.push(Check::pass("config", t.elapsed(), None));

    // 2. Session open, bounded by the remaining deadline.
    let t = Instant::now();
    let remaining = deadline.saturating_duration_since(Instant::now());
    let mut cfg = config.clone();
    if cfg.connect_timeout.is_none() {
        cfg.connect_timeout = Some(remaining);
    }
    let session = match tokio::time::timeout(remaining, open_session(&cfg)).await {
        Err(_) => {
            checks.push(Check::fail(
                "session",
                t.elapsed(),
                "deadline_exceeded",
                "opening the session exceeded the deadline",
                "increase --timeout or check the endpoint",
            ));
            return DoctorReport::new(checks);
        }
        Ok(Err(e)) => {
            checks.push(Check::fail(
                "session",
                t.elapsed(),
                session_fail_code(e.kind),
                &e.message,
                "check that the endpoint is reachable and a router is running",
            ));
            return DoctorReport::new(checks);
        }
        Ok(Ok(s)) => {
            checks.push(Check::pass("session", t.elapsed(), None));
            s
        }
    };

    // 3. Connection health for the configured mode.
    let t = Instant::now();
    match crate::info::session_info(&session, config.mode).await {
        Ok(detail) => {
            checks.push(evaluate_connection(
                config.mode,
                detail.routers.len(),
                detail.peers.len(),
                t.elapsed(),
            ));
        }
        Err(e) => checks.push(Check::fail(
            "connection",
            t.elapsed(),
            "session_connection",
            &e.to_string(),
            "session info was unavailable",
        )),
    }

    // 4. Admin space reachability — a warning, not a failure (a healthy router
    //    may not expose or permit admin queries).
    let t = Instant::now();
    let remaining = deadline.saturating_duration_since(Instant::now());
    match tokio::time::timeout(remaining, crate::registry::query_admin_nodes(&session)).await {
        Ok(Ok(nodes)) => checks.push(Check::pass(
            "admin",
            t.elapsed(),
            Some(format!("{} node(s)", nodes.len())),
        )),
        Ok(Err(_)) => checks.push(Check::warn(
            "admin",
            t.elapsed(),
            "admin_unavailable",
            "admin space query failed",
            "the router may not expose or permit admin queries",
        )),
        Err(_) => checks.push(Check::warn(
            "admin",
            t.elapsed(),
            "admin_timeout",
            "admin query exceeded the deadline",
            "increase --timeout",
        )),
    }

    let _ = session.close().await;
    DoctorReport::new(checks)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn check(status: CheckStatus, code: Option<&str>) -> Check {
        Check {
            name: "x".into(),
            status,
            latency_ms: 0,
            code: code.map(|c| c.to_string()),
            message: None,
            hint: None,
        }
    }

    #[test]
    fn overall_is_fail_if_any_fails() {
        let checks = vec![
            check(CheckStatus::Pass, None),
            check(CheckStatus::Warn, Some("w")),
            check(CheckStatus::Fail, Some("router_unreachable")),
        ];
        assert_eq!(overall_status(&checks), CheckStatus::Fail);
    }

    #[test]
    fn overall_is_warn_when_only_warnings() {
        let checks = vec![
            check(CheckStatus::Pass, None),
            check(CheckStatus::Warn, Some("admin_unavailable")),
        ];
        assert_eq!(overall_status(&checks), CheckStatus::Warn);
        // warnings alone exit 0
        assert_eq!(DoctorReport::new(checks).exit_code(), 0);
    }

    #[test]
    fn client_without_router_fails_connection() {
        let c = evaluate_connection(ConnectMode::Client, 0, 0, Duration::ZERO);
        assert_eq!(c.status, CheckStatus::Fail);
        assert_eq!(c.code.as_deref(), Some("router_unreachable"));
    }

    #[test]
    fn client_with_router_passes() {
        let c = evaluate_connection(ConnectMode::Client, 1, 0, Duration::ZERO);
        assert_eq!(c.status, CheckStatus::Pass);
    }

    #[test]
    fn peer_without_router_is_healthy() {
        let c = evaluate_connection(ConnectMode::Peer, 0, 2, Duration::ZERO);
        assert_eq!(c.status, CheckStatus::Pass);
    }

    #[test]
    fn exit_code_maps_router_unreachable_to_three() {
        let report = DoctorReport::new(vec![
            check(CheckStatus::Pass, None),
            check(CheckStatus::Fail, Some("router_unreachable")),
        ]);
        assert_eq!(report.exit_code(), 3);
    }

    #[test]
    fn exit_code_maps_config_invalid_to_two() {
        let report = DoctorReport::new(vec![check(CheckStatus::Fail, Some("config_invalid"))]);
        assert_eq!(report.exit_code(), 2);
    }
}
