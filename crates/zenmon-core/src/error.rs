//! The single error type of the `zenmon-core` public API.
//!
//! `zenmon-core` is a library: it is linked by zenmon's own CLI and TUI, and by
//! unrelated applications. Application-level reporting crates (`color_eyre`,
//! `anyhow`, ...) therefore never appear in its signatures — a library that
//! returns `color_eyre::Report` forces every consumer to adopt `color_eyre`.
//!
//! Every fallible public function in this crate returns [`ZenmonError`], a
//! `thiserror`-derived error carrying a stable, machine-readable [`ErrorKind`].
//! Consumers can either match on the kind, or convert into whatever report type
//! they already use: `ZenmonError` implements
//! `std::error::Error + Send + Sync + 'static`, so `?` into `anyhow::Error`,
//! `eyre::Report` or `Box<dyn Error>` works with no glue.

use serde::Serialize;
use thiserror::Error;

/// Convenience alias for results carrying a [`ZenmonError`].
pub type Result<T, E = ZenmonError> = std::result::Result<T, E>;

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
#[derive(Debug, Clone, Error)]
#[error("{message}")]
pub struct ZenmonError {
    pub kind: ErrorKind,
    pub message: String,
}

impl ZenmonError {
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

    /// Stable process exit code for this error's kind. `0` is reserved for
    /// success (including successful zero-result queries), so every kind maps
    /// to a distinct non-zero code that agents/shells can branch on.
    pub fn exit_code(&self) -> i32 {
        match self.kind {
            ErrorKind::Internal => 1,
            ErrorKind::InvalidInput => 2,
            ErrorKind::Connection => 3,
            ErrorKind::Timeout => 4,
            ErrorKind::NotFound => 5,
        }
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

/// Zenoh's own error type (`Box<dyn Error + Send + Sync>`) carries no category,
/// so it maps to [`ErrorKind::Internal`]. Call sites that *know* the failure is
/// a connection or input problem should build the error explicitly instead of
/// relying on `?`.
impl From<zenoh::Error> for ZenmonError {
    fn from(err: zenoh::Error) -> Self {
        ZenmonError::internal(err.to_string())
    }
}

impl From<serde_json::Error> for ZenmonError {
    fn from(err: serde_json::Error) -> Self {
        ZenmonError::internal(err.to_string())
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
        let e = ZenmonError::connection("boom");
        assert_eq!(
            e.to_json(),
            r#"{"error":{"kind":"connection","message":"boom"}}"#
        );
    }

    #[test]
    fn to_json_has_no_ansi_or_newline() {
        let e = ZenmonError::internal("line one\nline two\u{1b}[0m");
        let json = e.to_json();
        assert!(!json.contains('\u{1b}'), "must not contain ESC: {json}");
        assert!(!json.contains('\n'), "must be single line: {json}");
        // Still valid JSON.
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(v["error"]["kind"], "internal");
    }

    #[test]
    fn from_zenoh_error_is_internal() {
        let boxed: zenoh::Error = "some failure".into();
        let e: ZenmonError = boxed.into();
        assert_eq!(e.kind, ErrorKind::Internal);
        assert!(e.message.contains("some failure"));
    }

    /// A library error must be usable as a `std::error::Error` by consumers that
    /// erase it into their own report type (`anyhow`, `eyre`, `Box<dyn Error>`).
    #[test]
    fn is_a_send_sync_std_error() {
        fn assert_std_error<E: std::error::Error + Send + Sync + 'static>(_: &E) {}
        let e = ZenmonError::timeout("deadline");
        assert_std_error(&e);
        let boxed: Box<dyn std::error::Error + Send + Sync> = Box::new(e);
        assert_eq!(boxed.to_string(), "deadline");
    }

    #[test]
    fn exit_codes_are_stable_per_kind() {
        assert_eq!(ZenmonError::internal("x").exit_code(), 1);
        assert_eq!(ZenmonError::invalid_input("x").exit_code(), 2);
        assert_eq!(ZenmonError::connection("x").exit_code(), 3);
        assert_eq!(ZenmonError::timeout("x").exit_code(), 4);
        assert_eq!(ZenmonError::not_found("x").exit_code(), 5);
    }

    #[test]
    fn from_serde_json_error_is_internal() {
        let err = serde_json::from_str::<serde_json::Value>("{not json").unwrap_err();
        let e: ZenmonError = err.into();
        assert_eq!(e.kind, ErrorKind::Internal);
    }
}
