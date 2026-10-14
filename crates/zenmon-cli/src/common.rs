//! Helpers shared by several commands: error flattening, contract loading,
//! namespace warnings, payload arguments, and stdout writing.

use std::path::PathBuf;

use zenmon_core::contract::{Contract, Enrichment};
use zenmon_core::error::ZenmonError;

/// Flatten an untyped failure (Zenoh's `Box<dyn Error>`, the TUI's
/// `eyre::Report`, ...) into the CLI's error type.
///
/// `zenmon-core` no longer exposes `From<color_eyre::Report> for ZenmonError` —
/// a library must not put an application reporting crate in its public API — so
/// the flattening lives here, in the application. The mapping is unchanged:
/// untyped failures are `internal` (exit code 1).
pub(crate) fn internal_err(e: impl std::fmt::Display) -> ZenmonError {
    ZenmonError::internal(e.to_string())
}

/// Resolve a contract path: explicit arg → `--contract` flag → `ZENMON_CONTRACT`.
pub(crate) fn contract_path(
    explicit: Option<PathBuf>,
    cli_flag: &Option<PathBuf>,
) -> Option<PathBuf> {
    explicit
        .or_else(|| cli_flag.clone())
        .or_else(|| std::env::var_os("ZENMON_CONTRACT").map(PathBuf::from))
}

/// Load the contract for enrichment (`sub`/`discover`): `None` when no path is
/// configured, hard error when a configured path fails to load.
pub(crate) fn load_contract_opt(
    cli_flag: &Option<PathBuf>,
) -> Result<Option<Contract>, ZenmonError> {
    match contract_path(None, cli_flag) {
        Some(p) => Contract::load(p).map(Some),
        None => Ok(None),
    }
}

/// Load the contract for the `contract` subcommand, where one is required.
pub(crate) fn load_contract_required(
    explicit: Option<PathBuf>,
    cli_flag: &Option<PathBuf>,
) -> Result<Contract, ZenmonError> {
    match contract_path(explicit, cli_flag) {
        Some(p) => Contract::load(p),
        None => Err(ZenmonError::invalid_input(
            "no contract file: pass a path, use --contract, or set ZENMON_CONTRACT",
        )),
    }
}

/// One-line human annotation for an enriched `sub` message.
pub(crate) fn contract_annotation(e: &Enrichment, observed_encoding: &str) -> String {
    if !e.declared {
        return "# ⚠ not declared in contract".to_string();
    }
    let mut s = format!("# {}", e.matched_key.as_deref().unwrap_or_default());
    if let Some(d) = &e.description {
        s.push_str(" — ");
        s.push_str(d);
    }
    match e.encoding_matches {
        Some(true) => {
            s.push_str(&format!(
                "  [encoding: {} ok]",
                e.encoding_expected.as_deref().unwrap_or_default()
            ));
        }
        Some(false) => {
            s.push_str(&format!(
                "  [encoding: expected {}, got {}]",
                e.encoding_expected.as_deref().unwrap_or_default(),
                observed_encoding
            ));
        }
        None => {}
    }
    s
}

/// If `namespace` is set and `key` already begins with it (on a segment
/// boundary), return a warning: under `-n` the key is prefixed *again*, so it
/// silently matches nothing — an easy, hard-to-debug mistake.
pub(crate) fn namespace_redundant_prefix(key: &str, namespace: Option<&str>) -> Option<String> {
    let ns = namespace?.trim_matches('/');
    if ns.is_empty() {
        return None;
    }
    let redundant = key == ns
        || key
            .strip_prefix(ns)
            .is_some_and(|rest| rest.starts_with('/'));
    redundant.then(|| {
        format!(
            "# note: key '{key}' already starts with namespace '{ns}' — under -n it is prefixed \
             again and will match nothing (drop the namespace from the key)"
        )
    })
}

/// Emit the redundant-namespace warning for each key, if any.
pub(crate) fn warn_redundant_namespace<'a>(
    keys: impl IntoIterator<Item = &'a str>,
    namespace: Option<&str>,
) {
    for k in keys {
        if let Some(w) = namespace_redundant_prefix(k, namespace) {
            eprintln!("{w}");
        }
    }
}

/// Resolve a publish/request value argument: `@<path>` reads the value from that
/// file, `-` reads it from stdin, and anything else is the literal value. Lets
/// large or dynamically-built payloads (e.g. a VDA5050 job for `--task`) come
/// from a file instead of an unwieldy inline CLI string.
pub(crate) fn resolve_payload_arg(s: &str) -> Result<String, ZenmonError> {
    let bytes = resolve_payload_arg_bytes(s)?;
    String::from_utf8(bytes)
        .map_err(|_| ZenmonError::invalid_input(format!("value from '{s}' is not valid UTF-8")))
}

/// Byte-oriented variant of [`resolve_payload_arg`] for payloads that may be
/// binary, e.g. a queryable reply served from a file.
pub(crate) fn resolve_payload_arg_bytes(s: &str) -> Result<Vec<u8>, ZenmonError> {
    if s == "-" {
        use std::io::Read;
        let mut buf = Vec::new();
        std::io::stdin()
            .read_to_end(&mut buf)
            .map_err(|e| ZenmonError::invalid_input(format!("reading value from stdin: {e}")))?;
        Ok(buf)
    } else if let Some(path) = s.strip_prefix('@') {
        std::fs::read(path)
            .map_err(|e| ZenmonError::invalid_input(format!("reading value from '{path}': {e}")))
    } else {
        Ok(s.as_bytes().to_vec())
    }
}

/// Write to stdout, flushing when asked. `Ok(false)` means the reader closed
/// the pipe — an ordinary way for `sub | head` to end, not an error.
pub(crate) fn write_out(
    out: &mut impl std::io::Write,
    text: &str,
    flush: bool,
) -> Result<bool, ZenmonError> {
    let result = out
        .write_all(text.as_bytes())
        .and_then(|()| if flush { out.flush() } else { Ok(()) });
    match result {
        Ok(()) => Ok(true),
        Err(e) if e.kind() == std::io::ErrorKind::BrokenPipe => Ok(false),
        Err(e) => Err(ZenmonError::internal(format!("cannot write output: {e}"))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct ClosedPipe;
    impl std::io::Write for ClosedPipe {
        fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
            Err(std::io::ErrorKind::BrokenPipe.into())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn a_closed_pipe_ends_output_quietly() {
        assert!(!write_out(&mut ClosedPipe, "x\n", true).unwrap());
        let mut buf = Vec::new();
        assert!(write_out(&mut buf, "x\n", false).unwrap());
        assert_eq!(buf, b"x\n");
    }
    fn enr(declared: bool, matches: Option<bool>) -> Enrichment {
        Enrichment {
            declared,
            matched_key: declared.then(|| "topic/x".to_string()),
            description: declared.then(|| "desc".to_string()),
            encoding_expected: declared.then(|| "application/json".to_string()),
            encoding_matches: matches,
            enveloped: declared.then_some(true),
        }
    }

    #[test]
    fn resolve_payload_literal_returned_as_is() {
        assert_eq!(resolve_payload_arg("{\"a\":1}").unwrap(), "{\"a\":1}");
    }

    #[test]
    fn resolve_payload_at_file_reads_contents() {
        let path = std::env::temp_dir().join("zenmon_resolve_payload_test.json");
        std::fs::write(&path, "{\"from\":\"file\"}").unwrap();
        let arg = format!("@{}", path.display());
        assert_eq!(resolve_payload_arg(&arg).unwrap(), "{\"from\":\"file\"}");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn resolve_payload_missing_file_is_error() {
        assert!(resolve_payload_arg("@/no/such/zenmon/payload/file.json").is_err());
    }

    #[test]
    fn resolve_payload_bytes_literal_returned_as_is() {
        assert_eq!(
            resolve_payload_arg_bytes("{\"a\":1}").unwrap(),
            b"{\"a\":1}".to_vec()
        );
    }

    #[test]
    fn resolve_payload_bytes_at_file_reads_raw_bytes() {
        let path = std::env::temp_dir().join("zenmon_resolve_payload_bytes_test.bin");
        let data: &[u8] = &[0xff, 0x00, 0x88, 0x01];
        std::fs::write(&path, data).unwrap();
        let arg = format!("@{}", path.display());
        assert_eq!(resolve_payload_arg_bytes(&arg).unwrap(), data.to_vec());
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn resolve_payload_bytes_missing_file_is_error() {
        assert!(resolve_payload_arg_bytes("@/no/such/zenmon/payload/file.bin").is_err());
    }

    #[test]
    fn namespace_warns_when_key_already_prefixed() {
        let w = namespace_redundant_prefix("demo/topic/x", Some("demo"));
        assert!(w.is_some_and(|m| m.contains("already starts with namespace")));
        // Exact-equal to the namespace also warns.
        assert!(namespace_redundant_prefix("demo", Some("demo")).is_some());
    }

    #[test]
    fn namespace_no_warn_for_relative_or_absent() {
        assert!(namespace_redundant_prefix("topic/x", Some("demo")).is_none());
        assert!(namespace_redundant_prefix("demo/topic/x", None).is_none());
    }

    #[test]
    fn namespace_no_false_positive_on_partial_segment() {
        // "demox" must not be treated as under namespace "demo".
        assert!(namespace_redundant_prefix("demox/topic", Some("demo")).is_none());
    }

    #[test]
    fn annotation_flags_undeclared_topic() {
        let s = contract_annotation(&enr(false, None), "application/json");
        assert_eq!(s, "# ⚠ not declared in contract");
    }

    #[test]
    fn annotation_reports_matching_encoding() {
        let s = contract_annotation(&enr(true, Some(true)), "application/json");
        assert_eq!(s, "# topic/x — desc  [encoding: application/json ok]");
    }

    #[test]
    fn annotation_reports_encoding_mismatch_with_observed() {
        let s = contract_annotation(&enr(true, Some(false)), "application/octet-stream");
        assert_eq!(
            s,
            "# topic/x — desc  [encoding: expected application/json, got application/octet-stream]"
        );
    }

    #[test]
    fn annotation_omits_encoding_when_unknown() {
        // Discover has no payload encoding, so encoding_matches is None.
        let s = contract_annotation(&enr(true, None), "");
        assert_eq!(s, "# topic/x — desc");
    }
}
