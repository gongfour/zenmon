//! Shared JSON output shapes for the CLI.
//!
//! Finite collection queries (`discover`, `query`, `nodes`, `liveliness`,
//! `scout`, `info`) render a common envelope so agents can parse every command
//! the same way and never confuse "queried successfully, zero results" with a
//! connection failure (which is a structured error on stderr instead).

use serde::Serialize;

/// Render a finite collection as a compact `{"count":N,"items":[...]}` JSON
/// document. Invariant: `count == items.len()`. An empty slice renders exactly
/// `{"count":0,"items":[]}`.
pub fn to_collection_json<T: Serialize>(items: &[T]) -> Result<String, serde_json::Error> {
    #[derive(Serialize)]
    struct Envelope<'a, T> {
        count: usize,
        items: &'a [T],
    }
    serde_json::to_string(&Envelope {
        count: items.len(),
        items,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_is_canonical() {
        let empty: &[u32] = &[];
        assert_eq!(to_collection_json(empty).unwrap(), r#"{"count":0,"items":[]}"#);
    }

    #[test]
    fn count_matches_len() {
        let json = to_collection_json(&[1, 2, 3]).unwrap();
        assert_eq!(json, r#"{"count":3,"items":[1,2,3]}"#);
    }

    #[test]
    fn count_equals_items_len_invariant() {
        let items = vec!["a", "b"];
        let json = to_collection_json(&items).unwrap();
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(v["count"].as_u64().unwrap() as usize, v["items"].as_array().unwrap().len());
    }

    #[test]
    fn single_item_wraps_in_array() {
        // `info` renders one resource as a one-element collection.
        let json = to_collection_json(std::slice::from_ref(&"only")).unwrap();
        assert_eq!(json, r#"{"count":1,"items":["only"]}"#);
    }
}
