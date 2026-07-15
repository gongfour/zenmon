//! Typed errors for the core/CLI boundary.
//!
//! Zenoh's own errors don't implement `Into<color_eyre::Report>` and most
//! call sites flatten to string reports, so consumers can't reliably tell a
//! connection failure from an internal bug. `ZemonError` gives a stable,
//! machine-readable `kind` that the CLI maps to a single-line JSON error and
//! (in #10) to a stable exit code.

use serde::Serialize;
use std::fmt;

/// Stable, machine-readable error category.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorKind {
    /// Failed to reach / connect to the Zenoh network.
    Connection,
    /// An operation exceeded its deadline.
    Timeout,
    /// The user supplied invalid input (bad key expr, duration, range, ...).
    InvalidInput,
    /// A requested resource was not found.
    NotFound,
    /// An unexpected internal error (default for untyped failures).
    Internal,
}

/// A typed error carrying a stable `kind` and a human-readable message.
///
/// The message must not leak backtraces or sensitive config values.
#[derive(Debug, Clone)]
pub struct ZemonError {
    pub kind: ErrorKind,
    pub message: String,
}

impl ZemonError {
    pub fn new(kind: ErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }

    pub fn connection(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::Connection, message)
    }

    pub fn timeout(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::Timeout, message)
    }

    pub fn invalid_input(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::InvalidInput, message)
    }

    pub fn not_found(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::NotFound, message)
    }

    pub fn internal(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::Internal, message)
    }

    /// Render as a single-line JSON error envelope:
    /// `{"error":{"kind":"connection","message":"..."}}`.
    /// Guaranteed to contain no ANSI escapes and no embedded newline.
    pub fn to_json(&self) -> String {
        #[derive(Serialize)]
        struct Envelope<'a> {
            error: Body<'a>,
        }
        #[derive(Serialize)]
        struct Body<'a> {
            kind: ErrorKind,
            message: &'a str,
        }
        // Collapse any newlines so the whole error stays on one line.
        let flat = self.message.replace('\n', " ");
        serde_json::to_string(&Envelope {
            error: Body {
                kind: self.kind,
                message: &flat,
            },
        })
        // Serialization of a plain struct of strings cannot fail; fall back to
        // a hand-built object if it somehow does.
        .unwrap_or_else(|_| {
            format!(
                "{{\"error\":{{\"kind\":\"internal\",\"message\":\"{}\"}}}}",
                flat.replace('"', "'")
            )
        })
    }
}

impl fmt::Display for ZemonError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for ZemonError {}

impl From<color_eyre::Report> for ZemonError {
    fn from(report: color_eyre::Report) -> Self {
        ZemonError::internal(report.to_string())
    }
}

impl From<serde_json::Error> for ZemonError {
    fn from(err: serde_json::Error) -> Self {
        ZemonError::internal(err.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kind_serializes_snake_case() {
        let cases = [
            (ErrorKind::Connection, "\"connection\""),
            (ErrorKind::Timeout, "\"timeout\""),
            (ErrorKind::InvalidInput, "\"invalid_input\""),
            (ErrorKind::NotFound, "\"not_found\""),
            (ErrorKind::Internal, "\"internal\""),
        ];
        for (kind, expected) in cases {
            assert_eq!(serde_json::to_string(&kind).unwrap(), expected);
        }
    }

    #[test]
    fn to_json_exact_shape() {
        let e = ZemonError::connection("boom");
        assert_eq!(
            e.to_json(),
            r#"{"error":{"kind":"connection","message":"boom"}}"#
        );
    }

    #[test]
    fn to_json_has_no_ansi_or_newline() {
        let e = ZemonError::internal("line one\nline two\u{1b}[0m");
        let json = e.to_json();
        assert!(!json.contains('\u{1b}'), "must not contain ESC: {json}");
        assert!(!json.contains('\n'), "must be single line: {json}");
        // Still valid JSON.
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(v["error"]["kind"], "internal");
    }

    #[test]
    fn from_report_is_internal() {
        let report = color_eyre::eyre::eyre!("some failure");
        let e: ZemonError = report.into();
        assert_eq!(e.kind, ErrorKind::Internal);
        assert!(e.message.contains("some failure"));
    }

    #[test]
    fn from_serde_json_error_is_internal() {
        let err = serde_json::from_str::<serde_json::Value>("{not json").unwrap_err();
        let e: ZemonError = err.into();
        assert_eq!(e.kind, ErrorKind::Internal);
    }
}
