//! Pure key-expression relationship testing (no session required).
//!
//! Uses Zenoh's own `keyexpr::intersects()` and directional
//! `keyexpr::includes()` so agents can reason about key-expression overlap
//! deterministically and offline.

use crate::error::ZenmonError;
use serde::Serialize;
use zenoh::key_expr::keyexpr;

/// The single, direction-explicit relationship between two key expressions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Relation {
    /// A and B match exactly the same set of keys.
    Equal,
    /// A contains every key of B (A ⊇ B), and they are not equal.
    AIncludesB,
    /// B contains every key of A (B ⊇ A), and they are not equal.
    BIncludesA,
    /// They share at least one key but neither includes the other.
    Overlaps,
    /// They share no keys.
    Disjoint,
}

/// The full relationship between two key expressions.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct KeyExprRelation {
    pub a: String,
    pub b: String,
    /// Whether A and B share at least one matching key.
    pub intersects: bool,
    /// Whether A contains every key of B (i.e. A ⊇ B).
    pub a_includes_b: bool,
    /// Whether B contains every key of A (i.e. B ⊇ A).
    pub b_includes_a: bool,
    /// Whether A and B match exactly the same set of keys.
    pub equal: bool,
    /// The single, direction-explicit relationship.
    pub relation: Relation,
}

/// Compare two key expressions. Returns an `invalid_input` error if either is
/// not a valid, canonical key expression.
pub fn compare(a: &str, b: &str) -> Result<KeyExprRelation, ZenmonError> {
    let ka = parse(a)?;
    let kb = parse(b)?;

    let intersects = ka.intersects(kb);
    let a_includes_b = ka.includes(kb);
    let b_includes_a = kb.includes(ka);
    let equal = a_includes_b && b_includes_a;

    let relation = if equal {
        Relation::Equal
    } else if a_includes_b {
        Relation::AIncludesB
    } else if b_includes_a {
        Relation::BIncludesA
    } else if intersects {
        Relation::Overlaps
    } else {
        Relation::Disjoint
    };

    Ok(KeyExprRelation {
        a: a.to_string(),
        b: b.to_string(),
        intersects,
        a_includes_b,
        b_includes_a,
        equal,
        relation,
    })
}

fn parse(s: &str) -> Result<&keyexpr, ZenmonError> {
    keyexpr::new(s)
        .map_err(|e| ZenmonError::invalid_input(format!("invalid key expression '{}': {}", s, e)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_match_is_equal() {
        let r = compare("a/b", "a/b").unwrap();
        assert!(r.intersects && r.a_includes_b && r.b_includes_a && r.equal);
        assert_eq!(r.relation, Relation::Equal);
    }

    #[test]
    fn single_star_includes_concrete() {
        let r = compare("a/*", "a/b").unwrap();
        assert!(r.intersects);
        assert!(r.a_includes_b);
        assert!(!r.b_includes_a);
        assert!(!r.equal);
        assert_eq!(r.relation, Relation::AIncludesB);
    }

    #[test]
    fn double_star_includes_deep() {
        let r = compare("a/**", "a/b/c").unwrap();
        assert!(r.a_includes_b);
        assert_eq!(r.relation, Relation::AIncludesB);
    }

    #[test]
    fn reversed_inclusion() {
        let r = compare("a/b", "a/*").unwrap();
        assert!(r.b_includes_a);
        assert!(!r.a_includes_b);
        assert_eq!(r.relation, Relation::BIncludesA);
    }

    #[test]
    fn partial_overlap_neither_includes() {
        let r = compare("a/*/c", "a/b/*").unwrap();
        assert!(r.intersects);
        assert!(!r.a_includes_b);
        assert!(!r.b_includes_a);
        assert_eq!(r.relation, Relation::Overlaps);
    }

    #[test]
    fn disjoint_keys() {
        let r = compare("a/b", "x/y").unwrap();
        assert!(!r.intersects);
        assert_eq!(r.relation, Relation::Disjoint);
    }

    #[test]
    fn invalid_key_expr_is_input_error() {
        let err = compare("a//b", "x").unwrap_err();
        assert_eq!(err.kind, crate::error::ErrorKind::InvalidInput);
    }
}
