//! Helpers for the test `queryable serve` command.

use crate::error::ZenmonError;
use zenoh::key_expr::keyexpr;

/// Resolve the concrete key a reply must use.
///
/// Zenoh requires the reply key to be concrete (non-wildcard) and to match the
/// query. For a non-wildcard queryable we can reuse its key; for a wildcard
/// queryable the caller must supply `--reply-key <concrete-key>` (a wildcard
/// queryable's own key is not a valid reply key).
pub fn resolve_reply_key(key_expr: &str, reply_key: Option<&str>) -> Result<String, ZenmonError> {
    let ke = keyexpr::new(key_expr)
        .map_err(|e| ZenmonError::invalid_input(format!("invalid key expression '{}': {}", key_expr, e)))?;

    match reply_key {
        Some(rk) => {
            let rke = keyexpr::new(rk).map_err(|e| {
                ZenmonError::invalid_input(format!("invalid --reply-key '{}': {}", rk, e))
            })?;
            if rke.is_wild() {
                return Err(ZenmonError::invalid_input(format!(
                    "--reply-key '{}' must be a concrete (non-wildcard) key",
                    rk
                )));
            }
            Ok(rk.to_string())
        }
        None => {
            if ke.is_wild() {
                Err(ZenmonError::invalid_input(format!(
                    "queryable '{}' is a wildcard; pass --reply-key <concrete-key>",
                    key_expr
                )))
            } else {
                Ok(key_expr.to_string())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::ErrorKind;

    #[test]
    fn non_wild_key_reuses_itself() {
        assert_eq!(resolve_reply_key("robot/status", None).unwrap(), "robot/status");
    }

    #[test]
    fn wild_key_without_reply_key_errors() {
        let e = resolve_reply_key("robot/**", None).unwrap_err();
        assert_eq!(e.kind, ErrorKind::InvalidInput);
    }

    #[test]
    fn wild_key_with_concrete_reply_key_ok() {
        assert_eq!(
            resolve_reply_key("robot/**", Some("robot/status")).unwrap(),
            "robot/status"
        );
    }

    #[test]
    fn wild_reply_key_rejected() {
        let e = resolve_reply_key("robot/**", Some("robot/*")).unwrap_err();
        assert_eq!(e.kind, ErrorKind::InvalidInput);
    }

    #[test]
    fn invalid_syntax_rejected() {
        assert!(resolve_reply_key("a//b", None).is_err());
    }
}
