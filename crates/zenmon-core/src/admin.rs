//! Reading the Zenoh admin space (`@/**`): what each router knows about the
//! network, and in particular who declared what.
//!
//! A router lists every subscriber, publisher, queryable and liveliness token
//! routed through it under `@/<zid>/router/<kind>/<key expr>`, with the
//! sessions that declared it. That answers questions traffic alone cannot:
//! *is anything subscribed to this key at all?* — the first thing to know
//! when messages "don't arrive".
//!
//! Requires routers with the admin space enabled (zenohd's default for
//! reads). A peer-only network has no router to ask.

use std::time::Duration;

use serde::Serialize;
use serde_json::Value;
use zenoh::key_expr::keyexpr;
use zenoh::Session;

use crate::error::ZenmonError;
use crate::query::{self, ConsolidationMode};

/// Kinds of declaration a router reports.
pub const DECLARATION_KINDS: &[&str] = &["subscriber", "publisher", "queryable", "token"];

/// One declaration, as one router reports it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Declaration {
    /// `subscriber`, `publisher`, `queryable` or `token`.
    pub kind: String,
    /// The declared key expression.
    pub key_expr: String,
    /// The router reporting it.
    pub router: String,
    /// Sessions that declared it, by how they connect to that router.
    pub clients: Vec<String>,
    pub peers: Vec<String>,
    /// Other routers it was propagated from.
    pub routers: Vec<String>,
}

/// Split an admin key `@/<zid>/<whatami>/<kind>/<key expr…>`.
fn parse_declaration_key(admin_key: &str) -> Option<(String, String, String)> {
    let rest = admin_key.strip_prefix("@/")?;
    let mut parts = rest.splitn(4, '/');
    let zid = parts.next()?;
    let _whatami = parts.next()?;
    let kind = parts.next()?;
    let key = parts.next()?;
    DECLARATION_KINDS
        .contains(&kind)
        .then(|| (zid.to_string(), kind.to_string(), key.to_string()))
}

fn zids(v: &Value, field: &str) -> Vec<String> {
    v.get(field)
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|x| x.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default()
}

/// Parse admin-space replies into declarations whose key intersects `filter`
/// (all of them for `**`), of the requested kinds, sorted by key then kind.
pub fn declarations_from_replies(
    replies: &[(String, Value)],
    filter: &str,
    kinds: &[&str],
) -> Result<Vec<Declaration>, ZenmonError> {
    let filter = keyexpr::new(filter).map_err(|e| {
        ZenmonError::invalid_input(format!("invalid key expression '{filter}': {e}"))
    })?;
    let mut out: Vec<Declaration> = replies
        .iter()
        .filter_map(|(admin_key, payload)| {
            let (router, kind, key) = parse_declaration_key(admin_key)?;
            if !kinds.contains(&kind.as_str()) {
                return None;
            }
            // A declared key that is not a valid expression on its own cannot
            // be matched; keep it visible only when listing everything.
            let matches = keyexpr::new(key.as_str())
                .map(|k| k.intersects(filter))
                .unwrap_or(filter.as_str() == "**");
            matches.then(|| Declaration {
                kind,
                key_expr: key,
                router,
                clients: zids(payload, "clients"),
                peers: zids(payload, "peers"),
                routers: zids(payload, "routers"),
            })
        })
        .collect();
    out.sort_by(|a, b| {
        (a.key_expr.as_str(), a.kind.as_str(), a.router.as_str()).cmp(&(
            b.key_expr.as_str(),
            b.kind.as_str(),
            b.router.as_str(),
        ))
    });
    Ok(out)
}

/// Ask every reachable router which declarations intersect `filter`.
pub async fn declarations(
    session: &Session,
    filter: &str,
    kinds: &[&str],
    timeout: Duration,
) -> Result<Vec<Declaration>, ZenmonError> {
    let mut replies = Vec::new();
    for kind in kinds {
        let outcome = query::get(
            session,
            &format!("@/*/*/{kind}/**"),
            None,
            timeout,
            None,
            ConsolidationMode::None,
        )
        .await?;
        replies.extend(
            outcome
                .replies
                .into_iter()
                .map(|m| (m.key_expr, m.payload.to_view())),
        );
    }
    declarations_from_replies(&replies, filter, kinds)
}

/// Refuse a selector outside the admin space.
pub fn check_selector(selector: &str) -> Result<(), ZenmonError> {
    if selector.starts_with("@/") {
        Ok(())
    } else {
        Err(ZenmonError::invalid_input(format!(
            "'{selector}' is not in the admin space (it starts with @/)"
        )))
    }
}

/// Raw admin-space entries under `selector`, as `(key, decoded payload)`.
pub async fn entries(
    session: &Session,
    selector: &str,
    timeout: Duration,
) -> Result<Vec<(String, Value)>, ZenmonError> {
    check_selector(selector)?;
    let outcome = query::get(
        session,
        selector,
        None,
        timeout,
        None,
        ConsolidationMode::None,
    )
    .await?;
    let mut out: Vec<(String, Value)> = outcome
        .replies
        .into_iter()
        .map(|m| (m.key_expr, m.payload.to_view()))
        .collect();
    out.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn replies() -> Vec<(String, Value)> {
        let decl = |c: &[&str]| json!({ "clients": c, "peers": [], "routers": ["r1"] });
        vec![
            ("@/r1/router/subscriber/demo/**".into(), decl(&["c1"])),
            (
                "@/r1/router/publisher/demo/device1/pose".into(),
                decl(&["c2"]),
            ),
            (
                "@/r1/router/queryable/demo/device1/params/get".into(),
                decl(&["c3"]),
            ),
            ("@/r1/router/token/demo/device1/alive".into(), decl(&["c2"])),
            ("@/r1/router/subscriber/other/**".into(), decl(&["c4"])),
            ("@/r1/router".into(), json!({ "sessions": [] })),
            ("@/r1/router/linkstate/routers".into(), json!("graph {}")),
        ]
    }

    #[test]
    fn only_declarations_that_intersect_the_filter_are_kept() {
        let d =
            declarations_from_replies(&replies(), "demo/device1/pose", DECLARATION_KINDS).unwrap();
        let got: Vec<(&str, &str)> = d
            .iter()
            .map(|d| (d.kind.as_str(), d.key_expr.as_str()))
            .collect();
        assert_eq!(
            got,
            [
                ("subscriber", "demo/**"),
                ("publisher", "demo/device1/pose")
            ],
            "the subscriber on demo/** receives demo/device1/pose"
        );
        assert_eq!(d[0].clients, ["c1"]);
        assert_eq!(d[0].router, "r1");
    }

    #[test]
    fn kinds_can_be_narrowed_and_non_declarations_are_ignored() {
        let d = declarations_from_replies(&replies(), "**", &["queryable"]).unwrap();
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].key_expr, "demo/device1/params/get");
        assert_eq!(
            declarations_from_replies(&replies(), "**", DECLARATION_KINDS)
                .unwrap()
                .len(),
            5
        );
    }

    #[test]
    fn an_invalid_filter_is_a_usage_error() {
        assert!(declarations_from_replies(&replies(), "a/#", DECLARATION_KINDS).is_err());
    }
}
